//! 测试用的**假 edge**：一个真 tonic server，监听 tempdir 里的 UDS，实现
//! `PlatformService` / `SandboxService` / `EdgeStatusService` 三套服务。
//!
//! 形状刻意做成「可脚本化」：`fail(method, code, message)` 让某个方法下次返回指定 status，
//! 这样错误映射（UNAVAILABLE → retryable、NOT_FOUND 两种前缀、INVALID_ARGUMENT）
//! 都能在真 gRPC 上往返一遍验，而不是在 Rust 里对着转换函数空跑。
//!
//! 不依赖 `aite-testing`（R7 的），不碰网络（UDS），不碰 Docker。
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aite_contracts::{CONTRACT_VERSION, feishu_p0};
use aite_proto::pb;
use aite_proto::pb::edge_status_service_server::{EdgeStatusService, EdgeStatusServiceServer};
use aite_proto::pb::platform_service_server::{PlatformService, PlatformServiceServer};
use aite_proto::pb::sandbox_service_server::{SandboxService, SandboxServiceServer};
use chrono::TimeZone;
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tonic::{Code, Request, Response, Status};

/// 假 edge 的共享账本：谁被调过、下次该失败、收到多大的文件。
#[derive(Default)]
pub struct EdgeState {
    pub calls: Vec<String>,
    /// 方法名 → (code, message)；命中就返回这个 status（不清除，要连续失败就放着）
    pub failures: HashMap<String, (Code, String)>,
    pub last_send_file_bytes: usize,
    pub last_put_file: Option<(String, String, usize)>,
    pub last_read_history: Option<pb::ReadHistoryRequest>,
    pub last_exec: Option<pb::ExecCallRequest>,
    /// ReadHistory 要回的消息；默认一条真人消息
    pub history: Option<Vec<pb::HistoryMessage>>,
    pub download_bytes: usize,
}

impl EdgeState {
    fn enter(&mut self, method: &str) -> Result<(), Status> {
        self.calls.push(method.to_string());
        match self.failures.get(method) {
            Some((code, message)) => Err(Status::new(*code, message.clone())),
            None => Ok(()),
        }
    }
}

pub struct FakeEdge {
    dir: tempfile::TempDir,
    socket: PathBuf,
    state: Arc<Mutex<EdgeState>>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl FakeEdge {
    /// 起一个假 edge（socket 在 tempdir 里）。
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("aite-edge.sock");
        let state = Arc::new(Mutex::new(EdgeState::default()));
        let (shutdown, task) = serve(&socket, state.clone()).await;
        Self {
            dir,
            socket,
            state,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    /// 只占一个路径、先不起服务（验「edge 不在时 RPC 是 retryable」）。
    pub fn cold() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("aite-edge.sock");
        Self {
            dir,
            socket,
            state: Arc::new(Mutex::new(EdgeState::default())),
            shutdown: None,
            task: None,
        }
    }

    /// 在同一个路径上把服务起起来（验「edge 起来后自动恢复」）。
    pub async fn warm_up(&mut self) {
        assert!(self.task.is_none(), "已经起过了");
        let (shutdown, task) = serve(&self.socket, self.state.clone()).await;
        self.shutdown = Some(shutdown);
        self.task = Some(task);
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, EdgeState> {
        self.state.lock().expect("edge state 锁")
    }

    pub fn calls(&self) -> Vec<String> {
        self.state().calls.clone()
    }

    pub fn call_count(&self, method: &str) -> usize {
        self.state()
            .calls
            .iter()
            .filter(|m| m.as_str() == method)
            .count()
    }

    pub fn fail(&self, method: &str, code: Code, message: &str) {
        self.state()
            .failures
            .insert(method.to_string(), (code, message.to_string()));
    }

    pub async fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

async fn serve(
    socket: &Path,
    state: Arc<Mutex<EdgeState>>,
) -> (oneshot::Sender<()>, JoinHandle<()>) {
    let listener = UnixListener::bind(socket).expect("bind 假 edge");
    let (shutdown, wait) = oneshot::channel::<()>();
    let max_bytes = 64 * 1024 * 1024;
    let platform = PlatformServiceServer::new(Platform {
        state: state.clone(),
    })
    .max_decoding_message_size(max_bytes)
    .max_encoding_message_size(max_bytes);
    let sandbox = SandboxServiceServer::new(Sandbox {
        state: state.clone(),
    })
    .max_decoding_message_size(max_bytes)
    .max_encoding_message_size(max_bytes);
    let status = EdgeStatusServiceServer::new(EdgeStatusSvc { state });
    let task = tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(platform)
            .add_service(sandbox)
            .add_service(status)
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = wait.await;
            })
            .await;
    });
    // 让 serve 先把 accept 循环跑起来，避免第一发 RPC 撞上空档。
    tokio::task::yield_now().await;
    (shutdown, task)
}

