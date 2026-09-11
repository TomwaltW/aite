//! `aite evidence show` 的测试（移植 `tests/tools/test_evidence_show.py` 32 条）。
//!
//! 口径：证据一律用真的 `FileEvidenceWriter` 往 tempdir 里写，不手搓 jsonl ——
//! 手搓的 jsonl 只能证明「我的渲染和我的假数据自洽」，证明不了它读得懂真写出来的东西。

mod common;

use std::fs::{File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use aite_contracts::{EvidenceKind, EvidenceWriter, canonical_json, payload_hash_of};
use aite_evidence::FileEvidenceWriter;
use aite_evidence::cli::{self, MARK_TERMINAL, Timeline, filter_rows, list_tasks, render_text};
use common::{lines_of, p, rewrite_lines};
use serde_json::{Value, json};
use tempfile::TempDir;

const MODEL_PRICE_IN: f64 = 2.4;
const MODEL_PRICE_OUT: f64 = 9.6;

/// 一个走完全程的任务：建任务 → 两次模型调用 → 清单 → 一次失败的工具 → 产物 → 交付。
async fn write_full_task(root: &Path, task_id: &str) -> FileEvidenceWriter {
    let w = FileEvidenceWriter::new(root.to_path_buf());
    let add = |kind, payload: Value| {
        let w = &w;
        async move { w.append(task_id, kind, p(payload)).await.unwrap() }
    };

    add(
        EvidenceKind::TaskCreated,
        json!({"session_id": "ses_1", "task_no": "#A1", "chat_id": "oc_1",
               "created_by": "ou_1", "title": "把这个 CSV 画成月度趋势图"}),
    )
    .await;
    add(
        EvidenceKind::EventReceived,
        json!({"event_id": "evt_1", "kind": "message", "chat_id": "oc_1",
               "sender_id": "ou_1", "message_id": "om_1", "mentioned": true}),
    )
    .await;
    add(
        EvidenceKind::ModelCall,
        json!({"model": "qwen-max", "step": 0, "messages_hash": "a".repeat(64),
               "usage": {"input_tokens": 1000, "output_tokens": 100, "cached_tokens": 0},
               "finish_reason": "tool_calls"}),
    )
    .await;
    add(
        EvidenceKind::ToolCall,
        json!({"call_id": "c1", "name": "checklist_add",
               "arguments": {"items": ["读取 CSV", "按月汇总", "画趋势图"]}}),
    )
    .await;
    add(
        EvidenceKind::ChecklistOp,
        json!({"op": "add", "ids": ["c1", "c2", "c3"],
               "items": ["读取 CSV", "按月汇总", "画趋势图"]}),
    )
    .await;
    add(
        EvidenceKind::ToolResult,
        json!({"call_id": "c1", "name": "checklist_add", "ok": true, "error": null,
               "content_hash": "b".repeat(64), "duration_ms": 0}),
    )
    .await;
    add(
        EvidenceKind::ChecklistOp,
        json!({"op": "check", "id": "c2", "state": "done"}),
    )
    .await;
    add(
        EvidenceKind::ToolCall,
        json!({"call_id": "c2", "name": "run_python",
               "arguments": {"code": "print(1)", "timeout_sec": 60}}),
    )
    .await;
    add(
        EvidenceKind::ToolResult,
        json!({"call_id": "c2", "name": "run_python", "ok": false, "error": "timeout",
               "content_hash": "c".repeat(64), "duration_ms": 60001}),
    )
    .await;
    add(
        EvidenceKind::ModelCall,
        json!({"model": "qwen-max", "step": 1, "messages_hash": "d".repeat(64),
               "usage": {"input_tokens": 2000, "output_tokens": 300, "cached_tokens": 1024},
               "finish_reason": "tool_calls"}),
    )
    .await;
    add(
        EvidenceKind::Artifact,
        json!({"title": "月度趋势", "mime": "image/png", "sha256": "e".repeat(64), "size": 4096}),
    )
    .await;
    add(
        EvidenceKind::Delivered,
        json!({"artifacts": 1, "missing": [], "steps": 2}),
    )
    .await;

    w.finalize(
        task_id,
        p(json!({"session_id": "ses_1", "task_no": "#A1",
                 "created_by": "ou_1", "model": "qwen-max"})),
    )
    .await
    .unwrap();
    w
}

fn load(task_dir: &Path) -> Timeline {
    cli::load_timeline(task_dir, MODEL_PRICE_IN, MODEL_PRICE_OUT)
}

/// 跑一次 CLI，拿到 (退出码, stdout, stderr)。
fn run(args: &[&str]) -> (i32, String, String) {
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let argv: Vec<String> = std::iter::once("show".to_string())
        .chain(args.iter().map(|s| (*s).to_string()))
        .collect();
    let code = cli::run_with(argv, &mut out, &mut err);
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    )
}

