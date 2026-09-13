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

/// 收尾整段的上限。量的是**纯收尾**那一段，不含建场（见 `grace_timeout_...` 里的计时起点）。
const SHUTDOWN_CEILING_SEC: f64 = 1.0;

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
    let task = first_active_task(&app.store, CHAT, "长活落库并被 worker 领走").await;
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
    first_active_task(&app.store, CHAT, "收不完的那个活落库并被领走").await
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

    let mut run = RunningApp::start(app.clone(), &platform, Some(TINY_GRACE_SEC)).await;
    let inner = sandbox.inner.clone();
    let task = drive_until_stuck(&app, &platform, &model, || inner.calls.count("acquire")).await;
    let alive_before = sandbox.inner.alive();
    assert_eq!(alive_before.len(), 1, "run_python 之后应当有一个活着的沙箱");
    let sandbox_id = alive_before[0].clone();
    // 计时起点就压在这一句前面 —— 要量的是**收尾**，建场不算。
    // 原来 `started` 起在 `RunningApp::start` 之前，于是 `elapsed` 里裹着两个各 5s 预算的
    // `wait_until`（起飞那一个 + `drive_until_stuck` 那一个）：机器一忙，建场自己就能把
    // 3s 的阈值吃光，这条因此在并行跑的时候反复假红（台账 §4.1 / §五）。
    let started = std::time::Instant::now();
    run.shutdown_within(5.0).await.expect("run_app 正常收场");
    let elapsed = started.elapsed().as_secs_f64();

    // 不许挂死：宽限期 50ms，收尾整段远不该到秒级。
    // 阈值跟着起点一起复核过：现在量的是纯收尾（platform.stop → 50ms 超时 → abort+await
    // → 给硬取消的任务 cancel_task → close_all → store.close），实测在百毫秒量级，
    // 1s 留的是十倍的余量。
    assert!(
        elapsed < SHUTDOWN_CEILING_SEC,
        "收尾花了 {elapsed:.3}s 才返回，宽限期只有 {TINY_GRACE_SEC}s"
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

/// 收尾读 `worker.in_flight()` 的时机：必须在 `run_forever` **彻底停下之后**。
///
/// 这是审核记账 4.1 第 2 行那条并发窗口的根。原序列在 `abort()` **之前**抄快照，
/// 抄完到 abort 生效之间 worker 在另一条线上照样在跑（`new_multi_thread`，两条线真并行），
/// 于是快照两头都可能错：漏掉派发循环刚 pop 出来的新任务（它不在 stranded 里，
/// 没人给它善终），或者拿到一份过期的（那个任务已经自己跑完落了 delivered，
/// 而 `cancel_task` 会把它改写成 cancelled、卡片翻成「已取消」）。
///
/// 怎么把「之后」钉成**确定的**：`RecordingModel::chat()` 里挂了一个 drop guard，
/// future 被丢掉的那一刻必然把 `chat_alive` 置假。卡住的 worker 正停在 `chat()` 上，
/// 所以「抄快照那一刻 `chat_alive` 为假」⟺「`run_forever` 那条 future 已经被 abort 丢掉」。
/// 不靠 sleep，不靠自旋计数，也不拿墙钟当判据。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stranded_snapshot_is_taken_after_the_runner_has_stopped() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(stuck_script());
    let sandbox = ClosableFakeSandbox::new();
    let (app, spy) = build_with_spy(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;

    let mut run = RunningApp::start(app.clone(), &platform, Some(TINY_GRACE_SEC)).await;
    let inner = sandbox.inner.clone();
    let task = drive_until_stuck(&app, &platform, &model, || inner.calls.count("acquire")).await;
    run.shutdown_within(5.0).await.expect("run_app 正常收场");

    let alive = spy.alive_at_calls();
    assert_eq!(
        alive.len(),
        1,
        "收尾只该读一次 in_flight（多读一次就是又开了一个窗口）：{alive:?}"
    );
    assert!(
        !alive[0],
        "抄快照的时候 worker 那条 future 还活着 —— 这正是窗口：\
         抄完到 abort 生效之间它还能把自己跑完（快照就过期了）或者领走一个新任务\
         （新任务就不在快照里）"
    );

    // 顺带确认这一轮真的走了超时那一支（不然上面两条断言是空的）
    let finished = read_task_from_disk(&config, &task.id)
        .await
        .expect("任务还在库里");
    assert_eq!(finished.status, TaskStatus::Cancelled);
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

// --------------------------------------------------------------------------
// 宽限期取到非法值 —— 收尾一步都不许少（拆弹，不是再挡一次门）
// --------------------------------------------------------------------------

/// 不管 `shutdown_grace_sec` 递进来的是什么，C-TΩ-1 的退出序列都必须**整段走完**。
///
/// 病在哪：`shutdown()` 里那句 `Duration::from_secs_f64(grace.max(0.0))`。
/// `from_secs_f64` 对 `NaN`、负数、以及 `Duration` 装不下的有限数都 panic，
/// 而 `max(0.0)` 只兜住了负数一类（`NaN.max(0.0)` 碰巧是 `0.0`），**上溢一个字都没拦**。
/// panic 点在收尾中段，它后面的 `sandbox.close_all()` 与 `store.close()` 会整段被跳过：
/// 容器不还、SQLite 不关。
///
/// 为什么门口那道校验不算数：`cli.rs` 的 `0..=86400`（V6 ④a）只守着命令行这一个入口，
/// 而 `ServeOptions::shutdown_grace_sec` 是 `pub` 的裸 `f64` —— 本文件这样直接构造
/// `ServeOptions` 的调用方（以及将来任何嵌入式用法）从它旁边就走过去了。
///
/// 所以这里断的**不是「没 panic」**，是**「收尾跑完了」**：平台停了、`close_all` 走过、
/// 库关了、`run_app` 正常返回 —— 退出序列的最后三步一个不少。
async fn shutdown_runs_to_the_end_with_grace(grace: f64) {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let sandbox = ClosableFakeSandbox::new();
    let app = build_with(
        config.clone(),
        platform.clone(),
        RecordingModel::new(vec![final_step("没人叫我。")]),
        sandbox.clone(),
    )
    .await;

    // 队列是空的：宽限期取什么值都不影响这一轮要跑多久（`plane.join()` 立刻返回），
    // 唯一被考的就是「这个 f64 能不能安全地变成 Duration」。
    // 用套件标准的 10s 预算（`RunningApp::shutdown`），不自己压一个更紧的：
    // 这四条要钉的是「收尾一步没少」，不是「收尾很快」——「快」由上面 `elapsed <
    // SHUTDOWN_CEILING_SEC` 那条单独钉。实测这一段收尾只要 0.1–0.4ms，压 5s 预算并不能
    // 多验出什么，反而会在 `cargo test --workspace` 那种上百条测试抢 CPU 的场合被瞬时
    // 饿死撞红（撞到过一次：三条同时报「5s 内 run_app 没有返回」，而单独跑是 0.15s）。
    let mut run = RunningApp::start(app.clone(), &platform, Some(grace)).await;
    run.shutdown().await.expect("run_app 该正常收场");

    assert!(
        platform.inner.stopped(),
        "grace={grace}：platform.stop() 没走到"
    );
    assert_eq!(
        sandbox.close_calls(),
        1,
        "grace={grace}：sandbox.close_all() 没走到 —— 容器没还"
    );
    assert!(
        app.store.get_task("whatever").await.is_err(),
        "grace={grace}：store.close() 没走到 —— 退出序列的最后一步被跳过了"
    );
}

/// `NaN`：旧实现靠 `NaN.max(0.0) == 0.0` 侥幸不炸，等于「一秒都不等」，没人说得清是有意的。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nan_grace_still_runs_the_whole_shutdown() {
    shutdown_runs_to_the_end_with_grace(f64::NAN).await;
}

/// `f64::INFINITY`：旧实现在这里当场 panic，收尾从中间断掉。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_infinite_grace_still_runs_the_whole_shutdown() {
    shutdown_runs_to_the_end_with_grace(f64::INFINITY).await;
}

/// `1e300`：有限数，`max(0.0)` 原样放行，到 `Duration` 那里上溢 panic ——
/// 台账点名的正是这一个（`--grace 1e300` 曾一路穿过 CLI 校验）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_huge_finite_grace_still_runs_the_whole_shutdown() {
    shutdown_runs_to_the_end_with_grace(1e300).await;
}

/// 负数：旧实现被 `max(0.0)` 悄悄改写成 0；现在按「非法取值」退到默认 20s 并 warn 一行。
/// 两种结局在这条用例里都不影响判据 —— 队列是空的，收尾照样必须整段走完。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_negative_grace_still_runs_the_whole_shutdown() {
    shutdown_runs_to_the_end_with_grace(-1.0).await;
}
