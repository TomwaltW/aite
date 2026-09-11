//! `OpenAiCompatModel` 自测：契约 ↔ OpenAI 双向翻译、cost、密钥只从环境变量取。
//! 移植自 `tests/e2e/test_t4_model_openai_compat.py`（22 条）。
//!
//! 翻译逻辑直接对函数单测；`chat()` 全程打到一个本地起的最小 HTTP 假服务上，
//! 断言的是**真发出去的请求体与 header** —— Python 版因为 SDK 自带 vendored httpx
//! 拦不到流量，只能查建出来的 client 上的 base_url / api_key，这里比那边严。
mod common;

use std::collections::HashMap;
use std::sync::Arc;

use aite_contracts::{
    Message, ModelConfig, ModelError, ModelPort, ModelProvider, Role, ToolCallRequest, Usage,
    all_model_tools,
};
use aite_models::{
    OpenAiCompatModel, cost_of, resolve_api_key, to_openai_messages, to_openai_tools,
    turn_from_response, usage_from_openai,
};
use common::FakeServer;
use serde_json::{Value, json};

fn cfg() -> ModelConfig {
    ModelConfig {
        provider: ModelProvider::OpenaiCompat,
        base_url: "https://dashscope.example.com/compatible-mode/v1".into(),
        api_key_env: "AITE_MODEL_API_KEY".into(),
        model: "qwen-plus".into(),
        max_tokens: 4096,
        temperature: 0.0,
        price_in_per_mtok: 0.8,
        price_out_per_mtok: 2.0,
    }
}

fn env_with(key: &str, value: &str) -> HashMap<String, String> {
    HashMap::from([(key.to_string(), value.to_string())])
}

fn response(content: Value, tool_calls: Value, finish_reason: &str, usage: Value) -> Value {
    json!({
        "id": "chatcmpl-1",
        "model": "qwen-plus",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content, "tool_calls": tool_calls},
            "finish_reason": finish_reason,
        }],
        "usage": usage,
    })
}

fn text_response(content: &str) -> Value {
    response(json!(content), Value::Null, "stop", Value::Null)
}

