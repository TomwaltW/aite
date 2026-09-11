//! R6 排队侧的边界：steer 排给谁、什么时候不排、任务没跑起来时队列谁来清。
//! 对应 `tests/control/test_steer_routing.py`（6 条）。
//!
//! `tests/routing.rs` 只验了 R6 的主干（话题内追问 → 排成 steer / 新建 task）。这一份补的是
//! 主干外面那几条 —— 都是读实现时看得见、但没人验过的地方：
//!
//! - 队列只在内存里（`InProcessControlPlane` 的 `_steer`），进程一换就没了；
//! - 换进程之后，库里还挂在 `working` 上的旧任务没人会再领，追问排给它 = 扔了；
//! - 任务压根没被 worker 领走（已取消 / 没配 worker）时，队列谁来清。
//!
//! 「换进程」在这里 = **同一个 store 换一个控制面实例**（Python 那边是同一个 .db 换一个
//! `InProcessControlPlane`，一回事）。白盒那条 `_steer_target` 在 `src/plane.rs` 的
//! `#[cfg(test)]` 里（拆成 3 条：优先在跑的 / 退回最后创建的 / 孤儿一个都不选）。
mod support;

use aite_contracts::{ControlPlane, SessionStore, TaskStatus};
use support::{CHAT, Harness, ROOT, ScriptedWorker, active_tasks, ev, turn_texts};

async fn steer_once(
    plane: &std::sync::Arc<aite_control::InProcessControlPlane>,
    text: &str,
    event_id: &str,
    message_id: &str,
) {
    plane
        .handle_event(
            ev().id(event_id)
                .text(text)
                .mentioned(false)
                .message_id(message_id)
                .thread(ROOT)
                .build(),
        )
        .await
        .expect("追问不该报错");
}

// ---- ① 队列只在内存里 -----------------------------------------------------

/// **当前边界，不是缺陷判定**：`append_turn` 那一半落库，排队那一半不落。
///
/// §2.4 M6 要的是「杀进程重启后在旧线程追问仍能续接」—— 会话锚点和 transcript 都在库里，
/// 那一条成立。丢的是「这句话本该被合并进正在跑的那个任务」这件事。要不要把队列也落库
/// 涉及改 §3.2 `SessionStore` 契约，那是 R0/总管的地盘，本轨只钉事实。
#[tokio::test]
async fn pending_steer_does_not_survive_a_restart() {
    let h = Harness::new();
    let plane1 = h.plane();
    plane1
        .handle_event(ev().id("ev1").text("第一问").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);
    steer_once(&plane1, "顺便加上同比", "ev2", "om_2").await;
    assert_eq!(
        plane1.pending_steer(&task.id),
        vec!["顺便加上同比".to_string()],
        "排在内存里"
    );

    // 换实例 = 换进程（库还是那一个）
    let plane2 = h.plane();
    assert!(plane2.pending_steer(&task.id).is_empty(), "队列全丢");

    // 落库的那一半还在：用户说过的话在 transcript 里，下一个任务的上下文能读到它
    assert_eq!(
        turn_texts(&h.store, &task.session_id).await,
        vec!["第一问", "顺便加上同比"]
    );
}

