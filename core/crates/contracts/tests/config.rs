use std::path::PathBuf;

use aite_contracts::*;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn example_yaml_equals_defaults() {
    let text = std::fs::read_to_string(repo_root().join("config/aite.example.yaml")).unwrap();
    let cfg = AiteConfig::from_yaml_str(&text).unwrap();
    assert_eq!(cfg, AiteConfig::default());
}

#[test]
fn empty_is_default_and_partial_overrides_keep_the_rest() {
    assert_eq!(
        AiteConfig::from_yaml_str("").unwrap(),
        AiteConfig::default()
    );
    let cfg = AiteConfig::from_yaml_str(
        "platform: fake\nworker:\n  max_steps: 3\nmodel:\n  provider: scripted\n",
    )
    .unwrap();
    assert_eq!(cfg.platform, PlatformChoice::Fake);
    assert_eq!(cfg.worker.max_steps, 3);
    assert_eq!(cfg.worker.max_wall_sec, 1200);
    assert_eq!(cfg.model.provider, ModelProvider::Scripted);
    assert_eq!(cfg.edge, EdgeConfig::default());
}

#[test]
fn defaults_match_spec() {
    let d = AiteConfig::default();
    assert_eq!(d.tenant_id, "default");
    assert_eq!(d.feishu.history_window, 50);
    assert_eq!(d.model.api_key_env, "AITE_MODEL_API_KEY");
    assert_eq!(d.model.max_tokens, 4096);
    assert_eq!(d.sandbox.image, "aite-sandbox:p0");
    assert_eq!(d.sandbox.idle_sec, 300);
    assert_eq!(d.worker.card_update_min_interval_ms, 500);
    assert_eq!(
        d.worker.system_prompt_path,
        "core/crates/worker/prompts/platform.md"
    );
    assert_eq!(d.storage.sqlite_path, "data/aite.db");
    assert_eq!(d.edge.edge_socket, "data/run/aite-edge.sock");
    assert_eq!(d.edge.core_socket, "data/run/aite-core.sock");
    assert_eq!(d.edge.handle_event_deadline_ms, 1000);
    assert_eq!(d.edge.max_message_mb, 64);
}

#[test]
fn rejects_bad_shapes() {
    assert!(
        AiteConfig::from_yaml_str("- a\n- b\n").is_err(),
        "顶层必须是 mapping"
    );
    assert!(AiteConfig::from_yaml_str("platform: dingtalk\n").is_err());
    assert!(
        AiteConfig::from_yaml_str("worker:\n  max_step: 3\n").is_err(),
        "未知键必须报错"
    );
}

#[test]
fn secrets_are_env_var_names_only() {
    let text = std::fs::read_to_string(repo_root().join("config/aite.example.yaml")).unwrap();
    assert!(!text.contains("sk-"), "样例配置里不许出现密钥取值");
}
