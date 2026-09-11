//! 协议出牌观测装置自测（移植自 `tests/e2e/test_t12_protocol_probe.py` 里
//! **不依赖真 plane 的部分**：探针转发 / 切轮 / 报告序列化 / CLI 五条 / repeat_loops 四条）。
//!
//! Python 那份的另一半（`report_of()` 走真 control + 真 worker 造出兜底与 evidence）
//! 归 RΩ —— 并行期间 R4/R5 不存在，接不上。这里用 `DemoPlane` 与直接喂观测把装置本身
//! 钉住：**装置会不会看错**，而不是真模型的行为。
use std::path::Path;
use std::sync::Arc;

use aite_contracts::{
    Message, ModelError, ModelPort, ModelTurn, Role, ToolCallRequest, Usage, all_model_tools,
};
use aite_evals::cli::{Captured, Wiring, run_capture};
use aite_evals::deps::DepsOptions;
use aite_evals::protocol_probe::{MIN_REPEAT_RUN, ModelProbe, analyze, fingerprint, render_digest};
use aite_evals::runner::{RunOptions, run_scenario};
use aite_evals::scenario::Scenario;
use aite_evals::{DemoPlane, build_deps};
use aite_testing::{FakeModel, ScriptStep, kwargs};
use async_trait::async_trait;
use serde_json::{Map, Value, json};

const SUITE_DIR: &str = "../../../evals/p0";

fn msgs(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| Message::text(Role::User, format!("第 {i} 条")))
        .collect()
}

fn final_step(reply: &str) -> Value {
    json!({"tool_calls": [{"name": "final", "arguments": {"reply": reply}}]})
}

fn scenario(name: &str, script: Vec<Value>) -> Scenario {
    Scenario {
        model_script: script
            .into_iter()
            .map(|v| ScriptStep::from_value(v).expect("脚本"))
            .collect(),
        events: vec![aite_evals::scenario::EventSpec::new("e1", "帮我算一下")],
        ..Scenario::named(name)
    }
}

fn probe_of(script: Vec<Value>) -> (Arc<ModelProbe>, Arc<FakeModel>) {
    let inner = Arc::new(FakeModel::from_values(script).expect("脚本"));
    (Arc::new(ModelProbe::new(inner.clone())), inner)
}

// --- ModelProbe 本身：透明转发 -------------------------------------------------

#[tokio::test]
async fn probe_forwards_chat_and_counts_it() {
    let (probe, _) = probe_of(vec![final_step("算好了")]);
    let turn = probe
        .chat(&msgs(1), all_model_tools(), 16, 0.0)
        .await
        .unwrap();
    assert_eq!(
        turn.message
            .tool_calls
            .unwrap()
            .iter()
            .map(|tc| tc.name.clone())
            .collect::<Vec<_>>(),
        vec!["final".to_string()]
    );
    assert_eq!(probe.call_count(), 1);
    assert_eq!(probe.calls.len(), 1);
    assert_eq!(probe.tool_names_emitted(), vec!["final".to_string()]);
}

/// Rust 没有 Python 的 `__getattr__` 透传 —— 被包模型的额外方法由调用方留一份 Arc 去用。
/// 这条钉住「两头指的是同一个对象」。
#[tokio::test]
async fn probe_shares_the_inner_model_with_the_caller() {
    let (probe, inner) = probe_of(vec![json!({"text": "x", "hold_ticks": 3})]);
    assert_eq!(probe.name, "scripted");
    assert_eq!(ModelPort::name(&*probe), "scripted");
    inner.release_holds();
    assert!(inner.holds_released());
    probe
        .chat(&msgs(1), all_model_tools(), 16, 0.0)
        .await
        .unwrap();
    assert_eq!(inner.turns_served(), 1, "调的确实是同一个被包的模型");
}

#[tokio::test]
async fn probe_records_the_error_and_forwards_it() {
    let (probe, _) = probe_of(vec![json!({"error": "上游 503"})]);
    let err = probe
        .chat(&msgs(1), all_model_tools(), 16, 0.0)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("上游 503"), "{err}");
    assert_eq!(probe.call_count(), 1);
    let obs = probe.observations();
    assert!(obs[0].error.as_ref().unwrap().contains("上游 503"));
    assert!(probe.awaiting_retry());
}

