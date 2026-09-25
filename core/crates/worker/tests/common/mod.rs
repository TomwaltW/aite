//! R5 私有的测试替身（spec §3.2：并行期间不依赖 aite-testing，重复远比冲突便宜）。
//!
//! 对齐旧 `tests/worker/worker_fakes.py` + `conftest.py`。与 Python 版的一个结构性差异：
//! 那边的 worker 测试一律走完整链路（ControlPlane 收事件建 task → run_pending），
//! 这里没有 ControlPlane（归 R4），所以 `Harness` 把控制面在**开跑之前**做的那几件事
//! 自己做了：建 session / task、落 transcript turn、写 `task_created` 与
//! `event_received` 两条证据；steer 则由 `push_steer()` 复刻 `_continue_session`
//! （落 turn + 排队 + 写 `route=steer` 的证据）。断言面还是替身上的记录。
#![allow(dead_code)]

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aite_contracts::{
    AiteConfig, Anchor, Attachment, AttachmentKind, CardActionKind, ChecklistCard, DocumentContent,
    EventHandler, EvidenceError, EvidenceEvent, EvidenceKind, EvidenceWriter, GENESIS,
    HistoryMessage, Message, ModelError, ModelPort, ModelProvider, ModelTurn, OutboundFile,
    OutboundText, PlatformCapabilities, PlatformError, PlatformPort, ReactionKind, Role, RunHooks,
    SandboxError, SandboxErrorKind, SandboxPort, SandboxSpec, SendResult, Session, SessionKind,
    SessionStatus, SessionStore, StoreError, Task, TaskStatus, TaskWorker, ToolCallRequest,
    ToolContext, ToolError, ToolErrorCode, ToolGateway, ToolResult, ToolSpec, Turn, TurnRole,
    Usage, canonical_json, chain_hash, gateway_tools, payload_hash_of,
};
use aite_worker::{AgentWorker, Clock, Sleeper, WorkerDeps};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Map, Value, json};

pub const CHAT_ID: &str = "oc_chat";
pub const ROOT: &str = "om_1";

// ---- 假钟 ---------------------------------------------------------------

/// 可手动推进的单调钟。worker 的 wall_sec 与卡片 500ms 窗口都读它。
#[derive(Clone)]
pub struct FakeClock(Arc<Mutex<f64>>);

impl FakeClock {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(1000.0)))
    }

    pub fn now(&self) -> f64 {
        *self.0.lock().expect("clock")
    }

    pub fn advance(&self, dt: f64) {
        *self.0.lock().expect("clock") += dt;
    }

    pub fn as_clock(&self) -> Clock {
        let inner = self.0.clone();
        Arc::new(move || *inner.lock().expect("clock"))
    }

    /// 替掉真 sleep：不真等，只把钟拨过去。
    pub fn as_sleeper(&self) -> Sleeper {
        let inner = self.0.clone();
        Arc::new(move |dt: f64| {
            *inner.lock().expect("clock") += dt;
            Box::pin(async {}) as Pin<Box<dyn Future<Output = ()> + Send>>
        })
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new()
    }
}

// ---- 假平台 -------------------------------------------------------------

#[derive(Default)]
struct PlatformState {
    texts: Vec<OutboundText>,
    cards: Vec<(String, Option<String>, ChecklistCard)>,
    card_updates: Vec<(String, ChecklistCard)>,
    files: Vec<OutboundFile>,
    reactions: Vec<(String, ReactionKind)>,
    history: Vec<HistoryMessage>,
    downloads: Vec<(String, String)>,
    n: u64,
}

/// 记录所有出站调用。断言全部对着这些 list 做。
#[derive(Default)]
pub struct FakePlatform {
    state: Mutex<PlatformState>,
}

impl FakePlatform {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn set_history(&self, rows: Vec<HistoryMessage>) {
        self.state.lock().expect("platform").history = rows;
    }

    pub fn texts(&self) -> Vec<OutboundText> {
        self.state.lock().expect("platform").texts.clone()
    }

    pub fn last_text(&self) -> String {
        self.texts()
            .last()
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    pub fn cards(&self) -> Vec<(String, Option<String>, ChecklistCard)> {
        self.state.lock().expect("platform").cards.clone()
    }

    pub fn card_updates(&self) -> Vec<(String, ChecklistCard)> {
        self.state.lock().expect("platform").card_updates.clone()
    }

    pub fn last_card(&self) -> ChecklistCard {
        self.card_updates()
            .last()
            .map(|(_, c)| c.clone())
            .expect("至少要有一次 update_card")
    }

    pub fn files(&self) -> Vec<OutboundFile> {
        self.state.lock().expect("platform").files.clone()
    }

    pub fn downloads(&self) -> Vec<(String, String)> {
        self.state.lock().expect("platform").downloads.clone()
    }

    fn next_id(&self, prefix: &str) -> String {
        let mut st = self.state.lock().expect("platform");
        st.n += 1;
        format!("{prefix}_{}", st.n)
    }
}

#[async_trait]
impl PlatformPort for FakePlatform {
    fn capabilities(&self) -> PlatformCapabilities {
        let mut caps = aite_contracts::feishu_p0();
        caps.platform = "fake".into();
        caps.supports_passive_listen = true;
        caps
    }

