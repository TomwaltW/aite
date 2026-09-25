//! W3 的 Answering 路径、W5 的产物交付、W8 的证据落盘。
//! 移植自 `tests/worker/test_final.py`（9 条）。
mod common;

use aite_contracts::{
    EvidenceKind, EvidenceWriter, Role, SessionStore, Task, TaskStatus, TaskWorker, Turn, TurnRole,
};
use aite_worker::texts;
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

// ---- W3：第一步就 final → 不发卡片 -------------------------------------

#[tokio::test]
async fn answering_path_sends_text_only() {
    let run = run_script(vec![final_turn("北京今天 26 度。")], 0.0).await;

    assert!(run.h.platform.cards().is_empty(), "没有卡片");
    assert!(run.h.platform.card_updates().is_empty());
    assert_eq!(run.h.platform.texts().len(), 1);
    assert_eq!(run.h.platform.texts()[0].text, "北京今天 26 度。");
    assert_eq!(run.task.status, TaskStatus::Delivered);
    assert_eq!(run.model.call_count(), 1);
}

/// Answering 那一格在状态机上**真的被走过**。
///
/// 这条是 RΩ 审核之后补的钉子。在它之前全仓没有任何一条断言管着
/// `deliver()` 里那句 `status = if answering { Answering } else { Working }`：
/// `grep TaskStatus::Answering` 只命中实现本身、contracts 里那条「Answering 不活跃」的
/// 断言，和两处拿它当普通枚举值用的存储用例。谁把它改成 Working，整套测试一条都不会红。
///
/// 而它必须被钉住，是因为控制面 `!status` 那条「把 running 并进来」的补丁正是**因为**
/// 这一格不在 `ACTIVE_TASK_STATUSES` 里才需要存在。这一格没了，那条补丁就成了无因之果。
#[tokio::test]
async fn answering_path_passes_through_the_answering_status() {
    let run = run_script(vec![final_turn("北京今天 26 度。")], 0.0).await;

    let writes = run.h.store.status_writes(&run.task.id);
    assert!(
        writes.contains(&TaskStatus::Answering),
        "没发过卡片那一路必须先落 answering，实际落过：{writes:?}"
    );
    assert_eq!(
        writes.last(),
        Some(&TaskStatus::Delivered),
        "最后一笔还是 delivered：{writes:?}"
    );
    assert!(
        !TaskStatus::Answering.is_active(),
        "前提：answering 不在 ACTIVE_TASK_STATUSES 里（contracts 的 roundtrip.rs 钉着）"
    );
}

/// 对照组：发过卡片那一路落的是 `Working`，不是 `Answering`。
///
/// 判据是「卡片一次都没发过」（`answering = !ctx.card.sent()`），不是「没跑过工具」。
#[tokio::test]
async fn a_task_that_already_sent_a_card_delivers_from_working() {
    let mut h = Harness::new();
    h.seed("帮我算个数", Vec::new()).await;
    let model = std::sync::Arc::new(ScriptedModel::new(vec![
        tool_turn(&[("checklist_add", json!({"items": ["算一算"]}))]),
        final_turn("算完了：42。"),
    ]));
    let task = h.run(model).await;

    let writes = h.store.status_writes(&task.id);
    assert!(
        !writes.contains(&TaskStatus::Answering),
        "发过卡片就不该走 answering 那一格：{writes:?}"
    );
    assert!(writes.contains(&TaskStatus::Working));
    assert_eq!(writes.last(), Some(&TaskStatus::Delivered));
}

#[tokio::test]
async fn plain_text_on_first_step_is_treated_as_final() {
    // §3.3 对国内模型的兜底：第一步没调工具、只回文本 → 当作 final
    let run = run_script(vec![text_turn("直接回答你：42。")], 0.0).await;

    assert!(run.h.platform.cards().is_empty());
    assert_eq!(run.h.platform.texts()[0].text, "直接回答你：42。");
    assert_eq!(run.task.status, TaskStatus::Delivered);
}

// ---- W5：产物 -----------------------------------------------------------

