//! `cancel_task` 的双分支（`inventory-core.md` §2 末段）。
//!
//! Rust 侧的补充用例。Python 那边这两条分支散在 `test_commands.py`（不在跑）与
//! `tests/integration/test_t8_*.py`（在跑，收尾时的 stranded）里，后者归 RΩ；
//! 但分支本身是控制面写的，出错要在这一轨看得见：
//!
//! ```text
//! 不在跑 → 还 Gateway 沙箱 + cancelled 证据 + 卡片置 cancelled + finalize（manifest 收口）
//! 在跑   → 一个字都不写（worker 下一步开头自己收尾，那边拿到的步数才是准的，
//!          也免得同一条链上出现两条 cancelled）
//! ```
mod support;

use aite_contracts::{CardStatus, ControlPlane, EvidenceKind, SessionStore, TaskStatus};
use support::{
    CHAT, Harness, ParkedSleep, RunningPlane, ScriptedWorker, WorkerAction, active_tasks, ev,
    within,
};

/// 不在跑的那一支：四件事一件不少。
#[tokio::test]
async fn cancelling_an_idle_task_writes_everything() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("跑个长活").build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.card_id = Some("om_card_1".into());
    task.steps = 7;
    h.store.update_task(&task).await.expect("回写");
    let task = h.store.get_task(&task.id).await.expect("读").expect("有");

    let out = plane
        .cancel_task(task.clone(), Some("om_9".into()), Some(CHAT.into()), true)
        .await;

    assert_eq!(out.status, TaskStatus::Cancelled);
    // ① Gateway 手上的沙箱还了
    assert_eq!(h.gateway.released(), vec![task.id.clone()]);
    // ② 链上一条 cancelled，payload 是 {by: "stop", steps}
    let cancelled: Vec<_> = h
        .evidence
        .events(&task.id)
        .into_iter()
        .filter(|e| e.kind == EvidenceKind::Cancelled)
        .collect();
    assert_eq!(cancelled.len(), 1);
    let payload = cancelled[0].payload.clone().expect("有 payload");
    assert_eq!(payload.get("by").and_then(|v| v.as_str()), Some("stop"));
    assert_eq!(payload.get("steps").and_then(|v| v.as_i64()), Some(7));
    // ③ 卡片置 cancelled
    let (card_id, card) = h.platform.card_updates().pop().expect("该更新卡片");
    assert_eq!(card_id, "om_card_1");
    assert_eq!(card.status, CardStatus::Cancelled);
    assert_eq!(card.footer, "已用 7 步 · ¥0.00");
    assert_eq!(
        card.initiator, "张三",
        "initiator_name 从 config_snapshot 取"
    );
    // ④ manifest 收口，root_hash 进库
    let manifest = h.evidence.manifest(&task.id).expect("该有 manifest");
    assert_eq!(
        manifest.get("task_no").and_then(|v| v.as_str()),
        Some(task.task_no.as_str())
    );
    assert_eq!(
        manifest.get("model").and_then(|v| v.as_str()),
        Some("scripted-p0"),
        "task.model 非空时用它"
    );
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(
        saved.evidence_root_hash.as_deref(),
        Some(
            manifest
                .get("root_hash")
                .and_then(|v| v.as_str())
                .expect("root_hash")
        )
    );
    // 回帖
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        format!("任务 {} 已停止。", task.task_no)
    );
}

/// `task.model` 空时退回 `ControlDeps::model_name`（Python 的 `getattr(model, "name", "")`）。
#[tokio::test]
async fn manifest_model_falls_back_to_the_planes_model_name() {
    let h = Harness::new();
    let plane = h.plane_builder().model_name("退路模型").build();
    plane
        .handle_event(ev().text("跑个数").build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.model = String::new(); // 空配置下建的任务
    h.store.update_task(&task).await.expect("回写");
    let task = h.store.get_task(&task.id).await.expect("读").expect("有");

    plane.cancel_task(task.clone(), None, None, false).await;

    let manifest = h.evidence.manifest(&task.id).expect("该有 manifest");
    assert_eq!(
        manifest.get("model").and_then(|v| v.as_str()),
        Some("退路模型")
    );
}

/// `finalize` 每个任务只走一次：worker 收过尾的（`evidence_root_hash` 有值）不再算第二遍。
#[tokio::test]
async fn finalize_is_guarded_by_evidence_root_hash() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("跑个数").build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.evidence_root_hash = Some("worker 早就收过尾了".into());
    h.store.update_task(&task).await.expect("回写");
    let task = h.store.get_task(&task.id).await.expect("读").expect("有");

    let out = plane.cancel_task(task.clone(), None, None, false).await;

    assert!(
        h.evidence.manifest(&task.id).is_none(),
        "不该再 finalize 一遍"
    );
    assert_eq!(
        out.evidence_root_hash.as_deref(),
        Some("worker 早就收过尾了")
    );
}

/// `notify=false` → 一个字都不回（`!restart` 逐个取消时走的就是这条）。
#[tokio::test]
async fn cancelling_quietly_says_nothing() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("跑个数").build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    plane.cancel_task(task, None, None, false).await;

    assert!(h.platform.texts().is_empty());
}