async fn full_task(tmp: &TempDir) -> PathBuf {
    write_full_task(tmp.path(), "tsk_demo").await;
    tmp.path().join("tsk_demo")
}

/// 改 events.jsonl 的某一行后原样写回（行是合法 JSON，只是内容变了）。
fn rewrite_line(w: &FileEvidenceWriter, task_id: &str, index: usize, f: impl FnOnce(&mut Value)) {
    let mut rows: Vec<Value> = lines_of(w, task_id)
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    f(&mut rows[index]);
    let out: Vec<String> = rows
        .iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect();
    rewrite_lines(w, task_id, &out);
}

// ---------------------------------------------------------------- 正常任务

#[tokio::test]
async fn full_task_renders_every_event_and_verifies() {
    let tmp = TempDir::new().unwrap();
    let dir = full_task(&tmp).await;
    let tl = load(&dir);

    assert!(tl.ok());
    assert!(tl.finalized());
    assert_eq!(tl.rows.len(), 12);
    assert_eq!(tl.checked, 12);
    assert!(tl.issues.is_empty());
    assert!(tl.manifest_problems.is_empty());

    // root_hash 就是 manifest 里那个
    let m: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(tl.root_hash, m["root_hash"].as_str().unwrap());
}

#[tokio::test]
async fn summary_numbers_match_the_events() {
    let tmp = TempDir::new().unwrap();
    let s = load(&full_task(&tmp).await).stats;

    assert_eq!(s.events, 12);
    assert_eq!(s.model_calls, 2);
    assert_eq!(s.tokens_in, 3000);
    assert_eq!(s.tokens_out, 400);
    assert_eq!(s.tool_calls, 2);
    assert_eq!(s.tool_failed, 1, "run_python 那次 timeout");
    assert_eq!(s.artifacts, 1);
    assert_eq!(s.terminal, "delivered");
    assert_eq!(s.unreadable, 0);
    // 与 worker 的 _price 同式：(in*price_in + out*price_out)/1e6
    let want = (3000.0 * MODEL_PRICE_IN + 400.0 * MODEL_PRICE_OUT) / 1e6;
    assert!((s.cost - want).abs() < 1e-9, "{} vs {want}", s.cost);
}

/// 事件行长这样：[标记] seq +耗时
fn is_event_line(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '★' | '✗' | ' ') {
        return false;
    }
    let rest: String = chars.collect();
    let rest = rest.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return false;
    }
    rest[digits.len()..].trim_start().starts_with('+')
}

#[tokio::test]
async fn rendered_line_count_matches_event_count() {
    let tmp = TempDir::new().unwrap();
    let tl = load(&full_task(&tmp).await);
    let text = render_text(&tl, &tl.rows, false);

    let body = text.split("── 汇总").next().unwrap();
    let event_lines = body.lines().filter(|l| is_event_line(l)).count();
    assert_eq!(event_lines, tl.rows.len());
    assert_eq!(event_lines, 12);
    assert!(text.contains("hash 链   OK"));
    assert!(text.contains("12 条全部闭合"));
}

#[tokio::test]
async fn terminal_event_is_marked() {
    let tmp = TempDir::new().unwrap();
    let tl = load(&full_task(&tmp).await);
    let text = render_text(&tl, &tl.rows, false);

    let delivered: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("delivered ") && l.contains("已交付"))
        .collect();
    assert_eq!(delivered.len(), 1);
    assert!(delivered[0].starts_with(MARK_TERMINAL));
}

