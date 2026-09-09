# aite/contracts/__init__.py
"""Aite P0 冻结契约（dev-spec-2026-09-09 §3.1 / §3.2）。

各子模块的代码由 T0 从 spec 代码块逐字节落盘，本文件只负责 re-export。
契约文件的 sha256 记在仓库根的 .contracts.lock，用 `python -m aite.contracts.lock --check` 校验。
"""
from .capabilities import FEISHU_P0, Platform, PlatformCapabilities
from .config import (
    AiteConfig,
    FeishuConfig,
    ModelConfig,
    SandboxConfig,
    StorageConfig,
    WorkerConfig,
)
from .events import (
    Anchor,
    Attachment,
    CardAction,
    EventKind,
    NormalizedEvent,
    SenderKind,
)
from .evidence import (
    GENESIS,
    EvidenceEvent,
    EvidenceKind,
    canonical_json,
    chain_hash,
    payload_hash_of,
)
from .gateway import (
    DEFAULT_TOOL_TIMEOUT_SEC,
    MAX_TOOL_CONTENT_CHARS,
    ToolContext,
    ToolError,
    ToolErrorCode,
    ToolResult,
)
from .outbound import (
    ChecklistCard,
    ChecklistItemView,
    OutboundFile,
    OutboundText,
    ReactionKind,
    SendResult,
)
from .ports import (
    ControlPlane,
    DocumentContent,
    EventHandler,
    EvidenceWriter,
    HistoryMessage,
    ModelPort,
    PlatformPort,
    SandboxPort,
    SessionStore,
    ToolGateway,
)
from .protocol import (
    ALL_MODEL_TOOLS,
    CHECKLIST_TOOLS,
    FINAL_TOOL,
    GATEWAY_TOOLS,
    LOCAL_TOOL_NAMES,
    ArtifactRef,
    Message,
    ModelTurn,
    ToolCallRequest,
    ToolSpec,
    Usage,
)
from .sandbox import (
    MAX_EXEC_OUTPUT_CHARS,
    ExecRequest,
    ExecResult,
    FileEntry,
    SandboxSpec,
)
from .session import (
    ChecklistItem,
    Session,
    SessionKind,
    SessionStatus,
    Task,
    TaskStatus,
    Turn,
    encode_task_no,
)

CONTRACT_VERSION = "p0.1"

__all__ = [
    "CONTRACT_VERSION",
    # capabilities.py
    "Platform",
    "PlatformCapabilities",
    "FEISHU_P0",
    # events.py
    "SenderKind",
    "EventKind",
    "Anchor",
    "Attachment",
    "CardAction",
    "NormalizedEvent",
    # outbound.py
    "OutboundText",
    "ChecklistItemView",
    "ChecklistCard",
    "OutboundFile",
    "SendResult",
    "ReactionKind",
    # session.py
    "SessionKind",
    "SessionStatus",
    "TaskStatus",
    "Turn",
    "ChecklistItem",
    "Session",
    "Task",
    "encode_task_no",
    # protocol.py
    "ToolSpec",
    "ToolCallRequest",
    "ArtifactRef",
    "CHECKLIST_TOOLS",
    "FINAL_TOOL",
    "GATEWAY_TOOLS",
    "ALL_MODEL_TOOLS",
    "LOCAL_TOOL_NAMES",
    "Message",
    "Usage",
    "ModelTurn",
    # gateway.py
    "ToolErrorCode",
    "ToolError",
    "ToolContext",
    "ToolResult",
    "MAX_TOOL_CONTENT_CHARS",
    "DEFAULT_TOOL_TIMEOUT_SEC",
    # sandbox.py
    "SandboxSpec",
    "ExecRequest",
    "FileEntry",
    "ExecResult",
    "MAX_EXEC_OUTPUT_CHARS",
    # evidence.py
    "EvidenceKind",
    "EvidenceEvent",
    "GENESIS",
    "canonical_json",
    "chain_hash",
    "payload_hash_of",
    # config.py
    "FeishuConfig",
    "ModelConfig",
    "SandboxConfig",
    "WorkerConfig",
    "StorageConfig",
    "AiteConfig",
    # ports.py
    "HistoryMessage",
    "DocumentContent",
    "EventHandler",
    "PlatformPort",
    "ModelPort",
    "SandboxPort",
    "ToolGateway",
    "SessionStore",
    "EvidenceWriter",
    "ControlPlane",
]
