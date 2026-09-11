//! tests/ 的私有替身与 fixture（对应旧 `tests/gateway/conftest.py`）。
//!
//! §3.2 的测试替身规则：需要 FakePlatform / FakeSandbox 就在自己 crate 的 tests/ 下私写，
//! **不依赖 `aite-testing`**（那是 R7 的，并行期间还是空壳）。「重复远比冲突便宜」。
//!
//! 两个替身都照 `ports.rs` 的签名逐字实现，并且刻意保留了真实现的两条关键脾气：
//! - `FakeGatewayPlatform::read_history` **不做** sender_kind 过滤（契约注释写明过滤归
//!   Gateway 的 read_group_history 工具）—— 替身要是先过滤了，「过滤是 Gateway 的责任」
//!   这条就测不出来了。
//! - `FakeGatewaySandbox::put_file` 照样走一遍 `/work` 路径闸（真实现的 require_file_path）。
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aite_contracts::{
    ChecklistCard, DocumentContent, EventHandler, ExecRequest, ExecResult, FileEntry,
    HistoryMessage, OutboundFile, OutboundText, PlatformCapabilities, PlatformError, PlatformPort,
    ReactionKind, SandboxConfig, SandboxError, SandboxErrorKind, SandboxPort, SandboxSpec,
    SendResult, ToolCallRequest, ToolContext, ToolErrorCode, ToolGateway, ToolResult, feishu_p0,
};
use aite_gateway::P0ToolGateway;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Map, Value};

pub const TASK_ID: &str = "task-b5";
/// Task.session_token 是随机 32 hex
pub const SESSION_TOKEN: &str = "0123456789abcdef0123456789abcdef";
pub const CHAT_ID: &str = "oc_chat_1";
pub const THREAD_ID: &str = "om_root_1";
pub const ATTACHMENTS_MESSAGE_ID: &str = "om_msg_with_file";
pub const CSV_BYTES: &[u8] = "月份,销量\n2026-01,120\n".as_bytes();

pub fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 9, 9, minute, 0).unwrap()
}

// --------------------------------------------------------------------- 平台替身

#[derive(Debug, Clone, PartialEq)]
pub enum PlatformCall {
    ReadHistory {
        chat_id: String,
        limit: u32,
        thread_id: Option<String>,
    },
    ReadDocument {
        url_or_token: String,
    },
    DownloadFile {
        message_id: String,
        file_key: String,
    },
}

#[derive(Default)]
struct PlatformState {
    history: Vec<HistoryMessage>,
    documents: HashMap<String, DocumentContent>,
    files: HashMap<String, Vec<u8>>,
    read_history_error: Option<PlatformError>,
    read_document_error: Option<PlatformError>,
    download_error: Option<PlatformError>,
    delay: Duration,
    calls: Vec<PlatformCall>,
}

/// PlatformPort 的私有假实现（签名逐字对齐 `ports.rs`）。
pub struct FakeGatewayPlatform {
    state: Mutex<PlatformState>,
}

