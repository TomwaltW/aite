//! B2：§3.5 路由规则 R1–R8，每条至少一个用例。
//! 对应 `tests/control/test_routing.py`（12 条）。
//!
//! 规则按编号顺序求值、命中即停，所以除了「这条规则做对了什么」，还要盯住
//! 「更靠后的规则没有被顺带触发」—— 每个用例都断言了这一点。
mod support;

use aite_contracts::{
    CardActionKind, ControlPlane, EventKind, ReactionKind, SenderKind, SessionStore, TaskStatus,
};
use support::{CHAT, Harness, active_tasks, card_action, ev, turn_texts};

// ---- R1：sender_kind != human 一律丢弃 -----------------------------------

/// 机器人 @ 了 Aite 也不建会话、不出站。含 Aite 自己发的消息。
#[tokio::test]
async fn r1_bot_at_never_starts_task() {
    let h = Harness::new();
    let plane = h.plane();

    for (i, kind) in [SenderKind::Bot, SenderKind::App, SenderKind::System]
        .into_iter()
        .enumerate()
    {
        plane
            .handle_event(
                ev().id(&format!("ev{i}"))
                    .sender_kind(kind)
                    .text("@Aite 帮我算一下")
                    .message_id(&format!("om_{i}"))
                    .build(),
            )
            .await
            .expect("非人类事件不该报错");
    }

    assert_eq!(plane.counter("events.nonhuman"), 3);
    assert!(active_tasks(&h.store, CHAT).await.is_empty());
    assert!(h.platform.reactions().is_empty());
    assert!(h.platform.texts().is_empty());
}

// ---- R2：seen_event 去重 --------------------------------------------------

/// 重连后平台重推同一 event_id → 只建一个 task。
#[tokio::test]
async fn r2_duplicate_event_makes_one_task() {
    let h = Harness::new();
    let plane = h.plane();
    let event = ev().id("ev-dup").build();

    plane.handle_event(event.clone()).await.expect("第一条");
    plane.handle_event(event).await.expect("重推那条");

    assert_eq!(plane.counter("events.duplicate"), 1);
    assert_eq!(active_tasks(&h.store, CHAT).await.len(), 1);
}

// ---- R3：card_action ------------------------------------------------------

#[tokio::test]
async fn r3_card_stop_cancels_task_without_new_session() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("跑个数").build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.card_id = Some("om_card_1".into());
    task.sandbox_id = Some("sb_1".into());
    h.store.update_task(&task).await.expect("回写 card/sandbox");

    plane
        .handle_event(
            ev().id("ev-card")
                .kind(EventKind::CardAction)
                .text("")
                .message_id("om_9")
                .card_action(card_action(
                    "om_card_1",
                    CardActionKind::Stop,
                    Some(&task.id),
                ))
                .build(),
        )
        .await
        .expect("卡片停止");

    let saved = h
        .store
        .get_task(&task.id)
        .await
        .expect("读任务")
        .expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
    assert_eq!(h.sandbox.released(), vec!["sb_1".to_string()]);
    assert!(active_tasks(&h.store, CHAT).await.is_empty()); // 不再是活跃任务
    // R3 不建会话
    assert!(
        h.store
            .find_session_by_thread(CHAT, "om_9")
            .await
            .expect("查会话")
            .is_none()
    );
}

#[tokio::test]
async fn r3_card_evidence_replies_with_path() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("跑个数").build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    plane
        .handle_event(
            ev().id("ev-card2")
                .kind(EventKind::CardAction)
                .text("")
                .message_id("om_9")
                .card_action(card_action(
                    "om_card_1",
                    CardActionKind::Evidence,
                    Some(&task.id),
                ))
                .build(),
        )
        .await
        .expect("证据按钮");

    let body = h.platform.last_text().expect("该回一帖").text;
    assert!(body.contains(&task.id), "回帖里该有 task_id：{body}");
    assert!(
        body.contains(&h.config.storage.evidence_dir),
        "回帖里该有证据目录：{body}"
    );
}

// ---- R4：编辑 / 删除 ------------------------------------------------------

/// 编辑即使加上 @Aite 也不启动任务。
#[tokio::test]
async fn r4_edit_writes_system_note_and_starts_nothing() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("画个图").build())
        .await
        .expect("建任务");
    let session_id = active_tasks(&h.store, CHAT).await.remove(0).session_id;
    let before = active_tasks(&h.store, CHAT).await.len();

    plane
        .handle_event(
            ev().id("ev-edit")
                .kind(EventKind::MessageEdited)
                .text("@Aite 画个柱状图")
                .mentioned(true)
                .message_id(support::ROOT) // 编辑的正是话题 root 那条
                .build(),
        )
        .await
        .expect("编辑事件");

    let turns = h
        .store
        .list_turns(&session_id, 200)
        .await
        .expect("读 turns");
    let last = turns.last().expect("至少一条");
    assert_eq!(last.role, aite_contracts::TurnRole::SystemNote);
    assert_eq!(last.content, "[用户修改了消息] 新内容：@Aite 画个柱状图");
    assert_eq!(active_tasks(&h.store, CHAT).await.len(), before);
    assert_eq!(plane.counter("events.edited"), 1);
}

