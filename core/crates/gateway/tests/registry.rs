//! CC4 ② / ③：外部工具登记（这个测试文件本身就是「网关之外的 crate」）与调用前策略钩子。
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aite_contracts::ports::BoxFuture;
use aite_contracts::{
    ToolCallRequest, ToolContext, ToolErrorCode, ToolGateway, ToolSpec, gateway_tools,
};
use aite_gateway::{
    GatewayTool, PolicyDecision, ToolEnv, ToolFailure, ToolOutcome, ToolPolicy, ToolRegistry,
};
use async_trait::async_trait;
use common::{assert_failed, fixture, register, req, req_args};
use serde_json::{Map, Value, json};

/// 一个外部工具：回显 `text`，记下被调了几次；可选地只在某个群里露出来。
struct Echo {
    name: &'static str,
    only_in_chat: Option<&'static str>,
    calls: Arc<AtomicUsize>,
}

impl Echo {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            only_in_chat: None,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl GatewayTool for Echo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "回显".into(),
            parameters: json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
            }),
        }
    }

    fn enabled(&self, ctx: &ToolContext) -> bool {
        self.only_in_chat.is_none_or(|chat| ctx.chat_id == chat)
    }

    fn call(
        &self,
        _env: ToolEnv,
        _ctx: ToolContext,
        args: Map<String, Value>,
    ) -> BoxFuture<'static, Result<ToolOutcome, ToolFailure>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Box::pin(async move { Ok(ToolOutcome::new(format!("echo: {text}"))) })
    }
}

fn names(specs: &[ToolSpec]) -> Vec<String> {
    specs.iter().map(|s| s.name.clone()).collect()
}

#[tokio::test]
async fn registry_accepts_external_tool() {
    let f = fixture();
    let echo = Echo::new("echo");
    let calls = echo.calls.clone();
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(echo)).expect("登记外部工具");

    // 重名一律当场拒：与已登记的、与内建 / 本地工具（all_model_tools 的 10 个）
    let dup = registry
        .register(Arc::new(Echo::new("echo")))
        .expect_err("重名要拒");
    assert!(dup.0.contains("\"echo\""), "{dup}");
    for taken in ["list_files", "checklist_add", "final"] {
        let err = registry
            .register(Arc::new(Echo::new(taken)))
            .expect_err("与内建重名要拒");
        assert!(err.0.contains(taken) && err.0.contains("内建"), "{err}");
    }
    assert_eq!(registry.names(), vec!["echo".to_string()]);

    let gateway = f.raw().with_registry(registry);
    register(&gateway);

    let catalog = gateway.catalog(&f.ctx);
    assert_eq!(
        &catalog[..5],
        gateway_tools(),
        "前 5 个逐字等于 gateway_tools()"
    );
    assert_eq!(catalog.len(), 6);
    assert_eq!(catalog[5].name, "echo", "登记的排在末尾");

    let result = gateway
        .call(&f.ctx, &req_args("echo", json!({"text": "你好"})))
        .await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.content, "echo: 你好", "内容来自外部实现");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 参数照样按 spec 校验
    let bad = gateway.call(&f.ctx, &req("echo")).await;
    assert_failed(&bad, ToolErrorCode::InvalidArgs);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disabled_tool_hidden_and_call_returns_not_found() {
    let f = fixture();
    let mut echo = Echo::new("echo");
    echo.only_in_chat = Some("oc_allowed");
    let calls = echo.calls.clone();
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(echo)).expect("登记");
    let gateway = f.raw().with_registry(registry);
    register(&gateway);

    // fixture 的 ctx 不在 oc_allowed：目录里没有、调用回 not_found、「可用的是」里也没有它
    assert_eq!(gateway.catalog(&f.ctx), gateway_tools());
    let hidden = gateway
        .call(&f.ctx, &req_args("echo", json!({"text": "x"})))
        .await;
    assert_failed(&hidden, ToolErrorCode::NotFound);
    assert!(
        !hidden
            .error
            .as_ref()
            .expect("error")
            .message
            .contains("\"echo\",")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "实现一次都没被执行");

    // 双向：换到谓词为真的群，它就在、能调通（同一个 task，token 照样对得上）
    let allowed = ToolContext {
        chat_id: "oc_allowed".into(),
        ..f.ctx.clone()
    };
    assert_eq!(
        names(&gateway.catalog(&allowed)).last().map(String::as_str),
        Some("echo")
    );
    let shown = gateway
        .call(&allowed, &req_args("echo", json!({"text": "x"})))
        .await;
    assert!(shown.ok, "{:?}", shown.error);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// 拒绝某一个工具名的策略。
struct DenyTool(&'static str, &'static str);

#[async_trait]
impl ToolPolicy for DenyTool {
    async fn before_call(&self, _ctx: &ToolContext, req: &ToolCallRequest) -> PolicyDecision {
        if req.name == self.0 {
            PolicyDecision::Deny(self.1.into())
        } else {
            PolicyDecision::Allow
        }
    }
}

#[tokio::test]
async fn policy_hook_deny_returns_denied() {
    let f = fixture();
    let gateway = f.raw().with_policy(Arc::new(DenyTool(
        "run_python",
        "run_python 需要审批，这次先不跑",
    )));
    register(&gateway);

    let denied = gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    assert_failed(&denied, ToolErrorCode::Denied);
    assert_eq!(
        denied.error.as_ref().expect("error").message,
        "run_python 需要审批，这次先不跑",
        "reason 原样进 message"
    );
    assert!(
        f.sandbox.exec_requests().is_empty(),
        "工具实现没被执行（沙箱 0 次 exec）"
    );
    assert_eq!(f.sandbox.box_count(), 0, "连沙箱都没建");

    // 双向：同一个 gateway 调另一个没被拒的工具照常 ok
    let ok = gateway.call(&f.ctx, &req("list_files")).await;
    assert!(ok.ok, "{:?}", ok.error);
}