#[tokio::test]
async fn final_artifacts_are_sent_into_the_thread() {
    let mut h = Harness::new();
    h.sandbox.put("/work/out.png", PNG);
    h.seed("帮我出个图", Vec::new()).await;
    let model = std::sync::Arc::new(
        ScriptedModel::new(vec![
            tool_turn(&[("run_python", json!({"code": "..."}))]),
            final_turn_with(
                "图在这里。",
                json!([{"path": "/work/out.png", "title": "月度趋势"}]),
            ),
        ])
        .with_clock(h.clock.clone(), 0.6),
    );
    let task = h.run(model).await;

    let files = h.platform.files();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].data, PNG);
    assert_eq!(files[0].name, "out.png");
    assert_eq!(files[0].mime, "image/png");
    assert_eq!(files[0].reply_to.as_deref(), Some(ROOT), "回到话题 root");
    assert_eq!(h.session.anchor.thread_id.as_deref(), Some(ROOT));

    // 文件先发，再发正文
    assert_eq!(h.platform.last_text(), "图在这里。");
    assert_eq!(task.status, TaskStatus::Delivered);

    assert_eq!(h.evidence.count_kind(&task.id, EvidenceKind::Artifact), 1);
    let art = h.evidence.payloads(&task.id, EvidenceKind::Artifact)[0].clone();
    assert_eq!(
        serde_json::Value::Object(art),
        json!({
            "title": "月度趋势",
            "mime": "image/png",
            "sha256": sha256_hex(PNG),
            "size": PNG.len(),
        })
    );
}

#[tokio::test]
async fn missing_artifact_is_skipped_but_task_still_delivered() {
    // §3.3：path 不在 /work 下或不存在 → 跳过该产物，回帖附一行，任务仍 delivered
    let mut h = Harness::new();
    h.sandbox.put("/work/ok.txt", b"hi");
    h.seed("帮我出个图", Vec::new()).await;
    let model = std::sync::Arc::new(
        ScriptedModel::new(vec![
            tool_turn(&[("list_files", json!({}))]),
            final_turn_with(
                "两个产物。",
                json!([
                    {"path": "/work/ok.txt", "title": "好的"},
                    {"path": "/work/missing.png", "title": "缺的"},
                    {"path": "/tmp/outside.png", "title": "越界的"},
                ]),
            ),
        ])
        .with_clock(h.clock.clone(), 0.6),
    );
    let task = h.run(model).await;

    let names: Vec<String> = h.platform.files().into_iter().map(|f| f.name).collect();
    assert_eq!(names, ["ok.txt"]);
    let body = h.platform.last_text();
    assert!(body.starts_with("两个产物。"));
    assert!(body.contains("产物 缺的 未找到"));
    assert!(body.contains("产物 越界的 未找到"));
    assert_eq!(task.status, TaskStatus::Delivered);
}

#[tokio::test]
async fn final_without_reply_is_invalid_args() {
    let run = run_script(
        vec![
            tool_turn(&[("final", json!({"artifacts": []}))]),
            final_turn("这次带上了正文"),
        ],
        0.6,
    )
    .await;

    assert_eq!(run.task.status, TaskStatus::Delivered);
    assert_eq!(run.h.platform.last_text(), "这次带上了正文");
}

// ---- W8：证据 -----------------------------------------------------------

