//! T2 独有验收：`!status` / `!stop` / `!restart` / `!new` 四条命令（§3.5 R5 + §3.3）。
//! 对应 `tests/control/test_commands.py`（10 条）。
mod support;

use std::sync::Arc;

use aite_contracts::{CardStatus, ControlPlane, SessionStatus, SessionStore, TaskStatus};
use support::{CHAT, Harness, ROOT, active_tasks, ev, turn_texts};

use aite_control::{InProcessControlPlane, UNKNOWN_COMMAND_TEXT};

async fn cmd(plane: &Arc<InProcessControlPlane>, text: &str, message_id: &str) {
    plane
        .handle_event(
            ev().id(&format!("ev-{message_id}"))
                .text(text)
                .mentioned(true)
                .message_id(message_id)
                .build(),
        )
        .await
        .expect("命令不该报错");
}

async fn cmd_in_thread(plane: &Arc<InProcessControlPlane>, text: &str, message_id: &str) {
    plane
        .handle_event(
            ev().id(&format!("ev-{message_id}"))
                .text(text)
                .mentioned(false)
                .message_id(message_id)
                .thread(ROOT)
                .build(),
        )
        .await
        .expect("命令不该报错");
}

// ---- !status --------------------------------------------------------------

#[tokio::test]
async fn status_empty() {
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "!status", "om_9").await;
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务"
    );
    assert_eq!(plane.counter("commands!status"), 1);
}

#[tokio::test]
async fn status_lists_active_tasks() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("画个趋势图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    cmd(&plane, "!status", "om_9").await;

    let body = h.platform.last_text().expect("该回帖").text;
    assert!(body.contains(&task.task_no), "{body}");
    assert!(body.contains("画个趋势图"), "{body}");
    assert!(body.contains("created"), "{body}");
}

#[tokio::test]
async fn status_is_scoped_to_this_chat() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(
            ev().id("e1")
                .text("别的群的活")
                .message_id("om_x")
                .chat("oc_other")
                .build(),
        )
        .await
        .expect("别的群建任务");

    cmd(&plane, "!status", "om_9").await;

    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务"
    );
}

// ---- !stop ----------------------------------------------------------------

#[tokio::test]
async fn stop_cancels_and_releases_sandbox() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("跑个长活").message_id(ROOT).build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.card_id = Some("om_card_1".into());
    task.sandbox_id = Some("sb_x".into());
    h.store.update_task(&task).await.expect("回写");

    cmd(&plane, &format!("!stop {}", task.task_no), "om_9").await;

    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
    assert_eq!(h.sandbox.released(), vec!["sb_x".to_string()]);
    let (card_id, card) = h.platform.card_updates().pop().expect("该更新卡片");
    assert_eq!(card_id, "om_card_1");
    assert_eq!(card.status, CardStatus::Cancelled);
    assert!(
        h.platform
            .last_text()
            .expect("该回帖")
            .text
            .contains(&task.task_no)
    );
    // 「不在跑」分支该把 Gateway 手上的沙箱也还了、链上留一条 cancelled、manifest 收口
    assert_eq!(h.gateway.released(), vec![task.id.clone()]);
    assert!(
        h.evidence
            .kinds(&task.id)
            .contains(&aite_contracts::EvidenceKind::Cancelled)
    );
    assert!(h.evidence.manifest(&task.id).is_some());
    assert!(saved.evidence_root_hash.is_some(), "root_hash 该进库");
}

#[tokio::test]
async fn stop_accepts_task_no_without_hash() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("活儿").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    let bare = task.task_no.trim_start_matches('#').to_lowercase();
    cmd(&plane, &format!("!stop {bare}"), "om_9").await;

    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
}

#[tokio::test]
async fn stop_unknown_task() {
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "!stop #A99", "om_9").await;
    assert_eq!(h.platform.last_text().expect("该回帖").text, "没有这个任务");
}

// ---- !restart -------------------------------------------------------------

#[tokio::test]
async fn restart_archives_session_and_starts_new_one() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一版方案").message_id(ROOT).build())
        .await
        .expect("建任务");
    let old = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");

    cmd_in_thread(&plane, "!restart 换个思路重来", "om_2").await;

    let archived = h.store.get_session(&old.id).await.expect("读").expect("有");
    assert_eq!(archived.status, SessionStatus::Archived);
    let new = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("同一话题该有新会话");
    assert_ne!(new.id, old.id, "同一话题，换了会话");
    assert_eq!(new.anchor.thread_id.as_deref(), Some(ROOT));

    let tasks = active_tasks(&h.store, CHAT).await;
    assert_eq!(tasks.len(), 1, "旧会话那个已被终止");
    assert_eq!(tasks[0].session_id, new.id);
    assert_eq!(tasks[0].title, "换个思路重来");
    assert_eq!(turn_texts(&h.store, &new.id).await, vec!["换个思路重来"]);
    assert!(
        h.platform
            .last_text()
            .expect("该回帖")
            .text
            .contains("终止了 1 个进行中的任务")
    );
}

#[tokio::test]
async fn restart_without_text_only_archives() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一版").message_id(ROOT).build())
        .await
        .expect("建任务");
    let old = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");

    cmd_in_thread(&plane, "!restart", "om_2").await;

    let archived = h.store.get_session(&old.id).await.expect("读").expect("有");
    assert_eq!(archived.status, SessionStatus::Archived);
    assert!(active_tasks(&h.store, CHAT).await.is_empty());
    assert!(
        h.platform
            .last_text()
            .expect("该回帖")
            .text
            .contains("请直接说要做什么")
    );
}

// ---- !new -----------------------------------------------------------------

#[tokio::test]
async fn new_forces_fresh_session_inside_existing_thread() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("老话题").message_id(ROOT).build())
        .await
        .expect("建任务");
    let old = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");

    cmd_in_thread(&plane, "!new 另起一件事", "om_5").await;

    let still = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");
    assert_eq!(still.id, old.id, "老话题没被动");
    let fresh = h
        .store
        .find_session_by_thread(CHAT, "om_5")
        .await
        .expect("查")
        .expect("本条消息成了新 root");
    assert_ne!(fresh.id, old.id);

    let tasks = active_tasks(&h.store, CHAT).await;
    let session_ids: std::collections::HashSet<String> =
        tasks.iter().map(|t| t.session_id.clone()).collect();
    assert_eq!(
        session_ids,
        [old.id.clone(), fresh.id.clone()].into_iter().collect()
    );
    let fresh_task = tasks
        .iter()
        .find(|t| t.session_id == fresh.id)
        .expect("新会话下该有任务");
    assert_eq!(fresh_task.title, "另起一件事");
    // `!new <文本>` 不回帖（§9 第 3 条）
    assert!(h.platform.texts().is_empty(), "{:?}", h.platform.texts());
}

// ---- 未知命令 -------------------------------------------------------------

#[tokio::test]
async fn unknown_command() {
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "!oops 干点啥", "om_9").await;
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        UNKNOWN_COMMAND_TEXT
    );
    assert!(UNKNOWN_COMMAND_TEXT.contains("!status"));
    assert!(UNKNOWN_COMMAND_TEXT.contains("!restart"));
    assert_eq!(plane.counter("commands!oops"), 1);
}
