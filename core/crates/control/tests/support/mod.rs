//! `tests/control` 私有的测试替身 —— 对应 `tests/control/control_fakes.py` 与 `conftest.py`。
//!
//! §3.2 末段：并行期间各轨的替身一律私写，不依赖 `aite-testing`（那是 R7 的，
//! 现在还不存在）。「重复远比冲突便宜」。
//!
//! 与 Python 版的两处刻意不同：
//! - `FakeStore` 是内存实现（Python 用真 SqliteSessionStore，那是 R3 的 crate）。
//!   每个 async 方法开头 `yield_now()`，**故意留一个真挂起点** —— 不然
//!   `list_turns → append_turn` 之间根本不会被插进来，seq 竞态那两条用例会变成空转。
//! - `FlakyStore`（Python 里单独一个包装类）并进 `FakeStore::fail_next`，
//!   语义一样：抛在方法体**之前**，抛完即恢复。
#![allow(dead_code)] // 每个集成测试二进制只用到其中一部分

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tokio::sync::watch;

use aite_contracts::{
    AiteConfig, Anchor, Attachment, CardAction, CardActionKind, ChatType, ChecklistCard,
    DocumentContent, EventHandler, EventKind, EvidenceError, EvidenceEvent, EvidenceKind,
    EvidenceWriter, GENESIS, HistoryMessage, NormalizedEvent, OutboundFile, OutboundText,
    PlatformCapabilities, PlatformError, PlatformPort, ReactionKind, RunHooks, SandboxError,
    SandboxPort, SandboxSpec, SendResult, SenderKind, Session, SessionStore, StoreError, Task,
    TaskStatus, TaskWorker, ToolCallRequest, ToolContext, ToolGateway, ToolResult, ToolSpec, Turn,
    chain_hash, encode_task_no, feishu_p0, payload_hash_of,
};
use aite_control::{ControlDeps, InProcessControlPlane, SleepFn, WallClock};

pub const CHAT: &str = "oc_chat";
pub const ROOT: &str = "om_1";

/// 取锁，中毒也不 panic（同 crate 内的 `lock`，测试侧自己一份）。
fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---- 事件构造 ------------------------------------------------------------

/// 对应 Python 的 `make_event(...)`：默认是「张三 @ 了 Aite，在 oc_chat 里发了一条 om_1」。
pub struct EventBuilder {
    event_id: String,
    kind: EventKind,
    text: String,
    mentioned: bool,
    sender_kind: SenderKind,
    sender_id: String,
    sender_name: Option<String>,
    chat_id: String,
    chat_type: ChatType,
    message_id: String,
    thread_id: Option<String>,
    attachments: Vec<Attachment>,
    card_action: Option<CardAction>,
}

pub fn ev() -> EventBuilder {
    EventBuilder {
        event_id: "ev1".into(),
        kind: EventKind::Message,
        text: "你好".into(),
        mentioned: true,
        sender_kind: SenderKind::Human,
        sender_id: "ou_user".into(),
        sender_name: Some("张三".into()),
        chat_id: CHAT.into(),
        chat_type: ChatType::Group,
        message_id: ROOT.into(),
        thread_id: None,
        attachments: Vec::new(),
        card_action: None,
    }
}

impl EventBuilder {
    pub fn id(mut self, v: &str) -> Self {
        self.event_id = v.into();
        self
    }
    pub fn kind(mut self, v: EventKind) -> Self {
        self.kind = v;
        self
    }
    pub fn text(mut self, v: &str) -> Self {
        self.text = v.into();
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
    pub fn sender_id(mut self, v: &str) -> Self {
        self.sender_id = v.into();
        self
    }
    pub fn sender_name(mut self, v: Option<&str>) -> Self {
        self.sender_name = v.map(str::to_string);
        self
    }
    pub fn chat(mut self, v: &str) -> Self {
        self.chat_id = v.into();
        self
    }
    pub fn chat_type(mut self, v: ChatType) -> Self {
        self.chat_type = v;
        self
    }
    pub fn message_id(mut self, v: &str) -> Self {
        self.message_id = v.into();
        self
    }
    pub fn thread(mut self, v: &str) -> Self {
        self.thread_id = Some(v.into());
        self
    }
    pub fn attachments(mut self, v: Vec<Attachment>) -> Self {
        self.attachments = v;
        self
    }
    pub fn card_action(mut self, v: CardAction) -> Self {
        self.card_action = Some(v);
        self
    }
    pub fn build(self) -> NormalizedEvent {
        NormalizedEvent {
            event_id: self.event_id,
            kind: self.kind,
            platform: "fake".into(),
            tenant_id: "default".into(),
            workspace_id: "app_1".into(),
            chat_id: self.chat_id.clone(),
            chat_type: self.chat_type,
            sender_id: self.sender_id,
            sender_kind: self.sender_kind,
            sender_name: self.sender_name,
            text: self.text,
            raw_text: None,
            mentioned: self.mentioned,
            anchor: Anchor {
                platform: "fake".into(),
                chat_id: self.chat_id,
                message_id: self.message_id,
                thread_id: self.thread_id,
                task_no: None,
            },
            attachments: self.attachments,
            card_action: self.card_action,
            occurred_at: Utc::now(),
            raw: Map::new(),
        }
    }
}

pub fn card_action(card_id: &str, action: CardActionKind, task_id: Option<&str>) -> CardAction {
    CardAction {
        card_id: card_id.into(),
        action,
        task_id: task_id.map(str::to_string),
        value: Map::new(),
    }
}

// ---- 时钟与 sleep --------------------------------------------------------

/// 可手动推进的单调钟（Ingress 的慢回调判定读它）。
pub struct FakeClock {
    t: Mutex<f64>,
}

impl FakeClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            t: Mutex::new(1000.0),
        })
    }
    pub fn advance(&self, dt: f64) {
        *lk(&self.t) += dt;
    }
    pub fn now(&self) -> f64 {
        *lk(&self.t)
    }
    pub fn as_mono(self: &Arc<Self>) -> Arc<dyn Fn() -> f64 + Send + Sync> {
        let me = Arc::clone(self);
        Arc::new(move || me.now())
    }
}

