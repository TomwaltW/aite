//! BB2 ②：卡片的发送与更新进证据。
//!
//! M3 要求「卡片至少更新 3 次且不新增消息」。在这之前 `send_card` / `update_card`
//! 在证据里一个字都没有、也没有日志，只能肉眼数群里的卡片。
//!
//! 这个文件钉的是**那两个数必须是同一个数**：证据里的 `card_updated` 条数
//! 恒等于平台真被调到的 `update_card` 次数。W4 的合并把「worker 想更新几次」压小了，
//! 而 M3 要数的是压小之后的那个数 —— 事件因此发在 `CardCoalescer` 真调平台的那两行后面，
//! 不在 `agent.rs` 的三个触发点上。`merged` 记的是被压掉的那一半，两个数一起才说得清。
mod common;

use aite_contracts::{CardStatus, EvidenceKind};
use common::*;
use serde_json::{Map, Value, json};

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

/// 证据里所有 `checklist_op` 的 payload，按落盘顺序。
fn ops_of(run: &Run) -> Vec<Map<String, Value>> {
    run.h
        .evidence
        .payloads(&run.task.id, EvidenceKind::ChecklistOp)
}

fn with_op<'a>(ops: &'a [Map<String, Value>], op: &str) -> Vec<&'a Map<String, Value>> {
    ops.iter()
        .filter(|p| p["op"].as_str() == Some(op))
        .collect()
}

#[tokio::test]
async fn card_sent_and_updated_land_in_evidence() {
    // 每步之间推进 0.6s，越过 500ms 窗口 —— 每次变更都真推一次
    let run = run_script(script(), 0.6).await;
    let ops = ops_of(&run);

    let sent = with_op(&ops, "card_sent");
    assert_eq!(sent.len(), 1, "send_card 至多一次，证据也只该有一条");
    assert_eq!(
        sent[0]["card_id"].as_str().unwrap_or_default(),
        run.task.card_id.clone().unwrap_or_default(),
        "证据里的 card_id 要和任务身上那个是同一个"
    );

    let updated = with_op(&ops, "card_updated");
    assert_eq!(
        updated.len(),
        run.h.platform.card_updates().len(),
        "证据里的 card_updated 条数 ≠ 平台真被调到的 update_card 次数"
    );
    assert!(
        updated.len() >= 3,
        "M3 要的是 ≥3 次，实际 {}",
        updated.len()
    );

    // push 是 1,2,3…：漏记一条就会在这里断号，而不是悄悄少一个数
    let pushes: Vec<u64> = updated
        .iter()
        .map(|p| p["push"].as_u64().unwrap_or_default())
        .collect();
    assert_eq!(
        pushes,
        (1..=updated.len() as u64).collect::<Vec<_>>(),
        "card_updated 的 push 要从 1 起连续"
    );

    // 最后一条一定是终态卡片；而它后面还得跟着终态事件（终态事件必须是链上最后一条）
    assert_eq!(
        updated.last().expect("至少一条")["status"],
        json!(CardStatus::Delivered.as_str())
    );
    let kinds = run.h.evidence.kinds(&run.task.id);
    assert_eq!(
        kinds.last(),
        Some(&EvidenceKind::Delivered),
        "收卡片必须排在终态证据之前，否则 evidence show 的「终态」就读不出来了"
    );
}

/// 钟不动 = 所有变更落在同一个 500ms 窗口：平台只被调一次，证据也只有一条，
/// 而那一条的 `merged` 要把被压掉的那几次说出来。
#[tokio::test]
async fn merged_counts_what_w4_collapsed() {
    let run = run_script(
        vec![
            tool_turn(&[("checklist_add", json!({"items": ["一", "二"]}))]),
            tool_turn(&[("checklist_note", json!({"text": "在算了"}))]),
            tool_turn(&[("checklist_note", json!({"text": "还在算"}))]),
            tool_turn(&[("checklist_check", json!({"id": "c1"}))]),
            final_turn("好了"),
        ],
        0.0,
    )
    .await;

    let ops = ops_of(&run);
    let updated = with_op(&ops, "card_updated");
    assert_eq!(
        run.h.platform.card_updates().len(),
        1,
        "4 次变更合并成 1 次"
    );
    assert_eq!(updated.len(), 1, "证据也只该有一条");
    assert_eq!(updated[0]["push"], json!(1));
    assert!(
        updated[0]["merged"].as_u64().unwrap_or_default() >= 4,
        "merged 要把被 W4 压掉的那几次算进来，实际 {}",
        updated[0]["merged"]
    );
}

/// Answering 路径（第一步就 final）：卡片压根没发过 —— 证据里也一条都不许有。
/// 「没发卡片」和「发了但没记」必须分得开。
#[tokio::test]
async fn no_card_means_no_card_evidence() {
    let run = run_script(vec![final_turn("北京今天 26 度。")], 0.0).await;

    assert!(run.h.platform.cards().is_empty());
    let ops = ops_of(&run);
    assert!(
        with_op(&ops, "card_sent").is_empty() && with_op(&ops, "card_updated").is_empty(),
        "没发过卡片却有卡片证据：{ops:?}"
    );
}
