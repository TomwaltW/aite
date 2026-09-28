//! CC7 ⑤：探针按「这次调用实际提供的工具」判协议外。
//!
//! 网关注册表（CC4）一接上外部工具、目录按 bundle 裁剪（DD6）之后，模型拿到的工具
//! 不再恒等于冻结的 `all_model_tools()`。判据要跟着每次 `chat` 的 `tools` 走：
//! 给了的就认、参数按给的那份 schema 校验；没给却调了的才是协议外。
use aite_contracts::{Message, ModelPort, Role, ToolSpec, all_model_tools};
use aite_evals::protocol_probe::analyze;
use aite_evals::{Deps, DepsOptions, Scenario, build_deps};
use aite_testing::ScriptStep;
use serde_json::{Value, json};

fn deps_of(script: Vec<Value>) -> Deps {
    let sc = Scenario {
        model_script: script
            .into_iter()
            .map(|v| ScriptStep::from_value(v).expect("脚本"))
            .collect(),
        ..Scenario::named("cc7_probe")
    };
    build_deps(&sc, &DepsOptions::default()).expect("造 deps")
}

fn call(name: &str, args: Value) -> Value {
    json!({"tool_calls": [{"name": name, "arguments": args}]})
}

fn registry_tool() -> ToolSpec {
    ToolSpec {
        name: "search_docs".into(),
        description: "搜文档（注册表里的外部工具）".into(),
        parameters: json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
        }),
    }
}

#[tokio::test]
async fn probe_treats_an_offered_registry_tool_as_known() {
    let d = deps_of(vec![
        call("search_docs", json!({"query": "报销流程"})),
        call("search_docs", json!({"query": 42})),
    ]);
    let mut offered: Vec<ToolSpec> = all_model_tools().to_vec();
    offered.push(registry_tool());
    let messages = vec![Message::text(Role::User, "查一下")];
    for _ in 0..2 {
        d.model
            .chat(&messages, &offered, 16, 0.0)
            .await
            .expect("chat");
    }

    let obs = d.model.observations();
    let ok = &obs[0].tool_calls[0];
    assert!(ok.in_protocol, "提供了的注册表工具被判成协议外");
    assert_eq!(ok.schema_ok, Some(true));
    // 参数按它自己的 schema 校验
    let bad = &obs[1].tool_calls[0];
    assert!(bad.in_protocol);
    assert_eq!(bad.schema_ok, Some(false), "{:?}", bad.schema_error);

    let report = analyze(&d);
    assert_eq!(
        report["unknown_tools"],
        json!({}),
        "{}",
        report["unknown_tools"]
    );
    let violations = report["schema_violations"].as_array().expect("数组");
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0]["name"], json!("search_docs"));
}

#[tokio::test]
async fn probe_flags_a_tool_not_offered_in_that_call() {
    let d = deps_of(vec![
        call("run_python", json!({"code": "print(1)"})),
        call("run_python", json!({"code": "print(2)"})),
    ]);
    let messages = vec![Message::text(Role::User, "算一下")];
    // 第一次：没提供 run_python 却调了 → 协议外
    let trimmed: Vec<ToolSpec> = all_model_tools()
        .iter()
        .filter(|t| t.name != "run_python")
        .cloned()
        .collect();
    d.model
        .chat(&messages, &trimmed, 16, 0.0)
        .await
        .expect("第一次");
    // 第二次：提供了 → 认
    d.model
        .chat(&messages, all_model_tools(), 16, 0.0)
        .await
        .expect("第二次");

    let obs = d.model.observations();
    assert!(
        !obs[0].tool_calls[0].in_protocol,
        "本次没提供的工具该判协议外"
    );
    assert_eq!(obs[0].tool_calls[0].schema_ok, None);
    assert!(obs[1].tool_calls[0].in_protocol);
    assert_eq!(obs[1].tool_calls[0].schema_ok, Some(true));

    let report = analyze(&d);
    assert_eq!(report["unknown_tools"], json!({"run_python": 1}));
}
