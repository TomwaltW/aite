//! `evals/p0` 下的 10 个场景文件本身：名字、结构、断言可解析
//! （移植自 `tests/e2e/test_t4_evals_scenarios.py`，19 条）。
use std::collections::BTreeSet;
use std::path::Path;

use aite_contracts::{ChatType, EventKind, SenderKind, all_model_tools};
use aite_evals::scenario::{EventAfter, Scenario, load_scenario, load_suite};
use aite_testing::Repeat;
use serde_json::{Value, json};

const SUITE_DIR: &str = "../../../evals/p0";

/// 派单 §7 那张表，逐字抄下来。场景增删改名都要先过这一条。
const SPEC_SCENARIOS: [&str; 10] = [
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

fn tool_names() -> BTreeSet<String> {
    all_model_tools().iter().map(|t| t.name.clone()).collect()
}

#[test]
fn suite_is_exactly_the_ten_spec_scenarios() {
    assert_eq!(
        suite().iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        SPEC_SCENARIOS
    );
}

#[test]
fn every_scenario_documents_what_it_verifies() {
    for sc in suite() {
        assert!(!sc.title.is_empty(), "{} 缺 title", sc.name);
        assert!(!sc.verifies.is_empty(), "{} 缺 verifies", sc.name);
        assert!(
            sc.spec_ref.contains("§3.8"),
            "{} 的 spec_ref 没指回 §3.8：{:?}",
            sc.name,
            sc.spec_ref
        );
    }
}

#[test]
fn every_scenario_has_events_and_expectations() {
    for sc in suite() {
        assert!(!sc.events.is_empty(), "{} 一个事件都不投", sc.name);
        assert!(!sc.expect.is_empty(), "{} 一条断言都没有", sc.name);
    }
}

/// 每条 expect 都能被 checks 认出来（未知 check 会在这里露馅）。
#[test]
fn every_expect_uses_a_known_check() {
    let deps = aite_evals::build_deps(
        &Scenario::named("probe"),
        &aite_evals::DepsOptions::default(),
    )
    .expect("空场景");
    for sc in suite() {
        for (i, spec) in sc.expect.iter().enumerate() {
            let map = spec
                .as_object()
                .unwrap_or_else(|| panic!("{}.expect[{i}] 不是 mapping", sc.name));
            assert!(
                map.contains_key("check"),
                "{}.expect[{i}] 没有 check 键",
                sc.name
            );
            // 未知 check 会返回 Err 且消息里带「未知的 check」；别的 CheckError（比如
            // 缺比较子）不该在这里出现 —— 场景文件是写全了的
            if let Err(e) = aite_evals::checks::run_check(&deps, map) {
                assert!(
                    !e.0.contains("未知的 check"),
                    "{}.expect[{i}] 用了未知 check：{}",
                    sc.name,
                    e.0
                );
            }
        }
    }
}

/// 场景不许发明契约外的工具名 —— 那样跑到 RΩ 才会以 not_found 收场。
#[test]
fn model_scripts_only_call_contract_tools() {
    let known = tool_names();
    for sc in suite() {
        for step in &sc.model_script {
            for tc in &step.tool_calls {
                assert!(
                    known.contains(&tc.name),
                    "{} 用了契约外的工具 {:?}",
                    sc.name,
                    tc.name
                );
            }
        }
    }
}

#[test]
fn events_build_into_valid_normalized_events() {
    for sc in suite() {
        for ev in sc.build_events() {
            assert_eq!(ev.platform, "fake");
            assert!(!ev.anchor.message_id.is_empty());
            assert_eq!(ev.anchor.chat_id, ev.chat_id);
        }
    }
}

/// R1 只该在 09 被验到；别的场景混进非真人发送者会让结论变糊。
#[test]
fn only_09_has_a_non_human_sender() {
    let non_human: BTreeSet<String> = suite()
        .iter()
        .filter(|sc| {
            sc.build_events()
                .iter()
                .any(|ev| ev.sender_kind != SenderKind::Human)
        })
        .map(|sc| sc.name.clone())
        .collect();
    assert_eq!(non_human, BTreeSet::from(["09_bot_ignored".to_string()]));
}

#[test]
fn scenario_10_really_sends_the_same_event_id_twice() {
    let ids: Vec<String> = by_name("10_duplicate_event")
        .events
        .iter()
        .map(|e| e.event_id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 1);
}

/// R6 的形状：话题内、不带 @，thread_id 指向第一条消息。
#[test]
fn scenario_02_second_message_is_in_thread_without_at() {
    let sc = by_name("02_thread_followup");
    let (first, second) = (&sc.events[0], &sc.events[1]);
    assert!(first.mentioned);
    assert!(first.thread_id.is_none());
    assert!(!second.mentioned);
    assert_eq!(
        second.thread_id.as_deref(),
        Some(format!("om_{}", first.event_id).as_str())
    );
}

/// 附件的 (message_id, file_key) 必须和事件对得上，否则 download_attachment 必然 upstream。
#[test]
fn scenario_04_attachment_is_downloadable_from_the_fixture() {
    let sc = by_name("04_csv_to_chart");
    let ev = &sc.build_events()[0];
    assert!(!ev.attachments.is_empty(), "04 没带附件");
    let key = (
        ev.attachments[0].message_id.clone(),
        ev.attachments[0].file_key.clone(),
    );
    let files = sc.build_files().expect("附件");
    assert!(
        files.contains_key(&key),
        "附件 {key:?} 在 platform.files 里没有对应内容"
    );
    assert_eq!(files[&key], aite_testing::CSV_SAMPLE.as_bytes());
}

#[test]
fn scenario_05_history_mixes_bot_messages_in() {
    let kinds: BTreeSet<String> = by_name("05_history_summary")
        .platform
        .history
        .iter()
        .map(|h| h.sender_kind.clone())
        .collect();
    assert!(kinds.contains("human"));
    assert!(kinds.len() > 1, "05 的历史里没有非真人消息，过滤就无从验起");
}

#[test]
fn scenario_06_document_key_matches_what_the_model_asks_for() {
    let sc = by_name("06_read_document");
    let asked: BTreeSet<String> = sc
        .model_script
        .iter()
        .flat_map(|step| step.tool_calls.iter())
        .filter(|tc| tc.name == "read_document")
        .filter_map(|tc| tc.arguments.get("url_or_token").and_then(Value::as_str))
        .map(String::from)
        .collect();
    let available: BTreeSet<String> = sc.build_documents().keys().cloned().collect();
    assert!(!asked.is_empty());
    assert!(
        asked.is_subset(&available),
        "{asked:?} 不在 {available:?} 里"
    );
}

#[test]
fn scenario_07_uses_bang_commands() {
    let texts: Vec<String> = by_name("07_commands")
        .events
        .iter()
        .map(|e| e.text.clone())
        .collect();
    assert!(texts.iter().any(|t| t.starts_with("!status")));
    assert!(texts.iter().any(|t| t.starts_with("!stop")));
}

/// 07 靠两件事保证命令到达时任务还活着，少一件断言就成了摆设。
///
/// 一是命令事件的 `after: running`（C-T5T6-1）—— 等 e1 起的任务真的被 worker 领走再投；
/// 二是模型脚本里有一步卡住不返回（hold_ticks）—— 替身瞬时返回，worker 一旦被调度上
/// 就会一口气跑到步数上限，没有这个让出点，命令永远赶不上。
#[test]
fn scenario_07_keeps_the_task_alive_until_the_commands_arrive() {
    let sc = by_name("07_commands");
    let [e1, e2, e3] = &sc.events[..] else {
        panic!("07 应该是三条事件");
    };
    assert_eq!(e1.after, EventAfter::None);
    assert_eq!(e2.after, EventAfter::Running);
    assert_eq!(e3.after, EventAfter::Running);
    assert!(
        sc.model_script.iter().any(|s| s.hold_ticks > 0),
        "07 没有一步是卡住的"
    );
}

/// 08 靠 repeat: inf 把模型钉死，否则任务会自己 delivered，测不到上限。
#[test]
fn scenario_08_model_never_finishes() {
    let sc = by_name("08_step_limit");
    assert_eq!(sc.config["worker"]["max_steps"], json!(3));
    assert_eq!(sc.model_script.last().unwrap().repeat, Repeat::Inf);
    assert!(
        sc.model_script
            .iter()
            .flat_map(|s| s.tool_calls.iter())
            .all(|tc| tc.name != "final")
    );
}

/// 模型一次都不该被调用；脚本留空，真被调了就以「脚本已用尽」失败。
#[test]
fn scenario_09_has_an_empty_model_script() {
    assert!(by_name("09_bot_ignored").model_script.is_empty());
}

#[test]
fn name_must_match_filename() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("99_wrong.yaml");
    std::fs::write(&path, "name: something_else\n").unwrap();
    let err = load_scenario(&path).unwrap_err();
    assert!(err.0.contains("与文件名"), "{}", err.0);
}