/// 固定的墙钟，每次取都加 1 毫秒 —— 保证 `created_at` 严格递增，
/// `steer_target` 的 `(created_at, id)` 排序才有确定答案。
pub struct TickingWallClock {
    next: Mutex<DateTime<Utc>>,
}

impl TickingWallClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next: Mutex::new(Utc::now()),
        })
    }
    pub fn as_wall(self: &Arc<Self>) -> WallClock {
        let me = Arc::clone(self);
        Arc::new(move || {
            let mut g = lk(&me.next);
            let now = *g;
            *g = now + chrono::Duration::milliseconds(1);
            now
        })
    }
}

/// 替掉 reaper 的 `sleep`：前 `limit-1` 次立刻返回（让循环转起来），
/// 第 `limit` 次永久挂住 —— 否则一个不 yield 的假 sleep 会把 reaper 变成空转。
///
/// **「挂住了」这个信号一定要留得下痕迹**：`new()` 当场把建出来的 `_rx` 丢了，唯一订阅它的
/// 地方是 [`Self::wait_until_parked`]。`watch::Sender::send` 在一个活跃接收者都没有时返回
/// `Err` **并且连内部那个值都不改** —— 于是闭包先跑到第 `limit` 次、`wait_until_parked()`
/// 后到的那个时序里，等待方看到的值永远是 `false`，`wait_for` 永远不返回，用例挂死。
/// 药是 [`watch::Sender::send_replace`]（见 `as_sleep`），与 `app` 那边
/// `StopSignal::set()` 逐字同一条 —— 那条是产品缺陷（真机上 SIGTERM 会被吃掉），
/// 这条是同形状的测试替身，一并治掉。
/// 回归见本文件的 [`parking_counts_even_when_nobody_is_waiting_yet`]。
pub struct ParkedSleep {
    limit: usize,
    calls: Mutex<Vec<f64>>,
    parked: watch::Sender<bool>,
}

impl ParkedSleep {
    pub fn new(limit: usize) -> Arc<Self> {
        let (tx, _rx) = watch::channel(false);
        Arc::new(Self {
            limit,
            calls: Mutex::new(Vec::new()),
            parked: tx,
        })
    }

    pub fn calls(&self) -> Vec<f64> {
        lk(&self.calls).clone()
    }

    /// 等到第 `limit` 次 sleep 把自己挂住为止。
    pub async fn wait_until_parked(&self) {
        let mut rx = self.parked.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    pub fn as_sleep(self: &Arc<Self>) -> SleepFn {
        let me = Arc::clone(self);
        Arc::new(move |delay: f64| {
            let me = Arc::clone(&me);
            Box::pin(async move {
                let n = {
                    let mut calls = lk(&me.calls);
                    calls.push(delay);
                    calls.len()
                };
                if n >= me.limit {
                    // `send_replace` 而不是 `send`：这里通常一个接收者都还没有（见类型注释）。
                    me.parked.send_replace(true);
                    std::future::pending::<()>().await;
                }
                tokio::task::yield_now().await;
            })
        })
    }
}

/// **回归**：挂住这件事发生在任何 `wait_until_parked()` 之前也必须算数。
#[tokio::test]
async fn parking_counts_even_when_nobody_is_waiting_yet() {
    let sleeper = ParkedSleep::new(1);

    // 让闭包先跑到 —— `timeout` 先 poll 内层，一次 poll 就走到 `pending()` 挂住了，
    // 也就是说 `send` 已经发生，而这时一个订阅者都还没有。
    let parked = (sleeper.as_sleep())(1.0);
    tokio::time::timeout(std::time::Duration::from_millis(50), parked)
        .await
        .expect_err("第 limit 次 sleep 的本分就是永不返回");
    // 防空转：闭包确实跑进了挂住那条分支，不是压根没被 poll 到。
    assert_eq!(sleeper.calls(), vec![1.0], "第 1 次 sleep 没被记下来");

    // 后到的等待必须立刻返回。
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        sleeper.wait_until_parked(),
    )
    .await
    .expect("挂住早于订阅时 wait_until_parked() 必须立刻返回");
}

/// 永远不返回的 sleep：路由类用例不跑 reaper，但也不该真等 60 秒。
pub fn never_sleep() -> SleepFn {
    Arc::new(|_| Box::pin(std::future::pending()))
}

