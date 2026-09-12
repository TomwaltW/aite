//! FakeModel 自测（移植自 `tests/e2e/test_t4_fake_model.py`，20 条）：
//! 脚本化出牌、repeat、hold、那条纯文本兜底的牌造得出来。
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aite_contracts::{Message, ModelPort, PlatformPort, Role, Usage, all_model_tools};
use aite_testing::{FakeModel, ScriptStep};
use serde_json::json;

fn msgs() -> Vec<Message> {
    vec![Message::text(Role::User, "你好")]
}

async fn chat(model: &FakeModel) -> Result<aite_contracts::ModelTurn, aite_contracts::ModelError> {
    model.chat(&msgs(), all_model_tools(), 4096, 0.0).await
}

fn step(v: serde_json::Value) -> ScriptStep {
    ScriptStep::from_value(v).expect("脚本步骤")
}

fn model(steps: Vec<serde_json::Value>) -> FakeModel {
    FakeModel::new(steps.into_iter().map(step).collect())
}

/// `ModelPort::name` 对齐；trait 实现由编译期保证。
#[tokio::test]
async fn signature_matches_model_port() {
    let m = FakeModel::new(vec![]);
    assert_eq!(ModelPort::name(&m), "scripted");
    let named = FakeModel::new(vec![]).named("deepseek-chat");
    assert_eq!(ModelPort::name(&named), "deepseek-chat");
}

#[tokio::test]
async fn tool_call_step_becomes_assistant_message() {
    let m = model(vec![
        json!({"tool_calls": [{"name": "final", "arguments": {"reply": "好了"}}]}),
    ]);
    let turn = chat(&m).await.unwrap();
    assert_eq!(turn.message.role, Role::Assistant);
    assert_eq!(turn.finish_reason, "tool_calls");
    let calls = turn.message.tool_calls.unwrap();
    assert_eq!(
        calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["final"]
    );
    assert_eq!(calls[0].arguments["reply"], json!("好了"));
    assert_eq!(calls[0].call_id, "call_0_0");
}

/// 那条兜底牌：既无 tool_call 也无 final，只有文本。
/// 怎么兜底归 worker（R5）；替身负责的是把这种出牌造出来。
#[tokio::test]
async fn bare_text_step_has_no_tool_calls() {
    let m = model(vec![json!({"text": "北京今天晴。"})]);
    let turn = chat(&m).await.unwrap();
    assert!(turn.message.tool_calls.is_none());
    assert_eq!(turn.message.content, "北京今天晴。");
    assert_eq!(turn.finish_reason, "stop");
}

#[tokio::test]
async fn multiple_tool_calls_in_one_step() {
    let m = model(vec![json!({"tool_calls": [
        {"name": "checklist_check", "arguments": {"id": "c1"}},
        {"name": "checklist_check", "arguments": {"id": "c2"}}
    ]})]);
    let turn = chat(&m).await.unwrap();
    let calls = turn.message.tool_calls.unwrap();
    assert_eq!(
        calls.iter().map(|c| c.call_id.as_str()).collect::<Vec<_>>(),
        ["call_0_0", "call_0_1"]
    );
}

#[tokio::test]
async fn steps_are_served_in_order() {
    let m = model(vec![
        json!({"text": "一"}),
        json!({"text": "二"}),
        json!({"text": "三"}),
    ]);
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(chat(&m).await.unwrap().message.content);
    }
    assert_eq!(got, ["一", "二", "三"]);
    assert_eq!(m.call_count(), 3);
}

#[tokio::test]
async fn repeat_n_then_moves_on() {
    let m = model(vec![
        json!({"text": "重复", "repeat": 2}),
        json!({"text": "下一步"}),
    ]);
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(chat(&m).await.unwrap().message.content);
    }
    assert_eq!(got, ["重复", "重复", "下一步"]);
}

/// 08_step_limit 靠它把模型钉死在 checklist_note 上。
#[tokio::test]
async fn repeat_inf_never_runs_out() {
    let m = model(vec![json!({
        "tool_calls": [{"name": "checklist_note", "arguments": {"text": "再想想"}}],
        "repeat": "inf"
    })]);
    for _ in 0..50 {
        let turn = chat(&m).await.unwrap();
        assert_eq!(turn.message.tool_calls.unwrap()[0].name, "checklist_note");
    }
}

/// 脚本用尽要给一句能读的话，别让场景报告里出现异常栈。
#[tokio::test]
async fn exhausted_script_says_so_in_plain_words() {
    let m = model(vec![json!({"text": "只有一步"})]);
    chat(&m).await.unwrap();
    let err = chat(&m).await.unwrap_err();
    assert!(err.to_string().contains("模型脚本已用尽"), "{err}");
    assert!(err.to_string().contains("repeat: inf"), "{err}");
}

