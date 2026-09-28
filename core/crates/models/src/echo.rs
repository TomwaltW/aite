//! 思考字段：从响应里捞出来（进 `raw["provider_extra"]`），并在下一轮按 `tool_call_id`
//! 回挂到那条 assistant 行上（CC6）。
//!
//! 契约的 `Message` 还没有 `provider_extra`（T0 带来，DD4 落库），所以 worker 接回历史的
//! assistant 消息里没有思考字段。这里用一份**进程内**缓存兜底：`chat()` 拿到「带
//! `tool_calls` 且有思考字段」的响应时，对每个 `call_id` 存一条；`request_body` 把历史翻成
//! OpenAI 行之后，逐个带 `tool_calls` 的 assistant 行按 `call_id` 查，命中且指纹一致才插回去。
//!
//! * **指纹** = `name` + 解析后的 `arguments` 再序列化（与 `to_openai_messages` 同一写法）。
//!   不用响应里的原始 `arguments` 串：厂商的空白与键序和重新序列化的对不上，真实流量里就
//!   永远回挂不上。核指纹是因为有的厂商 tool_call id 按序号编（Kimi 的 `search:0`），全进程
//!   共用一个客户端，并发任务之间会撞 id。
//! * **读了不删**：worker 每一步都把整段历史重发一遍，删了第二步之后就回挂不上了。
//! * **有界**：满了先进先出淘汰，见 [`ECHO_CACHE_CAP`]。
use std::collections::{HashMap, VecDeque};

use aite_contracts::ToolCallRequest;
use serde_json::{Map, Value};

use crate::vendor::Vendor;

/// 缓存条数上限。一个任务最多 `max_steps` 步（默认 40，`config/aite.example.yaml`），
/// 每步至多一批 tool_call；T0c 之后并发任务默认 4 个 —— 40 × 4 = 160 条就够装下所有在跑
/// 任务的整段历史。取 1024 留出 6 倍余量（一步多个并行 tool_call、任务首尾交叠），
/// 每条只是几 KB 的思考文本，上限约几 MB。
pub(crate) const ECHO_CACHE_CAP: usize = 1024;

/// 响应里原样取的思考字段（有哪个取哪个）。
pub(crate) const THINKING_KEYS: [&str; 3] = [
    "reasoning_content",
    "encrypted_content",
    "reasoning_details",
];

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// 一条响应里捞到的思考字段。
#[derive(Debug, Default)]
pub(crate) struct Thinking {
    /// 写进 `raw["provider_extra"]` 的对象；空 → 不写这个键。
    pub extra: Map<String, Value>,
    /// 回挂时插进 assistant 行的字段（MiniMax 从 `<think>` 剥下来的那段不在这里）。
    pub fields: Map<String, Value>,
    /// MiniMax：从 content 开头剥下来的原样前缀（`<think>…</think>` 连同其后空白），
    /// 回挂时原样拼回 content 开头 —— 这是它自己返回的形状。
    pub content_prefix: Option<String>,
}

impl Thinking {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.content_prefix.is_none()
    }
}

/// 从 `choices[0].message` 捞思考字段；MiniMax 顺手把 `content` 开头的 `<think>` 剥掉。
/// 别的厂商的 content 一字不动。
pub(crate) fn capture(message: &Value, vendor: Vendor, content: &mut String) -> Thinking {
    let mut out = Thinking::default();
    for key in THINKING_KEYS {
        if let Some(v) = message.get(key).filter(|v| !is_blank(v)) {
            out.extra.insert(key.to_string(), v.clone());
            out.fields.insert(key.to_string(), v.clone());
        }
    }
    if vendor == Vendor::Minimax
        && let Some((prefix, inner)) = split_think(content)
    {
        // 原生 reasoning_content 已经有了就不覆盖（剥下来的那段仍在 content_prefix 里回挂）
        out.extra
            .entry("reasoning_content")
            .or_insert_with(|| Value::String(inner.to_string()));
        let rest = content[prefix.len()..].to_string();
        out.content_prefix = Some(prefix.to_string());
        *content = rest;
    }
    out
}

/// `<think>X</think>   正文` → `("<think>X</think>   ", "X")`。content 不是以 `<think>`
/// 开头、或没有闭合标签 → `None`。
fn split_think(content: &str) -> Option<(&str, &str)> {
    let body = content.strip_prefix(THINK_OPEN)?;
    let close = body.find(THINK_CLOSE)?;
    let inner = &body[..close];
    let after = &body[close + THINK_CLOSE.len()..];
    let rest = after.trim_start();
    let prefix_len = content.len() - rest.len();
    Some((&content[..prefix_len], inner))
}

fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// 指纹：`name` + 解析后的 `arguments` 再序列化（与 `to_openai_messages` 同一写法）。
pub(crate) fn fingerprint(tc: &ToolCallRequest) -> String {
    let args = serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".to_string());
    format!("{}\u{0}{args}", tc.name)
}

