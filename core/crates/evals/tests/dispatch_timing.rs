//! 事件投递时序（冻结契约 C-T5T6-1）：`EventSpec.after` / `after_timeout_sec`。
//!
//! 移植自 `tests/e2e/test_t4_dispatch_timing.py`（6）+ `test_t5_event_timing.py`（11）。
//!
//! 这里只测**从 yaml 外面看得见的行为**，一律不碰 runner 的内部：写了 `after: running`，
//! 事件就等到任务真被领走才投；写了 `after: idle`，就等到系统静默才投；等不到就以
//! `phase="dispatch"` 停下、不许接着往下投。判据换个实现照样得成立。
//!
//! 用的 plane 是本文件自带的最小实现，刻意把「建任务」和「领任务」分在两处 —— 那正是
//! 契约要区分的两件事：store 里有一行 task ≠ worker 领走了它。
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aite_contracts::{
    ControlPlane, IngressError, NormalizedEvent, Session, SessionKind, SessionStatus, SessionStore,
    Task, TaskStatus,
};
use aite_evals::deps::Deps;
use aite_evals::runner::{PlaneFactory, RunOptions, run_scenario};
use aite_evals::scenario::{EventAfter, EventSpec, Scenario, load_suite};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Map, Value, json};

/// 每条事件到达时，store 里各任务的状态（测试就看这个判断投递时机）。
type Arrivals = Arc<Mutex<Vec<(String, Vec<String>)>>>;

const SUITE_DIR: &str = "../../../evals/p0";

/// 时序机制只动了 02 与 07。其余八个必须保持 none。
const UNTOUCHED: [&str; 8] = [
    "01_simple_qa",
    "03_checklist_progress",
    "04_csv_to_chart",
    "05_history_summary",
    "06_read_document",
    "08_step_limit",
    "09_bot_ignored",
    "10_duplicate_event",
];

/// 最小 plane：`handle_event` 只建 task（created）并入队，`run_forever` 才领走。
///
/// 领走时把任务推到 planning —— 与 worker 一进主循环干的事一样，也是「worker 真的领走了」
/// 在外面唯一看得见的痕迹。三个开关用来摆出要测的时序：
///
///   take=false     队列里的任务永远没人领（验 after: running 等不到时的姿态）
///   settle_ticks   领走后隔几个 tick 才收尾（让 after: idle 有得可等）
///   finish         收尾时把任务落到 delivered
struct SplitPlane {
    deps_store: Arc<aite_testing::FakeSessionStore>,
    take: bool,
    settle_ticks: usize,
    finish: bool,
    queue: Mutex<VecDeque<Task>>,
    running: Mutex<usize>,
    /// 每条事件到达时，store 里各任务的状态 —— 测试就看这个判断投递时机
    seen: Arrivals,
}

impl SplitPlane {
    fn new(deps: &Deps, seen: Arrivals) -> Self {
        Self {
            deps_store: deps.store.clone(),
            take: true,
            settle_ticks: 0,
            finish: true,
            queue: Mutex::new(VecDeque::new()),
            running: Mutex::new(0),
            seen,
        }
    }
}

#[async_trait]
impl ControlPlane for SplitPlane {
    async fn handle_event(&self, ev: NormalizedEvent) -> Result<(), IngressError> {
        let states: Vec<String> = self
            .deps_store
            .task_list()
            .iter()
            .map(|t| t.status.as_str().to_string())
            .collect();
        self.seen
            .lock()
            .unwrap()
            .push((ev.event_id.clone(), states));

        let now = Utc::now();
        let mut anchor = ev.anchor.clone();
        if anchor.thread_id.is_none() {
            anchor.thread_id = Some(anchor.message_id.clone());
        }
        let session = Session {
            id: format!("s-{}", ev.event_id),
            tenant_id: ev.tenant_id.clone(),
            workspace_id: ev.workspace_id.clone(),
            chat_id: ev.chat_id.clone(),
            kind: SessionKind::Task,
            anchor,
            status: SessionStatus::Active,
            created_by: ev.sender_id.clone(),
            config_snapshot: Map::new(),
            created_at: now,
            last_active_at: now,
            archived_at: None,
        };
        self.deps_store.create_session(&session).await?;
        let task = Task {
            id: format!("t-{}", ev.event_id),
            session_id: session.id.clone(),
            task_no: self.deps_store.next_task_no(&ev.tenant_id).await?,
            status: TaskStatus::Created,
            title: String::new(),
            checklist: Vec::new(),
            card_id: None,
            sandbox_id: None,
            session_token: "tok".to_string(),
            model: String::new(),
            steps: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            max_steps: 40,
            max_wall_sec: 1200,
            result_summary: String::new(),
            evidence_root_hash: None,
            created_by: ev.sender_id.clone(),
            created_at: now,
            updated_at: now,
        };
        self.deps_store.create_task(&task).await?;
        self.queue.lock().unwrap().push_back(task);
        Ok(())
    }