#[tokio::test]
async fn evidence_chain_covers_the_run() {
    let run = run_script(
        vec![
            tool_turn(&[("checklist_add", json!({"items": ["取数"]}))]),
            tool_turn(&[("run_python", json!({"code": "print(1)"}))]),
            tool_turn(&[("checklist_check", json!({"id": "c1"}))]),
            final_turn("好了"),
        ],
        0.6,
    )
    .await;

    let ev = &run.h.evidence;
    let kinds = ev.kinds(&run.task.id);
    assert_eq!(kinds[0], EvidenceKind::TaskCreated);
    assert_eq!(kinds[1], EvidenceKind::EventReceived);
    assert_eq!(ev.count_kind(&run.task.id, EvidenceKind::ModelCall), 4);
    // `checklist_op` 现在装两类东西：checklist 本身的操作，和卡片的发送 / 更新
    // （BB2 ②，复用同一个 kind，靠 `op` 区分）。所以这里按 `op` 数，不按 kind 数 ——
    // 按 kind 数的话，卡片多推一次这条断言就得跟着改一个数字，而它想钉的根本不是卡片。
    let ops: Vec<String> = ev
        .payloads(&run.task.id, EvidenceKind::ChecklistOp)
        .iter()
        .map(|p| p["op"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ops.iter().filter(|o| *o == "add" || *o == "check").count(),
        2,
        "checklist 本身的操作：add ×1 + check ×1，实际 {ops:?}"
    );
    assert_eq!(
        ops.iter().filter(|o| *o == "card_sent").count(),
        1,
        "send_card 至多一次，证据里也只该有一条，实际 {ops:?}"
    );
    assert_eq!(
        ops.iter().filter(|o| *o == "card_updated").count(),
        run.h.platform.card_updates().len(),
        "证据里的 card_updated 条数必须等于平台真被调到的 update_card 次数，实际 {ops:?}"
    );
    assert_eq!(
        ev.count_kind(&run.task.id, EvidenceKind::ToolCall),
        4,
        "含 final"
    );
    // final 成功时没有 tool_result：它不给模型返回结果，任务到此为止，由 delivered 那条承接
    assert_eq!(ev.count_kind(&run.task.id, EvidenceKind::ToolResult), 3);
    assert_eq!(kinds[kinds.len() - 1], EvidenceKind::Delivered);

    assert!(ev.verify(&run.task.id));
    let manifest = ev
        .manifest(&run.task.id)
        .expect("finalize 应该写过 manifest");
    assert_eq!(
        manifest["root_hash"],
        json!(run.task.evidence_root_hash.clone().unwrap_or_default())
    );
    assert_eq!(manifest["task_no"], json!(run.task.task_no));
    assert_eq!(manifest["session_id"], json!(run.task.session_id));
    assert_eq!(manifest["event_count"], json!(kinds.len()));
}

#[tokio::test]
async fn model_message_text_never_enters_evidence() {
    // W8：模型消息全文不进 evidence，只进 transcript
    let secret = "这是模型写的一段很长的正文，绝不该出现在证据里";
    let run = run_script(vec![final_turn(secret)], 0.0).await;

    let calls = run
        .h
        .evidence
        .payloads(&run.task.id, EvidenceKind::ModelCall);
    assert!(!calls.is_empty());
    let keys: std::collections::BTreeSet<&str> = calls[0].keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["finish_reason", "messages_hash", "model", "step", "usage"]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    );
    // final 的 arguments 里确实有正文（那是工具调用参数，不是模型消息全文），
    // 但 model_call 那条只留 hash
    let rendered = serde_json::to_string(&calls[0]).unwrap_or_default();
    assert!(!rendered.contains(secret));
    assert_eq!(run.h.evidence.raw(&run.task.id).matches(secret).count(), 1);
}

#[tokio::test]
async fn gateway_tool_result_is_recorded_as_hash() {
    let run = run_script(
        vec![
            tool_turn(&[("read_document", json!({"url_or_token": "doc1"}))]),
            final_turn("读完了"),
        ],
        0.6,
    )
    .await;

    let results = run
        .h
        .evidence
        .payloads(&run.task.id, EvidenceKind::ToolResult);
    let keys: std::collections::BTreeSet<&str> = results[0].keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "call_id",
            "content_hash",
            "content_summary",
            "duration_ms",
            "error",
            "name",
            "ok",
            "sandbox_id"
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
    );
    assert_eq!(results[0]["name"], json!("read_document"));
    assert_eq!(results[0]["ok"], json!(true));
    assert_eq!(
        results[0]["content_hash"],
        json!(sha256_hex(b"read_document ok"))
    );
    // BB2 ③：摘要是 content 的另一个视图，**不进 content_hash** —— 上面那条
    // `content_hash` 断言仍然是对全文算的，摘要加进 payload 之后它一个字符都没变。
    assert_eq!(results[0]["content_summary"], json!("read_document ok"));
    // read_document 不碰沙箱，所以这里是 null；「没有沙箱」和「漏记了」在证据里
    // 长得不一样（漏记是这个键根本不在，上面 keys 那条钉着）。
    assert_eq!(results[0]["sandbox_id"], Value::Null);
    assert_eq!(run.h.gateway.calls().len(), 1);
}

