"""`PlatformPort`（§3.2）的飞书实现。

长连接收事件 → 归一化成 `NormalizedEvent` → 出站（文本 / 卡片 / 文件 / 表情）
→ 群历史与云文档读取。

三条不许走偏的：

* `update_card` 一定是 PATCH 同一条 `message_id`，绝不新发消息（§3.2 注释、W4）。
* adapter **不做去重** —— 去重是 ControlPlane 靠 `event_id` 干的（§3.3 / R2）。
  重连后平台重推的重复事件在这里照单全收往上送。
* `read_history` 按时间正序返回，**不做 sender_kind 过滤** —— 过滤归 T3 的
  `read_group_history` 工具（§3.2 注释、§6 T3）。
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import time
from collections.abc import Awaitable, Callable
from datetime import UTC, datetime
from typing import Any

from ...contracts import (
    FEISHU_P0,
    ChecklistCard,
    DocumentContent,
    EventHandler,
    FeishuConfig,
    HistoryMessage,
    OutboundFile,
    OutboundText,
    PlatformCapabilities,
    ReactionKind,
    SendResult,
)
from . import api as feishu_api
from .api import FeishuApiClient
from .cards import build_checklist_card, build_markdown_card, dumps_card
from .connection import LarkWSConnection, RawEventHandler, WSConnection, backoff_delay
from .errors import PlatformError
from .normalize import extract_text, normalize, sender_kind_of, to_datetime

logger = logging.getLogger(__name__)

#: §3.2：`on_event` 必须在 1s 内返回。超了不拦（拦了会丢事件），但要吼一声。
ON_EVENT_BUDGET_SEC = 1.0

#: 「表情文案说明」里确实存在的 emoji_type，只列本模块可能用到的几个。
#: 测试拿它兜住「别再往 REACTION_EMOJI 里写一个清单外的 key」。
#: https://open.feishu.cn/document/server-docs/im-v1/message-reaction/emojis-introduce
KNOWN_EMOJI_TYPES = frozenset({"OnIt", "DONE", "CRY", "GLANCE", "THUMBSUP", "MUSCLE", "OK"})

#: `ReactionKind` → 飞书表情 key（「添加消息表情回复」的 `reaction_type.emoji_type`）。
#: 取值必须落在官方那份固定清单里，不在清单里的会被打回 `231001 表情类型不合法`：
#: https://open.feishu.cn/document/server-docs/im-v1/message-reaction/create
#:
#: T16 拿清单逐个核过：`DONE` / `CRY` 在清单里；附录 A 时期写的 `EYES` **不在** ——
#: 有 `eyes` 的是云文档高亮块那套小写枚举，跟消息表情回复不是一套，照原样发上去
#: R7 的每一次 ack 都会 400。换成清单里的 `OnIt`，语义正好是「收到，正在处理」。
#: 想要字面 👀 的话清单里还有 `GLANCE`，但文档只给图不给文案，哪个更像只能在
#: 真实群里点一次看；换 key 只改这张表，不牵动任何调用方。
REACTION_EMOJI: dict[str, str] = {"ack": "OnIt", "done": "DONE", "fail": "CRY"}

#: 「上传文件」接口的 file_type 取值，按扩展名挑；认不出就 stream。
_FILE_TYPE_BY_EXT = {
    ".opus": "opus", ".mp4": "mp4", ".pdf": "pdf",
    ".doc": "doc", ".docx": "doc",
    ".xls": "xls", ".xlsx": "xls",
    ".ppt": "ppt", ".pptx": "ppt",
}

#: 一页历史消息最多 50 条（飞书 page_size 上限）。
_HISTORY_PAGE_SIZE = 50

#: 云文档链接里 token 的位置：/docx/<token>、/docs/<token>、/wiki/<token>。
_DOC_URL_RE = re.compile(r"/(docx|docs|wiki)/([A-Za-z0-9]+)")


class FeishuPlatform:
    """PlatformPort（T1）。"""

    capabilities: PlatformCapabilities

    def __init__(
        self,
        *,
        app_id: str = "",
        app_secret: str = "",
        bot_open_id: str = "",
        tenant_id: str = "default",
        domain: str = feishu_api.DEFAULT_DOMAIN,
        history_window: int = 50,
        api: FeishuApiClient | None = None,
        connection_factory: Callable[[RawEventHandler], WSConnection] | None = None,
        sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
        clock: Callable[[], float] = time.monotonic,
        capabilities: PlatformCapabilities | None = None,
    ) -> None:
        self.app_id = app_id
        self.bot_open_id = bot_open_id
        self.tenant_id = tenant_id
        self.history_window = history_window

        # §3.7：supports_passive_listen 的运行时取值要能改，但 FEISHU_P0 是全局共享的
        # 可变单例 —— 就地改它会污染所有引用方。每个实例拿自己的深拷贝。
        self.capabilities = (capabilities or FEISHU_P0).model_copy(deep=True)

        self.api = api or FeishuApiClient(
            app_id=app_id,
            app_secret=app_secret,
            domain=domain,
            rate_per_min=self.capabilities.outbound_rate_per_min,
            sleep=sleep,
            clock=clock,
        )
        self._connection_factory = connection_factory or self._default_connection_factory
        self._sleep = sleep
        self._on_event: EventHandler | None = None
        self._connection: WSConnection | None = None
        self._stopping = False

    @classmethod
    def from_config(
        cls,
        cfg: FeishuConfig,
        *,
        tenant_id: str = "default",
        env: dict[str, str] | None = None,
        **kwargs: Any,
    ) -> FeishuPlatform:
        """按 `FeishuConfig`（§3.1）里记的**环境变量名**取凭证建实例。

        契约里配置只存变量名不存值，所以取值这一步落在这里。
        """
        source = env if env is not None else os.environ
        return cls(
            app_id=source.get(cfg.app_id_env, ""),
            app_secret=source.get(cfg.app_secret_env, ""),
            bot_open_id=source.get(cfg.bot_open_id_env, ""),
            tenant_id=tenant_id,
            history_window=cfg.history_window,
            **kwargs,
        )

    def set_passive_listen(self, value: bool) -> None:
        """§3.7 权限核实后调这个，而不是去改 `FEISHU_P0`。"""
        self.capabilities.supports_passive_listen = value

    # ------------------------------------------------------------------
    # 长连接
    # ------------------------------------------------------------------

    def _default_connection_factory(self, on_raw: RawEventHandler) -> WSConnection:
        return LarkWSConnection(
            app_id=self.api.app_id,
            app_secret=self.api.app_secret,
            on_raw=on_raw,
            domain=self.api.domain,
        )

    async def start(self, on_event: EventHandler) -> None:
        """建连并持续投递事件；断线按 §3.3 指数退避无限重连。

        只有 `stop()` 能让它返回。
        """
        self._on_event = on_event
        self._stopping = False
        attempt = 0

        while not self._stopping:
            if attempt:
                delay = backoff_delay(attempt)
                logger.warning("feishu.reconnecting attempt=%s delay=%ss", attempt, delay)
                await self._sleep(delay)
                if self._stopping:
                    break

            connection = self._connection_factory(self._dispatch_raw)
            try:
                await connection.connect()
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                attempt += 1
                logger.warning("feishu.connect_failed attempt=%s err=%s", attempt, exc)
                continue

            if attempt:
                logger.info("feishu.reconnected after=%s attempts", attempt)
            attempt = 0
            self._connection = connection

            try:
                await connection.wait_closed()
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                logger.warning("feishu.connection_lost err=%s", exc)
            finally:
                self._connection = None
                await _safe_close(connection)

            # 断开了：下一轮从 1s 起退避重连。
            attempt = 1

    async def stop(self) -> None:
        self._stopping = True
        connection, self._connection = self._connection, None
        if connection is not None:
            await _safe_close(connection)
        await self.api.aclose()

    async def _dispatch_raw(self, raw: dict[str, Any]) -> None:
        """原始事件 → NormalizedEvent → on_event。

        adapter 不去重（§3.3）：重连后平台重推的同一条也照样往上送，
        由 ControlPlane 用 `event_id` 判（R2）。
        """
        event = normalize(
            raw,
            bot_open_id=self.bot_open_id,
            workspace_id=self.app_id,
            tenant_id=self.tenant_id,
        )
        if event is None:
            logger.debug("feishu.event_ignored type=%s", (raw.get("header") or {}).get("event_type"))
            return
        handler = self._on_event
        if handler is None:  # pragma: no cover - start() 之前不会有事件
            return

        started = time.monotonic()
        try:
            await handler(event)
        except Exception as exc:
            # 回调炸了不能把长连接带走（§3.3「任何未捕获异常…进程不退出」）。
            logger.exception("feishu.on_event_failed event_id=%s err=%s", event.event_id, exc)
        finally:
            elapsed = time.monotonic() - started
            if elapsed > ON_EVENT_BUDGET_SEC:
                logger.warning(
                    "feishu.on_event_slow event_id=%s elapsed=%.3fs budget=%.1fs",
                    event.event_id, elapsed, ON_EVENT_BUDGET_SEC,
                )

    # ------------------------------------------------------------------
    # 出站
    # ------------------------------------------------------------------

    async def _send_message(
        self,
        *,
        chat_id: str,
        reply_to: str | None,
        msg_type: str,
        content: str,
        in_thread: bool = True,
    ) -> str:
        """发一条消息，返回 message_id。

        有 `reply_to` 就走「回复消息」接口（带 `reply_in_thread` 才进话题，附录 A），
        否则走「发送消息」。
        """
        if reply_to:
            data = await self.api.request(
                "POST",
                feishu_api.PATH_MESSAGE_REPLY.format(message_id=reply_to),
                json={"content": content, "msg_type": msg_type, "reply_in_thread": in_thread},
                rate_limited=True,
            )
        else:
            data = await self.api.request(
                "POST",
                feishu_api.PATH_MESSAGES,
                params={"receive_id_type": "chat_id"},
                json={"receive_id": chat_id, "msg_type": msg_type, "content": content},
                rate_limited=True,
            )
        return str(data.get("message_id") or "")

    async def send_text(self, msg: OutboundText) -> SendResult:
        content = dumps_card(build_markdown_card(msg.text))
        message_id = await self._send_message(
            chat_id=msg.chat_id,
            reply_to=msg.reply_to,
            msg_type="interactive",
            content=content,
            in_thread=msg.in_thread,
        )
        return SendResult(message_id=message_id)

    async def send_card(self, chat_id: str, reply_to: str | None, card: ChecklistCard) -> SendResult:
        content = dumps_card(build_checklist_card(card))
        message_id = await self._send_message(
            chat_id=chat_id, reply_to=reply_to, msg_type="interactive", content=content
        )
        return SendResult(message_id=message_id, card_id=message_id)

    async def update_card(self, card_id: str, card: ChecklistCard) -> None:
        """原地更新同一条卡片消息。

        必须是 PATCH `/open-apis/im/v1/messages/{card_id}`（「更新应用发送的消息卡片」）。
        任何时候都不许退化成再发一条 —— W4 的「过程中卡片至少更新 3 次且不新增消息」
        就是靠这条。
        """
        await self.api.request(
            "PATCH",
            feishu_api.PATH_MESSAGE.format(message_id=card_id),
            json={"content": dumps_card(build_checklist_card(card))},
            rate_limited=True,
        )

    async def send_file(self, msg: OutboundFile) -> SendResult:
        """上传再发送（附录 A）。图片走 images 接口，其余走 files 接口。"""
        if msg.mime.startswith("image/"):
            data = await self.api.request(
                "POST",
                feishu_api.PATH_IMAGES,
                files={"image": (msg.name, msg.data, msg.mime)},
                data={"image_type": "message"},
            )
            content = json.dumps({"image_key": data.get("image_key")}, ensure_ascii=False)
            msg_type = "image"
        else:
            data = await self.api.request(
                "POST",
                feishu_api.PATH_FILES,
                files={"file": (msg.name, msg.data, msg.mime)},
                data={"file_type": _file_type_of(msg.name), "file_name": msg.name},
            )
            content = json.dumps({"file_key": data.get("file_key")}, ensure_ascii=False)
            msg_type = "file"

        message_id = await self._send_message(
            chat_id=msg.chat_id, reply_to=msg.reply_to, msg_type=msg_type, content=content
        )
        return SendResult(message_id=message_id)

    async def add_reaction(self, message_id: str, kind: ReactionKind) -> None:
        emoji = REACTION_EMOJI.get(kind)
        if emoji is None:  # pragma: no cover - ReactionKind 是 Literal，走不到
            raise PlatformError("bad_reaction", f"未知表情类型 {kind}", retryable=False)
        await self.api.request(
            "POST",
            feishu_api.PATH_MESSAGE_REACTIONS.format(message_id=message_id),
            json={"reaction_type": {"emoji_type": emoji}},
            rate_limited=True,
        )

    # ------------------------------------------------------------------
    # 读取
    # ------------------------------------------------------------------

    async def read_history(
        self, chat_id: str, *, limit: int = 50, thread_id: str | None = None
    ) -> list[HistoryMessage]:
        """群历史，**按时间正序**返回最近的 `limit` 条。

        拉取用 `ByCreateTimeDesc`（最新的在前）再翻转：W1 要的是「最近 N 条」，
        用正序翻页只会从群成立那天开始拿，拿到的是最老的 N 条。

        `thread_id` 给了就只留这条话题里的消息。飞书的 `container_id_type=thread`
        收的是 `omt_` 开头的话题 id，而我们锚点里存的是话题 root **消息** id
        （§3.1 Anchor 的注释、R7），两者不是一个 id 空间，所以这里在客户端筛，
        `root_id / parent_id / thread_id / message_id` 命中任一即算。

        不做 sender_kind 过滤 —— 那是 T3 `read_group_history` 工具的活（§3.2）。
        """
        wanted = max(limit, 0)
        if wanted == 0:
            return []
        # 要按话题筛就得多捞几页，否则一页里可能一条都不属于这个话题。
        budget = wanted * 4 if thread_id else wanted

        collected: list[dict[str, Any]] = []
        page_token: str | None = None
        while len(collected) < budget:
            params: dict[str, Any] = {
                "container_id_type": "chat",
                "container_id": chat_id,
                "sort_type": "ByCreateTimeDesc",
                "page_size": min(_HISTORY_PAGE_SIZE, budget - len(collected)),
                "with_sender_name": "true",
            }
            if page_token:
                params["page_token"] = page_token
            data = await self.api.request("GET", feishu_api.PATH_MESSAGES, params=params)
            items = data.get("items") or []
            collected.extend(item for item in items if isinstance(item, dict))
            page_token = data.get("page_token") if data.get("has_more") else None
            if not page_token or not items:
                break

        if thread_id:
            collected = [item for item in collected if _in_thread(item, thread_id)]

        # collected 是倒序的；取最近 wanted 条后翻回正序。
        recent = collected[:wanted]
        recent.reverse()
        return [_to_history_message(item) for item in recent]

    async def read_document(self, url_or_token: str) -> DocumentContent:
        """读一篇云文档，返回正文文本。

        wiki 链接先换成它挂的 docx token 再读。返回的是「获取文档纯文本内容」
        接口的产物 —— 是纯文本而不是带格式的 markdown，见回执里的说明。
        """
        kind, token = _parse_doc_ref(url_or_token)
        if kind == "wiki":
            node = await self.api.request(
                "GET", feishu_api.PATH_WIKI_NODE, params={"token": token, "obj_type": "wiki"}
            )
            obj = node.get("node") or {}
            token = str(obj.get("obj_token") or token)

        meta = await self.api.request(
            "GET", feishu_api.PATH_DOCX_DOCUMENT.format(document_id=token)
        )
        raw = await self.api.request(
            "GET",
            feishu_api.PATH_DOCX_RAW_CONTENT.format(document_id=token),
            params={"lang": 0},
        )
        title = str((meta.get("document") or {}).get("title") or "")
        return DocumentContent(
            title=title,
            text=str(raw.get("content") or ""),
            url=url_or_token if url_or_token.startswith("http") else f"/docx/{token}",
        )

    async def download_file(self, message_id: str, file_key: str) -> bytes:
        """下载消息里的资源文件（附录 A）。

        `type` 只有 image / file 两种取值，按 key 前缀判：飞书的图片 key 是
        `img_v2_…` / `img_v3_…`，文件 key 是 `file_v2_…`。
        """
        resource_type = "image" if file_key.startswith("img_") else "file"
        return await self.api.request(
            "GET",
            feishu_api.PATH_MESSAGE_RESOURCE.format(message_id=message_id, file_key=file_key),
            params={"type": resource_type},
            binary=True,
        )


# ---------------------------------------------------------------------------
# 私有小工具
# ---------------------------------------------------------------------------

async def _safe_close(connection: WSConnection) -> None:
    try:
        await connection.close()
    except asyncio.CancelledError:
        raise
    except Exception as exc:  # pragma: no cover - 关连接失败没什么可做的
        logger.warning("feishu.close_failed err=%s", exc)


def _parse_doc_ref(url_or_token: str) -> tuple[str, str]:
    """云文档链接或裸 token → `(类型, token)`。

    认 `/docx/<token>`、`/docs/<token>`、`/wiki/<token>` 三种链接；
    传进来的要是裸 token（没有 `/`），按 docx 处理。
    """
    match = _DOC_URL_RE.search(url_or_token)
    if match:
        return match.group(1), match.group(2)
    return "docx", url_or_token.strip().split("?", 1)[0].rstrip("/").rpartition("/")[2]


def _file_type_of(name: str) -> str:
    _, _, ext = name.rpartition(".")
    return _FILE_TYPE_BY_EXT.get(f".{ext.lower()}", "stream") if ext else "stream"


def _in_thread(item: dict[str, Any], thread_id: str) -> bool:
    return thread_id in (
        item.get("root_id"),
        item.get("parent_id"),
        item.get("thread_id"),
        item.get("message_id"),
    )


def _to_history_message(item: dict[str, Any]) -> HistoryMessage:
    sender = item.get("sender") or {}
    mentions = [m for m in (item.get("mentions") or []) if isinstance(m, dict)]
    body = item.get("body") or {}
    # 历史接口把正文放在 body.content，事件里放在 message.content —— 抹平成一个形状再复用解析。
    message = {"message_type": item.get("msg_type"), "content": body.get("content")}
    # bot_open_id 传 None：历史里别人 @ 机器人的那句话，去掉 @ 反而看不懂上下文。
    text, _raw_text, _image_keys = extract_text(message, mentions, None)

    created_at = _history_created_at(item)
    return HistoryMessage(
        message_id=str(item.get("message_id") or ""),
        sender_id=str(sender.get("id") or ""),
        sender_kind=str(sender_kind_of(sender.get("sender_type")).value),
        sender_name=sender.get("sender_name") or None,
        text=text,
        thread_id=(str(item.get("root_id")) if item.get("root_id") else None),
        created_at=created_at,
    )


def _history_created_at(item: dict[str, Any]) -> datetime:
    # 「获取会话历史消息」的 create_time 文档写的是毫秒，但 to_datetime 按量级自己判，
    # 跟事件面共用一套解析，省得哪天两边口径又分叉。
    return to_datetime(item.get("create_time")) or datetime.now(tz=UTC)


__all__ = ["ON_EVENT_BUDGET_SEC", "REACTION_EMOJI", "KNOWN_EMOJI_TYPES", "FeishuPlatform"]
