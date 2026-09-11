//! 跑场景：造替身 → 接上 ControlPlane → 投事件 → 等静默 → 跑断言
//! （对应旧 `aite/evals/runner.py` + `wiring.py` 的驱动部分）。
//!
//! 铁律：**每个场景必须报出失败原因而不是 panic**。所以这一层把所有失败都收干净，
//! 转成 `ScenarioResult.reason` 的一行人话，并标出断在哪个阶段：
//!
//! ```text
//! wiring    接不上被测系统（RΩ 还没接线时就停在这里）
//! dispatch  handle_event 失败了；或者 EventSpec.after 要等的东西没等到
//! drive     run_forever 起不来 / 中途炸了 / 超时
//! assert    跑到了，但断言没过
//! ok        全过
//! ```
//!
//! 投递不是零间隔连着投：每条事件按 `EventSpec.after`（契约 C-T5T6-1）先等系统消化
//! 上一条。默认 `after="none"` 就是原来那样紧接着投。
use std::sync::Arc;
use std::time::Duration;

use aite_contracts::{
    CONTRACT_VERSION, ControlPlane, EventHandler, PlatformPort, SessionStore, TaskStatus,
};
use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::checks::run_checks;
use crate::deps::{Deps, DepsOptions, PhaseError, brief, build_deps, busy};
use crate::protocol_probe::analyze;
use crate::scenario::{EventAfter, Scenario};

/// 造 `ControlPlane` 的工厂。RΩ 组装时把真 plane 的工厂传进来；`None` 时每个场景以
/// `phase="wiring"` 收场并说清「ControlPlane 未接线（RΩ）」。
pub type PlaneFactory = Arc<dyn Fn(&Deps) -> Result<Arc<dyn ControlPlane>, String> + Send + Sync>;

/// 没接线时的人话原因。
pub const NOT_WIRED: &str = "ControlPlane 未接线（RΩ）：aite-evals 并行期间拿不到真 ControlPlane，\
     由 RΩ 在 app 里通过 PlaneFactory 注入";

const IDLE_MS: u64 = 150;
const POLL_MS: u64 = 5;

#[derive(Debug, Clone, Default)]
pub struct ScenarioResult {
    pub name: String,
    pub passed: bool,
    pub phase: String,
    pub reason: Option<String>,
    pub failures: Vec<String>,
    pub duration_ms: u64,
    pub stats: Map<String, Value>,
    pub settled_by: String,
    /// 协议出牌报告。只有开了 `--protocol-report` 才非空 —— 默认 JSON 摘要一个字段都不多。
    pub protocol: Map<String, Value>,
}

impl ScenarioResult {
    pub fn to_json(&self) -> Value {
        let mut row = Map::new();
        row.insert("name".to_string(), json!(self.name));
        row.insert("passed".to_string(), json!(self.passed));
        row.insert("phase".to_string(), json!(self.phase));
        row.insert("duration_ms".to_string(), json!(self.duration_ms));
        if let Some(reason) = &self.reason
            && !reason.is_empty()
        {
            row.insert("reason".to_string(), json!(reason));
        }
        if !self.failures.is_empty() {
            row.insert("failures".to_string(), json!(self.failures));
        }
        if !self.settled_by.is_empty() {
            row.insert("settled_by".to_string(), json!(self.settled_by));
        }
        if !self.stats.is_empty() {
            row.insert("stats".to_string(), Value::Object(self.stats.clone()));
        }
        if !self.protocol.is_empty() {
            row.insert("protocol".to_string(), Value::Object(self.protocol.clone()));
        }
        Value::Object(row)
    }
}

#[derive(Debug, Clone)]
pub struct SuiteResult {
    pub suite: String,
    pub platform: String,
    pub model: String,
    pub results: Vec<ScenarioResult>,
}

impl SuiteResult {
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.passed).count()
    }

    pub fn total(&self) -> usize {
        self.results.len()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "suite": self.suite,
            "platform": self.platform,
            "model": self.model,
            "contract_version": CONTRACT_VERSION,
            "total": self.total(),
            "passed": self.passed(),
            "failed": self.total() - self.passed(),
            "scenarios": self.results.iter().map(ScenarioResult::to_json).collect::<Vec<_>>(),
        })
    }
}

/// 跑一个场景要给的东西。
#[derive(Clone, Default)]
pub struct RunOptions {
    pub deps: DepsOptions,
    pub plane_factory: Option<PlaneFactory>,
    pub collect_protocol: bool,
}

// --------------------------------------------------------------------------
// 驱动
// --------------------------------------------------------------------------

/// 把 `run_forever` 挂到后台。它立刻炸掉的话，这里就把原因报出来。
async fn start_loop(plane: Arc<dyn ControlPlane>) -> Result<JoinHandle<()>, PhaseError> {
    let handle = tokio::spawn(async move { plane.run_forever().await });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    if handle.is_finished() {
        // 已经收场了：正常返回（run_forever 不是无限循环）没关系，panic 才算「起不来」。
        return match handle.await {
            Err(e) if e.is_panic() => Err(PhaseError::new(
                "drive",
                format!("run_forever() 起不来：{}", panic_reason(&e)),
            )),
            // 正常返回：给回一个已经收场的空句柄，后面的 stop_loop 照常调
            _ => Ok(tokio::spawn(async {})),
        };
    }
    Ok(handle)
}

