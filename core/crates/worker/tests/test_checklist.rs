//! B3：checklist 协议与卡片。移植自 `tests/worker/test_checklist.py`（9 条）。
//!
//! 判据原文：脚本化模型先 `checklist_add` 3 项、逐项 `checklist_check`、最后 `final`
//! → FakePlatform 记录到 `send_card`×1、`update_card`≥3、`send_text`×1，
//! **且没有第二条 `send_card`**。
mod common;

use aite_contracts::{CardStatus, ChecklistState, Role, TaskStatus};
use common::*;
use serde_json::json;

fn script() -> Vec<aite_contracts::ModelTurn> {
    vec![
        tool_turn(&[(
            "checklist_add",
            json!({"items": ["读取数据", "按月聚合", "画趋势图"]}),
        )]),
        tool_turn(&[("checklist_check", json!({"id": "c1"}))]),
        tool_turn(&[("checklist_check", json!({"id": "c2"}))]),
        tool_turn(&[("checklist_check", json!({"id": "c3"}))]),
        final_turn("图已画好，趋势见附件。"),
    ]
}

#[tokio::test]
async fn b3_card_lifecycle() {
    // 每步之间推进 0.6s，越过 W4 的 500ms 合并窗口，让每次变更都真的推一次
    let run = run_script(script(), 0.6).await;

    assert_eq!(
        run.h.platform.cards().len(),
        1,
        "send_card × 1，且没有第二条"
    );
    assert!(run.h.platform.card_updates().len() >= 3, "update_card ≥ 3");
    assert_eq!(run.h.platform.texts().len(), 1, "send_text × 1");
    assert_eq!(run.model.call_count(), 5);
    assert_eq!(run.task.status, TaskStatus::Delivered);
}

#[tokio::test]
async fn b3_card_content() {
    let run = run_script(script(), 0.6).await;

    let (_, _, first) = run.h.platform.cards()[0].clone();
    assert_eq!(first.status, CardStatus::Working);
    assert_eq!(first.task_no, run.task.task_no);
    assert_eq!(first.initiator, "张三");

    let last = run.h.platform.last_card();
    assert_eq!(last.status, CardStatus::Delivered);
    let texts: Vec<&str> = last.items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, ["读取数据", "按月聚合", "画趋势图"]);
    assert!(last.items.iter().all(|i| i.state == ChecklistState::Done));

    // 所有更新都打在同一张卡片上（原地更新，绝不新发消息）
    let ids: std::collections::HashSet<String> = run
        .h
        .platform
        .card_updates()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids.len(), 1);
    assert_eq!(ids.into_iter().next(), run.task.card_id);
}

#[tokio::test]
async fn b3_card_is_replied_into_thread_root() {
    let run = run_script(script(), 0.6).await;

    let (chat_id, reply_to, _card) = run.h.platform.cards()[0].clone();
    assert_eq!(chat_id, CHAT_ID);
    assert_eq!(
        reply_to.as_deref(),
        run.h.session.anchor.thread_id.as_deref()
    );
    assert_eq!(reply_to.as_deref(), Some(ROOT));

    let text = &run.h.platform.texts()[0];
    assert_eq!(text.reply_to.as_deref(), Some(ROOT));
    assert!(text.in_thread);
}

// ---- W4：500ms 合并 -----------------------------------------------------

fn rapid() -> Vec<aite_contracts::ModelTurn> {
    vec![
        tool_turn(&[("checklist_add", json!({"items": ["一", "二"]}))]),
        tool_turn(&[("checklist_note", json!({"text": "在算了"}))]),
        tool_turn(&[("checklist_note", json!({"text": "还在算"}))]),
        tool_turn(&[("checklist_check", json!({"id": "c1"}))]),
        final_turn("好了"),
    ]
}

#[tokio::test]
async fn w4_changes_within_window_collapse() {
    // 钟不动 = 所有变更都落在同一个 500ms 窗口 → 只有任务结束那一次 update_card
    let run = run_script(rapid(), 0.0).await;

    assert_eq!(run.h.platform.cards().len(), 1);
    assert_eq!(
        run.h.platform.card_updates().len(),
        1,
        "4 次变更合并成 1 次"
    );
    assert_eq!(
        run.h.platform.card_updates()[0].1.status,
        CardStatus::Delivered
    );
}

#[tokio::test]
async fn w4_changes_across_windows_are_pushed() {
    // 同一个脚本，每步跨过 500ms → 逐次推送。对照上一条。
    let run = run_script(rapid(), 0.6).await;

    assert_eq!(run.h.platform.cards().len(), 1);
    assert_eq!(run.h.platform.card_updates().len(), 4);
}

#[tokio::test]
async fn checklist_note_lands_on_the_card_not_a_new_message() {
    let run = run_script(rapid(), 0.6).await;

    assert_eq!(run.h.platform.texts().len(), 1, "只有 final 那一条");
    assert!(run.h.platform.last_card().footer.contains("还在算"));
}

// ---- checklist 的边角 ---------------------------------------------------

#[tokio::test]
async fn checklist_items_are_clipped_to_20_chars() {
    let long = "这是一条特别特别特别特别特别特别啰嗦的待办项";
    let run = run_script(
        vec![
            tool_turn(&[("checklist_add", json!({"items": [long]}))]),
            final_turn("完"),
        ],
        0.6,
    )
    .await;

    let item = run.h.platform.last_card().items[0].clone();
    assert!(item.text.chars().count() <= 20);
    assert!(item.text.ends_with('…'));
}

#[tokio::test]
async fn checklist_fail_marks_item() {
    let run = run_script(
        vec![
            tool_turn(&[("checklist_add", json!({"items": ["下载附件", "解析"]}))]),
            tool_turn(&[(
                "checklist_fail",
                json!({"id": "c1", "reason": "文件下载失败"}),
            )]),
            final_turn("附件没拿到，先给了口径说明。"),
        ],
        0.6,
    )
    .await;

    let items = run.h.platform.last_card().items;
    assert_eq!(items[0].state, ChecklistState::Failed);
    assert_eq!(items[0].note.as_deref(), Some("文件下载失败"));
    assert_eq!(items[1].state, ChecklistState::Todo);
}

#[tokio::test]
async fn bad_checklist_id_is_invalid_args_not_a_crash() {
    let run = run_script(
        vec![
            tool_turn(&[("checklist_check", json!({"id": "c9"}))]),
            final_turn("没有这项，改口径答复。"),
        ],
        0.6,
    )
    .await;

    assert_eq!(run.task.status, TaskStatus::Delivered);
    // 模型看到的是一条 role=tool 的错误说明，而不是异常
    let last = run.model.last_call();
    let tool_msgs: Vec<&aite_contracts::Message> =
        last.iter().filter(|m| m.role == Role::Tool).collect();
    assert!(!tool_msgs.is_empty());
    let content = &tool_msgs[tool_msgs.len() - 1].content;
    assert!(content.contains("没有这一项"), "实际是 {content}");
    // Python 的 `f"没有这一项：{id!r}"`：字符串带单引号
    assert!(content.contains("'c9'"), "实际是 {content}");
}
