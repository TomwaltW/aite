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

use aite_app::features::{self, FeatureCtx, StartCtx};
use aite_app::{
    DEFAULT_SHUTDOWN_GRACE_SEC, Injections, ServeOptions, build_app, build_app_with_features,
};
use aite_contracts::{ModelPort, SandboxPort, ToolCallRequest, ToolContext, gateway_tools};
use aite_gateway::{GatewayBuilder, ToolImpl, ToolOutcome};
use aite_testing::FakeSandbox;
use aite_worker::WorkerDeps;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// 硬约束 1 的**注入路**：不起容器、不发消息、不惊动模型。
///
/// 「连网」那一半不在这条里 —— 它是下面 `..._when_all_three_are_injected` /
/// `..._when_one_injection_is_missing` 那一对钉的分界。这条顺带把允许的那个副作用
/// （三个落盘目录真被建出来）一起钉住。
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

// --------------------------------------------------------------------------
// 「注入了就不连 edge，少注入一个才连」—— C-TΩ-1 这条边界原来只有半边有测试
// --------------------------------------------------------------------------

/// `check_contract_version` 连不上 edge 时的固定开销：`EDGE_STATUS_ATTEMPTS`（5）发
/// `GetStatus`，每两发之间 `sleep(1s)` —— 也就是**至少 4s**。
///
/// 这 4s 就是下面两条用来分辨「到底走没走 edge 那条路」的判据。比起断言
/// 「`app.edge` 是不是 None」，它直接量的是**行为**：走了就一定付这笔钱，没走就是瞬间返回。
/// （原来想用一个拼不成 URI 的 socket 地址当探针，实测 `Endpoint::from_shared` 连带空格的
/// 路径都收，这条路不通。）
const EDGE_DIAL_COST_SEC: f64 = 4.0;

/// 快到不可能是走了 edge 的那条路。两条线中间留了 2s 的空档，机器再忙也分得开。
const NO_DIAL_CEILING_SEC: f64 = 2.0;

/// 一个语法合法、但那头什么都没有的 socket 路径。
fn nobody_home_socket(tmp: &std::path::Path) -> String {
    tmp.join("nobody-home.sock").display().to_string()
}

/// 三个口子全注入 → `need_edge` 为假 → 连 edge 那条路一步都不走。
///
/// 两条断言缺一不可：`edge` 是 `None` 只说明「没留下客户端」，**没付那 4s** 才说明
/// `EdgeClient::connect` / `check_contract_version` 根本没被叫到。
/// 把 `need_edge` 那个判断拿掉改成无条件连，这条会在计时上红。
#[tokio::test]
async fn build_app_does_not_touch_edge_when_all_three_are_injected() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let mut config = make_config(tmp.path());
    config.edge.edge_socket = nobody_home_socket(tmp.path());

    let started = std::time::Instant::now();
    let app = build_with(
        config,
        GatedPlatform::new(),
        RecordingModel::new(vec![final_step("没人会叫我。")]),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    let elapsed = started.elapsed().as_secs_f64();

    assert!(app.edge.is_none(), "三个口子都注入了，不该留下 edge 客户端");
    assert!(
        elapsed < NO_DIAL_CEILING_SEC,
        "组装花了 {elapsed:.3}s —— 连 edge 那条路要付 {EDGE_DIAL_COST_SEC}s，\
         这一趟不该走上去"
    );
}

/// 反过来：少注入一个（这里是沙箱）→ 就**真的**去连 edge，并且真付那 4s。
///
/// 这半边原来一条测试都没有，于是 `build_app` 的注释说「不连网」也没人拦得住。
/// 顺带钉住 §2.1「启动顺序无关」：edge 不在**不是**起飞失败，
/// `check_contract_version` 记一行 `aite.edge_unreachable` 就照常返回。
///
/// **这条是真等 4s**（tokio 的 `start_paused` 要 `test-util` feature，
/// 而 `Cargo.toml` 不在本轨可写面上）。断的是**下界**，所以它只会慢，不会抖。
#[tokio::test]
async fn build_app_does_go_to_edge_when_one_injection_is_missing() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let mut config = make_config(tmp.path());
    config.edge.edge_socket = nobody_home_socket(tmp.path());

    let started = std::time::Instant::now();
    let app = build_app(
        config,
        Injections {
            platform: Some(GatedPlatform::new()),
            model: Some(RecordingModel::new(vec![final_step("没人会叫我。")])),
            // 沙箱这个口子空着 —— 只缺一个也要连 edge
            sandbox: None,
            repo_root: Some(repo_root()),
        },
    )
    .await
    .expect("edge 连不上不算起飞失败（§2.1 启动顺序无关）");
    let elapsed = started.elapsed().as_secs_f64();

    assert!(app.edge.is_some(), "走了 edge 那条路就该留下客户端");
    assert!(
        elapsed >= EDGE_DIAL_COST_SEC,
        "只过了 {elapsed:.3}s —— 5 发 GetStatus 之间该有 4 次 sleep(1s)。\
         要么重试次数变了、要么 sleep 没了，`build_app` 注释里那个「最坏 4s」得跟着改"
    );
}