fn panic_reason(err: &tokio::task::JoinError) -> String {
    if err.is_cancelled() {
        return "任务被取消".to_string();
    }
    "run_forever 内部 panic".to_string()
}

async fn stop_loop(handle: JoinHandle<()>) -> Option<String> {
    if handle.is_finished() {
        return match handle.await {
            Err(e) if e.is_panic() => Some(panic_reason(&e)),
            _ => None,
        };
    }
    handle.abort();
    match handle.await {
        Err(e) if e.is_panic() => Some(panic_reason(&e)),
        _ => None,
    }
}

/// 等系统把手上的活干完。返回用了哪种方式（drain / quiesce / timeout）。
///
/// 先用契约给的 `join()` 把排队的活跑完（它是加速器，不是判据），再按「所有替身的记账
/// 都不再变 且 不 busy 且 队列空」持续 `idle_ms` 判静默。Python 版靠 `getattr` 找
/// `drain/run_until_idle/...`，Rust 里这两件事都在 `ControlPlane` trait 上。
pub async fn settle(plane: &Arc<dyn ControlPlane>, deps: &Deps, timeout_sec: f64) -> String {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout_sec.max(0.0));
    // join() 是加速器不是判据：它先把排队的活跑完，最终还是靠下面的静默判据收场。
    let _ = tokio::time::timeout_at(deadline, plane.join()).await;

    let mut last = deps.activity();
    let mut quiet_since = Instant::now();
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
        let now_activity = deps.activity();
        if now_activity != last || busy(deps) || plane.pending() > 0 {
            last = now_activity;
            quiet_since = Instant::now();
            continue;
        }
        if quiet_since.elapsed() >= Duration::from_millis(IDLE_MS) {
            return "quiesce".to_string();
        }
    }
    "timeout".to_string()
}

/// 等系统静默：在跑的任务都收了、待处理队列空了。
///
/// 判据直接复用 `settle()` —— 场景收尾用哪套标准判「系统不干活了」，投事件前就用哪套，
/// 免得同一件事在两处有两个说法。
async fn wait_idle(
    plane: &Arc<dyn ControlPlane>,
    deps: &Deps,
    label: &str,
    timeout_sec: f64,
) -> Result<(), PhaseError> {
    let settled_by = settle(plane, deps, timeout_sec).await;
    if settled_by == "timeout" {
        return Err(PhaseError::new(
            "dispatch",
            format!(
                "投 {label} 前等系统静默（after=idle）：{timeout_sec}s 内替身一直在被调用，\
                 任务没收完或队列没排空"
            ),
        ));
    }
    Ok(())
}

/// 等最近建的那个任务真的被 worker 领走、开始跑了。
///
/// 判据是「任务离开了 `created`」：状态机里只有 worker 会把任务推出 created。所以
/// **「store 里有一行 task」不算数** —— 任务建好还躺在队列里时它仍然是 created，
/// 07 栽的就是这个区别。
///
/// 「最近建的那个」在正常时序下就是上一条事件起的那个；上一条没起任务（比如 `!status`）
/// 时它落回更早那个仍在跑的任务，也正是场景想等的那个。
async fn wait_running(deps: &Deps, label: &str, timeout_sec: f64) -> Result<(), PhaseError> {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout_sec.max(0.0));
    loop {
        // 直接读字段、不走 store 的方法 —— 别让「等待」这个动作本身给 activity() 添活动
        if let Some(task) = deps.store.newest_task()
            && task.status != TaskStatus::Created
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
    let why = match deps.store.newest_task() {
        Some(t) => format!("任务 {} 一直停在 created，没被 worker 领走", t.task_no),
        None => "一个任务都没建起来".to_string(),
    };
    Err(PhaseError::new(
        "dispatch",
        format!("投 {label} 前等任务开跑（after=running）：{timeout_sec}s 内{why}"),
    ))
}

/// 按 C-T5T6-1 的 `after` 等一等，然后才轮到 runner 投这条事件。
///
/// 等不到就以 `phase="dispatch"` 失败收场，绝不「等不到就接着投」—— 那样测出来的绿是假的。
async fn wait_before_dispatch(
    plane: &Arc<dyn ControlPlane>,
    deps: &Deps,
    after: EventAfter,
    label: &str,
    timeout_sec: f64,
) -> Result<(), PhaseError> {
    match after {
        EventAfter::None => Ok(()),
        EventAfter::Idle => wait_idle(plane, deps, label, timeout_sec).await,
        EventAfter::Running => wait_running(deps, label, timeout_sec).await,
    }
}

// --------------------------------------------------------------------------
// 执行
// --------------------------------------------------------------------------

/// 把 `plane.handle_event` 包成契约要的 `EventHandler`。
fn handler_of(plane: Arc<dyn ControlPlane>) -> EventHandler {
    Arc::new(move |ev| {
        let plane = plane.clone();
        Box::pin(async move { plane.handle_event(ev).await })
    })
}