#[tokio::test]
async fn checklist_check_shows_the_item_text() {
    // payload 里只有 id 和 state，文本要从前面的 add 事件里补回来。
    let tmp = TempDir::new().unwrap();
    let tl = load(&full_task(&tmp).await);
    let row = tl
        .rows
        .iter()
        .find(|r| r.kind == "checklist_op" && r.fields.get("op") == Some(&json!("check")))
        .unwrap();

    assert!(row.detail.contains("c2"), "{}", row.detail);
    assert!(row.detail.contains("按月汇总"), "{}", row.detail);
}

#[tokio::test]
async fn model_call_line_carries_usage_and_cost() {
    let tmp = TempDir::new().unwrap();
    let tl = load(&full_task(&tmp).await);
    let row = tl.rows.iter().find(|r| r.kind == "model_call").unwrap();

    assert_eq!(row.fields["model"], json!("qwen-max"));
    assert_eq!(row.fields["finish_reason"], json!("tool_calls"));
    assert_eq!(row.fields["total_tokens"], json!(1100));
    assert!(
        row.detail.contains("in=1000") && row.detail.contains("out=100"),
        "{}",
        row.detail
    );
    assert!(row.detail.contains('¥'), "{}", row.detail);
}

#[tokio::test]
async fn tool_result_shows_error_code_and_duration() {
    let tmp = TempDir::new().unwrap();
    let tl = load(&full_task(&tmp).await);
    let row = tl
        .rows
        .iter()
        .find(|r| r.kind == "tool_result" && r.fields["ok"] == json!(false))
        .unwrap();

    assert!(row.detail.contains("FAIL[timeout]"), "{}", row.detail);
    assert!(row.detail.contains("60001ms"), "{}", row.detail);
}

// ---------------------------------------------------------------- 篡改

#[tokio::test]
async fn tampered_payload_is_caught_and_points_at_the_break() {
    let tmp = TempDir::new().unwrap();
    let w = write_full_task(tmp.path(), "tsk_demo").await;
    let dir = tmp.path().join("tsk_demo");

    let original_hash: String = {
        let row: Value = serde_json::from_str(&lines_of(&w, "tsk_demo")[3]).unwrap();
        row["payload_hash"].as_str().unwrap().to_string()
    };
    rewrite_line(&w, "tsk_demo", 3, |row| {
        row["payload"]["arguments"]["items"] = json!(["偷偷改成别的"]);
    });
    let tampered_payload: Value = {
        let row: Value = serde_json::from_str(&lines_of(&w, "tsk_demo")[3]).unwrap();
        row["payload"].clone()
    };

    let tl = load(&dir);

    assert!(!tl.ok());
    assert_eq!(tl.issues.len(), 1);
    let issue = &tl.issues[0];
    assert_eq!(issue.seq, Some(3), "断在第几条");
    assert_eq!(issue.line, 4, "events.jsonl 的第几行");
    assert!(issue.problem.contains("payload_hash"), "{}", issue.problem);
    assert_eq!(issue.expected, original_hash, "期望");
    assert_eq!(issue.actual, payload_hash_of(&p(tampered_payload)), "实际");
    // 只报一处，不级联：改一条不该把后面 8 条都染红
    assert_eq!(
        tl.rows
            .iter()
            .filter(|r| r.broken)
            .map(|r| r.seq)
            .collect::<Vec<_>>(),
        [3]
    );
}

#[tokio::test]
async fn tampered_chain_exits_nonzero_and_says_so() {
    let tmp = TempDir::new().unwrap();
    let w = write_full_task(tmp.path(), "tsk_demo").await;
    rewrite_line(&w, "tsk_demo", 2, |row| {
        row["payload"]["usage"]["input_tokens"] = json!(999999);
    });

    let dir = tmp.path().join("tsk_demo");
    let (code, out, _) = run(&["--dir", &dir.display().to_string()]);

    assert_eq!(code, 1);
    assert!(out.contains("hash 链   断了"), "{out}");
    assert!(out.contains("seq=2"), "{out}");
    assert!(out.contains("这份证据不可信"), "{out}");
}

