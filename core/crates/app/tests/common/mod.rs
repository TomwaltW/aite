//! `core/crates/app/tests` 的公共脚手架（对应 Python `tests/integration/integration_fakes.py`
//! + `conftest.py`）。
//!
//! 这一层测的是**进程级接线**：SQLite 真落盘、evidence 真写文件、`run_app` 真收尾。
//! 所以这里刻意不提供任何「跳过组装直接拿 plane / worker」的捷径 —— 每个用例都必须
//! 从 `build_app` 走一遍，否则测的就不是接线了。
#![allow(dead_code)] // 每个测试文件各用一部分；整体用得上，单文件看就有没用到的

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aite_app::{AiteApp, Injections, ServeOptions, StopSignal, build_app, run_app};
use aite_contracts::{
    AiteConfig, Anchor, Attachment, CardAction, ChecklistCard, DocumentContent, EventHandler,
    EventKind, EvidenceEvent, HistoryMessage, Message, ModelError, ModelPort, ModelTurn,
    NormalizedEvent, OutboundFile, OutboundText, PlatformCapabilities, PlatformError, PlatformPort,
    ReactionKind, SandboxError, SandboxPort, SandboxSpec, SendResult, SenderKind, SessionStore,
    Task, ToolSpec,
};
use aite_store::SqliteSessionStore;
use aite_testing::{FakeModel, FakePlatform, FakeSandbox, ScriptStep};
use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::task::JoinHandle;

pub const TENANT: &str = "t_test";
pub const CHAT: &str = "oc_1";
pub const SENDER: &str = "ou_zhang";
pub const WAIT_TIMEOUT_SEC: f64 = 5.0;

/// 仓库根（`core/crates/app` 往上三层）。
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn platform_md() -> PathBuf {
    repo_root().join("core/crates/worker/prompts/platform.md")
}

/// 一份全落在 `dir` 里的配置。system_prompt 走绝对路径，免得吃 cwd。
///
/// `platform: fake` + `model.provider: scripted` —— 两个都是「必须由调用方注入」的标记。
pub fn make_config(dir: &Path) -> AiteConfig {
    let mut cfg = AiteConfig {
        tenant_id: TENANT.to_string(),
        platform: aite_contracts::PlatformChoice::Fake,
        ..AiteConfig::default()
    };
    cfg.storage.sqlite_path = dir.join("aite.db").display().to_string();
    cfg.storage.evidence_dir = dir.join("evidence").display().to_string();
    cfg.storage.artifacts_dir = dir.join("artifacts").display().to_string();
    cfg.worker.system_prompt_path = platform_md().display().to_string();
    // 卡片合并窗口置 0：这一轨要看的是「卡片有没有被更新」，不是 W4 的 500ms 合并。
    cfg.worker.card_update_min_interval_ms = 0;
    cfg.model.provider = aite_contracts::ModelProvider::Scripted;
    cfg.model.model = "scripted-p0".to_string();
    cfg
}

/// 事件构造器。默认是「群里 @ 机器人说了句话」。
pub struct Ev {
    pub event_id: String,
    pub kind: EventKind,
    pub text: String,
    pub mentioned: bool,
    pub sender_kind: SenderKind,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub chat_id: String,
    pub message_id: String,
    pub thread_id: Option<String>,
    pub attachments: Vec<Attachment>,
    pub card_action: Option<CardAction>,
}

impl Default for Ev {
    fn default() -> Self {
        Self {
            event_id: "e1".to_string(),
            kind: EventKind::Message,
            text: "你好".to_string(),
            mentioned: true,
            sender_kind: SenderKind::Human,
            sender_id: SENDER.to_string(),
            sender_name: Some("张三".to_string()),
            chat_id: CHAT.to_string(),
            message_id: "om_1".to_string(),
            thread_id: None,
            attachments: Vec::new(),
            card_action: None,
        }
    }
}