    async fn run_forever(&self) {
        loop {
            let item = {
                let mut q = self.queue.lock().unwrap();
                let item = q.pop_front();
                if item.is_some() {
                    *self.running.lock().unwrap() += 1;
                }
                item
            };
            match item {
                None => tokio::task::yield_now().await,
                Some(mut task) => {
                    if !self.take {
                        // 领是领了，但永远不推进
                        std::future::pending::<()>().await;
                    }
                    task.status = TaskStatus::Planning; // ← 「被 worker 领走了」
                    let _ = self.deps_store.update_task(&task).await;
                    for _ in 0..self.settle_ticks {
                        tokio::task::yield_now().await;
                    }
                    if self.finish {
                        task.status = TaskStatus::Delivered;
                        let _ = self.deps_store.update_task(&task).await;
                    }
                    let mut running = self.running.lock().unwrap();
                    *running = running.saturating_sub(1);
                }
            }
        }
    }

    async fn run_pending(&self) {}

    fn pending(&self) -> usize {
        // take=false 时任务被领走后永远挂着；把它算成「还在跑」会让 settle 永不静默，
        // 而 after: running 的失败姿态正要靠 settle 之外的判据去验，所以这里只数队列。
        self.queue.lock().unwrap().len()
    }

    async fn join(&self) {
        while !self.queue.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    }

    async fn cancel_task(
        &self,
        task: Task,
        _reply_to: Option<String>,
        _chat_id: Option<String>,
        _notify: bool,
    ) -> Task {
        task
    }

    fn counters(&self) -> Map<String, Value> {
        Map::new()
    }
}

struct Rig {
    seen: Arrivals,
    factory: PlaneFactory,
}

fn rig(take: bool, settle_ticks: usize, finish: bool) -> Rig {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    Rig {
        seen,
        factory: Arc::new(move |deps: &Deps| {
            let mut plane = SplitPlane::new(deps, sink.clone());
            plane.take = take;
            plane.settle_ticks = settle_ticks;
            plane.finish = finish;
            Ok(Arc::new(plane) as Arc<dyn ControlPlane>)
        }),
    }
}

impl Rig {
    fn options(&self) -> RunOptions {
        RunOptions {
            plane_factory: Some(self.factory.clone()),
            ..RunOptions::default()
        }
    }

    fn status_at(&self, event_id: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|(eid, _)| eid == event_id)
            .map(|(_, states)| states.clone())
            .unwrap_or_else(|| panic!("{event_id} 没被投过"))
    }

    fn arrivals(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(eid, _)| eid.clone())
            .collect()
    }
}

/// 一条事件起个任务，第二条按 `after` 决定什么时候投。
fn two_events(after: EventAfter, after_timeout_sec: f64) -> Scenario {
    let mut e2 = EventSpec::new("e2", "第二条");
    e2.after = after;
    e2.after_timeout_sec = after_timeout_sec;
    Scenario {
        events: vec![EventSpec::new("e1", "起个活"), e2],
        timeout_sec: 2.0,
        ..Scenario::named("timing")
    }
}

// --- 默认值 ------------------------------------------------------------------

/// C-T5T6-1 的默认值。改了它，现在绿着的 8 个场景会一起变行为。
#[test]
fn after_defaults_to_none_and_five_seconds() {
    let spec = EventSpec::new("e1", "");
    assert_eq!(spec.after, EventAfter::None);
    assert_eq!(spec.after_timeout_sec, 5.0);
}

/// 只认 none / idle / running；写错了要在加载场景时就炸，不是投到一半才发现。
#[test]
fn unknown_after_values_are_rejected_at_load_time() {
    let err = serde_json::from_value::<EventSpec>(json!({"event_id": "e1", "after": "soon"}));
    assert!(err.is_err(), "after=soon 应该在加载期被拒");
}

/// 硬约束：时序机制只该动 02 与 07，其余八个一条 `after` 都不许多出来。
#[test]
fn the_eight_untouched_scenarios_still_dispatch_back_to_back() {
    for sc in load_suite(Path::new(SUITE_DIR)).unwrap() {
        if !UNTOUCHED.contains(&sc.name.as_str()) {
            continue;
        }
        assert!(
            sc.events.iter().all(|e| e.after == EventAfter::None),
            "{} 被加了投递等待，8 个场景的 stats 会跟着变",
            sc.name
        );
    }
}

// --- after: none / running ---------------------------------------------------