#[test]
fn missing_suite_dir_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_suite(&dir.path().join("nope")).unwrap_err();
    assert!(err.0.contains("场景目录不存在"), "{}", err.0);
    // 空目录也要说清楚
    let err = load_suite(dir.path()).unwrap_err();
    assert!(err.0.contains("没有 *.yaml 场景文件"), "{}", err.0);
}

/// 场景 yaml 只写关心的字段，其余靠默认值 —— 这条钉住默认值本身。
#[test]
fn defaults_keep_scenario_files_small() {
    let sc: Scenario =
        serde_json::from_value(json!({"name": "x", "events": [{"event_id": "e1", "text": "hi"}]}))
            .unwrap();
    let ev = &sc.build_events()[0];
    assert_eq!(ev.kind, EventKind::Message);
    assert_eq!(ev.sender_kind, SenderKind::Human);
    assert_eq!(ev.chat_type, ChatType::Group);
    assert!(ev.mentioned);
    assert_eq!(ev.anchor.message_id, "om_e1");
    assert!(ev.anchor.thread_id.is_none());
    assert_eq!(ev.chat_id, "oc_demo");
    assert_eq!(ev.workspace_id, "cli_fake_app");
    assert_eq!(ev.sender_name.as_deref(), Some("Alice"));
    assert_eq!(sc.timeout_sec, 10.0);
    assert_eq!(sc.events[0].after_timeout_sec, 5.0);
    // BASE_TIME + 下标秒
    assert_eq!(ev.occurred_at, aite_evals::scenario::base_time());
}

/// 三个场景把 `card_update_min_interval_ms` 调 0（清单 §11 第 35 条），07 还调了 max_steps。
#[test]
fn scenarios_that_turn_off_card_coalescing_are_the_expected_four() {
    let named: Vec<String> = suite()
        .iter()
        .filter(|sc| {
            sc.config
                .get("worker")
                .and_then(|w| w.get("card_update_min_interval_ms"))
                == Some(&json!(0))
        })
        .map(|sc| sc.name.clone())
        .collect();
    assert_eq!(
        named,
        vec![
            "03_checklist_progress",
            "04_csv_to_chart",
            "07_commands",
            "08_step_limit"
        ]
    );
    assert_eq!(
        by_name("07_commands").config["worker"]["max_steps"],
        json!(8)
    );
}
