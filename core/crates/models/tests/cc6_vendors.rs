//! CC6：国产模型加固 + 进程内思考字段回挂缓存。
//!
//! 夹具在 `tests/fixtures/`（来源与日期见那边的 README；不是云端真录的，云端没有 key）。
//! 点名的 8 条测试放在文件顶层、不包 `mod` —— 派单验收用 `--exact` 按全路径匹配。
mod common;

use std::collections::{BTreeSet, HashMap};

use aite_contracts::{
    Message, ModelConfig, ModelError, ModelPort, ModelProvider, Role, ToolSpec, Usage,
};
use aite_models::{OpenAiCompatModel, Vendor, usage_from_openai};
use common::FakeServer;
use serde_json::{Value, json};

const DEEPSEEK: &str = include_str!("fixtures/deepseek_thinking_tool_call.json");
const QWEN: &str = include_str!("fixtures/qwen_tool_call.json");
const GLM: &str = include_str!("fixtures/glm_tool_call.json");
const KIMI: &str = include_str!("fixtures/kimi_tool_call.json");
const DOUBAO: &str = include_str!("fixtures/doubao_encrypted.json");
const MINIMAX: &str = include_str!("fixtures/minimax_think.json");

/// 假值；真密钥一个字都不会出现在这个文件里。
const FAKE_KEY: &str = "sk-cc6-fake-key-0123456789";

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).expect("夹具是合法 JSON")
}

fn cfg(model: &str, base_url: &str) -> ModelConfig {
    ModelConfig {
        provider: ModelProvider::OpenaiCompat,
        base_url: base_url.into(),
        api_key_env: "AITE_MODEL_API_KEY".into(),
        model: model.into(),
        max_tokens: 4096,
        temperature: 0.3,
        price_in_per_mtok: 0.0,
        price_out_per_mtok: 0.0,
    }
}

/// cfg 里写厂商域名（厂商在 `from_config` 里就定了），流量再指到假服务。
fn model_at(model: &str, base_url: &str, server: &FakeServer) -> OpenAiCompatModel {
    let env = HashMap::from([("AITE_MODEL_API_KEY".to_string(), FAKE_KEY.to_string())]);
    OpenAiCompatModel::from_config(&cfg(model, base_url), &env)
        .expect("建模型")
        .with_base_url(&server.base_url)
}

fn search_tool() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "search".into(),
        description: "查资料".into(),
        parameters: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
    }]
}

fn text_reply(model: &str, content: &str) -> Value {
    json!({
        "id": "chatcmpl-2",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 2},
    })
}

fn user(text: &str) -> Vec<Message> {
    vec![
        Message::text(Role::System, "你是 Aite"),
        Message::text(Role::User, text),
    ]
}

