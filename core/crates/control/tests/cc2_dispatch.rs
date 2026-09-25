//! CC2 ②：按会话串行、跨会话至多 N 个（`with_max_parallel(n)`，默认 1）；
//! ④ 后半：卡死任务被下一条消息替换（跑在默认 1 上）。
mod support;

use std::time::Duration;

use aite_contracts::{ControlPlane, EvidenceKind, SessionStore, TaskStatus};
use aite_control::{STUCK_AFTER_SEC, stuck_task_replaced_text};
use support::{
    CHAT, Harness, ROOT, RunningPlane, ScriptedWorker, SettableWallClock, WorkerAction,
    active_tasks, ev, within,
};

/// 等到 `cond()` 为真（5 秒超时，给出人话）。
async fn wait_for(what: &str, cond: impl Fn() -> bool) {
    within(what, async {
        while !cond() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}

/// 给派发循环足够的机会去领下一个任务 —— 该领却没领，这段时间里就会领走。
async fn let_dispatch_run() {
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// 两个群各一个 @ 任务（两个会话）。
async fn two_chats(plane: &aite_control::InProcessControlPlane) {
    plane
        .handle_event(
            ev().id("e_a")
                .chat("oc_a")
                .message_id("om_a")
                .text("群 A 的问题")
                .build(),
        )
        .await
        .expect("建任务 A");
    plane
        .handle_event(
            ev().id("e_b")
                .chat("oc_b")
                .message_id("om_b")
                .text("群 B 的问题")
                .build(),
        )
        .await
        .expect("建任务 B");
}

#[tokio::test]
async fn default_max_parallel_is_strictly_serial() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::WaitForCancel, WorkerAction::WaitForCancel],
    );
    // 不调 max_parallel：默认 1
    let plane = h.plane_builder().worker(worker.clone()).build();
    two_chats(&plane).await;

    let _running = RunningPlane::start(plane.clone());
    wait_for("第一个任务被领走", || !worker.calls().is_empty()).await;
    let_dispatch_run().await;

    assert_eq!(
        worker.calls().len(),
        1,
        "默认全局串行：worker 卡住第一个时，另一个群的任务不许被领走"
    );
    assert_eq!(plane.pending(), 1, "第二个还在队里");
}

#[tokio::test]
async fn two_chats_dispatch_concurrently_with_max_parallel_2() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::WaitForCancel, WorkerAction::WaitForCancel],
    );
    let plane = h
        .plane_builder()
        .worker(worker.clone())
        .max_parallel(2)
        .build();
    two_chats(&plane).await;

    let _running = RunningPlane::start(plane.clone());
    wait_for("两个群的任务同时在 worker 手上", || {
        worker.calls().len() == 2
    })
    .await;
    assert_eq!(plane.pending(), 0);
    assert_eq!(plane.state().running.len(), 2, "两个都在跑");
}

#[tokio::test]
async fn same_session_stays_serial() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![
            WorkerAction::DeliverThenHold("第一个答完了".into()),
            WorkerAction::Deliver("第二个答完了".into()),
        ],
    );
    let plane = h
        .plane_builder()
        .worker(worker.clone())
        .max_parallel(2)
        .build();

    plane
        .handle_event(ev().text("第一个问题").build())
        .await
        .expect("建任务 A");
    let first = active_tasks(&h.store, CHAT).await.remove(0);

    let _running = RunningPlane::start(plane.clone());
    // A 交付了（出了活跃口径）但还占着 worker
    wait_for("A 已交付", || {
        h.platform.texts().iter().any(|t| t.text == "第一个答完了")
    })
    .await;

    // 同话题的追问：A 不在活跃口径里 → 在**同一个会话**里新建任务 B
    plane
        .handle_event(
            ev().id("e2")
                .message_id("om_2")
                .thread(ROOT)
                .mentioned(false)
                .text("再追问一句")
                .build(),
        )
        .await
        .expect("建任务 B");
    let second = active_tasks(&h.store, CHAT).await.remove(0);
    assert_eq!(second.session_id, first.session_id, "B 与 A 同一个会话");

    let_dispatch_run().await;
    assert_eq!(
        worker.calls(),
        vec![first.id.clone()],
        "n=2 也不许同一个会话里两个任务同时在跑"
    );
    assert_eq!(plane.pending(), 1);

    // 放掉 A → B 随即被领走
    plane.cancel_task(first.clone(), None, None, false).await;
    wait_for("A 收尾后 B 开跑", || worker.calls().len() == 2).await;
    within("join", plane.join()).await;
    let saved = h.store.get_task(&second.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Delivered);
    assert_eq!(worker.calls(), vec![first.id, second.id]);
}

