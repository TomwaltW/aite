"""FakePlatform —— PlatformPort 的官方替身（dev-spec §3.2）。

记账是它的主业：`send_card` / `update_card` / `send_text` / `send_file` /
`add_reaction` 的调用次数与参数全进 CallLog，§3.8 的场景断言都靠它。

三条刻意做严的地方：

* `update_card` 只认已经存在的 card_id。发第二条卡片再更新、或者拿消息 id
  当卡片 id 传进来，都会直接抛错 —— §3.8 的 03 要验的正是「原地更新，不新发消息」，
  这种错必须在替身这一层就炸掉，而不是等断言绕着弯地发现。
* `read_history` **不做** sender_kind 过滤（契约注释写死：过滤归 Gateway 的
  read_group_history）。05_history_summary 靠这条才验得出 Gateway 有没有过滤。
* 出站一律要 chat_id / message_id 对得上，对不上就抛 FakePlatformError。
"""
from __future__ import annotations

import itertools
from collections.abc import Iterable
from datetime import UTC, datetime

from ..contracts import (
    ChecklistCard,
    NormalizedEvent,
    OutboundFile,
    OutboundText,
    PlatformCapabilities,
    ReactionKind,
    SendResult,
)
from ..contracts.ports import DocumentContent, EventHandler, HistoryMessage
from .recorder import CallLog

#: fake 平台的能力位。除了「限速给大」以外与 FEISHU_P0 一致，
#: supports_passive_listen=True 是因为事件由测试直接注入，不存在「收不到」这回事。
FAKE_P0 = PlatformCapabilities(
    platform="fake",
    supports_thread=True,
    supports_history=True,
    supports_passive_listen=True,
    supports_card_edit=True,
    card_edit_window_sec=1209600,
    inbound_file_in_group=True,
    proactive_requires_prior_message=False,
    outbound_rate_per_min=6000,
)


class FakePlatformError(RuntimeError):
    """替身自己判定「这个调用姿势不对」时抛的错，用来把违规钉在现场。"""


