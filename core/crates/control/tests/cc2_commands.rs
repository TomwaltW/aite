//! CC2 的命令：④ `!restart` 回灌话题历史；⑤ 解析 / 别名 / `!help`；⑧ `!new` 之后的路由；
//! ⑨ `!stop` 记发起人、命令记 `route=command`。
mod support;

use std::sync::Arc;

use chrono::{TimeZone, Utc};

use aite_contracts::{
    CardActionKind, ControlPlane, EvidenceKind, HistoryMessage, SessionStore, TurnRole,
};
use aite_control::{InProcessControlPlane, ROUTE_COMMAND, UNKNOWN_COMMAND_TEXT, parse_command};
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

// ---- ⑤ 解析、别名、`!help` ------------------------------------------------

/// 群里 @ 一下发命令（R5 要求 @ 过或者在话题里）。
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

#[tokio::test]
async fn fullwidth_bang_and_ideographic_space_parse() {
    // 解析层：全角 ！ 等价于 !；U+3000 与制表符都算分隔
    assert_eq!(
        parse_command("！stop\u{3000}#A17"),
        ("!stop".to_string(), "#A17".to_string())
    );
    assert_eq!(
        parse_command("!stop\t#A17"),
        ("!stop".to_string(), "#A17".to_string())
    );
    assert_eq!(
        parse_command("！STATUS"),
        ("!status".to_string(), String::new())
    );

    // 路由层：全角开头也进 R5（不会被当成一句话建任务）
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "！status", "om_9").await;
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务"
    );
    assert_eq!(plane.counter("commands!status"), 1);
    assert_eq!(h.store.task_count(), 0, "命令不建任务");
}

#[tokio::test]
async fn chinese_alias_status() {
    assert_eq!(
        parse_command("！状态"),
        ("!status".to_string(), String::new())
    );
    assert_eq!(
        parse_command("!停止 a17"),
        ("!stop".to_string(), "a17".to_string())
    );
    assert_eq!(parse_command("!重开 换个思路").0, "!restart");
    assert_eq!(parse_command("!新话题").0, "!new");
    assert_eq!(parse_command("!帮助").0, "!help");

    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "！状态", "om_9").await;
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务"
    );
    assert_eq!(plane.counter("commands!status"), 1, "计数器用规范名");

    // 不带 ! 的「状态」不是命令：@ 了就是一句普通的话，照常建任务
    cmd(&plane, "状态", "om_10").await;
    assert_eq!(plane.counter("commands!status"), 1);
    assert_eq!(h.store.task_count(), 1);
}

#[tokio::test]
async fn help_lists_enabled_commands() {
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "!help", "om_9").await;
    let text = h.platform.last_text().expect("该回帖").text;
    assert_eq!(
        text,
        "可用命令：\n\
!status　列出本群的活跃任务\n\
!stop <任务号>　停止一个任务（本群只有一个活跃任务时可省略任务号）\n\
!restart [要做的事]　重开当前会话\n\
!new [要做的事]　另起一个新话题\n\
!help　看这份命令列表\n\
命令开头的 ! 也可以打全角的 ！；中文也行：！状态 ！停止 ！重开 ！新话题 ！帮助"
    );
    for enabled in ["!status", "!stop", "!restart", "!new", "!help"] {
        assert!(text.contains(enabled), "启用的 {enabled} 该列出");
    }
    for disabled in [
        "!about",
        "!access",
        "!configure",
        "!mute",
        "!unmute",
        "!feedback",
        "!routines",
        "!fork",
        "!memory",
        "!approve",
        "!reject",
        "!evidence",
        "!connect",
        "!usage",
        "!model",
    ] {
        assert!(!text.contains(disabled), "停用的 {disabled} 一个都不许出现");
    }
    assert_eq!(plane.counter("commands!help"), 1);

    // 停用的命令走「未知命令」那条路，指路 `!help`
    cmd(&plane, "!mute", "om_10").await;
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        UNKNOWN_COMMAND_TEXT
    );
    assert_eq!(UNKNOWN_COMMAND_TEXT, "未知命令，发 !help 看全部命令");
    assert_eq!(plane.counter("commands!mute"), 1);
}

// ---- ⑧ `!new` 之后同话题的回复 --------------------------------------------

/// 话题里 `!new 另起一件事` 之后，同话题一条无 @ 的回复（`thread` 仍是老 root）进新会话。
///
/// **不用固定时钟**：同一 `thread_id` 上此时挂着两个非归档会话，谁胜出看 `created_at`，
/// 相等时按随机 uuid —— 这条路由的正确性依赖 `created_at` 严格递增。
#[tokio::test]
async fn new_in_thread_routes_followups_to_the_new_session() {
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
    cmd_in_thread(&plane, "接着说另一件事", "om_6").await;

    let fresh = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");
    assert_ne!(fresh.id, old.id);
    assert_eq!(
        support::turn_texts(&h.store, &fresh.id).await,
        vec!["另起一件事", "接着说另一件事"],
        "回复进 `!new` 建的新会话"
    );
    assert_eq!(
        support::turn_texts(&h.store, &old.id).await,
        vec!["老话题"],
        "老会话一个字没多"
    );
}

