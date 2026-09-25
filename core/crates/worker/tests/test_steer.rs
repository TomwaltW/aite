//! R6 排队的 steer 消息，worker 每步开始前合并进上下文。
//! 移植自 `tests/worker/test_steer.py`（7 条）。
//!
//! 主干那条（第 1 步跑起来后往话题里塞一句不带 @ 的话 → 下一步进上下文）在第一个用例。
//! 其余是读实现时看得见、但没人验过的合并侧边界：连发几句的顺序、在 W1 那五层里插在哪、
//! 排队时机与 transcript 撞车、`!stop` 抢在 drain 前面、证据链上看不看得见、超长不截断。
//!
//! 排队侧（排给谁、任务没跑起来时队列谁清）在 R4 的 `tests/control/`。
mod common;

use aite_contracts::{Role, TaskStatus};
use aite_worker::context::{ATTACHMENT_HEADER, HISTORY_HEADER};
use common::*;
use serde_json::json;
use std::sync::Arc;

const STEER: &str = "顺便加上同比";

/// CC3 ④ 起进上下文的 User 行带署名。Harness 里所有 turn 都是发起人 `ou_user` 说的，
/// `run()` 传进来的发起人显示名是「张三」。
fn signed(text: &str) -> String {
    format!("[张三] {text}")
}

fn three_step_script(last: &str) -> Vec<aite_contracts::ModelTurn> {
    vec![
        tool_turn(&[("checklist_add", json!({"items": ["取数"]}))]),
        tool_turn(&[("checklist_check", json!({"id": "c1"}))]),
        final_turn(last),
    ]
}

/// 在第 `step` 步开始前把这些追问排进来（ScriptedModel 的 on_call 钩子）。
fn inject_at(h: Arc<Harness>, step: usize, texts: Vec<String>) -> OnCall {
    Arc::new(move |current| {
        let h = h.clone();
        let texts = texts.clone();
        Box::pin(async move {
            if current == step {
                for t in texts {
                    h.push_steer(&t).await;
                }
            }
        })
    })
}

async fn seeded(text: &str, attachments: Vec<aite_contracts::Attachment>) -> Arc<Harness> {
    let mut h = Harness::new();
    h.seed(text, attachments).await;
    Arc::new(h)
}

#[tokio::test]
async fn steer_message_reaches_the_next_step() {
    let h = seeded("按月画个图", Vec::new()).await;
    let model = Arc::new(
        ScriptedModel::new(three_step_script("加上同比之后的结果。"))
            .with_clock(h.clock.clone(), 0.6)
            .with_on_call(inject_at(h.clone(), 1, vec![STEER.to_string()])),
    );
    h.run(model.clone()).await;

    // 第 2 步（下标 1）之前注入，所以第 3 步（下标 2）的上下文里必须有它
    let step3 = model.call(2);
    assert!(
        step3
            .iter()
            .any(|m| m.role == Role::User && m.content == signed(STEER))
    );
    // CC3 ④ 改写的否定断言：原来比 `content == STEER`，署名后它永远成立（空转）；
    // 改成「任何一条 User 行里都不含这句」，比原来还严
    assert!(
        !model
            .call(1)
            .iter()
            .any(|m| m.role == Role::User && m.content.contains(STEER))
    );

    assert!(h.pending_steer().is_empty(), "已被消费");
    // CC3 ③ 起交付会多一条助手轮；这里钉的是「用户说过的话」，只看 User turn
    assert_eq!(
        h.store.user_turn_texts(&h.session.id),
        ["按月画个图", STEER]
    );
}

#[tokio::test]
async fn several_steer_messages_keep_their_order_as_separate_turns() {
    // 连发三句：drain 原序出来，合并成三条独立的 user 消息而不是并成一条。
    // 并成一条的话模型看到的是一段拼接文本，分不出这是三次追问；顺序反了更糟 ——
    // 「不要月度」「改成季度」倒过来读意思是反的。
    let texts = vec![
        "顺便加上同比".to_string(),
        "不要月度了".to_string(),
        "改成季度".to_string(),
    ];
    let h = seeded("按月画个图", Vec::new()).await;
    let model = Arc::new(
        ScriptedModel::new(three_step_script("按季度算好了。")).with_on_call(inject_at(
            h.clone(),
            1,
            texts.clone(),
        )),
    );
    h.run(model.clone()).await;

    let step3 = model.call(2);
    let tail: Vec<String> = step3[step3.len() - 3..]
        .iter()
        .map(|m| m.content.clone())
        .collect();
    let want_tail: Vec<String> = texts.iter().map(|t| signed(t)).collect();
    assert_eq!(tail, want_tail, "原序，且是尾巴上连着的三条（各自署名）");
    assert!(
        step3[step3.len() - 3..]
            .iter()
            .all(|m| m.role == Role::User)
    );
    assert!(h.pending_steer().is_empty());

    let mut want = vec!["按月画个图".to_string()];
    want.extend(texts);
    // CC3 ③：同上，只看 User turn
    assert_eq!(h.store.user_turn_texts(&h.session.id), want);
}