// ---- SessionStore --------------------------------------------------------

#[derive(Default)]
struct StoreData {
    sessions: HashMap<String, Session>,
    turns: HashMap<String, BTreeMap<u64, Turn>>,
    tasks: HashMap<String, Task>,
    task_counters: HashMap<String, u64>,
    seen: HashSet<String>,
}

/// 内存版 SessionStore：深拷贝语义（进出都 clone，改了返回值不影响库），可注入失败。
pub struct FakeStore {
    data: Mutex<StoreData>,
    fail: Mutex<HashMap<String, usize>>,
    pub attempts: Mutex<Vec<String>>,
}

impl FakeStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(StoreData::default()),
            fail: Mutex::new(HashMap::new()),
            attempts: Mutex::new(Vec::new()),
        })
    }

    /// 让某个方法接下来 `times` 次调用直接报错（抛在方法体之前，什么都不做）。
    pub fn fail_next(&self, method: &str, times: usize) {
        lk(&self.fail).insert(method.to_string(), times);
    }

    fn gate(&self, method: &str) -> Result<(), StoreError> {
        lk(&self.attempts).push(method.to_string());
        let mut fail = lk(&self.fail);
        if let Some(left) = fail.get_mut(method)
            && *left > 0
        {
            *left -= 1;
            return Err(StoreError::Other(format!("{method} 撞上了只读磁盘")));
        }
        Ok(())
    }

    /// 直接读库（测试断言用），不过 gate、不 yield。
    pub fn task_count(&self) -> usize {
        lk(&self.data).tasks.len()
    }
}

#[async_trait]
impl SessionStore for FakeStore {
    async fn init(&self) -> Result<(), StoreError> {
        self.gate("init")
    }

    async fn close(&self) -> Result<(), StoreError> {
        self.gate("close")
    }

    async fn get_session(&self, session_id: &str) -> Result<Option<Session>, StoreError> {
        self.gate("get_session")?;
        tokio::task::yield_now().await;
        Ok(lk(&self.data).sessions.get(session_id).cloned())
    }

    async fn find_session_by_thread(
        &self,
        chat_id: &str,
        thread_id: &str,
    ) -> Result<Option<Session>, StoreError> {
        self.gate("find_session_by_thread")?;
        tokio::task::yield_now().await;
        let data = lk(&self.data);
        let mut hits: Vec<&Session> = data
            .sessions
            .values()
            .filter(|s| {
                s.chat_id == chat_id
                    && s.anchor.thread_id.as_deref() == Some(thread_id)
                    && s.status != aite_contracts::SessionStatus::Archived
            })
            .collect();
        // ORDER BY created_at DESC, id DESC LIMIT 1
        hits.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(hits.last().map(|s| (*s).clone()))
    }

    async fn create_session(&self, s: &Session) -> Result<(), StoreError> {
        self.gate("create_session")?;
        tokio::task::yield_now().await;
        lk(&self.data).sessions.insert(s.id.clone(), s.clone());
        Ok(())
    }

    async fn update_session(&self, s: &Session) -> Result<(), StoreError> {
        self.gate("update_session")?;
        tokio::task::yield_now().await;
        lk(&self.data).sessions.insert(s.id.clone(), s.clone());
        Ok(())
    }

    async fn append_turn(&self, t: &Turn) -> Result<(), StoreError> {
        self.gate("append_turn")?;
        tokio::task::yield_now().await;
        let mut data = lk(&self.data);
        let turns = data.turns.entry(t.session_id.clone()).or_default();
        if turns.contains_key(&t.seq) {
            return Err(StoreError::DuplicateTurn {
                session_id: t.session_id.clone(),
                seq: t.seq,
            });
        }
        turns.insert(t.seq, t.clone());
        Ok(())
    }

    async fn list_turns(&self, session_id: &str, limit: u32) -> Result<Vec<Turn>, StoreError> {
        self.gate("list_turns")?;
        tokio::task::yield_now().await;
        let data = lk(&self.data);
        let Some(turns) = data.turns.get(session_id) else {
            return Ok(Vec::new());
        };
        // 最近 limit 轮，按 seq 正序返回
        let all: Vec<Turn> = turns.values().cloned().collect();
        let start = all.len().saturating_sub(limit as usize);
        Ok(all[start..].to_vec())
    }

    async fn next_turn_seq(&self, session_id: &str) -> Result<u64, StoreError> {
        self.gate("next_turn_seq")?;
        tokio::task::yield_now().await;
        let data = lk(&self.data);
        Ok(data
            .turns
            .get(session_id)
            .and_then(|t| t.keys().next_back().map(|s| s + 1))
            .unwrap_or(0))
    }

    async fn create_task(&self, t: &Task) -> Result<(), StoreError> {
        self.gate("create_task")?;
        tokio::task::yield_now().await;
        lk(&self.data).tasks.insert(t.id.clone(), t.clone());
        Ok(())
    }