impl Ev {
    pub fn new(event_id: &str, text: &str) -> Self {
        Self {
            event_id: event_id.to_string(),
            text: text.to_string(),
            ..Self::default()
        }
    }
    pub fn message_id(mut self, v: &str) -> Self {
        self.message_id = v.to_string();
        self
    }
    pub fn thread_id(mut self, v: &str) -> Self {
        self.thread_id = Some(v.to_string());
        self
    }
    pub fn mentioned(mut self, v: bool) -> Self {
        self.mentioned = v;
        self
    }
    pub fn sender_kind(mut self, v: SenderKind) -> Self {
        self.sender_kind = v;
        self
    }
    pub fn attachments(mut self, v: Vec<Attachment>) -> Self {
        self.attachments = v;
        self
    }
    pub fn build(self) -> NormalizedEvent {
        NormalizedEvent {
            event_id: self.event_id,
            kind: self.kind,
            platform: "fake".to_string(),
            tenant_id: TENANT.to_string(),
            workspace_id: "app_1".to_string(),
            chat_id: self.chat_id.clone(),
            chat_type: aite_contracts::ChatType::Group,
            sender_id: self.sender_id,
            sender_kind: self.sender_kind,
            sender_name: self.sender_name,
            text: self.text,
            raw_text: None,
            mentioned: self.mentioned,
            anchor: Anchor {
                platform: "fake".to_string(),
                chat_id: self.chat_id,
                message_id: self.message_id,
                thread_id: self.thread_id,
                task_no: None,
            },
            attachments: self.attachments,
            card_action: self.card_action,
            occurred_at: chrono::Utc::now(),
            raw: Map::new(),
        }
    }
}

pub fn event(event_id: &str, text: &str) -> NormalizedEvent {
    Ev::new(event_id, text).build()
}

// --------------------------------------------------------------------------
// 替身
// --------------------------------------------------------------------------

/// `stop()` 之后就不再往上投事件了 —— 长连接断了的真 adapter 就是这个行为。
///
/// `FakePlatform::stop` 只记一笔 `stopped=true`，`emit` 照样投得进去；这里补上那道闸，
/// 并数一数「收尾期间被丢掉的事件」（Python 版 `GatedPlatform.dropped_after_stop`）。
pub struct GatedPlatform {
    pub inner: Arc<FakePlatform>,
    dropped_after_stop: AtomicUsize,
    /// 置上之后 `start()` 直接报错。
    ///
    /// `FakePlatform::fail_next` 管不到 `start`（它只记一笔 `started=true`，不看失败表），
    /// 而「`store.init()` 成功之后的第一个可失败点」正是 `start` —— 想测「想退退不出去」
    /// 那条就非得让它失败。开关放在这里而不是去改 R7 的替身。
    fail_start: std::sync::Mutex<Option<String>>,
}

impl GatedPlatform {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(FakePlatform::new()),
            dropped_after_stop: AtomicUsize::new(0),
            fail_start: std::sync::Mutex::new(None),
        })
    }

    pub fn from_fake(inner: Arc<FakePlatform>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            dropped_after_stop: AtomicUsize::new(0),
            fail_start: std::sync::Mutex::new(None),
        })
    }

    /// 下一次（以及之后每一次）`start()` 都报这个错。
    pub fn fail_start(&self, message: &str) {
        *self.fail_start.lock().expect("fail_start 锁") = Some(message.to_string());
    }

    pub fn dropped_after_stop(&self) -> usize {
        self.dropped_after_stop.load(Ordering::SeqCst)
    }

    /// 投一个事件。`stop()` 之后只记一笔丢弃，不往上走。
    pub async fn emit(&self, ev: &NormalizedEvent) {
        if self.inner.stopped() {
            self.dropped_after_stop.fetch_add(1, Ordering::SeqCst);
            return;
        }
        self.inner.emit(ev).await.expect("emit");
    }
}

