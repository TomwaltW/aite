//! CC6 的 live 测试：真打一个模型端点，跑一轮带工具 + 回挂的第 2 轮。
//!
//! 这个仓库平时不许 `#[ignore]` —— 这里是 CC6 卡片明令的例外：key 只在总管本机
//! （总计划 D19），云端与 CI 都没有。双保险：默认 ignored，就算 `--ignored` 跑起来，
//! 没设 `AITE_LIVE_MODEL=1` 也立即返回、不发任何网络请求。
//!
//! 本机跑法（`--nocapture` 才看得到打印的 `model` 字段）：
//!
//! ```text
//! cd core && AITE_LIVE_MODEL=1 cargo test -p aite-models live -- --ignored --nocapture
//! ```
//!
//! 环境变量：`AITE_MODEL_API_KEY`（密钥，只从这里取）；`AITE_LIVE_BASE_URL`（默认
//! `https://api.deepseek.com`）；`AITE_LIVE_MODEL_NAME`（默认 `deepseek-flash`）。
use aite_contracts::{Message, ModelConfig, ModelPort, ModelProvider, Role, ToolSpec};
use aite_models::{OpenAiCompatModel, env_snapshot};
use serde_json::json;

#[tokio::test]
#[ignore = "live：真打模型端点，只在总管本机跑（AITE_LIVE_MODEL=1 + AITE_MODEL_API_KEY）"]
async fn live_model_tool_roundtrip() {
    if std::env::var("AITE_LIVE_MODEL").as_deref() != Ok("1") {
        eprintln!("AITE_LIVE_MODEL != 1，跳过（不发网络请求）");
        return;
    }
    let base_url = std::env::var("AITE_LIVE_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string());
    let model_name =
        std::env::var("AITE_LIVE_MODEL_NAME").unwrap_or_else(|_| "deepseek-flash".to_string());
    let cfg = ModelConfig {
        provider: ModelProvider::OpenaiCompat,
        base_url,
        api_key_env: "AITE_MODEL_API_KEY".into(),
        model: model_name,
        max_tokens: 1024,
        temperature: 0.0,
        price_in_per_mtok: 0.0,
        price_out_per_mtok: 0.0,
    };
    let model = OpenAiCompatModel::from_config(&cfg, &env_snapshot()).expect("建模型");
    eprintln!("vendor = {}", model.vendor().as_str());

    let tools = vec![ToolSpec {
        name: "get_weather".into(),
        description: "查询某个城市今天的天气".into(),
        parameters: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
        }),
    }];
    let mut history = vec![
        Message::text(Role::System, "你是 Aite。需要天气时必须调用 get_weather。"),
        Message::text(Role::User, "北京今天天气怎么样？"),
    ];

    let first = model
        .chat(&history, &tools, cfg.max_tokens, cfg.temperature)
        .await
        .expect("第 1 轮");
    eprintln!("第 1 轮 model = {}", first.raw["model"]);
    eprintln!(
        "第 1 轮 provider_extra 键 = {:?}",
        first
            .raw
            .get("provider_extra")
            .and_then(|v| v.as_object())
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
    );
    let call = first
        .message
        .tool_calls
        .as_ref()
        .and_then(|c| c.first())
        .cloned()
        .expect("第 1 轮该调 get_weather");
    history.push(first.message.clone());
    history.push(Message {
        role: Role::Tool,
        content: "北京：晴，25 度".into(),
        tool_calls: None,
        tool_call_id: Some(call.call_id),
        name: Some(call.name),
    });

    let second = model
        .chat(&history, &tools, cfg.max_tokens, cfg.temperature)
        .await
        .expect("第 2 轮（回挂思考字段后）");
    eprintln!("第 2 轮 model = {}", second.raw["model"]);
    eprintln!("第 2 轮回复 = {}", second.message.content);
    assert!(!second.message.content.contains("<think>"));
}
