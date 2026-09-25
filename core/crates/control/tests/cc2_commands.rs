//! CC2 的命令：④ `!restart` 回灌话题历史；⑤ 解析 / 别名 / `!help`；⑧ `!new` 之后的路由；
//! ⑨ `!stop` 记发起人、命令记 `route=command`。
mod support;

use std::sync::Arc;

use chrono::{TimeZone, Utc};

use aite_contracts::{ControlPlane, HistoryMessage, SessionStore, TurnRole};
use aite_control::InProcessControlPlane;
use support::{CHAT, Harness, ROOT, ev};

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

fn hist(
    message_id: &str,
    sender_kind: &str,
    name: &str,
    text: &str,
    minute: u32,
) -> HistoryMessage {
    HistoryMessage {
        message_id: message_id.into(),
        sender_id: format!("id_{name}"),
        sender_kind: sender_kind.into(),
        sender_name: Some(name.into()),
        text: text.into(),
        thread_id: Some(ROOT.into()),
        created_at: Utc
            .with_ymd_and_hms(2026, 9, 25, 10, minute, 0)
            .single()
            .expect("固定时间"),
    }
}

// ---- ④ `!restart` 回灌话题历史 --------------------------------------------

#[tokio::test]
async fn restart_seeds_thread_history() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一版方案").message_id(ROOT).build())
        .await
        .expect("建任务");
    h.platform.set_history(vec![
        hist(ROOT, "human", "张三", "第一版方案", 0),
        hist("om_bot", "bot", "Aite", "方案如下……", 1),
        hist("om_2", "human", "张三", "!restart 换个思路重来", 2), // `!restart` 本身：不写
    ]);

    cmd_in_thread(&plane, "!restart 换个思路重来", "om_2").await;

    assert_eq!(
        h.platform.history_calls(),
        vec![(CHAT.to_string(), 50, Some(ROOT.to_string()))],
        "按原话题读一次历史"
    );
    let new = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("新会话");
    let turns = h.store.list_turns(&new.id, 100).await.expect("读 turns");
    let got: Vec<(TurnRole, String, Option<String>)> = turns
        .into_iter()
        .map(|t| (t.role, t.content, t.platform_user_id))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                TurnRole::User,
                "第一版方案".to_string(),
                Some("id_张三".to_string())
            ),
            (
                TurnRole::SystemNote,
                "[Aite] 方案如下……".to_string(),
                Some("id_Aite".to_string())
            ),
            (
                TurnRole::User,
                "换个思路重来".to_string(),
                Some("ou_user".to_string())
            ),
        ],
        "历史在前（正序、人记 User、其余记 SystemNote 标发言人）、`rest` 在后"
    );
}

#[tokio::test]
async fn restart_skips_history_when_platform_cannot_read_it() {
    let h = Harness::new();
    let mut caps = aite_contracts::feishu_p0();
    caps.supports_history = false;
    h.platform.set_capabilities(caps);
    h.platform
        .set_history(vec![hist(ROOT, "human", "张三", "第一版方案", 0)]);
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一版方案").message_id(ROOT).build())
        .await
        .expect("建任务");

    cmd_in_thread(&plane, "!restart 换个思路重来", "om_2").await;

    assert!(h.platform.history_calls().is_empty(), "不支持就不读");
    let new = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("新会话");
    assert_eq!(
        support::turn_texts(&h.store, &new.id).await,
        vec!["换个思路重来"]
    );
}