// ---- ⑨ `!stop` 记发起人；命令记 `route=command` ---------------------------

/// 这个任务链上 `cancelled` 那条的载荷。
fn cancelled_payload(h: &Harness, task_id: &str) -> serde_json::Map<String, serde_json::Value> {
    h.evidence
        .events(task_id)
        .into_iter()
        .find(|e| e.kind == EvidenceKind::Cancelled)
        .and_then(|e| e.payload)
        .expect("该有 cancelled")
}

#[tokio::test]
async fn stop_records_issuer_and_command_evidence() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("跑个长活").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = support::active_tasks(&h.store, CHAT).await.remove(0);

    plane
        .handle_event(
            ev().id("e2")
                .text("!stop")
                .mentioned(false)
                .message_id("om_2")
                .thread(ROOT)
                .sender_id("ou_boss")
                .build(),
        )
        .await
        .expect("!stop");

    assert_eq!(
        h.evidence.kinds(&task.id),
        vec![
            EvidenceKind::TaskCreated,
            EvidenceKind::EventReceived,
            EvidenceKind::EventReceived,
            EvidenceKind::Cancelled,
        ],
        "前两条照旧；命令证据在 cancelled 之前（finalize 之后不许再写）"
    );
    let command = h.evidence.received(&task.id).remove(1);
    assert_eq!(
        command.get("route").and_then(|v| v.as_str()),
        Some(ROUTE_COMMAND)
    );
    assert_eq!(ROUTE_COMMAND, "command");
    assert_eq!(
        command.get("command").and_then(|v| v.as_str()),
        Some("!stop")
    );
    assert_eq!(
        command.get("sender_id").and_then(|v| v.as_str()),
        Some("ou_boss")
    );
    let cancelled = cancelled_payload(&h, &task.id);
    assert_eq!(cancelled.get("by").and_then(|v| v.as_str()), Some("stop"));
    assert_eq!(
        cancelled.get("stopped_by").and_then(|v| v.as_str()),
        Some("ou_boss")
    );
    assert!(h.evidence.manifest(&task.id).is_some());
}

#[tokio::test]
async fn card_stop_records_issuer_but_no_command_evidence() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("跑个长活").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = support::active_tasks(&h.store, CHAT).await.remove(0);

    plane
        .handle_event(
            ev().id("e2")
                .kind(aite_contracts::EventKind::CardAction)
                .sender_id("ou_boss")
                .card_action(support::card_action(
                    "om_card",
                    CardActionKind::Stop,
                    Some(&task.id),
                ))
                .build(),
        )
        .await
        .expect("卡片 stop");

    assert!(
        h.evidence
            .received(&task.id)
            .iter()
            .all(|p| p.get("route").and_then(|v| v.as_str()) != Some(ROUTE_COMMAND)),
        "卡片按钮不是命令"
    );
    assert_eq!(
        cancelled_payload(&h, &task.id)
            .get("stopped_by")
            .and_then(|v| v.as_str()),
        Some("ou_boss")
    );
}

/// trait 方法（收尾那条路）传 `None`：`stopped_by` 整个不写，载荷与 CC2 之前一致。
#[tokio::test]
async fn trait_cancel_writes_no_stopped_by_key() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("跑个长活").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = support::active_tasks(&h.store, CHAT).await.remove(0);

    plane.cancel_task(task.clone(), None, None, false).await;

    let payload = cancelled_payload(&h, &task.id);
    let keys: Vec<&str> = payload.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["by", "steps"]);
}

#[tokio::test]
async fn restart_writes_command_evidence_on_the_tasks_it_stops() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一版方案").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = support::active_tasks(&h.store, CHAT).await.remove(0);

    cmd_in_thread(&plane, "!restart 换个思路重来", "om_2").await;

    let command = h.evidence.received(&task.id).remove(1);
    assert_eq!(
        command.get("route").and_then(|v| v.as_str()),
        Some(ROUTE_COMMAND)
    );
    assert_eq!(
        command.get("command").and_then(|v| v.as_str()),
        Some("!restart")
    );
    assert_eq!(
        h.evidence.kinds(&task.id).last(),
        Some(&EvidenceKind::Cancelled)
    );
    assert!(
        cancelled_payload(&h, &task.id).get("stopped_by").is_none(),
        "`!restart` 不记 stopped_by（只有 `!stop` 与卡片按钮记）"
    );
}