/// 杀进程时在跑的任务，库里还是 `working`，但本进程谁也不会去领它。
///
/// 改动前（T14 之前）：R6 看到「本会话有活跃 task」→ 把追问排进孤儿的队列，那条队列永远
/// 没人 drain，用户看到的就是 Aite 一点反应都没有 —— M6 直接红。
/// 改动后：孤儿不算 steer 目标，走 R6 原文的「否则新建 task 继续」。
#[tokio::test]
async fn followup_after_a_restart_starts_a_new_task_instead_of_feeding_an_orphan() {
    let h = Harness::new();
    let plane1 = h.plane();
    plane1
        .handle_event(ev().id("ev1").text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let mut orphan = active_tasks(&h.store, CHAT).await.remove(0);
    orphan.status = TaskStatus::Working; // 被杀的那一刻正在跑
    h.store.update_task(&orphan).await.expect("落 working");

    let plane2 = h.plane();
    steer_once(&plane2, "再按季度画一张", "ev2", "om_2").await;

    let active = active_tasks(&h.store, CHAT).await;
    let fresh: Vec<&aite_contracts::Task> = active.iter().filter(|t| t.id != orphan.id).collect();
    assert_eq!(fresh.len(), 1, "追问应当新建一个 task，而不是排给孤儿");
    assert_eq!(fresh[0].session_id, orphan.session_id, "R6：续接同一会话");
    assert_eq!(plane2.counter("events.orphan_task"), 1);
    assert_eq!(plane2.counter("events.steer"), 0);
    assert!(plane2.pending_steer(&orphan.id).is_empty());
    assert!(plane2.state().steer.is_empty(), "也没留下空条目");

    // 残留（报给总管）：孤儿本身还挂在 active 上，`!status` 里看得见，也没人给它收尾。
    assert!(active.iter().any(|t| t.id == orphan.id));
}

// ---- ③ 任务没被领走时，队列谁来清 -----------------------------------------

/// `!stop` 掉一个还在队列里排着的任务：`_dispatch_task` 在「已取消」那条提前 return，
/// worker 那圈 finally 压根不会执行。清理不放在最外层的话，`_steer` 里就永远留着这一条
/// —— 内存泄漏，用户那句追问也石沉大海。
#[tokio::test]
async fn stop_on_a_still_queued_task_clears_the_steer_queue() {
    let h = Harness::new();
    // 空脚本的 worker：被调用就说明「已取消的任务被领走了」
    let worker = ScriptedWorker::idle(h.store.clone(), h.platform.clone());
    let plane = h.plane_builder().worker(worker.clone()).build();

    plane
        .handle_event(ev().text("按月画个图").build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);
    steer_once(&plane, "顺便加上同比", "ev2", "om_2").await;
    assert_eq!(
        plane.pending_steer(&task.id),
        vec!["顺便加上同比".to_string()]
    );

    plane
        .handle_event(
            ev().id("ev3")
                .text("!stop")
                .mentioned(false)
                .message_id("om_3")
                .thread(ROOT)
                .build(),
        )
        .await
        .expect("停掉");
    plane.run_pending().await;

    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
    assert!(worker.calls().is_empty());
    assert!(plane.pending_steer(&task.id).is_empty());
    let state = plane.state();
    assert!(
        state.steer.is_empty(),
        "连空条目都不许留：{:?}",
        state.steer
    );
    assert!(state.owned.is_empty());
    // !stop 就是让它停，这句追问不再自动变成新任务；但它在 transcript 里，
    // 下一次话题内追问会把它带进新任务的上下文。
    assert_eq!(
        turn_texts(&h.store, &task.session_id).await,
        vec!["按月画个图", "顺便加上同比"]
    );
}

/// 另一条提前 return：没配 worker（路由测试的默认装配）也要清干净。
#[tokio::test]
async fn queue_is_cleared_even_when_no_worker_is_configured() {
    let h = Harness::new();
    let plane = h.plane();

    plane
        .handle_event(ev().text("按月画个图").build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);
    steer_once(&plane, "顺便加上同比", "ev2", "om_2").await;
    assert_eq!(
        plane.pending_steer(&task.id),
        vec!["顺便加上同比".to_string()]
    );

    plane.run_pending().await;

    let state = plane.state();
    assert!(state.steer.is_empty());
    assert!(state.owned.is_empty());
}

/// R6 的「否则新建 task 继续」：任务落终态之后到达的追问不该排队，也不该给死掉的
/// task_id 留下一个永远没人 drain 的条目（Python 那边 `_steer` 是 `defaultdict`，
/// 读一下就会凭空长出一条；Rust 用 `HashMap` + `entry().or_default()`，只在真排队时才建）。
#[tokio::test]
async fn steer_arriving_after_delivery_becomes_a_new_task() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("第一问").build())
        .await
        .expect("建任务");
    let mut first = active_tasks(&h.store, CHAT).await.remove(0);
    first.status = TaskStatus::Delivered;
    h.store.update_task(&first).await.expect("落终态");

    steer_once(&plane, "再按季度画一张", "ev2", "om_2").await;

    let active = active_tasks(&h.store, CHAT).await;
    assert_eq!(active.len(), 1);
    assert_ne!(active[0].id, first.id);
    assert_eq!(active[0].session_id, first.session_id);
    assert_eq!(plane.counter("events.steer"), 0);
    assert!(plane.state().steer.is_empty());
    assert!(plane.pending_steer(&first.id).is_empty());
}