impl FakeGatewayPlatform {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(PlatformState::default()),
        }
    }

    pub fn with_history(self, rows: Vec<HistoryMessage>) -> Self {
        self.state.lock().unwrap().history = rows;
        self
    }

    pub fn with_document(self, key: &str, doc: DocumentContent) -> Self {
        self.state
            .lock()
            .unwrap()
            .documents
            .insert(key.to_string(), doc);
        self
    }

    pub fn with_file(self, file_key: &str, data: &[u8]) -> Self {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(file_key.to_string(), data.to_vec());
        self
    }

    pub fn put_file_fixture(&self, file_key: &str, data: &[u8]) {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(file_key.to_string(), data.to_vec());
    }

    pub fn file(&self, file_key: &str) -> Vec<u8> {
        self.state.lock().unwrap().files[file_key].clone()
    }

    pub fn set_history(&self, rows: Vec<HistoryMessage>) {
        self.state.lock().unwrap().history = rows;
    }

    pub fn history(&self) -> Vec<HistoryMessage> {
        self.state.lock().unwrap().history.clone()
    }

    pub fn fail_read_history(&self, error: PlatformError) {
        self.state.lock().unwrap().read_history_error = Some(error);
    }

    pub fn fail_download(&self, error: PlatformError) {
        self.state.lock().unwrap().download_error = Some(error);
    }

    pub fn set_delay(&self, delay: Duration) {
        self.state.lock().unwrap().delay = delay;
    }

    pub fn calls(&self) -> Vec<PlatformCall> {
        self.state.lock().unwrap().calls.clone()
    }

    pub fn last_call(&self) -> Option<PlatformCall> {
        self.state.lock().unwrap().calls.last().cloned()
    }

    async fn enter(&self, call: PlatformCall) {
        let delay = {
            let mut state = self.state.lock().unwrap();
            state.calls.push(call);
            state.delay
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

#[async_trait]
impl PlatformPort for FakeGatewayPlatform {
    fn capabilities(&self) -> PlatformCapabilities {
        feishu_p0()
    }

    async fn start(&self, _on_event: EventHandler) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn stop(&self) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn send_text(&self, _msg: &OutboundText) -> Result<SendResult, PlatformError> {
        Ok(SendResult {
            message_id: "om_sent".into(),
            card_id: None,
        })
    }

    async fn send_card(
        &self,
        _chat_id: &str,
        _reply_to: Option<&str>,
        _card: &ChecklistCard,
    ) -> Result<SendResult, PlatformError> {
        Ok(SendResult {
            message_id: "om_card".into(),
            card_id: Some("om_card".into()),
        })
    }

    async fn update_card(
        &self,
        _card_id: &str,
        _card: &ChecklistCard,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn send_file(&self, _msg: &OutboundFile) -> Result<SendResult, PlatformError> {
        Ok(SendResult {
            message_id: "om_file".into(),
            card_id: None,
        })
    }

    async fn add_reaction(
        &self,
        _message_id: &str,
        _kind: ReactionKind,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    /// 按 thread 过滤 + 取最后 limit 条；**不按 sender_kind 过滤**（那是 Gateway 的活）。
    async fn read_history(
        &self,
        chat_id: &str,
        limit: u32,
        thread_id: Option<&str>,
    ) -> Result<Vec<HistoryMessage>, PlatformError> {
        self.enter(PlatformCall::ReadHistory {
            chat_id: chat_id.to_string(),
            limit,
            thread_id: thread_id.map(str::to_string),
        })
        .await;
        let state = self.state.lock().unwrap();
        if let Some(error) = &state.read_history_error {
            return Err(error.clone());
        }
        let mut rows: Vec<HistoryMessage> = state.history.clone();
        if let Some(thread) = thread_id {
            rows.retain(|m| m.thread_id.as_deref() == Some(thread));
        }
        let keep = limit as usize;
        if rows.len() > keep {
            rows = rows.split_off(rows.len() - keep);
        }
        Ok(rows)
    }

    async fn read_document(&self, url_or_token: &str) -> Result<DocumentContent, PlatformError> {
        self.enter(PlatformCall::ReadDocument {
            url_or_token: url_or_token.to_string(),
        })
        .await;
        let state = self.state.lock().unwrap();
        if let Some(error) = &state.read_document_error {
            return Err(error.clone());
        }
        state
            .documents
            .get(url_or_token)
            .cloned()
            .ok_or_else(|| PlatformError::new("404", format!("文档不存在：{url_or_token}"), false))
    }

    async fn download_file(
        &self,
        message_id: &str,
        file_key: &str,
    ) -> Result<Vec<u8>, PlatformError> {
        self.enter(PlatformCall::DownloadFile {
            message_id: message_id.to_string(),
            file_key: file_key.to_string(),
        })
        .await;
        let state = self.state.lock().unwrap();
        if let Some(error) = &state.download_error {
            return Err(error.clone());
        }
        state
            .files
            .get(file_key)
            .cloned()
            .ok_or_else(|| PlatformError::new("404", format!("附件不存在：{file_key}"), false))
    }
}

// --------------------------------------------------------------------- 沙箱替身

#[derive(Default)]
struct SandboxState {
    /// sandbox_id → （路径 → 内容）；BTreeMap 让 list_files 天然有序
    trees: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    tasks: HashMap<String, String>,
    released: Vec<String>,
    touched: Vec<String>,
    exec_requests: Vec<(String, ExecRequest)>,
    seq: u32,
    acquire_error: Option<SandboxError>,
    exec_error: Option<SandboxError>,
    exec_result: Option<ExecResult>,
    exec_delay: Duration,
}

/// SandboxPort 的私有假实现，/work 全在内存里。
pub struct FakeGatewaySandbox {
    state: Mutex<SandboxState>,
}

impl FakeGatewaySandbox {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SandboxState::default()),
        }
    }

    pub fn fail_acquire(&self, error: SandboxError) {
        self.state.lock().unwrap().acquire_error = Some(error);
    }

    pub fn fail_exec(&self, error: SandboxError) {
        self.state.lock().unwrap().exec_error = Some(error);
    }

    pub fn set_exec_result(&self, result: ExecResult) {
        self.state.lock().unwrap().exec_result = Some(result);
    }

    pub fn set_exec_delay(&self, delay: Duration) {
        self.state.lock().unwrap().exec_delay = delay;
    }

    pub fn exec_requests(&self) -> Vec<(String, ExecRequest)> {
        self.state.lock().unwrap().exec_requests.clone()
    }

    pub fn released(&self) -> Vec<String> {
        self.state.lock().unwrap().released.clone()
    }

    pub fn touched(&self) -> Vec<String> {
        self.state.lock().unwrap().touched.clone()
    }

    pub fn box_count(&self) -> usize {
        self.state.lock().unwrap().trees.len()
    }

    pub fn files_of(&self, sandbox_id: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .trees
            .get(sandbox_id)
            .map(|tree| tree.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// 真实现（R2 的 Go 沙箱）那道 `/work` 闸：路径必须在 /work 下、不能是 /work 本身、不许有 `..`。
fn require_file_path(path: &str) -> Result<String, SandboxError> {
    if !path.starts_with('/') {
        return Err(SandboxError::new(
            SandboxErrorKind::InvalidPath,
            format!("路径必须是绝对路径：{path}"),
        ));
    }
    let mut stack: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return Err(SandboxError::new(
                        SandboxErrorKind::InvalidPath,
                        format!("路径越过了根目录：{path}"),
                    ));
                }
            }
            other => stack.push(other),
        }
    }
    let normalized = format!("/{}", stack.join("/"));
    if normalized == "/work" {
        return Err(SandboxError::new(
            SandboxErrorKind::InvalidPath,
            "/work 是目录，不是文件",
        ));
    }
    if !normalized.starts_with("/work/") {
        return Err(SandboxError::new(
            SandboxErrorKind::InvalidPath,
            format!("路径不在 /work 下：{path}"),
        ));
    }
    Ok(normalized)
}

