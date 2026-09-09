# aite/contracts/gateway.py
from enum import StrEnum
from typing import Any

from pydantic import BaseModel, Field

from .protocol import ArtifactRef


class ToolErrorCode(StrEnum):
    not_found = "not_found"
    invalid_args = "invalid_args"
    denied = "denied"
    timeout = "timeout"
    upstream = "upstream"      # 平台/外部系统错误
    sandbox = "sandbox"

class ToolError(BaseModel):
    code: ToolErrorCode
    message: str

class ToolContext(BaseModel):
    tenant_id: str
    workspace_id: str
    chat_id: str
    session_id: str
    task_id: str
    session_token: str
    thread_id: str | None = None
    attachments_message_id: str | None = None   # download_attachment 用

class ToolResult(BaseModel):
    call_id: str
    name: str
    ok: bool
    content: str                                # 给模型看的文本，已截断到 ≤ MAX_TOOL_CONTENT_CHARS
    data: dict[str, Any] | None = None
    error: ToolError | None = None
    duration_ms: int = 0
    artifacts: list[ArtifactRef] = Field(default_factory=list)

MAX_TOOL_CONTENT_CHARS = 12000
DEFAULT_TOOL_TIMEOUT_SEC = 60