/// 默认（none）不等：第二条到达时第一条起的任务还躺在队列里，状态是 created。
/// 这不是缺陷，是基准 —— 07_commands 当初正是把这种时序当成了「任务在跑」。
#[tokio::test]
async fn without_after_the_second_event_lands_before_anyone_takes_the_task() {
    let rig = rig(true, 0, true);
    let r = run_scenario(&two_events(EventAfter::None, 5.0), &rig.options()).await;
    assert_eq!(r.phase, "ok", "{:?}", r.reason);
    assert_eq!(rig.status_at("e2"), vec!["created".to_string()]);
}

/// 写了 after: running，第二条就要等到任务真的被领走（离开 created）才投。
#[tokio::test]
async fn after_running_waits_until_the_task_is_actually_taken() {
    let rig = rig(true, 0, false);
    let r = run_scenario(&two_events(EventAfter::Running, 5.0), &rig.options()).await;
    assert_eq!(r.phase, "ok", "{:?}", r.reason);
    assert_eq!(rig.status_at("e2"), vec!["planning".to_string()]);
}

/// 等不到就失败，不许接着投 —— 「等不到就接着投」测出来的绿是假的。
#[tokio::test]
async fn after_running_that_never_happens_fails_the_dispatch_phase() {
    let rig = rig(false, 0, true);
    let r = run_scenario(&two_events(EventAfter::Running, 0.2), &rig.options()).await;
    assert!(!r.passed);
    assert_eq!(r.phase, "dispatch");
    let reason = r.reason.unwrap();
    assert!(!reason.contains('\n'), "{reason}");
    assert!(!reason.contains("panicked"), "{reason}");
    assert!(reason.contains("e2"), "{reason}"); // 卡在哪条事件
    assert!(reason.contains("after=running"), "{reason}"); // 等的是什么
    assert!(reason.contains("created"), "{reason}"); // 为什么没等到
    assert!(reason.contains("0.2"), "{reason}"); // 等了多久
    assert_eq!(
        rig.arrivals(),
        vec!["e1".to_string()],
        "等不到却还是把 e2 投出去了"
    );
}

// --- after: idle -------------------------------------------------------------

/// 写了 after: idle，第二条要等在跑的任务收了、队列空了才投。
#[tokio::test]
async fn after_idle_waits_until_the_running_task_is_done() {
    let rig = rig(true, 5, true);
    let r = run_scenario(&two_events(EventAfter::Idle, 5.0), &rig.options()).await;
    assert_eq!(r.phase, "ok", "{:?}", r.reason);
    assert_eq!(rig.status_at("e2"), vec!["delivered".to_string()]);
}

/// idle 判的是**系统活动静默**（settle 那一套），不是「任务落到终态」。
///
/// 并轨时 `after` 的判据以契约 owner 那份为准：连续一段没有替身被调用就算静默 ——
/// 收尾判「系统不干活了」用的也是这套，同一件事不该有两个说法。
///
/// `finish=false` 把任务撂在 planning 却一个替身都不碰，于是这里等到的是「静默」、
/// e2 照投。真实系统里 worker 跑任务必然要调模型/网关，这种「活着但一动不动」的形态
/// 只在这个人造 plane 里出现，所以它钉的是判据本身，不是某个场景的期望。
#[tokio::test]
async fn after_idle_judges_by_activity_not_by_task_status() {
    let rig = rig(true, 0, false);
    let r = run_scenario(&two_events(EventAfter::Idle, 2.0), &rig.options()).await;
    assert_eq!(r.phase, "ok", "{:?}", r.reason);
    assert_eq!(rig.status_at("e2"), vec!["planning".to_string()]);
}

