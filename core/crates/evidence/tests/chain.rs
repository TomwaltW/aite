//! B7：§3.1 的两个 hash 测试向量逐字节相等；篡改任一 payload 后 verify 返回 false
//! （移植 `tests/evidence/test_chain.py` 10 条；参数化那 1 处 2 case 拆成 2 条）。

mod common;

use aite_contracts::{EvidenceKind, EvidenceWriter, GENESIS};
use aite_evidence::FileEvidenceWriter;
use common::{events_of, lines_of, p, read_text, rewrite_lines};
use serde_json::{Value, json};
use tempfile::TempDir;

// §3.1 evidence.rs 末尾的测试向量，逐字符抄下来
const VEC1_PAYLOAD_HASH: &str = "015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862";
const VEC1_HASH: &str = "cdae94bd29b7de0c2a7af392f4d984ed5a41b15bdd872f7b9726132508047adc";
const VEC2_PAYLOAD_HASH: &str = "1e8763171f38ca61b0bb0f996142a149ce16ba66c664d341d03c91f54ae4ea10";
const VEC2_HASH: &str = "11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e";

const TASK: &str = "task-vec";

fn vec1() -> serde_json::Map<String, Value> {
    p(json!({"a": 1}))
}

fn vec2() -> serde_json::Map<String, Value> {
    p(json!({"b": "文"}))
}

fn writer(tmp: &TempDir) -> FileEvidenceWriter {
    FileEvidenceWriter::new(tmp.path().join("evidence"))
}

async fn write_vectors(w: &FileEvidenceWriter) {
    w.append(TASK, EvidenceKind::TaskCreated, vec1())
        .await
        .unwrap();
    w.append(TASK, EvidenceKind::ModelCall, vec2())
        .await
        .unwrap();
}

/// 改一行的 payload / 字段后原样写回。
fn rewrite(w: &FileEvidenceWriter, index: usize, mutate: impl FnOnce(&mut Value)) {
    let mut lines: Vec<Value> = lines_of(w, TASK)
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    mutate(&mut lines[index]);
    let out: Vec<String> = lines
        .iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect();
    rewrite_lines(w, TASK, &out);
}

#[tokio::test]
async fn hash_vectors_match_spec() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    let a = w
        .append(TASK, EvidenceKind::TaskCreated, vec1())
        .await
        .unwrap();
    let b = w
        .append(TASK, EvidenceKind::ModelCall, vec2())
        .await
        .unwrap();

    assert_eq!(a.seq, 0);
    assert_eq!(a.prev_hash, GENESIS);
    assert_eq!(a.payload_hash, VEC1_PAYLOAD_HASH);
    assert_eq!(a.hash, VEC1_HASH);

    assert_eq!(b.seq, 1);
    assert_eq!(b.prev_hash, VEC1_HASH);
    assert_eq!(b.payload_hash, VEC2_PAYLOAD_HASH);
    assert_eq!(b.hash, VEC2_HASH);

    assert!(w.verify(TASK));
}

#[tokio::test]
async fn payload_is_inlined_and_ref_is_none() {
    // payload_ref 在契约里没有默认值：内联时落盘是 null，不是省略。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    let ev = w
        .append(TASK, EvidenceKind::TaskCreated, vec1())
        .await
        .unwrap();
    assert!(ev.payload_ref.is_none());
    assert_eq!(ev.payload.as_ref().unwrap(), &vec1());

    let line: Value = serde_json::from_str(&lines_of(&w, TASK)[0]).unwrap();
    assert_eq!(line.get("payload_ref"), Some(&Value::Null));
    assert_eq!(line["payload"], json!({"a": 1}));
}

#[tokio::test]
async fn one_line_per_event() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    write_vectors(&w).await;

    let lines = lines_of(&w, TASK);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        events_of(&w, TASK)
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        [0, 1]
    );
}

// ---- 篡改 ----------------------------------------------------------------

#[tokio::test]
async fn tampering_payload_of_line_0_fails_verify() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    write_vectors(&w).await;
    assert!(w.verify(TASK));

    rewrite(&w, 0, |row| row["payload"] = json!({"a": 999}));
    assert!(!w.verify(TASK));
}

#[tokio::test]
async fn tampering_payload_of_line_1_fails_verify() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    write_vectors(&w).await;
    assert!(w.verify(TASK));

    rewrite(&w, 1, |row| row["payload"] = json!({"a": 999}));
    assert!(!w.verify(TASK));
}

#[tokio::test]
async fn tampering_payload_hash_fails_verify() {
    // 只改 payload_hash（让它和 payload 对上是不可能的）→ 链断
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    write_vectors(&w).await;

    rewrite(&w, 0, |row| row["payload_hash"] = json!(VEC2_PAYLOAD_HASH));
    assert!(!w.verify(TASK));
}

#[tokio::test]
async fn tampering_chain_hash_fails_verify() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    write_vectors(&w).await;

    rewrite(&w, 1, |row| row["prev_hash"] = json!(GENESIS));
    assert!(!w.verify(TASK));
}

#[tokio::test]
async fn dropping_a_line_fails_verify() {
    // 删掉中间一条 → seq 不连续，也是 false
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    write_vectors(&w).await;
    w.append(TASK, EvidenceKind::Delivered, p(json!({"c": 3})))
        .await
        .unwrap();

    let lines = lines_of(&w, TASK);
    rewrite_lines(&w, TASK, &[lines[0].clone(), lines[2].clone()]);
    assert!(!w.verify(TASK));
}

#[test]
fn verify_missing_task_is_false() {
    let tmp = TempDir::new().unwrap();
    assert!(!writer(&tmp).verify("没有这个任务"));
}

// ---- 大 payload 外置 ------------------------------------------------------

#[tokio::test]
async fn large_payload_goes_to_payload_ref() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    let big = p(json!({"blob": "x".repeat(70_000)}));
    let ev = w.append(TASK, EvidenceKind::ToolResult, big).await.unwrap();

    assert!(ev.payload.is_none());
    assert_eq!(ev.payload_ref.as_deref(), Some("payloads/0.json"));
    let external = w.task_dir(TASK).join(ev.payload_ref.as_ref().unwrap());
    assert!(external.exists());
    assert_eq!(
        read_text(&external),
        format!(r#"{{"blob":"{}"}}"#, "x".repeat(70_000))
    );
    assert!(!ev.payload_hash.is_empty()); // 仍然按原 payload 算
    assert!(w.verify(TASK));
}

#[tokio::test]
async fn chain_continues_after_new_writer_instance() {
    // 进程重启：新实例从已落盘的文件接着写，链不断。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    w.append(TASK, EvidenceKind::TaskCreated, vec1())
        .await
        .unwrap();

    let other = writer(&tmp);
    let second = other
        .append(TASK, EvidenceKind::ModelCall, vec2())
        .await
        .unwrap();

    assert_eq!(second.seq, 1);
    assert_eq!(second.prev_hash, VEC1_HASH);
    assert_eq!(second.hash, VEC2_HASH);
    assert!(other.verify(TASK));
}
