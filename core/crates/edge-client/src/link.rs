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

    pub(crate) fn platform_client(&self) -> PlatformServiceClient<Channel> {
        PlatformServiceClient::new(self.channel.clone())
            .max_decoding_message_size(self.max_bytes)
            .max_encoding_message_size(self.max_bytes)
    }

    pub(crate) fn sandbox_client(&self) -> SandboxServiceClient<Channel> {
        SandboxServiceClient::new(self.channel.clone())
            .max_decoding_message_size(self.max_bytes)
            .max_encoding_message_size(self.max_bytes)
    }

    pub(crate) fn status_client(&self) -> EdgeStatusServiceClient<Channel> {
        EdgeStatusServiceClient::new(self.channel.clone())
            .max_decoding_message_size(self.max_bytes)
            .max_encoding_message_size(self.max_bytes)
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
