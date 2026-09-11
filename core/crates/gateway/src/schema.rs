//! 按 `ToolSpec.parameters` 校验 arguments（对应旧 `aite/gateway/schema.py`）。
//!
//! §3.2 的调用顺序里第三步是「按 ToolSpec.parameters 校验 arguments」。校验本体交给
//! `jsonschema` crate（workspace 已钉 0.56），这一层只做三件旧实现里的本层策略：
//!
//! 1. **未知参数直接拒**（JSON Schema 默认允许额外字段；这是给模型用的工具入口，
//!    参数名写错时早点收到 invalid_args 比静默丢掉好）。消息里列出可用参数名，模型能照着改。
//! 2. **补 `default`**，返回可以直接喂给工具实现的 arguments（嵌套对象里的 default 也补）。
//! 3. **不做类型转换**，而且比 JSON Schema 更严一条：`integer` 只认整数字面量。
//!    draft 2020-12 把 `50.0` 当合法 integer，旧实现的 `isinstance(v, int)` 不认 ——
//!    这里补一道严格检查，保持与 Python 版逐条一致（`bool` 不是 integer 由 JSON 类型系统天然保证）。
//!
//! 错误消息沿用旧实现的中文措辞：它会原样进 `ToolResult.content` 给模型看。
use jsonschema::error::{TypeKind, ValidationErrorKind};
use jsonschema::paths::LocationSegment;
use serde_json::{Map, Value};

/// arguments 不合 schema。Gateway 把它翻成 `ToolErrorCode::InvalidArgs`。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SchemaViolation(pub String);

impl SchemaViolation {
    fn of(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// 校验并补默认值，返回可以直接喂给工具实现的 arguments。
///
/// 顺序与旧实现一致：顶层 schema 形状 → 未知参数 → 缺必填 → 逐字段校验 → 补 default。
/// 顺序要紧：打错一个参数名时，「不认识的参数」比「缺少必填参数」更能让模型改对。
pub fn validate_arguments(
    schema: &Value,
    arguments: &Map<String, Value>,
) -> Result<Map<String, Value>, SchemaViolation> {
    match schema.get("type") {
        None => {}
        Some(Value::String(t)) if t == "object" => {}
        Some(other) => {
            return Err(SchemaViolation::of(format!(
                "工具的 parameters 顶层必须是 object，schema 写的是 {other}"
            )));
        }
    }

    let empty = Map::new();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let mut unknown: Vec<&str> = arguments
        .keys()
        .filter(|k| !properties.contains_key(k.as_str()))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        let mut known: Vec<&str> = properties.keys().map(String::as_str).collect();
        known.sort_unstable();
        let known = if known.is_empty() {
            "[\"（这个工具不接受任何参数）\"]".to_string()
        } else {
            render_list(&known)
        };
        return Err(SchemaViolation::of(format!(
            "不认识的参数 {}；可用的是 {known}",
            render_list(&unknown)
        )));
    }

    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|name| !arguments.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        return Err(SchemaViolation::of(format!(
            "缺少必填参数 {}",
            render_list(&missing)
        )));
    }

    let validator = jsonschema::validator_for(schema)
        .map_err(|e| SchemaViolation::of(format!("工具的 parameters schema 有问题：{e}")))?;
    let instance = Value::Object(arguments.clone());
    if let Some(error) = validator.iter_errors(&instance).next() {
        return Err(SchemaViolation::of(render_violation(&error)));
    }

    match walk(&instance, schema, "")? {
        Value::Object(map) => Ok(map),
        // instance 是上面亲手包的 Object，walk 不改变种类。
        other => Err(SchemaViolation::of(format!(
            "校验后的 arguments 不是对象：{other}"
        ))),
    }
}

/// 递归补 `default` + 严格 integer 检查。返回新值，不原地改。
fn walk(value: &Value, schema: &Value, path: &str) -> Result<Value, SchemaViolation> {
    check_strict_integer(value, schema, path)?;
    match value {
        Value::Object(map) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return Ok(value.clone());
            };
            let mut out = map.clone();
            for (name, subschema) in properties {
                let child = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                if let Some(given) = map.get(name) {
                    out.insert(name.clone(), walk(given, subschema, &child)?);
                } else if let Some(default) = subschema.get("default") {
                    out.insert(name.clone(), default.clone());
                }
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => {
            let Some(subschema) = schema.get("items").filter(|s| s.is_object()) else {
                return Ok(value.clone());
            };
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(walk(item, subschema, &format!("{path}[{i}]"))?);
            }
            Ok(Value::Array(out))
        }
        _ => Ok(value.clone()),
    }
}