#[test]
fn deps_always_carries_a_probe() {
    let deps = build_deps(
        &scenario("x", vec![final_step("好")]),
        &DepsOptions::default(),
    )
    .unwrap();
    assert_eq!(deps.activity(), 0);
    assert_eq!(deps.stats()["model_calls"], json!(0));
    assert_eq!(deps.model.call_count(), 0);
}

#[test]
fn build_deps_uses_the_injected_model() {
    let inner: Arc<dyn ModelPort> = Arc::new(FakeModel::new(vec![]).named("injected"));
    let deps = build_deps(
        &scenario("x", vec![]),
        &DepsOptions {
            model: Some(inner),
            ..DepsOptions::default()
        },
    )
    .unwrap();
    assert_eq!(deps.model.name, "injected");
}

// --- 切「轮」的两条判据 -------------------------------------------------------

/// 续接：前缀相同 **且** 第 N 条就是上次那张 assistant 回牌 → 同一轮。
#[tokio::test]
async fn a_continuation_stays_in_the_same_run() {
    let (probe, _) = probe_of(vec![final_step("一"), final_step("二")]);
    let first = msgs(1);
    let turn = probe
        .chat(&first, all_model_tools(), 16, 0.0)
        .await
        .unwrap();

    // worker 下一步的第一件事：把模型刚才那张回牌 append 进去
    let mut second = first.clone();
    second.push(turn.message.clone());
    second.push(Message::text(Role::Tool, "工具结果"));
    probe
        .chat(&second, all_model_tools(), 16, 0.0)
        .await
        .unwrap();

    let obs = probe.observations();
    assert_eq!(obs.iter().map(|o| o.run).collect::<Vec<_>>(), vec![0, 0]);
    assert_eq!(obs[1].attempt, 1);
    assert_eq!(obs[1].delta.len(), 2, "delta 只含新长出来的两条");
}