#[tokio::test]
async fn steer_lands_at_the_tail_after_the_history_and_attachment_blocks() {
    // W1 的顺序是 system → transcript → 群历史 → 附件清单 → 工具目录。
    // steer 是**运行中新到的指令**，不是历史的一部分，所以只能落在这五层之后、
    // 上一步的 assistant/tool 消息之后 —— 也就是消息列表的最末尾。插进 transcript
    // 那一层的话，模型读到的就是「用户当时说过这么一句」而不是「现在要求改了」。
    let mut base = Harness::new();
    base.platform
        .set_history(history(&[("om_h1", "human", "李四", "这周的数在共享盘")]));
    base.seed(
        "按月画个图",
        vec![attachment("fk_1", "sales.csv", Some(64))],
    )
    .await;
    let h = Arc::new(base);
    let model = Arc::new(
        ScriptedModel::new(three_step_script("好了。")).with_on_call(inject_at(
            h.clone(),
            1,
            vec![STEER.to_string()],
        )),
    );
    h.run(model.clone()).await;

    let step3 = model.call(2);
    let idx = |needle: &str| {
        step3
            .iter()
            .position(|m| m.content.contains(needle))
            .expect("上下文里找不到这一块")
    };
    let h_idx = idx(HISTORY_HEADER);
    let a_idx = idx(ATTACHMENT_HEADER);
    let s_idx = step3
        .iter()
        .position(|m| m.content == signed(STEER))
        .expect("steer 没进上下文");

    assert!(h_idx < a_idx && a_idx < s_idx);
    assert_eq!(s_idx, step3.len() - 1, "就在最末尾");
    // transcript 那一层还是任务开跑时的样子，没被 steer 挤进去
    assert_eq!(step3[1].role, Role::User);
    assert_eq!(step3[1].content, signed("按月画个图"));
}

#[tokio::test]
async fn steer_queued_before_the_task_starts_is_not_injected_twice() {
    // 任务还躺在队列里（status=created）就来的追问：`_continue_session` 先 append_turn
    // 落库、再排队，而 worker 是开跑时才去 list_turns —— transcript 里本来就有这句话了，
    // 队列里那份再合并一次就是同一句话进两遍。清队列那一下归控制面 `_dispatch_task`
    // （R4），这里显式调 `clear_steer()` 复刻它，验的是 worker 侧不会另外再补一条。
    let h = seeded("按月画个图", Vec::new()).await;
    h.push_steer(STEER).await; // 任务还没被 worker 领走
    assert_eq!(h.pending_steer(), [STEER], "排队侧照 R6 排上了");
    h.clear_steer();

    let model = Arc::new(ScriptedModel::new(vec![final_turn("算好了。")]));
    h.run(model.clone()).await;

    let step1 = model.call(0);
    // CC3 ④ 改写：数「含这句话」的条数（署名后原来的 `== STEER` 会数成 0）
    let hits = step1.iter().filter(|m| m.content.contains(STEER)).count();
    assert_eq!(hits, 1, "同一句话不许进两遍");
    assert!(h.pending_steer().is_empty());
    // 进上下文的那一份来自 transcript，位置在 system prompt 之后的第二条
    assert_eq!(step1[1].content, signed("按月画个图"));
    assert_eq!(step1[2].content, signed(STEER));
}

#[tokio::test]
async fn stop_before_the_drain_keeps_the_steer_out_of_the_model() {
    // steer 排着队时 !stop 掉这个任务：主循环的取消判定在 drain 前面，
    // 所以这句话不会再进模型；队列也要跟着清掉，不许留在内存里。
    let h = seeded("按月画个图", Vec::new()).await;
    let hook_h = h.clone();
    let model = Arc::new(
        ScriptedModel::new(three_step_script("不该走到这里。")).with_on_call(Arc::new(
            move |step| {
                let h = hook_h.clone();
                Box::pin(async move {
                    if step == 1 {
                        h.push_steer(STEER).await;
                        h.cancel();
                    }
                })
            },
        )),
    );
    let task = h.run(model.clone()).await;

    assert_eq!(model.call_count(), 2, "第 3 步在取消判定处就返回了");
    assert!(
        !model
            .calls()
            .iter()
            // CC3 ④ 改写的否定断言：署名后 `== STEER` 会空转，改成 contains
            .any(|call| call.iter().any(|m| m.content.contains(STEER)))
    );
    assert!(h.pending_steer().is_empty());
    assert_eq!(task.status, TaskStatus::Cancelled);
}