/// `integer` 只认整数字面量：`50.0` 在 JSON Schema 里算 integer，旧实现不算。
fn check_strict_integer(value: &Value, schema: &Value, path: &str) -> Result<(), SchemaViolation> {
    let types = declared_types(schema);
    if !types.contains(&"integer") || types.contains(&"number") {
        return Ok(());
    }
    if let Value::Number(n) = value
        && n.as_i64().is_none()
        && n.as_u64().is_none()
    {
        return Err(SchemaViolation::of(format!(
            "{} 应该是 {}，收到 {}",
            display_path(path),
            types.join("/"),
            received(value)
        )));
    }
    Ok(())
}

fn declared_types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn render_violation(error: &jsonschema::ValidationError<'_>) -> String {
    let raw_path = python_path(error.instance_path());
    let path = display_path(&raw_path);
    let instance = error.instance();
    match error.kind() {
        ValidationErrorKind::Type {
            kind: TypeKind::Single(expected),
        } => format!("{path} 应该是 {expected}，收到 {}", received(instance)),
        ValidationErrorKind::Type {
            kind: TypeKind::Multiple(expected),
        } => {
            let names: Vec<String> = expected.into_iter().map(|t| t.to_string()).collect();
            format!(
                "{path} 应该是 {}，收到 {}",
                names.join("/"),
                received(instance)
            )
        }
        ValidationErrorKind::Enum { options } => {
            format!("{path} 只能是 {options} 之一，收到 {instance}")
        }
        ValidationErrorKind::Minimum { limit } => {
            format!("{path} 不能小于 {limit}，收到 {instance}")
        }
        ValidationErrorKind::Maximum { limit } => {
            format!("{path} 不能大于 {limit}，收到 {instance}")
        }
        ValidationErrorKind::ExclusiveMinimum { limit } => {
            format!("{path} 必须大于 {limit}，收到 {instance}")
        }
        ValidationErrorKind::ExclusiveMaximum { limit } => {
            format!("{path} 必须小于 {limit}，收到 {instance}")
        }
        ValidationErrorKind::MinLength { limit } => format!(
            "{path} 至少要 {limit} 个字符，收到 {} 个",
            char_count(instance)
        ),
        ValidationErrorKind::MaxLength { limit } => format!(
            "{path} 最多 {limit} 个字符，收到 {} 个",
            char_count(instance)
        ),
        ValidationErrorKind::MinItems { limit } => {
            format!("{path} 至少要 {limit} 项，收到 {} 项", item_count(instance))
        }
        ValidationErrorKind::MaxItems { limit } => {
            format!("{path} 最多 {limit} 项，收到 {} 项", item_count(instance))
        }
        ValidationErrorKind::Required { property } => {
            format!("{path} 缺少必填字段 [{property}]")
        }
        _ => format!("{path} 不合 schema：{error}"),
    }
}

/// jsonschema 的 instance_path 是 JSON Pointer（`/who/id`、`/items/0`）；
/// 旧实现的消息用 `who.id` / `items[0]`，照它渲染，模型看到的字样才一致。
fn python_path(location: &jsonschema::paths::Location) -> String {
    let mut out = String::new();
    for segment in location.segments() {
        match segment {
            LocationSegment::Property(name) => {
                if out.is_empty() {
                    out.push_str(&name);
                } else {
                    out.push('.');
                    out.push_str(&name);
                }
            }
            LocationSegment::Index(i) => {
                out.push_str(&format!("[{i}]"));
            }
        }
    }
    out
}

fn display_path(path: &str) -> &str {
    if path.is_empty() { "arguments" } else { path }
}

/// 与旧实现的 `_name_of` 同一用途：把收到的值连类型一起说出来（截到 80 字符）。
fn received(value: &Value) -> String {
    if value.is_null() {
        return "null".to_string();
    }
    let kind = match value {
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Null => "null",
    };
    let text = format!("{kind}({value})");
    text.chars().take(80).collect()
}

fn char_count(value: &Value) -> usize {
    value.as_str().map(|s| s.chars().count()).unwrap_or(0)
}

fn item_count(value: &Value) -> usize {
    value.as_array().map(Vec::len).unwrap_or(0)
}

fn render_list(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("\"{s}\"")).collect();
    format!("[{}]", quoted.join(", "))
}