class FakePlatform:
    """PlatformPort 的替身。构造后可直接改 history / documents / files 三个字段喂数据。"""

    capabilities: PlatformCapabilities = FAKE_P0

    def __init__(
        self,
        *,
        history: Iterable[HistoryMessage] = (),
        documents: dict[str, DocumentContent] | None = None,
        files: dict[tuple[str, str], bytes] | None = None,
    ) -> None:
        self.calls = CallLog()
        self.history: list[HistoryMessage] = list(history)
        self.documents: dict[str, DocumentContent] = dict(documents or {})
        #: (message_id, file_key) -> 文件内容
        self.files: dict[tuple[str, str], bytes] = dict(files or {})

        self.sent_texts: list[OutboundText] = []
        self.sent_files: list[OutboundFile] = []
        self.reactions: list[tuple[str, ReactionKind]] = []
        #: card_id -> 该卡片的全部快照（[0] 是 send_card 那次，其后每次 update_card 追加一份）
        self.cards: dict[str, list[ChecklistCard]] = {}

        #: 方法名 -> 下一次调用要抛的异常（抛完即清），用来演 §3.3 的失败面
        self.fail_next: dict[str, Exception] = {}

        self.started = False
        self.stopped = False
        self._on_event: EventHandler | None = None
        self._ids = itertools.count(1)

    # ---- 内部小工具 ----------------------------------------------------

    def _next_id(self, prefix: str) -> str:
        return f"{prefix}-{next(self._ids)}"

    def _maybe_fail(self, method: str) -> None:
        exc = self.fail_next.pop(method, None)
        if exc is not None:
            raise exc

    # ---- 事件注入（测试侧用，不属于 PlatformPort）------------------------

    async def emit(self, ev: NormalizedEvent) -> None:
        """把一个归一化事件投给已注册的 on_event —— 相当于平台推了一条消息过来。"""
        if self._on_event is None:
            raise FakePlatformError("还没有 start()，没有 on_event 可以投递")
        self.calls.record("emit", event_id=ev.event_id, kind=str(ev.kind))
        await self._on_event(ev)

    # ---- PlatformPort ---------------------------------------------------

    async def start(self, on_event: EventHandler) -> None:
        self.calls.record("start")
        self._on_event = on_event
        self.started = True

    async def stop(self) -> None:
        self.calls.record("stop")
        self.stopped = True

    async def send_text(self, msg: OutboundText) -> SendResult:
        call = self.calls.record(
            "send_text", chat_id=msg.chat_id, reply_to=msg.reply_to, in_thread=msg.in_thread, text=msg.text
        )
        self._maybe_fail("send_text")
        self.sent_texts.append(msg)
        result = SendResult(message_id=self._next_id("msg"))
        call.result = result
        return result

    async def send_card(self, chat_id: str, reply_to: str | None, card: ChecklistCard) -> SendResult:
        call = self.calls.record(
            "send_card", chat_id=chat_id, reply_to=reply_to, task_id=card.task_id, status=card.status
        )
        self._maybe_fail("send_card")
        card_id = self._next_id("card")
        self.cards[card_id] = [card.model_copy(deep=True)]
        result = SendResult(message_id=card_id, card_id=card_id)
        call.result = result
        return result

    async def update_card(self, card_id: str, card: ChecklistCard) -> None:
        call = self.calls.record("update_card", card_id=card_id, task_id=card.task_id, status=card.status)
        if card_id not in self.cards:
            call.error = "unknown card_id"
            raise FakePlatformError(
                f"update_card 的 card_id={card_id!r} 不存在。"
                f"已发出的卡片：{sorted(self.cards)}。卡片必须原地更新（§3.2），不能新发消息。"
            )
        self._maybe_fail("update_card")
        self.cards[card_id].append(card.model_copy(deep=True))

    async def send_file(self, msg: OutboundFile) -> SendResult:
        call = self.calls.record(
            "send_file", chat_id=msg.chat_id, reply_to=msg.reply_to, name=msg.name,
            mime=msg.mime, size=len(msg.data),
        )
        self._maybe_fail("send_file")
        self.sent_files.append(msg)
        result = SendResult(message_id=self._next_id("file"))
        call.result = result
        return result

    async def add_reaction(self, message_id: str, kind: ReactionKind) -> None:
        self.calls.record("add_reaction", message_id=message_id, kind=kind)
        self._maybe_fail("add_reaction")
        self.reactions.append((message_id, kind))

    async def read_history(
        self, chat_id: str, *, limit: int = 50, thread_id: str | None = None
    ) -> list[HistoryMessage]:
        call = self.calls.record("read_history", chat_id=chat_id, limit=limit, thread_id=thread_id)
        self._maybe_fail("read_history")
        rows = [h for h in self.history if thread_id is None or h.thread_id == thread_id]
        rows.sort(key=lambda h: h.created_at)          # 契约要求时间正序
        rows = rows[-limit:]
        call.result = [h.message_id for h in rows]
        return rows

    async def read_document(self, url_or_token: str) -> DocumentContent:
        call = self.calls.record("read_document", url_or_token=url_or_token)
        self._maybe_fail("read_document")
        doc = self.documents.get(url_or_token)
        if doc is None:
            call.error = "not found"
            raise FakePlatformError(f"没有这篇文档：{url_or_token!r}（已备：{sorted(self.documents)}）")
        return doc

    async def download_file(self, message_id: str, file_key: str) -> bytes:
        call = self.calls.record("download_file", message_id=message_id, file_key=file_key)
        self._maybe_fail("download_file")
        data = self.files.get((message_id, file_key))
        if data is None:
            call.error = "not found"
            raise FakePlatformError(
                f"没有这个附件：message_id={message_id!r} file_key={file_key!r}"
                f"（已备：{sorted(self.files)}）"
            )
        return data

    # ---- 给断言用的便捷视图 ---------------------------------------------

    def count(self, method: str) -> int:
        return self.calls.count(method)

    @property
    def card_count(self) -> int:
        """发出过几张卡片（不是更新了几次）。"""
        return len(self.cards)

    @property
    def update_count(self) -> int:
        return self.calls.count("update_card")

    @property
    def outbound_count(self) -> int:
        """所有出站动作之和 —— 09_bot_ignored 要它等于 0。"""
        return sum(
            self.calls.count(m)
            for m in ("send_text", "send_card", "update_card", "send_file", "add_reaction")
        )

    def texts(self) -> list[str]:
        return [m.text for m in self.sent_texts]

    def card_snapshots(self, card_id: str | None = None) -> list[ChecklistCard]:
        if card_id is not None:
            return list(self.cards.get(card_id, []))
        return [snap for snaps in self.cards.values() for snap in snaps]


def history_message(
    message_id: str,
    text: str,
    *,
    sender_id: str = "ou_someone",
    sender_kind: str = "human",
    sender_name: str | None = None,
    thread_id: str | None = None,
    created_at: datetime | None = None,
) -> HistoryMessage:
    """造一条群历史，字段默认值齐全，场景 yaml 里只写关心的那几个。"""
    return HistoryMessage(
        message_id=message_id,
        sender_id=sender_id,
        sender_kind=sender_kind,
        sender_name=sender_name if sender_name is not None else sender_id,
        text=text,
        thread_id=thread_id,
        created_at=created_at or datetime.now(UTC),
    )
