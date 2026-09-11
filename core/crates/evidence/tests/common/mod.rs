//! 各测试文件共用的小工具。
#![allow(dead_code)]

use aite_contracts::EvidenceEvent;
use aite_evidence::FileEvidenceWriter;
use serde_json::{Map, Value};

/// `json!({...})` → payload（契约要的是 `Map<String, Value>`）。
pub fn p(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => panic!("payload 必须是对象，得到 {other}"),
    }
}

pub fn read_text(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).expect("读不了")
}

/// events.jsonl 的每一行（不含末尾空行）。
pub fn lines_of(writer: &FileEvidenceWriter, task_id: &str) -> Vec<String> {
    read_text(&writer.events_path(task_id))
        .lines()
        .map(str::to_string)
        .collect()
}

pub fn events_of(writer: &FileEvidenceWriter, task_id: &str) -> Vec<EvidenceEvent> {
    lines_of(writer, task_id)
        .iter()
        .map(|l| serde_json::from_str(l).expect("解析 EvidenceEvent"))
        .collect()
}

pub fn manifest_of(writer: &FileEvidenceWriter, task_id: &str) -> Map<String, Value> {
    p(serde_json::from_str(&read_text(&writer.manifest_path(task_id))).expect("解析 manifest"))
}

/// 在 events.jsonl 末尾留下一段没有换行符收尾的字节。
pub fn tear(writer: &FileEvidenceWriter, task_id: &str, text: &str) {
    use std::io::Write;
    let path = writer.events_path(task_id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut fh = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    fh.write_all(text.as_bytes()).unwrap();
}

/// 把 events.jsonl 整份换成这些行（每行一个 `\n` 收尾）。
pub fn rewrite_lines(writer: &FileEvidenceWriter, task_id: &str, lines: &[String]) {
    let mut text = lines.join("\n");
    text.push('\n');
    std::fs::write(writer.events_path(task_id), text).unwrap();
}
