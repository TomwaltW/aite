//! 优雅退出与宽限期超时（移植 `test_t8_graceful_shutdown.py` 3 条
//! + `test_t8_shutdown_grace_timeout.py` 3 条）。
//!
//! 冻结下来的顺序是：
//!
//! ```text
//! platform.stop()            ← 先闭嘴，不再收新事件
//! plane.join() 限时 shutdown_grace_sec
//! （超时才取消 run_forever，取消前先抄 worker.in_flight）
//! 给硬取消的任务 cancel_task(notify=false)
//! sandbox.close_all()
//! store.close()
//! ```
//!
//! 所以这一组钉住的不是「退出了」，而是**退出的顺序**：闭嘴要发生在收尾之前，收尾期间
//! 到达的事件一条都不许被处理，而在跑的任务必须善终。
//!
//! 怎么把「正在跑」做成确定的：`FakeModel` 的 `hold_ticks` 让 worker 停在「下一次 chat」
//! 上 —— 上一步的副作用（卡片、库里的状态）都已经落地，别的任务（我们的收尾）终于插得进来。
//! 放行由 `release_holds()` 显式给出，不靠睡时间。
mod common;

use common::*;

use aite_contracts::TaskStatus;
use aite_testing::FakeSandbox;
use serde_json::json;
use std::sync::Arc;

/// 第 2 步停在 chat 里不返回，直到测试显式 `release_holds()`。
fn holding_script() -> Vec<aite_testing::ScriptStep> {
    vec![
        tool_step("checklist_add", json!({"items": ["干活"]})),
        holding(final_step("收尾时把手上的活干完了。"), HOLD_FOREVER),
    ]
}

/// 宽限期压到 50ms：`plane.join()` 必然超时，走取消那一支。
const TINY_GRACE_SEC: f64 = 0.05;

/// 第 1 步先把沙箱建起来（`run_python` 会让 Gateway acquire），
/// 第 2 步停在 chat 里 —— hold_ticks 大到没人放行就等于永远收不完。
fn stuck_script() -> Vec<aite_testing::ScriptStep> {
    vec![
        tool_step(
            "run_python",
            json!({"code": "print('起个沙箱')", "timeout_sec": 30}),
        ),
        holding(final_step("永远到不了这一句。"), HOLD_FOREVER),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_closes_the_platform_before_draining_and_the_store_last() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(holding_script());
    let sandbox = ClosableFakeSandbox::new();
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;

    let mut run = RunningApp::start(app.clone(), &platform, None).await;
    platform.emit(&event("e1", "干个长活")).await;
    // 等到任务真的在跑：模型停在第 2 步上（这时任务必然已落库并被领走）
    wait_until(|| model.holds() >= 1, "worker 停在第 2 步的 chat 上").await;
    let task = app.store.list_active_tasks(CHAT).await.expect("list")[0].clone();
    // 卡片发出去了（W3），而且只有一张
    assert_eq!(platform.inner.card_count(), 1);
    assert_eq!(
        app.store
            .get_task(&task.id)
            .await
            .expect("get_task")
            .expect("task")
            .status,
        TaskStatus::Working
    );

    run.stop.set();
    wait_until(|| platform.inner.stopped(), "run_app 调 platform.stop()").await;

    // 收尾期间平台上又来了一条消息 —— 连接已经断了，它不该被处理
    platform
        .emit(&Ev::new("e2", "收尾期间的新消息").message_id("om_2").build())
        .await;
    assert_eq!(platform.dropped_after_stop(), 1);

    model.release_holds(); // 手上的活可以收了
    run.shutdown().await.expect("run_app 正常收场");

    // ── 退出之后 ────────────────────────────────────────────────
    let methods = platform.inner.calls.methods();
    let stop_at = methods.iter().position(|m| m == "stop").expect("stop");
    let last_send_text = methods
        .iter()
        .rposition(|m| m == "send_text")
        .expect("send_text");
    assert!(
        stop_at < last_send_text,
        "platform.stop() 必须发生在在跑任务的交付之前，实际顺序：{methods:?}"
    );

    // 在跑的任务善终了：状态、证据、manifest 一个不少
    let finished = read_task_from_disk(&config, &task.id)
        .await
        .expect("任务还在库里");
    assert_eq!(finished.status, TaskStatus::Delivered);
    assert_eq!(
        read_manifest(&config, &task.id)["root_hash"],
        json!(finished.evidence_root_hash)
    );

    // 第二条事件一点痕迹都没留下
    assert_eq!(evidence_task_dirs(&config), vec![task.id.clone()]);
    assert_eq!(model.call_count(), 2, "只有脚本里那两步，e2 没惊动模型");
    assert_eq!(app.ingress.counter("events.handled"), 1);

    // store.close() 调过了：再用它查任何东西都该报「未初始化」
    assert!(
        app.store.get_task(&task.id).await.is_err(),
        "store.close() 之后还查得到，说明最后一步没走到"
    );
    // 有沙箱就 close_all（C-TΩ-1 退出序列倒数第二步）
    assert_eq!(sandbox.close_calls(), 1);
}

/// 队列是空的时候，收尾不许干等宽限期。
///
/// 这里刻意**不传** grace，走 C-TΩ-1 的默认 20.0：2s 的 exit_timeout 还能过，
/// 就说明 `plane.join()` 是等队列而不是等钟。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_with_nothing_running_returns_at_once() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let app = build_with(
        config.clone(),
        platform.clone(),
        RecordingModel::new(vec![final_step("没人叫我。")]),
        // 裸 FakeSandbox：它走的是契约里 `close_all` 的默认空实现。
        // 收尾不许因为「沙箱没自己实现 close_all」就炸掉。
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;

    let mut run = RunningApp::start(app.clone(), &platform, None).await;
    run.shutdown_within(2.0).await.expect("run_app 正常收场");

    assert!(platform.inner.stopped());
    assert_eq!(platform.inner.outbound_count(), 0);
    assert!(app.store.get_task("whatever").await.is_err());
}

