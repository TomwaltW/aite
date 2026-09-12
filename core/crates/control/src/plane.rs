//! `InProcessControlPlane` —— 对应 `aite/control/plane.py`（712 行）。
//!
//! `handle_event` 是 §3.5 路由的唯一入口，R1–R8 **按编号顺序求值、命中即停**。
//! 读这个文件时对着 §3.5 那张表看，[`InProcessControlPlane::route`] 里每个分支都标了规则号。
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rand::Rng;
use serde_json::{Map, Value};

use aite_contracts::{
    AiteConfig, Anchor, CardActionKind, CardStatus, ChatType, ControlPlane, EventKind,
    EvidenceKind, EvidenceWriter, IngressError, NormalizedEvent, OutboundText, PlatformPort,
    ReactionKind, RunHooks, SandboxPort, SenderKind, Session, SessionKind, SessionStatus,
    SessionStore, StoreError, Task, TaskStatus, TaskWorker, ToolGateway, Turn, TurnRole,
};

use crate::card::{MAX_TITLE_CHARS, clip, render_card};
use crate::commands::{UNKNOWN_COMMAND_TEXT, normalize_task_no, parse_command};
use crate::lock;
use crate::queue::DispatchQueue;

/// W7：沙箱 reaper 的节奏。
pub const REAPER_INTERVAL_SEC: f64 = 60.0;

pub const NO_SUCH_TASK_TEXT: &str = "没有这个任务";
pub const NO_ACTIVE_TASK_TEXT: &str = "本群没有活跃任务";
pub const RESTART_EMPTY_TEXT: &str = "已重开会话，请直接说要做什么。";

/// R4 的四种「不触发任务」事件。
const R4_KINDS: [EventKind; 4] = [
    EventKind::MessageEdited,
    EventKind::MessageDeleted,
    EventKind::BotAdded,
    EventKind::MemberChanged,
];

/// `event_received` 的 `route`：这条事件是**建了任务**的那一条（R6 新建 / R7）。
pub const ROUTE_NEW_TASK: &str = "new_task";
/// `event_received` 的 `route`：这条是**任务跑到一半**排进来的追问（R6 steer）。
pub const ROUTE_STEER: &str = "steer";

/// 墙钟。Python 用的是 `datetime.now(UTC)`，这里显式化成可注入，测试才能钉住时间。
pub type WallClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;
/// 可注入的 sleep（reaper 的节奏）。Python 用 `asyncio.sleep`。
pub type SleepFn = Arc<dyn Fn(f64) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// `event_received` 的 payload。
///
/// `EvidenceKind` 是冻结契约，加不了新 kind，所以「建任务的那条事件」和「中途追问的那条」
/// 共用 `event_received`。共用就得分得开 —— 不然时间线上两条长得一模一样，读的人分不出
/// 哪条是任务的起点、哪条是用户改了主意。分辨靠 `route`：两个写入点都带。
fn event_payload(ev: &NormalizedEvent, route: &str, text: Option<String>) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("event_id".into(), Value::String(ev.event_id.clone()));
    p.insert("kind".into(), Value::String(ev.kind.as_str().into()));
    p.insert("chat_id".into(), Value::String(ev.chat_id.clone()));
    p.insert("sender_id".into(), Value::String(ev.sender_id.clone()));
    p.insert(
        "message_id".into(),
        Value::String(ev.anchor.message_id.clone()),
    );
    p.insert("mentioned".into(), Value::Bool(ev.mentioned));
    p.insert("route".into(), Value::String(route.into()));
    if let Some(text) = text {
        p.insert("text".into(), Value::String(text));
    }
    p
}

/// 追问那条额外带上用户说的话（截断口径同卡片标题）。
///
/// 为什么这一条要带 `text` 而建任务那条不带：建任务那条紧挨着的 `task_created` 已经把
/// 用户原话截进 `title` 了，再抄一遍是噪音；追问这条旁边什么都没有，不带的话
/// `model_call` 只留 `messages_hash`（W8：模型消息全文不进证据），从证据里反推不出
/// 「用户中途把要求改成了什么」。
fn steer_payload(ev: &NormalizedEvent) -> Map<String, Value> {
    event_payload(ev, ROUTE_STEER, Some(clip(&ev.text, MAX_TITLE_CHARS)))
}

/// 32 hex（Python 的 `secrets.token_hex(16)`）。
fn token_hex_16() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// 发起人显示名：`config_snapshot["initiator_name"]`，空则退回 `created_by`。
fn initiator_of(session: &Session) -> String {
    session
        .config_snapshot
        .get("initiator_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| session.created_by.clone())
}

/// RΩ 组装控制面时的入参（§3.2「各轨对外暴露的构造入口」）。
pub struct ControlDeps {
    pub store: Arc<dyn SessionStore>,
    pub platform: Arc<dyn PlatformPort>,
    pub evidence: Arc<dyn EvidenceWriter>,
    pub config: AiteConfig,
    pub worker: Option<Arc<dyn TaskWorker>>,
    pub gateway: Option<Arc<dyn ToolGateway>>,
    pub sandbox: Option<Arc<dyn SandboxPort>>,
    /// manifest 里 `model` 字段的退路（Python 用 `getattr(self._model, "name", "")`）。
    /// 没被领走就被停的任务 `task.model` 可能是空串，那时用这个。
    pub model_name: String,
}

/// 控制面的内部状态快照 —— 测试与排障用（RΩ 组装不需要它）。
///
/// 存在的理由：Python 那边的用例直接读 `plane._steer` / `plane._owned` 断言
/// 「连空条目都不许留」（`tests/control/test_steer_routing.py`）。Rust 的集成测试进不了
/// 私有字段，又不该为此把内部容器公开出去，所以给一个只读快照。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlaneState {
    /// 排队中的 task_id（队首在前）
    pub queued: Vec<String>,
    /// 本进程接手过、还没收尾的（排序后）
    pub owned: Vec<String>,
    /// 正在 worker 手上的（排序后）
    pub running: Vec<String>,
    /// 已取消的（排序后）
    pub cancelled: Vec<String>,
    /// steer 队列的全部条目（按 task_id 排序）；**空条目也会出现在这里**
    pub steer: Vec<(String, Vec<String>)>,
}