#[async_trait]
impl SandboxPort for FakeGatewaySandbox {
    async fn acquire(&self, task_id: &str, _spec: &SandboxSpec) -> Result<String, SandboxError> {
        // 让出一次：真 DockerSandbox 的 acquire 要拉镜像、起容器、探路，中间全是挂起点。
        // 替身如果一路同步跑完，`concurrent_run_python_shares_one_sandbox` 就没有交错窗口，
        // 把 Inner::acquire_sandbox 的 per-task 双检锁整段删掉那条测试照样绿 —— 它就白写了。
        tokio::task::yield_now().await;
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.acquire_error {
            return Err(error.clone());
        }
        state.seq += 1;
        let sandbox_id = format!("fake-sbx-{}", state.seq);
        state.trees.insert(sandbox_id.clone(), BTreeMap::new());
        state.tasks.insert(sandbox_id.clone(), task_id.to_string());
        Ok(sandbox_id)
    }

    async fn exec(&self, sandbox_id: &str, req: &ExecRequest) -> Result<ExecResult, SandboxError> {
        let delay = {
            let mut state = self.state.lock().unwrap();
            state
                .exec_requests
                .push((sandbox_id.to_string(), req.clone()));
            state.exec_delay
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.exec_error {
            return Err(error.clone());
        }
        if let Some(result) = &state.exec_result {
            return Ok(result.clone());
        }
        state
            .trees
            .entry(sandbox_id.to_string())
            .or_default()
            .insert("/work/out.txt".to_string(), b"fake".to_vec());
        Ok(ExecResult {
            exit_code: 0,
            stdout: "fake stdout".into(),
            stderr: String::new(),
            duration_ms: 7,
            truncated: false,
            files_out: vec![FileEntry {
                path: "/work/out.txt".into(),
                size: 4,
            }],
        })
    }

    async fn put_file(
        &self,
        sandbox_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), SandboxError> {
        let target = require_file_path(path)?;
        self.state
            .lock()
            .unwrap()
            .trees
            .entry(sandbox_id.to_string())
            .or_default()
            .insert(target, data.to_vec());
        Ok(())
    }

    async fn get_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError> {
        let target = require_file_path(path)?;
        self.state
            .lock()
            .unwrap()
            .trees
            .get(sandbox_id)
            .and_then(|tree| tree.get(&target))
            .cloned()
            .ok_or_else(|| {
                SandboxError::new(SandboxErrorKind::FileNotFound, format!("没有 {path}"))
            })
    }

    async fn list_files(&self, sandbox_id: &str) -> Result<Vec<String>, SandboxError> {
        Ok(self.files_of(sandbox_id))
    }

    async fn touch(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.state
            .lock()
            .unwrap()
            .touched
            .push(sandbox_id.to_string());
        Ok(())
    }

    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        let mut state = self.state.lock().unwrap();
        state.trees.remove(sandbox_id);
        state.tasks.remove(sandbox_id);
        state.released.push(sandbox_id.to_string());
        Ok(())
    }