    async fn update_task(&self, t: &Task) -> Result<(), StoreError> {
        self.gate("update_task")?;
        tokio::task::yield_now().await;
        lk(&self.data).tasks.insert(t.id.clone(), t.clone());
        Ok(())
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, StoreError> {
        self.gate("get_task")?;
        tokio::task::yield_now().await;
        Ok(lk(&self.data).tasks.get(task_id).cloned())
    }

    async fn list_active_tasks(&self, chat_id: &str) -> Result<Vec<Task>, StoreError> {
        self.gate("list_active_tasks")?;
        tokio::task::yield_now().await;
        let data = lk(&self.data);
        let mut hits: Vec<Task> = data
            .tasks
            .values()
            .filter(|t| {
                t.status.is_active()
                    && data
                        .sessions
                        .get(&t.session_id)
                        .is_some_and(|s| s.chat_id == chat_id)
            })
            .cloned()
            .collect();
        hits.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(hits)
    }

    async fn next_task_no(&self, tenant_id: &str) -> Result<String, StoreError> {
        self.gate("next_task_no")?;
        tokio::task::yield_now().await;
        let n = {
            let mut data = lk(&self.data);
            let slot = data.task_counters.entry(tenant_id.to_string()).or_insert(0);
            *slot += 1;
            *slot
        };
        encode_task_no(n).map_err(|e| StoreError::Other(e.to_string()))
    }

    async fn seen_event(&self, event_id: &str) -> Result<bool, StoreError> {
        self.gate("seen_event")?;
        tokio::task::yield_now().await;
        Ok(!lk(&self.data).seen.insert(event_id.to_string()))
    }

    async fn recover_orphan_tasks(&self) -> Result<Vec<Task>, StoreError> {
        self.gate("recover_orphan_tasks")?;
        tokio::task::yield_now().await;
        let mut data = lk(&self.data);
        let mut orphans: Vec<Task> = data
            .tasks
            .values()
            .filter(|t| t.status.is_active())
            .cloned()
            .collect();
        orphans.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        for t in &mut orphans {
            t.status = TaskStatus::Failed;
            t.result_summary = "进程重启前该任务仍在执行，已终止。请重新发起。".into();
            t.updated_at = Utc::now();
            data.tasks.insert(t.id.clone(), t.clone());
        }
        Ok(orphans)
    }
}

// ---- PlatformPort --------------------------------------------------------

#[derive(Default)]
struct PlatformData {
    texts: Vec<OutboundText>,
    cards: Vec<(String, Option<String>, ChecklistCard)>,
    card_updates: Vec<(String, ChecklistCard)>,
    files: Vec<OutboundFile>,
    reactions: Vec<(String, ReactionKind)>,
    started: bool,
    n: u64,
    send_text_failures: usize,
}

/// 记录所有出站调用。断言全部对着这些 list 做。
pub struct FakePlatform {
    data: Mutex<PlatformData>,
    handler: Mutex<Option<EventHandler>>,
}

impl FakePlatform {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(PlatformData::default()),
            handler: Mutex::new(None),
        })
    }

    /// 让接下来 `times` 次 `send_text` 报错（平台抽风）。
    pub fn fail_send_text(&self, times: usize) {
        lk(&self.data).send_text_failures = times;
    }

    pub fn texts(&self) -> Vec<OutboundText> {
        lk(&self.data).texts.clone()
    }
    pub fn last_text(&self) -> Option<OutboundText> {
        lk(&self.data).texts.last().cloned()
    }
    pub fn cards(&self) -> Vec<(String, Option<String>, ChecklistCard)> {
        lk(&self.data).cards.clone()
    }
    pub fn card_updates(&self) -> Vec<(String, ChecklistCard)> {
        lk(&self.data).card_updates.clone()
    }
    pub fn files(&self) -> Vec<OutboundFile> {
        lk(&self.data).files.clone()
    }
    pub fn reactions(&self) -> Vec<(String, ReactionKind)> {
        lk(&self.data).reactions.clone()
    }
    pub fn started(&self) -> bool {
        lk(&self.data).started
    }
    pub fn handler(&self) -> Option<EventHandler> {
        lk(&self.handler).clone()
    }

    fn next_id(&self, prefix: &str) -> String {
        let mut data = lk(&self.data);
        data.n += 1;
        format!("{prefix}_{}", data.n)
    }
}

#[async_trait]
impl PlatformPort for FakePlatform {
    fn capabilities(&self) -> PlatformCapabilities {
        feishu_p0()
    }

    async fn start(&self, on_event: EventHandler) -> Result<(), PlatformError> {
        lk(&self.data).started = true;
        *lk(&self.handler) = Some(on_event);
        Ok(())
    }

    async fn stop(&self) -> Result<(), PlatformError> {
        lk(&self.data).started = false;
        Ok(())
    }