#[tokio::test]
async fn gateway_gets_the_task_session_token() {
    let run = run_script(
        vec![tool_turn(&[("list_files", json!({}))]), final_turn("好")],
        0.6,
    )
    .await;

    let (ctx, req) = run.h.gateway.calls()[0].clone();
    assert_eq!(ctx.session_token, run.task.session_token);
    assert_eq!(run.task.session_token.len(), 32);
    assert_eq!(ctx.task_id, run.task.id);
    assert_eq!(ctx.session_id, run.task.session_id);
    assert_eq!(ctx.thread_id.as_deref(), Some(ROOT));
    assert_eq!(req.name, "list_files");

    // AppWorker 并进来的那件事：开跑前把 session_token 登记给 Gateway
    assert_eq!(
        run.h.gateway.registered(),
        vec![(run.task.id.clone(), run.task.session_token.clone())]
    );
}

// ---- `final.artifacts` 的真值语义（RΩ 补，审核记账 R5）------------------

/// Python 那边是 `raw = args.get("artifacts") or []`（loop.py:634）：**假值**一律
/// 当成「没有产物」照常交付。Rust 原来把 `""` / `{}` / `0` / `false` 都判成
/// invalid_args 退回重来 —— 交付路径上的行为翻转，而弱模型给 `artifacts: ""` 不罕见。
///
/// 四个取值各走一遍：每一个都必须**一步就交付**（模型只被调 1 次），
/// 而不是退回去再要一轮。
#[tokio::test]
async fn falsy_artifacts_are_treated_as_no_artifacts() {
    for falsy in [json!(""), json!({}), json!(0), json!(false), json!(null)] {
        let run = run_script(
            vec![tool_turn(&[(
                "final",
                json!({"reply": "干完了。", "artifacts": falsy}),
            )])],
            0.0,
        )
        .await;
        assert_eq!(
            run.task.status,
            TaskStatus::Delivered,
            "artifacts={falsy} 该照常交付"
        );
        assert_eq!(run.h.platform.last_text(), "干完了。");
        assert!(
            run.h.platform.files().is_empty(),
            "artifacts={falsy} 不该发产物"
        );
        assert_eq!(run.model.call_count(), 1, "artifacts={falsy} 不该退回重来");
    }
}

/// 反过来：**真值但不是数组**仍然是 invalid_args（这条 Python 也一样）。
#[tokio::test]
async fn truthy_non_array_artifacts_are_still_invalid_args() {
    let run = run_script(
        vec![
            tool_turn(&[(
                "final",
                json!({"reply": "干完了。", "artifacts": "/work/out.png"}),
            )]),
            final_turn("这次给数组了"),
        ],
        0.6,
    )
    .await;

    assert_eq!(run.model.call_count(), 2, "真值非数组该退回去要第二轮");
    assert_eq!(run.task.status, TaskStatus::Delivered);
    assert_eq!(run.h.platform.last_text(), "这次给数组了");
}

// ---- 单个 artifact 的 title / mime 真值语义（RΩ 补，审核记账 R5 的另一半）----

/// 一步就 final、只带一个产物；沙箱里预置 `/work/out.png`。
async fn run_one_artifact(art: Value) -> (Task, Harness) {
    let mut h = Harness::new();
    h.sandbox.put("/work/out.png", PNG);
    h.seed("帮我出个图", Vec::new()).await;
    let model = std::sync::Arc::new(
        ScriptedModel::new(vec![final_turn_with("图在这里。", json!([art]))])
            .with_clock(h.clock.clone(), 0.0),
    );
    let task = h.run(model).await;
    (task, h)
}