#[async_trait]
impl PlatformPort for GatedPlatform {
    fn capabilities(&self) -> PlatformCapabilities {
        self.inner.capabilities()
    }
    async fn start(&self, on_event: EventHandler) -> Result<(), PlatformError> {
        if let Some(msg) = self.fail_start.lock().expect("fail_start 锁").clone() {
            return Err(PlatformError::new("internal", msg, false));
        }
        self.inner.start(on_event).await
    }
    async fn stop(&self) -> Result<(), PlatformError> {
        self.inner.stop().await
    }
    async fn send_text(&self, msg: &OutboundText) -> Result<SendResult, PlatformError> {
        self.inner.send_text(msg).await
    }
    async fn send_card(
        &self,
        chat_id: &str,
        reply_to: Option<&str>,
        card: &ChecklistCard,
    ) -> Result<SendResult, PlatformError> {
        self.inner.send_card(chat_id, reply_to, card).await
    }
    async fn update_card(&self, card_id: &str, card: &ChecklistCard) -> Result<(), PlatformError> {
        self.inner.update_card(card_id, card).await
    }
    async fn send_file(&self, msg: &OutboundFile) -> Result<SendResult, PlatformError> {
        self.inner.send_file(msg).await
    }
    async fn add_reaction(
        &self,
        message_id: &str,
        kind: ReactionKind,
    ) -> Result<(), PlatformError> {
        self.inner.add_reaction(message_id, kind).await
    }
    async fn read_history(
        &self,
        chat_id: &str,
        limit: u32,
        thread_id: Option<&str>,
    ) -> Result<Vec<HistoryMessage>, PlatformError> {
        self.inner.read_history(chat_id, limit, thread_id).await
    }
    async fn read_document(&self, url_or_token: &str) -> Result<DocumentContent, PlatformError> {
        self.inner.read_document(url_or_token).await
    }
    async fn download_file(
        &self,
        message_id: &str,
        file_key: &str,
    ) -> Result<Vec<u8>, PlatformError> {
        self.inner.download_file(message_id, file_key).await
    }
}

/// 实现了 `close_all` 的沙箱替身（`FakeSandbox` 走的是契约里的默认空实现）。
///
/// C-TΩ-1 的退出序列倒数第二步是「有沙箱就 close_all」；裸 `FakeSandbox` 证明的是
/// 「没实现也不许炸」，这个证明的是「实现了就一定被调到」。
pub struct ClosableFakeSandbox {
    pub inner: Arc<FakeSandbox>,
    close_calls: AtomicUsize,
}

impl ClosableFakeSandbox {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(FakeSandbox::new(Vec::new())),
            close_calls: AtomicUsize::new(0),
        })
    }
    pub fn from_fake(inner: Arc<FakeSandbox>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            close_calls: AtomicUsize::new(0),
        })
    }
    pub fn close_calls(&self) -> usize {
        self.close_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SandboxPort for ClosableFakeSandbox {
    async fn acquire(&self, task_id: &str, spec: &SandboxSpec) -> Result<String, SandboxError> {
        self.inner.acquire(task_id, spec).await
    }
    async fn exec(
        &self,
        sandbox_id: &str,
        req: &aite_contracts::ExecRequest,
    ) -> Result<aite_contracts::ExecResult, SandboxError> {
        self.inner.exec(sandbox_id, req).await
    }
    async fn put_file(
        &self,
        sandbox_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), SandboxError> {
        self.inner.put_file(sandbox_id, path, data).await
    }
    async fn get_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError> {
        self.inner.get_file(sandbox_id, path).await
    }
    async fn list_files(&self, sandbox_id: &str) -> Result<Vec<String>, SandboxError> {
        self.inner.list_files(sandbox_id).await
    }
    async fn touch(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.inner.touch(sandbox_id).await
    }
    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        self.inner.release(sandbox_id).await
    }
    async fn reap_idle(&self, idle_sec: u32) -> Result<Vec<String>, SandboxError> {
        self.inner.reap_idle(idle_sec).await
    }
    async fn close_all(&self) -> Result<(), SandboxError> {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.close_all().await
    }
}

/// 把每一次 chat 的 prompt 原文记下来的模型替身（Python `RecordingModel`）。
///
/// `FakeModel` 记的是 `n_messages` / `roles` / `tools`，不记正文 —— 而「system prompt
/// 真的被喂进去了」这件事只有正文答得上。
pub struct RecordingModel {
    pub inner: Arc<FakeModel>,
    prompts: std::sync::Mutex<Vec<Vec<Message>>>,
}

impl RecordingModel {
    pub fn new(script: Vec<ScriptStep>) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(FakeModel::new(script)),
            prompts: std::sync::Mutex::new(Vec::new()),
        })
    }
    pub fn call_count(&self) -> usize {
        self.inner.call_count()
    }
    pub fn holds(&self) -> usize {
        self.inner.holds()
    }
    pub fn release_holds(&self) {
        self.inner.release_holds();
    }
    /// 每一次 chat 的全部消息正文（按调用顺序）。
    pub fn prompt_texts(&self) -> Vec<Vec<String>> {
        self.prompts
            .lock()
            .expect("prompts 锁")
            .iter()
            .map(|ms| ms.iter().map(|m| m.content.clone()).collect())
            .collect()
    }
}

