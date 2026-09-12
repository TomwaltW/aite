//! 到 edge 的那根 Unix domain socket 连接（懒连接 + 无限退避重拨）。
//!
//! §2.1「启动顺序无关」：core 起飞不等 edge。所以这里是**懒连接** —— `EdgeClient::connect`
//! 只把地址备好（tonic 0.14 原生认 `unix://` 的 Endpoint），第一次 RPC 才真拨号；
//! 拨不通的那次 RPC 拿到一个 retryable 的错误（tonic 把连接失败映射成 UNAVAILABLE，
//! 经 `aite_proto::status` 就是 `retryable: true`），不是 panic，也不是卡死。
//!
//! 同时后台有一条探针在按 1→2→4→…→30s 封顶的退避拨号（无限，不退出），
//! 日志三个事件名：`edge.connecting` / `edge.connected` / `edge.reconnecting`。
//! 探针只负责「记录连接态 + 拨通了就停」；RPC 失败会把它叫起来，
//! 所以 edge 起来之后下一发 RPC 自己就恢复了（tonic 的 Channel 每次请求都会重拨）。
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::gate::{ContractGate, ContractState};
use aite_contracts::{PlatformError, SandboxError};
use aite_proto::pb::edge_status_service_client::EdgeStatusServiceClient;
use aite_proto::pb::platform_service_client::PlatformServiceClient;
use aite_proto::pb::sandbox_service_client::SandboxServiceClient;
use aite_proto::status;
use tonic::transport::{Channel, Endpoint};
use tonic::{Code, Status};

/// 退避上限（秒）。§2.1：1s→2s→…→30s 封顶，无限重试。
const MAX_BACKOFF_SEC: u64 = 30;

pub(crate) struct Link {
    endpoint: Endpoint,
    channel: Channel,
    socket: PathBuf,
    /// 两个方向的 gRPC 消息体上限（附件走 bytes，默认 64MB）
    max_bytes: usize,
    connected: AtomicBool,
    probing: AtomicBool,
    /// 后台补比契约版本的防重入闸（见 verify_contract_detached）
    verifying: AtomicBool,
    /// `contract_version` 门禁（见 `gate.rs`）。探针每连上一次、`status()` 每答上来一次
    /// 都往里记一笔；比出不一致就闸断 platform / sandbox 两条链路。
    gate: ContractGate,
}