/// Python 是 `str(art.get("title") or art.get("path", ""))`（loop.py:464）：**真值语义**。
/// Rust 原来判的是「字符串化之后为空」，于是 `title: false` / `0` / `{}` 会把标题
/// 发成字面量 `"false"` / `"0"` / `"{}"`，而不是退回 `path`。
#[tokio::test]
async fn falsy_artifact_title_falls_back_to_path() {
    for falsy in [json!(false), json!(0), json!({}), json!(""), json!(null)] {
        let (task, h) = run_one_artifact(json!({"path": "/work/out.png", "title": falsy})).await;
        assert_eq!(task.status, TaskStatus::Delivered, "title={falsy}");
        let art = h.evidence.payloads(&task.id, EvidenceKind::Artifact)[0].clone();
        assert_eq!(
            art["title"],
            json!("/work/out.png"),
            "title={falsy} 该退回 path"
        );
    }

    // 干脆没写 title 的那一路
    let (task, h) = run_one_artifact(json!({"path": "/work/out.png"})).await;
    let art = h.evidence.payloads(&task.id, EvidenceKind::Artifact)[0].clone();
    assert_eq!(art["title"], json!("/work/out.png"), "缺 title 该退回 path");
}

/// 产物取不到时回帖那行「未找到」用的是同一个 title（Python `missing.append(title or path)`），
/// 所以假值 title 在这条路上也该显示成 `path`，而不是字面量 "false"。
#[tokio::test]
async fn falsy_title_on_missing_artifact_shows_path() {
    let (task, h) = run_one_artifact(json!({"path": "/work/nope.png", "title": false})).await;
    assert_eq!(task.status, TaskStatus::Delivered);
    assert!(
        h.platform
            .last_text()
            .contains("产物 /work/nope.png 未找到"),
        "回帖该报 path 而不是 \"false\"，实际：{}",
        h.platform.last_text()
    );
}

/// Python 是 `art.get("mime") or mimetypes.guess_type(path)[0] or "application/octet-stream"`
/// （loop.py:469）：同一个真值语义。Rust 原来同样只看「字符串化之后为空」，
/// `mime: false` 会把 `Content-Type` 发成 `"false"`。
#[tokio::test]
async fn falsy_artifact_mime_falls_back_to_guess() {
    for falsy in [json!(false), json!(0), json!({}), json!(""), json!(null)] {
        let (task, h) = run_one_artifact(json!({"path": "/work/out.png", "mime": falsy})).await;
        let files = h.platform.files();
        assert_eq!(files.len(), 1, "mime={falsy}");
        assert_eq!(files[0].mime, "image/png", "mime={falsy} 该按扩展名猜");
        let art = h.evidence.payloads(&task.id, EvidenceKind::Artifact)[0].clone();
        assert_eq!(
            art["mime"],
            json!("image/png"),
            "mime={falsy} 证据里也要一致"
        );
    }

    // 干脆没写 mime 的那一路
    let (_, h) = run_one_artifact(json!({"path": "/work/out.png"})).await;
    assert_eq!(h.platform.files()[0].mime, "image/png", "缺 mime 该猜");

    // 反过来：真值 mime 照用，别矫枉过正成一律猜
    let (_, h) =
        run_one_artifact(json!({"path": "/work/out.png", "mime": "application/x-custom"})).await;
    assert_eq!(h.platform.files()[0].mime, "application/x-custom");
}

// ---- CC3 ③ 助手回复入 transcript ------------------------------------------