#[tokio::test]
async fn cut_line_breaks_the_chain_at_the_next_event() {
    // 有人把中间一行删了：seq 不连续 + prev_hash 接不住，两条都要报。
    let tmp = TempDir::new().unwrap();
    let w = write_full_task(tmp.path(), "tsk_demo").await;
    let mut lines = lines_of(&w, "tsk_demo");
    lines.remove(5);
    rewrite_lines(&w, "tsk_demo", &lines);

    let tl = load(&tmp.path().join("tsk_demo"));

    assert!(!tl.ok());
    let problems: Vec<&str> = tl.issues.iter().map(|i| i.problem.as_str()).collect();
    assert!(problems.contains(&"seq 不连续"), "{problems:?}");
    assert!(problems.contains(&"prev_hash 接不住上一条"), "{problems:?}");
    assert_eq!(
        tl.issues.iter().filter_map(|i| i.seq).min(),
        Some(6),
        "删掉 seq=5 之后从 6 开始对不上"
    );
}

#[tokio::test]
async fn manifest_root_hash_mismatch_is_reported() {
    let tmp = TempDir::new().unwrap();
    write_full_task(tmp.path(), "tsk_demo").await;
    let dir = tmp.path().join("tsk_demo");
    let mut m: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    m["root_hash"] = json!("0".repeat(64));
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string(&m).unwrap(),
    )
    .unwrap();

    let tl = load(&dir);

    assert!(!tl.ok());
    assert!(tl.issues.is_empty(), "事件本身没问题");
    assert!(
        tl.manifest_problems.iter().any(|p| p.contains("root_hash")),
        "{:?}",
        tl.manifest_problems
    );
}

#[tokio::test]
async fn garbage_line_does_not_crash() {
    let tmp = TempDir::new().unwrap();
    let w = write_full_task(tmp.path(), "tsk_demo").await;
    let mut lines = lines_of(&w, "tsk_demo");
    lines[4] = "{ 这不是 json".to_string();
    rewrite_lines(&w, "tsk_demo", &lines);

    let tl = load(&tmp.path().join("tsk_demo"));

    assert!(!tl.ok());
    assert!(
        tl.issues
            .iter()
            .any(|i| i.problem.contains("不是合法的 EvidenceEvent")),
        "{:?}",
        tl.issues
    );
    assert_eq!(tl.rows.len(), 11, "剩下的照渲染");
}

// ---------------------------------------------------------------- 未 finalize

#[tokio::test]
async fn unfinalized_task_still_renders() {
    let tmp = TempDir::new().unwrap();
    let w = FileEvidenceWriter::new(tmp.path().to_path_buf());
    w.append(
        "tsk_live",
        EvidenceKind::TaskCreated,
        p(json!({"task_no": "#A2", "title": "还在跑"})),
    )
    .await
    .unwrap();
    w.append(
        "tsk_live",
        EvidenceKind::ModelCall,
        p(
            json!({"model": "qwen-max", "step": 0, "messages_hash": "f".repeat(64),
                 "usage": {"input_tokens": 10, "output_tokens": 1, "cached_tokens": 0},
                 "finish_reason": "tool_calls"}),
        ),
    )
    .await
    .unwrap();

    let tl = load(&tmp.path().join("tsk_live"));
    let text = render_text(&tl, &tl.rows, false);

    assert!(!tl.finalized());
    assert!(tl.ok(), "没 finalize 不等于损坏");
    assert_eq!(tl.rows.len(), 2);
    assert!(text.contains("未 finalize"), "{text}");
    assert!(text.contains("无终态事件"), "{text}");
}

#[tokio::test]
async fn unfinalized_task_exits_zero() {
    let tmp = TempDir::new().unwrap();
    let w = FileEvidenceWriter::new(tmp.path().to_path_buf());
    w.append(
        "tsk_live",
        EvidenceKind::TaskCreated,
        p(json!({"task_no": "#A2", "title": "还在跑"})),
    )
    .await
    .unwrap();

    let (code, out, _) = run(&["--dir", &tmp.path().join("tsk_live").display().to_string()]);
    assert_eq!(code, 0);
    assert!(out.contains("未 finalize"), "{out}");
}

// ---------------------------------------------------------------- 外置 payload

