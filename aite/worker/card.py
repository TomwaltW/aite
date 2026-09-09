"""Checklist 卡片的渲染与更新合并（W3 / W4）。

W4 要求「同一任务 500ms 内多次变更只调一次 `update_card`，任务结束时必定再调一次」。
这里用**拉取式**合并，不起后台定时器：

- 一个 500ms 窗口内的第一次变更立即推送，窗口内后续变更只更新待发状态；
- 每步结束时 `maybe_flush()` 会把过期的待发状态推出去；
- 任务结束时 `force_flush()` 无条件再推一次，兜住窗口末尾没推出去的那次。

取舍：没有定时器就意味着「最后一次变更后如果再无任何活动」，要等到任务结束才落地。
P0 里任务结束一定会到（delivered / failed / cancelled 三条路都调 force_flush），
所以变更不会丢；换来的是测试完全确定、不依赖真实时钟。
"""
import time
from collections.abc import Callable

from ..contracts import (
    ChecklistCard,
    ChecklistItemView,
    PlatformPort,
    Session,
    Task,
)

MAX_TITLE_CHARS = 40      # ChecklistCard.title：「≤40 字，worker 截断」
MAX_ITEM_CHARS = 20       # W9：checklist 每项 ≤20 字


def clip(text: str, limit: int) -> str:
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 1] + "…"


def render_card(
    task: Task, session: Session, *, initiator: str, status: str, note: str | None = None
) -> ChecklistCard:
    """把 Task 的当前状态渲染成卡片。

    footer 只写步数与花费，P0 不做耗时预估；`checklist_note` 的备注也挂在 footer 上 ——
    契约里卡片没有单独的备注字段，而「不新增消息」要求它必须落在这张卡片里面。
    """
    footer = f"已用 {task.steps} 步 · ¥{task.cost:.2f}"
    if note:
        footer = f"{clip(note, 40)} · {footer}"
    return ChecklistCard(
        task_id=task.id,
        task_no=task.task_no,
        title=clip(task.title or "处理中", MAX_TITLE_CHARS),
        initiator=initiator or session.created_by,
        started_at=f"{task.created_at.hour}:{task.created_at.minute:02d}",
        status=status,
        items=[
            ChecklistItemView(id=i.id, text=clip(i.text, MAX_ITEM_CHARS), state=i.state, note=i.note)
            for i in task.checklist
        ],
        footer=footer,
    )


class CardCoalescer:
    """一个任务一个实例。保证 `send_card` 至多一次，`update_card` 按 W4 合并。"""

    def __init__(
        self,
        platform: PlatformPort,
        *,
        min_interval_ms: int = 500,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._platform = platform
        self._interval = min_interval_ms / 1000.0
        self._clock = clock
        self._card_id: str | None = None
        self._pending: ChecklistCard | None = None
        self._last_flush: float = 0.0

    @property
    def card_id(self) -> str | None:
        return self._card_id

    @property
    def sent(self) -> bool:
        return self._card_id is not None

    async def ensure_card(self, chat_id: str, reply_to: str | None, card: ChecklistCard) -> str:
        """首次调用发卡片；之后是 no-op —— 「绝不出现第二条 send_card」靠这里保证。"""
        if self._card_id is None:
            res = await self._platform.send_card(chat_id, reply_to, card)
            self._card_id = res.card_id or res.message_id
            self._last_flush = self._clock()
            self._pending = None
        return self._card_id

    async def update(self, card: ChecklistCard) -> None:
        """状态变了就调这个。是否真的推送由 500ms 窗口决定。"""
        if self._card_id is None:
            return
        self._pending = card
        await self.maybe_flush()

    async def maybe_flush(self) -> None:
        if self._card_id is None or self._pending is None:
            return
        if self._clock() - self._last_flush < self._interval:
            return
        await self._flush()

    async def force_flush(self, card: ChecklistCard | None = None) -> None:
        """任务结束时调用：无条件推一次。"""
        if self._card_id is None:
            return
        if card is not None:
            self._pending = card
        if self._pending is None:
            return
        await self._flush()

    async def _flush(self) -> None:
        card = self._pending
        self._pending = None
        self._last_flush = self._clock()
        await self._platform.update_card(self._card_id, card)
