//! manifest.json 的字段就是 §3.1 写死的那一组，root_hash 取最后一条的 hash
//! （移植 `tests/evidence/test_manifest.py` 3 条）。

mod common;

use aite_contracts::{CONTRACT_VERSION, EvidenceKind, EvidenceWriter, GENESIS};
use aite_evidence::FileEvidenceWriter;
use common::{manifest_of, p};
use serde_json::json;
use tempfile::TempDir;

const TASK: &str = "task-manifest";

const EXPECTED_FIELDS: [&str; 8] = [
    "contract_version",
    "created_by",
    "event_count",
    "model",
    "root_hash",
    "session_id",
    "task_id",
    "task_no",
];

fn writer(tmp: &TempDir) -> FileEvidenceWriter {
    FileEvidenceWriter::new(tmp.path().join("evidence"))
}

#[tokio::test]
async fn manifest_shape() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    let mut last = None;
    for i in 0..3 {
        last = Some(
            w.append(TASK, EvidenceKind::ModelCall, p(json!({"step": i})))
                .await
                .unwrap(),
        );
    }

    let root = w
        .finalize(
            TASK,
            p(json!({
                "session_id": "s-1", "task_no": "#A17",
                "created_by": "ou_1", "model": "scripted-p0",
            })),
        )
        .await
        .unwrap();

    let m = manifest_of(&w, TASK);
    assert_eq!(
        m.keys().map(String::as_str).collect::<Vec<_>>(),
        EXPECTED_FIELDS,
        "不多不少"
    );
    assert_eq!(m["task_id"], json!(TASK));
    assert_eq!(m["session_id"], json!("s-1"));
    assert_eq!(m["task_no"], json!("#A17"));
    assert_eq!(m["created_by"], json!("ou_1"));
    assert_eq!(m["model"], json!("scripted-p0"));
    assert_eq!(m["contract_version"], json!(CONTRACT_VERSION));
    assert_eq!(CONTRACT_VERSION, "p0.2");
    assert_eq!(m["event_count"], json!(3));
    assert_eq!(m["root_hash"], json!(root));
    assert_eq!(root, last.unwrap().hash);
}

#[tokio::test]
async fn finalize_with_no_events() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    assert_eq!(w.finalize(TASK, Default::default()).await.unwrap(), GENESIS);

    let m = manifest_of(&w, TASK);
    assert_eq!(m["event_count"], json!(0));
    assert_eq!(
        m.keys().map(String::as_str).collect::<Vec<_>>(),
        EXPECTED_FIELDS
    );
    // 缺省字段一律空串
    for f in ["session_id", "task_no", "created_by", "model"] {
        assert_eq!(m[f], json!(""));
    }
}

#[tokio::test]
async fn directory_layout() {
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    w.append(TASK, EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();
    w.finalize(TASK, Default::default()).await.unwrap();

    let task_dir = w.task_dir(TASK);
    assert_eq!(task_dir.file_name().unwrap(), TASK);
    let mut names: Vec<String> = std::fs::read_dir(&task_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["events.jsonl", "manifest.json"]);

    // pretty（2 空格）+ 键排序 + 末尾换行
    let text = std::fs::read_to_string(w.manifest_path(TASK)).unwrap();
    assert!(text.ends_with("}\n"));
    assert!(
        text.contains("\n  \"contract_version\": "),
        "缩进 2 空格：{text}"
    );
    let first_key = text.lines().nth(1).unwrap();
    assert!(
        first_key.trim_start().starts_with("\"contract_version\""),
        "键按序：{first_key}"
    );
}
