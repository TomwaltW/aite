//! aite-models —— `ModelPort` 的 OpenAI-compatible 实现（对应旧
//! `aite/models/openai_compat.py`）。owner: R5
//!
//! 指向百炼 / 智谱 / DeepSeek 这类 OpenAI 兼容端点：`base_url`、`model`、`max_tokens`、
//! `temperature` 全部从 `ModelConfig` 读，**密钥只从 `api_key_env` 点名的环境变量取**
//! （默认 `AITE_MODEL_API_KEY`），代码、配置文件、日志、`Debug` 里都不出现取值。
//!
//! 这一层只做翻译，不做策略：
//!
//! * 契约的 `Message` / `ToolSpec` ↔ OpenAI 的 messages / tools
//! * OpenAI 的 choices / usage ↔ 契约的 `ModelTurn` / `Usage`
//! * `cost_of()` 按 `price_in_per_mtok` / `price_out_per_mtok` 折算人民币，
//!   只给卡片上的「已用 ¥」用
//!
//! **不在这里做**的两件事，避免和 worker 抢格子：
//! * 重试（§3.3「模型调用异常 / 5xx → worker 重试 2 次」是 worker 的事）
//! * §3.3 那条兜底 —— 模型既无 tool_call 也无 final 只回文本时，`steps==0` 视为
//!   `final(reply=文本)`。本层如实返回「content 有值、tool_calls 为空」的 `ModelTurn`。
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use aite_contracts::{
    Message, ModelConfig, ModelError, ModelPort, ModelTurn, Role, ToolCallRequest, ToolSpec, Usage,
};
use async_trait::async_trait;
use serde_json::{Map, Value, json};

/// 没配超时的话 HTTP 会一直挂着；worker 的重试计数因此永远走不到。
const REQUEST_TIMEOUT_SEC: u64 = 120;

// --------------------------------------------------------------------------
// 契约 -> OpenAI
// --------------------------------------------------------------------------

/// `role=tool` 的消息缺 `tool_call_id`（调用方的 bug；worker 的 `tool_message()` 不会这样）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("role=tool 的消息必须带 tool_call_id：{0}")]
pub struct MessageError(pub String);

/// 契约 `Message` 列表翻成 OpenAI 的 messages。
pub fn to_openai_messages(messages: &[Message]) -> Result<Vec<Value>, MessageError> {
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        let mut row = Map::new();
        row.insert("role".into(), Value::String(m.role.as_str().to_string()));
        row.insert("content".into(), Value::String(m.content.clone()));
        if m.role == Role::Assistant
            && let Some(calls) = m.tool_calls.as_ref().filter(|c| !c.is_empty())
        {
            let encoded: Vec<Value> = calls
                .iter()
                .map(|tc| {
                    json!({
                        "id": tc.call_id,
                        "type": "function",
                        // 中文不转义：serde_json 默认就是 ensure_ascii=False 的行为
                        "function": {
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.arguments)
                                .unwrap_or_else(|_| "{}".to_string()),
                        },
                    })
                })
                .collect();
            row.insert("tool_calls".into(), Value::Array(encoded));
            // 兼容那些不接受 content=null 的国内网关：带 tool_calls 时空内容写空串而不是 null
            row.insert("content".into(), Value::String(m.content.clone()));
        }
        if m.role == Role::Tool {
            let Some(id) = m.tool_call_id.as_ref().filter(|s| !s.is_empty()) else {
                return Err(MessageError(format!("{m:?}")));
            };
            row.insert("tool_call_id".into(), Value::String(id.clone()));
            if let Some(name) = m.name.as_ref().filter(|s| !s.is_empty()) {
                row.insert("name".into(), Value::String(name.clone()));
            }
        }
        out.push(Value::Object(row));
    }
    Ok(out)
}

/// 契约 `ToolSpec` 列表翻成 OpenAI 的 tools（function calling）。
pub fn to_openai_tools(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                },
            })
        })
        .collect()
}