fn sent(message_id: &str, card_id: Option<&str>) -> pb::SendResult {
    pb::SendResult {
        message_id: message_id.to_string(),
        card_id: card_id.map(str::to_string),
    }
}

struct Platform {
    state: Arc<Mutex<EdgeState>>,
}

#[tonic::async_trait]
impl PlatformService for Platform {
    async fn get_capabilities(
        &self,
        _request: Request<pb::GetCapabilitiesRequest>,
    ) -> Result<Response<pb::PlatformCapabilities>, Status> {
        self.state.lock().unwrap().enter("GetCapabilities")?;
        Ok(Response::new(pb::PlatformCapabilities::from(feishu_p0())))
    }

    async fn send_text(
        &self,
        request: Request<pb::OutboundText>,
    ) -> Result<Response<pb::SendResult>, Status> {
        self.state.lock().unwrap().enter("SendText")?;
        let msg = request.into_inner();
        Ok(Response::new(sent(
            &format!("om_text_{}", msg.chat_id),
            None,
        )))
    }

    async fn send_card(
        &self,
        request: Request<pb::SendCardRequest>,
    ) -> Result<Response<pb::SendResult>, Status> {
        self.state.lock().unwrap().enter("SendCard")?;
        let req = request.into_inner();
        let card = req
            .card
            .ok_or_else(|| Status::invalid_argument("没有 card"))?;
        Ok(Response::new(sent(
            &format!("om_card_{}", card.task_no),
            Some(&format!("om_card_{}", card.task_no)),
        )))
    }

    async fn update_card(
        &self,
        request: Request<pb::UpdateCardRequest>,
    ) -> Result<Response<pb::UpdateCardResponse>, Status> {
        self.state.lock().unwrap().enter("UpdateCard")?;
        let req = request.into_inner();
        if req.card.is_none() {
            return Err(Status::invalid_argument("没有 card"));
        }
        Ok(Response::new(pb::UpdateCardResponse {}))
    }

    async fn send_file(
        &self,
        request: Request<pb::OutboundFile>,
    ) -> Result<Response<pb::SendResult>, Status> {
        let mut state = self.state.lock().unwrap();
        state.enter("SendFile")?;
        let msg = request.into_inner();
        state.last_send_file_bytes = msg.data.len();
        Ok(Response::new(sent("om_file", None)))
    }

    async fn add_reaction(
        &self,
        request: Request<pb::AddReactionRequest>,
    ) -> Result<Response<pb::AddReactionResponse>, Status> {
        self.state.lock().unwrap().enter("AddReaction")?;
        let req = request.into_inner();
        if req.kind == pb::ReactionKind::Unspecified as i32 {
            return Err(Status::invalid_argument("kind 未指定"));
        }
        Ok(Response::new(pb::AddReactionResponse {}))
    }

    async fn read_history(
        &self,
        request: Request<pb::ReadHistoryRequest>,
    ) -> Result<Response<pb::ReadHistoryResponse>, Status> {
        let mut state = self.state.lock().unwrap();
        state.enter("ReadHistory")?;
        let req = request.into_inner();
        state.last_read_history = Some(req.clone());
        let messages = state.history.clone().unwrap_or_else(|| {
            vec![pb::HistoryMessage {
                message_id: "om_1".into(),
                sender_id: "ou_zhang".into(),
                sender_kind: "human".into(),
                sender_name: Some("张三".into()),
                text: "这周的退款单据我整理好了".into(),
                thread_id: req.thread_id.clone(),
                created_at: Some(aite_proto::convert::chrono_to_ts(
                    chrono::Utc
                        .with_ymd_and_hms(2026, 9, 9, 9, 0, 0)
                        .single()
                        .expect("固定时间"),
                )),
            }]
        });
        Ok(Response::new(pb::ReadHistoryResponse { messages }))
    }

    async fn read_document(
        &self,
        request: Request<pb::ReadDocumentRequest>,
    ) -> Result<Response<pb::DocumentContent>, Status> {
        self.state.lock().unwrap().enter("ReadDocument")?;
        let req = request.into_inner();
        Ok(Response::new(pb::DocumentContent {
            title: "退款流程 SOP".into(),
            text: "## 步骤\n\n1. 核对单据".into(),
            url: req.url_or_token,
        }))
    }