#[async_trait]
impl ModelPort for RecordingModel {
    fn name(&self) -> String {
        self.inner.name()
    }
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        max_tokens: u32,
        temperature: f32,
    ) -> Result<ModelTurn, ModelError> {
        self.prompts
            .lock()
            .expect("prompts 锁")
            .push(messages.to_vec());
        self.inner
            .chat(messages, tools, max_tokens, temperature)
            .await
    }
}

// --------------------------------------------------------------------------
// 脚本小工具
// --------------------------------------------------------------------------

pub fn final_step(reply: &str) -> ScriptStep {
    ScriptStep::final_reply(reply)
}

/// `final(reply, artifacts)`。
pub fn final_with_artifacts(reply: &str, artifacts: Vec<Value>) -> ScriptStep {
    let mut args = Map::new();
    args.insert("reply".into(), Value::String(reply.to_string()));
    args.insert("artifacts".into(), Value::Array(artifacts));
    ScriptStep::tool("final", args)
}

pub fn tool_step(name: &str, arguments: Value) -> ScriptStep {
    let args = match arguments {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    ScriptStep::tool(name, args)
}

/// 给某一步加上 hold_ticks：worker 会确定性地停在「下一次 chat」上。
pub fn holding(mut step: ScriptStep, ticks: u32) -> ScriptStep {
    step.hold_ticks = ticks;
    step
}

/// 「没人放行就一直停着」—— 上限够大，所以永远不会挂死。
pub const HOLD_FOREVER: u32 = 1_000_000;

// --------------------------------------------------------------------------
// 跑起来的 app
// --------------------------------------------------------------------------

pub struct RunningApp {
    pub stop: StopSignal,
    runner: Option<JoinHandle<Result<(), aite_app::StartupError>>>,
}

impl RunningApp {
    /// 把 `run_app` 挂到后台跑，并等到它真的调了 `platform.start`。
    ///
    /// `grace` 不传就走 C-TΩ-1 的默认 20.0 —— 默认值本身也是被测面之一。
    pub async fn start(
        app: Arc<AiteApp>,
        platform: &Arc<GatedPlatform>,
        grace: Option<f64>,
    ) -> Self {
        let stop = StopSignal::new();
        let opts = ServeOptions {
            stop: Some(stop.clone()),
            shutdown_grace_sec: grace.unwrap_or(aite_app::DEFAULT_SHUTDOWN_GRACE_SEC),
            // 测试里不碰进程信号：tokio 的 signal handler 是进程级的，
            // 多个用例并发跑会互相覆盖。
            install_signals: false,
        };
        let runner = tokio::spawn(async move { run_app(&app, &opts).await });
        let inner = platform.inner.clone();
        wait_until(
            || inner.started(),
            "run_app 调 platform.start(ingress.handler())",
        )
        .await;
        Self {
            stop,
            runner: Some(runner),
        }
    }

    /// 置停机信号并等 `run_app` 返回（超时就是挂死，直接红）。
    pub async fn shutdown(&mut self) -> Result<(), aite_app::StartupError> {
        self.shutdown_within(10.0).await
    }

    pub async fn shutdown_within(&mut self, secs: f64) -> Result<(), aite_app::StartupError> {
        self.stop.set();
        let runner = self.runner.take().expect("shutdown 只能调一次");
        match tokio::time::timeout(Duration::from_secs_f64(secs), runner).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => panic!("run_app 那条 task 异常收场：{e}"),
            Err(_) => panic!("{secs}s 内 run_app 没有返回 —— 收尾挂死了"),
        }
    }
}