    async fn send_text(&self, msg: &OutboundText) -> Result<SendResult, PlatformError> {
        {
            let mut data = lk(&self.data);
            if data.send_text_failures > 0 {
                data.send_text_failures -= 1;
                return Err(PlatformError::new("transport_error", "平台抽风", true));
            }
            data.texts.push(msg.clone());
        }
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
        lk(&self.data).cards.push((
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
        lk(&self.data)
            .card_updates
            .push((card_id.to_string(), card.clone()));
        Ok(())
    }

    async fn send_file(&self, msg: &OutboundFile) -> Result<SendResult, PlatformError> {
        lk(&self.data).files.push(msg.clone());
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
        lk(&self.data)
            .reactions
            .push((message_id.to_string(), kind));
        Ok(())
    }

    async fn read_history(
        &self,
        _chat_id: &str,
        _limit: u32,
        _thread_id: Option<&str>,
    ) -> Result<Vec<HistoryMessage>, PlatformError> {
        Ok(Vec::new())
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
        _message_id: &str,
        _file_key: &str,
    ) -> Result<Vec<u8>, PlatformError> {
        Ok(Vec::new())
    }
}

// ---- EvidenceWriter ------------------------------------------------------

/// 内存证据链，hash 用契约里的那三个函数真算 —— `verify` 才有意义。
pub struct FakeEvidence {
    dir: PathBuf,
    chains: Mutex<BTreeMap<String, Vec<EvidenceEvent>>>,
    manifests: Mutex<BTreeMap<String, Map<String, Value>>>,
}

impl FakeEvidence {
    pub fn new(dir: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.into(),
            chains: Mutex::new(BTreeMap::new()),
            manifests: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn events(&self, task_id: &str) -> Vec<EvidenceEvent> {
        lk(&self.chains).get(task_id).cloned().unwrap_or_default()
    }

    /// 有链的 task_id（Python 那边数的是 evidence 目录下的子目录）。
    pub fn task_ids(&self) -> Vec<String> {
        lk(&self.chains).keys().cloned().collect()
    }

    pub fn manifest(&self, task_id: &str) -> Option<Map<String, Value>> {
        lk(&self.manifests).get(task_id).cloned()
    }

    /// 这个任务链上的 `event_received` payload（按顺序）。
    pub fn received(&self, task_id: &str) -> Vec<Map<String, Value>> {
        self.events(task_id)
            .into_iter()
            .filter(|e| e.kind == EvidenceKind::EventReceived)
            .filter_map(|e| e.payload)
            .collect()
    }

    pub fn kinds(&self, task_id: &str) -> Vec<EvidenceKind> {
        self.events(task_id).into_iter().map(|e| e.kind).collect()
    }
}

#[async_trait]
impl EvidenceWriter for FakeEvidence {
    async fn append(
        &self,
        task_id: &str,
        kind: EvidenceKind,
        payload: Map<String, Value>,
    ) -> Result<EvidenceEvent, EvidenceError> {
        let mut chains = lk(&self.chains);
        let chain = chains.entry(task_id.to_string()).or_default();
        let seq = chain.len() as u64;
        let prev_hash = chain
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| GENESIS.to_string());
        let payload_hash = payload_hash_of(&payload);
        let hash = chain_hash(&prev_hash, &payload_hash);
        let event = EvidenceEvent {
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
        chain.push(event.clone());
        Ok(event)
    }

    async fn finalize(
        &self,
        task_id: &str,
        manifest_extra: Map<String, Value>,
    ) -> Result<String, EvidenceError> {
        let root_hash = lk(&self.chains)
            .get(task_id)
            .and_then(|c| c.last().map(|e| e.hash.clone()))
            .unwrap_or_else(|| GENESIS.to_string());
        let mut manifest = manifest_extra;
        manifest.insert("task_id".into(), Value::String(task_id.to_string()));
        manifest.insert("root_hash".into(), Value::String(root_hash.clone()));
        manifest.insert(
            "contract_version".into(),
            Value::String(aite_contracts::CONTRACT_VERSION.to_string()),
        );
        manifest.insert(
            "event_count".into(),
            Value::from(lk(&self.chains).get(task_id).map_or(0, Vec::len)),
        );
        lk(&self.manifests).insert(task_id.to_string(), manifest);
        Ok(root_hash)
    }

    fn verify(&self, task_id: &str) -> bool {
        let chains = lk(&self.chains);
        let Some(chain) = chains.get(task_id) else {
            return false;
        };
        let mut prev = GENESIS.to_string();
        for (i, e) in chain.iter().enumerate() {
            let Some(payload) = e.payload.as_ref() else {
                return false;
            };
            if e.task_id != task_id || e.seq != i as u64 || e.prev_hash != prev {
                return false;
            }
            let payload_hash = payload_hash_of(payload);
            if payload_hash != e.payload_hash || chain_hash(&prev, &payload_hash) != e.hash {
                return false;
            }
            prev = e.hash.clone();
        }
        true
    }

    fn task_dir(&self, task_id: &str) -> PathBuf {
        self.dir.join(task_id)
    }
}

// ---- SandboxPort ---------------------------------------------------------

#[derive(Default)]
struct SandboxData {
    acquired: Vec<String>,
    released: Vec<String>,
    reap_calls: Vec<u32>,
    reap_returns: Vec<String>,
    reap_fails: bool,
}

pub struct FakeSandbox {
    data: Mutex<SandboxData>,
}

impl FakeSandbox {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(SandboxData::default()),
        })
    }
    pub fn set_reap_returns(&self, ids: &[&str]) {
        lk(&self.data).reap_returns = ids.iter().map(|s| (*s).to_string()).collect();
    }
    pub fn set_reap_fails(&self, fails: bool) {
        lk(&self.data).reap_fails = fails;
    }
    pub fn released(&self) -> Vec<String> {
        lk(&self.data).released.clone()
    }
    pub fn acquired(&self) -> Vec<String> {
        lk(&self.data).acquired.clone()
    }
    pub fn reap_calls(&self) -> Vec<u32> {
        lk(&self.data).reap_calls.clone()
    }
}

