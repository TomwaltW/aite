//! 够用就好的 JSON Schema 子集校验器（对应旧 `aite/testing/jsonschema_mini.py`）。
//!
//! 只覆盖冻结的那几份 `ToolSpec.parameters` 实际用到的关键字：
//! type / properties / required / items / minItems / maxItems / minimum / maximum / enum，
//! 外加把 `default` 填进结果。
//!
//! 与 `aite-gateway::schema`（R6，用 jsonschema crate）**有意不同**：这一份关键字更少
//! （没有 minLength / maxLength / exclusive*），而且**不拒未知参数**。两份差异是有意保留的
//! （清单 §1）：FakeToolGateway 与 ModelProbe 吃的是这一份。
//!
//! 返回值是「补齐默认值之后的 arguments」，出错则给一句带路径的话，直接能塞进
//! `ToolResult.error.message` 给模型看。
use serde_json::{Map, Value};

/// 参数不合 schema。消息带路径，模型看得懂改哪儿。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SchemaError(pub String);

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "int"
            } else {
                "float"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn matches_type(expected: &str, v: &Value) -> Result<bool, SchemaError> {
    Ok(match expected {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        // bool 在 Python 里是 int 的子类，整数/数字校验必须先把它排掉；
        // serde_json 天然分开，这里保留同样的语义注释以便对读。
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "null" => v.is_null(),
        other => return Err(SchemaError(format!("schema 用了不支持的 type={other:?}"))),
    })
}

/// 按 schema 校验并返回补过默认值的副本。
pub fn validate(value: &Value, schema: &Value) -> Result<Value, SchemaError> {
    validate_at(value, schema, "arguments")
}

/// 顶层是 object 时的便捷入口：`arguments` 直接进、补默认值后原样出。
pub fn validate_arguments(
    arguments: &Map<String, Value>,
    schema: &Value,
) -> Result<Map<String, Value>, SchemaError> {
    let out = validate(&Value::Object(arguments.clone()), schema)?;
    match out {
        Value::Object(map) => Ok(map),
        other => Err(SchemaError(format!(
            "arguments: 期望 object，实际 {}",
            type_name(&other)
        ))),
    }
}

fn validate_at(value: &Value, schema: &Value, path: &str) -> Result<Value, SchemaError> {
    let expected = schema.get("type").and_then(Value::as_str);
    if let Some(expected) = expected
        && !matches_type(expected, value).map_err(|e| SchemaError(format!("{path}: {}", e.0)))?
    {
        return Err(SchemaError(format!(
            "{path}: 期望 {expected}，实际 {}（{value}）",
            type_name(value)
        )));
    }

    if let Some(Value::Array(choices)) = schema.get("enum")
        && !choices.contains(value)
    {
        return Err(SchemaError(format!(
            "{path}: 必须是 {} 之一，实际 {value}",
            Value::Array(choices.clone())
        )));
    }

    match expected {
        Some("object") => validate_object(value, schema, path),
        Some("array") => validate_array(value, schema, path),
        Some("integer") | Some("number") => {
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
                && value.as_f64().is_some_and(|v| v < min)
            {
                return Err(SchemaError(format!(
                    "{path}: 不能小于 {}，实际 {value}",
                    schema["minimum"]
                )));
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
                && value.as_f64().is_some_and(|v| v > max)
            {
                return Err(SchemaError(format!(
                    "{path}: 不能大于 {}，实际 {value}",
                    schema["maximum"]
                )));
            }
            Ok(value.clone())
        }
        _ => Ok(value.clone()),
    }
}

fn validate_object(value: &Value, schema: &Value, path: &str) -> Result<Value, SchemaError> {
    let obj = value
        .as_object()
        .ok_or_else(|| SchemaError(format!("{path}: 期望 object，实际 {}", type_name(value))))?;
    let empty = Map::new();
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    if let Some(Value::Array(required)) = schema.get("required") {
        let missing: Vec<&str> = required
            .iter()
            .filter_map(Value::as_str)
            .filter(|k| !obj.contains_key(*k))
            .collect();
        if !missing.is_empty() {
            let mut got: Vec<&String> = obj.keys().collect();
            got.sort();
            return Err(SchemaError(format!(
                "{path}: 缺少必填参数 {missing:?}（收到 {got:?}）"
            )));
        }
    }

    // 未知参数**不拒**（与真 Gateway 的策略有意不同，见模块 docstring）
    let mut out = Map::new();
    for (key, item) in obj {
        match props.get(key) {
            None => {
                out.insert(key.clone(), item.clone());
            }
            Some(sub) => {
                out.insert(
                    key.clone(),
                    validate_at(item, sub, &format!("{path}.{key}"))?,
                );
            }
        }
    }
    for (key, sub) in props {
        if !out.contains_key(key)
            && let Some(default) = sub.get("default")
        {
            out.insert(key.clone(), default.clone());
        }
    }
    Ok(Value::Object(out))
}

fn validate_array(value: &Value, schema: &Value, path: &str) -> Result<Value, SchemaError> {
    let items = value
        .as_array()
        .ok_or_else(|| SchemaError(format!("{path}: 期望 array，实际 {}", type_name(value))))?;
    if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
        && (items.len() as u64) < min
    {
        return Err(SchemaError(format!(
            "{path}: 至少 {min} 项，实际 {} 项",
            items.len()
        )));
    }
    if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
        && (items.len() as u64) > max
    {
        return Err(SchemaError(format!(
            "{path}: 最多 {max} 项，实际 {} 项",
            items.len()
        )));
    }
    match schema.get("items") {
        None => Ok(Value::Array(items.clone())),
        Some(sub) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(validate_at(item, sub, &format!("{path}[{i}]"))?);
            }
            Ok(Value::Array(out))
        }
    }
}