#[tokio::test]
async fn external_payload_is_followed() {
    let tmp = TempDir::new().unwrap();
    let w = FileEvidenceWriter::new(tmp.path().to_path_buf());
    let big = json!({"call_id": "c9", "name": "run_python", "ok": true, "error": null,
                     "content_hash": "9".repeat(64), "duration_ms": 12, "blob": "x".repeat(70000)});
    let ev = w
        .append("tsk_big", EvidenceKind::ToolResult, p(big))
        .await
        .unwrap();
    assert_eq!(ev.payload_ref.as_deref(), Some("payloads/0.json"));
    assert!(ev.payload.is_none(), "前提：真外置了");

    let tl = load(&tmp.path().join("tsk_big"));

    assert!(tl.ok());
    assert_eq!(
        tl.rows[0].fields["name"],
        json!("run_python"),
        "跟过去读到了"
    );
    assert_eq!(tl.rows[0].fields["duration_ms"], json!(12));
    assert!(
        tl.rows[0].note.contains("payloads/0.json"),
        "{}",
        tl.rows[0].note
    );
    assert_eq!(tl.stats.unreadable, 0);
}

#[tokio::test]
async fn missing_external_payload_degrades_gracefully() {
    let tmp = TempDir::new().unwrap();
    let w = FileEvidenceWriter::new(tmp.path().to_path_buf());
    w.append(
        "tsk_big",
        EvidenceKind::ToolResult,
        p(
            json!({"call_id": "c9", "name": "run_python", "ok": true, "error": null,
                 "content_hash": "9".repeat(64), "duration_ms": 12, "blob": "x".repeat(70000)}),
        ),
    )
    .await
    .unwrap();
    w.append(
        "tsk_big",
        EvidenceKind::Delivered,
        p(json!({"artifacts": 0, "missing": [], "steps": 1})),
    )
    .await
    .unwrap();
    std::fs::remove_file(tmp.path().join("tsk_big").join("payloads").join("0.json")).unwrap();

    let tl = load(&tmp.path().join("tsk_big"));
    let text = render_text(&tl, &tl.rows, false);

    assert!(!tl.ok(), "校验不了就是校验不了");
    assert_eq!(tl.rows.len(), 2, "但不崩，剩下的照渲染");
    assert!(
        tl.rows[0].detail.contains("payload 缺失"),
        "{}",
        tl.rows[0].detail
    );
    assert!(text.contains("payloads/0.json"), "{text}");
    // 读不到的那条不许混进汇总数字（否则 ok=false 会被当成「工具失败一次」）
    assert_eq!(tl.stats.tool_calls, 0);
    assert_eq!(tl.stats.tool_failed, 0);
    assert_eq!(tl.stats.unreadable, 1);
}

// ---------------------------------------------------------------- CLI

#[tokio::test]
async fn only_and_tail_filter_display_but_not_summary() {
    let tmp = TempDir::new().unwrap();
    let dir = full_task(&tmp).await;

    let (code, out, _) = run(&[
        "--dir",
        &dir.display().to_string(),
        "--only",
        "model_call",
        "--tail",
        "1",
    ]);

    assert_eq!(code, 0);
    assert!(out.contains("显示 1/12 条"), "{out}");
    assert!(out.contains("模型调用  2 次"), "汇总仍按全量：{out}");
    assert!(out.contains("12 条全部闭合"), "校验也按全量：{out}");
}

#[tokio::test]
async fn json_output_is_machine_readable() {
    let tmp = TempDir::new().unwrap();
    let dir = full_task(&tmp).await;

    let (code, out, _) = run(&["--dir", &dir.display().to_string(), "--json"]);
    let data: Value = serde_json::from_str(&out).unwrap();

    assert_eq!(code, 0);
    assert_eq!(data["task_id"], json!("tsk_demo"));
    assert_eq!(data["finalized"], json!(true));
    assert_eq!(data["chain"]["ok"], json!(true));
    assert_eq!(data["chain"]["checked"], json!(12));
    assert_eq!(data["summary"]["model_calls"], json!(2));
    assert_eq!(data["events"].as_array().unwrap().len(), 12);
    assert_eq!(data["events"][0]["kind"], json!("task_created"));
}

fn set_mtime(path: &Path, secs: u64) {
    let f = File::options().write(true).open(path).unwrap();
    f.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(secs)))
        .unwrap();
}