/// 跨闭包共享的可变状态。
///
/// 单独拎出来是因为 `RunHooks` 里那两个回调是 `Arc<dyn Fn() + 'static>`，没有生命周期
/// 参数可借 `&self`，只能捕获一个 `Arc`。
///
/// **锁序（全局，不许反向取）**：`cancelled` → `owned` → `running` → `steer` → `counters`。
/// 同时持有两把以上的地方只有三处，都按这个顺序：`dispatch_task` 的准入
/// （cancelled → running）、`cancel_task` 的同序对偶（cancelled → running）、
/// `continue_session` 的 steer 目标判定（owned → running）与 `FinishGuard::drop`
/// （owned → steer）。加新的组合前先回来看这一行。
struct Shared {
    queue: DispatchQueue,
    /// 待合并的追问文本，只在内存（进程一换就没了，见 test_steer_routing ①）
    steer: Mutex<HashMap<String, Vec<String>>>,
    /// 已取消的 task_id；worker 每步开头查
    cancelled: Mutex<HashSet<String>>,
    /// 正在 worker 手上
    running: Mutex<HashSet<String>>,
    /// 本进程接手过、还没收尾的（排队中 + 在跑）；孤儿判定靠它
    owned: Mutex<HashSet<String>>,
    counters: Mutex<BTreeMap<String, i64>>,
}

impl Shared {
    fn bump(&self, key: &str) {
        *lock(&self.counters).entry(key.to_string()).or_insert(0) += 1;
    }

    fn counter(&self, key: &str) -> i64 {
        lock(&self.counters).get(key).copied().unwrap_or(0)
    }

    fn drain_steer(&self, task_id: &str) -> Vec<String> {
        lock(&self.steer).remove(task_id).unwrap_or_default()
    }
}

/// ControlPlane（T2）。进程内队列 + 单 worker，P0 只跑单副本。
pub struct InProcessControlPlane {
    store: Arc<dyn SessionStore>,
    platform: Arc<dyn PlatformPort>,
    evidence: Arc<dyn EvidenceWriter>,
    config: AiteConfig,
    worker: Option<Arc<dyn TaskWorker>>,
    gateway: Option<Arc<dyn ToolGateway>>,
    sandbox: Option<Arc<dyn SandboxPort>>,
    model_name: String,
    now: WallClock,
    sleep: SleepFn,
    reaper_interval_sec: f64,
    shared: Arc<Shared>,
    /// 分配 turn.seq 的临界区。为什么要有它见 [`InProcessControlPlane::append_turn`]。
    turn_seq_lock: tokio::sync::Mutex<()>,
}

/// `_run_task` 那圈 `finally`：任务是跑完了、还是压根没被领走（已取消 / 记录没了 /
/// 没配 worker），排在它名下的 steer 都不会再有人来 drain。
struct FinishGuard<'a> {
    shared: &'a Shared,
    task_id: &'a str,
}

impl Drop for FinishGuard<'_> {
    fn drop(&mut self) {
        // 两个容器必须在**同一个临界区**里清：只清掉 steer 就放手的话，
        // `continue_session` 的 `steer_target`（它持 owned + running）还会把这个任务
        // 挑成 steer 目标，于是一条追问排进一个再也不会被 drain 的队列。
        // 取锁顺序照本文件的全局序 owned → steer（见 `Shared` 的注释），不反向取。
        let mut owned = lock(&self.shared.owned);
        let mut steer = lock(&self.shared.steer);
        steer.remove(self.task_id);
        owned.remove(self.task_id);
    }
}

/// `run_one` 那圈 `finally`：不管是跑完、报错，还是整条 future 被 `abort()` 丢掉，
/// 队列的完成计数都要落一笔。
struct TaskDoneGuard<'a> {
    shared: &'a Shared,
}

impl Drop for TaskDoneGuard<'_> {
    fn drop(&mut self) {
        self.shared.queue.task_done();
    }
}

/// `_dispatch_task` 那圈 `finally`。
struct RunningGuard<'a> {
    shared: &'a Shared,
    task_id: &'a str,
}

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        lock(&self.shared.running).remove(self.task_id);
    }
}

