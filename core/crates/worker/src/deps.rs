//! `WorkerDeps`：组装入口（CC3 从 `agent.rs` 原样搬来，字段一个不动 —— `app` 按结构体字面量构造它）。
use std::sync::Arc;

use aite_contracts::{
    AiteConfig, EvidenceWriter, ModelPort, PlatformPort, SandboxPort, SessionStore, ToolGateway,
};

/// 组装入口（RΩ 按这个接，名字别自己发明）。
pub struct WorkerDeps {
    pub store: Arc<dyn SessionStore>,
    pub platform: Arc<dyn PlatformPort>,
    pub model: Arc<dyn ModelPort>,
    pub evidence: Arc<dyn EvidenceWriter>,
    pub config: AiteConfig,
    pub gateway: Option<Arc<dyn ToolGateway>>,
    pub sandbox: Option<Arc<dyn SandboxPort>>,
}