#[tokio::test]
async fn list_orders_newest_first_and_flags_broken() {
    let tmp = TempDir::new().unwrap();
    let old = write_full_task(tmp.path(), "tsk_old").await;
    write_full_task(tmp.path(), "tsk_new").await;
    rewrite_line(&old, "tsk_old", 0, |row| {
        row["payload"]["title"] = json!("改过了")
    });

    // 两个任务是连着写的，mtime 可能一模一样 —— 把顺序钉死，别让文件系统精度决定断言
    set_mtime(
        &tmp.path().join("tsk_old").join("events.jsonl"),
        1_700_000_000,
    );
    set_mtime(
        &tmp.path().join("tsk_new").join("events.jsonl"),
        1_700_000_600,
    );

    let (code, out, _) = run(&["--list", "--root", &tmp.path().display().to_string()]);

    assert_eq!(code, 1, "有断链 → 非零");
    assert_eq!(
        list_tasks(tmp.path(), 0.0, 0.0)
            .iter()
            .map(|r| r.task_id.clone())
            .collect::<Vec<_>>(),
        ["tsk_new", "tsk_old"]
    );
    assert!(
        out.find("tsk_new") < out.find("tsk_old"),
        "最近写入的排前面：{out}"
    );
    assert!(out.contains("证据链不可信"), "{out}");
    let after = out.split("证据链不可信").nth(1).unwrap();
    assert!(after.contains("tsk_old"), "{after}");
    assert!(!after.contains("tsk_new"), "{after}");
}

#[tokio::test]
async fn missing_task_exits_two() {
    let tmp = TempDir::new().unwrap();
    let (code, _, err) = run(&["--dir", &tmp.path().join("nope").display().to_string()]);

    assert_eq!(code, 2);
    assert!(err.contains("找不到证据"), "{err}");
}

#[tokio::test]
async fn bad_only_value_exits_two() {
    let tmp = TempDir::new().unwrap();
    let dir = full_task(&tmp).await;
    let (code, _, err) = run(&[
        "--dir",
        &dir.display().to_string(),
        "--only",
        "model_call,不存在的kind",
    ]);

    assert_eq!(code, 2);
    assert!(err.contains("不认识的 kind"), "{err}");
}

#[tokio::test]
async fn list_and_dir_together_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let (code, _, err) = run(&["--list", "--dir", &tmp.path().display().to_string()]);

    assert_eq!(code, 2);
    assert!(err.contains("--root"), "{err}");
}

// ---------------------------------------------------------------- 不许漏密钥

const SECRET: &str = "s3cr3t-do-not-print-me";

#[tokio::test]
async fn secretish_tool_arguments_are_redacted() {
    // 工具参数里键名带 token/secret/key 的，只打 ***，取值一个字都不许出去。
    let tmp = TempDir::new().unwrap();
    let w = FileEvidenceWriter::new(tmp.path().to_path_buf());
    w.append(
        "tsk_sec",
        EvidenceKind::ToolCall,
        p(json!({"call_id": "c1", "name": "http_get",
                 "arguments": {"url": "https://x/y", "api_key": SECRET, "auth_token": SECRET,
                               "password": SECRET, "harmless": "ok"}})),
    )
    .await
    .unwrap();

    let (code, out, err) = run(&["--dir", &tmp.path().join("tsk_sec").display().to_string()]);

    assert_eq!(code, 0);
    assert!(!out.contains(SECRET), "{out}");
    assert!(!err.contains(SECRET), "{err}");
    assert!(out.contains("***"), "{out}");
    assert!(out.contains("harmless=ok"), "{out}");
}

#[tokio::test]
async fn secretish_arguments_are_redacted_in_json_too() {
    let tmp = TempDir::new().unwrap();
    let w = FileEvidenceWriter::new(tmp.path().to_path_buf());
    w.append(
        "tsk_sec",
        EvidenceKind::ToolCall,
        p(json!({"call_id": "c1", "name": "http_get", "arguments": {"api_key": SECRET}})),
    )
    .await
    .unwrap();

    let (_, out, err) = run(&[
        "--dir",
        &tmp.path().join("tsk_sec").display().to_string(),
        "--json",
    ]);

    assert!(!out.contains(SECRET), "{out}");
    assert!(!err.contains(SECRET), "{err}");
}

