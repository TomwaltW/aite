//! 跑到一半被 `!stop` / 卡片 stop 打断（§3.3 + W4 的收尾更新）。
//! 移植自 `tests/worker/test_cancel.py`（2 条）。
mod common;

use aite_contracts::{CardStatus, EvidenceKind, EvidenceWriter, TaskStatus};
use common::*;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn stop_mid_run_cancels_at_next_step() {
    let mut base = Harness::new();
    base.seed("跑个长活", Vec::new()).await;
    let h = Arc::new(base);
    let hook_h = h.clone();
    let model = Arc::new(
        ScriptedModel::new(vec![
            tool_turn(&[("checklist_add", json!({"items": ["取数", "画图"]}))]),
            final_turn("不该走到这一步"),
        ])
        .with_clock(h.clock.clone(), 0.6)
        .with_on_call(Arc::new(move |step| {
            let h = hook_h.clone();
            Box::pin(async move {
                // 第一步刚开始跑，就有人点了 stop
                if step == 0 {
                    h.cancel();
                }
            })
        })),
    );
    let task = h.run(model.clone()).await;

    assert_eq!(task.status, TaskStatus::Cancelled);
    assert_eq!(model.call_count(), 1, "下一步开始前就停了");
    // 「任务 #A1 已停止。」那句回帖是控制面 `!stop` 分支发的（归 R4）：
    // worker 这一侧取消时不发任何文本，只收卡片与证据。
    assert!(h.platform.texts().is_empty());

    // W4：结束时必定再更新一次卡片，状态是 cancelled
    assert_eq!(h.platform.cards().len(), 1);
    assert_eq!(h.platform.last_card().status, CardStatus::Cancelled);

    // 控制面与 worker 不重复写 cancelled
    assert_eq!(h.evidence.count_kind(&task.id, EvidenceKind::Cancelled), 1);
    assert!(h.evidence.verify(&task.id));
    assert!(task.evidence_root_hash.is_some());
}

#[tokio::test]
async fn stop_before_the_first_step_never_calls_the_model() {
    // Python 那条（`test_stop_before_dispatch_never_runs_the_worker`）验的是控制面
    // 压根不派发（归 R4）。worker 这一侧对应的不变量：取消判定排在每步最开头，
    // 第一步之前就成立时，模型一次都不调、卡片一张都不发。
    let mut base = Harness::new();
    base.seed("马上就后悔", Vec::new()).await;
    let h = Arc::new(base);
    h.cancel();

    let model = Arc::new(ScriptedModel::new(vec![final_turn("不该跑")]));
    let task = h.run(model.clone()).await;

    assert_eq!(model.call_count(), 0);
    assert_eq!(task.status, TaskStatus::Cancelled);
    assert!(h.platform.cards().is_empty());
    assert_eq!(task.steps, 0);
}