    async fn start(&self, _on_event: EventHandler) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn stop(&self) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn send_text(&self, msg: &OutboundText) -> Result<SendResult, PlatformError> {
        self.state.lock().expect("platform").texts.push(msg.clone());
        Ok(SendResult {
            message_id: self.next_id("om_text"),
            card_id: None,
        })
    }

    async fn send_card(
        &self,
        chat_id: &str,
        reply_to: Option<&str>,
        card: &ChecklistCard,
    ) -> Result<SendResult, PlatformError> {
        self.state.lock().expect("platform").cards.push((
            chat_id.to_string(),
            reply_to.map(str::to_string),
            card.clone(),
        ));
        let mid = self.next_id("om_card");
        Ok(SendResult {
            message_id: mid.clone(),
            card_id: Some(mid),
        })
    }

    async fn update_card(&self, card_id: &str, card: &ChecklistCard) -> Result<(), PlatformError> {
        self.state
            .lock()
            .expect("platform")
            .card_updates
            .push((card_id.to_string(), card.clone()));
        Ok(())
    }

    async fn send_file(&self, msg: &OutboundFile) -> Result<SendResult, PlatformError> {
        self.state.lock().expect("platform").files.push(msg.clone());
        Ok(SendResult {
            message_id: self.next_id("om_file"),
            card_id: None,
        })
    }

    async fn add_reaction(
        &self,
        message_id: &str,
        kind: ReactionKind,
    ) -> Result<(), PlatformError> {
        self.state
            .lock()
            .expect("platform")
            .reactions
            .push((message_id.to_string(), kind));
        Ok(())
    }

    async fn read_history(
        &self,
        _chat_id: &str,
        limit: u32,
        _thread_id: Option<&str>,
    ) -> Result<Vec<HistoryMessage>, PlatformError> {
        let st = self.state.lock().expect("platform");
        let start = st.history.len().saturating_sub(limit as usize);
        Ok(st.history[start..].to_vec())
    }

    async fn read_document(&self, url_or_token: &str) -> Result<DocumentContent, PlatformError> {
        Ok(DocumentContent {
            title: String::new(),
            text: String::new(),
            url: url_or_token.to_string(),
        })
    }

    async fn download_file(
        &self,
        message_id: &str,
        file_key: &str,
    ) -> Result<Vec<u8>, PlatformError> {
        self.state
            .lock()
            .expect("platform")
            .downloads
            .push((message_id.to_string(), file_key.to_string()));
        Ok(Vec::new())
    }
}

// ---- 脚本化模型 ---------------------------------------------------------

pub type OnCall = Arc<dyn Fn(usize) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

struct ModelState {
    script: Vec<ModelTurn>,
    fail_times: u32,
    calls: Vec<Vec<Message>>,
    tool_catalogs: Vec<Vec<ToolSpec>>,
}

/// 按脚本出牌。脚本用完后：有 repeat 就一直出 repeat（死循环测试），否则报错。
pub struct ScriptedModel {
    name: String,
    repeat: Option<ModelTurn>,
    clock: Option<FakeClock>,
    step_seconds: f64,
    /// on_call(step_index)：在这一步开始前做点别的事，比如往队列里塞一条 steer 消息
    on_call: Option<OnCall>,
    state: Mutex<ModelState>,
}

impl ScriptedModel {
    pub fn new(script: Vec<ModelTurn>) -> Self {
        Self {
            name: "scripted".to_string(),
            repeat: None,
            clock: None,
            step_seconds: 0.0,
            on_call: None,
            state: Mutex::new(ModelState {
                script,
                fail_times: 0,
                calls: Vec::new(),
                tool_catalogs: Vec::new(),
            }),
        }
    }

    pub fn repeating(turn: ModelTurn) -> Self {
        let mut m = Self::new(Vec::new());
        m.repeat = Some(turn);
        m
    }

    pub fn with_clock(mut self, clock: FakeClock, step_seconds: f64) -> Self {
        self.clock = Some(clock);
        self.step_seconds = step_seconds;
        self
    }

    pub fn with_repeat(mut self, turn: ModelTurn) -> Self {
        self.repeat = Some(turn);
        self
    }

    pub fn with_fail_times(self, n: u32) -> Self {
        self.state.lock().expect("model").fail_times = n;
        self
    }

    pub fn with_on_call(mut self, hook: OnCall) -> Self {
        self.on_call = Some(hook);
        self
    }

    pub fn calls(&self) -> Vec<Vec<Message>> {
        self.state.lock().expect("model").calls.clone()
    }

    pub fn call_count(&self) -> usize {
        self.state.lock().expect("model").calls.len()
    }

    /// 第 i 次调用时模型看到的消息列表。
    pub fn call(&self, i: usize) -> Vec<Message> {
        self.calls().get(i).cloned().unwrap_or_default()
    }

    pub fn last_call(&self) -> Vec<Message> {
        self.calls().last().cloned().unwrap_or_default()
    }

