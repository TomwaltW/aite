//! aite-edge-client —— `PlatformPort` / `SandboxPort` 的 gRPC 实现（tonic client 到 Go edge）
//! 外加 core 侧的 `IngressServer`。owner: R6
//!
//! Python 版是单进程，这块没有对应物；规格在 `proto/aite/v1/edge.proto` 头注释与 spec §2.1–§2.2：
//!
//! ```text
//! core ──PlatformService / SandboxService / EdgeStatusService──▶ edge   (unix: edge_socket)
//! core ◀────────────────IngressService.HandleEvent───────────── edge   (unix: core_socket)
//! ```
//!
//! 三件事值得记住：
//! 1. **懒连接**：`EdgeClient::connect` 不等 edge 起来（§2.1 启动顺序无关），后台探针按
//!    1→2→…→30s 退避拨号；edge 不在时 RPC 返回 retryable 错误，edge 起来后自动恢复。
//! 2. **错误映射是 R0 冻结的**（`aite_proto::status`），这里一条都不自己编。
//! 3. **`start/stop` 不是连飞书**，而是起/停 core 侧监听 `core_socket` 的 IngressService（D1）。
mod ingress;
mod link;
mod platform;
mod sandbox;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aite_contracts::{EdgeConfig, PlatformError, PlatformPort, SandboxPort};
use aite_proto::pb;

pub use aite_proto::pb::EdgeStatus;
pub use ingress::IngressServer;
pub use platform::EdgePlatform;
pub use sandbox::EdgeSandbox;

use crate::link::Link;

/// 到 edge 的那套客户端。`platform()` / `sandbox()` 给 RΩ 当 Port 用，`status()` 给起飞体检用。
pub struct EdgeClient {
    link: Arc<Link>,
    platform: Arc<EdgePlatform>,
    sandbox: Arc<EdgeSandbox>,
}

impl EdgeClient {
    /// 懒连接：只备地址、起后台探针，不等 edge（连不上 → 第一次 RPC 拿 retryable 错误）。
    ///
    /// `repo_root` 用来把 config 里的相对路径（`data/run/aite-edge.sock`）落到实处。
    pub async fn connect(cfg: &EdgeConfig, repo_root: &Path) -> Result<EdgeClient, PlatformError> {
        let edge_socket = resolve(repo_root, &cfg.edge_socket);
        let core_socket = resolve(repo_root, &cfg.core_socket);
        let max_bytes = (cfg.max_message_mb as usize).saturating_mul(1024 * 1024);

        let link = Link::open(edge_socket, max_bytes)?;
        link.probe();

        Ok(Self {
            platform: Arc::new(EdgePlatform::new(link.clone(), core_socket)),
            sandbox: Arc::new(EdgeSandbox::new(link.clone())),
            link,
        })
    }

    pub fn platform(&self) -> Arc<dyn PlatformPort> {
        self.platform.clone()
    }

    pub fn sandbox(&self) -> Arc<dyn SandboxPort> {
        self.sandbox.clone()
    }

    /// 具体类型那一面（`started()` / `refresh_capabilities()` / `ingress()` 在这上面）。
    pub fn edge_platform(&self) -> Arc<EdgePlatform> {
        self.platform.clone()
    }

    /// edge 自身健康：RΩ 起飞时拿它比对 `contract_version`（不等就拒绝起飞）。
    pub async fn status(&self) -> Result<EdgeStatus, PlatformError> {
        let reply = self
            .link
            .status_client()
            .get_status(pb::GetStatusRequest {})
            .await
            .map_err(|st| self.link.platform_error(&st))?;
        Ok(reply.into_inner())
    }

    /// edge socket 的绝对路径（日志/体检用）。
    pub fn edge_socket(&self) -> &Path {
        self.link.socket()
    }

    /// 后台探针最近一次拨号的结果。
    pub fn connected(&self) -> bool {
        self.link.connected()
    }
}

fn resolve(repo_root: &Path, socket: &str) -> PathBuf {
    let path = Path::new(socket);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    }
}
