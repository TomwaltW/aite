//! final：解析参数（CC3 从 `agent.rs` 原样搬来）。交付本身在 `deliver.rs`。
use aite_contracts::{ToolCallRequest, ToolErrorCode};
use serde_json::{Map, Value};

use crate::texts;

pub const ENABLED: bool = true;

pub(crate) struct FinalError {
    pub(crate) content: &'static str,
    pub(crate) code: ToolErrorCode,
}

pub(crate) fn parse_final(
    call: &ToolCallRequest,
) -> Result<(String, Vec<Map<String, Value>>), FinalError> {
    let args = &call.arguments;
    let reply = args.get("reply").and_then(Value::as_str);
    let Some(reply) = reply.filter(|r| !r.trim().is_empty()) else {
        return Err(FinalError {
            content: texts::FINAL_REPLY_REQUIRED,
            code: ToolErrorCode::InvalidArgs,
        });
    };
    let artifacts = match args.get("artifacts") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_object)
            .filter(|m| m.get("path").is_some_and(is_truthy))
            .cloned()
            .collect(),
        // Python 是 `raw = args.get("artifacts") or []`（loop.py:634）：**假值**
        // （`null` / `""` / `{}` / `0` / `false` / `[]`）一律当成「没有产物」照常交付，
        // 只有**真值但不是数组**才判 invalid_args。
        // 这条曾经是 Rust 侧的行为翻转：弱模型给 `artifacts: ""` 不罕见，那时 Python
        // 交付、Rust 退回重来 —— 而这是**交付路径**，退回去的代价是用户什么都收不到。
        Some(v) if !is_truthy(v) => Vec::new(),
        Some(_) => {
            return Err(FinalError {
                content: texts::FINAL_ARTIFACTS_MUST_BE_ARRAY,
                code: ToolErrorCode::InvalidArgs,
            });
        }
    };
    Ok((reply.to_string(), artifacts))
}

/// Python 的真值语义：空串 / 0 / false / null / 空容器都是假。
pub(crate) fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// 对齐 Python 的 `str(art.get(key, ""))`：字符串取原文，缺省取空串，别的取 JSON 形态。
pub(crate) fn plain_string(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}