impl InProcessControlPlane {
    pub fn new(deps: ControlDeps) -> Self {
        Self {
            store: deps.store,
            platform: deps.platform,
            evidence: deps.evidence,
            config: deps.config,
            worker: deps.worker,
            gateway: deps.gateway,
            sandbox: deps.sandbox,
            model_name: deps.model_name,
            now: Arc::new(Utc::now),
            sleep: Arc::new(|secs: f64| {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_secs_f64(secs.max(0.0))).await;
                })
            }),
            reaper_interval_sec: REAPER_INTERVAL_SEC,
            shared: Arc::new(Shared {
                queue: DispatchQueue::new(),
                steer: Mutex::new(HashMap::new()),
                cancelled: Mutex::new(HashSet::new()),
                running: Mutex::new(HashSet::new()),
                owned: Mutex::new(HashSet::new()),
                counters: Mutex::new(BTreeMap::new()),
            }),
            turn_seq_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// 注入墙钟（测试）。落库的 `created_at` / `updated_at` / `archived_at` 都走它。
    pub fn with_clock(mut self, clock: WallClock) -> Self {
        self.now = clock;
        self
    }

    /// 注入 sleep（测试）。只有 reaper 用得着。
    pub fn with_sleep(mut self, sleep: SleepFn) -> Self {
        self.sleep = sleep;
        self
    }

    /// 改 reaper 的节奏（测试）。默认 [`REAPER_INTERVAL_SEC`]。
    pub fn with_reaper_interval_sec(mut self, secs: f64) -> Self {
        self.reaper_interval_sec = secs;
        self
    }

    /// 建表。Python 的 `InProcessControlPlane.init()`（`ControlPlane` trait 里没有它）。
    pub async fn init(&self) -> Result<(), StoreError> {
        self.store.init().await
    }

    /// 排队中的 steer 消息（R6）。worker 每步开始前会把它们合并进上下文。**只读不消费。**
    pub fn pending_steer(&self, task_id: &str) -> Vec<String> {
        lock(&self.shared.steer)
            .get(task_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 单个计数器，没被碰过就是 0（对齐 Python 的 `defaultdict(int)`）。
    pub fn counter(&self, key: &str) -> i64 {
        self.shared.counter(key)
    }

    /// 内部状态快照，见 [`PlaneState`]。
    pub fn state(&self) -> PlaneState {
        let mut owned: Vec<String> = lock(&self.shared.owned).iter().cloned().collect();
        owned.sort();
        let mut running: Vec<String> = lock(&self.shared.running).iter().cloned().collect();
        running.sort();
        let mut cancelled: Vec<String> = lock(&self.shared.cancelled).iter().cloned().collect();
        cancelled.sort();
        let mut steer: Vec<(String, Vec<String>)> = lock(&self.shared.steer)
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        steer.sort();
        PlaneState {
            queued: self.shared.queue.queued_ids(),
            owned,
            running,
            cancelled,
            steer,
        }
    }

    fn now(&self) -> DateTime<Utc> {
        (self.now)()
    }

    // ---- §3.5 路由 -------------------------------------------------------

    /// R1–R8，按编号顺序求值、命中即停。
    async fn route(&self, ev: &NormalizedEvent) -> Result<(), IngressError> {
        // R1：非真人一律丢弃。机器人/应用/系统消息永远不触发任务（含 Aite 自己发的）
        if ev.sender_kind != SenderKind::Human {
            self.shared.bump("events.nonhuman");
            return Ok(());
        }

        // R2：重连后平台重推的重复事件
        if self.store.seen_event(&ev.event_id).await? {
            self.shared.bump("events.duplicate");
            return Ok(());
        }

        // R3：卡片回传，不建会话
        if ev.kind == EventKind::CardAction {
            return self.on_card_action(ev).await;
        }

        // R4：编辑/删除/成员变动，不触发任务
        if R4_KINDS.contains(&ev.kind) {
            return self.on_non_message(ev).await;
        }

        // 每条 message 事件都先做一次 find_session_by_thread —— R5 要用它判「在不在话题里」
        let session = self.thread_session(ev, false).await?;
        let text = ev.text.trim().to_string();

        // R5：`!` 命令，先于 R6/R7 判定；只接受 @ 过的或已在话题内的消息
        if text.starts_with('!') && (ev.mentioned || session.is_some()) {
            return self.on_command(ev, session, &text).await;
        }

        // R6：话题内续接，不要求 mentioned
        if ev.chat_type == ChatType::Group
            && let Some(session) = session
        {
            return self.continue_session(ev, session).await;
        }

        // R7：@ 了 Aite → 新建 task session，本条消息成为话题 root
        if ev.mentioned {
            let thread_id = ev.anchor.message_id.clone();
            let text = ev.text.clone();
            self.new_session(ev, &thread_id, &text, true).await?;
            return Ok(());
        }

        // R8：其余丢弃（P0 无群会话、无 DM 主动监听）
        self.shared.bump("events.ignored");
        Ok(())
    }

    // ---- R3 --------------------------------------------------------------

    async fn on_card_action(&self, ev: &NormalizedEvent) -> Result<(), IngressError> {
        let Some(action) = ev.card_action.as_ref() else {
            self.shared.bump("events.bad_card_action");
            return Ok(());
        };
        match action.action {
            CardActionKind::Stop => {
                let task = self
                    .resolve_task(&ev.chat_id, action.task_id.as_deref(), &action.card_id)
                    .await?;
                match task {
                    None => self.reply(ev, NO_SUCH_TASK_TEXT).await?,
                    Some(task) => {
                        self.cancel_task(
                            task,
                            Some(ev.anchor.message_id.clone()),
                            Some(ev.chat_id.clone()),
                            true,
                        )
                        .await;
                    }
                }
            }
            CardActionKind::Evidence => {
                let task_id = action.task_id.clone().unwrap_or_default();
                // 契约里 EvidenceWriter 显式有 task_dir（D3），不再靠 getattr 探测
                let where_ = self.evidence.task_dir(&task_id).display().to_string();
                self.reply(ev, &format!("任务 {task_id} 的证据目录：{where_}"))
                    .await?;
            }
        }
        Ok(())
    }

    // ---- R4 --------------------------------------------------------------

    async fn on_non_message(&self, ev: &NormalizedEvent) -> Result<(), IngressError> {
        match ev.kind {
            EventKind::MessageEdited => {
                // 编辑即使加上 @Aite 也不启动任务，只在命中的会话里留一条 system_note
                if let Some(session) = self.thread_session(ev, true).await? {
                    let content = format!("[用户修改了消息] 新内容：{}", ev.text);
                    self.append_turn(&session, ev, TurnRole::SystemNote, &content)
                        .await?;
                }
                self.shared.bump("events.edited");
            }
            EventKind::MessageDeleted => self.shared.bump("events.deleted"),
            other => self.shared.bump(&format!("events.{}", other.as_str())),
        }
        Ok(())
    }

    // ---- R5 --------------------------------------------------------------

    async fn on_command(
        &self,
        ev: &NormalizedEvent,
        session: Option<Session>,
        text: &str,
    ) -> Result<(), IngressError> {
        let (name, rest) = parse_command(text);
        // 计数器 key 是拼出来的：`commands!status`（§9 第 2 条）
        self.shared.bump(&format!("commands{name}"));

        match name.as_str() {
            "!status" => self.cmd_status(ev).await,
            "!stop" => self.cmd_stop(ev, &rest).await,
            "!restart" => self.cmd_restart(ev, session, &rest).await,
            "!new" => self.cmd_new(ev, &rest).await,
            _ => self.reply(ev, UNKNOWN_COMMAND_TEXT).await,
        }
    }

    /// `!status` 要列的任务。
    ///
    /// 库里那一半是 `list_active_tasks`（口径 `status in ACTIVE_TASK_STATUSES` =
    /// created / planning / working）。那三个值在契约里冻结，还被两条冻结测试逐值钉着
    /// （`contracts` 的 `frozen_values.rs` 与 `roundtrip.rs`），**不动它**。
    ///
    /// **另一半是控制面自己的 `running`。** 漏的是这一条路：worker 的 `deliver()` 在
    /// 「第一步就 final、一张卡片都没发过」那一路把状态落成 `Answering`
    /// （`worker/src/agent.rs` 里 `let answering = !ctx.card.sent()`），而 `Answering`
    /// **不在**活跃口径里 —— `roundtrip.rs` 专门有一条断言钉着 `!Answering.is_active()`。
    /// 于是从落 `Answering` 到 `finish()` 落 `Delivered` 之间那几笔（逐个发产物、
    /// send_text、写 delivered 证据、收卡片）任务从 `!status` 里整个消失，
    /// 用户这时问一句，得到的是「本群没有活跃任务」。
    ///
    /// Python 原版一模一样（`store.py` 的 ACTIVE_TASK_STATUSES 同三值，`plane.py` 的
    /// `!status` 同样只走 `list_active_tasks`），所以这不是 Rust 引入的偏差。补它也
    /// **不需要动契约**：`!status` 本来就该回答「现在到底什么情况」，一个正在把文件
    /// 发给你的任务不该从这句话里消失。
    ///
    /// **取锁**：只取 `running` 一把，抄完即放，之后才 await（std 的 Mutex 不许跨 await）。
    /// 全程没有第二把，`Shared` 那条全局锁序（cancelled → owned → running → steer →
    /// counters）自然成立。
    async fn status_tasks(&self, chat_id: &str) -> Result<Vec<Task>, IngressError> {
        let mut tasks = self.store.list_active_tasks(chat_id).await?;
        let extra: Vec<String> = {
            let listed: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
            lock(&self.shared.running)
                .iter()
                .filter(|id| !listed.contains(id.as_str()))
                .cloned()
                .collect()
        };
        for task_id in extra {
            let Some(task) = self.store.get_task(&task_id).await? else {
                continue;
            };
            // 抄的是快照：`RunningGuard` 摘条目和 worker 落终态之间有先后，
            // 已经收场的不该被这句话重新拉出来。
            if task.status.is_terminal() {
                continue;
            }
            // `running` 是进程级的，别把别的群的任务串进这一句。
            let same_chat = self
                .store
                .get_session(&task.session_id)
                .await?
                .is_some_and(|s| s.chat_id == chat_id);
            if !same_chat {
                continue;
            }
            tasks.push(task);
        }
        // 与 §3.2 给 `list_active_tasks` 的承诺同序：(created_at, id) 正序。
        tasks.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(tasks)
    }

    /// 取消的那一句回音（`!stop` / 卡片 stop 按钮走到这里；收尾那条路 `notify=false`）。
    async fn notify_cancelled(
        &self,
        task: &Task,
        session: Option<&Session>,
        reply_to: Option<String>,
        chat_id: Option<String>,
    ) {
        let chat_id =
            chat_id.unwrap_or_else(|| session.map(|s| s.chat_id.clone()).unwrap_or_default());
        let msg = OutboundText {
            chat_id,
            text: format!("任务 {} 已停止。", task.task_no),
            reply_to,
            in_thread: true,
        };
        if let Err(e) = self.platform.send_text(&msg).await {
            tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.notify_failed");
        }
    }

    async fn cmd_status(&self, ev: &NormalizedEvent) -> Result<(), IngressError> {
        let tasks = self.status_tasks(&ev.chat_id).await?;
        if tasks.is_empty() {
            let text = format!("{NO_ACTIVE_TASK_TEXT}{}", self.dropped_note());
            return self.reply(ev, &text).await;
        }
        let lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                let title = if t.title.is_empty() {
                    "(无标题)"
                } else {
                    t.title.as_str()
                };
                format!("{} {} {}", t.task_no, t.status.as_str(), title)
            })
            .collect();
        let text = format!(
            "本群活跃任务：\n{}{}",
            lines.join("\n"),
            self.dropped_note()
        );
        self.reply(ev, &text).await
    }

    async fn cmd_stop(&self, ev: &NormalizedEvent, rest: &str) -> Result<(), IngressError> {
        let Some(task) = self.resolve_stop_target(&ev.chat_id, rest).await? else {
            return self.reply(ev, NO_SUCH_TASK_TEXT).await;
        };
        self.cancel_task(
            task,
            Some(ev.anchor.message_id.clone()),
            Some(ev.chat_id.clone()),
            true,
        )
        .await;
        Ok(())
    }

    async fn cmd_restart(
        &self,
        ev: &NormalizedEvent,
        session: Option<Session>,
        rest: &str,
    ) -> Result<(), IngressError> {
        let mut stopped = 0usize;
        if let Some(mut session) = session.clone() {
            // 归档的会话不该留着还在跑的任务：它们的结果会落进一个已经不存在的会话里，
            // 而且会继续出现在 !status 里。§3.3 的 cancel 路径正好是它们该有的收尾。
            for t in self.store.list_active_tasks(&ev.chat_id).await? {
                if t.session_id == session.id {
                    self.cancel_task(t, None, None, false).await;
                    stopped += 1;
                }
            }
            session.status = SessionStatus::Archived;
            session.archived_at = Some(self.now());
            self.store.update_session(&session).await?;
        }
        // 用其余文本新建：还在原话题里的话就继续用同一个 thread_id
        let thread_id = session
            .as_ref()
            .and_then(|s| s.anchor.thread_id.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| ev.anchor.message_id.clone());
        self.new_session(ev, &thread_id, rest, !rest.is_empty())
            .await?;

        let note = if stopped > 0 {
            format!("终止了 {stopped} 个进行中的任务。")
        } else {
            String::new()
        };
        if rest.is_empty() {
            self.reply(ev, &format!("{note}{RESTART_EMPTY_TEXT}")).await
        } else if !note.is_empty() {
            self.reply(ev, &format!("已重开会话，{note}")).await
        } else {
            // rest 非空且没停掉任何任务 → 不回帖（§9 第 3 条钉住的现状）
            Ok(())
        }
    }

    async fn cmd_new(&self, ev: &NormalizedEvent, rest: &str) -> Result<(), IngressError> {
        // 强制新建，即使已经在别的话题里：本条消息成为新话题的 root
        let thread_id = ev.anchor.message_id.clone();
        self.new_session(ev, &thread_id, rest, !rest.is_empty())
            .await?;
        if rest.is_empty() {
            return self.reply(ev, RESTART_EMPTY_TEXT).await;
        }
        // rest 非空 → 不回帖（§9 第 3 条）
        Ok(())
    }

    /// 丢过事件才多说这一句，平时一个字都不加。
    ///
    /// 这条警告是**进程级**的（不分群）：`events.dropped` 数的是路由抛出去的事件，
    /// 而抛在 `seen_event` 上时连 `chat_id` 归谁都还没走到，分不了群。
    fn dropped_note(&self) -> String {
        let n = self.shared.counter("events.dropped");
        if n == 0 {
            return String::new();
        }
        format!(
            "\n⚠ 本进程启动以来有 {n} 条事件没接住（多半是存储异常），\
可能有消息没被处理。翻日志看 ingress.handle_failed。"
        )
    }

    async fn resolve_stop_target(
        &self,
        chat_id: &str,
        raw: &str,
    ) -> Result<Option<Task>, IngressError> {
        let tasks = self.store.list_active_tasks(chat_id).await?;
        if raw.is_empty() {
            // 只有一个活跃任务时允许省略任务号
            if tasks.len() == 1 {
                return Ok(tasks.into_iter().next());
            }
            return Ok(None);
        }
        let want = normalize_task_no(raw);
        Ok(tasks.into_iter().find(|t| t.task_no == want))
    }

    async fn resolve_task(
        &self,
        chat_id: &str,
        task_id: Option<&str>,
        card_id: &str,
    ) -> Result<Option<Task>, IngressError> {
        let tasks = self.store.list_active_tasks(chat_id).await?;
        if let Some(task_id) = task_id.filter(|t| !t.is_empty()) {
            return Ok(tasks.into_iter().find(|t| t.id == task_id));
        }
        Ok(tasks.into_iter().find(|t| {
            t.card_id
                .as_deref()
                .is_some_and(|c| !c.is_empty() && c == card_id)
        }))
    }

    // ---- R6 / R7 ---------------------------------------------------------

    async fn continue_session(
        &self,
        ev: &NormalizedEvent,
        mut session: Session,
    ) -> Result<(), IngressError> {
        let text = ev.text.clone();
        self.append_turn(&session, ev, TurnRole::User, &text)
            .await?;
        session.last_active_at = self.now();
        self.store.update_session(&session).await?;

        // 先把库读完再取锁：`std::sync::Mutex` 的 guard 不许跨 await 活着（§7.6）。
        let listed = self.store.list_active_tasks(&session.chat_id).await?;
        let active: Vec<Task> = {
            let cancelled = lock(&self.shared.cancelled);
            listed
                .into_iter()
                .filter(|t| t.session_id == session.id && !cancelled.contains(&t.id))
                .collect()
        };

        let target = {
            let owned = lock(&self.shared.owned);
            let running = lock(&self.shared.running);
            steer_target(&active, &owned, &running)
        };

        if let Some(target) = target {
            // 先写证据再排队，和 `start_task` 一个顺序：证据链宁可少一条也不撒谎，
            // 不能出现「任务收到了这句追问，但链上查不到它是什么时候来的」。
            self.evidence
                .append(&target.id, EvidenceKind::EventReceived, steer_payload(ev))
                .await?;
            // worker 每步开始前会把这些合并进上下文
            lock(&self.shared.steer)
                .entry(target.id.clone())
                .or_default()
                .push(ev.text.clone());
            self.shared.bump("events.steer");
            return Ok(());
        }
        if !active.is_empty() {
            // 库里还是 active，可本进程既没在跑它也没排过它 —— 上个进程被杀之后留在
            // working 上的孤儿，没人会来 drain 它的 steer。按 R6 的「否则新建 task
            // 继续」走：不然用户这句追问排进一个永不消费的队列，看起来就是 Aite 没反应。
            self.shared.bump("events.orphan_task");
        }
        self.start_task(&session, ev, None).await?;
        Ok(())
    }

    async fn new_session(
        &self,
        ev: &NormalizedEvent,
        thread_id: &str,
        text: &str,
        react: bool,
    ) -> Result<Session, IngressError> {
        let now = self.now();
        let mut config_snapshot = Map::new();
        config_snapshot.insert(
            "model".into(),
            Value::String(self.config.model.model.clone()),
        );
        config_snapshot.insert(
            "initiator_name".into(),
            Value::String(
                ev.sender_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| ev.sender_id.clone()),
            ),
        );
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            tenant_id: ev.tenant_id.clone(),
            workspace_id: ev.workspace_id.clone(),
            chat_id: ev.chat_id.clone(),
            kind: SessionKind::Task,
            anchor: Anchor {
                platform: ev.platform.clone(),
                chat_id: ev.chat_id.clone(),
                message_id: ev.anchor.message_id.clone(),
                thread_id: Some(thread_id.to_string()),
                task_no: None,
            },
            status: SessionStatus::Active,
            created_by: ev.sender_id.clone(),
            config_snapshot,
            created_at: now,
            last_active_at: now,
            archived_at: None,
        };
        self.store.create_session(&session).await?;
        if react {
            // ack 失败不影响建任务（Python 的 contextlib.suppress）
            if let Err(e) = self
                .platform
                .add_reaction(&ev.anchor.message_id, ReactionKind::Ack)
                .await
            {
                tracing::warn!(target: "aite.control", error = %e, "control.ack_failed");
            }
        }
        if !text.is_empty() {
            self.append_turn(&session, ev, TurnRole::User, text).await?;
            self.start_task(&session, ev, Some(text)).await?;
        }
        Ok(session)
    }

    /// 六步顺序严格：造 Task → `create_task` → `task_created` 证据 → `event_received`
    /// 证据 → `_owned` → 入队。证据链前两条永远是 `task_created, event_received`。
    async fn start_task(
        &self,
        session: &Session,
        ev: &NormalizedEvent,
        text: Option<&str>,
    ) -> Result<Task, IngressError> {
        let now = self.now();
        let task_no = self.store.next_task_no(&session.tenant_id).await?;
        let task = Task {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            task_no,
            status: TaskStatus::Created,
            title: clip(text.unwrap_or(&ev.text), MAX_TITLE_CHARS),
            checklist: Vec::new(),
            card_id: None,
            sandbox_id: None,
            session_token: token_hex_16(),
            model: self.config.model.model.clone(),
            steps: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            max_steps: self.config.worker.max_steps,
            max_wall_sec: self.config.worker.max_wall_sec,
            result_summary: String::new(),
            evidence_root_hash: None,
            created_by: ev.sender_id.clone(),
            created_at: now,
            updated_at: now,
        };
        self.store.create_task(&task).await?;

        let mut created = Map::new();
        created.insert("session_id".into(), Value::String(session.id.clone()));
        created.insert("task_no".into(), Value::String(task.task_no.clone()));
        created.insert("chat_id".into(), Value::String(session.chat_id.clone()));
        created.insert("created_by".into(), Value::String(task.created_by.clone()));
        created.insert("title".into(), Value::String(task.title.clone()));
        self.evidence
            .append(&task.id, EvidenceKind::TaskCreated, created)
            .await?;
        self.evidence
            .append(
                &task.id,
                EvidenceKind::EventReceived,
                event_payload(ev, ROUTE_NEW_TASK, None),
            )
            .await?;

        lock(&self.shared.owned).insert(task.id.clone());
        self.shared.queue.put(task.id.clone());
        Ok(task)
    }

    // ---- 任务派发 --------------------------------------------------------

    async fn dispatch_loop(&self) {
        loop {
            let task_id = self.shared.queue.pop().await;
            self.run_one(&task_id).await;
        }
    }

    async fn run_one(&self, task_id: &str) {
        // `task_done()` 必须是 drop-safe（Python 那边是 `finally`）：收尾时
        // `runner.abort()` 会在任意一个 await 点把这条 future 整个丢掉，而 `join()`
        // 等的就是这个计数 —— 漏一次，队列永远清不空，下一次优雅退出就卡满整个宽限期，
        // 而且 `pending()` 会一直报一个不存在的任务。
        let _done = TaskDoneGuard {
            shared: &self.shared,
        };
        // §3.3 任何未捕获异常都不能让进程退出
        if let Err(e) = self.run_task(task_id).await {
            tracing::error!(target: "aite.control", task = %task_id, error = %e, "control.dispatch_failed");
        }
    }

    async fn run_task(&self, task_id: &str) -> Result<(), IngressError> {
        let _finish = FinishGuard {
            shared: &self.shared,
            task_id,
        };
        self.dispatch_task(task_id).await
    }

    async fn dispatch_task(&self, task_id: &str) -> Result<(), IngressError> {
        let Some(task) = self.store.get_task(task_id).await? else {
            return Ok(());
        };
        let Some(session) = self.store.get_session(&task.session_id).await? else {
            return Ok(());
        };
        let Some(worker) = self.worker.clone() else {
            tracing::warn!(target: "aite.control", task = %task_id, "control.no_worker");
            return Ok(());
        };
        // 「查 cancelled」与「登记 running」必须在同一个临界区。Python 靠单事件循环
        // 白拿这条：`plane.py:505-511` 从判 `_cancelled` 到 `_running.add` 之间没有 await。
        // Rust 多线程 runtime 下拆成两次独立取锁就开了窗口 —— cancel_task 正好在中间
        // 抄到 running 为空，于是走「不在跑」分支写一条 cancelled 证据 + finalize，
        // 而 worker 下一步见 is_cancelled 为真又按契约自己收一次尾：同一条链两条
        // cancelled，manifest 也可能 finalize 两遍。
        // 取锁顺序与 cancel_task 侧一致（cancelled → running），全文件没有反向获取点。
        let admitted = {
            let cancelled = lock(&self.shared.cancelled);
            if cancelled.contains(task_id) {
                false
            } else {
                lock(&self.shared.running).insert(task_id.to_string());
                true
            }
        };
        if !admitted {
            return Ok(());
        }
        // 开跑前排进来的 steer 已经在 transcript 里了：`continue_session` 先 append_turn
        // 再排队，而 worker 的 `_build_messages` 是现在才去 list_turns。不清掉的话第一步
        // 开头会把同一句话再注入一遍，上下文里出现两条一样的用户发言。
        lock(&self.shared.steer).remove(task_id);
        let _running = RunningGuard {
            shared: &self.shared,
            task_id,
        };

        let initiator = initiator_of(&session);
        let hooks = RunHooks {
            drain_steer: {
                let shared = Arc::clone(&self.shared);
                let id = task_id.to_string();
                Arc::new(move || shared.drain_steer(&id))
            },
            is_cancelled: {
                let shared = Arc::clone(&self.shared);
                let id = task_id.to_string();
                Arc::new(move || lock(&shared.cancelled).contains(&id))
            },
        };
        worker.run(task, session, Some(initiator), hooks).await;
        Ok(())
    }

    /// W7：**先 sleep 后 reap**，异常不打断循环。
    async fn reaper_loop(&self) {
        loop {
            (self.sleep)(self.reaper_interval_sec).await;
            let Some(sandbox) = self.sandbox.as_ref() else {
                continue;
            };
            match sandbox.reap_idle(self.config.sandbox.idle_sec).await {
                Err(e) => {
                    tracing::error!(target: "aite.control", error = %e, "control.reap_failed");
                    continue;
                }
                Ok(released) if !released.is_empty() => {
                    let n = released.len();
                    for _ in 0..n {
                        self.shared.bump("sandbox.reaped");
                    }
                    tracing::info!(target: "aite.control", n, "control.reaped");
                }
                Ok(_) => {}
            }
        }
    }

    // ---- 取消（!stop / 卡片 stop 按钮，§3.3）-----------------------------

    async fn finalize_evidence(&self, task: &mut Task, session: Option<&Session>) {
        // `finalize` 每个任务只该走一次：worker 收过尾的任务 `evidence_root_hash` 已经有值，
        // 再 finalize 一遍会把它算进这条 cancelled 之后，manifest 就和 worker 写的那份对不上了。
        if task
            .evidence_root_hash
            .as_deref()
            .is_some_and(|h| !h.is_empty())
        {
            return;
        }
        let mut extra = Map::new();
        extra.insert(
            "session_id".into(),
            Value::String(
                session
                    .map(|s| s.id.clone())
                    .unwrap_or_else(|| task.session_id.clone()),
            ),
        );
        extra.insert("task_no".into(), Value::String(task.task_no.clone()));
        extra.insert("created_by".into(), Value::String(task.created_by.clone()));
        // Task.model 建任务时取的是 config.model.model（默认空串），worker 开跑才覆盖成
        // 真模型名。没被领走就被停的任务遇上空配置就还是空的，退回本进程装着的模型名。
        extra.insert(
            "model".into(),
            Value::String(if task.model.is_empty() {
                self.model_name.clone()
            } else {
                task.model.clone()
            }),
        );
        match self.evidence.finalize(&task.id, extra).await {
            Ok(root_hash) => {
                task.evidence_root_hash = Some(root_hash);
                // 上面那次 update_task 在 finalize 之前，root_hash 得靠这一次才进库。
                task.updated_at = self.now();
                if let Err(e) = self.store.update_task(task).await {
                    tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.cancel_save_failed");
                }
            }
            Err(e) => {
                tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.finalize_failed");
            }
        }
    }

    /// 让 Gateway 释放它给这个 task 建的沙箱并撤掉 token。
    ///
    /// 沙箱是 Gateway 按 task_id 自己记着的（`Task.sandbox_id` 只记 worker 兜底建的那种），
    /// 契约里已经把这个方法显式化了（D2），不再靠 getattr 探测。
    async fn release_gateway_sandbox(&self, task_id: &str) {
        if let Some(gateway) = self.gateway.as_ref() {
            gateway.release_task(task_id).await;
        }
    }

    // ---- 小工具 ----------------------------------------------------------

    async fn thread_session(
        &self,
        ev: &NormalizedEvent,
        fallback_to_message_id: bool,
    ) -> Result<Option<Session>, IngressError> {
        if let Some(thread_id) = ev.anchor.thread_id.as_deref().filter(|t| !t.is_empty())
            && let Some(hit) = self
                .store
                .find_session_by_thread(&ev.chat_id, thread_id)
                .await?
        {
            return Ok(Some(hit));
        }
        if fallback_to_message_id {
            // 被编辑的可能正是话题 root 那条消息
            return Ok(self
                .store
                .find_session_by_thread(&ev.chat_id, &ev.anchor.message_id)
                .await?);
        }
        Ok(None)
    }

    /// seq 由调用方分配（§3.2）。只用协议里有的 `list_turns` 推下一个 seq，
    /// 这样换任何 SessionStore 实现都成立。
    ///
    /// 读 seq 和写 turn 之间不许有别人插进来。同一个话题里的两条事件**是会同时**进
    /// `handle_event` 的 —— 重连那一刻整批一起上来，而这中间隔着两次真会挂起的存储往返。
    /// 两条都读到「还没有 turn」就都写 seq=0，第二条撞上 (session_id, seq) 唯一约束报
    /// `DuplicateTurn`；这一报发生在 `seen_event` **已经落库之后**，于是事件既没变成任务，
    /// 也永远不会被重推第二次（R2 认得它了）—— 用户那句话就此消失。M2 后半句在这里破。
    ///
    /// P0 是单进程单副本，进程内一把锁就够。
    async fn append_turn(
        &self,
        session: &Session,
        ev: &NormalizedEvent,
        role: TurnRole,
        content: &str,
    ) -> Result<(), IngressError> {
        let _seq_guard = self.turn_seq_lock.lock().await;
        let recent = self.store.list_turns(&session.id, 1).await?;
        let seq = recent.last().map(|t| t.seq + 1).unwrap_or(0);
        self.store
            .append_turn(&Turn {
                session_id: session.id.clone(),
                seq,
                role,
                platform_user_id: Some(ev.sender_id.clone()),
                content: content.to_string(),
                attachments: ev.attachments.clone(),
                created_at: ev.occurred_at,
            })
            .await?;
        Ok(())
    }

    async fn reply(&self, ev: &NormalizedEvent, text: &str) -> Result<(), IngressError> {
        self.platform
            .send_text(&OutboundText {
                chat_id: ev.chat_id.clone(),
                text: text.to_string(),
                reply_to: Some(ev.anchor.message_id.clone()),
                in_thread: true,
            })
            .await?;
        Ok(())
    }
}