/// 对照 `dispatch.rs` 的 abort 那条：并发派发下在飞的任务是 `run_forever` 自己的子 future，
/// abort 之后它们一起被丢掉、guard 照常收尾，`join()` 立刻返回。
#[tokio::test]
async fn aborting_run_forever_with_max_parallel_2_lets_join_return() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::WaitForCancel, WorkerAction::WaitForCancel],
    );
    let plane = h
        .plane_builder()
        .worker(worker.clone())
        .max_parallel(2)
        .build();
    two_chats(&plane).await;

    let running = RunningPlane::start(plane.clone());
    wait_for("两个都在跑", || worker.calls().len() == 2).await;
    drop(running);

    within("abort 之后 join 立刻返回", plane.join()).await;
    wait_for("running 清空", || plane.state().running.is_empty()).await;
    assert!(plane.state().owned.is_empty(), "owned 也跟着清掉");
}

/// ④ 后半：卡死的在跑任务被这个会话的下一条消息替换。**不调 `with_max_parallel`**（默认 1，
/// 全局串行）—— 光 `cancel_task` 没用，worker 卡在一步里看不到取消标志，新任务会永远排在它后面。
#[tokio::test]
async fn stuck_task_is_replaced_on_next_message_with_notice() {
    let h = Harness::new();
    let clock = SettableWallClock::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![
            WorkerAction::Hang,
            WorkerAction::Deliver("重新做完了".into()),
        ],
    );
    let plane = h
        .plane_builder()
        .worker(worker.clone())
        .clock(clock.as_wall())
        .build();

    plane
        .handle_event(ev().text("做个很慢的活").build())
        .await
        .expect("建任务 A");
    let stuck = active_tasks(&h.store, CHAT).await.remove(0);
    let _running = RunningPlane::start(plane.clone());
    wait_for("A 被 worker 领走", || !worker.calls().is_empty()).await;

    // 还没到阈值：照常是 steer，不替换
    plane
        .handle_event(
            ev().id("e2")
                .message_id("om_2")
                .thread(ROOT)
                .mentioned(false)
                .text("进展如何？")
                .build(),
        )
        .await
        .expect("追问");
    assert_eq!(plane.counter("events.steer"), 1);
    assert_eq!(plane.counter("control.stuck_replaced"), 0);

    // 过了阈值：下一条消息替换它
    clock.advance_sec(STUCK_AFTER_SEC + 60);
    plane
        .handle_event(
            ev().id("e3")
                .message_id("om_3")
                .thread(ROOT)
                .mentioned(false)
                .text("卡住了就重来")
                .build(),
        )
        .await
        .expect("替换");

    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        stuck_task_replaced_text(&stuck.task_no, 15)
    );
    assert_eq!(
        stuck_task_replaced_text("#A7", 15),
        "任务 #A7 超过 15 分钟没有进展，已停止，按这条消息重新开始。"
    );
    let old = h.store.get_task(&stuck.id).await.expect("读").expect("有");
    assert_eq!(old.status, TaskStatus::Cancelled);
    assert_eq!(
        h.evidence.kinds(&stuck.id).last(),
        Some(&EvidenceKind::Cancelled),
        "不在跑了：控制面自己写 cancelled 并 finalize"
    );
    assert!(
        h.evidence.manifest(&stuck.id).is_some(),
        "证据链已 finalize"
    );

    // 新任务在同一个会话里接着跑，默认 1 下也不再被卡死的那个挡住
    within("join", plane.join()).await;
    let calls = worker.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], stuck.id);
    let fresh = h.store.get_task(&calls[1]).await.expect("读").expect("有");
    assert_eq!(fresh.session_id, stuck.session_id);
    assert_eq!(fresh.title, "卡住了就重来");
    assert_eq!(fresh.status, TaskStatus::Delivered);
    assert_eq!(plane.counter("control.stuck_replaced"), 1);
}