    async fn download_file(
        &self,
        request: Request<pb::DownloadFileRequest>,
    ) -> Result<Response<pb::DownloadFileResponse>, Status> {
        let mut state = self.state.lock().unwrap();
        state.enter("DownloadFile")?;
        let _req = request.into_inner();
        let size = if state.download_bytes > 0 {
            state.download_bytes
        } else {
            4
        };
        Ok(Response::new(pb::DownloadFileResponse {
            data: vec![7u8; size],
        }))
    }
}

struct Sandbox {
    state: Arc<Mutex<EdgeState>>,
}

#[tonic::async_trait]
impl SandboxService for Sandbox {
    async fn acquire(
        &self,
        request: Request<pb::AcquireRequest>,
    ) -> Result<Response<pb::AcquireResponse>, Status> {
        self.state.lock().unwrap().enter("Acquire")?;
        let req = request.into_inner();
        let spec = req
            .spec
            .ok_or_else(|| Status::invalid_argument("没有 spec"))?;
        Ok(Response::new(pb::AcquireResponse {
            sandbox_id: format!("sb-{}-{}", req.task_id, spec.mem_mb),
        }))
    }

    async fn exec(
        &self,
        request: Request<pb::ExecCallRequest>,
    ) -> Result<Response<pb::ExecResult>, Status> {
        let mut state = self.state.lock().unwrap();
        state.enter("Exec")?;
        let req = request.into_inner();
        state.last_exec = Some(req.clone());
        Ok(Response::new(pb::ExecResult {
            exit_code: 0,
            stdout: "hi\n".into(),
            stderr: String::new(),
            duration_ms: 12,
            truncated: false,
            files_out: vec![pb::FileEntry {
                path: "/work/out.png".into(),
                size: 2048,
            }],
        }))
    }

    async fn put_file(
        &self,
        request: Request<pb::PutFileRequest>,
    ) -> Result<Response<pb::PutFileResponse>, Status> {
        let mut state = self.state.lock().unwrap();
        state.enter("PutFile")?;
        let req = request.into_inner();
        state.last_put_file = Some((req.sandbox_id, req.path, req.data.len()));
        Ok(Response::new(pb::PutFileResponse {}))
    }

    async fn get_file(
        &self,
        request: Request<pb::GetFileRequest>,
    ) -> Result<Response<pb::GetFileResponse>, Status> {
        self.state.lock().unwrap().enter("GetFile")?;
        let _req = request.into_inner();
        Ok(Response::new(pb::GetFileResponse {
            data: b"png-bytes".to_vec(),
        }))
    }

    async fn list_files(
        &self,
        request: Request<pb::ListFilesRequest>,
    ) -> Result<Response<pb::ListFilesResponse>, Status> {
        self.state.lock().unwrap().enter("ListFiles")?;
        let _req = request.into_inner();
        Ok(Response::new(pb::ListFilesResponse {
            paths: vec!["/work/in/a.csv".into(), "/work/out.png".into()],
        }))
    }

    async fn touch(
        &self,
        _request: Request<pb::TouchRequest>,
    ) -> Result<Response<pb::TouchResponse>, Status> {
        self.state.lock().unwrap().enter("Touch")?;
        Ok(Response::new(pb::TouchResponse {}))
    }

    async fn release(
        &self,
        _request: Request<pb::ReleaseRequest>,
    ) -> Result<Response<pb::ReleaseResponse>, Status> {
        self.state.lock().unwrap().enter("Release")?;
        Ok(Response::new(pb::ReleaseResponse {}))
    }

    async fn reap_idle(
        &self,
        request: Request<pb::ReapIdleRequest>,
    ) -> Result<Response<pb::ReapIdleResponse>, Status> {
        self.state.lock().unwrap().enter("ReapIdle")?;
        let req = request.into_inner();
        Ok(Response::new(pb::ReapIdleResponse {
            released: vec![format!("sb-idle-{}", req.idle_sec)],
        }))
    }
}

struct EdgeStatusSvc {
    state: Arc<Mutex<EdgeState>>,
}

#[tonic::async_trait]
impl EdgeStatusService for EdgeStatusSvc {
    async fn get_status(
        &self,
        _request: Request<pb::GetStatusRequest>,
    ) -> Result<Response<pb::EdgeStatus>, Status> {
        self.state.lock().unwrap().enter("GetStatus")?;
        Ok(Response::new(pb::EdgeStatus {
            version: "fake-edge".into(),
            contract_version: CONTRACT_VERSION.to_string(),
            platform_connected: true,
            reconnect_count: 0,
            sandbox_ok: true,
            platform: "fake".into(),
        }))
    }
}