/// **只比前缀不够**：同一会话里的第二个 task，上下文是第一个 task 的前缀扩展
/// （多一条 user turn），只看第 1 条会把两个 task 并成一轮 —— 02_thread_followup 就是。
#[tokio::test]
async fn a_prefix_extension_that_is_not_the_reply_starts_a_new_run() {
    let (probe, _) = probe_of(vec![final_step("一"), final_step("二")]);
    let first = msgs(1);
    probe
        .chat(&first, all_model_tools(), 16, 0.0)
        .await
        .unwrap();

    let mut second = first.clone();
    second.push(Message::text(Role::User, "第二个 task 的提问")); // 不是 assistant 回牌
    probe
        .chat(&second, all_model_tools(), 16, 0.0)
        .await
        .unwrap();

    assert_eq!(
        probe
            .observations()
            .iter()
            .map(|o| o.run)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}

/// 重试：一条都没长、且上一发报错了 —— 单独认这一种，落在同一轮里。
#[tokio::test]
async fn a_retry_after_an_error_stays_in_the_same_run() {
    let (probe, _) = probe_of(vec![json!({"error": "上游 503"}), final_step("这回成了")]);
    let messages = msgs(2);
    let _ = probe.chat(&messages, all_model_tools(), 16, 0.0).await;
    probe
        .chat(&messages, all_model_tools(), 16, 0.0)
        .await
        .unwrap();

    let obs = probe.observations();
    assert_eq!(obs.iter().map(|o| o.run).collect::<Vec<_>>(), vec![0, 0]);
    assert_eq!(obs[1].attempt, 1);
    assert!(!probe.awaiting_retry(), "第二发成功之后就不算在等重试了");
}

/// 长度相同但上一发是成功的 → 不是重试，另起一轮。
#[tokio::test]
async fn same_length_after_a_success_is_a_new_run() {
    let (probe, _) = probe_of(vec![final_step("一"), final_step("二")]);
    let messages = msgs(2);
    probe
        .chat(&messages, all_model_tools(), 16, 0.0)
        .await
        .unwrap();
    probe
        .chat(&messages, all_model_tools(), 16, 0.0)
        .await
        .unwrap();
    assert_eq!(
        probe
            .observations()
            .iter()
            .map(|o| o.run)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}

/// `in_flight` 在调用期间非零 —— settle 的静默判据靠它看见「正在等模型回包」。
#[tokio::test]
async fn in_flight_is_non_zero_while_the_call_is_out() {
    struct Slow {
        seen: Arc<std::sync::Mutex<Vec<usize>>>,
        probe: Arc<std::sync::Mutex<Option<Arc<ModelProbe>>>>,
    }
    #[async_trait]
    impl ModelPort for Slow {
        fn name(&self) -> String {
            "slow".to_string()
        }
        async fn chat(
            &self,
            _m: &[Message],
            _t: &[aite_contracts::ToolSpec],
            _mt: u32,
            _tp: f32,
        ) -> Result<ModelTurn, ModelError> {
            // 在调用途中读一次 in_flight
            let probe = self.probe.lock().unwrap().clone();
            if let Some(p) = probe {
                self.seen.lock().unwrap().push(p.in_flight());
            }
            Ok(ModelTurn {
                message: Message {
                    role: Role::Assistant,
                    content: "慢但是到了".to_string(),
                    tool_calls: Some(vec![ToolCallRequest {
                        call_id: "c1".to_string(),
                        name: "final".to_string(),
                        arguments: kwargs! {"reply" => json!("慢但是到了")},
                    }]),
                    tool_call_id: None,
                    name: None,
                },
                usage: Usage::default(),
                finish_reason: "tool_calls".to_string(),
                raw: Map::new(),
            })
        }
    }
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let slot = Arc::new(std::sync::Mutex::new(None));
    let probe = Arc::new(ModelProbe::new(Arc::new(Slow {
        seen: seen.clone(),
        probe: slot.clone(),
    })));
    *slot.lock().unwrap() = Some(probe.clone());

    assert_eq!(probe.in_flight(), 0);
    probe
        .chat(&msgs(1), all_model_tools(), 16, 0.0)
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap().as_slice(), [1]);
    assert_eq!(probe.in_flight(), 0);
}

// --- 报告的形状 ---------------------------------------------------------------

async fn report_of(sc: &Scenario) -> Map<String, Value> {
    let r = run_scenario(
        sc,
        &RunOptions {
            plane_factory: Some(DemoPlane::factory()),
            collect_protocol: true,
            ..RunOptions::default()
        },
    )
    .await;
    assert_ne!(r.phase, "wiring", "接不上被测系统：{:?}", r.reason);
    assert!(!r.protocol.is_empty(), "开了 collect_protocol 却没拿到报告");
    r.protocol
}

#[tokio::test]
async fn report_is_json_serialisable_and_digest_is_text() {
    let report = report_of(&scenario("p_json", vec![final_step("算好了")])).await;
    let round: Value = serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    assert_eq!(round["model"], json!("scripted"));
    let digest = render_digest(&[("p_json".to_string(), report)]);
    assert!(digest.contains("协议出牌报告"), "{digest}");
    assert!(digest.contains("final"), "{digest}");
}

#[tokio::test]
async fn report_lists_every_tool_call_in_order() {
    let report = report_of(&scenario("p_steps", vec![final_step("算好了")])).await;
    assert_eq!(report["chat_attempts"], json!(1));
    assert_eq!(report["chat_ok"], json!(1));
    assert_eq!(report["reached_final"], json!(1));
    assert_eq!(report["runs"][0]["tools"], json!(["final"]));
    assert_eq!(report["runs"][0]["steps_to_final"], json!(1));
    assert_eq!(report["tool_names"], json!({"final": 1}));
    assert_eq!(report["outbound_texts"], json!(["算好了"]));
}

/// 协议外的工具名要被点名计数；参数不合 schema 要带着违规参数报出来。
#[test]
fn out_of_protocol_and_schema_violations_are_named() {
    let report = report_from_calls(&[
        ("no_such_tool", json!({"x": 1})),
        ("checklist_add", json!({"items": "不是数组"})),
        ("final", json!({"reply": "收工"})),
    ]);
    assert_eq!(report["unknown_tools"], json!({"no_such_tool": 1}));
    let violations = report["schema_violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0]["name"], json!("checklist_add"));
    assert!(
        violations[0]["error"]
            .as_str()
            .unwrap()
            .contains("期望 array"),
        "{}",
        violations[0]["error"]
    );
    assert_eq!(violations[0]["arguments"]["items"], json!("不是数组"));
}

#[tokio::test]
async fn a_clean_run_has_no_violations() {
    let report = report_of(&scenario("p_clean", vec![final_step("好")])).await;
    assert_eq!(report["schema_violations"], json!([]));
    assert_eq!(report["unknown_tools"], json!({}));
    assert_eq!(report["repeat_loops"], json!([]));
}

#[test]
fn analyze_on_an_untouched_probe_is_empty_shaped() {
    let deps = build_deps(&scenario("x", vec![]), &DepsOptions::default()).unwrap();
    let report = analyze(&deps);
    assert_eq!(report["chat_attempts"], json!(0));
    assert_eq!(report["runs"], json!([]));
    assert_eq!(report["tasks"], json!([]));
    let digest = render_digest(&[("x".to_string(), report)]);
    assert!(digest.contains("模型一次都没被调用"), "{digest}");
    // 空 map = 没装探针
    assert!(render_digest(&[("y".to_string(), Map::new())]).contains("没装探针"));
}

// --- repeat_loops 四条 --------------------------------------------------------

/// 原地打转要被标出来 —— 没有哪条兜底接得住它。
///
/// 实测 04_csv_to_chart：沙箱对不含 savefig 的代码一律回 exit_code=0 + 空 stdout，
/// 模型对逐字节相同的 run_python 连发 31 次，一路烧到 max_steps。
#[test]
fn repeating_one_call_verbatim_is_flagged_as_a_loop() {
    let report = report_from_calls(&[
        ("checklist_note", json!({"text": "再查一遍"})),
        ("checklist_note", json!({"text": "再查一遍"})),
        ("checklist_note", json!({"text": "再查一遍"})),
        ("final", json!({"reply": "收工"})),
    ]);
    let loops = report["repeat_loops"].as_array().unwrap();
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0]["name"], json!("checklist_note"));
    assert_eq!(loops[0]["count"], json!(3));
    assert_eq!(loops[0]["first_step"], json!(0));
    assert_eq!(loops[0]["last_step"], json!(2));
    assert_eq!(loops[0]["arguments"], json!({"text": "再查一遍"}));
    // 工具每次都成功 —— 所以两条计数兜底都没数到它
    assert_eq!(report["fallbacks"]["invalid_args"]["count"], json!(0));
    assert_eq!(report["fallbacks"]["sandbox_errors"], json!(0));
    assert!(render_digest(&[("loop".to_string(), report)]).contains("原地打转"));
}

