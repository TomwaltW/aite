//! 证据链（对应旧 aite/contracts/evidence.py）。hash 口径逐字节与 Python 版一致。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::events::str_enum;

str_enum! {
    EvidenceKind {
        TaskCreated => "task_created",
        EventReceived => "event_received",
        /// payload: 模型名、messages hash、usage、finish_reason（不存全文）
        ModelCall => "model_call",
        ChecklistOp => "checklist_op",
        /// payload: name、arguments
        ToolCall => "tool_call",
        /// payload: ok、error、content_hash、duration_ms
        ToolResult => "tool_result",
        /// payload: title、mime、sha256、size
        Artifact => "artifact",
        Delivered => "delivered",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEvent {
    pub task_id: String,
    /// 从 0 开始
    pub seq: u64,
    pub kind: EvidenceKind,
    /// sha256(canonical_json(payload)) hex
    pub payload_hash: String,
    /// payload 内联时 None；>64KB 时为 payloads/{seq}.json 相对路径
    pub payload_ref: Option<String>,
    /// 内联 payload
    pub payload: Option<Map<String, Value>>,
    /// 第 0 条为 GENESIS
    pub prev_hash: String,
    /// sha256(prev_hash + payload_hash) hex
    pub hash: String,
    pub created_at: DateTime<Utc>,
}

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// 等价于 Python `json.dumps(payload, sort_keys=True, ensure_ascii=False, separators=(",", ":"))`：
/// 键按码点排序（递归）、无空白、非 ASCII 原样、控制字符/引号/反斜杠转义。
/// 不依赖 serde_json 的 Map 顺序（哪怕别的 crate 开了 preserve_order 也不受影响）。
pub fn canonical_json(payload: &Map<String, Value>) -> String {
    let mut out = String::new();
    write_object(payload, &mut out);
    out
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => write_object(map, out),
    }
}

fn write_object(map: &Map<String, Value>, out: &mut String) {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort(); // UTF-8 字节序 == 码点序，与 Python str 排序一致
    out.push('{');
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_string(k, out);
        out.push(':');
        write_value(&map[k.as_str()], out);
    }
    out.push('}');
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn chain_hash(prev_hash: &str, payload_hash: &str) -> String {
    sha256_hex(format!("{prev_hash}{payload_hash}").as_bytes())
}

pub fn payload_hash_of(payload: &Map<String, Value>) -> String {
    sha256_hex(canonical_json(payload).as_bytes())
}

// 测试向量（tests/evidence_vectors.rs 逐字节校验）：
//   payload {"a": 1}      canonical '{"a":1}'
//   payload_hash          015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862
//   hash(GENESIS, ·)      cdae94bd29b7de0c2a7af392f4d984ed5a41b15bdd872f7b9726132508047adc
//   payload {"b": "文"}   canonical '{"b":"文"}'
//   payload_hash          1e8763171f38ca61b0bb0f996142a149ce16ba66c664d341d03c91f54ae4ea10
//   hash(上一条 hash, ·)  11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e
// 落盘：{evidence_dir}/{task_id}/events.jsonl（一行一个 EvidenceEvent）+ manifest.json
//   manifest = {task_id, session_id, task_no, created_by, model, contract_version, root_hash(最后一条 hash), event_count}