    pub fn tool_catalog(&self, i: usize) -> Vec<ToolSpec> {
        self.state
            .lock()
            .expect("model")
            .tool_catalogs
            .get(i)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl ModelPort for ScriptedModel {
    fn name(&self) -> String {
        self.name.clone()
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        _max_tokens: u32,
        _temperature: f32,
    ) -> Result<ModelTurn, ModelError> {
        let step = self.state.lock().expect("model").calls.len();
        if let Some(hook) = &self.on_call {
            hook(step).await;
        }
        let next = {
            let mut st = self.state.lock().expect("model");
            st.calls.push(messages.to_vec());
            st.tool_catalogs.push(tools.to_vec());
            if st.fail_times > 0 {
                st.fail_times -= 1;
                None
            } else if !st.script.is_empty() {
                Some(Some(st.script.remove(0)))
            } else {
                Some(self.repeat.clone())
            }
        };
        if let Some(clock) = &self.clock
            && self.step_seconds != 0.0
        {
            clock.advance(self.step_seconds);
        }
        match next {
            None => Err(ModelError::Upstream("model 5xx".into())),
            Some(Some(turn)) => Ok(turn),
            Some(None) => Err(ModelError::Upstream(format!(
                "脚本已用完，但模型被第 {} 次调用",
                step + 1
            ))),
        }
    }
}

// ---- 假沙箱 -------------------------------------------------------------

#[derive(Default)]
struct SandboxState {
    files: HashMap<String, Vec<u8>>,
    acquired: Vec<String>,
    released: Vec<String>,
    get_file_calls: Vec<(String, String)>,
}

#[derive(Default)]
pub struct FakeSandbox {
    state: Mutex<SandboxState>,
}

impl FakeSandbox {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn put(&self, path: &str, data: &[u8]) {
        self.state
            .lock()
            .expect("sandbox")
            .files
            .insert(path.to_string(), data.to_vec());
    }

    pub fn acquired(&self) -> Vec<String> {
        self.state.lock().expect("sandbox").acquired.clone()
    }

    pub fn released(&self) -> Vec<String> {
        self.state.lock().expect("sandbox").released.clone()
    }

    pub fn get_file_calls(&self) -> Vec<(String, String)> {
        self.state.lock().expect("sandbox").get_file_calls.clone()
    }
}

#[async_trait]
impl SandboxPort for FakeSandbox {
    async fn acquire(&self, task_id: &str, _spec: &SandboxSpec) -> Result<String, SandboxError> {
        let sid = format!("sb_{task_id}");
        self.state
            .lock()
            .expect("sandbox")
            .acquired
            .push(sid.clone());
        Ok(sid)
    }

    async fn exec(
        &self,
        _sandbox_id: &str,
        _req: &aite_contracts::ExecRequest,
    ) -> Result<aite_contracts::ExecResult, SandboxError> {
        Err(SandboxError::new(
            SandboxErrorKind::Internal,
            "R5 不测执行面",
        ))
    }

    async fn put_file(
        &self,
        _sandbox_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), SandboxError> {
        self.put(path, data);
        Ok(())
    }

    async fn get_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError> {
        // 这个替身故意不按 sandbox_id 分桶（R5 只测取产物这条链，不测隔离），
        // 所以「取错沙箱」看 files 是看不出来的 —— 记下来让测试能断言取的是哪个。
        let mut st = self.state.lock().expect("sandbox");
        st.get_file_calls
            .push((sandbox_id.to_string(), path.to_string()));
        st.files.get(path).cloned().ok_or_else(|| {
            SandboxError::new(
                SandboxErrorKind::FileNotFound,
                format!("没有这个文件：{path}"),
            )
        })
    }

    async fn list_files(&self, _sandbox_id: &str) -> Result<Vec<String>, SandboxError> {
        let st = self.state.lock().expect("sandbox");
        let mut names: Vec<String> = st.files.keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    async fn touch(&self, _sandbox_id: &str) -> Result<(), SandboxError> {
        Ok(())
    }

    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.state
            .lock()
            .expect("sandbox")
            .released
            .push(sandbox_id.to_string());
        Ok(())
    }

    async fn reap_idle(&self, _idle_sec: u32) -> Result<Vec<String>, SandboxError> {
        Ok(Vec::new())
    }
}

// ---- 假 Gateway ---------------------------------------------------------

#[derive(Default)]
struct GatewayState {
    calls: Vec<(ToolContext, ToolCallRequest)>,
    released_tasks: Vec<String>,
    registered: Vec<(String, String)>,
    sandbox_of: HashMap<String, String>,
}

/// 按工具名返回预置 `ToolResult`；没预置的返回 `ok=true` + 一句没用的话
/// —— 跟 T17 里那个「工具成功、内容没用」的形状同构，T20 的用例正靠它。
pub struct FakeGateway {
    results: HashMap<String, ToolResult>,
    /// `catalog()` 的返回值（CC3 ②）；`None` = `gateway_tools()`
    catalog: Option<Vec<ToolSpec>>,
    sandbox: Option<Arc<FakeSandbox>>,
    state: Mutex<GatewayState>,
}

/// 这些工具真跑起来会让 Gateway 建沙箱（run_python 执行代码、下载附件要落到 /work）
const SANDBOX_TOOLS: [&str; 2] = ["run_python", "download_attachment"];

impl FakeGateway {
    pub fn new(sandbox: Option<Arc<FakeSandbox>>) -> Arc<Self> {
        Arc::new(Self {
            results: HashMap::new(),
            catalog: None,
            sandbox,
            state: Mutex::new(GatewayState::default()),
        })
    }

    /// 让 `catalog()` 返回这一份（CC3 ②）。
    pub fn with_catalog(mut self, catalog: Vec<ToolSpec>) -> Self {
        self.catalog = Some(catalog);
        self
    }

    pub fn with_result(mut self, name: &str, result: ToolResult) -> Self {
        self.results.insert(name.to_string(), result);
        self
    }

    pub fn calls(&self) -> Vec<(ToolContext, ToolCallRequest)> {
        self.state.lock().expect("gateway").calls.clone()
    }

    pub fn call_names(&self) -> Vec<String> {
        self.calls().into_iter().map(|(_, r)| r.name).collect()
    }

    pub fn released_tasks(&self) -> Vec<String> {
        self.state.lock().expect("gateway").released_tasks.clone()
    }

    pub fn registered(&self) -> Vec<(String, String)> {
        self.state.lock().expect("gateway").registered.clone()
    }

    /// 演「某个工具调用已经给这个 task 建过沙箱了」。
    pub fn hold_sandbox(&self, task_id: &str) -> String {
        let sid = "sb_gateway".to_string();
        self.state
            .lock()
            .expect("gateway")
            .sandbox_of
            .insert(task_id.to_string(), sid.clone());
        sid
    }
}

#[async_trait]
impl ToolGateway for FakeGateway {
    fn catalog(&self, _ctx: &ToolContext) -> Vec<ToolSpec> {
        self.catalog
            .clone()
            .unwrap_or_else(|| gateway_tools().to_vec())
    }

    async fn call(&self, ctx: &ToolContext, req: &ToolCallRequest) -> ToolResult {
        self.state
            .lock()
            .expect("gateway")
            .calls
            .push((ctx.clone(), req.clone()));
        if SANDBOX_TOOLS.contains(&req.name.as_str()) {
            self.hold_sandbox(&ctx.task_id);
        }
        match self.results.get(&req.name) {
            Some(preset) => ToolResult {
                call_id: req.call_id.clone(),
                ..preset.clone()
            },
            None => ToolResult {
                call_id: req.call_id.clone(),
                name: req.name.clone(),
                ok: true,
                content: format!("{} ok", req.name),
                data: None,
                error: None,
                duration_ms: 0,
                artifacts: Vec::new(),
            },
        }
    }

    fn register_task(&self, task_id: &str, session_token: &str) {
        self.state
            .lock()
            .expect("gateway")
            .registered
            .push((task_id.to_string(), session_token.to_string()));
    }

    fn unregister_task(&self, task_id: &str) {
        self.state
            .lock()
            .expect("gateway")
            .sandbox_of
            .remove(task_id);
    }

    async fn sandbox_id_of(&self, task_id: &str) -> Option<String> {
        self.state
            .lock()
            .expect("gateway")
            .sandbox_of
            .get(task_id)
            .cloned()
    }

    async fn release_task(&self, task_id: &str) {
        let sandbox_id = {
            let mut st = self.state.lock().expect("gateway");
            st.released_tasks.push(task_id.to_string());
            st.sandbox_of.remove(task_id)
        };
        if let (Some(sid), Some(sandbox)) = (sandbox_id, &self.sandbox) {
            let _ = sandbox.release(&sid).await;
        }
    }
}

// ---- 假会话存储 ---------------------------------------------------------

#[derive(Default)]
struct StoreState {
    sessions: HashMap<String, Session>,
    tasks: HashMap<String, Task>,
    turns: HashMap<String, Vec<Turn>>,
    /// 每一次 `update_task` 落的状态，按顺序。
    ///
    /// 只看最终态的话，状态机中间那几格谁都钉不住 —— `deliver()` 的 Answering 就是
    /// 这么长期没人管的（全仓一条断言都没有）。
    status_writes: Vec<(String, TaskStatus)>,
}

#[derive(Default)]
pub struct FakeSessionStore {
    state: Mutex<StoreState>,
    /// 接下来几次 `append_turn` 先替「别的写者」占掉该 seq、再报 `DuplicateTurn`（CC3 ③）
    duplicate: Mutex<usize>,
}

impl FakeSessionStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn put_session(&self, s: &Session) {
        self.state
            .lock()
            .expect("store")
            .sessions
            .insert(s.id.clone(), s.clone());
    }

    pub fn put_task(&self, t: &Task) {
        self.state
            .lock()
            .expect("store")
            .tasks
            .insert(t.id.clone(), t.clone());
    }

    pub fn push_turn(&self, t: Turn) {
        self.state
            .lock()
            .expect("store")
            .turns
            .entry(t.session_id.clone())
            .or_default()
            .push(t);
    }

    /// 这个会话的全部 turn（含角色 / seq），按写入顺序。
    pub fn turns(&self, session_id: &str) -> Vec<Turn> {
        self.state
            .lock()
            .expect("store")
            .turns
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 只取 User turn 的正文（CC3 ③ 起交付会多一条助手轮，只关心用户说了什么的断言用这个）。
    pub fn user_turn_texts(&self, session_id: &str) -> Vec<String> {
        self.turns(session_id)
            .into_iter()
            .filter(|t| t.role == TurnRole::User)
            .map(|t| t.content)
            .collect()
    }

    /// 模拟控制面抢先写了一轮：接下来 `times` 次 `append_turn` 先用一条 User 轮占掉要写的 seq，
    /// 再报 `DuplicateTurn`。
    pub fn duplicate_next_append_turn(&self, times: usize) {
        *self.duplicate.lock().expect("dup") = times;
    }

    pub fn turn_texts(&self, session_id: &str) -> Vec<String> {
        self.state
            .lock()
            .expect("store")
            .turns
            .get(session_id)
            .map(|ts| ts.iter().map(|t| t.content.clone()).collect())
            .unwrap_or_default()
    }

    pub fn task(&self, task_id: &str) -> Option<Task> {
        self.state
            .lock()
            .expect("store")
            .tasks
            .get(task_id)
            .cloned()
    }

    /// worker 调了几次 update_task（每步末尾一次 + 几个关键节点）。
    pub fn saved_status(&self, task_id: &str) -> Option<TaskStatus> {
        self.task(task_id).map(|t| t.status)
    }

    /// 这个任务被 `update_task` 落过的状态，按先后顺序（含中间态）。
    pub fn status_writes(&self, task_id: &str) -> Vec<TaskStatus> {
        self.state
            .lock()
            .expect("store")
            .status_writes
            .iter()
            .filter(|(id, _)| id == task_id)
            .map(|(_, status)| *status)
            .collect()
    }
}

#[async_trait]
impl SessionStore for FakeSessionStore {
    async fn init(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn close(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get_session(&self, session_id: &str) -> Result<Option<Session>, StoreError> {
        Ok(self
            .state
            .lock()
            .expect("store")
            .sessions
            .get(session_id)
            .cloned())
    }

    async fn find_session_by_thread(
        &self,
        _chat_id: &str,
        _thread_id: &str,
    ) -> Result<Option<Session>, StoreError> {
        Ok(None)
    }

    async fn create_session(&self, s: &Session) -> Result<(), StoreError> {
        self.put_session(s);
        Ok(())
    }

    async fn update_session(&self, s: &Session) -> Result<(), StoreError> {
        self.put_session(s);
        Ok(())
    }

    async fn append_turn(&self, t: &Turn) -> Result<(), StoreError> {
        {
            let mut dup = self.duplicate.lock().expect("dup");
            if *dup > 0 {
                *dup -= 1;
                let mut other = t.clone();
                other.role = TurnRole::User;
                other.platform_user_id = Some("ou_other".into());
                other.content = format!("（别的写者抢先写的 seq={}）", t.seq);
                drop(dup);
                self.push_turn(other);
            }
        }
        // 与真 store 同口径：(session_id, seq) 唯一
        let taken = self
            .turns(&t.session_id)
            .iter()
            .any(|existing| existing.seq == t.seq);
        if taken {
            return Err(StoreError::DuplicateTurn {
                session_id: t.session_id.clone(),
                seq: t.seq,
            });
        }
        self.push_turn(t.clone());
        Ok(())
    }

    async fn list_turns(&self, session_id: &str, limit: u32) -> Result<Vec<Turn>, StoreError> {
        let st = self.state.lock().expect("store");
        let all = st.turns.get(session_id).cloned().unwrap_or_default();
        let start = all.len().saturating_sub(limit as usize);
        Ok(all[start..].to_vec())
    }

    async fn next_turn_seq(&self, session_id: &str) -> Result<u64, StoreError> {
        let st = self.state.lock().expect("store");
        Ok(st
            .turns
            .get(session_id)
            .map(|t| t.len() as u64)
            .unwrap_or(0))
    }

    async fn create_task(&self, t: &Task) -> Result<(), StoreError> {
        self.put_task(t);
        Ok(())
    }

    async fn update_task(&self, t: &Task) -> Result<(), StoreError> {
        self.put_task(t);
        self.state
            .lock()
            .expect("store")
            .status_writes
            .push((t.id.clone(), t.status));
        Ok(())
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, StoreError> {
        Ok(self.task(task_id))
    }

    async fn list_active_tasks(&self, chat_id: &str) -> Result<Vec<Task>, StoreError> {
        let st = self.state.lock().expect("store");
        let mut out: Vec<Task> = st
            .tasks
            .values()
            .filter(|t| t.status.is_active())
            .filter(|t| {
                st.sessions
                    .get(&t.session_id)
                    .is_some_and(|s| s.chat_id == chat_id)
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(out)
    }

    async fn next_task_no(&self, _tenant_id: &str) -> Result<String, StoreError> {
        Ok("#A1".to_string())
    }

    async fn seen_event(&self, _event_id: &str) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn recover_orphan_tasks(&self) -> Result<Vec<Task>, StoreError> {
        Ok(Vec::new())
    }
}

// ---- 假证据写手（hash 链真算）-------------------------------------------

#[derive(Default)]
struct EvidenceState {
    events: HashMap<String, Vec<EvidenceEvent>>,
    manifests: HashMap<String, Map<String, Value>>,
}

pub struct FakeEvidenceWriter {
    state: Mutex<EvidenceState>,
    root: PathBuf,
}

impl FakeEvidenceWriter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(EvidenceState::default()),
            root: PathBuf::from("/tmp/aite-fake-evidence"),
        })
    }

    pub fn events(&self, task_id: &str) -> Vec<EvidenceEvent> {
        self.state
            .lock()
            .expect("evidence")
            .events
            .get(task_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn kinds(&self, task_id: &str) -> Vec<EvidenceKind> {
        self.events(task_id).into_iter().map(|e| e.kind).collect()
    }

    pub fn count_kind(&self, task_id: &str, kind: EvidenceKind) -> usize {
        self.kinds(task_id).iter().filter(|k| **k == kind).count()
    }

    pub fn payloads(&self, task_id: &str, kind: EvidenceKind) -> Vec<Map<String, Value>> {
        self.events(task_id)
            .into_iter()
            .filter(|e| e.kind == kind)
            .filter_map(|e| e.payload)
            .collect()
    }

    pub fn manifest(&self, task_id: &str) -> Option<Map<String, Value>> {
        self.state
            .lock()
            .expect("evidence")
            .manifests
            .get(task_id)
            .cloned()
    }

    /// 落盘形态的每一行（`events.jsonl` 的等价物），给「正文只出现一次」那类断言用。
    pub fn lines(&self, task_id: &str) -> Vec<String> {
        self.events(task_id)
            .iter()
            .map(|e| serde_json::to_string(e).unwrap_or_default())
            .collect()
    }

    pub fn raw(&self, task_id: &str) -> String {
        self.lines(task_id).join("\n")
    }
}

#[async_trait]
impl EvidenceWriter for FakeEvidenceWriter {
    async fn append(
        &self,
        task_id: &str,
        kind: EvidenceKind,
        payload: Map<String, Value>,
    ) -> Result<EvidenceEvent, EvidenceError> {
        let mut st = self.state.lock().expect("evidence");
        let chain = st.events.entry(task_id.to_string()).or_default();
        let seq = chain.len() as u64;
        let prev_hash = chain
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| GENESIS.to_string());
        let payload_hash = payload_hash_of(&payload);
        let hash = chain_hash(&prev_hash, &payload_hash);
        let ev = EvidenceEvent {
            task_id: task_id.to_string(),
            seq,
            kind,
            payload_hash,
            payload_ref: None,
            payload: Some(payload),
            prev_hash,
            hash,
            created_at: Utc::now(),
        };
        chain.push(ev.clone());
        Ok(ev)
    }

    async fn finalize(
        &self,
        task_id: &str,
        manifest_extra: Map<String, Value>,
    ) -> Result<String, EvidenceError> {
        let mut st = self.state.lock().expect("evidence");
        let chain = st.events.get(task_id).cloned().unwrap_or_default();
        let root_hash = chain
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| GENESIS.to_string());
        let mut manifest = Map::new();
        manifest.insert("task_id".into(), json!(task_id));
        for key in ["session_id", "task_no", "created_by", "model"] {
            let v = manifest_extra
                .get(key)
                .cloned()
                .unwrap_or_else(|| json!(""));
            manifest.insert(key.into(), v);
        }
        manifest.insert(
            "contract_version".into(),
            json!(aite_contracts::CONTRACT_VERSION),
        );
        manifest.insert("root_hash".into(), json!(root_hash));
        manifest.insert("event_count".into(), json!(chain.len()));
        st.manifests.insert(task_id.to_string(), manifest);
        Ok(root_hash)
    }

    fn verify(&self, task_id: &str) -> bool {
        let chain = self.events(task_id);
        let mut prev = GENESIS.to_string();
        for (i, ev) in chain.iter().enumerate() {
            if ev.task_id != task_id || ev.seq != i as u64 || ev.prev_hash != prev {
                return false;
            }
            let Some(payload) = &ev.payload else {
                return false;
            };
            if payload_hash_of(payload) != ev.payload_hash {
                return false;
            }
            if chain_hash(&ev.prev_hash, &ev.payload_hash) != ev.hash {
                return false;
            }
            // canonical_json 走一遍，确认 payload 真能序列化（与真实现同口径）
            let _ = canonical_json(payload);
            prev = ev.hash.clone();
        }
        true
    }

    fn task_dir(&self, task_id: &str) -> PathBuf {
        self.root.join(task_id)
    }
}

// ---- ModelTurn 构造器 ---------------------------------------------------

fn usage() -> Usage {
    Usage {
        input_tokens: 10,
        output_tokens: 5,
        cached_tokens: 0,
    }
}

pub fn text_turn(text: &str) -> ModelTurn {
    ModelTurn {
        message: Message::text(Role::Assistant, text),
        usage: usage(),
        finish_reason: "stop".into(),
        raw: Map::new(),
    }
}

pub fn tool_turn(calls: &[(&str, Value)]) -> ModelTurn {
    let tool_calls: Vec<ToolCallRequest> = calls
        .iter()
        .enumerate()
        .map(|(i, (name, args))| ToolCallRequest {
            call_id: format!("call_{i}"),
            name: (*name).to_string(),
            arguments: args.as_object().cloned().unwrap_or_default(),
        })
        .collect();
    ModelTurn {
        message: Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        },
        usage: usage(),
        finish_reason: "tool_calls".into(),
        raw: Map::new(),
    }
}

pub fn final_turn(reply: &str) -> ModelTurn {
    tool_turn(&[("final", json!({"reply": reply}))])
}

pub fn final_turn_with(reply: &str, artifacts: Value) -> ModelTurn {
    tool_turn(&[("final", json!({"reply": reply, "artifacts": artifacts}))])
}

pub fn history(rows: &[(&str, &str, &str, &str)]) -> Vec<HistoryMessage> {
    rows.iter()
        .map(|(mid, kind, name, text)| HistoryMessage {
            message_id: (*mid).to_string(),
            sender_id: format!("ou_{name}"),
            sender_kind: (*kind).to_string(),
            sender_name: Some((*name).to_string()),
            text: (*text).to_string(),
            thread_id: None,
            created_at: Utc::now(),
        })
        .collect()
}

pub fn attachment(file_key: &str, name: &str, size: Option<i64>) -> Attachment {
    Attachment {
        kind: AttachmentKind::File,
        file_key: file_key.to_string(),
        message_id: ROOT.to_string(),
        name: Some(name.to_string()),
        size,
        mime: None,
    }
}

pub fn image_attachment(file_key: &str, name: &str) -> Attachment {
    Attachment {
        kind: AttachmentKind::Image,
        file_key: file_key.to_string(),
        message_id: ROOT.to_string(),
        name: Some(name.to_string()),
        size: None,
        mime: None,
    }
}

pub fn gateway_error_result(name: &str, code: ToolErrorCode, message: &str) -> ToolResult {
    ToolResult {
        call_id: String::new(),
        name: name.to_string(),
        ok: false,
        content: String::new(),
        data: None,
        error: Some(ToolError {
            code,
            message: message.to_string(),
        }),
        duration_ms: 7,
        artifacts: Vec::new(),
    }
}

// ---- 测试台 -------------------------------------------------------------

pub fn prompt_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("prompts/platform.md")
}

pub fn make_config() -> AiteConfig {
    let mut cfg = AiteConfig {
        tenant_id: "default".into(),
        platform: aite_contracts::PlatformChoice::Fake,
        ..AiteConfig::default()
    };
    cfg.model.provider = ModelProvider::Scripted;
    cfg.model.model = "scripted-p0".into();
    cfg.worker.system_prompt_path = prompt_path().display().to_string();
    cfg
}

static TOKEN_SEED: AtomicU64 = AtomicU64::new(1);

fn session_token() -> String {
    // 只要「32 位十六进制」这条性质成立即可，测试不关心随机性
    let n = TOKEN_SEED.fetch_add(1, Ordering::SeqCst);
    format!("{n:032x}")
}

/// 一次 run 需要的全部替身与前置状态。
pub struct Harness {
    pub store: Arc<FakeSessionStore>,
    pub platform: Arc<FakePlatform>,
    pub sandbox: Arc<FakeSandbox>,
    pub gateway: Arc<FakeGateway>,
    pub evidence: Arc<FakeEvidenceWriter>,
    pub clock: FakeClock,
    pub config: AiteConfig,
    pub session: Session,
    pub task: Task,
    steer: Arc<Mutex<Vec<String>>>,
    cancelled: Arc<AtomicBool>,
    turn_seq: Arc<AtomicU64>,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_gateway(None)
    }