/// 连着两次一样是正常探测（真模型实测里 list_files() 连发两次是常态），别报警。
#[test]
fn two_identical_calls_are_not_a_loop() {
    assert_eq!(MIN_REPEAT_RUN, 3);
    let report = report_from_calls(&[
        ("checklist_note", json!({"text": "再查一遍"})),
        ("checklist_note", json!({"text": "再查一遍"})),
        ("final", json!({"reply": "收工"})),
    ]);
    assert_eq!(report["repeat_loops"], json!([]));
}

/// 同一个工具、参数在变 = 模型还在换招，不算卡住。
#[test]
fn different_arguments_are_not_a_loop() {
    let report = report_from_calls(&[
        ("checklist_note", json!({"text": "第 0 次"})),
        ("checklist_note", json!({"text": "第 1 次"})),
        ("checklist_note", json!({"text": "第 2 次"})),
        ("final", json!({"reply": "收工"})),
    ]);
    assert_eq!(report["repeat_loops"], json!([]));
}

/// 中间插进别的调用就断了连续 —— 只认**连续**相同。
#[test]
fn an_interruption_breaks_the_run() {
    let report = report_from_calls(&[
        ("checklist_note", json!({"text": "同"})),
        ("checklist_note", json!({"text": "同"})),
        ("list_files", json!({})),
        ("checklist_note", json!({"text": "同"})),
        ("final", json!({"reply": "收工"})),
    ]);
    assert_eq!(report["repeat_loops"], json!([]));
}

/// 指纹与 worker 的 `_call_signature` 逐字同构：`"{name}:{顶层 key 排序的 JSON}"`。
#[test]
fn fingerprint_sorts_top_level_keys_and_keeps_chinese() {
    let mut args = Map::new();
    args.insert("z".to_string(), json!(1));
    args.insert("a".to_string(), json!("中文"));
    assert_eq!(
        fingerprint("run_python", &args),
        r#"run_python:{"a":"中文","z":1}"#
    );
}

