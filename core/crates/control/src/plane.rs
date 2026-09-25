//! `InProcessControlPlane` —— 对应 `aite/control/plane.py`（712 行）。
//!
//! `handle_event` 是 §3.5 路由的唯一入口，R1–R8 **按编号顺序求值、命中即停**。
//! 读这个文件时对着 §3.5 那张表看，[`InProcessControlPlane::route`] 里每个分支都标了规则号。
//!
//! CC2 起这个文件只留**状态与装配**（`ControlDeps`、`Shared`、`InProcessControlPlane` 本体、
//! `with_*` builder）和冻结的 `ControlPlane` trait 实现；路由、会话、派发、命令各自搬进了
//! 兄弟模块（`routing/`、`sessions.rs`、`dispatch.rs`、`reaper.rs`、`evidence_log.rs`、
//! `commands/`）。搬家是零行为变化的：每一段都是原样挪过去的，只把私有方法放宽成 `pub(crate)`。
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use aite_contracts::{
    AiteConfig, ControlPlane, EvidenceWriter, IngressError, NormalizedEvent, PlatformPort,
    SandboxPort, SessionStore, StoreError, Task, TaskWorker, ToolGateway,
};

use crate::lock;
use crate::queue::DispatchQueue;
use crate::reaper::REAPER_INTERVAL_SEC;

/// 墙钟。Python 用的是 `datetime.now(UTC)`，这里显式化成可注入，测试才能钉住时间。
pub type WallClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;
/// 可注入的 sleep（reaper 的节奏）。Python 用 `asyncio.sleep`。
pub type SleepFn = Arc<dyn Fn(f64) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

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
pub(crate) struct Shared {
    pub(crate) queue: DispatchQueue,
    /// 待合并的追问文本，只在内存（进程一换就没了，见 test_steer_routing ①）
    pub(crate) steer: Mutex<HashMap<String, Vec<String>>>,
    /// 已取消的 task_id；worker 每步开头查
    pub(crate) cancelled: Mutex<HashSet<String>>,
    /// 正在 worker 手上
    pub(crate) running: Mutex<HashSet<String>>,
    /// 本进程接手过、还没收尾的（排队中 + 在跑）；孤儿判定靠它
    pub(crate) owned: Mutex<HashSet<String>>,
    pub(crate) counters: Mutex<BTreeMap<String, i64>>,
}

impl Shared {
    pub(crate) fn bump(&self, key: &str) {
        *lock(&self.counters).entry(key.to_string()).or_insert(0) += 1;
    }

    pub(crate) fn counter(&self, key: &str) -> i64 {
        lock(&self.counters).get(key).copied().unwrap_or(0)
    }

    pub(crate) fn drain_steer(&self, task_id: &str) -> Vec<String> {
        lock(&self.steer).remove(task_id).unwrap_or_default()
    }
}

/// ControlPlane（T2）。进程内队列 + 单 worker，P0 只跑单副本。
pub struct InProcessControlPlane {
    pub(crate) store: Arc<dyn SessionStore>,
    pub(crate) platform: Arc<dyn PlatformPort>,
    pub(crate) evidence: Arc<dyn EvidenceWriter>,
    pub(crate) config: AiteConfig,
    pub(crate) worker: Option<Arc<dyn TaskWorker>>,
    pub(crate) gateway: Option<Arc<dyn ToolGateway>>,
    pub(crate) sandbox: Option<Arc<dyn SandboxPort>>,
    pub(crate) model_name: String,
    pub(crate) now: WallClock,
    pub(crate) sleep: SleepFn,
    pub(crate) reaper_interval_sec: f64,
    pub(crate) shared: Arc<Shared>,
    /// 分配 turn.seq 的临界区。为什么要有它见 [`InProcessControlPlane::append_turn`]。
    pub(crate) turn_seq_lock: tokio::sync::Mutex<()>,
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

    pub(crate) fn now(&self) -> DateTime<Utc> {
        (self.now)()
    }
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
                // 日志由 Ingress 打，这里不打第二遍 —— BB1 逐条核过这句话，它是真的：
                // `ingress.rs` 的 `on_event` 打 ERROR `ingress.handle_failed`，
                // 带 `event` / `kind` / `error` 三个字段，比这里能凑出来的还全
                // （§7 那张表上 core 那条 `ingress.handle_failed` 写的就是它）。
                //
                // **要查被丢的事件，去 `aite.ingress` 这个 target 上 grep
                // `ingress.handle_failed`。** 写下这个指路是因为那份日志在另一个模块里，
                // 而 `dropped_note()` 给用户的那句话也正是这么说的 —— 两处别再各说各的。
                //
                // ⚠️ `edge-client/src/ingress.rs` 里那条同名的 WARN（只有 `event_id` /
                // `error`，没有 `kind`）**在当前接线下到不了**：`app.rs` 交给
                // `platform.start()` 的是 `control::Ingress::handler()`，它无条件返回
                // `Ok(())`，gRPC 那一层的 `Err` 分支永远不进。详见 BB1 回执的记账。
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
        self.cancel_task_inner(task, reply_to, chat_id, notify)
            .await
    }

    fn counters(&self) -> Map<String, Value> {
        lock(&self.shared.counters)
            .iter()
            .map(|(k, v)| (k.clone(), Value::from(*v)))
            .collect()
    }
}