// --------------------------------------------------------------------------
// OpenAI -> 契约
// --------------------------------------------------------------------------

pub fn usage_from_openai(raw: Option<&Value>) -> Usage {
    let Some(raw) = raw.filter(|v| !v.is_null()) else {
        return Usage::default();
    };
    let cached = raw
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Usage {
        input_tokens: raw
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: raw
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_tokens: cached,
    }
}

/// OpenAI 的 chat.completions 响应 → 契约 `ModelTurn`。
///
/// tool_call 的 `arguments` 是 JSON 字符串；解析不出来时**不报错**，而是把 arguments
/// 置空并把原文记进 `raw["arg_parse_errors"]` —— 让它走 §3.3「tool_call 参数不合
/// schema → invalid_args，计 1 步」那条路，比整轮炸掉好。
pub fn turn_from_response(payload: &Value) -> Result<ModelTurn, ModelError> {
    let choices = payload.get("choices").and_then(Value::as_array);
    let Some(choice) = choices.and_then(|c| c.first()) else {
        return Err(ModelError::BadResponse("模型响应里没有 choices".into()));
    };
    let msg = choice.get("message").cloned().unwrap_or_else(|| json!({}));

    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut arg_errors: Vec<Value> = Vec::new();
    if let Some(raw_calls) = msg.get("tool_calls").and_then(Value::as_array) {
        for (i, tc) in raw_calls.iter().enumerate() {
            let function = tc.get("function").cloned().unwrap_or_else(|| json!({}));
            let raw_args = function.get("arguments").cloned().unwrap_or(Value::Null);
            let (arguments, err) = parse_arguments(&raw_args);
            if let Some(err) = err {
                let id = tc
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| i.to_string());
                arg_errors.push(json!({
                    "call_id": id,
                    "error": err,
                    "raw": stringify_raw(&raw_args),
                }));
            }
            tool_calls.push(ToolCallRequest {
                call_id: tc
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("call_{i}")),
                name: function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                arguments,
            });
        }
    }

    let mut raw = Map::new();
    raw.insert(
        "id".into(),
        payload.get("id").cloned().unwrap_or(Value::Null),
    );
    raw.insert(
        "model".into(),
        payload.get("model").cloned().unwrap_or(Value::Null),
    );
    if !arg_errors.is_empty() {
        raw.insert("arg_parse_errors".into(), Value::Array(arg_errors));
    }

    Ok(ModelTurn {
        message: Message {
            role: Role::Assistant,
            content: msg
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
            name: None,
        },
        usage: usage_from_openai(payload.get("usage")),
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("stop")
            .to_string(),
        raw,
    })
}