#[async_trait]
impl SandboxPort for FakeSandbox {
    async fn acquire(&self, task_id: &str, _spec: &SandboxSpec) -> Result<String, SandboxError> {
        let sid = format!("sb_{task_id}");
        lk(&self.data).acquired.push(sid.clone());
        Ok(sid)
    }

    async fn exec(
        &self,
        _sandbox_id: &str,
        _req: &aite_contracts::ExecRequest,
    ) -> Result<aite_contracts::ExecResult, SandboxError> {
        Err(SandboxError::new(
            aite_contracts::SandboxErrorKind::Internal,
            "控制面用例不碰执行面",
        ))
    }

    async fn put_file(
        &self,
        _sandbox_id: &str,
        _path: &str,
        _data: &[u8],
    ) -> Result<(), SandboxError> {
        Ok(())
    }

    async fn get_file(&self, _sandbox_id: &str, path: &str) -> Result<Vec<u8>, SandboxError> {
        Err(SandboxError::new(
            aite_contracts::SandboxErrorKind::FileNotFound,
            format!("file_not_found: {path}"),
        ))
    }

    async fn list_files(&self, _sandbox_id: &str) -> Result<Vec<String>, SandboxError> {
        Ok(Vec::new())
    }

    async fn touch(&self, _sandbox_id: &str) -> Result<(), SandboxError> {
        Ok(())
    }

    async fn release(&self, sandbox_id: &str) -> Result<(), SandboxError> {
        lk(&self.data).released.push(sandbox_id.to_string());
        Ok(())
    }

    async fn reap_idle(&self, idle_sec: u32) -> Result<Vec<String>, SandboxError> {
        let mut data = lk(&self.data);
        data.reap_calls.push(idle_sec);
        if data.reap_fails {
            return Err(SandboxError::new(
                aite_contracts::SandboxErrorKind::Unavailable,
                "docker 不在",
            ));
        }
        Ok(data.reap_returns.clone())
    }
}

// ---- ToolGateway ---------------------------------------------------------

pub struct FakeGateway {
    released: Mutex<Vec<String>>,
    registered: Mutex<Vec<(String, String)>>,
}

impl FakeGateway {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            released: Mutex::new(Vec::new()),
            registered: Mutex::new(Vec::new()),
        })
    }
    pub fn released(&self) -> Vec<String> {
        lk(&self.released).clone()
    }
    pub fn registered(&self) -> Vec<(String, String)> {
        lk(&self.registered).clone()
    }
}

#[async_trait]
impl ToolGateway for FakeGateway {
    fn catalog(&self, _ctx: &ToolContext) -> Vec<ToolSpec> {
        Vec::new()
    }

    async fn call(&self, _ctx: &ToolContext, req: &ToolCallRequest) -> ToolResult {
        ToolResult {
            call_id: req.call_id.clone(),
            name: req.name.clone(),
            ok: true,
            content: format!("{} ok", req.name),
            data: None,
            error: None,
            duration_ms: 0,
            artifacts: Vec::new(),
        }
    }

    fn register_task(&self, task_id: &str, session_token: &str) {
        lk(&self.registered).push((task_id.to_string(), session_token.to_string()));
    }

    fn unregister_task(&self, task_id: &str) {
        lk(&self.registered).retain(|(id, _)| id != task_id);
    }

    async fn sandbox_id_of(&self, _task_id: &str) -> Option<String> {
        None
    }

    async fn release_task(&self, task_id: &str) {
        lk(&self.released).push(task_id.to_string());
    }
}

// ---- TaskWorker ----------------------------------------------------------

/// 脚本化 worker 的一步。
#[derive(Debug, Clone)]
pub enum WorkerAction {
    /// 直接交付：`send_text(reply)` → `status=delivered`
    Deliver(String),
    /// 先 `drain_steer()` 记一笔，把取到的追问拼进正文，再交付
    DrainThenDeliver(String),
    /// 先查 `is_cancelled()`：真 → `status=cancelled` 不回帖；假 → 按 Deliver 走
    CancelAwareDeliver(String),
    /// 一直等到 `is_cancelled()` 变真，再把状态落成 cancelled。
    /// 用来把任务**按在 worker 手上**，好去验 `cancel_task` 的「在跑」分支。
    WaitForCancel,
}

/// 按脚本出牌的 TaskWorker（真 AgentWorker 是 R5 的 crate）。
///
/// 只复刻控制面用得着的那一小片：改 task 状态、落库、往群里回一句、
/// 以及把 `RunHooks` 的两个回调真的调一次。交付时平台抽风的话，照 `AgentWorker.run()`
/// 的外层 `except` 收敛成 `failed` + 再回一句「执行出错」。
pub struct ScriptedWorker {
    store: Arc<FakeStore>,
    platform: Arc<FakePlatform>,
    script: Mutex<VecDeque<WorkerAction>>,
    calls: Mutex<Vec<String>>,
    drained: Mutex<Vec<Vec<String>>>,
    cancel_checks: Mutex<Vec<bool>>,
    in_flight: Mutex<Vec<(Task, Session)>>,
}

