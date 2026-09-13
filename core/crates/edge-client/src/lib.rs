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
mod gate;
mod ingress;
mod link;
mod platform;
mod sandbox;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aite_contracts::{EdgeConfig, PlatformError, PlatformPort, SandboxPort};
use aite_proto::pb;

pub use aite_proto::pb::EdgeStatus;
pub use gate::ContractState;
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
    ///
    /// 每答上来一次都顺手记进契约闸门（`gate.rs`），所以它也是「连上就比一次」的入口。
    ///
    /// 产品代码里的三个调用方：`app.rs` 的 `check_contract_version`（起飞时比版本）、
    /// `preflight.rs` 第 6 组（沙箱可用，先问 daemon 可达）、`wiring.rs` 的评测接线。
    /// 原注释写的「`!status` 的健康行」**全仓不存在** —— 与 `app.rs` 上那句同源的谎话
    /// （W2 已改掉那一处）。
    pub async fn status(&self) -> Result<EdgeStatus, PlatformError> {
        let reply = self
            .link
            .status_client()
            .get_status(pb::GetStatusRequest {})
            .await
            .map_err(|st| self.link.platform_error(&st))
            .inspect(|_| self.link.note_ok())?;
        let status = reply.into_inner();
        self.link.note_contract(&status);
        Ok(status)
    }

    /// 契约闸门此刻的状态：闸落着时 platform / sandbox 的每一发 RPC 都会在本地直接失败。
    ///
    /// **它是给测试当观测口的，产品代码不走这条路 —— 零调用方，这是刻意的。**
    /// 唯一的使用者是 `tests/contract_gate.rs`（9 处），那一组拿它当闸门三态
    /// （未验证 / 放行 / 落闸）的判据。产品代码感知闸门的方式是**撞上它**：
    /// 落闸时 `platform` / `sandbox` 的 RPC 在本地直接失败，没有谁需要先问一句状态。
    ///
    /// **AA2 量过删它的代价，结论是留着。** `link` 是私有字段、`Link::contract_state`
    /// 是 `pub(crate)`，所以删掉这三行之后 `contract_gate.rs`（外部 crate）就够不着闸门了：
    /// 要么把 `Link::contract_state` 放成 `pub` 再新开一个 `EdgeClient::link()` 公开访问器
    /// —— 暴露出去的面比删掉的这一个方法大得多；要么让那 234 行测试改用「发一发 RPC
    /// 看它失不失败」去间接推断三态，把一组直给的判据换成一组间接的。两条都比留着贵。
    ///
    /// （原注释说它是给「`!status` 的健康行」用的，而那条健康行全仓不存在 ——
    /// 与 `app.rs` 上那句同源的谎话，W2 已改掉那一处。）
    pub fn contract_state(&self) -> ContractState {
        self.link.contract_state()
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