/// 系统一直在动 → 同样是 dispatch 失败，而不是「等不到就接着投」。
#[tokio::test]
async fn idle_that_never_settles_fails_at_dispatch() {
    /// 一刻不停地调替身 —— 用来验 `after: idle` 等不到时的失败姿态。
    struct NeverQuiet {
        store: Arc<aite_testing::FakeSessionStore>,
    }
    #[async_trait]
    impl ControlPlane for NeverQuiet {
        async fn handle_event(&self, _ev: NormalizedEvent) -> Result<(), IngressError> {
            Ok(())
        }
        async fn run_forever(&self) {
            loop {
                // 每一圈都往 CallLog 里加一笔
                let _ = self.store.get_task("没这个任务").await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        }
        async fn run_pending(&self) {}
        fn pending(&self) -> usize {
            0
        }
        async fn join(&self) {}
        async fn cancel_task(
            &self,
            task: Task,
            _reply_to: Option<String>,
            _chat_id: Option<String>,
            _notify: bool,
        ) -> Task {
            task
        }
        fn counters(&self) -> Map<String, Value> {
            Map::new()
        }
    }
    let factory: PlaneFactory = Arc::new(|deps: &Deps| {
        Ok(Arc::new(NeverQuiet {
            store: deps.store.clone(),
        }) as Arc<dyn ControlPlane>)
    });
    let r = run_scenario(
        &two_events(EventAfter::Idle, 0.2),
        &RunOptions {
            plane_factory: Some(factory),
            ..RunOptions::default()
        },
    )
    .await;
    assert!(!r.passed);
    assert_eq!(r.phase, "dispatch");
    let reason = r.reason.unwrap();
    assert!(!reason.contains('\n'), "{reason}");
    assert!(reason.contains("e2"), "{reason}");
    assert!(reason.contains("after=idle"), "{reason}");
    assert!(reason.contains("0.2"), "{reason}");
}

/// 等不到就不许把后面的事件投出去 —— 投出去测出来的绿是假的。
#[tokio::test]
async fn a_failed_wait_stops_the_dispatch_loop() {
    let mut e2 = EventSpec::new("e2", "等不到");
    e2.after = EventAfter::Running;
    e2.after_timeout_sec = 0.2;
    let sc = Scenario {
        events: vec![
            EventSpec::new("e1", "起个活"),
            e2,
            EventSpec::new("e3", "这条不该被投出去"),
        ],
        timeout_sec: 2.0,
        ..Scenario::named("timing_halts")
    };
    let rig = rig(false, 0, true);
    let r = run_scenario(&sc, &rig.options()).await;
    assert_eq!(r.phase, "dispatch");
    assert_eq!(rig.arrivals(), vec!["e1".to_string()]);
}

/// 02 不等就投 = 改动前的样子：第二条落在第一条还在跑的时候。
///
/// 真控制面会按 R6 把它并成 steer（那是 R4 的正确行为）；这里用 SplitPlane 只验
/// **runner 这一头**：把 `after` 从 idle 改成 none，e2 到达时第一个任务确实还没收。
#[tokio::test]
async fn scenario_02_without_the_wait_lands_while_the_first_task_is_still_queued() {
    let mut sc = load_suite(Path::new(SUITE_DIR))
        .unwrap()
        .into_iter()
        .find(|s| s.name == "02_thread_followup")
        .unwrap();
    assert_eq!(
        sc.events[1].after,
        EventAfter::Idle,
        "02 的 e2 应该是 after: idle"
    );
    sc.events[1].after = EventAfter::None;
    let rig = rig(true, 5, true);
    run_scenario(&sc, &rig.options()).await;
    assert_eq!(rig.status_at("e2"), vec!["created".to_string()]);
}

/// 02 原样（after: idle）：e2 到达时第一个任务已经收了。
#[tokio::test]
async fn scenario_02_with_after_idle_lands_after_the_first_task_is_done() {
    let sc = load_suite(Path::new(SUITE_DIR))
        .unwrap()
        .into_iter()
        .find(|s| s.name == "02_thread_followup")
        .unwrap();
    let rig = rig(true, 5, true);
    run_scenario(&sc, &rig.options()).await;
    assert_eq!(rig.status_at("e2"), vec!["delivered".to_string()]);
}

/// `--timeout-scale` 同时乘 `timeout_sec` 与每条事件的 `after_timeout_sec`，
/// 且**不改原场景对象**。
#[test]
fn timeout_scale_multiplies_both_waits() {
    let sc = two_events(EventAfter::Idle, 5.0);
    let scaled = sc.scaled(12.0);
    assert_eq!(scaled.timeout_sec, 24.0);
    assert_eq!(
        scaled
            .events
            .iter()
            .map(|e| e.after_timeout_sec)
            .collect::<Vec<_>>(),
        vec![60.0, 60.0]
    );
    assert_eq!(sc.timeout_sec, 2.0, "原场景对象不该被改");
}

/// `after: none` 的场景 stats 逐字稳定（DemoPlane 下的 01 基线）。
#[tokio::test]
async fn after_none_keeps_a_green_scenario_stable() {
    let sc = load_suite(Path::new(SUITE_DIR))
        .unwrap()
        .into_iter()
        .find(|s| s.name == "01_simple_qa")
        .unwrap();
    let r = run_scenario(
        &sc,
        &RunOptions {
            plane_factory: Some(aite_evals::DemoPlane::factory()),
            ..RunOptions::default()
        },
    )
    .await;
    assert!(r.passed, "01 没过：{:?} / {:?}", r.reason, r.failures);
    assert_eq!(r.stats["model_calls"], json!(1));
    assert_eq!(r.stats["gateway_calls"], json!(0));
    assert_eq!(r.stats["sandbox_calls"], json!(0));
    assert_eq!(r.stats["sessions"], json!(1));
    assert_eq!(r.stats["tasks"], json!(1));
}