/// 退出之后平台再推事件（比如 adapter 没停干净），系统状态一动不动。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_arriving_after_shutdown_change_nothing() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(vec![final_step("干完了。")]);
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;

    let mut run = RunningApp::start(app.clone(), &platform, None).await;
    platform.emit(&event("e1", "干个活")).await;
    wait_until(|| platform.inner.count("send_text") == 1, "任务交付").await;
    let task = app.store.list_active_tasks(CHAT).await.expect("list");
    let task_id = if task.is_empty() {
        // 已经交付了，活跃列表里就没有了 —— 从 evidence 目录反查
        evidence_task_dirs(&config)
            .first()
            .cloned()
            .expect("evidence 目录里该有这个任务")
    } else {
        task[0].id.clone()
    };
    run.shutdown().await.expect("run_app 正常收场");

    let before = platform.inner.outbound_count();
    platform
        .emit(&Ev::new("e2", "人走了才到").message_id("om_2").build())
        .await;
    settle().await;

    assert_eq!(platform.dropped_after_stop(), 1);
    assert_eq!(platform.inner.outbound_count(), before);
    assert_eq!(model.call_count(), 1);
    assert_eq!(evidence_task_dirs(&config), vec![task_id]);
}

// --------------------------------------------------------------------------
// 宽限期超时那一支
// --------------------------------------------------------------------------