/// 一整轮里所有步骤的出牌，按顺序喂给探针，再出报告。
fn report_from_calls(calls: &[(&str, Value)]) -> Map<String, Value> {
    let script: Vec<Value> = calls
        .iter()
        .map(|(name, args)| json!({"tool_calls": [{"name": name, "arguments": args}]}))
        .collect();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let deps = build_deps(&scenario("loop", script), &DepsOptions::default()).unwrap();
        // 一轮 = messages 持续增长且每步头一条新消息是上次的回牌
        let mut messages = msgs(1);
        loop {
            match deps.model.chat(&messages, all_model_tools(), 16, 0.0).await {
                Err(_) => break,
                Ok(turn) => {
                    messages.push(turn.message.clone());
                    messages.push(Message::text(Role::Tool, "ok"));
                }
            }
        }
        analyze(&deps)
    })
}

// --- CLI 五条 -----------------------------------------------------------------

fn cli(args: &[&str]) -> Captured {
    run_capture(
        args.iter().map(|s| s.to_string()).collect(),
        &Wiring {
            plane: Some(DemoPlane::factory()),
            ..Wiring::default()
        },
    )
}

#[test]
fn cli_without_the_flag_adds_no_protocol_field() {
    let out = cli(&["run", SUITE_DIR, "--only", "01_simple_qa"]);
    let payload = out.payload().unwrap();
    assert!(payload["scenarios"][0].get("protocol").is_none());
}

#[test]
fn cli_protocol_report_writes_stderr_digest_and_keeps_stdout_shape() {
    let out = cli(&[
        "run",
        SUITE_DIR,
        "--only",
        "01_simple_qa",
        "--protocol-report",
    ]);
    assert_eq!(out.code, 0, "{}", out.stderr_text());
    assert_eq!(out.last_line(), "passed 1/1");
    let payload = out.payload().unwrap();
    assert_eq!(
        payload["scenarios"][0]["protocol"]["runs"][0]["tools"],
        json!(["final"])
    );
    assert!(out.stderr_text().contains("协议出牌报告"));
}

#[test]
fn cli_protocol_report_path_writes_a_json_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("protocol.json");
    let out = cli(&[
        "run",
        SUITE_DIR,
        "--only",
        "01_simple_qa",
        "--protocol-report",
        path.to_str().unwrap(),
    ]);
    assert_eq!(out.code, 0);
    let payload: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(payload["model"], json!("scripted"));
    assert_eq!(payload["contract_version"], json!("p0.2"));
    assert_eq!(payload["sandbox"], json!("fake"));
    assert_eq!(payload["scenarios"][0]["name"], json!("01_simple_qa"));
    assert_eq!(
        payload["scenarios"][0]["runs"][0]["tools"],
        json!(["final"])
    );
}

#[test]
fn cli_digest_goes_to_stderr_not_stdout() {
    let out = cli(&[
        "run",
        SUITE_DIR,
        "--only",
        "01_simple_qa",
        "--protocol-report",
    ]);
    assert!(out.stderr_text().contains("协议出牌报告"));
    assert!(!out.stdout_text().contains("协议出牌报告"));
}

/// `--timeout-scale` 会在 stderr 说一句；scripted 默认 1.0 所以什么都不说。
#[test]
fn cli_timeout_scale_announces_itself() {
    let out = cli(&[
        "run",
        SUITE_DIR,
        "--only",
        "01_simple_qa",
        "--timeout-scale",
        "12",
    ]);
    assert!(out.stderr_text().contains("x12"), "{}", out.stderr_text());
    let out = cli(&["run", SUITE_DIR, "--only", "01_simple_qa"]);
    assert_eq!(out.stderr_text(), "");
}

/// `--traceback` 把失败细节写 stderr，不污染 stdout 的 JSON 摘要。
#[test]
fn cli_traceback_goes_to_stderr_only() {
    let out = run_capture(
        [
            "run",
            SUITE_DIR,
            "--only",
            "03_checklist_progress",
            "--traceback",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        &Wiring {
            plane: Some(DemoPlane::factory()),
            ..Wiring::default()
        },
    );
    assert_eq!(out.code, 1, "03 在 DemoPlane 下必然红（它不发卡片）");
    assert!(out.stderr_text().contains("03_checklist_progress"));
    assert!(out.payload().is_some(), "stdout 仍是一份 JSON + 最后一行");
    assert_eq!(out.last_line(), "passed 0/1");
}

/// `Path` 只在这里用一次（`load_suite` 的签名），避免未使用导入。
#[test]
fn suite_dir_exists() {
    assert!(Path::new(SUITE_DIR).is_dir(), "{SUITE_DIR} 不在");
}
