"""PlatformPort 的飞书实现骨架（owner: T1，契约见 aite/contracts/ports.py §3.2）。

T0 只落空实现：签名与契约一致，方法体一律 raise NotImplementedError。
"""
from ...contracts import (
    FEISHU_P0,
    ChecklistCard,
    DocumentContent,
    EventHandler,
    HistoryMessage,
    OutboundFile,
    OutboundText,
    PlatformCapabilities,
    ReactionKind,
    SendResult,
)


class FeishuPlatform:
    """PlatformPort（T1）。"""

    # 用副本而不是 FEISHU_P0 本身：§3.7 允许 adapter 在运行时把
    # supports_passive_listen 改成 True，直接改契约常量会就地污染全局单例。
    capabilities: PlatformCapabilities = FEISHU_P0.model_copy()

    async def start(self, on_event: EventHandler) -> None:
        raise NotImplementedError("T1")

    async def stop(self) -> None:
        raise NotImplementedError("T1")

    async def send_text(self, msg: OutboundText) -> SendResult:
        raise NotImplementedError("T1")

    async def send_card(self, chat_id: str, reply_to: str | None, card: ChecklistCard) -> SendResult:
        raise NotImplementedError("T1")

    async def update_card(self, card_id: str, card: ChecklistCard) -> None:
        raise NotImplementedError("T1")

    async def send_file(self, msg: OutboundFile) -> SendResult:
        raise NotImplementedError("T1")

    async def add_reaction(self, message_id: str, kind: ReactionKind) -> None:
        raise NotImplementedError("T1")

    async def read_history(
        self, chat_id: str, *, limit: int = 50, thread_id: str | None = None
    ) -> list[HistoryMessage]:
        raise NotImplementedError("T1")

    async def read_document(self, url_or_token: str) -> DocumentContent:
        raise NotImplementedError("T1")

    async def download_file(self, message_id: str, file_key: str) -> bytes:
        raise NotImplementedError("T1")