/// 「模型调用异常 / 5xx」这条路要能演。
#[tokio::test]
async fn error_step_raises_model_error() {
    let m = model(vec![json!({"error": "上游 503"})]);
    let err = chat(&m).await.unwrap_err();
    assert!(err.to_string().contains("上游 503"), "{err}");
    assert_eq!(
        m.calls.last("chat").unwrap().error.as_deref(),
        Some("上游 503")
    );
}

#[tokio::test]
async fn usage_is_carried_through() {
    let m = model(vec![
        json!({"text": "x", "usage": {"input_tokens": 120, "output_tokens": 30}}),
    ]);
    let turn = chat(&m).await.unwrap();
    assert_eq!(
        turn.usage,
        Usage {
            input_tokens: 120,
            output_tokens: 30,
            cached_tokens: 0
        }
    );
}

#[tokio::test]
async fn calls_record_what_worker_sent() {
    let m = model(vec![json!({"text": "x"})]);
    m.chat(&msgs(), all_model_tools(), 1024, 0.7).await.unwrap();
    let call = m.calls.last("chat").unwrap();
    assert_eq!(call.arg("max_tokens").unwrap().as_u64(), Some(1024));
    assert!((call.arg("temperature").unwrap().as_f64().unwrap() - 0.7).abs() < 1e-6);
    assert_eq!(call.arg("roles").unwrap(), &json!(["user"]));
    let tools = call.arg("tools").unwrap().as_array().unwrap();
    assert!(tools.contains(&json!("final")));
}

#[tokio::test]
async fn tool_names_emitted_counts_local_and_gateway_tools() {
    let m = model(vec![
        json!({"tool_calls": [{"name": "checklist_add", "arguments": {"items": ["a"]}}]}),
        json!({"tool_calls": [{"name": "run_python", "arguments": {"code": "x"}}]}),
        json!({"tool_calls": [{"name": "final", "arguments": {"reply": "done"}}]}),
    ]);
    for _ in 0..3 {
        chat(&m).await.unwrap();
    }
    assert_eq!(
        m.tool_names_emitted(),
        vec![
            "checklist_add".to_string(),
            "run_python".to_string(),
            "final".to_string()
        ]
    );
}

#[test]
fn bad_repeat_is_rejected_at_load_time() {
    let err = ScriptStep::from_value(json!({"text": "x", "repeat": 0})).unwrap_err();
    assert!(err.contains("repeat"), "{err}");
    let err = ScriptStep::from_value(json!({"text": "x", "repeat": "forever"})).unwrap_err();
    assert!(err.contains("repeat"), "{err}");
}

// --- 卡住一步（hold_ticks）---------------------------------------------------
//
// 这一组钉的是能力本身，不是哪个场景：替身全是瞬时返回的，一次 chat() 里没有任何真会
// 挂起的 await 点，worker 被调度上就会一口气跑到步数上限。凡是「任务还活着的时候才有
// 意义」的验证（!status / !stop）都要靠这一步卡住，别的任务才轮得上跑。

/// 卡住的这一步没返回之前，别的任务能跑完自己的事 —— 07_commands 靠的就是这条。
#[tokio::test]
async fn hold_ticks_lets_another_task_run_before_the_turn_comes_back() {
    let m = Arc::new(model(vec![json!({"text": "终于出牌", "hold_ticks": 50})]));
    let marks = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

    let meanwhile = {
        let marks = marks.clone();
        tokio::spawn(async move {
            for i in 0..3 {
                tokio::task::yield_now().await;
                marks.lock().unwrap().push(format!("别的任务{i}"));
            }
        })
    };
    let ask = {
        let m = m.clone();
        let marks = marks.clone();
        tokio::spawn(async move {
            let turn = chat(&m).await.unwrap();
            marks.lock().unwrap().push("chat 返回".to_string());
            turn.message.content
        })
    };

    let (content, _) = tokio::join!(ask, meanwhile);
    assert_eq!(content.unwrap(), "终于出牌");
    let marks = marks.lock().unwrap().clone();
    assert_eq!(&marks[..3], ["别的任务0", "别的任务1", "别的任务2"]);
    assert_eq!(
        marks.last().unwrap(),
        "chat 返回",
        "chat 没被卡住：{marks:?}"
    );
}

/// 等的那件事永远不发生也不能挂死：让满 N 次就照常出牌。
#[tokio::test]
async fn hold_lets_go_when_the_budget_runs_out() {
    let m = model(vec![json!({"text": "牌还是出了", "hold_ticks": 5})]);
    let turn = chat(&m).await.unwrap();
    assert_eq!(turn.message.content, "牌还是出了");
    assert_eq!(m.holds(), 1);
    assert_eq!(m.hold_ticks_yielded(), 5);
}