/// 返回 (arguments, 解析失败的原因)。失败时 arguments 为空对象。
fn parse_arguments(raw: &Value) -> (Map<String, Value>, Option<String>) {
    match raw {
        // 缺省 / null 当成 "{}"
        Value::Null => (Map::new(), None),
        Value::String(s) => {
            let text = if s.is_empty() { "{}" } else { s.as_str() };
            match serde_json::from_str::<Value>(text) {
                Ok(Value::Object(m)) => (m, None),
                Ok(other) => (
                    Map::new(),
                    Some(format!(
                        "arguments 必须是对象，实际 {}",
                        json_type_name(&other)
                    )),
                ),
                Err(e) => (Map::new(), Some(e.to_string())),
            }
        }
        // 有些网关直接给对象而不是字符串，照收
        Value::Object(m) => (m.clone(), None),
        other => (
            Map::new(),
            Some(format!(
                "arguments 必须是对象，实际 {}",
                json_type_name(other)
            )),
        ),
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn stringify_raw(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "{}".to_string(),
        other => other.to_string(),
    }
}

/// 按 元/百万 token 折算这一轮的花费。只用于卡片上的「已用 ¥」（§3.1 ModelConfig）。
pub fn cost_of(usage: &Usage, cfg: &ModelConfig) -> f64 {
    (usage.input_tokens as f64 * cfg.price_in_per_mtok
        + usage.output_tokens as f64 * cfg.price_out_per_mtok)
        / 1_000_000.0
}

/// 从 `cfg.api_key_env` 点名的环境变量取密钥；缺了就报错，且消息里**只出现变量名**。
pub fn resolve_api_key(
    cfg: &ModelConfig,
    env: &HashMap<String, String>,
) -> Result<String, ModelError> {
    let key = env.get(&cfg.api_key_env).map(|k| k.trim()).unwrap_or("");
    if key.is_empty() {
        return Err(ModelError::Config(format!(
            "环境变量 {} 没设置或为空。密钥只从环境变量读，不要写进 config/aite.yaml（§3.1 ModelConfig）",
            cfg.api_key_env
        )));
    }
    Ok(key.to_string())
}

/// 进程环境变量的快照，给 `from_config` 用。
pub fn env_snapshot() -> HashMap<String, String> {
    std::env::vars().collect()
}

// --------------------------------------------------------------------------
// ModelPort 实现
// --------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
struct Accounting {
    total_cost: f64,
    last_usage: Usage,
}

/// `ModelPort`（live）。指向任意 OpenAI 兼容端点。
pub struct OpenAiCompatModel {
    cfg: ModelConfig,
    base_url: String,
    api_key: String,
    http: reqwest::Client,
    acct: Mutex<Accounting>,
}

impl OpenAiCompatModel {
    /// 缺 base_url / model / 密钥变量都在这里就报出来，绝不拖到第一次请求。
    pub fn from_config(
        cfg: &ModelConfig,
        env: &HashMap<String, String>,
    ) -> Result<Self, ModelError> {
        if cfg.base_url.trim().is_empty() {
            return Err(ModelError::Config(
                "ModelConfig.base_url 是空的：填百炼 / 智谱的 OpenAI 兼容端点".into(),
            ));
        }
        if cfg.model.trim().is_empty() {
            return Err(ModelError::Config(
                "ModelConfig.model 是空的：填要用的模型名".into(),
            ));
        }
        let api_key = resolve_api_key(cfg, env)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SEC))
            .build()
            .map_err(|e| ModelError::Config(format!("HTTP 客户端建不起来：{e}")))?;
        Ok(Self {
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            cfg: cfg.clone(),
            api_key,
            http,
            acct: Mutex::new(Accounting::default()),
        })
    }

    /// 测试注入：把端点指到本地假服务。只动 base_url，别的都不变。
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 累计花费（元）。供上层做卡片 footer 用；每次 chat 后累加。
    pub fn total_cost(&self) -> f64 {
        self.acct.lock().map(|a| a.total_cost).unwrap_or(0.0)
    }

    pub fn last_usage(&self) -> Usage {
        self.acct.lock().map(|a| a.last_usage).unwrap_or_default()
    }

    fn request_body(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        max_tokens: u32,
        temperature: f32,
    ) -> Result<Value, ModelError> {
        let messages =
            to_openai_messages(messages).map_err(|e| ModelError::Upstream(e.to_string()))?;
        let mut body = Map::new();
        body.insert("model".into(), Value::String(self.cfg.model.clone()));
        body.insert("messages".into(), Value::Array(messages));
        body.insert("max_tokens".into(), json!(max_tokens));
        body.insert("temperature".into(), json!(clean_f32(temperature)));
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(to_openai_tools(tools)));
            // 永远 auto：要不要调工具归模型判断，worker 只管接住它的出牌
            body.insert("tool_choice".into(), Value::String("auto".into()));
        }
        Ok(Value::Object(body))
    }
}

impl std::fmt::Debug for OpenAiCompatModel {
    /// 绝不打印密钥：这个类型会被塞进日志与 panic 消息里。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = if self.cfg.model.is_empty() {
            "(未配置)"
        } else {
            self.cfg.model.as_str()
        };
        let url = if self.base_url.is_empty() {
            "(未配置)"
        } else {
            self.base_url.as_str()
        };
        write!(f, "<OpenAiCompatModel {name} @ {url}>")
    }
}

