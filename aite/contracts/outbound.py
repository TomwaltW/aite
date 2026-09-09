# aite/contracts/outbound.py
from typing import Literal

from pydantic import BaseModel, Field


class OutboundText(BaseModel):
    chat_id: str
    text: str                       # markdown（飞书 post/markdown 由 adapter 转）
    reply_to: str | None = None     # 回复哪条消息
    in_thread: bool = True          # reply_to 存在时是否进话题

class ChecklistItemView(BaseModel):
    id: str
    text: str
    state: Literal["todo", "doing", "done", "failed"]
    note: str | None = None

class ChecklistCard(BaseModel):
    task_id: str
    task_no: str                    # "#A17"
    title: str                      # 任务一句话（≤40 字，worker 截断）
    initiator: str                  # 发起人显示名
    started_at: str                 # "9:02"
    status: Literal["working", "delivered", "failed", "cancelled"]
    items: list[ChecklistItemView] = Field(default_factory=list)
    footer: str = ""                # "预计 2 分钟 · 已用 ¥0.12"
    actions: list[Literal["stop", "evidence"]] = Field(default_factory=lambda: ["stop"])

class OutboundFile(BaseModel):
    chat_id: str
    reply_to: str | None
    name: str
    mime: str
    data: bytes

class SendResult(BaseModel):
    message_id: str
    card_id: str | None = None      # 卡片消息的 id，后续 update_card 用

ReactionKind = Literal["ack", "done", "fail"]   # adapter 负责映射到平台 emoji
