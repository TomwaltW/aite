//! 各调用面的错误类型。ToolGateway 永远不抛：失败一律表达为 ToolResult{ok:false}。
use crate::gateway::ToolErrorCode;

/// 平台（飞书）错误。retryable 的判定与 gRPC status 的映射见 proto/aite/v1/edge.proto 头注释。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("[{code}] {message}")]
pub struct PlatformError {
    /// 平台错误码（飞书 code 十进制）或短 token（"transport_error" / "timeout" / "unimplemented"）
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub http_status: Option<u16>,
}

impl PlatformError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            http_status: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxErrorKind {
    /// docker daemon 不可达 / 镜像缺失
    Unavailable,
    /// sandbox_id 不存在
    NotFound,
    /// 路径不在 /work 下或含 ..
    InvalidPath,
    /// 文件不存在（final.artifacts 指到不存在的文件时 worker 据此"跳过该产物"）
    FileNotFound,
    Timeout,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("sandbox {kind:?}: {message}")]
pub struct SandboxError {
    pub kind: SandboxErrorKind,
    pub message: String,
}

impl SandboxError {
    pub fn new(kind: SandboxErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// append_turn 撞上已有的 (session_id, seq)
    #[error("重复的 turn：session={session_id} seq={seq}")]
    DuplicateTurn { session_id: String, seq: u64 },
    #[error("store 未初始化：先调 init()")]
    NotInitialized,
    #[error("sqlite: {0}")]
    Sqlite(String),
    #[error("序列化: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// 缺 base_url / model / 密钥环境变量。消息里只出现变量名，绝不出现取值。
    #[error("模型配置不完整：{0}")]
    Config(String),
    #[error("模型服务错误：{0}")]
    Upstream(String),
    #[error("模型响应无法解析：{0}")]
    BadResponse(String),
}

#[derive(Debug, thiserror::Error)]
pub enum EvidenceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("证据链损坏（{task_id}）：{detail}")]
    Corrupt { task_id: String, detail: String },
    #[error("序列化: {0}")]
    Serde(#[from] serde_json::Error),
}

/// ControlPlane.handle_event 的失败面：IngressService 把它翻译成 gRPC INTERNAL，
/// edge 计 ingress.errors 并向平台返回错误让其重推。
#[derive(Debug, thiserror::Error)]
pub enum IngressError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("evidence: {0}")]
    Evidence(#[from] EvidenceError),
    #[error("platform: {0}")]
    Platform(#[from] PlatformError),
    #[error("非法事件：{0}")]
    Invalid(String),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("配置文件不存在：{0}（可从 config/aite.example.yaml 复制）")]
    Missing(String),
    #[error("配置文件顶层必须是 mapping")]
    NotAMapping,
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// 供各实现把外部错误统一成 ToolErrorCode 时用的小工具。
pub fn tool_error_code_of_sandbox(_e: &SandboxError) -> ToolErrorCode {
    ToolErrorCode::Sandbox
}