/// 这句话该排给哪个任务（R6）。没有能收的就返回 `None`，调用方按「新建 task」走。
///
/// 两条判据：
///
/// 1. **只认本进程接手过的**（`owned`）。`list_active_tasks` 读的是库，杀进程重启后上一批
///    任务还挂在 `working` 上，谁也不会再领它们。
/// 2. **优先给正在跑的那个**。它的 messages 在任务开跑那一刻就定型了，不合并进去就看不到
///    这句话；还躺在队列里的任务不需要 steer —— 它开跑时 `_build_messages` 会从 transcript
///    里读到。
///
/// 多个候选时取最后创建的：§3.2 `SessionStore.list_active_tasks` 虽然承诺了顺序，但这里
/// 按 `(created_at, id)` 自己排，不吃实现的 `ORDER BY`（同毫秒时靠 uuid 字符串比大小）。
pub(crate) fn steer_target(
    active: &[Task],
    owned: &HashSet<String>,
    running: &HashSet<String>,
) -> Option<Task> {
    let owned_tasks: Vec<&Task> = active.iter().filter(|t| owned.contains(&t.id)).collect();
    if owned_tasks.is_empty() {
        return None;
    }
    let running_tasks: Vec<&Task> = owned_tasks
        .iter()
        .copied()
        .filter(|t| running.contains(&t.id))
        .collect();
    let pool = if running_tasks.is_empty() {
        &owned_tasks
    } else {
        &running_tasks
    };
    pool.iter()
        .max_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)))
        .map(|t| (*t).clone())
}

