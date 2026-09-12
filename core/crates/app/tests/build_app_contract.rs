//! 冻结契约 C-TΩ-1 的组装面本身（移植 `tests/integration/test_t8_build_app_contract.py`，4 条）。
//!
//! 别的几组测的是「接起来之后跑得对不对」，这一组测的是**接口没走样**：`AiteApp` 的字段、
//! 三个替身口子、`ServeOptions` 那个 20.0 的默认值、以及硬约束 1
//! 「`build_app` 只组装，不产生副作用」。
//!
//! Python 版用 `inspect.signature` + `dataclasses.fields` 做反射断言；Rust 里这些
//! 「接口没走样」由编译器管 —— 字段少一个、口子改个名，本文件直接编译不过。所以同名的
//! 四条在这里改成**用到**那些字段与口子，顺带把值也钉住。
mod common;

use common::*;

use aite_app::{DEFAULT_SHUTDOWN_GRACE_SEC, Injections, ServeOptions, build_app};
use aite_contracts::{ModelPort, SandboxPort};
use aite_testing::FakeSandbox;
use std::sync::Arc;

/// C-TΩ-1 里 `AiteApp` 逐字写死的字段一个都在，而且都装上了。
#[tokio::test]
async fn aite_app_has_the_frozen_fields() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let app = build_with(
        config.clone(),
        platform.clone(),
        RecordingModel::new(vec![final_step("没人会叫我。")]),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;

    // 十个字段（config / platform / store / evidence / sandbox / gateway / model /
    // worker / plane / ingress）逐个摸一遍：少一个就编译不过，装不上就在这里红。
    assert_eq!(app.config.tenant_id, TENANT);
    assert!(app.platform.capabilities().platform == "fake");
    assert!(app.store.get_task("nope").await.is_err()); // 还没 init()
    assert!(app.evidence.task_dir("t1").ends_with("t1"));
    assert!(app.sandbox.is_some(), "AiteApp.sandbox 没装上");
    assert!(app.gateway.is_some(), "AiteApp.gateway 没装上");
    assert_eq!(app.model.name(), "scripted");
    assert_eq!(app.worker.in_flight().len(), 0);
    assert_eq!(app.plane.pending(), 0);
    assert_eq!(app.ingress.counter("events.handled"), 0);
    // 注入的三个口子都没给 edge 留活（`platform: fake` 那条路不连 edge）
    assert!(app.edge.is_none(), "三个口子都注入了就不该去连 edge");
}

/// 三个口子：给了就用给的。
#[tokio::test]
async fn build_app_keeps_the_three_injection_points() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(vec![final_step("没人会叫我。")]);
    let sandbox = Arc::new(FakeSandbox::new(Vec::new()));

    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;

    // 同一个对象，不是新造的
    assert!(Arc::ptr_eq(
        &(platform.clone() as Arc<dyn aite_contracts::PlatformPort>),
        &app.platform
    ));
    assert!(Arc::ptr_eq(&(model as Arc<dyn ModelPort>), &app.model));
    assert!(Arc::ptr_eq(
        &(sandbox as Arc<dyn SandboxPort>),
        app.sandbox.as_ref().expect("sandbox")
    ));
}

/// 一个口子都不给、config 又是 `platform: fake` → 必须 `StartupError`。
///
/// fake 不是「内建替身」，是「必须注入」的标记（§3.1）。Python 版那句话在
/// `_build_platform` 里，这里原样搬过来并钉住措辞里的两个关键词。
#[tokio::test]
async fn fake_platform_without_injection_refuses_to_fly() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let err = build_app(
        config,
        Injections {
            repo_root: Some(repo_root()),
            ..Injections::default()
        },
    )
    .await
    .expect_err("platform=fake 且没注入平台，必须拒绝起飞");
    assert!(err.0.contains("platform=fake"), "{err}");
    assert!(err.0.contains("注入"), "{err}");
}

/// 宽限期默认 20.0，对齐 `docker-compose.yml` 的 `stop_grace_period: 20s`。
#[test]
fn run_app_grace_period_defaults_to_twenty_seconds() {
    assert_eq!(DEFAULT_SHUTDOWN_GRACE_SEC, 20.0);
    assert_eq!(ServeOptions::default().shutdown_grace_sec, 20.0);
    assert!(ServeOptions::default().stop.is_none());
    assert!(ServeOptions::default().install_signals);
}

/// 硬约束 1：只组装。不连网、不起容器、不发消息。
///
/// 唯一允许的副作用是建那三个落盘目录 —— 所以这条顺带把「目录真被建出来了」也钉住。
#[tokio::test]
async fn build_app_has_no_side_effects() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    // 往下套一层 data/：三个路径的父目录一个都不存在，组装得自己建。
    let config = make_config(&tmp.path().join("data"));
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(vec![final_step("没人会叫我。")]);
    let sandbox = Arc::new(FakeSandbox::new(Vec::new()));

    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;

    // 一个副作用都没有
    assert_eq!(
        platform.inner.calls.len(),
        0,
        "build_app 碰了平台：{:?}",
        platform.inner.calls.methods()
    );
    assert_eq!(
        sandbox.calls.len(),
        0,
        "build_app 碰了沙箱：{:?}",
        sandbox.calls.methods()
    );
    assert_eq!(model.call_count(), 0);
    assert!(!platform.inner.started() && !platform.inner.stopped());
    // 三个落盘目录都建出来了（preflight 第 7 项对人承诺的「起飞时自动 mkdir」）
    assert!(
        std::path::Path::new(&app.config.storage.evidence_dir).is_dir(),
        "evidence_dir 没建"
    );
    assert!(
        std::path::Path::new(&app.config.storage.artifacts_dir).is_dir(),
        "artifacts_dir 没建"
    );
    assert!(
        std::path::Path::new(&app.config.storage.sqlite_path)
            .parent()
            .expect("父目录")
            .is_dir(),
        "sqlite 的父目录没建"
    );
}

/// 读不到 system prompt 就不许起飞，而且话要说到点上（W9 那四条铁律在 platform.md 里）。
#[tokio::test]
async fn missing_system_prompt_refuses_to_fly_with_the_config_key_named() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let mut config = make_config(tmp.path());
    config.worker.system_prompt_path = "no/such/platform.md".to_string();
    let err = build_app(
        config,
        Injections {
            platform: Some(GatedPlatform::new()),
            model: Some(RecordingModel::new(vec![])),
            sandbox: Some(Arc::new(FakeSandbox::new(Vec::new()))),
            repo_root: Some(repo_root()),
        },
    )
    .await
    .expect_err("读不到 system prompt 必须拒绝起飞");
    assert!(err.0.contains("worker.system_prompt_path"), "{err}");
    assert!(err.0.contains("no/such/platform.md"), "{err}");
    assert!(err.0.contains("仓库根"), "{err}");
}
