//! `run_forever`：派发队列里的任务给 worker，并跑沙箱 reaper（W7：每 60s 一次）。
//! 对应 `tests/control/test_dispatch.py`（5 条，其中 `run_pending` 那条参数化 ×3）。
mod support;

use aite_contracts::{ControlPlane, SessionStore, TaskStatus};
use aite_control::REAPER_INTERVAL_SEC;
use support::{
    CHAT, Harness, ParkedSleep, RunningPlane, ScriptedWorker, WorkerAction, active_tasks, ev,
    within,
};

#[tokio::test]
async fn run_forever_dispatches_queued_tasks() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::Deliver("答完了".into())],
    );
    let sleeper = ParkedSleep::new(1);
    let plane = h
        .plane_builder()
        .worker(worker.clone())
        .sleep(sleeper.as_sleep())
        .build();

    plane
        .handle_event(ev().text("一个问题").build())
        .await
        .expect("建任务");
    let task_id = active_tasks(&h.store, CHAT).await.remove(0).id;

    let _running = RunningPlane::start(plane.clone());
    within("join", plane.join()).await;

    let saved = h.store.get_task(&task_id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Delivered);
    assert_eq!(h.platform.last_text().expect("该回帖").text, "答完了");
    assert_eq!(worker.calls(), vec![task_id]);
}

#[tokio::test]
async fn reaper_calls_reap_idle_on_the_configured_cadence() {
    let h = Harness::new();
    h.sandbox.set_reap_returns(&["sb_1"]);
    let sleeper = ParkedSleep::new(3); // 转 2 圈后停住
    let plane = h.plane_builder().sleep(sleeper.as_sleep()).build();

    let _running = RunningPlane::start(plane.clone());
    within("reaper 转两圈", sleeper.wait_until_parked()).await;

    assert_eq!(
        &sleeper.calls()[..2],
        &[REAPER_INTERVAL_SEC, REAPER_INTERVAL_SEC]
    );
    assert_eq!(&sleeper.calls()[..2], &[60.0, 60.0]);
    assert_eq!(h.sandbox.reap_calls(), vec![h.config.sandbox.idle_sec; 2]);
    assert_eq!(h.sandbox.reap_calls(), vec![300, 300]);
    assert_eq!(plane.counter("sandbox.reaped"), 2);
}

#[tokio::test]
async fn reaper_survives_a_failing_sandbox() {
    let h = Harness::new();
    h.sandbox.set_reap_fails(true);
    let sleeper = ParkedSleep::new(3);
    let plane = h.plane_builder().sleep(sleeper.as_sleep()).build();

    let _running = RunningPlane::start(plane.clone());
    within("reaper 转两圈", sleeper.wait_until_parked()).await;

    assert_eq!(sleeper.calls().len(), 3, "抛异常没打断循环");
    assert_eq!(h.sandbox.reap_calls().len(), 2);
    assert_eq!(plane.counter("sandbox.reaped"), 0);
}

/// §3.3：任何未捕获异常 → task failed + 回帖，进程不退出。第一个炸了第二个照跑。
#[tokio::test]
async fn dispatch_failure_does_not_kill_the_loop() {
    let h = Harness::new();
    h.platform.fail_send_text(1); // 交付第一个任务时平台抽风
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![
            WorkerAction::Deliver("第一份结果".into()),
            WorkerAction::Deliver("第二份结果".into()),
        ],
    );
    let sleeper = ParkedSleep::new(1);
    let plane = h
        .plane_builder()
        .worker(worker)
        .sleep(sleeper.as_sleep())
        .build();

    plane
        .handle_event(ev().id("e1").text("会炸的").message_id("om_1").build())
        .await
        .expect("建第一个");
    let first_id = active_tasks(&h.store, CHAT).await.remove(0).id;

    let _running = RunningPlane::start(plane.clone());
    within("第一轮 join", plane.join()).await;
    let first = h.store.get_task(&first_id).await.expect("读").expect("有");
    assert_eq!(first.status, TaskStatus::Failed);

    plane
        .handle_event(ev().id("e2").text("第二件事").message_id("om_2").build())
        .await
        .expect("建第二个");
    let second_id = active_tasks(&h.store, CHAT).await.remove(0).id;
    within("第二轮 join", plane.join()).await;

    let second = h.store.get_task(&second_id).await.expect("读").expect("有");
    assert_eq!(second.status, TaskStatus::Delivered);
    assert_eq!(h.platform.last_text().expect("该回帖").text, "第二份结果");
}