#[async_trait]
impl ModelPort for OpenAiCompatModel {
    fn name(&self) -> String {
        self.cfg.model.clone()
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        max_tokens: u32,
        temperature: f32,
    ) -> Result<ModelTurn, ModelError> {
        let body = self.request_body(messages, tools, max_tokens, temperature)?;
        let url = format!("{}/chat/completions", self.base_url);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            // 拼 source() 链：`reqwest::Error` 的 Display 只写 kind + url，真正的原因
            // （连接被拒 / DNS 解析不了 / TLS 被中间设备换掉 / 代理不通）全在链上。
            // 不拼的话这三种病在 `aite preflight` 第 5 组里打出来一模一样，排障没有线索。
            // 仍然过 redact —— 链上的文本同样是上游给的，兜底闸不许绕。
            .map_err(|e| ModelError::Upstream(redact(&error_chain(&e), &self.api_key)))?;

        let status = response.status();
        let text = response
            .text()
            .await
            // 同上：读响应体炸掉时，根因（连接被对端切断 / 解压失败）也只在链上。
            .map_err(|e| ModelError::Upstream(redact(&error_chain(&e), &self.api_key)))?;
        // 响应体也要过 redact：国内网关在 4xx 的调试信息里回显请求头不是没有过的事，
        // 一旦回显 Authorization，这条错误会原样进 worker 的失败日志和群里的回帖路径。
        // 同函数上面两处已经确立了这个约定，这里不能漏（派单纪律 5：任何输出不得出现取值）。
        if !status.is_success() {
            return Err(ModelError::Upstream(format!(
                "HTTP {}：{}",
                status.as_u16(),
                redact(&clip_chars(&text, 500), &self.api_key)
            )));
        }
        let payload: Value = serde_json::from_str(&text).map_err(|e| {
            ModelError::BadResponse(format!(
                "{e}：{}",
                redact(&clip_chars(&text, 200), &self.api_key)
            ))
        })?;

        let mut turn = turn_from_response(&payload)?;
        let cost = cost_of(&turn.usage, &self.cfg);
        let total = match self.acct.lock() {
            Ok(mut acct) => {
                acct.last_usage = turn.usage;
                acct.total_cost += cost;
                acct.total_cost
            }
            // 锁中毒只可能来自别处 panic；记账退化成本次花费，不影响这一轮回答
            Err(_) => cost,
        };
        turn.raw.insert("cost_cny".into(), json!(round6(total)));
        Ok(turn)
    }
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// `ModelConfig.temperature` 是 f32，直接 `as f64` 会让请求体里出现
/// `0.30000001192092896`。走一次最短往返十进制，线上看到的就是 `0.3`。
fn clean_f32(v: f32) -> f64 {
    v.to_string().parse::<f64>().unwrap_or(v as f64)
}

fn clip_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}

/// 把 `source()` 链一路拼进消息。
///
/// `reqwest::Error` 的 `Display` 只写 kind + url；根因在链上。`ModelError` 的三个变体
/// 都是纯 `String`（contracts 冻结，加不了 `#[source]`），所以链只能在这里**拼进字符串**，
/// 拼完照旧过 [`redact`]。`core/crates/app/src/preflight.rs` 里有一份同样的实现 ——
/// 那边给飞书那三发请求用；两边各自 10 行，谁也不必为这个去扩对方的公开 API 面。
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        let msg = s.to_string();
        // 上游常把下一层的 Display 原样嵌进自己的消息里，拼两遍只会更难读。
        if !msg.is_empty() && !out.contains(&msg) {
            out.push_str(" ← ");
            out.push_str(&msg);
        }
        cur = s.source();
    }
    out
}

/// reqwest 的错误里可能带上完整 URL；密钥虽然在 header 里，仍然兜一层。
fn redact(text: &str, key: &str) -> String {
    if key.len() < 4 {
        return text.to_string();
    }
    text.replace(key, "***")
}