#[async_trait]
impl ControlPlane for InProcessControlPlane {
    /// R1–R8 的唯一入口。按编号顺序求值，命中即停。
    ///
    /// 路由半路的错误照旧往上传（`Ingress::on_event` 兜住它、不让长连接的读循环被带走，
    /// §3.3 第一行），这里只在路过时记一笔。为什么记在这儿而不是复用
    /// `Ingress` 的 `ingress.errors`：`!status` 是控制面回的，控制面拿不到 ingress。
    /// 两个计数器数的是同一批事件，只是待在不同的层上。
    async fn handle_event(&self, ev: NormalizedEvent) -> Result<(), IngressError> {
        match self.route(&ev).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // 日志由 Ingress 打（那里有完整的 event_id / kind），这里不打第二遍。
                self.shared.bump("events.dropped");
                Err(e)
            }
        }
    }

    /// 派发队列里的任务给 worker，同时跑沙箱 reaper（W7）。
    ///
    /// 派发**串行**：一次只跑一个任务（T24 的用例依赖这条）。reaper 和派发在同一个
    /// future 里并行推进，整个 `run_forever` 被 drop / abort 时两条一起停 ——
    /// 对齐 Python 那句 `finally: reaper.cancel()`。
    async fn run_forever(&self) {
        tokio::join!(self.reaper_loop(), self.dispatch_loop());
    }

    /// 把当前排队的任务跑完就返回。给测试和一次性回放用，不起 reaper。
    async fn run_pending(&self) {
        while let Some(task_id) = self.shared.queue.try_pop() {
            self.run_one(&task_id).await;
        }
    }

    fn pending(&self) -> usize {
        self.shared.queue.len()
    }

    /// 等队列里已入队的任务都跑完。`run_forever` 在另一条任务上跑时用。
    async fn join(&self) {
        self.shared.queue.join().await;
    }

    async fn cancel_task(
        &self,
        task: Task,
        reply_to: Option<String>,
        chat_id: Option<String>,
        notify: bool,
    ) -> Task {
        let mut task = task;
        // 与 dispatch_task 侧成对：抄 running 和落 cancelled 必须是同一个临界区，
        // 否则两边会同时认为「对方没接手」，把收尾做两遍。顺序同为 cancelled → running。
        let running = {
            let mut cancelled = lock(&self.shared.cancelled);
            let r = lock(&self.shared.running).contains(&task.id);
            cancelled.insert(task.id.clone());
            r
        };

        // 传进来的 `task` 可能是一份**过期快照**，落刀之前按 id 重读一次。
        //
        // 真会撞上的是收尾那一支：`app` 的 `shutdown()` 在宽限期超时时抄一份
        // `worker.in_flight()`，抄到 `runner.abort()` 生效之间，那个任务完全可能已经
        // 自己跑完了（worker 在别的 task 上，runtime 是 multi_thread，两条线真并行）。
        // 拿旧快照往下走的后果是硬的：`status = Cancelled` + `update_task` 会把库里
        // 一条已经 `delivered` 的任务**改写成 cancelled**，卡片跟着翻成「已取消」——
        // 而用户手上的答复和文件早就收到了；`finalize_evidence` 的幂等判据读的又是
        // 快照里的 `evidence_root_hash`（旧快照那一份是空的），manifest 还会再
        // finalize 一遍，和 worker 写的那份对不上。
        //
        // 所以：库里已经是终态就一个字都不改，只把 notify 走完（卡片 stop 按钮那条路
        // 的用户点了得有回音）。多读一次库，换掉一条对用户说假话的路。
        let settled = match self.store.get_task(&task.id).await {
            Ok(Some(fresh)) if fresh.status.is_terminal() => {
                tracing::info!(
                    target: "aite.control",
                    task = %task.id,
                    status = fresh.status.as_str(),
                    "control.cancel_skipped 任务已经收场，取消不再改写它"
                );
                task = fresh;
                true
            }
            // 读不出来（store 抖动 / 已关）就按原样往下走：这条路原本就是「尽力善终」，
            // 为一次读失败放弃取消，比多写一条 cancelled 更糟。
            Err(e) => {
                tracing::warn!(target: "aite.control", task = %task.id, error = %e, "control.cancel_reread_failed");
                false
            }
            _ => false,
        };
        if settled {
            if notify {
                let session = self
                    .store
                    .get_session(&task.session_id)
                    .await
                    .ok()
                    .flatten();
                self.notify_cancelled(&task, session.as_ref(), reply_to, chat_id)
                    .await;
            }
            return task;
        }

        task.status = TaskStatus::Cancelled;
        task.updated_at = self.now();
        if let Err(e) = self.store.update_task(&task).await {
            // Python 这里会把异常抛给 handle_event；`ControlPlane::cancel_task` 的签名
            // 返回 Task（冻结契约，D5），没有错误通道 —— 不能上抛这条不是我们能改的。
            //
            // 但**落库都没成功就不该接着往下走**：继续写 cancelled 证据 + finalize，
            // 而 finalize 的 root_hash 回写同样会失败 → 下次起飞 recover_orphan_tasks
            // 把它当孤儿再 finalize 一遍（幂等守卫读的正是库里的 evidence_root_hash），
            // 两份 manifest 对不上；再回一句「已停止」更是直接对用户说假话 ——
            // 库里任务还是 created，下一条 !status 照样列着它。
            // 至少要让 `!status` 的那句进程级警告能说出「有事情没办成」。
            tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.cancel_save_failed");
            self.shared.bump("events.dropped");
            return task;
        }
        if let (Some(sandbox), Some(sandbox_id)) = (
            self.sandbox.as_ref(),
            task.sandbox_id.clone().filter(|s| !s.is_empty()),
        ) && let Err(e) = sandbox.release(&sandbox_id).await
        {
            tracing::warn!(target: "aite.control", task = %task.id, error = %e, "control.release_failed");
        }

        let session = match self.store.get_session(&task.session_id).await {
            Ok(session) => session,
            Err(e) => {
                tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.cancel_session_failed");
                None
            }
        };

        // 任务正在 worker 手里跑：证据、收卡片、还沙箱都交给它下一步开头做，那边拿到的
        // 步数才是准的，也免得同一条链上写两次 cancelled。反过来，没在跑的任务 worker
        // 不会再收尾，Gateway 手上那个沙箱只能在这里还。
        if !running {
            self.release_gateway_sandbox(&task.id).await;
            let mut payload = Map::new();
            payload.insert("by".into(), Value::String("stop".into()));
            payload.insert("steps".into(), Value::from(task.steps));
            if let Err(e) = self
                .evidence
                .append(&task.id, EvidenceKind::Cancelled, payload)
                .await
            {
                tracing::error!(target: "aite.control", task = %task.id, error = %e, "control.cancel_evidence_failed");
            }
            if let (Some(session), Some(card_id)) = (
                session.as_ref(),
                task.card_id.clone().filter(|c| !c.is_empty()),
            ) {
                let card = render_card(
                    &task,
                    session,
                    &initiator_of(session),
                    CardStatus::Cancelled,
                    None,
                );
                if let Err(e) = self.platform.update_card(&card_id, &card).await {
                    tracing::warn!(target: "aite.control", task = %task.id, error = %e, "control.cancel_card_failed");
                }
            }
            self.finalize_evidence(&mut task, session.as_ref()).await;
        }
        if notify {
            self.notify_cancelled(&task, session.as_ref(), reply_to, chat_id)
                .await;
        }
        task
    }

    fn counters(&self) -> Map<String, Value> {
        lock(&self.shared.counters)
            .iter()
            .map(|(k, v)| (k.clone(), Value::from(*v)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// 给 `steer_target` 造候选。`minutes` 决定 created_at 的先后。
    fn task(id: &str, minutes: i64) -> Task {
        let now = Utc
            .with_ymd_and_hms(2026, 9, 10, 12, 0, 0)
            .single()
            .expect("固定时间戳")
            + chrono::Duration::minutes(minutes);
        Task {
            id: id.into(),
            session_id: "s1".into(),
            task_no: format!("#A{minutes}"),
            status: TaskStatus::Created,
            title: String::new(),
            checklist: Vec::new(),
            card_id: None,
            sandbox_id: None,
            session_token: "tok".into(),
            model: String::new(),
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
        }
    }

    fn ids(xs: &[&str]) -> HashSet<String> {
        xs.iter().map(|s| (*s).to_string()).collect()
    }

    /// 白盒测 `steer_target`（对应 Python 的 test_steer_target_prefers_the_running_task_and_ignores_orphans）。
    ///
    /// P0 的路由不会让同一会话出现两个活跃 task（`continue_session` 有活跃任务就只排
    /// steer、不新建），所以这个分支从事件面造不出来 —— 直接喂候选列表。
    #[test]
    fn steer_target_prefers_the_running_task() {
        let old = task("t-old", 0);
        let running = task("t-running", 1);
        let newest = task("t-newest", 2);
        let owned = ids(&["t-old", "t-running", "t-newest"]);
        let running_set = ids(&["t-running"]);

        // 1. 正在跑的优先；2. 结果不依赖入参顺序
        for order in [
            vec![old.clone(), running.clone(), newest.clone()],
            vec![newest.clone(), running.clone(), old.clone()],
            vec![running.clone(), newest.clone(), old.clone()],
        ] {
            assert_eq!(
                steer_target(&order, &owned, &running_set).map(|t| t.id),
                Some("t-running".to_string()),
            );
        }
    }

    #[test]
    fn steer_target_falls_back_to_the_newest_owned_task() {
        let old = task("t-old", 0);
        let running = task("t-running", 1);
        let newest = task("t-newest", 2);
        let owned = ids(&["t-old", "t-running", "t-newest"]);
        let empty = HashSet::new();

        // 没有在跑的 → 取最后创建的那个，同样不看入参顺序
        for order in [
            vec![old.clone(), newest.clone(), running.clone()],
            vec![newest.clone(), old.clone(), running.clone()],
        ] {
            assert_eq!(
                steer_target(&order, &owned, &empty).map(|t| t.id),
                Some("t-newest".to_string()),
            );
        }
    }

    #[test]
    fn steer_target_ignores_orphans() {
        let old = task("t-old", 0);
        let running = task("t-running", 1);
        let newest = task("t-newest", 2);
        let nothing = HashSet::new();

        // 全是孤儿 → None，调用方按「新建 task」走
        assert!(steer_target(&[old, running, newest], &nothing, &nothing).is_none());
        assert!(steer_target(&[], &nothing, &nothing).is_none());
    }

    #[test]
    fn token_hex_16_is_32_hex_chars() {
        let token = token_hex_16();
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token, token_hex_16(), "两次取不该一样");
    }
}