fn final_call_response(reply_json: &str, usage: Value) -> Value {
    response(
        Value::Null,
        json!([{
            "id": "c1",
            "type": "function",
            "function": {"name": "final", "arguments": reply_json},
        }]),
        "tool_calls",
        usage,
    )
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

// --- 契约 -> OpenAI -------------------------------------------------------

#[tokio::test]
async fn signature_matches_model_port() {
    // Rust 里签名一致是编译期保证的；这里验的是它真能当 `Arc<dyn ModelPort>` 用
    // （对象安全 + async_trait），RΩ 组装时接的就是这个形态。
    let server = FakeServer::start(vec![text_response("好")]).await;
    let model = OpenAiCompatModel::from_config(&cfg(), &env_with("AITE_MODEL_API_KEY", "sk-x"))
        .expect("建模型")
        .with_base_url(&server.base_url);
    let port: Arc<dyn ModelPort> = Arc::new(model);

    assert_eq!(port.name(), "qwen-plus");
    let turn = port
        .chat(&[Message::text(Role::User, "嗨")], &[], 100, 0.0)
        .await
        .expect("chat");
    assert_eq!(turn.message.content, "好");
}

#[test]
fn assistant_tool_calls_are_serialised_as_json_arguments() {
    let msgs = vec![
        Message::text(Role::System, "你是 Aite"),
        Message::text(Role::User, "画个图"),
        Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: Some(vec![ToolCallRequest {
                call_id: "c1".into(),
                name: "run_python".into(),
                arguments: json!({"code": "x=1"})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            }]),
            tool_call_id: None,
            name: None,
        },
        Message {
            role: Role::Tool,
            content: "exit_code=0".into(),
            tool_calls: None,
            tool_call_id: Some("c1".into()),
            name: Some("run_python".into()),
        },
    ];
    let out = to_openai_messages(&msgs).expect("翻译");
    let roles: Vec<&str> = out
        .iter()
        .map(|m| m["role"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool"]);

    let tc = &out[2]["tool_calls"][0];
    assert_eq!(tc["id"], json!("c1"));
    assert_eq!(tc["type"], json!("function"));
    assert_eq!(tc["function"]["name"], json!("run_python"));
    // arguments 是**字符串化的 JSON**，不是嵌套对象
    let raw = tc["function"]["arguments"]
        .as_str()
        .expect("arguments 必须是字符串");
    assert_eq!(
        serde_json::from_str::<Value>(raw).expect("可解析"),
        json!({"code": "x=1"})
    );
    assert_eq!(
        out[2]["content"],
        json!(""),
        "带 tool_calls 时用空串，不用 null"
    );
    assert_eq!(
        out[3],
        json!({"role": "tool", "content": "exit_code=0", "tool_call_id": "c1", "name": "run_python"})
    );
}

#[test]
fn chinese_arguments_are_not_escaped() {
    let msgs = vec![Message {
        role: Role::Assistant,
        content: String::new(),
        tool_calls: Some(vec![ToolCallRequest {
            call_id: "c1".into(),
            name: "final".into(),
            arguments: json!({"reply": "好了"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        }]),
        tool_call_id: None,
        name: None,
    }];
    let out = to_openai_messages(&msgs).expect("翻译");
    let raw = out[0]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap_or_default();
    assert!(raw.contains("好了"), "中文被转义了：{raw}");
}

#[test]
fn tool_message_without_call_id_is_rejected() {
    let err = to_openai_messages(&[Message::text(Role::Tool, "x")]).expect_err("该拒掉");
    assert!(err.to_string().contains("tool_call_id"));
}

#[test]
fn all_model_tools_translate_to_functions() {
    let out = to_openai_tools(all_model_tools());
    assert_eq!(out.len(), all_model_tools().len());
    assert_eq!(out.len(), 10);

    let got: std::collections::BTreeSet<String> = out
        .iter()
        .map(|t| {
            t["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let want: std::collections::BTreeSet<String> =
        all_model_tools().iter().map(|t| t.name.clone()).collect();
    assert_eq!(got, want);
    assert!(out.iter().all(|t| t["type"] == json!("function")));
    assert!(
        out.iter()
            .all(|t| t["function"]["parameters"]["type"] == json!("object"))
    );
}

// --- OpenAI -> 契约 -------------------------------------------------------

#[test]
fn tool_call_response_becomes_model_turn() {
    let turn = turn_from_response(&final_call_response(r#"{"reply": "好了"}"#, Value::Null))
        .expect("解析");
    assert_eq!(turn.finish_reason, "tool_calls");
    assert_eq!(
        turn.message.tool_calls,
        Some(vec![ToolCallRequest {
            call_id: "c1".into(),
            name: "final".into(),
            arguments: json!({"reply": "好了"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        }])
    );
}

#[test]
fn text_only_response_keeps_the_fallback_shape() {
    // §3.3 的兜底：本层如实返回 content 有值、tool_calls 为空，兜底归 worker
    let turn = turn_from_response(&text_response("北京今天晴。")).expect("解析");
    assert_eq!(turn.message.content, "北京今天晴。");
    assert_eq!(turn.message.tool_calls, None);
}

#[test]
fn broken_tool_arguments_degrade_instead_of_raising() {
    // 参数不是合法 JSON 时不炸整轮，走 §3.3 的 invalid_args 那条路
    let turn = turn_from_response(&final_call_response("{不是 JSON", Value::Null)).expect("解析");
    let calls = turn.message.tool_calls.clone().unwrap_or_default();
    assert!(calls[0].arguments.is_empty());
    assert_eq!(turn.raw["arg_parse_errors"][0]["call_id"], json!("c1"));
    assert_eq!(turn.raw["arg_parse_errors"][0]["raw"], json!("{不是 JSON"));
}

#[test]
fn empty_choices_is_an_error() {
    let err = turn_from_response(&json!({"choices": []})).expect_err("该报错");
    assert!(matches!(err, ModelError::BadResponse(_)));
    assert!(err.to_string().contains("没有 choices"));
}

#[test]
fn usage_maps_including_cached_tokens() {
    let u = usage_from_openai(Some(&json!({
        "prompt_tokens": 1200,
        "completion_tokens": 300,
        "prompt_tokens_details": {"cached_tokens": 800},
    })));
    assert_eq!(
        u,
        Usage {
            input_tokens: 1200,
            output_tokens: 300,
            cached_tokens: 800,
        }
    );
    assert_eq!(usage_from_openai(None), Usage::default());
    assert_eq!(usage_from_openai(Some(&Value::Null)), Usage::default());
}

#[test]
fn cost_uses_price_per_mtok() {
    let cost = cost_of(
        &Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cached_tokens: 0,
        },
        &cfg(),
    );
    assert!(approx(cost, 0.8 + 1.0), "实际 {cost}");
}

#[test]
fn responses_are_translated_straight_off_the_wire() {
    // Python 那条验的是「SDK 返回的 pydantic 对象也能翻」（`model_dump()` 那条路）。
    // Rust 这一层拿到的本来就是网络上的字节，所以等价的判据是：翻译不依赖我们自己的
    // 结构体形状，把响应原样序列化再解回来照样翻得出来。
    let wire = serde_json::to_string(&text_response("来自网络")).expect("序列化");
    let parsed: Value = serde_json::from_str(&wire).expect("反序列化");
    assert_eq!(
        turn_from_response(&parsed).expect("解析").message.content,
        "来自网络"
    );
}

// --- 密钥与配置 -----------------------------------------------------------

#[test]
fn api_key_comes_from_the_configured_env_var() {
    let env = env_with("AITE_MODEL_API_KEY", "sk-secret");
    assert_eq!(resolve_api_key(&cfg(), &env).expect("取密钥"), "sk-secret");
}

#[test]
fn missing_api_key_names_the_var_not_the_value() {
    let err = resolve_api_key(&cfg(), &HashMap::new()).expect_err("该报错");
    assert!(err.to_string().contains("AITE_MODEL_API_KEY"));
    // 空白串也算没设
    let blank = env_with("AITE_MODEL_API_KEY", "   ");
    assert!(resolve_api_key(&cfg(), &blank).is_err());
}

#[test]
fn custom_api_key_env_is_honoured() {
    let c = ModelConfig {
        api_key_env: "ZHIPU_KEY".into(),
        ..cfg()
    };
    let env = env_with("ZHIPU_KEY", "sk-zhipu");
    assert_eq!(resolve_api_key(&c, &env).expect("取密钥"), "sk-zhipu");
}

#[test]
fn blank_base_url_is_reported_before_any_network() {
    let c = ModelConfig {
        base_url: String::new(),
        ..cfg()
    };
    let err = OpenAiCompatModel::from_config(&c, &env_with("AITE_MODEL_API_KEY", "sk-x"))
        .expect_err("该报错");
    assert!(err.to_string().contains("base_url"), "实际是 {err}");

    // model 空着也一样，都在起飞前就说清楚
    let c2 = ModelConfig {
        model: String::new(),
        ..cfg()
    };
    let err2 = OpenAiCompatModel::from_config(&c2, &env_with("AITE_MODEL_API_KEY", "sk-x"))
        .expect_err("该报错");
    assert!(err2.to_string().contains("model"), "实际是 {err2}");
}

// --- chat() 全程 ----------------------------------------------------------

#[tokio::test]
async fn chat_sends_tools_and_accumulates_cost() {
    let payload = final_call_response(
        r#"{"reply": "好"}"#,
        json!({"prompt_tokens": 1_000_000, "completion_tokens": 0}),
    );
    let server = FakeServer::start(vec![payload.clone(), payload]).await;
    let model = OpenAiCompatModel::from_config(&cfg(), &env_with("AITE_MODEL_API_KEY", "sk-x"))
        .expect("建模型")
        .with_base_url(&server.base_url);

    let turn = model
        .chat(
            &[Message::text(Role::User, "嗨")],
            all_model_tools(),
            2048,
            0.3,
        )
        .await
        .expect("chat");

    let sent = server.request(0);
    assert_eq!(sent.field("model"), Some(&json!("qwen-plus")));
    assert_eq!(sent.field("max_tokens"), Some(&json!(2048)));
    assert!(approx(
        sent.field("temperature")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        0.3
    ));
    assert_eq!(sent.field("tool_choice"), Some(&json!("auto")));
    assert_eq!(
        sent.field("tools").and_then(Value::as_array).map(Vec::len),
        Some(10)
    );

    let calls = turn.message.tool_calls.clone().unwrap_or_default();
    assert_eq!(calls[0].name, "final");
    assert!(approx(model.total_cost(), 0.8));
    assert!(approx(
        turn.raw["cost_cny"].as_f64().unwrap_or_default(),
        0.8
    ));
    assert_eq!(
        model.last_usage(),
        Usage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_tokens: 0,
        }
    );

    model
        .chat(
            &[Message::text(Role::User, "再来")],
            all_model_tools(),
            2048,
            0.3,
        )
        .await
        .expect("chat");
    assert!(approx(model.total_cost(), 1.6), "累加，不是覆盖");
}

#[tokio::test]
async fn chat_without_tools_omits_the_tools_field() {
    let server = FakeServer::start(vec![text_response("纯聊天")]).await;
    let model = OpenAiCompatModel::from_config(&cfg(), &env_with("AITE_MODEL_API_KEY", "sk-x"))
        .expect("建模型")
        .with_base_url(&server.base_url);

    model
        .chat(&[Message::text(Role::User, "嗨")], &[], 100, 0.0)
        .await
        .expect("chat");

    let sent = server.request(0);
    assert!(!sent.has("tools"));
    assert!(!sent.has("tool_choice"));
}

#[test]
fn name_is_the_configured_model() {
    let model = OpenAiCompatModel::from_config(&cfg(), &env_with("AITE_MODEL_API_KEY", "sk-x"))
        .expect("建模型");
    assert_eq!(model.name(), "qwen-plus");
}

#[tokio::test]
async fn upstream_errors_never_leak_the_api_key() {
    // 非 2xx 这条路此前一条测试都没有（22 条全在 200 上，`start_raw` 写了没人调），
    // 而它恰恰是最容易漏密钥的地方：国内网关在 4xx 调试信息里回显请求头不是没有过的事。
    // 这条错误会一路进 worker 的失败日志和群里的「模型服务暂不可用」。
    let leaky = r#"{"error":{"message":"bad auth","echo":{"Authorization":"Bearer sk-from-env"}}}"#;
    let server = FakeServer::start_raw(vec![(401, leaky.to_string())]).await;
    let model =
        OpenAiCompatModel::from_config(&cfg(), &env_with("AITE_MODEL_API_KEY", "sk-from-env"))
            .expect("建模型")
            .with_base_url(&server.base_url);

    let err = model
        .chat(&[Message::text(Role::User, "嗨")], &[], 100, 0.0)
        .await
        .expect_err("401 该是错误");
    let text = err.to_string();
    assert!(text.contains("401"), "要说清是哪个状态码：{text}");
    assert!(
        !text.contains("sk-from-env"),
        "密钥泄漏进了错误消息：{text}"
    );
}

/// 响应体不是 JSON 时同样不许漏密钥（BadResponse 那条路）。
#[tokio::test]
async fn unparseable_responses_never_leak_the_api_key() {
    let server = FakeServer::start_raw(vec![(200, "sk-from-env 这不是 JSON".to_string())]).await;
    let model =
        OpenAiCompatModel::from_config(&cfg(), &env_with("AITE_MODEL_API_KEY", "sk-from-env"))
            .expect("建模型")
            .with_base_url(&server.base_url);

    let err = model
        .chat(&[Message::text(Role::User, "嗨")], &[], 100, 0.0)
        .await
        .expect_err("非 JSON 该是错误");
    assert!(
        !err.to_string().contains("sk-from-env"),
        "密钥泄漏进了解析错误：{err}"
    );
}

#[tokio::test]
async fn requests_go_to_the_base_url_with_the_env_key() {
    // 接线是真的：不注入任何 client，请求就该打到 {base_url}/chat/completions 上，
    // 密钥取自环境变量并放在 Authorization header 里
    let server = FakeServer::start(vec![text_response("好")]).await;
    let model =
        OpenAiCompatModel::from_config(&cfg(), &env_with("AITE_MODEL_API_KEY", "sk-from-env"))
            .expect("建模型")
            .with_base_url(&server.base_url);

    model
        .chat(&[Message::text(Role::User, "嗨")], &[], 100, 0.0)
        .await
        .expect("chat");

    let sent = server.request(0);
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/v1/chat/completions");
    assert_eq!(
        sent.headers.get("authorization").map(String::as_str),
        Some("Bearer sk-from-env")
    );
    assert_eq!(
        server.request_count(),
        1,
        "一次 chat 只发一次请求，本层不重试"
    );
}

#[test]
fn refuses_to_build_without_the_env_key() {
    let err = OpenAiCompatModel::from_config(&cfg(), &HashMap::new()).expect_err("该报错");
    assert!(err.to_string().contains("AITE_MODEL_API_KEY"));
}

#[test]
fn debug_never_leaks_the_key() {
    let model =
        OpenAiCompatModel::from_config(&cfg(), &env_with("AITE_MODEL_API_KEY", "sk-super-secret"))
            .expect("建模型");
    let text = format!("{model:?}");
    assert!(
        !text.contains("sk-super-secret"),
        "Debug 泄露了密钥：{text}"
    );
    assert!(text.contains("qwen-plus"));
}
