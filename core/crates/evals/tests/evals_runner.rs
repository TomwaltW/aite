//! 评测 runner 自测（移植自 `tests/e2e/test_t4_evals_runner.py`，22 条）。
//!
//! 两件事最要紧：
//!
//! 1. **RΩ 没接线时，每个场景报的是人话原因，不是 panic** —— 这是派单的原文要求。
//! 2. **这套 harness 不是空壳**：用 `DemoPlane`（一个只覆盖 R1/R2/R6/R7 + 一步 final 的
//!    最小实现）把 01/02/09/10 真跑绿，证明「场景 yaml → 事件 → 替身 → expect 断言」
//!    整条链是通的。
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use aite_contracts::{
    ControlPlane, IngressError, NormalizedEvent, PlatformPort, ReactionKind, Task,
};
use aite_evals::cli::{Captured, Wiring, run_capture};
use aite_evals::deps::{Deps, DepsOptions};
use aite_evals::runner::{PlaneFactory, RunOptions, run_scenario, run_suite};
use aite_evals::scenario::{Scenario, load_suite};
use aite_evals::{DemoPlane, checks, demo_plane};
use async_trait::async_trait;
use serde_json::{Map, Value, json};

const SUITE_DIR: &str = "../../../evals/p0";

/// DemoPlane 覆盖得到的场景（它只实现最短路径，卡片/工具/命令那几条覆盖不到）。
const DEMO_PASSABLE: [&str; 4] = [
    "01_simple_qa",
    "02_thread_followup",
    "09_bot_ignored",
    "10_duplicate_event",
];

fn suite() -> Vec<Scenario> {
    load_suite(Path::new(SUITE_DIR)).expect("evals/p0 加载")
}

fn by_name(name: &str) -> Scenario {
    suite()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("没有场景 {name}"))
}

fn demo_options() -> RunOptions {
    RunOptions {
        plane_factory: Some(demo_plane::DemoPlane::factory()),
        ..RunOptions::default()
    }
}

fn options_with(factory: PlaneFactory) -> RunOptions {
    RunOptions {
        plane_factory: Some(factory),
        ..RunOptions::default()
    }
}

// --- RΩ 没接线时的优雅降级 ----------------------------------------------------

/// 并行期的核心要求：跑得完、每条都有原因、原因是一行人话。
#[tokio::test]
async fn every_scenario_reports_a_reason_not_a_panic() {
    let scenarios = suite();
    let result = run_suite(
        &scenarios,
        "evals/p0",
        "fake",
        "scripted",
        &RunOptions::default(),
        None,
    )
    .await;
    assert_eq!(result.total(), 10);
    for r in &result.results {
        assert!(!r.passed || r.phase == "ok");
        if !r.passed {
            let reason = r.reason.clone().unwrap_or_default();
            assert!(!reason.is_empty(), "{} 失败了却没给原因", r.name);
            assert!(!reason.contains("panicked"), "{}: {reason}", r.name);
            assert!(
                !reason.contains('\n'),
                "{} 的原因是多行的：{reason:?}",
                r.name
            );
            assert!(
                ["wiring", "dispatch", "drive", "assert", "error"].contains(&r.phase.as_str()),
                "{}: phase={}",
                r.name,
                r.phase
            );
        }
    }
}

#[tokio::test]
async fn summary_is_json_serialisable() {
    let scenarios = suite();
    let result = run_suite(
        &scenarios,
        "evals/p0",
        "fake",
        "scripted",
        &RunOptions::default(),
        None,
    )
    .await;
    let payload: Value =
        serde_json::from_str(&serde_json::to_string(&result.to_json()).unwrap()).unwrap();
    assert_eq!(payload["total"], json!(10));
    assert_eq!(payload["contract_version"], json!("p0.2"));
    assert_eq!(payload["scenarios"].as_array().unwrap().len(), 10);
    assert_eq!(
        payload["passed"].as_u64().unwrap() + payload["failed"].as_u64().unwrap(),
        10
    );
}

