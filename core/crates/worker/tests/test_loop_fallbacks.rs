//! T20：同一张牌原样连发 → 第 3 次提示换招、第 5 次 failed（§3.3 那张表接不住的那种）。
//! 移植自 `tests/worker/test_loop_fallbacks.py`（13 条，其中指纹对拍那条 Python 是
//! 参数化 5 组，这里合成一条表驱动）。
//!
//! T17 实测 `04_csv_to_chart`：替身沙箱对不含 `savefig` 的代码一律回 `exit_code=0` + 空
//! stdout，模型自己诊断了几步之后对**逐字节相同**的 `run_python` 连发 33 次，一路烧到
//! `max_steps` 才停。§3.3 的四条兜底一条都接不住：参数完全合法、工具返回 `ok=true`、
//! 没超时、每次都有 tool_call。这里的 FakeGateway 对没预置的工具正好返回
//! `ok=true` + 一句没用的话，跟那个形状同构。
mod common;

use aite_contracts::{CardStatus, TaskStatus, ToolCallRequest};
use aite_worker::fingerprint::call_signature;
use aite_worker::{MAX_CONSECUTIVE_REPEATS, REPEAT_NUDGE_AT};
use common::*;
use serde_json::json;

/// T17 里那张牌：参数合法、工具成功、内容没用。
fn spin_args() -> serde_json::Value {
    json!({"code": "print(open('/work/in/file_sales_csv').read())"})
}

fn spin_turn() -> aite_contracts::ModelTurn {
    tool_turn(&[("run_python", spin_args())])
}

// ---- 兜底开火 -----------------------------------------------------------

#[tokio::test]
async fn repeated_gateway_call_fails_at_the_fifth() {
    // 连续第 5 次原样重复 → failed，不再等 max_steps
    let run = run_repeat(spin_turn(), 0.6).await;

    assert_eq!(MAX_CONSECUTIVE_REPEATS, 5);
    assert_eq!(run.model.call_count(), 5, "第 5 步之后不再调模型");
    assert_eq!(run.task.status, TaskStatus::Failed);
    let text = run.h.platform.last_text();
    assert!(text.contains("原样重复调用"), "实际是 {text}");
    assert!(text.contains("run_python"));
    assert!(text.contains(&run.task.task_no));
}

#[tokio::test]
async fn failing_early_saves_the_fifth_sandbox_run() {
    // 第 5 次在**执行之前**就收 —— 前 4 次结果一模一样，没必要再起一次沙箱执行
    let run = run_repeat(spin_turn(), 0.6).await;

    assert_eq!(run.h.gateway.call_names(), vec!["run_python"; 4]);
}

#[tokio::test]
async fn repeat_failure_closes_the_card() {
    // W4：终态照样把卡片收干净，只此一张
    let run = run_repeat(spin_turn(), 0.6).await;

    assert_eq!(run.h.platform.cards().len(), 1);
    assert_eq!(run.h.platform.last_card().status, CardStatus::Failed);
}

// ---- 提示换招 -----------------------------------------------------------

#[tokio::test]
async fn third_repeat_nudges_the_model_to_change_tack() {
    // 第 3 次回一条 system 提示：给结论 + 两个具体的下一步，而不是「你重复了」
    let run = run_repeat(spin_turn(), 0.6).await;

    let nudges = systems(&run.model.last_call(), "这条路走不通");
    assert_eq!(nudges.len(), 1);
    assert!(nudges[0].contains("run_python"));
    assert!(nudges[0].contains(&REPEAT_NUDGE_AT.to_string()));
    assert!(nudges[0].contains("换个做法"));
    assert!(nudges[0].contains("final"));
}

#[tokio::test]
async fn nudge_does_not_end_the_task() {
    // 提示只是提示：模型换招后照常交付
    let run = run_script(
        vec![
            spin_turn(),
            spin_turn(),
            spin_turn(),
            final_turn("换了个法子，拿到了"),
        ],
        0.6,
    )
    .await;

    assert_eq!(run.task.status, TaskStatus::Delivered);
    assert_eq!(run.h.platform.last_text(), "换了个法子，拿到了");
    assert_eq!(systems(&run.model.last_call(), "这条路走不通").len(), 1);
}

// ---- 「连续」的边界 -----------------------------------------------------

#[tokio::test]
async fn different_arguments_are_not_a_repeat() {
    // 参数不同就是另一张牌。正常探测会连着调同一个工具，不该被算成打转。
    let mut script: Vec<aite_contracts::ModelTurn> = (0..6)
        .map(|i| tool_turn(&[("run_python", json!({"code": format!("print({i})")}))]))
        .collect();
    script.push(final_turn("探完了"));
    let run = run_script(script, 0.6).await;

    assert_eq!(run.model.call_count(), 7);
    assert_eq!(run.task.status, TaskStatus::Delivered);
    assert_eq!(run.h.platform.last_text(), "探完了");
}

#[tokio::test]
async fn a_different_call_in_between_resets_the_run() {
    // 中间插进别的牌 → 清零。换招了说明模型还在推进，不算卡住。
    // 这里一共出了 5 张一样的 run_python，只是被 list_files 断成 2 + 3；
    // 不清零的话第 5 张就该 failed 了。
    let run = run_script(
        vec![
            spin_turn(),
            spin_turn(),
            tool_turn(&[("list_files", json!({}))]),
            spin_turn(),
            spin_turn(),
            spin_turn(),
            final_turn("绕过去了"),
        ],
        0.6,
    )
    .await;

    assert_eq!(run.model.call_count(), 7);
    assert_eq!(run.task.status, TaskStatus::Delivered);
    assert_eq!(run.h.platform.last_text(), "绕过去了");
}