#[tokio::test]
async fn r4_delete_does_nothing() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("画个图").build())
        .await
        .expect("建任务");
    let session_id = active_tasks(&h.store, CHAT).await.remove(0).session_id;
    let before = turn_texts(&h.store, &session_id).await;
    let texts_before = h.platform.texts().len();

    plane
        .handle_event(
            ev().id("ev-del")
                .kind(EventKind::MessageDeleted)
                .text("")
                .message_id(support::ROOT)
                .build(),
        )
        .await
        .expect("删除事件");

    assert_eq!(plane.counter("events.deleted"), 1);
    assert_eq!(turn_texts(&h.store, &session_id).await, before);
    assert_eq!(h.platform.texts().len(), texts_before);
}

// ---- R5：`!` 命令先于 R6/R7 -----------------------------------------------

/// 话题内的 `!status` 走命令，不当成续接消息去建 task。
#[tokio::test]
async fn r5_command_in_thread_beats_r6() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("第一件事").build())
        .await
        .expect("建任务");
    let before: Vec<String> = active_tasks(&h.store, CHAT)
        .await
        .into_iter()
        .map(|t| t.id)
        .collect();

    plane
        .handle_event(
            ev().id("ev-cmd")
                .text("!status")
                .mentioned(false)
                .message_id("om_2")
                .thread(support::ROOT)
                .build(),
        )
        .await
        .expect("命令");

    let after: Vec<String> = active_tasks(&h.store, CHAT)
        .await
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(after, before, "没有新 task");
    assert!(
        h.platform
            .last_text()
            .expect("该回一帖")
            .text
            .contains("本群活跃任务")
    );
}

/// 既没 @ 也不在话题里的 `!xxx` 不算命令，落到 R8 被丢弃。
#[tokio::test]
async fn r5_command_without_at_or_thread_is_not_a_command() {
    let h = Harness::new();
    let plane = h.plane();

    plane
        .handle_event(
            ev().id("ev-cmd2")
                .text("!status")
                .mentioned(false)
                .message_id("om_5")
                .build(),
        )
        .await
        .expect("裸命令");

    assert!(h.platform.texts().is_empty());
    assert_eq!(plane.counter("events.ignored"), 1);
}

// ---- R6：话题内续接，不要求 mentioned -------------------------------------

#[tokio::test]
async fn r6_followup_hits_same_session_and_starts_new_task() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("第一问").build())
        .await
        .expect("第一问");
    let mut first = active_tasks(&h.store, CHAT).await.remove(0);
    first.status = TaskStatus::Delivered; // 上一个任务已结束
    h.store.update_task(&first).await.expect("落终态");

    plane
        .handle_event(
            ev().id("ev2")
                .text("再按季度画一张")
                .mentioned(false)
                .message_id("om_2")
                .thread(support::ROOT)
                .build(),
        )
        .await
        .expect("追问");

    let active = active_tasks(&h.store, CHAT).await;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].session_id, first.session_id, "同一 session");
    assert_ne!(active[0].id, first.id, "新 task");
    assert_ne!(active[0].task_no, first.task_no);
}

#[tokio::test]
async fn r6_followup_with_active_task_queues_steer() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("第一问").build())
        .await
        .expect("第一问");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

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

    assert_eq!(
        plane.pending_steer(&task.id),
        vec!["顺便加上同比".to_string()]
    );
    assert_eq!(
        active_tasks(&h.store, CHAT).await.len(),
        1,
        "没有第二个 task"
    );
    assert_eq!(plane.counter("events.steer"), 1);
}

// ---- R7：@ 新建 -----------------------------------------------------------

#[tokio::test]
async fn r7_mention_creates_session_rooted_at_this_message() {
    let h = Harness::new();
    let plane = h.plane();

    plane
        .handle_event(ev().text("帮我看下这个").message_id("om_root").build())
        .await
        .expect("@ 建任务");

    let task = active_tasks(&h.store, CHAT).await.remove(0);
    let session = h
        .store
        .get_session(&task.session_id)
        .await
        .expect("读会话")
        .expect("有");
    assert_eq!(
        session.anchor.thread_id.as_deref(),
        Some("om_root"),
        "本条消息成为话题 root"
    );
    assert_eq!(
        h.platform.reactions(),
        vec![("om_root".to_string(), ReactionKind::Ack)]
    );
    assert!(task.task_no.starts_with("#A"));
    assert_eq!(plane.pending(), 1, "已入队");
    assert_eq!(task.session_token.len(), 32, "token_hex(16) = 32 hex");
}

// ---- R8：其余丢弃 ---------------------------------------------------------

#[tokio::test]
async fn r8_plain_group_message_is_dropped() {
    let h = Harness::new();
    let plane = h.plane();

    plane
        .handle_event(
            ev().id("ev-plain")
                .text("大家早")
                .mentioned(false)
                .message_id("om_7")
                .build(),
        )
        .await
        .expect("闲聊");

    assert_eq!(plane.counter("events.ignored"), 1);
    assert!(active_tasks(&h.store, CHAT).await.is_empty());
    assert!(h.platform.texts().is_empty());
    assert!(h.platform.reactions().is_empty());
}