    pub fn with_gateway(gateway: Option<Arc<FakeGateway>>) -> Self {
        let sandbox = FakeSandbox::new();
        let gateway = gateway.unwrap_or_else(|| FakeGateway::new(Some(sandbox.clone())));
        let now = Utc::now();
        let session = Session {
            id: "s1".into(),
            tenant_id: "default".into(),
            workspace_id: "app_1".into(),
            chat_id: CHAT_ID.into(),
            kind: SessionKind::Task,
            anchor: Anchor {
                platform: "fake".into(),
                chat_id: CHAT_ID.into(),
                message_id: ROOT.into(),
                // 控制面 R7：话题 root 用触发消息自己（事件没带 thread_id）
                thread_id: Some(ROOT.into()),
                task_no: None,
            },
            status: SessionStatus::Active,
            created_by: "ou_user".into(),
            config_snapshot: Map::new(),
            created_at: now,
            last_active_at: now,
            archived_at: None,
        };
        let task = Task {
            id: "t1".into(),
            session_id: session.id.clone(),
            task_no: "#A1".into(),
            status: TaskStatus::Created,
            title: String::new(),
            checklist: Vec::new(),
            card_id: None,
            sandbox_id: None,
            session_token: session_token(),
            model: "scripted-p0".into(),
            steps: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            max_steps: 40,
            max_wall_sec: 1200,
            result_summary: String::new(),
            evidence_root_hash: None,
            created_by: "ou_user".into(),
            created_at: now,
            updated_at: now,
        };
        Self {
            store: FakeSessionStore::new(),
            platform: FakePlatform::new(),
            sandbox,
            gateway,
            evidence: FakeEvidenceWriter::new(),
            clock: FakeClock::new(),
            config: make_config(),
            session,
            task,
            steer: Arc::new(Mutex::new(Vec::new())),
            cancelled: Arc::new(AtomicBool::new(false)),
            turn_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 控制面 `_start_task` 在 worker 开跑之前做的那几件事。
    pub async fn seed(&mut self, text: &str, attachments: Vec<Attachment>) {
        self.store.put_session(&self.session);
        self.store.put_task(&self.task);
        let seq = self.turn_seq.fetch_add(1, Ordering::SeqCst);
        self.store.push_turn(Turn {
            session_id: self.session.id.clone(),
            seq,
            role: TurnRole::User,
            platform_user_id: Some("ou_user".into()),
            content: text.to_string(),
            attachments,
            created_at: Utc::now(),
        });
        let _ = self
            .evidence
            .append(
                &self.task.id,
                EvidenceKind::TaskCreated,
                json_map(json!({
                    "session_id": self.session.id,
                    "task_no": self.task.task_no,
                    "chat_id": self.session.chat_id,
                    "created_by": self.task.created_by,
                    "title": text,
                })),
            )
            .await;
        let _ = self
            .evidence
            .append(
                &self.task.id,
                EvidenceKind::EventReceived,
                json_map(json!({
                    "event_id": "ev1",
                    "kind": "message",
                    "chat_id": self.session.chat_id,
                    "sender_id": "ou_user",
                    "message_id": ROOT,
                    "mentioned": true,
                    "route": "new_task",
                })),
            )
            .await;
    }

    /// 控制面 `_continue_session` 的 steer 分支：落 turn + 排队 + 写 `route=steer` 证据。
    pub async fn push_steer(&self, text: &str) {
        let seq = self.turn_seq.fetch_add(1, Ordering::SeqCst);
        self.store.push_turn(Turn {
            session_id: self.session.id.clone(),
            seq,
            role: TurnRole::User,
            platform_user_id: Some("ou_user".into()),
            content: text.to_string(),
            attachments: Vec::new(),
            created_at: Utc::now(),
        });
        self.steer.lock().expect("steer").push(text.to_string());
        let clipped: String = text.chars().take(40).collect();
        let _ = self
            .evidence
            .append(
                &self.task.id,
                EvidenceKind::EventReceived,
                json_map(json!({
                    "event_id": "ev2",
                    "kind": "message",
                    "chat_id": self.session.chat_id,
                    "sender_id": "ou_user",
                    "message_id": "om_2",
                    "mentioned": false,
                    "route": "steer",
                    "text": clipped,
                })),
            )
            .await;
    }

    /// 同 `push_steer`，但说话的是 `uid`（CC3 ④ 署名用例）。不写证据。
    pub async fn push_steer_as(&self, uid: &str, text: &str) {
        let seq = self.turn_seq.fetch_add(1, Ordering::SeqCst);
        self.store.push_turn(Turn {
            session_id: self.session.id.clone(),
            seq,
            role: TurnRole::User,
            platform_user_id: Some(uid.to_string()),
            content: text.to_string(),
            attachments: Vec::new(),
            created_at: Utc::now(),
        });
        self.steer.lock().expect("steer").push(text.to_string());
    }

    /// 控制面 `cancel_task` 的「在跑」分支：只置旗 + 清掉排队的追问，收尾归 worker。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.clear_steer();
    }

    /// 控制面 `_dispatch_task` 派发前那一下：开跑前排进来的 steer 已经在 transcript 里了，
    /// 不清掉的话第一步开头会把同一句话再注入一遍（归 R4，这里复刻给用例用）。
    pub fn clear_steer(&self) {
        self.steer.lock().expect("steer").clear();
    }

    pub fn pending_steer(&self) -> Vec<String> {
        self.steer.lock().expect("steer").clone()
    }

    pub fn hooks(&self) -> RunHooks {
        let queue = self.steer.clone();
        let cancelled = self.cancelled.clone();
        RunHooks {
            drain_steer: Arc::new(move || std::mem::take(&mut *queue.lock().expect("steer"))),
            is_cancelled: Arc::new(move || cancelled.load(Ordering::SeqCst)),
        }
    }

    pub fn worker(&self, model: Arc<ScriptedModel>) -> AgentWorker {
        AgentWorker::new(WorkerDeps {
            store: self.store.clone(),
            platform: self.platform.clone(),
            model,
            evidence: self.evidence.clone(),
            config: self.config.clone(),
            gateway: Some(self.gateway.clone()),
            sandbox: Some(self.sandbox.clone()),
        })
        .with_clock(self.clock.as_clock())
        .with_sleep(self.clock.as_sleeper())
    }

    pub async fn run(&self, model: Arc<ScriptedModel>) -> Task {
        let worker = self.worker(model);
        worker
            .run(
                self.task.clone(),
                self.session.clone(),
                Some("张三".into()),
                self.hooks(),
            )
            .await
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

pub fn json_map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// 最常见的形状：给脚本、每步推进 `step_seconds` 秒、跑完返回 (task, model, harness)。
pub struct Run {
    pub task: Task,
    pub model: Arc<ScriptedModel>,
    pub h: Harness,
}

pub async fn run_script(script: Vec<ModelTurn>, step_seconds: f64) -> Run {
    run_with(script, None, step_seconds, "帮我出个图", Vec::new(), 0).await
}

pub async fn run_repeat(turn: ModelTurn, step_seconds: f64) -> Run {
    run_with(
        Vec::new(),
        Some(turn),
        step_seconds,
        "帮我出个图",
        Vec::new(),
        0,
    )
    .await
}

pub async fn run_with(
    script: Vec<ModelTurn>,
    repeat: Option<ModelTurn>,
    step_seconds: f64,
    text: &str,
    attachments: Vec<Attachment>,
    fail_times: u32,
) -> Run {
    let mut h = Harness::new();
    h.seed(text, attachments).await;
    let mut model = ScriptedModel::new(script).with_clock(h.clock.clone(), step_seconds);
    if let Some(turn) = repeat {
        model = model.with_repeat(turn);
    }
    if fail_times > 0 {
        model = model.with_fail_times(fail_times);
    }
    let model = Arc::new(model);
    let task = h.run(model.clone()).await;
    Run { task, model, h }
}

/// 消息列表里符合条件的 system 消息正文。
pub fn systems(messages: &[Message], needle: &str) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m.role == Role::System && m.content.contains(needle))
        .map(|m| m.content.clone())
        .collect()
}

pub fn tool_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .cloned()
        .collect()
}

pub fn card_actions_default() -> Vec<CardActionKind> {
    vec![CardActionKind::Stop]
}