/// 把任务推到「沙箱已建、模型停在第 2 步」这个确定的状态。
async fn drive_until_stuck(
    app: &Arc<aite_app::AiteApp>,
    platform: &Arc<GatedPlatform>,
    model: &Arc<RecordingModel>,
    sandbox_calls: impl Fn() -> usize,
) -> aite_contracts::Task {
    platform.emit(&event("e1", "干个收不完的活")).await;
    wait_until(
        || model.holds() >= 1 && sandbox_calls() == 1,
        "沙箱建好、模型停在第 2 步",
    )
    .await;
    app.store.list_active_tasks(CHAT).await.expect("list")[0].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grace_timeout_cancels_the_stuck_task_and_still_returns() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(stuck_script());
    let sandbox = ClosableFakeSandbox::new();
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;

    let started = std::time::Instant::now();
    let mut run = RunningApp::start(app.clone(), &platform, Some(TINY_GRACE_SEC)).await;
    let inner = sandbox.inner.clone();
    let task = drive_until_stuck(&app, &platform, &model, || inner.calls.count("acquire")).await;
    let alive_before = sandbox.inner.alive();
    assert_eq!(alive_before.len(), 1, "run_python 之后应当有一个活着的沙箱");
    let sandbox_id = alive_before[0].clone();
    run.shutdown_within(5.0).await.expect("run_app 正常收场");
    let elapsed = started.elapsed().as_secs_f64();

    // 不许挂死：宽限期 50ms，整轮下来远不该到秒级
    assert!(
        elapsed < 3.0,
        "run_app 花了 {elapsed:.3}s 才返回，宽限期只有 {TINY_GRACE_SEC}s"
    );

    // 被取消的那条真的死了：再让一批调度，模型那个自旋不许再往前走一格
    let frozen = model.inner.hold_ticks_yielded();
    settle().await;
    assert_eq!(
        model.inner.hold_ticks_yielded(),
        frozen,
        "run_forever 被取消了，卡住的 worker 却还在跑"
    );
    assert_eq!(model.inner.turns_served(), 1, "第 2 步不该出牌");

    // 任务确实没交付
    assert_eq!(platform.inner.count("send_text"), 0);
    assert_eq!(platform.inner.count("send_file"), 0);

    // 沙箱还回去了 —— 走的是 C-TΩ-1 退出序列里的 close_all + cancel_task
    assert_eq!(sandbox.close_calls(), 1);
    assert_eq!(sandbox.inner.released_ids(), vec![sandbox_id]);
    assert_eq!(sandbox.inner.alive(), Vec::<String>::new());

    // store.close() 照样走到
    assert!(app.store.get_task(&task.id).await.is_err());
}

/// 沙箱只实现 §3.2 冻结的 `SandboxPort`（`close_all` 用默认空实现）时，收尾不许炸。
///
/// Python 那边这条测的是「`aclose` 不在协议里，裸调会崩」；Rust 把 `close_all` 写进了
/// 契约并给了默认实现，所以「崩」这条路在类型上就没了 —— 这里钉住的是它的结论：
/// 收尾整段照样走完（平台停了、库关了、被取消的协程死透了）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grace_timeout_survives_a_protocol_only_sandbox() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(stuck_script());
    let sandbox = Arc::new(FakeSandbox::new(Vec::new()));
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;

    let mut run = RunningApp::start(app.clone(), &platform, Some(TINY_GRACE_SEC)).await;
    let sb = sandbox.clone();
    let task = drive_until_stuck(&app, &platform, &model, || sb.calls.count("acquire")).await;
    run.shutdown_within(5.0).await.expect("run_app 正常收场");

    // 收尾整段走完了：平台停了、库关了
    assert!(platform.inner.stopped());
    assert!(app.store.get_task(&task.id).await.is_err());

    // 被取消的协程也死透了
    let frozen = model.inner.hold_ticks_yielded();
    settle().await;
    assert_eq!(model.inner.hold_ticks_yielded(), frozen);
}

/// 被硬取消的任务也得善终：沙箱还回去、任务落 cancelled、证据链 finalize。
///
/// 三条一起断言，免得修的人只看见最先炸的那一条。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_task_lands_on_cancelled_and_returns_its_sandbox() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(stuck_script());
    let sandbox = Arc::new(FakeSandbox::new(Vec::new()));
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;

    let mut run = RunningApp::start(app.clone(), &platform, Some(TINY_GRACE_SEC)).await;
    let sb = sandbox.clone();
    let task = drive_until_stuck(&app, &platform, &model, || sb.calls.count("acquire")).await;
    run.shutdown_within(5.0).await.expect("run_app 正常收场");

    let finished = read_task_from_disk(&config, &task.id)
        .await
        .expect("任务还在库里");
    let observed = [
        ("沙箱还回去了", sandbox.alive().is_empty()),
        (
            "任务落到 cancelled 终态",
            finished.status == TaskStatus::Cancelled,
        ),
        (
            "证据链 finalize 了（有 manifest.json）",
            manifest_path(&config, &task.id).is_file(),
        ),
    ];
    assert!(
        observed.iter().all(|(_, v)| *v),
        "被取消的任务没善终：{observed:?}"
    );
}