    async fn reap_idle(&self, _idle_sec: u32) -> Result<Vec<String>, SandboxError> {
        let victims: Vec<String> = self.state.lock().unwrap().trees.keys().cloned().collect();
        for sandbox_id in &victims {
            self.release(sandbox_id).await?;
        }
        Ok(victims)
    }
}

// --------------------------------------------------------------------- fixtures

#[allow(clippy::too_many_arguments)]
fn message(
    index: u32,
    message_id: &str,
    sender_id: &str,
    sender_kind: &str,
    sender_name: Option<&str>,
    text: &str,
    thread_id: Option<&str>,
) -> HistoryMessage {
    HistoryMessage {
        message_id: message_id.to_string(),
        sender_id: sender_id.to_string(),
        sender_kind: sender_kind.to_string(),
        sender_name: sender_name.map(str::to_string),
        text: text.to_string(),
        thread_id: thread_id.map(str::to_string),
        created_at: at(index),
    }
}

/// 故意混进 bot / app / system —— read_group_history 必须把它们过滤掉。
pub fn history_rows() -> Vec<HistoryMessage> {
    let thread = Some(THREAD_ID);
    vec![
        message(
            0,
            "om_1",
            "ou_zhang",
            "human",
            Some("张三"),
            "这周的退款单据我整理好了",
            thread,
        ),
        message(
            1,
            "om_2",
            "cli_ci",
            "bot",
            Some("CI 机器人"),
            "构建 #481 成功",
            thread,
        ),
        message(
            2,
            "om_3",
            "ou_li",
            "human",
            Some("李四"),
            "麻烦按月度画个趋势图",
            thread,
        ),
        message(
            3,
            "om_4",
            "cli_aite",
            "app",
            Some("Aite"),
            "已收到，正在处理",
            thread,
        ),
        message(
            4,
            "om_5",
            "sys",
            "system",
            None,
            "张三 邀请 李四 加入了群聊",
            None,
        ),
        message(
            5,
            "om_6",
            "ou_wang",
            "human",
            Some("王五"),
            "顺手把 Q3 也带上",
            None,
        ),
    ]
}

pub struct Fixture {
    pub platform: Arc<FakeGatewayPlatform>,
    pub sandbox: Arc<FakeGatewaySandbox>,
    pub ctx: ToolContext,
}