/// 派发时存储抖了一下：那一轮什么都没做，但循环还在，下一个照跑。
///
/// Rust 侧的补充用例：`TaskWorker::run` 契约上不外抛（失败收敛成 failed），所以
/// 「派发半路炸掉」在 Rust 里只可能来自 store/evidence。Python 那条走的是平台抽风，
/// 两条一起才盖住 `run_forever` 的 `except Exception` 分支。
#[tokio::test]
async fn dispatch_survives_a_failing_store() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::Deliver("第二份结果".into())],
    );
    let sleeper = ParkedSleep::new(1);
    let plane = h
        .plane_builder()
        .worker(worker.clone())
        .sleep(sleeper.as_sleep())
        .build();

    plane
        .handle_event(ev().id("e1").text("第一件事").message_id("om_1").build())
        .await
        .expect("建第一个");
    let first_id = active_tasks(&h.store, CHAT).await.remove(0).id;
    h.store.fail_next("get_task", 1); // 派发它的时候库抖一下

    let _running = RunningPlane::start(plane.clone());
    within("第一轮 join", plane.join()).await;
    assert!(worker.calls().is_empty(), "get_task 炸了就轮不到 worker");
    // 收尾照常：steer / owned 都清干净了
    assert!(plane.state().owned.is_empty());

    plane
        .handle_event(ev().id("e2").text("第二件事").message_id("om_2").build())
        .await
        .expect("建第二个");
    // 第一个还挂在 active 上（没人给它收尾），所以得按 id 挑出第二个
    let second_id = active_tasks(&h.store, CHAT)
        .await
        .into_iter()
        .map(|t| t.id)
        .find(|id| id != &first_id)
        .expect("第二个任务");
    within("第二轮 join", plane.join()).await;

    assert_eq!(worker.calls(), vec![second_id.clone()]);
    let second = h.store.get_task(&second_id).await.expect("读").expect("有");
    assert_eq!(second.status, TaskStatus::Delivered);
    // 第一个任务原样留在库里（没人给它收尾 —— 这正是 §9 第 7 条说的那种残局）
    let first = h.store.get_task(&first_id).await.expect("读").expect("有");
    assert_eq!(first.status, TaskStatus::Created);
}

/// 没配 worker 的控制面：`run_pending` 不该抛，只是什么都不做。
#[tokio::test]
async fn run_pending_drains_without_a_worker() {
    for qsize in [0usize, 1, 3] {
        let h = Harness::new();
        let plane = h.plane();
        for i in 0..qsize {
            plane
                .handle_event(
                    ev().id(&format!("e{i}"))
                        .text("活儿")
                        .message_id(&format!("om_{i}"))
                        .build(),
                )
                .await
                .expect("建任务");
        }
        assert_eq!(plane.pending(), qsize, "qsize={qsize}");
        plane.run_pending().await;
        assert_eq!(plane.pending(), 0, "qsize={qsize}");
    }
}

/// 控制面交给 worker 的那两个回调是不是真接上了（`RunHooks`，D4）。
///
/// Rust 侧的补充用例：Python 里这两个闭包由 `tests/worker/test_steer.py` 从 worker 那头验，
/// 而 Rust 的 `RunHooks` 是在控制面这边现造的，构造错了（比如闭包捕错 task_id）
/// 从 worker 那头看不出来。
#[tokio::test]
async fn run_hooks_are_wired_to_this_task() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::DrainThenDeliver("画好了".into())],
    );
    let plane = h.plane_builder().worker(worker.clone()).build();

    plane
        .handle_event(ev().text("按月画个图").build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);
    // 排队期间的 steer 会在开跑前被清掉（它已经在 transcript 里），所以这里直接塞给
    // 「在跑的那个」测不了。改成：跑起来之前先排一条，验 `_dispatch_task` 的清理；
    // 再验 drain_steer 拿到的是空表（而不是别的任务的）。
    plane
        .handle_event(
            ev().id("ev2")
                .text("顺便加上同比")
                .mentioned(false)
                .message_id("om_2")
                .thread(support::ROOT)
                .build(),
        )
        .await
        .expect("追问");
    assert_eq!(plane.pending_steer(&task.id).len(), 1);

    plane.run_pending().await;

    assert_eq!(worker.calls(), vec![task.id.clone()]);
    assert_eq!(
        worker.drained(),
        vec![Vec::<String>::new()],
        "开跑前那条 steer 该已经被 `_dispatch_task` 清掉"
    );
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Delivered);
}

/// `is_cancelled` 在 `!stop` 之后为真 —— 但只有任务已经在 worker 手上时才走得到；
/// 还在队列里就被停掉的会在 `_dispatch_task` 的第四条提前 return 上被拦下。
#[tokio::test]
async fn cancel_before_dispatch_never_reaches_the_worker() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::CancelAwareDeliver("不该跑到这".into())],
    );
    let plane = h.plane_builder().worker(worker.clone()).build();

    plane
        .handle_event(ev().text("按月画个图").build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);
    plane
        .handle_event(
            ev().id("ev-stop")
                .text("!stop")
                .mentioned(false)
                .message_id("om_3")
                .thread(support::ROOT)
                .build(),
        )
        .await
        .expect("停掉");

    plane.run_pending().await;

    assert!(worker.calls().is_empty(), "已取消的任务不该被领走");
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
}
