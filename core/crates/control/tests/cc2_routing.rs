//! CC2 的路由与入口：③ 入口把存储 / 证据错误传出去；⑥ 能力位与 p2p；⑦ R6 追问补 ack；
//! ⑧ `!new` 之后同话题的回复；⑩ `append_turn` 撞号重试。
mod support;

use aite_contracts::{
    ChatType, ControlPlane, IngressError, ReactionKind, SenderKind, SessionStore, TaskStatus,
    feishu_p0,
};
use aite_control::Ingress;
use support::{CHAT, Harness, ROOT, active_tasks, ev, turn_texts};

/// ③：`handler()` 对存储错误返回 `Err`（gRPC 入口翻成 INTERNAL，`proto/src/status.rs`：
/// 非 `Invalid` 一律 INTERNAL；control 不依赖 aite-proto，断言到这里为止），计数照打。
#[tokio::test]
async fn ingress_store_failure_returns_internal() {
    let h = Harness::new();
    h.store.fail_next("seen_event", 1);
    let plane = h.plane();
    let ingress = Ingress::new(plane.clone());

    let out = (ingress.handler())(ev().build()).await;
    assert!(
        matches!(out, Err(IngressError::Store(_))),
        "存储错误该交回给 gRPC 那一层，实际是 {out:?}"
    );
    assert_eq!(ingress.counter("ingress.errors"), 1);
    assert_eq!(plane.counter("events.dropped"), 1);
}

// ---- ⑥ R6 看 `supports_thread`；p2p 不进 R6 ------------------------------

/// 话题里一条无 @ 的回复（R6 的典型输入）。
fn followup(id: &str, message_id: &str, text: &str) -> aite_contracts::NormalizedEvent {
    ev().id(id)
        .message_id(message_id)
        .thread(ROOT)
        .mentioned(false)
        .text(text)
        .build()
}

#[tokio::test]
async fn r6_requires_supports_thread() {
    let h = Harness::new();
    let mut caps = feishu_p0();
    caps.supports_thread = false;
    h.platform.set_capabilities(caps);
    let plane = h.plane();
    plane
        .handle_event(ev().text("第一件事").build())
        .await
        .expect("R7 建会话");
    let session = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");

    plane
        .handle_event(followup("e2", "om_2", "再补一句"))
        .await
        .expect("追问");

    assert_eq!(
        turn_texts(&h.store, &session.id).await,
        vec!["第一件事"],
        "平台没有原生话题：R6 不接，这句不进会话"
    );
    assert_eq!(plane.counter("events.ignored"), 1, "无 @ → R8 丢弃");
    assert_eq!(plane.counter("events.steer"), 0);
}

/// DM 桩关着时 p2p 的净行为与今天一致：有 @ → R7，无 @ → R8，**即使该话题有会话也不进 R6**。
#[tokio::test]
async fn p2p_goes_to_disabled_dm_stub() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().chat_type(ChatType::P2p).text("私聊里的活").build())
        .await
        .expect("R7 建会话");
    let session = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("p2p 有 @ 照样走 R7");

    let mut ev2 = followup("e2", "om_2", "私聊里的追问");
    ev2.chat_type = ChatType::P2p;
    plane.handle_event(ev2).await.expect("追问");

    assert_eq!(
        turn_texts(&h.store, &session.id).await,
        vec!["私聊里的活"],
        "p2p 不进 R6"
    );
    assert_eq!(plane.counter("events.ignored"), 1);
    assert_eq!(plane.counter("events.steer"), 0);
}

// ---- ⑦ R6 追问补 ack ------------------------------------------------------

#[tokio::test]
async fn r6_followup_gets_ack() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("第一件事").build())
        .await
        .expect("R7 建任务");

    // steer 那条路：任务还在（排着队，本进程接手过）
    plane
        .handle_event(followup("e2", "om_2", "顺便看看这个"))
        .await
        .expect("steer");
    assert_eq!(plane.counter("events.steer"), 1);

    // 新建那条路：任务已经交付了，同话题再问一句 → 在同一个会话里新建任务
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.status = TaskStatus::Delivered;
    h.store.update_task(&task).await.expect("落 delivered");
    plane
        .handle_event(followup("e3", "om_3", "再来一个"))
        .await
        .expect("新建");
    assert_eq!(active_tasks(&h.store, CHAT).await.len(), 1, "新建了一个");

    // 机器人在话题里说话：R1 丢掉，不 ack
    let mut bot = followup("e4", "om_bot", "我是另一个机器人");
    bot.sender_kind = SenderKind::Bot;
    plane.handle_event(bot).await.expect("bot");

    assert_eq!(
        h.platform.reactions(),
        vec![
            (ROOT.to_string(), ReactionKind::Ack),
            ("om_2".to_string(), ReactionKind::Ack),
            ("om_3".to_string(), ReactionKind::Ack),
        ],
        "R7 一次、R6 两条路各一次、bot 零次"
    );
}

// ---- ⑩ `append_turn` 撞 `DuplicateTurn` 重读 seq 重试 ---------------------

/// 锁外的写者（CC3 起 worker 在 `deliver()` 里写 Assistant 轮）抢先占了追问要写的那个 seq：
/// plane 重读 seq 再写一次，追问不丢、入口不报错。
#[tokio::test]
async fn plane_append_turn_retries_duplicate_turn() {
    let h = Harness::new();
    let plane = h.plane();
    let ingress = Ingress::new(plane.clone());
    plane
        .handle_event(ev().text("第一件事").build())
        .await
        .expect("R7 建会话");
    let session = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");

    h.store.duplicate_next_append_turn(1);
    let before = h
        .store
        .attempts
        .lock()
        .expect("attempts 锁")
        .iter()
        .filter(|m| *m == "append_turn")
        .count();

    let out = (ingress.handler())(followup("e2", "om_2", "再补一句")).await;
    assert!(out.is_ok(), "撞号该被重试兜住，实际 {out:?}");
    assert_eq!(ingress.counter("ingress.errors"), 0);

    let turns = h.store.list_turns(&session.id, 100).await.expect("读");
    let got: Vec<(u64, String)> = turns.into_iter().map(|t| (t.seq, t.content)).collect();
    assert_eq!(
        got,
        vec![
            (0, "第一件事".to_string()),
            (1, "（别的写者抢先写的 seq=1）".to_string()),
            (2, "再补一句".to_string()),
        ],
        "追问接在被占的那个号后面"
    );
    let after = h
        .store
        .attempts
        .lock()
        .expect("attempts 锁")
        .iter()
        .filter(|m| *m == "append_turn")
        .count();
    assert_eq!(after - before, 2, "撞一次、重试一次");
}

/// 撞满 3 次就放弃，照旧把 `Store` 错误传出去（不无限重试）。
#[tokio::test]
async fn append_turn_gives_up_after_three_duplicates() {
    let h = Harness::new();
    let plane = h.plane();
    let ingress = Ingress::new(plane.clone());
    plane
        .handle_event(ev().text("第一件事").build())
        .await
        .expect("R7 建会话");

    h.store.duplicate_next_append_turn(3);
    let out = (ingress.handler())(followup("e2", "om_2", "再补一句")).await;
    assert!(
        matches!(out, Err(IngressError::Store(_))),
        "3 次都撞：存储错误照旧传出去，实际 {out:?}"
    );
}
