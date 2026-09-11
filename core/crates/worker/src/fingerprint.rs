//! T20「同一张牌」的指纹。对应旧 `aite/worker/loop.py::_call_signature`。
//!
//! 刻意和观测侧（旧 `aite/evals/protocol_probe.py` 的 `_repeat_loops()`，Rust 版是
//! R7 的 probe）用**同一个函数**：观测侧说「打转了 33 次」而兜底侧说「没打转」的话，
//! 排查时两边会打架，所以「同一张牌」在两处必须指同一件事。R7 直接 `use` 这里。
//!
//! 格式沿用 Python `json.dumps(dict(sorted(args.items())), ensure_ascii=False)`：
//! 顶层 key 排序、`, ` 与 `: ` 两个分隔符带空格、非 ASCII 原样输出。
//! 与 Python 的唯一差别：**嵌套对象的 key 也排序**（Python 保留插入序）——
//! Rust 侧 `serde_json::Map` 的顺序取决于是否开 `preserve_order`，显式排序才稳定。
use aite_contracts::ToolCallRequest;
use serde_json::{Map, Value};

/// 一次工具调用的指纹：工具名 + 顶层 key 排过序的 JSON。
pub fn call_signature(call: &ToolCallRequest) -> String {
    format!("{}:{}", call.name, stringify_object(&call.arguments))
}

/// 等价于 Python `json.dumps(value, ensure_ascii=False)`（默认分隔符带空格）。
pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn stringify_object(map: &Map<String, Value>) -> String {
    let mut out = String::new();
    write_object(map, &mut out);
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
                    out.push_str(", ");
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
    keys.sort();
    out.push('{');
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_string(k, out);
        out.push_str(": ");
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

/// Python `repr()` 的够用版本：错误文案「没有这一项：{id!r}」要靠它。
/// 字符串用单引号（内含单引号且不含双引号时改用双引号，跟 CPython 一致）。
pub fn py_repr(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => {
            let quote = if s.contains('\'') && !s.contains('"') {
                '"'
            } else {
                '\''
            };
            let mut out = String::with_capacity(s.len() + 2);
            out.push(quote);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c);
                    }
                    c => out.push(c),
                }
            }
            out.push(quote);
            out
        }
        // 容器类型只有模型乱给参数时才会撞上，给个可读形态即可（不进证据、不进断言）。
        Some(other) => stringify(other),
    }
}