/// 像 worker 那样：assistant 消息原样进历史，再接一条 tool 结果。
fn with_tool_result(mut history: Vec<Message>, assistant: Message) -> Vec<Message> {
    let call_id = assistant
        .tool_calls
        .as_ref()
        .and_then(|c| c.first())
        .map(|c| c.call_id.clone())
        .expect("这一轮该有 tool_call");
    history.push(assistant);
    history.push(Message {
        role: Role::Tool,
        content: "北京晴，25 度".into(),
        tool_calls: None,
        tool_call_id: Some(call_id),
        name: Some("search".into()),
    });
    history
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// 请求体里 messages 的第 i 行。
fn sent_row(server: &FakeServer, req: usize, row: usize) -> Value {
    server.request(req).body["messages"][row].clone()
}

// --- 1 + 2：厂商识别与请求体 ---------------------------------------------

#[tokio::test]
async fn kimi_request_omits_temperature() {
    // 两条识别路径：百炼域名 + `kimi-` 前缀；`api.moonshot.cn` + 无前缀的模型名
    let paths = [
        (
            "kimi-k2-turbo-preview",
            "https://ws-abc.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        ),
        ("k2-turbo-preview", "https://api.moonshot.cn/v1"),
    ];
    for (name, url) in paths {
        let server = FakeServer::start(vec![fixture(KIMI)]).await;
        let model = model_at(name, url, &server);
        assert_eq!(model.vendor(), Vendor::Kimi, "{name} @ {url}");

        model
            .chat(&user("北京天气"), &search_tool(), 1024, 0.3)
            .await
            .expect("chat");
        let sent = server.request(0);
        assert!(
            !sent.has("temperature"),
            "Kimi 传 temperature 会 400：{name} @ {url} 发了 {:?}",
            sent.field("temperature")
        );
        assert_eq!(sent.field("max_tokens"), Some(&json!(1024)));
    }

    // 对照：认不出厂商的照发 temperature（这条测试不是空转）
    let server = FakeServer::start(vec![fixture(KIMI)]).await;
    let generic = model_at("some-model", "https://example.com/v1", &server);
    assert_eq!(generic.vendor(), Vendor::Generic);
    generic.chat(&user("嗨"), &[], 64, 0.3).await.expect("chat");
    assert!(server.request(0).has("temperature"));
}

#[tokio::test]
async fn qwen_sets_parallel_tool_calls() {
    let server = FakeServer::start(vec![fixture(QWEN)]).await;
    let model = model_at(
        "qwen-plus",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        &server,
    );
    assert_eq!(model.vendor(), Vendor::Qwen);

    let turn = model
        .chat(&user("北京天气"), &search_tool(), 1024, 0.3)
        .await
        .expect("chat");
    assert_eq!(
        server.request(0).field("parallel_tool_calls"),
        Some(&json!(true)),
        "Qwen 的 parallel_tool_calls 默认关，有工具要显式打开"
    );
    // 夹具里 arguments 是对象而不是字符串：照收（lib.rs parse_arguments）
    let calls = turn.message.tool_calls.clone().unwrap_or_default();
    assert_eq!(
        Value::Object(calls[0].arguments.clone()),
        json!({"q": "北京天气"})
    );
    assert!(!turn.raw.contains_key("arg_parse_errors"));

    model.chat(&user("嗨"), &[], 64, 0.3).await.expect("chat");
    let sent = server.request(1);
    assert!(!sent.has("parallel_tool_calls"), "没有工具时不加");
    assert!(!sent.has("tools"));
}

#[tokio::test]
async fn glm_tool_choice_auto_only() {
    let server = FakeServer::start(vec![fixture(GLM)]).await;
    let model = model_at("glm-4.6", "https://open.bigmodel.cn/api/paas/v4", &server);
    assert_eq!(model.vendor(), Vendor::Glm);

    model
        .chat(&user("北京天气"), &search_tool(), 1024, 0.3)
        .await
        .expect("chat");
    assert_eq!(
        server.request(0).field("tool_choice"),
        Some(&json!("auto")),
        "GLM 的 tool_choice 只支持 auto"
    );

    model.chat(&user("嗨"), &[], 64, 0.3).await.expect("chat");
    assert!(!server.request(1).has("tool_choice"), "没有工具时不带");
}

// --- 3：响应侧 -----------------------------------------------------------

#[tokio::test]
async fn reasoning_captured_in_raw() {
    let ds = fixture(DEEPSEEK);
    let doubao = fixture(DOUBAO);
    let server = FakeServer::start(vec![ds.clone(), doubao.clone(), fixture(GLM)]).await;

    let deepseek = model_at("deepseek-flash", "https://api.deepseek.com", &server);
    let turn = deepseek
        .chat(&user("北京天气"), &search_tool(), 1024, 0.3)
        .await
        .expect("chat");
    assert_eq!(
        turn.raw.get("provider_extra"),
        Some(&json!({"reasoning_content": ds["choices"][0]["message"]["reasoning_content"]})),
        "DeepSeek 的 reasoning_content 原样进 raw"
    );
    // DeepSeek 的缓存命中字段顺带读出来
    assert_eq!(turn.usage.cached_tokens, 1024);

    let db = model_at(
        "doubao-seed-1-6-250615",
        "https://ark.cn-beijing.volces.com/api/v3",
        &server,
    );
    assert_eq!(db.vendor(), Vendor::Doubao);
    let turn = db
        .chat(&user("北京天气"), &search_tool(), 1024, 0.3)
        .await
        .expect("chat");
    assert_eq!(
        turn.raw.get("provider_extra"),
        Some(&json!({"encrypted_content": doubao["choices"][0]["message"]["encrypted_content"]})),
        "豆包的 encrypted_content 原样进 raw"
    );

    let glm = model_at("glm-4.6", "https://open.bigmodel.cn/api/paas/v4", &server);
    let turn = glm
        .chat(&user("北京天气"), &search_tool(), 1024, 0.3)
        .await
        .expect("chat");
    assert!(
        !turn.raw.contains_key("provider_extra"),
        "无思考字段的响应不写这个键：{:?}",
        turn.raw
    );
}

#[tokio::test]
async fn minimax_think_stripped() {
    let fx = fixture(MINIMAX);
    let original = fx["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let server = FakeServer::start(vec![fx.clone(), text_reply("MiniMax-M2", "北京晴。")]).await;
    let model = model_at("MiniMax-M2", "https://api.minimax.cn/v1", &server);
    assert_eq!(model.vendor(), Vendor::Minimax);

    let history = user("北京天气");
    let turn = model
        .chat(&history, &search_tool(), 1024, 0.3)
        .await
        .expect("chat");
    assert_eq!(turn.message.content, "我先查一下北京的天气。");
    assert!(!turn.message.content.contains("<think>"));
    assert_eq!(
        turn.raw["provider_extra"],
        json!({"reasoning_content": "\n用户问北京天气，先查一下。\n"}),
        "剥下来的正文放进 provider_extra"
    );

    // 回挂形状：按它自己返回的样子原样拼回 content 开头，不加 reasoning_content 字段
    let history = with_tool_result(history, turn.message);
    model
        .chat(&history, &search_tool(), 1024, 0.3)
        .await
        .expect("chat");
    let row = sent_row(&server, 1, 2);
    assert_eq!(row["content"], json!(original));
    assert!(row.get("reasoning_content").is_none(), "{row}");

    // 非 MiniMax 的同样文本不剥
    let server = FakeServer::start(vec![fx]).await;
    let generic = model_at("some-model", "https://example.com/v1", &server);
    let turn = generic
        .chat(&user("北京天气"), &search_tool(), 1024, 0.3)
        .await
        .expect("chat");
    assert_eq!(turn.message.content, original);
    assert!(!turn.raw.contains_key("provider_extra"));
}

// --- 4：进程内回挂缓存 ---------------------------------------------------

#[tokio::test]
async fn deepseek_thinking_tool_roundtrip_reattaches_reasoning() {
    let ds = fixture(DEEPSEEK);
    let reasoning = ds["choices"][0]["message"]["reasoning_content"].clone();
    let server = FakeServer::start(vec![ds, text_reply("deepseek-flash", "北京晴。")]).await;
    // 探测事实 2：配置写 deepseek-chat，服务端静默路由到 deepseek-flash
    let model = model_at("deepseek-chat", "https://api.deepseek.com", &server);
    assert_eq!(model.vendor(), Vendor::Deepseek);

    let history = user("北京天气");
    let turn = model
        .chat(&history, &search_tool(), 1024, 0.3)
        .await
        .expect("第 1 轮");
    assert_eq!(turn.raw["model"], json!("deepseek-flash"));
    assert_eq!(model.name(), "deepseek-chat");

    let history = with_tool_result(history, turn.message);
    // 事实 3：第 2 轮假服务照回 200
    let second = model
        .chat(&history, &search_tool(), 1024, 0.3)
        .await
        .expect("第 2 轮");
    assert_eq!(second.message.content, "北京晴。");

    let first_req = server.request(0);
    assert!(
        !first_req.body.to_string().contains("reasoning_content"),
        "第 1 个请求里不该有思考字段"
    );
    let row = sent_row(&server, 1, 2);
    assert_eq!(row["role"], json!("assistant"));
    assert_eq!(
        row["reasoning_content"], reasoning,
        "第 2 个请求里那条 assistant 行要带着原样的 reasoning_content：{row}"
    );
    // 请求侧的 arguments 是重新序列化的（没有空格），与夹具里带空格的原始串不同 ——
    // 指纹若误用原始串，上面那条就回挂不上
    assert_eq!(
        row["tool_calls"][0]["function"]["arguments"],
        json!("{\"q\":1}")
    );
    assert_eq!(
        keys(&server.request(1).body),
        keys(&first_req.body),
        "顶层不多出任何键"
    );

    // 无思考字段时，第 2 轮请求体的 assistant 行不多出任何键
    let mut plain = fixture(DEEPSEEK);
    if let Some(msg) = plain["choices"][0]["message"].as_object_mut() {
        msg.remove("reasoning_content");
    }
    let server = FakeServer::start(vec![plain, text_reply("deepseek-flash", "北京晴。")]).await;
    let model = model_at("deepseek-chat", "https://api.deepseek.com", &server);
    let history = user("北京天气");
    let turn = model
        .chat(&history, &search_tool(), 1024, 0.3)
        .await
        .expect("第 1 轮");
    assert!(!turn.raw.contains_key("provider_extra"));
    let history = with_tool_result(history, turn.message);
    model
        .chat(&history, &search_tool(), 1024, 0.3)
        .await
        .expect("第 2 轮");
    assert_eq!(
        keys(&sent_row(&server, 1, 2)),
        ["content", "role", "tool_calls"]
            .into_iter()
            .map(String::from)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(keys(&server.request(1).body), keys(&server.request(0).body));
}

// --- 5：缓存命中三种字段 -------------------------------------------------

#[test]
fn cache_hit_field_variants() {
    let base = |extra: Value| {
        let mut u = json!({"prompt_tokens": 1000, "completion_tokens": 10});
        if let (Some(u), Some(extra)) = (u.as_object_mut(), extra.as_object()) {
            u.extend(extra.clone());
        }
        usage_from_openai(Some(&u)).cached_tokens
    };
    // OpenAI / 百炼
    assert_eq!(
        base(json!({"prompt_tokens_details": {"cached_tokens": 800}})),
        800
    );
    // DeepSeek
    assert_eq!(base(json!({"prompt_cache_hit_tokens": 700})), 700);
    // Kimi：顶层
    assert_eq!(base(json!({"cached_tokens": 600})), 600);
    // 都没有 → 0
    assert_eq!(base(json!({})), 0);
    // 先出现的为准
    assert_eq!(
        base(json!({
            "prompt_tokens_details": {"cached_tokens": 1},
            "prompt_cache_hit_tokens": 2,
            "cached_tokens": 3,
        })),
        1
    );
    assert_eq!(
        base(json!({"prompt_cache_hit_tokens": 2, "cached_tokens": 3})),
        2
    );
    // details 在但没有 cached_tokens → 往下找
    assert_eq!(
        base(json!({"prompt_tokens_details": {}, "prompt_cache_hit_tokens": 5})),
        5
    );

    // 夹具形状（真响应长这样）
    let kimi = fixture(KIMI);
    assert_eq!(
        usage_from_openai(kimi.get("usage")),
        Usage {
            input_tokens: 800,
            output_tokens: 18,
            cached_tokens: 640,
        }
    );
    assert_eq!(
        usage_from_openai(fixture(DEEPSEEK).get("usage")).cached_tokens,
        1024
    );
}

// --- 7：错误文本定形 -----------------------------------------------------

fn upstream_text(err: ModelError) -> String {
    match err {
        ModelError::Upstream(s) => s,
        other => panic!("该是 Upstream：{other}"),
    }
}

#[tokio::test]
async fn http_status_and_retry_after_in_error_text() {
    let body = format!(r#"{{"error":{{"message":"slow down","echo":"Bearer {FAKE_KEY}"}}}}"#);
    let h = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    let server = FakeServer::start_with_headers(vec![
        (429, body.clone(), h(&[("Retry-After", "2")])),
        (429, body.clone(), h(&[])),
        (500, body.clone(), h(&[])),
        (
            429,
            body.clone(),
            h(&[("retry-after-ms", "1500"), ("Retry-After", "9")]),
        ),
        (
            429,
            body.clone(),
            h(&[("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT")]),
        ),
        (401, body.clone(), h(&[])),
    ])
    .await;
    let model = model_at("deepseek-chat", "https://api.deepseek.com", &server);
    let mut texts = Vec::new();
    for _ in 0..6 {
        let err = model
            .chat(&user("嗨"), &[], 16, 0.0)
            .await
            .expect_err("非 2xx 该是错误");
        let display = err.to_string();
        assert!(!display.contains(FAKE_KEY), "密钥泄漏：{display}");
        texts.push((display, upstream_text(err)));
    }

    let (display, t) = &texts[0];
    assert!(t.starts_with("HTTP 429: "), "{t}");
    assert!(t.ends_with(" retry-after-ms=2000"), "{t}");
    assert!(t.contains("***"), "detail 照旧过 redact：{t}");
    assert!(
        display.starts_with("模型服务错误：HTTP 429: "),
        "外层前缀不动：{display}"
    );

    let (_, t) = &texts[1];
    assert!(t.starts_with("HTTP 429: "), "{t}");
    assert!(!t.contains("retry-after-ms"), "无头不追加：{t}");

    let (_, t) = &texts[2];
    assert!(t.starts_with("HTTP 500: "), "{t}");
    assert!(!t.contains("retry-after-ms"), "{t}");

    let (_, t) = &texts[3];
    assert!(
        t.ends_with(" retry-after-ms=1500"),
        "retry-after-ms 头优先：{t}"
    );

    let (_, t) = &texts[4];
    assert!(!t.contains("retry-after-ms"), "HTTP-date 形式不追加：{t}");

    let (_, t) = &texts[5];
    assert!(t.starts_with("HTTP 401: "), "{t}");
    assert!(!t.contains('：'), "全角冒号改成半角：{t}");
}

/// retry-after-ms 追加在 clip 之后，永不被截。
#[tokio::test]
async fn retry_after_survives_a_long_detail() {
    let body = "x".repeat(2000);
    let server = FakeServer::start_with_headers(vec![(
        429,
        body,
        vec![("Retry-After".to_string(), "3".to_string())],
    )])
    .await;
    let model = model_at("deepseek-chat", "https://api.deepseek.com", &server);
    let t = upstream_text(
        model
            .chat(&user("嗨"), &[], 16, 0.0)
            .await
            .expect_err("429"),
    );
    assert_eq!(
        t,
        format!("HTTP 429: {} retry-after-ms=3000", "x".repeat(500))
    );
}
