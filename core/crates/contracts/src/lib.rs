//! Aite 冻结契约（Rust 版；上游是 docs/dev-spec-2026-09-11-rustgo.md §3）。
//!
//! 与旧 Python 契约（aite/contracts/**，CONTRACT_VERSION "p0.1"）的关系：
//! - 数据形状逐字段对应，JSON 字段名与枚举取值完全一致（snake_case 字符串）；
//! - 跨进程的那部分（events / outbound / capabilities / sandbox）另有 proto 定义
//!   （proto/aite/v1/*.proto），本 crate 的类型是 core 内部的 domain 形态，
//!   两者的互转在 aite-proto crate；
//! - 调用面（ports.rs）把旧版靠 getattr 鸭子类型探测的「协议外方法」全部显式化。
//!
//! 本 crate 的每个文件都在 .contracts.lock 里：`aite contracts lock --check` 必须始终 OK。

pub mod capabilities;
pub mod config;
pub mod errors;
pub mod events;
pub mod evidence;
pub mod gateway;
pub mod outbound;
pub mod ports;
pub mod protocol;
pub mod sandbox;
pub mod session;

/// 契约版本。p0.1 是 Python 版；p0.2 = 同样的数据形状 + core/edge 进程边界 + 显式化的调用面。
/// edge 的 EdgeStatus.contract_version 必须等于它，否则 core 拒绝起飞。
pub const CONTRACT_VERSION: &str = "p0.2";

pub use capabilities::{PlatformCapabilities, feishu_p0};
pub use config::{
    AiteConfig, EdgeConfig, FeishuConfig, ModelConfig, ModelProvider, PlatformChoice,
    SandboxConfig, StorageConfig, WorkerConfig,
};
pub use errors::{
    ConfigError, EvidenceError, IngressError, ModelError, PlatformError, SandboxError,
    SandboxErrorKind, StoreError,
};
pub use events::{
    Anchor, Attachment, AttachmentKind, CardAction, CardActionKind, ChatType, EventKind,
    NormalizedEvent, SenderKind,
};
pub use evidence::{
    EvidenceEvent, EvidenceKind, GENESIS, canonical_json, chain_hash, payload_hash_of,
};
pub use gateway::{
    DEFAULT_TOOL_TIMEOUT_SEC, MAX_TOOL_CONTENT_CHARS, ToolContext, ToolError, ToolErrorCode,
    ToolResult,
};
pub use outbound::{
    CardStatus, ChecklistCard, ChecklistItemView, ChecklistState, DocumentContent, HistoryMessage,
    OutboundFile, OutboundText, ReactionKind, SendResult,
};
pub use ports::{
    ControlPlane, EventHandler, EvidenceWriter, ModelPort, PlatformPort, RunHooks, SandboxPort,
    SessionStore, TaskWorker, ToolGateway,
};
pub use protocol::{
    ArtifactRef, Message, ModelTurn, Role, ToolCallRequest, ToolSpec, Usage, all_model_tools,
    checklist_tools, final_tool, gateway_tools, is_local_tool, local_tool_names,
};
pub use sandbox::{
    ExecLanguage, ExecRequest, ExecResult, FileEntry, MAX_EXEC_OUTPUT_CHARS, SandboxNetwork,
    SandboxSpec,
};
pub use session::{
    ACTIVE_TASK_STATUSES, ChecklistItem, Session, SessionKind, SessionStatus, Task, TaskNoError,
    TaskStatus, Turn, TurnRole, encode_task_no,
};