#[derive(Debug, Clone)]
struct Entry {
    fingerprint: String,
    fields: Map<String, Value>,
    content_prefix: Option<String>,
}

/// `call_id` → 思考字段。有界，先进先出。
#[derive(Debug, Default)]
pub(crate) struct EchoCache {
    map: HashMap<String, Entry>,
    order: VecDeque<String>,
}

impl EchoCache {
    /// 对这一轮的每个 tool_call 存一条。同一个 `call_id` 再来一次（按序号编 id 的厂商）
    /// → 覆盖旧条并挪到队尾。
    pub fn remember(&mut self, calls: &[ToolCallRequest], thinking: &Thinking) {
        if thinking.is_empty() {
            return;
        }
        for tc in calls {
            let entry = Entry {
                fingerprint: fingerprint(tc),
                fields: thinking.fields.clone(),
                content_prefix: thinking.content_prefix.clone(),
            };
            if self.map.insert(tc.call_id.clone(), entry).is_some() {
                self.order.retain(|id| id != &tc.call_id);
            }
            self.order.push_back(tc.call_id.clone());
            while self.order.len() > ECHO_CACHE_CAP {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }

    /// 一条带 `tool_calls` 的 assistant 行：任一 `call_id` 命中且指纹一致 → 把字段插回去
    /// （行里已有同名键就不覆盖）；MiniMax 的前缀拼回 content 开头（已以 `<think>` 开头就不拼）。
    /// 读了不删。
    pub fn reattach(&self, row: &mut Map<String, Value>, calls: &[ToolCallRequest]) {
        let Some(entry) = calls.iter().find_map(|tc| {
            self.map
                .get(&tc.call_id)
                .filter(|e| e.fingerprint == fingerprint(tc))
        }) else {
            return;
        };
        for (k, v) in &entry.fields {
            if !row.contains_key(k) {
                row.insert(k.clone(), v.clone());
            }
        }
        if let Some(prefix) = &entry.content_prefix {
            let content = row.get("content").and_then(Value::as_str).unwrap_or("");
            if !content.starts_with(THINK_OPEN) {
                let joined = format!("{prefix}{content}");
                row.insert("content".into(), Value::String(joined));
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, q: i64) -> ToolCallRequest {
        ToolCallRequest {
            call_id: id.into(),
            name: "search".into(),
            arguments: json!({ "q": q }).as_object().cloned().unwrap_or_default(),
        }
    }

    fn thinking(text: &str) -> Thinking {
        let mut t = Thinking::default();
        t.fields
            .insert("reasoning_content".into(), Value::String(text.into()));
        t
    }

    fn reattached(cache: &EchoCache, tc: &ToolCallRequest) -> Option<Value> {
        let mut row = Map::new();
        cache.reattach(&mut row, std::slice::from_ref(tc));
        row.get("reasoning_content").cloned()
    }

    #[test]
    fn echo_cache_evicts_oldest_first_and_stays_bounded() {
        let mut cache = EchoCache::default();
        for i in 0..(ECHO_CACHE_CAP + 10) {
            cache.remember(&[call(&format!("c{i}"), 1)], &thinking(&format!("t{i}")));
        }
        assert_eq!(cache.len(), ECHO_CACHE_CAP);
        assert_eq!(cache.order.len(), ECHO_CACHE_CAP);
        assert_eq!(reattached(&cache, &call("c0", 1)), None, "最老的先淘汰");
        assert_eq!(reattached(&cache, &call("c9", 1)), None);
        assert_eq!(
            reattached(&cache, &call("c10", 1)),
            Some(json!("t10")),
            "第 11 条起还在"
        );
        // 读了不删
        assert_eq!(reattached(&cache, &call("c10", 1)), Some(json!("t10")));

        // 同一个 id 再来一次：覆盖、挪到队尾，队列里不留旧的
        cache.remember(&[call("c10", 2)], &thinking("again"));
        assert_eq!(cache.order.len(), ECHO_CACHE_CAP);
        assert_eq!(cache.order.back().map(String::as_str), Some("c10"));
        assert_eq!(reattached(&cache, &call("c10", 2)), Some(json!("again")));
    }

    #[test]
    fn echo_cache_requires_matching_fingerprint() {
        let mut cache = EchoCache::default();
        cache.remember(&[call("search:0", 1)], &thinking("任务 A 的思考"));
        // 另一个任务撞了同一个按序号编的 id，但参数不同 → 不挂
        assert_eq!(reattached(&cache, &call("search:0", 2)), None);
        assert_eq!(
            reattached(&cache, &call("search:0", 1)),
            Some(json!("任务 A 的思考"))
        );
        // 行里已有同名键就不覆盖
        let mut row = Map::new();
        row.insert("reasoning_content".into(), json!("已有"));
        cache.reattach(&mut row, &[call("search:0", 1)]);
        assert_eq!(row["reasoning_content"], json!("已有"));
    }
}
