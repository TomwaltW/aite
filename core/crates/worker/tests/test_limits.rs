//! R5 独有验收：脚本化模型死循环 → 第 40 步 `failed` 且回帖含「上限」（W6 / §3.3）。
//! 移植自 `tests/worker/test_limits.py`（7 条）。
mod common;

use aite_contracts::{CardStatus, TaskStatus};
use common::*;
use serde_json::json;

fn loop_turn() -> aite_contracts::ModelTurn {
    tool_turn(&[("checklist_note", json!({"text": "再想想"}))])
}

#[tokio::test]
async fn step_limit_fails_at_max_steps() {
    let run = run_repeat(loop_turn(), 0.6).await;

    assert_eq!(run.h.config.worker.max_steps, 40);
    assert_eq!(run.model.call_count(), 40, "第 40 步之后不再调模型");
    assert_eq!(run.task.steps, 40);
    assert_eq!(run.task.status, TaskStatus::Failed);
    assert!(run.h.platform.last_text().contains("上限"));
    assert!(run.h.platform.last_text().contains(&run.task.task_no));
}

#[tokio::test]
async fn step_limit_closes_the_card() {
    let run = run_repeat(loop_turn(), 0.6).await;

    assert_eq!(run.h.platform.cards().len(), 1);
    // W4：结束时必定再更新一次
    assert_eq!(run.h.platform.last_card().status, CardStatus::Failed);
}

#[tokio::test]
async fn wall_clock_limit() {
    // 每步 700s、上限 1200s → 第 2 步之后超时收工
    let run = run_repeat(loop_turn(), 700.0).await;

    assert_eq!(run.h.config.worker.max_wall_sec, 1200);
    assert_eq!(run.model.call_count(), 2);
    assert_eq!(run.task.status, TaskStatus::Failed);
    assert!(run.h.platform.last_text().contains("上限"));
}

#[tokio::test]
async fn model_failure_retries_twice_then_fails() {
    // §3.3：模型异常重试 2 次（共 3 次调用）仍失败 → task failed +「模型服务暂不可用」
    let run = run_with(
        vec![final_turn("不会走到这")],
        None,
        0.0,
        "帮我出个图",
        Vec::new(),
        3,
    )
    .await;

    assert_eq!(run.model.call_count(), 3);
    assert_eq!(run.task.status, TaskStatus::Failed);
    assert!(run.h.platform.last_text().contains("模型服务暂不可用"));
    assert!(run.h.platform.last_text().contains(&run.task.task_no));
}

#[tokio::test]
async fn model_failure_recovers_within_retries() {
    let run = run_with(
        vec![final_turn("重试后成功了")],
        None,
        0.0,
        "帮我出个图",
        Vec::new(),
        2,
    )
    .await;

    assert_eq!(run.model.call_count(), 3);
    assert_eq!(run.task.status, TaskStatus::Delivered);
    assert_eq!(run.h.platform.last_text(), "重试后成功了");
}

#[tokio::test]
async fn repeated_invalid_args_fails() {
    // 连续 3 次不合 schema 的工具参数 → failed（§3.3）
    let bad = tool_turn(&[("checklist_add", json!({"items": []}))]);
    let run = run_repeat(bad, 0.6).await;

    assert_eq!(run.model.call_count(), 3);
    assert_eq!(run.task.status, TaskStatus::Failed);
    assert!(run.h.platform.last_text().contains("不合法的工具参数"));
}

#[tokio::test]
async fn text_only_after_first_step_is_nudged_not_delivered() {
    // §3.3：steps>0 时只回文本 → 提示它调 final，计 1 步，不当成交付
    let run = run_script(
        vec![
            tool_turn(&[("checklist_add", json!({"items": ["先想想"]}))]),
            text_turn("我觉得可以这样做"),
            final_turn("结论在这里"),
        ],
        0.6,
    )
    .await;

    assert_eq!(run.task.status, TaskStatus::Delivered);
    assert_eq!(run.model.call_count(), 3);
    assert_eq!(run.h.platform.last_text(), "结论在这里");
    assert_eq!(systems(&run.model.last_call(), "请调用 final").len(), 1);
}
