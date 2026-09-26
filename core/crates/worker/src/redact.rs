//! 证据写入侧脱敏（CC3 ⑨）。手写，不加依赖（worker 的 `Cargo.toml` 不在本轨可写面，regex 也不许加）。
//!
//! 五个写入点：`tool_call.arguments`（本地工具、final、gateway 工具各一处）与 `tool_result.content_summary`
//! （本地、gateway 各一处）。`content_hash` 仍对**原文**算 —— 哈希不泄露原文，而且证据要能对得上工具真实返回的东西。
//!
//! 规则：
//! - 键名**精确匹配**（大小写不敏感、任意深度）[`SECRET_KEYS`] → 值整个换成 `***`。**不做子串匹配**：
//!   `read_document` 的冻结参数 `url_or_token`（eval 06）必须原样留在证据里（渲染侧 `evidence show`
//!   的 `SECRET_HINTS` 本来就会打它）。
//! - 值层面（任何字符串里）：`sk-…` → `sk-***`；`Bearer <x>` → `Bearer ***`；
//!   `<名单键>=值` / `<名单键>: 值`（含全角冒号、带引号的 JSON 形态）→ 值换成 `***`。
//! - 摘要**先脱敏再 `clip`**，免得截断把一个密钥切成半截、刚好躲过匹配。
use serde_json::{Map, Value};

/// 值要整个打掉的键名（小写比较，精确匹配）。
pub const SECRET_KEYS: [&str; 10] = [
    "api_key",
    "apikey",
    "token",
    "access_token",
    "password",
    "passwd",
    "secret",
    "app_secret",
    "client_secret",
    "authorization",
];

const MASK: &str = "***";

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    SECRET_KEYS.contains(&lower.as_str())
}

/// 工具参数（JSON 对象）脱敏后的副本。
pub fn redact_args(args: &Map<String, Value>) -> Map<String, Value> {
    args.iter()
        .map(|(k, v)| {
            let v = if is_secret_key(k) {
                Value::String(MASK.into())
            } else {
                redact_value(v)
            };
            (k.clone(), v)
        })
        .collect()
}

/// 任意 JSON 值脱敏后的副本（对象按键名、字符串按 [`redact_text`]）。
pub fn redact_value(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(redact_args(m)),
        Value::Array(xs) => Value::Array(xs.iter().map(redact_value).collect()),
        Value::String(s) => Value::String(redact_text(s)),
        other => other.clone(),
    }
}

/// 一段自由文本脱敏（摘要、字符串参数）。
pub fn redact_text(text: &str) -> String {
    let text = mask_after_prefix(text, "sk-", |c| {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    });
    let text = mask_bearer(&text);
    mask_key_values(&text)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `<prefix><token 字符>+` → `<prefix>***`，要求 prefix 前面不是单词字符（`task-` 里的 `sk-` 不算）。
fn mask_after_prefix(text: &str, prefix: &str, token: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut prev: Option<char> = None;
    while let Some(at) = rest.find(prefix) {
        let before = &rest[..at];
        let boundary = before.chars().last().or(prev).is_none_or(|c| !is_word(c));
        out.push_str(before);
        let after = &rest[at + prefix.len()..];
        let tok_len: usize = after
            .chars()
            .take_while(|c| token(*c))
            .map(char::len_utf8)
            .sum();
        if boundary && tok_len > 0 {
            out.push_str(prefix);
            out.push_str(MASK);
            prev = Some('*');
            rest = &after[tok_len..];
        } else {
            out.push_str(prefix);
            prev = prefix.chars().last();
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// `Bearer <非空白>` → `Bearer ***`（大小写不敏感地认 `bearer`）。
fn mask_bearer(text: &str) -> String {
    let lower = text.to_lowercase();
    // 大小写折叠可能改变字节长度（极少见）；那种情况下不冒险按位置切，原样返回交给其它规则
    if lower.len() != text.len() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(off) = lower[i..].find("bearer ") {
        let at = i + off;
        let boundary = text[..at].chars().last().is_none_or(|c| !is_word(c));
        let tok_start = at + "bearer ".len();
        let tok_len: usize = text[tok_start..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
            .map(char::len_utf8)
            .sum();
        out.push_str(&text[i..tok_start]);
        if boundary && tok_len > 0 {
            out.push_str(MASK);
            i = tok_start + tok_len;
        } else {
            i = tok_start;
        }
    }
    out.push_str(&text[i..]);
    out
}

/// `<名单键>=值`、`<名单键>: 值`、`"<名单键>": "值"` → 值换成 `***`。
fn mask_key_values(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if is_word(chars[i]) && (i == 0 || !is_word(chars[i - 1])) {
            let mut j = i;
            while j < chars.len() && is_word(chars[j]) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if is_secret_key(&word)
                && let Some((value_start, value_end)) = value_span(&chars, j)
            {
                out.extend(&chars[i..value_start]);
                out.push_str(MASK);
                i = value_end;
                continue;
            }
            out.extend(&chars[i..j]);
            i = j;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 键名之后：可选的右引号、空白、`=` / `:` / `：`、空白、可选的左引号，然后是值。返回值的 `[start, end)`。
fn value_span(chars: &[char], mut k: usize) -> Option<(usize, usize)> {
    if k < chars.len() && (chars[k] == '"' || chars[k] == '\'') {
        k += 1;
    }
    while k < chars.len() && chars[k] == ' ' {
        k += 1;
    }
    if k >= chars.len() || !matches!(chars[k], '=' | ':' | '：') {
        return None;
    }
    k += 1;
    while k < chars.len() && chars[k] == ' ' {
        k += 1;
    }
    if k < chars.len() && (chars[k] == '"' || chars[k] == '\'') {
        k += 1;
    }
    let start = k;
    while k < chars.len()
        && !chars[k].is_whitespace()
        && !matches!(chars[k], '"' | '\'' | ',' | '&' | ';' | '}' | ')' | ']')
    {
        k += 1;
    }
    (k > start).then_some((start, k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rules() {
        assert_eq!(redact_text("key sk-live-ABC_123 done"), "key sk-*** done");
        assert_eq!(redact_text("task-123 不是密钥"), "task-123 不是密钥");
        assert_eq!(
            redact_text("Authorization: Bearer abc.def"),
            "Authorization: *** ***"
        );
        assert_eq!(redact_text("curl -H 'bearer xyz'"), "curl -H 'bearer ***'");
        assert_eq!(redact_text("password=hunter2&x=1"), "password=***&x=1");
        assert_eq!(redact_text(r#"{"token": "t0k"}"#), r#"{"token": "***"}"#);
        assert_eq!(redact_text("url_or_token=doccn_1"), "url_or_token=doccn_1");
        assert_eq!(redact_text("secret：值"), "secret：***");
    }

    #[test]
    fn keys_are_exact_and_case_insensitive_at_any_depth() {
        let args = serde_json::json!({
            "url_or_token": "doccn_1",
            "API_KEY": "k",
            "nested": {"Password": "p", "list": [{"token": "t"}]},
            "note": "tokenish"
        });
        let got = redact_args(args.as_object().expect("obj"));
        assert_eq!(
            Value::Object(got),
            serde_json::json!({
                "url_or_token": "doccn_1",
                "API_KEY": "***",
                "nested": {"Password": "***", "list": [{"token": "***"}]},
                "note": "tokenish"
            })
        );
    }
}