/// 交付之后多一条 Assistant 轮：正文 = 发出去的文字（含「产物 X 未找到」行）+ 一行已发附件标题。
#[tokio::test]
async fn assistant_turn_persisted_after_delivery() {
    let mut h = Harness::new();
    h.sandbox.put("/work/out.png", PNG);
    h.seed("帮我出个图", Vec::new()).await;
    let model = std::sync::Arc::new(
        ScriptedModel::new(vec![
            tool_turn(&[("run_python", json!({"code": "..."}))]),
            final_turn_with(
                "图在这里。",
                json!([
                    {"path": "/work/out.png", "title": "月度趋势"},
                    {"path": "/work/missing.csv", "title": "明细"},
                ]),
            ),
        ])
        .with_clock(h.clock.clone(), 0.6),
    );
    let task = h.run(model).await;
    assert_eq!(task.status, TaskStatus::Delivered);

    let sent = h.platform.last_text();
    assert_eq!(sent, "图在这里。\n产物 明细 未找到");
    let turns = h.store.turns(&h.session.id);
    assert_eq!(turns.len(), 2, "用户那一轮 + 助手这一轮");
    let assistant = &turns[1];
    assert_eq!(assistant.role, TurnRole::Assistant);
    assert_eq!(assistant.seq, 1);
    assert_eq!(assistant.platform_user_id, None);
    assert!(assistant.attachments.is_empty());
    assert_eq!(
        assistant.content,
        "图在这里。\n产物 明细 未找到\n[已发送附件] 月度趋势"
    );
    assert_eq!(
        texts::assistant_turn_content("好", &[]),
        "好",
        "没有已发附件时逐字等于发出去的正文"
    );
}

/// 写助手轮撞 `DuplicateTurn`（控制面抢先写了同一个 seq）→ 重取 seq 再写，交付照常。
#[tokio::test]
async fn assistant_turn_retries_duplicate_seq() {
    let mut h = Harness::new();
    h.seed("帮我出个图", Vec::new()).await;
    h.store.duplicate_next_append_turn(1);
    let task = h
        .run(std::sync::Arc::new(ScriptedModel::new(vec![final_turn(
            "北京今天晴。",
        )])))
        .await;
    assert_eq!(task.status, TaskStatus::Delivered);
    let turns: Vec<(u64, TurnRole, String)> = h
        .store
        .turns(&h.session.id)
        .into_iter()
        .map(|t| (t.seq, t.role, t.content))
        .collect();
    assert_eq!(
        turns,
        vec![
            (0, TurnRole::User, "帮我出个图".to_string()),
            (1, TurnRole::User, "（别的写者抢先写的 seq=1）".to_string()),
            (2, TurnRole::Assistant, "北京今天晴。".to_string()),
        ]
    );
    assert_eq!(
        h.platform.texts().len(),
        1,
        "回复只发一次，不因为撞号多发失败通知"
    );
}

/// 同会话第二个任务：首次 `chat` 里有一条 `Role::Assistant` = 上一轮的回复。
#[tokio::test]
async fn followup_context_contains_previous_answer() {
    let mut h = Harness::new();
    h.seed("北京今天天气怎样？", Vec::new()).await;
    h.run(std::sync::Arc::new(ScriptedModel::new(vec![final_turn(
        "北京今天晴，最高 28℃。",
    )])))
    .await;

    // 控制面那一侧：同话题的追问落一条 User 轮（seq 从 store 取，别撞上助手轮）、建第二个任务
    let seq = h.store.next_turn_seq(&h.session.id).await.expect("seq");
    h.store.push_turn(Turn {
        session_id: h.session.id.clone(),
        seq,
        role: TurnRole::User,
        platform_user_id: Some("ou_user".into()),
        content: "那明天呢？".into(),
        attachments: Vec::new(),
        created_at: chrono::Utc::now(),
    });
    let task2 = Task {
        id: "t2".into(),
        task_no: "#A2".into(),
        title: String::new(),
        status: TaskStatus::Created,
        ..h.task.clone()
    };
    let model2 = std::sync::Arc::new(ScriptedModel::new(vec![final_turn("明天多云。")]));
    let out = h
        .worker(model2.clone())
        .run(task2, h.session.clone(), Some("张三".into()), h.hooks())
        .await;
    assert_eq!(out.status, TaskStatus::Delivered);

    let first = model2.call(0);
    assert!(
        first
            .iter()
            .any(|m| m.role == Role::Assistant && m.content == "北京今天晴，最高 28℃。"),
        "第二个任务看得到上一轮自己说了什么：{first:?}"
    );
}
