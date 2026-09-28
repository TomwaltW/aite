//! CC7 ①②⑥：场景级的三个旋钮 —— `EventSpec.platform`、`platform.capabilities`、`worker_options`。
//!
//! 覆盖**确实生效**由这里的 Rust 测试证明；`evals/p1/CC7_capability_override.yaml`
//! 按设计对覆盖不敏感（B0 上 core 不读能力位），不靠它。
use std::path::PathBuf;

use aite_contracts::PlatformPort;
use aite_evals::scenario::{EventSpec, Scenario, WorkerOptions, load_scenario};
use aite_evals::{DepsOptions, build_deps};
use serde_json::{Value, json};

fn scenario_from(v: Value) -> Scenario {
    serde_json::from_value(v).expect("场景")
}

/// 写一个临时 yaml 再走 `load_scenario`（与 `evals run` 同一条加载路）。
fn load_yaml(name: &str, body: &str) -> Result<Scenario, String> {
    let dir = std::env::temp_dir().join(format!("cc7-scenario-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let path: PathBuf = dir.join(format!("{name}.yaml"));
    std::fs::write(&path, body).expect("写 yaml");
    let out = load_scenario(&path).map_err(|e| e.0);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn wiring_error(sc: &Scenario) -> String {
    match build_deps(sc, &DepsOptions::default()) {
        Ok(_) => panic!("该在 wiring 阶段失败，实际造出来了"),
        Err(e) => {
            assert_eq!(e.phase, "wiring", "{}", e.message);
            e.message
        }
    }
}

// --- ① EventSpec.platform ------------------------------------------------------

#[test]
fn event_spec_platform_defaults_to_fake() {
    let spec = EventSpec::new("e1", "你好");
    assert_eq!(spec.platform, "fake");
    let ev = spec.build(0);
    assert_eq!(ev.platform, "fake");
    assert_eq!(ev.anchor.platform, "fake");
}

#[test]
fn event_spec_platform_is_carried_into_event_and_anchor() {
    let spec: EventSpec = serde_json::from_value(json!({
        "event_id": "e1",
        "text": "你好",
        "platform": "dingtalk",
    }))
    .expect("EventSpec");
    let ev = spec.build(0);
    assert_eq!(ev.platform, "dingtalk");
    assert_eq!(ev.anchor.platform, "dingtalk");
    // deny_unknown_fields 仍在：拼错的键照样当场报错
    let bad = serde_json::from_value::<EventSpec>(json!({"event_id": "e1", "platfrom": "x"}));
    assert!(bad.is_err(), "拼错的键该被拒");
}

// --- ② platform.capabilities ---------------------------------------------------

#[test]
fn capabilities_fixture_reaches_platform_port() {
    let sc = scenario_from(json!({
        "name": "cap",
        "platform": {"capabilities": {"supports_thread": false, "outbound_rate_per_min": 20}},
    }));
    let deps = build_deps(&sc, &DepsOptions::default()).expect("造 deps");
    let caps = deps.platform.capabilities();
    assert!(!caps.supports_thread, "覆盖没到 PlatformPort：{caps:?}");
    assert_eq!(caps.outbound_rate_per_min, 20);
    // 没覆盖的键仍是 fake_p0() 的值（部分覆盖）
    let base = aite_testing::fake_p0();
    assert_eq!(caps.platform, base.platform);
    assert_eq!(caps.supports_history, base.supports_history);
    assert_eq!(caps.card_edit_window_sec, base.card_edit_window_sec);

    // 不写 capabilities 就是原样的 fake_p0()
    let plain = build_deps(&Scenario::named("plain"), &DepsOptions::default()).expect("造 deps");
    assert_eq!(plain.platform.capabilities(), base);
}

#[test]
fn capabilities_fixture_unknown_key_is_a_wiring_error() {
    // 最常见的拼错：多一个 s。契约结构没有 deny_unknown_fields，不查就是静默忽略
    let sc = scenario_from(json!({
        "name": "cap_typo",
        "platform": {"capabilities": {"supports_threads": false}},
    }));
    let msg = wiring_error(&sc);
    assert!(msg.contains("supports_threads"), "没点名错键：{msg}");
    assert!(msg.contains("supports_thread\""), "没列合法键：{msg}");
    assert!(msg.contains("outbound_rate_per_min"), "没列合法键：{msg}");

    // 类型错也在 wiring 报
    let sc = scenario_from(json!({
        "name": "cap_type",
        "platform": {"capabilities": {"supports_thread": "否"}},
    }));
    let msg = wiring_error(&sc);
    assert!(msg.contains("platform.capabilities"), "{msg}");
}

// --- ⑥ worker_options ----------------------------------------------------------

#[test]
fn worker_options_default_off_reaches_deps() {
    let sc = Scenario::named("wo");
    assert_eq!(sc.worker_options, WorkerOptions::default());
    assert!(!sc.worker_options.aigc_label);
    let deps = build_deps(&sc, &DepsOptions::default()).expect("造 deps");
    assert_eq!(deps.worker_options, WorkerOptions::default());

    // 显式写 false 也照常走通
    let sc = load_yaml(
        "wo_off",
        "name: wo_off\nworker_options: {aigc_label: false}\n",
    )
    .expect("加载");
    let deps = build_deps(&sc, &DepsOptions::default()).expect("造 deps");
    assert!(!deps.worker_options.aigc_label);
}

#[test]
fn worker_options_unknown_key_rejected_at_load() {
    let err = load_yaml(
        "wo_typo",
        "name: wo_typo\nworker_options: {aigc_lable: true}\n",
    )
    .expect_err("拼错的键该在加载时被拒");
    assert!(err.contains("aigc_lable"), "{err}");
}

#[test]
fn worker_options_label_without_consumer_is_a_wiring_error() {
    let sc = load_yaml("wo_on", "name: wo_on\nworker_options: {aigc_label: true}\n").expect("加载");
    let msg = wiring_error(&sc);
    assert!(
        msg.contains("worker_options.aigc_label 还没有消费方"),
        "闸门文案不对：{msg}"
    );
}