// --------------------------------------------------------------------------
// CC4 ⑥：功能接缝（`features::wire_all` / `start_all` 与两个有序槽）
// --------------------------------------------------------------------------

/// W1 的 18 个功能文件全是空壳：默认 `wire_all` 什么都不留下，`build_app` 的网关目录
/// 逐字等于 `gateway_tools()`，`start_all` 不碰平台。
#[tokio::test]
async fn features_wire_all_is_noop_by_default() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let store: Arc<dyn aite_contracts::SessionStore> = Arc::new(
        aite_store::SqliteSessionStore::open(&config.storage.sqlite_path).expect("open store"),
    );
    let mut ctx = FeatureCtx::new(config.clone(), store);
    features::wire_all(&mut ctx).expect("空壳的 wire 不会失败");
    assert!(ctx.registry.is_empty(), "登记：{:?}", ctx.registry.names());
    assert!(ctx.model_override.is_none(), "模型覆盖槽被写了");
    assert!(ctx.gateway_options.is_empty(), "gateway 槽不空");
    assert!(ctx.worker_options.is_empty(), "worker 槽不空");

    let platform = GatedPlatform::new();
    let app = build_with(
        config,
        platform.clone(),
        RecordingModel::new(vec![final_step("没人会叫我。")]),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    let gateway = app.gateway.as_ref().expect("gateway");
    assert_eq!(gateway.catalog(&tool_ctx()), gateway_tools());

    features::start_all(&StartCtx {
        plane: app.plane.clone(),
        platform: app.platform.clone(),
    });
    assert_eq!(
        platform.inner.calls.len(),
        0,
        "start_all 碰了平台：{:?}",
        platform.inner.calls.methods()
    );
}

/// worker 槽在 `AgentWorker::new` **之前**应用：换掉 `deps.model` 之后，真跑一条 @，
/// 被调的是换进去的那个，注入的那个一次都没被叫到。（build 之后再改 deps 什么都换不掉。）
#[tokio::test]
async fn feature_worker_option_applied_before_build() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let injected = RecordingModel::new(vec![final_step("我是注入的那个。")]);
    let swapped = RecordingModel::new(vec![final_step("我是 worker 槽换进来的。")]);

    let swap = swapped.clone();
    let app = build_app_with_features(
        config,
        Injections {
            platform: Some(platform.clone()),
            model: Some(injected.clone()),
            sandbox: Some(Arc::new(FakeSandbox::new(Vec::new()))),
            repo_root: Some(repo_root()),
        },
        move |ctx| {
            ctx.worker_options
                .push(Box::new(move |deps: &mut WorkerDeps| deps.model = swap));
            Ok(())
        },
    )
    .await
    .expect("build_app_with_features");
    let app: Arc<aite_app::AiteApp> = Arc::from(app);

    let mut run = RunningApp::start(app.clone(), &platform, None).await;
    platform.emit(&event("e1", "你好")).await;
    let p = platform.clone();
    wait_until(|| p.inner.count("send_text") == 1, "交付回帖").await;
    let a = app.clone();
    wait_until(|| a.worker.in_flight().is_empty(), "worker.run() 返回").await;
    run.shutdown().await.expect("shutdown");

    assert_eq!(swapped.call_count(), 1, "worker 槽换进来的模型没被调");
    assert_eq!(injected.call_count(), 0, "注入的模型不该被 worker 调到");
}

/// gateway 槽在包 `Arc` **之前**应用：`with_tool("list_files", 计数器)` 之后经 `app.gateway`
/// 调 `list_files`，走到的是计数器。
#[tokio::test]
async fn feature_gateway_option_applied_before_build() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter: ToolImpl = {
        let hits = hits.clone();
        Arc::new(move |_env, _ctx, _args| {
            hits.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(ToolOutcome::new("计数器")) })
        })
    };

    let app = build_app_with_features(
        config,
        Injections {
            platform: Some(GatedPlatform::new()),
            model: Some(RecordingModel::new(vec![final_step("没人会叫我。")])),
            sandbox: Some(Arc::new(FakeSandbox::new(Vec::new()))),
            repo_root: Some(repo_root()),
        },
        move |ctx| {
            ctx.gateway_options
                .push(Box::new(move |gw: GatewayBuilder| {
                    gw.with_tool("list_files", counter)
                }));
            Ok(())
        },
    )
    .await
    .expect("build_app_with_features");

    let gateway = app.gateway.as_ref().expect("gateway");
    let ctx = tool_ctx();
    gateway.register_task(&ctx.task_id, &ctx.session_token);
    let result = gateway
        .call(
            &ctx,
            &ToolCallRequest {
                call_id: "c1".into(),
                name: "list_files".into(),
                arguments: Default::default(),
            },
        )
        .await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.content, "计数器");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "没走到 gateway 槽换进来的实现"
    );
}

fn tool_ctx() -> ToolContext {
    ToolContext {
        tenant_id: TENANT.into(),
        workspace_id: "cli_app".into(),
        chat_id: "oc_cc4".into(),
        session_id: "sess-cc4".into(),
        task_id: "task-cc4".into(),
        session_token: "tok-cc4".into(),
        thread_id: None,
        attachments_message_id: None,
    }
}
