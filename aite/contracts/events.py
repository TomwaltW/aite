# aite/contracts/events.py
from datetime import datetime
from enum import StrEnum
from typing import Any, Literal

from pydantic import BaseModel, Field


class SenderKind(StrEnum):
    human = "human"
    bot = "bot"        # 其他机器人
    app = "app"        # 应用/集成（含 Aite 自己）
    system = "system"  # 平台系统消息

class EventKind(StrEnum):
    message = "message"
    message_edited = "message_edited"
    message_deleted = "message_deleted"
    card_action = "card_action"
    bot_added = "bot_added"
    member_changed = "member_changed"

class Anchor(BaseModel):
    platform: str
    chat_id: str
    message_id: str                 # 触发消息 id
    thread_id: str | None = None    # 飞书：话题 root 消息 id；顶层消息则为 None
    task_no: str | None = None      # "#A17"，P0 只展示不路由

class Attachment(BaseModel):
    kind: Literal["image", "file"]
    file_key: str                   # 平台文件 key，配合 message_id 下载
    message_id: str
    name: str | None = None
    size: int | None = None
    mime: str | None = None

class CardAction(BaseModel):
    card_id: str                    # = 卡片所在消息的 message_id
    action: Literal["stop", "evidence"]
    task_id: str | None = None
    value: dict[str, Any] = Field(default_factory=dict)

class NormalizedEvent(BaseModel):
    event_id: str                   # 平台 message_id 或 event_id，去重键
    kind: EventKind
    platform: str
    tenant_id: str = "default"
    workspace_id: str               # 飞书 app_id
    chat_id: str
    chat_type: Literal["group", "p2p"]
    sender_id: str                  # 飞书 open_id
    sender_kind: SenderKind
    sender_name: str | None = None
    text: str                       # 去掉 @Aite 后、strip 过的纯文本；非文本消息为 ""
    raw_text: str | None = None
    mentioned: bool                 # 是否 @ 了 Aite
    anchor: Anchor
    attachments: list[Attachment] = Field(default_factory=list)
    card_action: CardAction | None = None
    occurred_at: datetime
    raw: dict[str, Any] = Field(default_factory=dict)   # 原始事件，仅供审计；任何逻辑不得依赖 raw
