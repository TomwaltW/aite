//! Tool Gateway 的调用形状（对应旧 aite/contracts/gateway.py）。core 内部类型。
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::events::str_enum;
use crate::protocol::ArtifactRef;

str_enum! {
    ToolErrorCode {
        NotFound => "not_found",
        InvalidArgs => "invalid_args",
        Denied => "denied",
        Timeout => "timeout",
        /// 平台/外部系统错误
        Upstream => "upstream",
        Sandbox => "sandbox",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContext {
    pub tenant_id: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub session_id: String,
    pub task_id: String,
    pub session_token: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    /// download_attachment 用
    #[serde(default)]
    pub attachments_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub ok: bool,
    /// 给模型看的文本，已截断到 ≤ MAX_TOOL_CONTENT_CHARS（按 Unicode 标量计）
    pub content: String,
    #[serde(default)]
    pub data: Option<Map<String, Value>>,
    #[serde(default)]
    pub error: Option<ToolError>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
}

pub const MAX_TOOL_CONTENT_CHARS: usize = 12_000;
pub const DEFAULT_TOOL_TIMEOUT_SEC: u64 = 60;