#[tokio::test]
async fn two_identical_calls_in_one_step_count_twice() {
    // 一步出两张一样的牌算 2 次 —— 那比隔了一步再重复更卡
    let double = tool_turn(&[("run_python", spin_args()), ("run_python", spin_args())]);
    let run = run_repeat(double, 0.6).await;

    assert_eq!(run.model.call_count(), 3, "1,2 / 3,4 / 第 5 张开火");
    assert_eq!(run.h.gateway.call_names(), vec!["run_python"; 4]);
    assert_eq!(run.task.status, TaskStatus::Failed);
    assert!(run.h.platform.last_text().contains("原样重复调用"));
}

#[tokio::test]
async fn counter_survives_a_text_only_step() {
    // 只回文本的那一步不出牌，也就不打断连发（观测侧也只摊平 tool_calls）
    let run = run_script(
        vec![
            spin_turn(),
            spin_turn(),
            text_turn("我再想想"),
            spin_turn(),
            spin_turn(),
            spin_turn(),
        ],
        0.6,
    )
    .await;

    assert_eq!(run.model.call_count(), 6);
    assert_eq!(run.task.status, TaskStatus::Failed);
    assert!(run.h.platform.last_text().contains("原样重复调用"));
}

// ---- 不抢别人的活 -------------------------------------------------------

#[tokio::test]
async fn local_tool_spin_is_still_the_step_limit_case() {
    // 连发 `checklist_note` 归 `max_steps`，不归这条兜底。
    // §3.8 08_step_limit 的 verifies 就是「模型只会重复 checklist_note → max_steps 处
    // failed」，回帖里那句「上限」是它的判据之一。本地工具照样进计数（口径要和观测侧
    // 对得上），但不由它开火。
    let run = run_repeat(
        tool_turn(&[("checklist_note", json!({"text": "再想想"}))]),
        0.6,
    )
    .await;

    assert_eq!(run.h.config.worker.max_steps, 40);
    assert_eq!(run.model.call_count(), 40);
    assert_eq!(run.task.status, TaskStatus::Failed);
    assert!(run.h.platform.last_text().contains("上限"));
    assert!(!run.h.platform.last_text().contains("原样重复调用"));
}

#[tokio::test]
async fn repeated_invalid_final_is_reported_as_invalid_args() {
    // `final` 也是本地工具：原样重发不合法的 final 由 invalid_args 在第 3 次接住，
    // 诊断比「原地打转」更具体。
    let run = run_repeat(tool_turn(&[("final", json!({"reply": "   "}))]), 0.6).await;

    assert_eq!(run.model.call_count(), 3);
    assert_eq!(run.task.status, TaskStatus::Failed);
    assert!(run.h.platform.last_text().contains("不合法的工具参数"));
}

// ---- 与观测侧的口径 -----------------------------------------------------

#[test]
fn signature_matches_the_pinned_vectors() {
    // 兜底侧和观测侧对「同一张牌」必须是同一个口径 —— Rust 里两边直接用同一个函数
    // （R7 的 protocol probe `use aite_worker::fingerprint`），所以这里钉的是**向量本身**：
    // 格式变了（比如分隔符改成无空格）要在本轨就红，免得报告和实现悄悄错开。
    let cases: [(&str, serde_json::Value, &str); 5] = [
        (
            "run_python",
            json!({"code": "print(1)"}),
            r#"run_python:{"code": "print(1)"}"#,
        ),
        (
            // 顶层 key 顺序无关：一律按 key 排序
            "run_python",
            json!({"timeout": 30, "code": "print(1)"}),
            r#"run_python:{"code": "print(1)", "timeout": 30}"#,
        ),
        ("list_files", json!({}), "list_files:{}"),
        (
            // 非 ASCII 不转义
            "checklist_add",
            json!({"items": ["甲", "乙"]}),
            r#"checklist_add:{"items": ["甲", "乙"]}"#,
        ),
        (
            "final",
            json!({"reply": "好", "artifacts": [{"path": "/work/a.png"}]}),
            r#"final:{"artifacts": [{"path": "/work/a.png"}], "reply": "好"}"#,
        ),
    ];
    for (name, args, want) in cases {
        let call = ToolCallRequest {
            call_id: "c1".into(),
            name: name.to_string(),
            arguments: args.as_object().cloned().unwrap_or_default(),
        };
        assert_eq!(call_signature(&call), want);
    }
}

#[test]
fn signature_survives_exotic_arguments() {
    // 真模型给回来的参数是 JSON 解出来的，理论上一定可序列化；万一形状很怪，
    // 指纹只能退化成一个稳定的字符串，不能把整个 worker 掀了。
    let weird = ToolCallRequest {
        call_id: "c1".into(),
        name: "run_python".into(),
        arguments: json!({"code": {"nested": [1, 2, null, true]}})
            .as_object()
            .cloned()
            .unwrap_or_default(),
    };
    let sig = call_signature(&weird);
    assert!(sig.starts_with("run_python:"));
    // 同一份参数两次算出来必须一样，否则重复检测永远不会开火
    assert_eq!(sig, call_signature(&weird));
}