/// 轮询等一个条件成立，死线由调用方给。等不到就是失败，不是「再等等」。
///
/// 为什么要有「自带死线」这一版：`WAIT_TIMEOUT_SEC` 是 5.0，而 `aite_store::BUSY_TIMEOUT_MS`
/// 也正好是 5000 —— 一次撞锁的写**合法地**最多等满 5s 才回 SQLITE_BUSY。凡是等待路径上
/// 真会碰到存储写的用例，5.0 这个死线分不开「还在合法重试」和「彻底不动了」，得由用例
/// 自己按最坏路径报一个有依据的值。
pub async fn wait_until_within(secs: f64, mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs_f64(secs);
    while !predicate() {
        if std::time::Instant::now() >= deadline {
            panic!("{secs}s 内没等到：{what}");
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// 轮询等一个条件成立。等不到就是失败，不是「再等等」。
pub async fn wait_until(predicate: impl FnMut() -> bool, what: &str) {
    wait_until_within(WAIT_TIMEOUT_SEC, predicate, what).await;
}

/// 让出若干 tick + 极短的真等待。
///
/// 用在「断言某件事**没有**发生」之前：给系统足够的机会去做那件不该做的事。
/// Python 版让的是纯 tick（单事件循环）；Rust 这边 worker 在别的 task 上跑，
/// 所以要让调度器真有机会跑一轮 —— 毫秒级，不是「睡到它大概好了」。
pub async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// --------------------------------------------------------------------------
// 落盘结果的读取（全部从磁盘 / 新连接读，不看 app 手上的实例）
// --------------------------------------------------------------------------

/// 用一条**新的** SQLite 连接把任务读回来。
pub async fn read_task_from_disk(config: &AiteConfig, task_id: &str) -> Option<Task> {
    let store = SqliteSessionStore::open(&config.storage.sqlite_path).expect("open");
    store.init().await.expect("init");
    let got = store.get_task(task_id).await.expect("get_task");
    store.close().await.expect("close");
    got
}

/// evidence 目录下有几个任务。
pub fn evidence_task_dirs(config: &AiteConfig) -> Vec<String> {
    let root = Path::new(&config.storage.evidence_dir);
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

pub fn events_path(config: &AiteConfig, task_id: &str) -> PathBuf {
    Path::new(&config.storage.evidence_dir)
        .join(task_id)
        .join("events.jsonl")
}

pub fn manifest_path(config: &AiteConfig, task_id: &str) -> PathBuf {
    Path::new(&config.storage.evidence_dir)
        .join(task_id)
        .join("manifest.json")
}

pub fn read_events(config: &AiteConfig, task_id: &str) -> Vec<EvidenceEvent> {
    let text = std::fs::read_to_string(events_path(config, task_id)).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("events.jsonl 每行都该是 EvidenceEvent"))
        .collect()
}

pub fn read_manifest(config: &AiteConfig, task_id: &str) -> Map<String, Value> {
    let text = std::fs::read_to_string(manifest_path(config, task_id)).expect("manifest.json");
    match serde_json::from_str(&text).expect("manifest.json 是 JSON") {
        Value::Object(m) => m,
        other => panic!("manifest.json 顶层该是对象，实际 {other}"),
    }
}

/// 一套注入齐全的组装。三个口子都给替身，所以不碰 edge、不碰网络。
pub async fn build_with(
    config: AiteConfig,
    platform: Arc<GatedPlatform>,
    model: Arc<dyn ModelPort>,
    sandbox: Arc<dyn SandboxPort>,
) -> Arc<AiteApp> {
    let app = build_app(
        config,
        Injections {
            platform: Some(platform),
            model: Some(model),
            sandbox: Some(sandbox),
            repo_root: Some(repo_root()),
        },
    )
    .await
    .expect("build_app");
    Arc::from(app)
}

/// 附件（`download_attachment` 的入口）。`message_id` 是下载时要配的那一半。
pub fn attachment(file_key: &str, name: &str, message_id: &str) -> Attachment {
    Attachment {
        kind: aite_contracts::AttachmentKind::File,
        file_key: file_key.to_string(),
        message_id: message_id.to_string(),
        name: Some(name.to_string()),
        size: None,
        mime: None,
    }
}

/// `platform.files` 的键是 `(message_id, file_key)`。
pub fn files_of(pairs: Vec<((&str, &str), Vec<u8>)>) -> BTreeMap<(String, String), Vec<u8>> {
    pairs
        .into_iter()
        .map(|((m, k), v)| ((m.to_string(), k.to_string()), v))
        .collect()
}