pub fn fixture() -> Fixture {
    let platform = FakeGatewayPlatform::new()
        .with_history(history_rows())
        .with_document(
            "https://feishu.cn/docx/abc",
            DocumentContent {
                title: "退款流程 SOP".into(),
                text: "## 步骤\n\n1. 核对单据\n2. 走审批".into(),
                url: "https://feishu.cn/docx/abc".into(),
            },
        )
        .with_file("file_v3_csv", CSV_BYTES);
    Fixture {
        platform: Arc::new(platform),
        sandbox: Arc::new(FakeGatewaySandbox::new()),
        ctx: ToolContext {
            tenant_id: "default".into(),
            workspace_id: "cli_app".into(),
            chat_id: CHAT_ID.into(),
            session_id: "sess-1".into(),
            task_id: TASK_ID.into(),
            session_token: SESSION_TOKEN.into(),
            thread_id: Some(THREAD_ID.into()),
            attachments_message_id: Some(ATTACHMENTS_MESSAGE_ID.into()),
        },
    }
}

pub fn spec() -> SandboxSpec {
    let cfg = SandboxConfig::default();
    SandboxSpec {
        image: cfg.image,
        cpu: cfg.cpu,
        mem_mb: cfg.mem_mb,
        ..SandboxSpec::new("ignored")
    }
}

impl Fixture {
    /// 两个面都接上，什么都没登记。
    pub fn raw(&self) -> P0ToolGateway {
        P0ToolGateway::new(self.platform.clone(), self.sandbox.clone(), spec())
    }

    /// 已经登记好 session_token 的 Gateway。
    pub fn gateway(&self) -> P0ToolGateway {
        let gateway = self.raw();
        register(&gateway);
        gateway
    }

    pub fn without_platform(&self) -> P0ToolGateway {
        let gateway = P0ToolGateway::with_optional_ports(None, Some(self.sandbox.clone()), spec());
        register(&gateway);
        gateway
    }

    pub fn without_sandbox(&self) -> P0ToolGateway {
        let gateway = P0ToolGateway::with_optional_ports(Some(self.platform.clone()), None, spec());
        register(&gateway);
        gateway
    }

    /// 改一个字段的 ctx（旧 `ctx.model_copy(update=...)`）。
    pub fn ctx_with_token(&self, token: &str) -> ToolContext {
        ToolContext {
            session_token: token.to_string(),
            ..self.ctx.clone()
        }
    }

    pub fn ctx_with_task(&self, task_id: &str) -> ToolContext {
        ToolContext {
            task_id: task_id.to_string(),
            ..self.ctx.clone()
        }
    }
}

pub fn register(gateway: &P0ToolGateway) {
    gateway.register_task(TASK_ID, SESSION_TOKEN);
}

pub fn req(name: &str) -> ToolCallRequest {
    ToolCallRequest {
        call_id: "call-1".into(),
        name: name.to_string(),
        arguments: Map::new(),
    }
}

pub fn req_args(name: &str, arguments: Value) -> ToolCallRequest {
    ToolCallRequest {
        call_id: "call-1".into(),
        name: name.to_string(),
        arguments: match arguments {
            Value::Object(map) => map,
            _ => Map::new(),
        },
    }
}

pub fn assert_failed(result: &ToolResult, code: ToolErrorCode) {
    assert!(!result.ok, "期望失败，实际 ok=true：{result:?}");
    let error = result.error.as_ref().expect("失败必须带 error");
    assert_eq!(error.code, code, "错误码不对：{error:?}");
    assert!(!error.message.is_empty(), "错误消息不能空");
    assert!(
        !result.content.is_empty(),
        "失败时 content 也要有话说，模型可能只读 content"
    );
    assert_eq!(result.call_id, "call-1");
}

pub fn error_code(result: &ToolResult) -> Option<ToolErrorCode> {
    result.error.as_ref().map(|e| e.code)
}

pub fn data_of(result: &ToolResult) -> Map<String, Value> {
    result.data.clone().expect("成功结果该带 data")
}