// --------------------------------------------------------------------------
// 单元测试
// --------------------------------------------------------------------------
//
// 这个 crate 的测试主体在 `tests/test_openai_compat.rs`（那边有一台 HTTP 假服务）。
// 这里只钉两件**那边钉不住**的事：
//
// 1. [`error_chain`] 是私有纯函数，集成测试够不着；
// 2. 「拼完链**仍然**过 [`redact`]」要的现场是一个**连不上**的端点 —— 起假服务
//    反而碍事，因为假服务会老老实实应答。
#[cfg(test)]
mod tests {
    use super::*;

    /// 假取值。真密钥一个字都不会出现在这个文件里。
    const FAKE_KEY: &str = "sk-models-unit-fake-key-0123456789";

    /// 一条能自己嵌套的假错误，用来造 `source()` 链。
    #[derive(Debug)]
    struct Layer(String, Option<Box<Layer>>);

    impl std::fmt::Display for Layer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1
                .as_deref()
                .map(|inner| inner as &(dyn std::error::Error + 'static))
        }
    }

    fn layer(msg: &str, inner: Option<Layer>) -> Layer {
        Layer(msg.to_string(), inner.map(Box::new))
    }

    /// 链一路走到底 —— 根因在最后一层，而 `Display` 只给得出第一层。
    #[test]
    fn error_chain_walks_all_the_way_down() {
        let e = layer(
            "error sending request",
            Some(layer(
                "client error (Connect)",
                Some(layer(
                    "tcp connect error: Connection refused (os error 61)",
                    None,
                )),
            )),
        );

        let out = error_chain(&e);

        assert!(out.starts_with("error sending request"), "{out}");
        assert!(out.contains("client error (Connect)"), "{out}");
        assert!(
            out.contains("Connection refused (os error 61)"),
            "根因没拼上，这条链等于白走：{out}"
        );
    }

    /// 上游常把下一层的 `Display` 原样嵌进自己的消息里，拼两遍只会更难读。
    #[test]
    fn error_chain_skips_a_layer_the_parent_already_quoted() {
        let e = layer(
            "dns error: failed to lookup address information",
            Some(layer("failed to lookup address information", None)),
        );

        let out = error_chain(&e);

        assert_eq!(
            out.matches("failed to lookup address information").count(),
            1,
            "{out}"
        );
        assert!(!out.contains(" ← "), "重复的一层不该拼进来：{out}");
    }

    /// 拼完链**仍然**过 [`redact`]：把那句 `redact(...)` 换回 `e.to_string()` 这条就红。
    ///
    /// 现场是真的 —— 有些网关把 key 放在 URL 里（Gemini 的 `?key=` 就是这样），
    /// 于是 reqwest 的传输错误 `Display` 自带完整 URL、自带 key。端点指到一个刚被
    /// 放掉的本机端口，`send()` 必然失败（127.0.0.1 在 NO_PROXY 里，不经代理）。
    #[tokio::test]
    async fn a_key_echoed_in_the_url_is_still_redacted() {
        let port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let p = l.local_addr().expect("addr").port();
            drop(l);
            p
        };
        let cfg = ModelConfig {
            base_url: format!("http://127.0.0.1:{port}/v1/{FAKE_KEY}"),
            api_key_env: "AITE_MODELS_UNIT_FAKE_KEY_ENV".into(),
            model: "fake-model".into(),
            ..Default::default()
        };
        let env = HashMap::from([(cfg.api_key_env.clone(), FAKE_KEY.to_string())]);
        let model = OpenAiCompatModel::from_config(&cfg, &env).expect("from_config");

        let err = ModelPort::chat(&model, &[Message::text(Role::User, "ping")], &[], 16, 0.0)
            .await
            .expect_err("端口已经放掉了，这一发必须失败");

        let msg = err.to_string();
        assert!(!msg.contains(FAKE_KEY), "密钥漏进错误消息了：{msg}");
        assert!(
            msg.contains("***"),
            "确实是被抹掉的，而不是这段文本压根没走到错误消息里：{msg}"
        );
    }
}
