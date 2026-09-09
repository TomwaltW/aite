# aite/contracts/session.py
from datetime import datetime
from enum import StrEnum
from typing import Any, Literal

from pydantic import BaseModel, Field

from .events import Anchor, Attachment


class SessionKind(StrEnum):
    channel = "channel"   # P0 不创建
    task = "task"
    dm = "dm"             # P0 不创建

class SessionStatus(StrEnum):
    active = "active"
    idle = "idle"
    archived = "archived"

class TaskStatus(StrEnum):            # 与立项方案 §4.5 状态机一致
    created = "created"
    planning = "planning"
    answering = "answering"
    working = "working"
    awaiting_approval = "awaiting_approval"   # P0 不会进入
    delivered = "delivered"
    failed = "failed"
    cancelled = "cancelled"

class Turn(BaseModel):
    session_id: str
    seq: int
    role: Literal["user", "assistant", "system_note"]
    platform_user_id: str | None
    content: str
    attachments: list[Attachment] = Field(default_factory=list)
    created_at: datetime

class ChecklistItem(BaseModel):
    id: str                          # "c1", "c2", ...（worker 分配）
    text: str
    state: Literal["todo", "doing", "done", "failed"] = "todo"
    note: str | None = None

class Session(BaseModel):
    id: str                          # uuid4
    tenant_id: str
    workspace_id: str
    chat_id: str
    kind: SessionKind
    anchor: Anchor
    status: SessionStatus = SessionStatus.active
    created_by: str
    config_snapshot: dict[str, Any] = Field(default_factory=dict)  # 创建时冻结的模型名/系统指令 hash
    created_at: datetime
    last_active_at: datetime
    archived_at: datetime | None = None

class Task(BaseModel):
    id: str                          # uuid4
    session_id: str
    task_no: str                     # encode_task_no(n)
    status: TaskStatus = TaskStatus.created
    title: str = ""
    checklist: list[ChecklistItem] = Field(default_factory=list)
    card_id: str | None = None
    sandbox_id: str | None = None
    session_token: str               # 随机 32 hex，Tool Gateway 校验用
    model: str = ""
    steps: int = 0
    tokens_in: int = 0
    tokens_out: int = 0
    cost: float = 0.0
    max_steps: int = 40
    max_wall_sec: int = 1200
    result_summary: str = ""
    evidence_root_hash: str | None = None
    created_by: str
    created_at: datetime
    updated_at: datetime

_B32 = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"   # Crockford base32

def encode_task_no(n: int) -> str:
    """租户内递增计数 → '#A..'。向量：1→'#A1'，17→'#AH'，32→'#A10'，1000→'#AZ8'。n<1 抛 ValueError。"""
    if n < 1:
        raise ValueError(n)
    s = ""
    while n:
        s = _B32[n % 32] + s
        n //= 32
    return "#A" + s