async fn execute(
    sc: &Scenario,
    deps: &Deps,
    factory: Option<&PlaneFactory>,
) -> Result<String, PhaseError> {
    let Some(factory) = factory else {
        return Err(PhaseError::new("wiring", NOT_WIRED));
    };
    let plane = factory(deps).map_err(|e| PhaseError::new("wiring", e))?;

    deps.store
        .init()
        .await
        .map_err(|e| PhaseError::new("wiring", format!("接线失败：{}", brief(&e))))?;
    deps.platform
        .start(handler_of(plane.clone()))
        .await
        .map_err(|e| PhaseError::new("wiring", format!("接线失败：{}", brief(&e))))?;

    let loop_task = start_loop(plane.clone()).await?;
    let outcome = drive(sc, deps, &plane).await;
    if let Some(panic) = stop_loop(loop_task).await
        && outcome.is_ok()
    {
        return Err(PhaseError::new(
            "drive",
            format!("run_forever() 中途异常：{panic}"),
        ));
    }
    let settled_by = outcome?;
    if settled_by == "timeout" {
        return Err(PhaseError::new(
            "drive",
            format!("{}s 内没有停下来（系统一直在动）", sc.timeout_sec),
        ));
    }
    Ok(settled_by)
}

async fn drive(
    sc: &Scenario,
    deps: &Deps,
    plane: &Arc<dyn ControlPlane>,
) -> Result<String, PhaseError> {
    for (spec, ev) in sc.dispatch_plan() {
        // after="none"（默认）一步都不多走 —— 投递路径与时序机制引入前完全一致
        if spec.after != EventAfter::None {
            wait_before_dispatch(
                plane,
                deps,
                spec.after,
                &ev.event_id,
                spec.after_timeout_sec,
            )
            .await?;
        }
        deps.platform.emit(&ev).await.map_err(|e| {
            PhaseError::new(
                "dispatch",
                format!("handle_event({}) 失败了：{}", ev.event_id, brief(&e)),
            )
        })?;
    }
    Ok(settle(plane, deps, sc.timeout_sec).await)
}

/// 协议报告。装置本身出错也只算「报告没出来」，绝不改场景的成败结论。
fn protocol_of(deps: &Deps, wanted: bool) -> Map<String, Value> {
    if !wanted {
        return Map::new();
    }
    analyze(deps)
}

pub async fn run_scenario(sc: &Scenario, options: &RunOptions) -> ScenarioResult {
    let started = Instant::now();
    let deps = match build_deps(sc, &options.deps) {
        Ok(deps) => deps,
        Err(e) => {
            return ScenarioResult {
                name: sc.name.clone(),
                passed: false,
                phase: e.phase.to_string(),
                reason: Some(e.message),
                duration_ms: started.elapsed().as_millis() as u64,
                ..ScenarioResult::default()
            };
        }
    };

    let outcome = execute(sc, &deps, options.plane_factory.as_ref()).await;
    // 场景一收就把沙箱收摊。默认档是空操作，docker 档靠它别把容器留给下一个场景。
    deps.close().await;

    match outcome {
        Err(e) => ScenarioResult {
            name: sc.name.clone(),
            passed: false,
            phase: e.phase.to_string(),
            reason: Some(e.message),
            duration_ms: started.elapsed().as_millis() as u64,
            stats: deps.stats(),
            protocol: protocol_of(&deps, options.collect_protocol),
            ..ScenarioResult::default()
        },
        Ok(settled_by) => {
            let failures = run_checks(&deps, &sc.expect);
            ScenarioResult {
                name: sc.name.clone(),
                passed: failures.is_empty(),
                phase: if failures.is_empty() { "ok" } else { "assert" }.to_string(),
                reason: if failures.is_empty() {
                    None
                } else {
                    Some(format!("{} 条断言没过", failures.len()))
                },
                failures,
                duration_ms: started.elapsed().as_millis() as u64,
                stats: deps.stats(),
                settled_by,
                protocol: protocol_of(&deps, options.collect_protocol),
            }
        }
    }
}

pub async fn run_suite(
    scenarios: &[Scenario],
    suite: &str,
    platform: &str,
    model_name: &str,
    options: &RunOptions,
    model_factory: Option<&crate::deps::ModelFactory>,
) -> SuiteResult {
    let mut results = Vec::with_capacity(scenarios.len());
    for sc in scenarios {
        let mut per_scenario = options.clone();
        if let Some(factory) = model_factory {
            match factory() {
                Ok(model) => per_scenario.deps.model = Some(model),
                Err(e) => {
                    results.push(ScenarioResult {
                        name: sc.name.clone(),
                        passed: false,
                        phase: "wiring".to_string(),
                        reason: Some(format!("模型起不来：{e}")),
                        ..ScenarioResult::default()
                    });
                    continue;
                }
            }
        }
        results.push(run_scenario(sc, &per_scenario).await);
    }
    SuiteResult {
        suite: suite.to_string(),
        platform: platform.to_string(),
        model: model_name.to_string(),
        results,
    }
}