impl Link {
    pub(crate) fn open(socket: PathBuf, max_bytes: usize) -> Result<Arc<Self>, PlatformError> {
        // tonic 0.14 的 Endpoint 原生认 unix:// —— 不需要自己写 connector（也就不用多钉依赖）。
        let endpoint =
            Endpoint::from_shared(format!("unix://{}", socket.display())).map_err(|e| {
                PlatformError::new(
                    "bad_config",
                    format!("edge socket 不是合法地址（{}）：{e}", socket.display()),
                    false,
                )
            })?;
        let channel = endpoint.connect_lazy();
        Ok(Arc::new(Self {
            endpoint,
            channel,
            socket,
            max_bytes,
            connected: AtomicBool::new(false),
            probing: AtomicBool::new(false),
            verifying: AtomicBool::new(false),
            gate: ContractGate::new(),
        }))
    }

    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }

    pub(crate) fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub(crate) fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// 一次成功的 RPC 就是「现在连得上」的最强证据 —— 比探针新鲜。
    ///
    /// 不加这条的话：探针一旦进了退避 sleep（最长 30s），哪怕 edge 已经起来、
    /// RPC 一路跑通，`connected()` 也要等下一次拨号才翻 true。RΩ 拿它出健康行
    /// 或做起飞门禁就会误判成「edge 不在」。
    pub(crate) fn note_ok(self: &Arc<Self>) {
        self.connected.store(true, Ordering::SeqCst);
        // 同一个道理用在契约上：连得通、但版本还一次都没比成过（起飞那 4s 没拨通，
        // 或者 edge 是后起的），就立刻补比一次。光靠探针的话要等它从退避里醒来 ——
        // 最长 30s，这段时间里 core 是在「契约没比过」的状态下真干活的。
        if matches!(self.gate.state(), ContractState::Unverified) {
            self.verify_contract_detached();
        }
    }

    /// 后台补比一次契约版本（`note_ok` 与闸门解锁那条路共用）。
    ///
    /// `verifying` 那把闸是防重入的：一轮里几十发 RPC 同时成功，只该有一发真去问。
    fn verify_contract_detached(self: &Arc<Self>) {
        if self.verifying.swap(true, Ordering::SeqCst) {
            return;
        }
        let link = self.clone();
        tokio::spawn(async move {
            link.verify_contract().await;
            link.verifying.store(false, Ordering::SeqCst);
        });
    }

    /// 取 client 是**唯一**发得出 RPC 的路，所以契约闸门就守在这里 ——
    /// 守在每个方法开头的话，19 个方法漏一个就是一条绕过去的暗道。
    pub(crate) fn platform_client(
        self: &Arc<Self>,
    ) -> Result<PlatformServiceClient<Channel>, PlatformError> {
        if let Some(e) = self.gate.platform_error() {
            self.wake_probe_for_recovery();
            return Err(e);
        }
        Ok(PlatformServiceClient::new(self.channel.clone())
            .max_decoding_message_size(self.max_bytes)
            .max_encoding_message_size(self.max_bytes))
    }

    pub(crate) fn sandbox_client(
        self: &Arc<Self>,
    ) -> Result<SandboxServiceClient<Channel>, SandboxError> {
        if let Some(e) = self.gate.sandbox_error() {
            self.wake_probe_for_recovery();
            return Err(e);
        }
        Ok(SandboxServiceClient::new(self.channel.clone())
            .max_decoding_message_size(self.max_bytes)
            .max_encoding_message_size(self.max_bytes))
    }

    /// **不过闸门**：`GetStatus` 正是用来比版本的，拦住它闸门就永远开不了；
    /// `!status` 的健康行也还得问得出来。
    pub(crate) fn status_client(&self) -> EdgeStatusServiceClient<Channel> {
        EdgeStatusServiceClient::new(self.channel.clone())
            .max_decoding_message_size(self.max_bytes)
            .max_encoding_message_size(self.max_bytes)
    }

    /// 闸落着的时候，被拒的那一下顺手把探针叫起来。
    ///
    /// 这是闸门**唯一**的解锁入口：RPC 全被拦在本地，不会再产生 `UNAVAILABLE`，
    /// `note()` 那条叫醒探针的路就断了。探针拨通一次就重比一次版本，对上就解闸。
    fn wake_probe_for_recovery(self: &Arc<Self>) {
        self.probe();
    }

    /// 记一次比对结果（探针连上、以及 `EdgeClient::status()` 答上来时各走一次）。
    pub(crate) fn note_contract(&self, status: &aite_proto::pb::EdgeStatus) {
        self.gate
            .observe(&status.contract_version, &status.version, &self.socket);
    }

    pub(crate) fn contract_state(&self) -> ContractState {
        self.gate.state()
    }

    /// 「连上就比一次」：探针拨通之后发一发 `GetStatus` 比对 `contract_version`。
    ///
    /// 这是起飞那一次之后**唯一**的复查点 —— `docker compose up` 下 core 先起、
    /// M6 的「只重启 edge」、真机手动重启 aite-edge，三条路都只经过这里。
    async fn verify_contract(&self) {
        match self
            .status_client()
            .get_status(aite_proto::pb::GetStatusRequest {})
            .await
        {
            Ok(reply) => self.note_contract(&reply.into_inner()),
            // 连上了但问不出来：不改闸门状态（宁可放行也不误伤），下一发 RPC / 下一轮探针会再试。
            Err(e) => tracing::warn!(
                target: "aite.edge",
                socket = %self.socket.display(),
                error = %e,
                "edge.contract_uncheckable 连上了但 GetStatus 没答上来，这一轮没比成"
            ),
        }
    }

    /// gRPC status → PlatformError（映射是 R0 冻结的，这里只负责顺手叫醒探针）。
    pub(crate) fn platform_error(self: &Arc<Self>, st: &Status) -> PlatformError {
        self.note(st);
        status::platform_error_from_status(st)
    }

    /// gRPC status → SandboxError（同上）。
    pub(crate) fn sandbox_error(self: &Arc<Self>, st: &Status) -> SandboxError {
        self.note(st);
        status::sandbox_error_from_status(st)
    }

    /// core 侧把 pb 转不回 domain：edge 发来的东西不合契约，不是可重试的抖动。
    pub(crate) fn bad_response(detail: impl std::fmt::Display) -> PlatformError {
        PlatformError::new(
            "bad_response",
            format!("edge 返回的内容不合契约：{detail}"),
            false,
        )
    }

    fn note(self: &Arc<Self>, st: &Status) {
        // UNAVAILABLE 既可能是「拨不通」也可能是 edge 自己报的（docker 不可达等），
        // 分不清就让探针去分：它拨一次就知道，拨通了什么都不记。
        if matches!(st.code(), Code::Unavailable | Code::DeadlineExceeded) {
            self.probe();
        }
    }

    /// 起一条后台探针（已经有一条在跑就不重复起）。
    pub(crate) fn probe(self: &Arc<Self>) {
        if self.probing.swap(true, Ordering::SeqCst) {
            return;
        }
        let link = self.clone();
        tokio::spawn(async move {
            let socket = link.socket.display().to_string();
            tracing::info!(socket = %socket, "edge.connecting");
            let mut delay = 1u64;
            let mut announced = false;
            loop {
                match link.endpoint.connect().await {
                    Ok(_probe) => {
                        if !link.connected.swap(true, Ordering::SeqCst) {
                            tracing::info!(socket = %socket, "edge.connected");
                        }
                        // 连上就比一次契约版本（`gate.rs`）—— 起飞那一次之后就靠这里了。
                        link.verify_contract().await;
                        break;
                    }
                    Err(e) => {
                        let was_connected = link.connected.swap(false, Ordering::SeqCst);
                        if was_connected || !announced {
                            tracing::warn!(socket = %socket, error = %e, retry_in_sec = delay, "edge.reconnecting");
                            announced = true;
                        }
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        delay = (delay * 2).min(MAX_BACKOFF_SEC);
                    }
                }
            }
            link.probing.store(false, Ordering::SeqCst);
        });
    }
}