/// RΩ 还没接线时，原因要指得出是哪一环 —— 而不是一句「失败」。
#[tokio::test]
async fn wiring_failure_is_attributed_to_the_missing_track() {
    let r = run_scenario(&by_name("01_simple_qa"), &RunOptions::default()).await;
    assert!(!r.passed);
    assert_eq!(r.phase, "wiring");
    let reason = r.reason.unwrap();
    assert!(reason.contains("ControlPlane"), "{reason}");
    assert!(reason.contains("RΩ"), "{reason}");
}

#[tokio::test]
async fn broken_plane_factory_is_caught_not_raised() {
    let boom: PlaneFactory = Arc::new(|_: &Deps| Err("接线炸了".to_string()));
    let r = run_scenario(&by_name("01_simple_qa"), &options_with(boom)).await;
    assert!(!r.passed);
    let reason = r.reason.unwrap();
    assert!(reason.contains("接线炸了"), "{reason}");
    assert_eq!(r.phase, "wiring");
}

/// handle_event 失败 → phase=dispatch，原因里带着事件 id 与那句话。
#[tokio::test]
async fn handle_event_failure_is_reported_as_dispatch() {
    struct Angry;
    #[async_trait]
    impl ControlPlane for Angry {
        async fn handle_event(&self, _ev: NormalizedEvent) -> Result<(), IngressError> {
            Err(IngressError::Other("我不接这个事件".to_string()))
        }
        async fn run_forever(&self) {
            std::future::pending::<()>().await;
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
    let factory: PlaneFactory = Arc::new(|_: &Deps| Ok(Arc::new(Angry) as Arc<dyn ControlPlane>));
    let r = run_scenario(&by_name("01_simple_qa"), &options_with(factory)).await;
    assert_eq!(r.phase, "dispatch");
    assert!(r.reason.unwrap().contains("我不接这个事件"));
}

/// plane 里 panic 不能带走整套评测 —— 收成 phase="error" 的一行人话。
///
/// 回归：此前 `phase="error"` 是个**没有任何路径产出**的死分支（上面那条用例的
/// 白名单里却写着它），`execute` 到 `cli::run` 一路没有 catch。R4 的真 plane
/// 一个 unwrap 就能让 B8 的 10 条一起丢，且 JSON 摘要和 `passed k/n` 都出不来。
#[tokio::test]
async fn a_panicking_plane_is_caught_as_an_error_phase() {
    struct Exploding;
    #[async_trait::async_trait]
    impl ControlPlane for Exploding {
        async fn handle_event(&self, _ev: NormalizedEvent) -> Result<(), IngressError> {
            panic!("plane 里有个没兜住的 unwrap");
        }
        async fn run_forever(&self) {
            std::future::pending::<()>().await
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
    let factory: PlaneFactory =
        Arc::new(|_: &Deps| Ok(Arc::new(Exploding) as Arc<dyn ControlPlane>));
    let r = run_scenario(&by_name("01_simple_qa"), &options_with(factory)).await;
    assert_eq!(r.phase, "error", "panic 要收成 error 相，而不是打穿进程");
    let reason = r.reason.unwrap_or_default();
    assert!(reason.contains("panic"), "原因要说清是 panic：{reason}");
    assert!(
        reason.contains("没兜住的 unwrap"),
        "panic 的原话不能丢：{reason}"
    );
    assert!(!reason.contains('\n'), "原因要是一行：{reason:?}");
}

/// 断言 DSL 自己出错（magic 写成全角）也不许打穿 —— 此前 hex_decode 按字节切片会 panic。
#[tokio::test]
async fn a_malformed_magic_fails_the_check_instead_of_panicking() {
    let mut sc = by_name("01_simple_qa");
    let mut spec = Map::new();
    spec.insert("check".into(), Value::String("file".into()));
    // 全角数字：6 个字节、长度是偶数，能过「偶数长度」那关，然后在 char 边界上炸。
    spec.insert("magic".into(), Value::String("８９".into()));
    sc.expect = vec![Value::Object(spec)];
    let r = run_scenario(&sc, &options_with(demo_plane::DemoPlane::factory())).await;
    assert_ne!(
        r.phase, "error",
        "不该是 panic 被兜住，而该是断言正常判失败"
    );
    assert!(!r.passed);
}

/// 一直在动的系统要以 drive 超时收场，而不是永远挂着。
#[tokio::test]
async fn a_plane_that_never_settles_times_out_with_a_reason() {
    struct Busy {
        platform: Arc<aite_testing::FakePlatform>,
    }
    #[async_trait]
    impl ControlPlane for Busy {
        async fn handle_event(&self, _ev: NormalizedEvent) -> Result<(), IngressError> {
            Ok(())
        }
        async fn run_forever(&self) {
            loop {
                let _ = self.platform.add_reaction("om_x", ReactionKind::Ack).await;
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
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
    let mut sc = by_name("01_simple_qa");
    sc.timeout_sec = 0.3;
    let factory: PlaneFactory = Arc::new(|deps: &Deps| {
        Ok(Arc::new(Busy {
            platform: deps.platform.clone(),
        }) as Arc<dyn ControlPlane>)
    });
    let r = run_scenario(&sc, &options_with(factory)).await;
    assert_eq!(r.phase, "drive");
    assert!(r.reason.unwrap().contains("没有停下来"));
}

// --- harness 不是空壳 --------------------------------------------------------

/// 最小 ControlPlane 一接上，这几条就该真绿 —— 证明断言不是永远失败的摆设。
#[tokio::test]
async fn demo_plane_makes_scenarios_actually_pass() {
    for name in DEMO_PASSABLE {
        let r = run_scenario(&by_name(name), &demo_options()).await;
        assert!(r.passed, "{name} 没过：{:?} / {:?}", r.reason, r.failures);
        assert_eq!(r.phase, "ok", "{name}");
        assert_eq!(r.settled_by, "quiesce", "{name}");
    }
}

#[tokio::test]
async fn demo_plane_dedups_and_creates_one_task() {
    let r = run_scenario(&by_name("10_duplicate_event"), &demo_options()).await;
    assert!(r.passed, "{:?} / {:?}", r.reason, r.failures);
    assert_eq!(r.stats["tasks"], json!(1));
    assert_eq!(r.stats["model_calls"], json!(1));
}

#[tokio::test]
async fn demo_plane_ignores_bots_entirely() {
    let r = run_scenario(&by_name("09_bot_ignored"), &demo_options()).await;
    assert!(r.passed, "{:?} / {:?}", r.reason, r.failures);
    assert_eq!(r.stats["sessions"], json!(0));
    assert_eq!(r.stats["model_calls"], json!(0));
}

/// 02：话题内追问在 `after: idle` 之后起第二个 task，落在同一个会话里。
#[tokio::test]
async fn demo_plane_continues_a_thread_into_a_second_task() {
    let r = run_scenario(&by_name("02_thread_followup"), &demo_options()).await;
    assert!(r.passed, "{:?} / {:?}", r.reason, r.failures);
    assert_eq!(r.stats["tasks"], json!(2));
    assert_eq!(r.stats["sessions"], json!(1));
}

/// 故意把断言改错，看报告说不说得清「期望什么、实际什么」。
#[tokio::test]
async fn failing_expectation_names_the_gap() {
    let mut sc = by_name("01_simple_qa");
    sc.expect = vec![json!({"check": "platform_calls", "method": "send_text", "equals": 7})];
    let r = run_scenario(&sc, &demo_options()).await;
    assert!(!r.passed);
    assert_eq!(r.phase, "assert");
    assert_eq!(r.failures, vec!["platform.send_text 期望 == 7，实际 1"]);
    assert_eq!(r.reason.unwrap(), "1 条断言没过");
}

// --- 断言 DSL 本身 -----------------------------------------------------------

fn empty_deps() -> Deps {
    aite_evals::build_deps(&Scenario::named("x"), &DepsOptions::default()).expect("空场景")
}

#[test]
fn unknown_check_is_reported_not_raised() {
    let deps = empty_deps();
    let failures = checks::run_checks(&deps, &[json!({"check": "no_such_check"})]);
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("未知的 check"), "{}", failures[0]);
}

#[test]
fn check_without_comparator_is_reported() {
    let deps = empty_deps();
    let failures = checks::run_checks(
        &deps,
        &[json!({"check": "platform_calls", "method": "send_text"})],
    );
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].contains("equals / min / max"),
        "{}",
        failures[0]
    );
}

#[test]
fn malformed_expect_entry_is_reported() {
    let deps = empty_deps();
    let failures = checks::run_checks(&deps, &[json!("不是 mapping")]);
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].contains("不是带 check 键的 mapping"),
        "{}",
        failures[0]
    );
}

#[test]
fn compare_reports_min_and_max_separately() {
    let deps = empty_deps();
    assert!(checks::run_checks(&deps, &[json!({"check": "model_calls", "equals": 0})]).is_empty());
    assert_eq!(
        checks::run_checks(&deps, &[json!({"check": "model_calls", "min": 1})]),
        vec!["ModelPort.chat 次数 期望 >= 1，实际 0"]
    );
    assert_eq!(
        checks::run_checks(&deps, &[json!({"check": "model_calls", "max": -1})]),
        vec!["ModelPort.chat 次数 期望 <= -1，实际 0"]
    );
}

#[test]
fn check_error_is_returned_for_bad_dsl_usage() {
    let deps = empty_deps();
    let spec = json!({"check": "text", "where": "nowhere", "contains": "x"});
    let err = checks::run_check(&deps, spec.as_object().unwrap()).unwrap_err();
    assert!(err.0.contains("where 只能是"), "{}", err.0);
}

// --- CLI --------------------------------------------------------------------

fn cli(args: &[&str]) -> Captured {
    run_capture(
        args.iter().map(|s| s.to_string()).collect(),
        &Wiring::default(),
    )
}

fn cli_with(args: &[&str], wiring: &Wiring) -> Captured {
    run_capture(args.iter().map(|s| s.to_string()).collect(), wiring)
}

#[test]
fn cli_list_prints_the_ten_names() {
    let out = cli(&["run", SUITE_DIR, "--list"]);
    assert_eq!(out.code, 0);
    let names: Vec<String> = serde_json::from_str(&out.stdout_text()).unwrap();
    assert_eq!(
        names,
        vec![
            "01_simple_qa",
            "02_thread_followup",
            "03_checklist_progress",
            "04_csv_to_chart",
            "05_history_summary",
            "06_read_document",
            "07_commands",
            "08_step_limit",
            "09_bot_ignored",
            "10_duplicate_event",
        ]
    );
}

#[test]
fn cli_run_ends_with_passed_k_of_10() {
    let out = cli(&[
        "run",
        SUITE_DIR,
        "--platform",
        "fake",
        "--model",
        "scripted",
    ]);
    let last = out.last_line();
    assert!(last.starts_with("passed "), "{last}");
    assert!(last.ends_with("/10"), "{last}");
    let passed: usize = last
        .split_whitespace()
        .nth(1)
        .and_then(|p| p.split('/').next())
        .and_then(|n| n.parse().ok())
        .unwrap();
    assert_eq!(out.code, if passed == 10 { 0 } else { 1 });
    assert!(!out.stdout_text().contains("panicked"));
}

#[test]
fn cli_run_summary_is_json_before_the_last_line() {
    let out = cli(&["run", SUITE_DIR]);
    let payload = out.payload().expect("最后一行之前是一份 JSON");
    assert_eq!(payload["suite"], json!(SUITE_DIR));
    assert_eq!(payload["total"], json!(10));
}

#[test]
fn cli_only_filters_scenarios() {
    let out = cli(&["run", SUITE_DIR, "--only", "01_simple_qa", "--list"]);
    let names: Vec<String> = serde_json::from_str(&out.stdout_text()).unwrap();
    assert_eq!(names, vec!["01_simple_qa"]);
}

#[test]
fn cli_rejects_unknown_scenario() {
    let out = cli(&["run", SUITE_DIR, "--only", "99_nope"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr_text().contains("没有这些场景"));
    assert!(
        out.stdout.is_empty(),
        "起不来时 stdout 不该有半份 JSON 摘要"
    );
}

#[test]
fn cli_rejects_missing_suite() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let out = cli(&["run", missing.to_str().unwrap()]);
    assert_eq!(out.code, 2);
    assert!(out.stderr_text().contains("场景加载失败"));
}

#[test]
fn cli_writes_json_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("summary.json");
    let out = cli(&["run", SUITE_DIR, "--json", path.to_str().unwrap()]);
    assert!(!out.stdout.is_empty());
    let payload: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(payload["total"], json!(10));
}

/// scripted 默认 scale=1.0 —— 一个字都不往 stderr 写，check.sh 的 B8 靠这个干净。
#[test]
fn cli_scripted_run_keeps_stderr_empty() {
    let out = cli(&["run", SUITE_DIR, "--only", "01_simple_qa"]);
    assert_eq!(out.stderr_text(), "");
}

/// DemoPlane 接进 CLI：passed 4/4，退出码 0。
#[test]
fn cli_with_a_wired_plane_passes_the_demo_scenarios() {
    let wiring = Wiring {
        plane: Some(DemoPlane::factory()),
        ..Wiring::default()
    };
    let mut args = vec!["run", SUITE_DIR];
    for name in DEMO_PASSABLE {
        args.push("--only");
        args.push(name);
    }
    let out = cli_with(&args, &wiring);
    assert_eq!(out.last_line(), "passed 4/4", "{}", out.stdout_text());
    assert_eq!(out.code, 0);
    assert_eq!(out.stderr_text(), "");
}

/// `--sandbox docker` 与 `--model live` 并行期间都还没接线：一行人话 + 退出 2。
#[test]
fn cli_reports_unwired_lanes_with_exit_code_two() {
    let out = cli(&["run", SUITE_DIR, "--sandbox", "docker"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr_text().contains("--sandbox docker 起不来"));
    assert!(out.stderr_text().contains("SandboxFactory"));

    let out = cli(&["run", SUITE_DIR, "--model", "live"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr_text().contains("--model live 起不来"));
    assert!(out.stderr_text().contains("ModelFactory"));
}

#[test]
fn cli_rejects_unknown_flags_and_values() {
    assert_eq!(cli(&["run", SUITE_DIR, "--nope"]).code, 2);
    assert_eq!(cli(&["run", SUITE_DIR, "--platform", "feishu"]).code, 2);
    assert_eq!(cli(&["run", SUITE_DIR, "--sandbox", "podman"]).code, 2);
    assert_eq!(cli(&["nope"]).code, 2);
    assert_eq!(cli(&[]).code, 2);
}

/// `check: store` 的 `task_sessions` / `seen_events` 两格（Python 里只有场景间接用到）。
#[tokio::test]
async fn store_check_covers_task_sessions_and_seen_events() {
    let r = run_scenario(&by_name("02_thread_followup"), &demo_options()).await;
    assert!(r.passed, "{:?}", r.failures);
    let deps = build_after(&by_name("02_thread_followup")).await;
    let failures = checks::run_checks(
        &deps,
        &[
            json!({"check": "store", "task_sessions_equals": 1}),
            json!({"check": "store", "seen_events_equals": 2}),
        ],
    );
    assert!(failures.is_empty(), "{failures:?}");
}

/// 把场景跑一遍再把 deps 交出来（上面那条要在跑完之后读账）。
async fn build_after(sc: &Scenario) -> Deps {
    let deps = aite_evals::build_deps(sc, &DepsOptions::default()).expect("deps");
    let plane: Arc<dyn ControlPlane> = Arc::new(DemoPlane::new(&deps));
    aite_contracts::SessionStore::init(&*deps.store)
        .await
        .unwrap();
    let p = plane.clone();
    deps.platform
        .start(Arc::new(move |ev: NormalizedEvent| {
            let p = p.clone();
            Box::pin(async move { p.handle_event(ev).await })
        }))
        .await
        .unwrap();
    for (_, ev) in sc.dispatch_plan() {
        deps.platform.emit(&ev).await.unwrap();
        plane.run_pending().await;
    }
    deps
}

/// `Deps::stats()` 的六个键一个不少（`ScenarioResult.stats` 的形状）。
#[tokio::test]
async fn stats_shape_is_stable() {
    let r = run_scenario(&by_name("01_simple_qa"), &demo_options()).await;
    let keys: BTreeMap<&String, &Value> = r.stats.iter().collect();
    assert_eq!(
        keys.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
        vec![
            "gateway_calls",
            "model_calls",
            "platform_calls",
            "sandbox_calls",
            "sessions",
            "tasks"
        ]
    );
}