#[tokio::test]
async fn steer_is_recorded_in_the_evidence_chain() {
    // T24 关掉了 T14 在这里留的口子：W8 只点名 model_call / tool_call / tool_result /
    // checklist_op 四类，steer 不在其中，于是 evidence_show 的时间线上缺了
    // 「任务跑到一半用户改了要求」这一段 —— 而 model_call 只记 messages_hash，
    // 原文不落盘，从证据里也反推不出来。补法：排 steer 时给目标任务写一条
    // event_received（不新增 EvidenceKind，那是冻结契约），payload 带 route: "steer"。
    let h = seeded("按月画个图", Vec::new()).await;
    let model = Arc::new(
        ScriptedModel::new(three_step_script("好了。")).with_on_call(inject_at(
            h.clone(),
            1,
            vec![STEER.to_string()],
        )),
    );
    h.run(model.clone()).await;

    let lines = h.evidence.lines(&h.task.id);
    assert!(!lines.is_empty(), "任务跑完了却一条证据都没有");
    assert!(
        lines.iter().any(|l| l.contains(r#""route":"steer""#)),
        "追问排进来了，证据链上却找不到 route=steer 那一条"
    );
    assert!(
        lines.iter().any(|l| l.contains(STEER)),
        "证据里读不到用户把要求改成了什么"
    );
    // 建任务那条也认得出自己是谁，两种语义在 payload 上分得开
    assert!(lines.iter().any(|l| l.contains(r#""route":"new_task""#)));
    // 模型确实看到了它 —— 证据记的和模型收到的是同一件事
    assert!(model.call(2).iter().any(|m| m.content == signed(STEER)));
}

#[tokio::test]
async fn a_very_long_steer_is_not_truncated() {
    // 几千字的追问原样进上下文。与 transcript 里的普通 user turn 同口径：W1 只按**轮数**
    // 截断（40 轮留头 2 尾 30），从来不按字数；契约里也没有用户文本的字数上限
    // （MAX_EXEC_OUTPUT_CHARS / MAX_TOOL_CONTENT_CHARS 管的是工具输出）。
    let long_text = "再确认一遍口径".repeat(600); // 4200 字
    let h = seeded("按月画个图", Vec::new()).await;
    let model = Arc::new(
        ScriptedModel::new(three_step_script("好了。")).with_on_call(inject_at(
            h.clone(),
            1,
            vec![long_text.clone()],
        )),
    );
    h.run(model.clone()).await;

    let merged: Vec<aite_contracts::Message> = model
        .call(2)
        .into_iter()
        .filter(|m| m.role == Role::User && m.content == signed(&long_text))
        .collect();
    assert_eq!(merged.len(), 1);
    let body = merged[0].content.strip_prefix("[张三] ").expect("带署名");
    assert_eq!(body.chars().count(), 4200, "署名之外一个字没截");
}

/// CC3 ④：steer 也署名。发言人从 transcript 里认领（控制面先落 turn 再入队）；
/// 同一句话被不同的人说过 → 认不准，不署名。
#[tokio::test]
async fn steer_lines_are_attributed() {
    let h = seeded("按月画个图", Vec::new()).await;
    let hook_h = h.clone();
    let model = Arc::new(
        ScriptedModel::new(three_step_script("好了。")).with_on_call(Arc::new(move |step| {
            let h = hook_h.clone();
            Box::pin(async move {
                if step == 1 {
                    h.push_steer_as("ou_lisi", "我也要一份").await;
                    h.push_steer_as("ou_lisi", "好").await;
                    h.push_steer_as("ou_wangwu", "好").await;
                }
            })
        })),
    );
    h.run(model.clone()).await;

    let step3 = model.call(2);
    let tail: Vec<String> = step3[step3.len() - 3..]
        .iter()
        .map(|m| m.content.clone())
        .collect();
    assert_eq!(
        tail,
        vec![
            "[ou_lisi] 我也要一份".to_string(),
            "好".to_string(),
            "好".to_string(),
        ],
        "非发起人且群历史里没名字 → 署 id；同一句话两个人说过 → 不署"
    );
}
