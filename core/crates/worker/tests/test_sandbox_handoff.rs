//! 沙箱归属：Gateway 建的沙箱，worker 取产物要用它、任务收尾要还它。
//! 移植自 `tests/worker/test_sandbox_handoff.py`（6 条）。
//!
//! 旧契约的 `ToolGateway` 只有 catalog / call，沙箱归属没进协议 —— Gateway 按 task_id
//! 自己记着容器，`Task.sandbox_id` 记的则是 worker 兜底建的那种。p0.2 把
//! `sandbox_id_of / release_task` 显式化了（§3.1 差异表 D2），但两边不通气的后果不变：
//!
//! 1. **产物丢**：run_python 把 /work/out.png 写在 Gateway 那个沙箱里，worker 取产物时
//!    看 `task.sandbox_id is None`，自己 acquire 一个全新的空容器，产物必然找不到。
//! 2. **容器泄漏**：Gateway 手上那个沙箱没人 release，只能等 reaper 的 idle_sec 空闲超时
//!    才被收 —— 任务早结束了还占着内存和 CPU 配额。
mod common;

use aite_contracts::TaskStatus;
use common::*;
use serde_json::json;
use std::sync::Arc;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

fn chart_script() -> Vec<aite_contracts::ModelTurn> {
    vec![
        tool_turn(&[(
            "run_python",
            json!({"code": "plt.savefig('/work/out.png')"}),
        )]),
        final_turn_with(
            "图在这里。",
            json!([{"path": "/work/out.png", "title": "月度趋势"}]),
        ),
    ]
}

// ---- 取产物 -------------------------------------------------------------

#[tokio::test]
async fn artifact_is_fetched_from_the_gateway_sandbox() {
    // 产物在 Gateway 那个沙箱里 —— worker 得问它要，不能自己另开一个
    let mut h = Harness::new();
    h.sandbox.put("/work/out.png", PNG);
    h.seed("帮我出个图", Vec::new()).await;
    let model = Arc::new(ScriptedModel::new(chart_script()).with_clock(h.clock.clone(), 0.6));
    let task = h.run(model).await;

    let used: Vec<String> = h
        .sandbox
        .get_file_calls()
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    assert_eq!(used, ["sb_gateway"], "取的是 Gateway 那个");
    assert!(h.sandbox.acquired().is_empty(), "worker 没有另开");
    assert_eq!(h.platform.files().len(), 1);
    assert_eq!(h.platform.files()[0].data, PNG);
    assert_eq!(task.status, TaskStatus::Delivered);
}

#[tokio::test]
async fn worker_still_acquires_when_gateway_holds_nothing() {
    // 兜底路径不变：Gateway 没建过沙箱（比如产物是别处放的），worker 照旧自己建
    let mut h = Harness::new();
    h.sandbox.put("/work/out.txt", b"hi");
    h.seed("帮我出个图", Vec::new()).await;
    let model = Arc::new(ScriptedModel::new(vec![final_turn_with(
        "给你。",
        json!([{"path": "/work/out.txt", "title": "结果"}]),
    )]));
    let task = h.run(model).await;

    assert_eq!(h.sandbox.acquired().len(), 1);
    let used: Vec<String> = h
        .sandbox
        .get_file_calls()
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    assert_eq!(used, h.sandbox.acquired());
    assert_eq!(h.platform.files().len(), 1);
    assert_eq!(task.status, TaskStatus::Delivered);
}

// ---- 还沙箱 -------------------------------------------------------------

#[tokio::test]
async fn delivered_task_returns_the_gateway_sandbox() {
    // 任务交付完，Gateway 的沙箱当场还掉，不等 reaper
    let mut h = Harness::new();
    h.sandbox.put("/work/out.png", PNG);
    h.seed("帮我出个图", Vec::new()).await;
    let model = Arc::new(ScriptedModel::new(chart_script()).with_clock(h.clock.clone(), 0.6));
    let task = h.run(model).await;

    assert_eq!(task.status, TaskStatus::Delivered);
    assert_eq!(h.gateway.released_tasks(), vec![task.id.clone()]);
    assert_eq!(h.sandbox.released(), ["sb_gateway"]);
    assert_eq!(
        aite_contracts::ToolGateway::sandbox_id_of(&*h.gateway, &task.id).await,
        None
    );
}

#[tokio::test]
async fn failed_task_returns_the_gateway_sandbox() {
    // failed 也走同一个收尾口，沙箱一样要还（这里由 T20 的第 5 次重复开火）
    let run = run_repeat(tool_turn(&[("run_python", json!({"code": "1/0"}))]), 0.6).await;

    assert_eq!(run.task.status, TaskStatus::Failed);
    assert_eq!(run.h.gateway.released_tasks(), vec![run.task.id.clone()]);
    assert_eq!(run.h.sandbox.released(), ["sb_gateway"]);
}

#[tokio::test]
async fn cancel_mid_run_returns_the_gateway_sandbox() {
    // 跑到一半被 !stop：任务在 worker 手里，收尾由 worker 做，沙箱在那里还
    let mut h = Harness::new();
    h.seed("跑个长活", Vec::new()).await;
    let h = Arc::new(h);
    let hook_h = h.clone();
    let model = Arc::new(
        ScriptedModel::new(vec![
            tool_turn(&[("run_python", json!({"code": "长活"}))]),
            // 跑完这步回到循环开头才检查取消
            tool_turn(&[("checklist_note", json!({"text": "还在算"}))]),
            final_turn("不该走到这一步"),
        ])
        .with_clock(h.clock.clone(), 0.6)
        .with_on_call(Arc::new(move |step| {
            let h = hook_h.clone();
            Box::pin(async move {
                // 第一步（run_python）已经建了沙箱，这时点 stop
                if step == 1 {
                    h.cancel();
                }
            })
        })),
    );
    let task = h.run(model).await;

    assert_eq!(task.status, TaskStatus::Cancelled);
    assert_eq!(h.gateway.released_tasks(), vec![task.id.clone()]);
    assert_eq!(h.sandbox.released(), ["sb_gateway"]);
}

#[tokio::test]
async fn cancel_before_the_first_step_returns_the_gateway_sandbox() {
    // Python 那条走的是控制面「任务还没被 worker 领走」的分支（沙箱由控制面还，归 R4）。
    // worker 这一侧对应的不变量是：取消旗在第一步之前就竖着时，模型一次都不调，
    // 但 cancelled 证据、卡片与沙箱归还一样都不少。
    let mut h = Harness::new();
    h.seed("先建个任务", Vec::new()).await;
    h.gateway.hold_sandbox(&h.task.id); // 演上一轮工具调用留下的沙箱
    h.cancel();
    let model = Arc::new(ScriptedModel::new(vec![final_turn("不该跑到这")]));
    let task = h.run(model.clone()).await;

    assert_eq!(model.call_count(), 0);
    assert_eq!(task.status, TaskStatus::Cancelled);
    assert!(h.platform.cards().is_empty(), "没跑到发卡片那一步");
    assert_eq!(h.gateway.released_tasks(), vec![task.id.clone()]);
    assert_eq!(h.sandbox.released(), ["sb_gateway"]);
}
