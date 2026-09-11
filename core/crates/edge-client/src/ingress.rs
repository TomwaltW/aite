//! core 侧的 `IngressService`：edge 每收到一个平台事件就调一次 `HandleEvent`。
//!
//! 契约（`proto/aite/v1/edge.proto` 头注释 + §3.3）：
//! - core 必须在 1s 内返回（只做去重/入库/入队）；edge 用 1s deadline 调用。
//! - 事件转不回 domain（anchor 缺失、枚举 UNSPECIFIED）→ `INVALID_ARGUMENT`，edge 不重推。
//! - handler 失败 → `INTERNAL`（`status_from_ingress_error`），edge 让平台重推，
//!   重复事件由 ControlPlane 靠 event_id 去重。
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use aite_contracts::{EventHandler, IngressError, NormalizedEvent, PlatformError};
use aite_proto::pb;
use aite_proto::pb::ingress_service_server::{IngressService, IngressServiceServer};
use aite_proto::status::status_from_ingress_error;
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

/// 停服务时等 tonic 优雅收尾的上限。
const STOP_GRACE: Duration = Duration::from_secs(5);

struct Running {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// 监听 `core_socket` 的 IngressService（`EdgePlatform::start` 起它，`stop` 关它）。
pub struct IngressServer {
    socket: PathBuf,
    max_bytes: usize,
    running: Mutex<Option<Running>>,
}

impl IngressServer {
    pub(crate) fn new(socket: PathBuf, max_bytes: usize) -> Self {
        Self {
            socket,
            max_bytes,
            running: Mutex::new(None),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn started(&self) -> bool {
        self.running.lock().expect("ingress 锁").is_some()
    }

    /// 绑好 socket 才返回 —— 「返回即表示投递面已就绪」。
    pub async fn start(&self, on_event: EventHandler) -> Result<(), PlatformError> {
        if self.started() {
            tracing::debug!(socket = %self.socket.display(), "ingress.already_started");
            return Ok(());
        }

        if let Some(parent) = self.socket.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                PlatformError::new(
                    "ingress_bind",
                    format!("建 socket 目录失败（{}）：{e}", parent.display()),
                    false,
                )
            })?;
        }
        // 上一条命留下的 socket 文件会让 bind 直接 EADDRINUSE，要清掉；
        // 但「残留」和「另一个 core 正在监听」长得一模一样，无条件删会把活着的
        // 实例静默挤下线（core_socket 是固定路径）。先拨一下：连得上就是有人在用。
        if self.socket.exists() {
            if std::os::unix::net::UnixStream::connect(&self.socket).is_ok() {
                return Err(PlatformError::new(
                    "ingress_bind",
                    format!(
                        "{} 已经有人在监听 —— 多半是另一个 core 进程还活着。                         确认它退干净了再起，别把它的事件流抢过来。",
                        self.socket.display()
                    ),
                    false,
                ));
            }
            let _ = std::fs::remove_file(&self.socket);
        }

        let listener = UnixListener::bind(&self.socket).map_err(|e| {
            PlatformError::new(
                "ingress_bind",
                format!("监听 {} 失败：{e}", self.socket.display()),
                false,
            )
        })?;

        let service = IngressServiceServer::new(Ingress { on_event })
            .max_decoding_message_size(self.max_bytes)
            .max_encoding_message_size(self.max_bytes);
        let (shutdown, wait) = oneshot::channel::<()>();
        let socket = self.socket.clone();
        let task = tokio::spawn(async move {
            let serve = Server::builder()
                .add_service(service)
                .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                    let _ = wait.await;
                });
            if let Err(e) = serve.await {
                tracing::warn!(socket = %socket.display(), error = %e, "ingress.serve_failed");
            }
        });

        *self.running.lock().expect("ingress 锁") = Some(Running { shutdown, task });
        tracing::info!(socket = %self.socket.display(), "ingress.listening");
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), PlatformError> {
        let Some(running) = self.running.lock().expect("ingress 锁").take() else {
            return Ok(());
        };
        let _ = running.shutdown.send(());
        match tokio::time::timeout(STOP_GRACE, running.task).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "ingress.stop_join_failed"),
            Err(_) => tracing::warn!("ingress.stop_timeout"),
        }
        // 自己建的 socket 文件自己收掉，下次起飞不用靠「删残留」那条兜。
        let _ = std::fs::remove_file(&self.socket);
        tracing::info!(socket = %self.socket.display(), "ingress.stopped");
        Ok(())
    }
}

struct Ingress {
    on_event: EventHandler,
}

#[tonic::async_trait]
impl IngressService for Ingress {
    async fn handle_event(
        &self,
        request: Request<pb::NormalizedEvent>,
    ) -> Result<Response<pb::HandleEventResponse>, Status> {
        // 走 aite-proto::status 的映射函数，不在这里自己拼前缀 —— R0 哪天改了
        // `invalid_event:` 这个约定，这里要跟着变（派单点名禁止复制一份映射）。
        let event = NormalizedEvent::try_from(request.into_inner())
            .map_err(|e| status_from_ingress_error(&IngressError::Invalid(e.to_string())))?;
        let event_id = event.event_id.clone();
        match (self.on_event)(event).await {
            Ok(()) => Ok(Response::new(pb::HandleEventResponse {})),
            Err(e) => {
                tracing::warn!(event_id = %event_id, error = %e, "ingress.handle_failed");
                Err(status_from_ingress_error(&e))
            }
        }
    }
}