#[tokio::test]
async fn long_arguments_are_truncated() {
    // 模型写的一大段 code 不该整段进终端 —— 截断，别把全文倒出来。
    let tmp = TempDir::new().unwrap();
    let w = FileEvidenceWriter::new(tmp.path().to_path_buf());
    let code_text = "print('x')\n".repeat(500);
    w.append(
        "tsk_long",
        EvidenceKind::ToolCall,
        p(json!({"call_id": "c1", "name": "run_python", "arguments": {"code": code_text}})),
    )
    .await
    .unwrap();

    let tl = load(&tmp.path().join("tsk_long"));

    assert!(
        tl.rows[0].detail.chars().count() < 120,
        "{}",
        tl.rows[0].detail
    );
    assert!(tl.rows[0].detail.contains('…'), "{}", tl.rows[0].detail);
}

#[tokio::test]
async fn payload_hash_only_fields_never_become_text() {
    // W8：证据里存的是 messages_hash，渲染时不许假装能还原全文。
    let tmp = TempDir::new().unwrap();
    let tl = load(&full_task(&tmp).await);
    let row = tl.rows.iter().find(|r| r.kind == "model_call").unwrap();

    assert!(!row.fields.contains_key("messages_hash"));
    assert!(!row.detail.contains(&"a".repeat(64)));
}

// ---------------------------------------------------------------- 契约没漂

#[tokio::test]
async fn verification_agrees_with_the_writer() {
    // 同一份证据，工具说 OK 时 FileEvidenceWriter::verify 也得说 true，反之亦然。
    let tmp = TempDir::new().unwrap();
    let w = write_full_task(tmp.path(), "tsk_demo").await;
    let dir = tmp.path().join("tsk_demo");

    assert!(load(&dir).ok());
    assert!(w.verify("tsk_demo"));

    rewrite_line(&w, "tsk_demo", 1, |row| {
        row["payload"]["mentioned"] = json!(false)
    });

    assert!(!load(&dir).ok());
    assert!(!w.verify("tsk_demo"));
}

#[test]
fn canonical_json_is_the_contract_one() {
    // 花费和 hash 都建立在契约那两个函数上，这里钉一下没被本地实现替换掉。
    assert_eq!(canonical_json(&p(json!({"a": 1}))), r#"{"a":1}"#);
    assert_eq!(
        payload_hash_of(&p(json!({"a": 1}))),
        "015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
    );
}

// ---------------------------------------------------------------- 边角

#[tokio::test]
async fn broken_manifest_is_not_reported_as_unfinalized() {
    // manifest.json 在但读不了，和「压根没写」是两码事，别混成一句话。
    let tmp = TempDir::new().unwrap();
    let dir = full_task(&tmp).await;
    std::fs::write(dir.join("manifest.json"), "not json at all").unwrap();

    let tl = load(&dir);
    let text = render_text(&tl, &tl.rows, false);

    assert!(tl.finalized());
    assert!(tl.manifest.is_none());
    assert!(!tl.ok());
    assert!(
        tl.manifest_problems.join(" ").contains("manifest.json"),
        "{:?}",
        tl.manifest_problems
    );
    assert!(!text.contains("未 finalize"), "{text}");
}

#[tokio::test]
async fn negative_tail_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let dir = full_task(&tmp).await;
    let (code, _, err) = run(&["--dir", &dir.display().to_string(), "--tail", "-3"]);

    assert_eq!(code, 2);
    assert!(err.contains("--tail"), "{err}");
}

#[test]
fn list_says_when_the_root_is_missing() {
    let tmp = TempDir::new().unwrap();
    let (code, out, _) = run(&[
        "--list",
        "--root",
        &tmp.path().join("nowhere").display().to_string(),
    ]);

    assert_eq!(code, 0);
    assert!(out.contains("这个目录不存在"), "{out}");
}

#[tokio::test]
async fn tail_zero_shows_nothing() {
    let tmp = TempDir::new().unwrap();
    let tl = load(&full_task(&tmp).await);

    assert!(filter_rows(&tl.rows, None, Some(0)).is_empty());
    assert_eq!(filter_rows(&tl.rows, None, Some(3)).len(), 3);
    assert_eq!(filter_rows(&tl.rows, None, None).len(), 12);
}
