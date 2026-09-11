//! W3 的 Answering 路径、W5 的产物交付、W8 的证据落盘。
//! 移植自 `tests/worker/test_final.py`（9 条）。
mod common;

use aite_contracts::{EvidenceKind, EvidenceWriter, TaskStatus};
use common::*;
use serde_json::json;
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
    assert_eq!(ev.count_kind(&run.task.id, EvidenceKind::ChecklistOp), 2);
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
            "duration_ms",
            "error",
            "name",
            "ok"
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
