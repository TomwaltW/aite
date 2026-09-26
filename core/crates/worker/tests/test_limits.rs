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

// ---- CC3 ⑤ 重试分类 -------------------------------------------------------

/// 返回 (任务, 模型, harness, 这一趟在假钟上花掉的秒数)。
async fn run_failing(
    failures: Vec<FailWith>,
) -> (
    aite_contracts::Task,
    std::sync::Arc<ScriptedModel>,
    Harness,
    f64,
) {
    let mut h = Harness::new();
    h.seed("帮我出个图", Vec::new()).await;
    let model =
        std::sync::Arc::new(ScriptedModel::new(vec![final_turn("好了。")]).with_failures(failures));
    let started = h.clock.now();
    let task = h.run(model.clone()).await;
    let elapsed = h.clock.now() - started;
    (task, model, h, elapsed)
}

#[tokio::test]
async fn config_error_not_retried() {
    let (task, model, h, elapsed) =
        run_failing(vec![FailWith::Config("缺 AITE_MODEL_API_KEY".into())]).await;
    assert_eq!(model.call_count(), 1, "配置缺项再调一次也一样，不重试");
    assert_eq!(task.status, aite_contracts::TaskStatus::Failed);
    assert_eq!(h.platform.last_text(), "模型服务暂不可用，任务 #A1 已终止");
    assert_eq!(elapsed, 0.0, "一秒都没等");
}

#[tokio::test]
async fn http_4xx_not_retried() {
    for (status, sep) in [(400, ":"), (401, "："), (403, ":"), (404, "："), (422, ":")] {
        let (task, model, _h, _) = run_failing(vec![FailWith::Upstream(format!(
            "HTTP {status}{sep} bad request"
        ))])
        .await;
        assert_eq!(model.call_count(), 1, "HTTP {status} 不重试");
        assert_eq!(task.status, aite_contracts::TaskStatus::Failed);
    }
}

#[tokio::test]
async fn http_429_backs_off_with_retry_after() {
    let (task, model, _h, elapsed) = run_failing(vec![FailWith::Upstream(
        "HTTP 429: rate limited retry-after-ms=1500".into(),
    )])
    .await;
    assert_eq!(model.call_count(), 2, "第 2 次成功");
    assert_eq!(task.status, aite_contracts::TaskStatus::Delivered);
    assert_eq!(elapsed, 1.5, "恰好按 retry-after-ms 退避 1.5 秒");
}