impl ScriptedWorker {
    pub fn new(
        store: Arc<FakeStore>,
        platform: Arc<FakePlatform>,
        script: Vec<WorkerAction>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            platform,
            script: Mutex::new(script.into()),
            calls: Mutex::new(Vec::new()),
            drained: Mutex::new(Vec::new()),
            cancel_checks: Mutex::new(Vec::new()),
            in_flight: Mutex::new(Vec::new()),
        })
    }

    /// 空脚本：被调用就说明测试的前提破了（对齐 Python 的 `ScriptedModel([])`）。
    pub fn idle(store: Arc<FakeStore>, platform: Arc<FakePlatform>) -> Arc<Self> {
        Self::new(store, platform, Vec::new())
    }

    pub fn calls(&self) -> Vec<String> {
        lk(&self.calls).clone()
    }
    pub fn drained(&self) -> Vec<Vec<String>> {
        lk(&self.drained).clone()
    }
    pub fn cancel_checks(&self) -> Vec<bool> {
        lk(&self.cancel_checks).clone()
    }

    async fn deliver(&self, mut task: Task, session: &Session, reply: String) -> Task {
        let thread_root = session
            .anchor
            .thread_id
            .clone()
            .unwrap_or_else(|| session.anchor.message_id.clone());
        let sent = self
            .platform
            .send_text(&OutboundText {
                chat_id: session.chat_id.clone(),
                text: reply.clone(),
                reply_to: Some(thread_root.clone()),
                in_thread: true,
            })
            .await;
        match sent {
            Ok(_) => {
                task.status = TaskStatus::Delivered;
                task.result_summary = reply;
            }
            Err(e) => {
                // AgentWorker.run() 的外层 except → _fail(f"任务 {task_no} 执行出错：{exc}")
                let text = format!("任务 {} 执行出错：{}", task.task_no, e);
                task.status = TaskStatus::Failed;
                task.result_summary = text.clone();
                let _ = self
                    .platform
                    .send_text(&OutboundText {
                        chat_id: session.chat_id.clone(),
                        text,
                        reply_to: Some(thread_root),
                        in_thread: true,
                    })
                    .await;
            }
        }
        task.updated_at = Utc::now();
        let _ = self.store.update_task(&task).await;
        task
    }
}