/// 在跑的那一支：证据、卡片、manifest 一件都不写，全交给 worker 下一步开头。
#[tokio::test]
async fn cancelling_a_running_task_writes_nothing() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::WaitForCancel],
    );
    let sleeper = ParkedSleep::new(1);
    let plane = h
        .plane_builder()
        .worker(worker.clone())
        .sleep(sleeper.as_sleep())
        .build();

    plane
        .handle_event(ev().text("跑个长活").build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.card_id = Some("om_card_1".into());
    task.sandbox_id = Some("sb_x".into());
    h.store.update_task(&task).await.expect("回写");
    let task = h.store.get_task(&task.id).await.expect("读").expect("有");

    let _running = RunningPlane::start(plane.clone());
    // 等任务真的进了 worker 的手
    within("等 worker 接手", async {
        while worker.calls().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(plane.state().running, vec![task.id.clone()]);

    let evidence_before = h.evidence.events(&task.id).len();
    let out = plane
        .cancel_task(task.clone(), Some("om_9".into()), Some(CHAT.into()), true)
        .await;
    within("等 worker 自己收场", plane.join()).await;

    assert_eq!(out.status, TaskStatus::Cancelled);
    // 在跑的分支：不写证据、不动卡片、不 finalize、不问 Gateway 要沙箱
    assert_eq!(
        h.evidence.events(&task.id).len(),
        evidence_before,
        "在跑的任务不该在这里多写证据"
    );
    assert!(h.platform.card_updates().is_empty());
    assert!(h.evidence.manifest(&task.id).is_none());
    assert!(h.gateway.released().is_empty());
    // 但 task.sandbox_id 记的那个（worker 兜底建的）照样还
    assert_eq!(h.sandbox.released(), vec!["sb_x".to_string()]);
    // 回帖照发
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        format!("任务 {} 已停止。", task.task_no)
    );
    // worker 那头看到了 is_cancelled，自己落的终态
    assert!(worker.cancel_checks().contains(&true));
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
}

// --------------------------------------------------------------------------
// 过期快照那一支（审核记账 4.1 第 2 行，方向乙）
// --------------------------------------------------------------------------

/// 拿一份**过期快照**来取消：库里那条已经收场了，就一个字都不许改。
///
/// 真会撞上的是收尾那一支。`app` 的 `shutdown()` 在宽限期超时时先抄一份
/// `worker.in_flight()`，抄到 `runner.abort()` 生效之间，那个任务完全可能已经在另一条
/// task 上把自己跑完了（落 delivered、写 delivered 证据、finalize、收卡片、回写 in_flight）。
/// 手上那份快照此后就是过期的 —— 它停在 `deliver()` 开头那次 `save()` 写的 working 上。
///
/// 没有这道防线的话，四件事一起发生：库里 delivered 被改写成 cancelled、
/// 卡片从「已交付」翻成「已取消」、链上多一条 cancelled 证据、manifest 再 finalize 一遍
/// （幂等判据读的是**快照里**的 evidence_root_hash，那一份还是空的）。
/// 而用户手上的答复和文件早就收到了。
#[tokio::test]
async fn a_stale_snapshot_never_rewrites_a_task_that_already_finished() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("跑个数").build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.card_id = Some("om_card_1".into());
    // `deliver()` 开头那次 save 落的就是 working（answering 那一路落 answering）
    task.status = TaskStatus::Working;
    h.store.update_task(&task).await.expect("回写");

    // ① 收尾抄下来的那一份：还停在 working，root_hash 是空的
    let stale = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(stale.status, TaskStatus::Working);
    assert!(stale.evidence_root_hash.is_none());

    // ② 抄完之后 worker 自己跑完了：落 delivered + finalize
    let mut done = stale.clone();
    done.status = TaskStatus::Delivered;
    done.result_summary = "答复和文件都发出去了".into();
    done.evidence_root_hash = Some("worker 自己 finalize 过了".into());
    h.store.update_task(&done).await.expect("回写 delivered");

    // ③ 收尾拿着过期快照来取消
    let out = plane.cancel_task(stale.clone(), None, None, false).await;

    // 返回的是库里那份，不是被改写过的快照
    assert_eq!(
        out.status,
        TaskStatus::Delivered,
        "取消不许改写一条已经收场的任务"
    );
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(
        saved.status,
        TaskStatus::Delivered,
        "库里那条必须还是 delivered"
    );
    assert_eq!(saved.result_summary, "答复和文件都发出去了");
    assert_eq!(
        saved.evidence_root_hash.as_deref(),
        Some("worker 自己 finalize 过了")
    );
    // 证据链上不许多一条 cancelled
    assert!(
        h.evidence
            .events(&task.id)
            .iter()
            .all(|e| e.kind != EvidenceKind::Cancelled),
        "已经交付的任务不该再写一条 cancelled 证据"
    );
    // manifest 不许有第二遍（幂等判据不能读过期快照）
    assert!(
        h.evidence.manifest(&task.id).is_none(),
        "worker 已经 finalize 过了，不该在这里再算一遍"
    );
    // 用户看到的卡片不许从「已交付」翻成「已取消」
    assert!(
        h.platform.card_updates().is_empty(),
        "不该动卡片：{:?}",
        h.platform.card_updates()
    );
    // 沙箱也不用再还一次（worker 的 finish() 已经还过）
    assert!(h.gateway.released().is_empty());
    assert!(h.sandbox.released().is_empty());
}

/// 同一条路上的对照组：库里**还没**收场时，取消照常走完四件事。
///
/// 防线只该拦终态，不该把「宽限期真的没收完、必须硬取消」那条正路也一起挡了。
#[tokio::test]
async fn a_snapshot_of_a_still_running_task_is_cancelled_as_usual() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("跑个数").build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.card_id = Some("om_card_1".into());
    h.store.update_task(&task).await.expect("回写");
    let task = h.store.get_task(&task.id).await.expect("读").expect("有");

    let out = plane.cancel_task(task.clone(), None, None, false).await;

    assert_eq!(out.status, TaskStatus::Cancelled);
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
    assert_eq!(
        h.evidence.kinds(&task.id).last(),
        Some(&EvidenceKind::Cancelled)
    );
    assert!(h.evidence.manifest(&task.id).is_some());
    assert_eq!(h.platform.card_updates().len(), 1);
}