/// 确定性的根据：让出的是调度 tick，不是 sleep 真实时间。
///
/// 判据分两条，各挡一类「偷偷睡过去」：
///
/// * **虚拟时钟不动**（主判据，确定性）：`start_paused` 把 tokio 时钟冻住，
///   只有运行时无事可做、且有 timer 在等的时候才会自动把它推到下一个到期点。
///   `yield_now` 让出的任务立刻又可运行，永远不会触发这个自动推进 —— 于是
///   「两万个 tick 之后虚拟时钟纹丝不动」就等价于「实现里没有 `tokio::time::sleep`
///   这类 timer」。这条判据只看时钟有没有跳，和 CPU 忙闲无关，不会随机抖。
/// * **真实时间兜底**（粗判据）：`std::thread::sleep` 之类的阻塞不走 tokio timer，
///   虚拟时钟照样不动，只能拿墙钟抓。阈值取 10s —— 两万次 yield 空闲时是毫秒量级、
///   满载时也就百毫秒量级，离 10s 远得离谱，所以它只在真有秒级阻塞时才响。
#[tokio::test(start_paused = true)]
async fn hold_spends_scheduler_ticks_not_wall_clock() {
    let m = model(vec![json!({"text": "x", "hold_ticks": 20000})]);
    let virtual_start = tokio::time::Instant::now();
    let wall_start = std::time::Instant::now();
    chat(&m).await.unwrap();
    assert_eq!(m.hold_ticks_yielded(), 20000);

    let virtual_elapsed = virtual_start.elapsed();
    assert_eq!(
        virtual_elapsed,
        Duration::ZERO,
        "虚拟时钟走了 {virtual_elapsed:?}：hold 里有 tokio timer sleep，不是纯调度 tick"
    );
    let wall_elapsed = wall_start.elapsed();
    assert!(
        wall_elapsed < Duration::from_secs(10),
        "两万个 tick 花了 {wall_elapsed:?}：八成有 std::thread::sleep 这类阻塞（虚拟时钟抓不到）"
    );
}

/// 要等的事已经发生了就别再空转：release_holds() 让当前这一步立刻出牌。
#[tokio::test]
async fn release_holds_cuts_a_long_hold_short() {
    let m = Arc::new(model(vec![json!({"text": "x", "hold_ticks": 10000000})]));
    let releaser = {
        let m = m.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            m.release_holds();
        })
    };
    let asker = {
        let m = m.clone();
        tokio::spawn(async move { chat(&m).await.unwrap().message.content })
    };
    let (turn, _) = tokio::join!(asker, releaser);
    assert_eq!(turn.unwrap(), "x");
    assert!(m.hold_ticks_yielded() < 10_000_000);
}

/// repeat: inf 是「无限出牌」，hold_ticks 是「这一次不返回」—— 正交，可以叠。
#[tokio::test]
async fn hold_and_repeat_inf_are_two_different_knobs() {
    let m = model(vec![json!({
        "tool_calls": [{"name": "checklist_note", "arguments": {"text": "再想想"}}],
        "hold_ticks": 3,
        "repeat": "inf"
    })]);
    for _ in 0..4 {
        let turn = chat(&m).await.unwrap();
        assert_eq!(turn.message.tool_calls.unwrap()[0].name, "checklist_note");
    }
    assert_eq!(m.holds(), 4);
    assert_eq!(m.hold_ticks_yielded(), 12);
}

/// 默认不挂：没写 hold_ticks 的步骤一个 tick 都不该让。
#[tokio::test]
async fn no_hold_by_default_costs_nothing() {
    assert_eq!(step(json!({"text": "x"})).hold_ticks, 0);
    let m = model(vec![json!({"text": "x"})]);
    chat(&m).await.unwrap();
    assert_eq!(m.holds(), 0);
    assert_eq!(m.hold_ticks_yielded(), 0);
}

#[test]
fn negative_hold_ticks_is_rejected_at_load_time() {
    let err = ScriptStep::from_value(json!({"text": "x", "hold_ticks": -1})).unwrap_err();
    assert!(err.contains("hold_ticks"), "{err}");
}

/// CallLog 的 seq 是全局单调的：跨替身也能排出先后（清单 §11 第 24 条）。
#[tokio::test]
async fn call_log_seq_is_globally_monotonic() {
    let platform = aite_testing::FakePlatform::new();
    let m = model(vec![json!({"text": "x"})]);
    platform
        .send_text(&aite_contracts::OutboundText::new("oc_1", "先"))
        .await
        .unwrap();
    chat(&m).await.unwrap();
    platform
        .send_text(&aite_contracts::OutboundText::new("oc_1", "后"))
        .await
        .unwrap();

    let texts = platform.calls.of("send_text");
    let chat_seq = m.calls.last("chat").unwrap().seq;
    assert!(texts[0].seq < chat_seq && chat_seq < texts[1].seq);
}

/// 静态计数器共享：两个 FakeModel 的 seq 也不会撞。
#[tokio::test]
async fn two_models_do_not_share_a_cursor() {
    let a = model(vec![json!({"text": "a"})]);
    let b = model(vec![json!({"text": "b"})]);
    let counter = AtomicUsize::new(0);
    for m in [&a, &b] {
        let turn = chat(m).await.unwrap();
        counter.fetch_add(turn.message.content.len(), Ordering::SeqCst);
    }
    assert_eq!(a.turns_served(), 1);
    assert_eq!(b.turns_served(), 1);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}