#[async_trait]
impl TaskWorker for ScriptedWorker {
    async fn run(
        &self,
        task: Task,
        session: Session,
        _initiator: Option<String>,
        hooks: RunHooks,
    ) -> Task {
        lk(&self.calls).push(task.id.clone());
        lk(&self.in_flight).push((task.clone(), session.clone()));
        let action = lk(&self.script).pop_front();
        let out = match action {
            Some(WorkerAction::Deliver(reply)) => self.deliver(task, &session, reply).await,
            Some(WorkerAction::DrainThenDeliver(reply)) => {
                let steer = (hooks.drain_steer)();
                lk(&self.drained).push(steer.clone());
                let reply = if steer.is_empty() {
                    reply
                } else {
                    format!("{reply}（合并了：{}）", steer.join("；"))
                };
                self.deliver(task, &session, reply).await
            }
            Some(WorkerAction::CancelAwareDeliver(reply)) => {
                let cancelled = (hooks.is_cancelled)();
                lk(&self.cancel_checks).push(cancelled);
                if cancelled {
                    let mut task = task;
                    task.status = TaskStatus::Cancelled;
                    task.updated_at = Utc::now();
                    let _ = self.store.update_task(&task).await;
                    task
                } else {
                    self.deliver(task, &session, reply).await
                }
            }
            Some(WorkerAction::WaitForCancel) => {
                loop {
                    let cancelled = (hooks.is_cancelled)();
                    lk(&self.cancel_checks).push(cancelled);
                    if cancelled {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                let mut task = task;
                task.status = TaskStatus::Cancelled;
                task.updated_at = Utc::now();
                let _ = self.store.update_task(&task).await;
                task
            }
            None => {
                // 脚本用完还被调 = 这条用例的前提破了。不 panic，落一个能断言的终态。
                let mut task = task;
                task.status = TaskStatus::Failed;
                task.result_summary = format!("任务 {} 执行出错：脚本已用完", task.task_no);
                task.updated_at = Utc::now();
                let _ = self.store.update_task(&task).await;
                task
            }
        };
        lk(&self.in_flight).retain(|(t, _)| t.id != out.id);
        out
    }

    fn in_flight(&self) -> Vec<(Task, Session)> {
        lk(&self.in_flight).clone()
    }
}

// ---- 装配 ----------------------------------------------------------------

/// 对应 `control_fakes.make_config(tmp_path)`。
///
/// 存储路径只是**字符串**：`FakeStore` / `FakeEvidence` 都在内存里，没有文件被创建，
/// 但 R3 的 evidence 按钮回帖要把 `evidence_dir` 拼进去，所以它得有个稳定取值。
pub fn make_config() -> AiteConfig {
    AiteConfig {
        tenant_id: "default".into(),
        platform: aite_contracts::PlatformChoice::Fake,
        storage: aite_contracts::StorageConfig {
            sqlite_path: "data/test/aite.db".into(),
            evidence_dir: "data/test/evidence".into(),
            artifacts_dir: "data/test/artifacts".into(),
        },
        model: aite_contracts::ModelConfig {
            provider: aite_contracts::ModelProvider::Scripted,
            model: "scripted-p0".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 一整套替身 + 配置（对应 `tests/control/conftest.py` 的那批 fixture）。
pub struct Harness {
    pub store: Arc<FakeStore>,
    pub platform: Arc<FakePlatform>,
    pub evidence: Arc<FakeEvidence>,
    pub sandbox: Arc<FakeSandbox>,
    pub gateway: Arc<FakeGateway>,
    pub config: AiteConfig,
}

impl Harness {
    pub fn new() -> Self {
        let config = make_config();
        Self {
            store: FakeStore::new(),
            platform: FakePlatform::new(),
            evidence: FakeEvidence::new(config.storage.evidence_dir.clone()),
            sandbox: FakeSandbox::new(),
            gateway: FakeGateway::new(),
            config,
        }
    }

    pub fn plane_builder(&self) -> PlaneBuilder {
        PlaneBuilder {
            store: Arc::clone(&self.store) as Arc<dyn SessionStore>,
            platform: Arc::clone(&self.platform) as Arc<dyn PlatformPort>,
            evidence: Arc::clone(&self.evidence) as Arc<dyn EvidenceWriter>,
            sandbox: Some(Arc::clone(&self.sandbox) as Arc<dyn SandboxPort>),
            gateway: Some(Arc::clone(&self.gateway) as Arc<dyn ToolGateway>),
            config: self.config.clone(),
            worker: None,
            sleep: never_sleep(),
            clock: None,
            model_name: "scripted".into(),
        }
    }

    /// 默认装配：不给 worker —— 路由用例不需要 worker 真跑（同 Python 的 `make_plane()`）。
    pub fn plane(&self) -> Arc<InProcessControlPlane> {
        self.plane_builder().build()
    }
}

pub struct PlaneBuilder {
    store: Arc<dyn SessionStore>,
    platform: Arc<dyn PlatformPort>,
    evidence: Arc<dyn EvidenceWriter>,
    sandbox: Option<Arc<dyn SandboxPort>>,
    gateway: Option<Arc<dyn ToolGateway>>,
    config: AiteConfig,
    worker: Option<Arc<dyn TaskWorker>>,
    sleep: SleepFn,
    clock: Option<WallClock>,
    model_name: String,
}

impl PlaneBuilder {
    pub fn worker(mut self, worker: Arc<dyn TaskWorker>) -> Self {
        self.worker = Some(worker);
        self
    }
    pub fn store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.store = store;
        self
    }
    pub fn platform(mut self, platform: Arc<dyn PlatformPort>) -> Self {
        self.platform = platform;
        self
    }
    pub fn sandbox(mut self, sandbox: Option<Arc<dyn SandboxPort>>) -> Self {
        self.sandbox = sandbox;
        self
    }
    pub fn gateway(mut self, gateway: Option<Arc<dyn ToolGateway>>) -> Self {
        self.gateway = gateway;
        self
    }
    pub fn sleep(mut self, sleep: SleepFn) -> Self {
        self.sleep = sleep;
        self
    }
    pub fn clock(mut self, clock: WallClock) -> Self {
        self.clock = Some(clock);
        self
    }
    pub fn model_name(mut self, name: &str) -> Self {
        self.model_name = name.into();
        self
    }

    pub fn build(self) -> Arc<InProcessControlPlane> {
        let mut plane = InProcessControlPlane::new(ControlDeps {
            store: self.store,
            platform: self.platform,
            evidence: self.evidence,
            config: self.config,
            worker: self.worker,
            gateway: self.gateway,
            sandbox: self.sandbox,
            model_name: self.model_name,
        })
        .with_sleep(self.sleep);
        if let Some(clock) = self.clock {
            plane = plane.with_clock(clock);
        }
        Arc::new(plane)
    }
}

// ---- 断言小工具 ----------------------------------------------------------

/// 跑 `run_forever`，并在返回的守卫被 drop 时停掉它（对齐 Python 的 `_running` 上下文管理器）。
pub struct RunningPlane {
    handle: tokio::task::JoinHandle<()>,
}

impl RunningPlane {
    pub fn start(plane: Arc<InProcessControlPlane>) -> Self {
        use aite_contracts::ControlPlane;
        Self {
            handle: tokio::spawn(async move { plane.run_forever().await }),
        }
    }
}

impl Drop for RunningPlane {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// 两秒超时的 await —— 卡住时给出人话，而不是让整个用例挂死。
pub async fn within<F: std::future::Future>(what: &str, fut: F) -> F::Output {
    match tokio::time::timeout(std::time::Duration::from_secs(5), fut).await {
        Ok(v) => v,
        Err(_) => panic!("{what} 超时（5s）"),
    }
}

/// 库里这个群的活跃任务（按 (created_at, id) 正序）。
pub async fn active_tasks(store: &Arc<FakeStore>, chat_id: &str) -> Vec<Task> {
    store
        .list_active_tasks(chat_id)
        .await
        .expect("list_active_tasks 不该失败")
}

/// 这个会话的 transcript 正文。
pub async fn turn_texts(store: &Arc<FakeStore>, session_id: &str) -> Vec<String> {
    store
        .list_turns(session_id, 500)
        .await
        .expect("list_turns 不该失败")
        .into_iter()
        .map(|t| t.content)
        .collect()
}
