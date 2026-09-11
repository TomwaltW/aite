//! §2.2 B5：Gateway 的错误面（对应旧 `tests/gateway/test_gateway_errors.py`，25 条）。
//!
//! B5 原文四条：未知工具 → not_found；参数不合 schema → invalid_args；
//! 工具超时 → timeout；session_token 不匹配 → denied。
//!
//! 外加对应表剩下的两条（upstream / sandbox）、§3.2 那句「永远不抛异常给调用方」，
//! 以及 §3.2 注释里的**执行顺序**：
//! 「校验 session_token → 查工具存在 → 校验 arguments → 执行（带超时）→ 截断 content」。
//! 顺序错了错误码就会串味（比如带错 token 调不存在的工具应该是 denied 而不是 not_found），
//! 所以顺序本身也单独钉三条。
//!
//! 超时那几条用 `tokio::time::pause()`（`start_paused`）：不靠真实 sleep，毫秒级跑完。
mod common;

use std::sync::Arc;
use std::time::Duration;

use aite_contracts::{
    ExecResult, MAX_TOOL_CONTENT_CHARS, PlatformError, SandboxError, SandboxErrorKind,
    ToolCallRequest, ToolContext, ToolErrorCode, ToolGateway,
};
use aite_gateway::{ToolEnv, ToolFailure, ToolImpl, ToolOutcome};
use common::{assert_failed, error_code, fixture, register, req, req_args};
use serde_json::{Map, Value, json};

fn tool_impl<F>(f: F) -> ToolImpl
where
    F: Fn() -> Result<ToolOutcome, ToolFailure> + Send + Sync + 'static,
{
    Arc::new(
        move |_env: ToolEnv, _ctx: ToolContext, _args: Map<String, Value>| {
            let outcome = f();
            Box::pin(async move { outcome })
        },
    )
}

// --- B5 四条 ---------------------------------------------------------------

#[tokio::test]
async fn unknown_tool_is_not_found() {
    let f = fixture();
    let result = f.gateway().call(&f.ctx, &req("read_the_room")).await;
    assert_failed(&result, ToolErrorCode::NotFound);
    assert_eq!(result.name, "read_the_room");
}

#[tokio::test]
async fn bad_arguments_are_invalid_args() {
    let f = fixture();
    let result = f
        .gateway()
        .call(
            &f.ctx,
            &req_args("read_group_history", json!({"limit": 9999})),
        )
        .await;
    assert_failed(&result, ToolErrorCode::InvalidArgs);
}

#[tokio::test]
async fn missing_required_argument_is_invalid_args() {
    let f = fixture();
    let result = f.gateway().call(&f.ctx, &req("read_document")).await;
    assert_failed(&result, ToolErrorCode::InvalidArgs);
}

#[tokio::test]
async fn wrong_session_token_is_denied() {
    let f = fixture();
    let forged = f.ctx_with_token(&"f".repeat(32));
    let result = f.gateway().call(&forged, &req("list_files")).await;
    assert_failed(&result, ToolErrorCode::Denied);
}

#[tokio::test]
async fn unregistered_task_is_denied() {
    // 失败关闭：没登记过 token 的 task 一律 denied，不是「没登记就放过」。
    let f = fixture();
    let result = f.raw().call(&f.ctx, &req("list_files")).await;
    assert_failed(&result, ToolErrorCode::Denied);
}

#[tokio::test]
async fn empty_session_token_is_denied() {
    let f = fixture();
    let bare = f.ctx_with_token("");
    let result = f.gateway().call(&bare, &req("list_files")).await;
    assert_failed(&result, ToolErrorCode::Denied);
}

#[tokio::test]
async fn registering_an_empty_token_still_denies() {
    // register_task 拒绝空 token（旧实现抛 ValueError；trait 没有返回值，就是不登记），
    // 于是这个 task 仍然一个调用都过不去。
    let f = fixture();
    let gateway = f.raw();
    gateway.register_task(&f.ctx.task_id, "");
    let result = gateway.call(&f.ctx, &req("list_files")).await;
    assert_failed(&result, ToolErrorCode::Denied);
}

#[tokio::test]
async fn token_of_another_task_is_denied() {
    // token 对得上但 task_id 不是它的 —— 一样不放过。
    let f = fixture();
    let other = f.ctx_with_task("task-someone-else");
    let result = f.gateway().call(&other, &req("list_files")).await;
    assert_failed(&result, ToolErrorCode::Denied);
}

#[tokio::test(start_paused = true)]
async fn tool_timeout_is_timeout() {
    // 默认预算（DEFAULT_TOOL_TIMEOUT_SEC）那条路：平台迟迟不返回。
    let f = fixture();
    let gateway = f.raw().with_default_timeout(0.2);
    register(&gateway);
    f.platform.set_delay(Duration::from_secs(30));

    let result = gateway.call(&f.ctx, &req("read_group_history")).await;

    assert_failed(&result, ToolErrorCode::Timeout);
    assert!(result.duration_ms < 5_000, "{}", result.duration_ms);
}

#[tokio::test]
async fn run_python_timeout_from_inside_the_sandbox() {
    // 沙箱内 coreutils timeout 把退出码打成 124 → 工具翻成 timeout。
    let f = fixture();
    f.sandbox.set_exec_result(ExecResult {
        exit_code: 124,
        stdout: String::new(),
        stderr: "[aite] 执行超过 5s 上限，已在沙箱内终止".into(),
        duration_ms: 5010,
        truncated: false,
        files_out: Vec::new(),
    });

    let result = f
        .gateway()
        .call(
            &f.ctx,
            &req_args(
                "run_python",
                json!({"code": "import time; time.sleep(99)", "timeout_sec": 5}),
            ),
        )
        .await;

    assert_failed(&result, ToolErrorCode::Timeout);
    assert!(result.error.unwrap().message.contains("5s"));
}

#[tokio::test(start_paused = true)]
async fn run_python_timeout_from_the_outer_budget() {
    // Docker daemon 卡住、连 exec 都不返回时，外层 timeout 兜底。
    let f = fixture();
    let gateway = f.raw().with_run_python_grace(0.05);
    register(&gateway);
    f.sandbox.set_exec_delay(Duration::from_secs(30));

    let result = gateway
        .call(
            &f.ctx,
            &req_args("run_python", json!({"code": "print(1)", "timeout_sec": 1})),
        )
        .await;

    assert_failed(&result, ToolErrorCode::Timeout);
    assert!(result.duration_ms < 5_000, "{}", result.duration_ms);
}

#[tokio::test(start_paused = true)]
async fn run_python_budget_follows_the_request_not_the_default() {
    // §3.3：run_python 用请求里的 timeout_sec，不是 60s 的默认值。
    let f = fixture();
    let gateway = f
        .raw()
        .with_default_timeout(0.05)
        .with_run_python_grace(0.5);
    register(&gateway);
    f.sandbox.set_exec_delay(Duration::from_millis(200)); // 比默认预算长，比 timeout_sec 短

    let result = gateway
        .call(
            &f.ctx,
            &req_args("run_python", json!({"code": "print(1)", "timeout_sec": 2})),
        )
        .await;

    assert!(result.ok, "{:?}", result.error);
}

// --- 对应表剩下的两条 -------------------------------------------------------

#[tokio::test]
async fn platform_failure_is_upstream() {
    let f = fixture();
    f.platform
        .fail_read_history(PlatformError::new("500", "飞书 500", true));
    let result = f.gateway().call(&f.ctx, &req("read_group_history")).await;
    assert_failed(&result, ToolErrorCode::Upstream);
    assert!(result.error.unwrap().message.contains("飞书 500"));
}

#[tokio::test]
async fn download_failure_is_upstream() {
    // §3.3：群里发的附件 adapter 下载失败 → download_attachment 返回 upstream。
    let f = fixture();
    f.platform
        .fail_download(PlatformError::new("404", "file_key 已过期", false));
    let result = f
        .gateway()
        .call(
            &f.ctx,
            &req_args("download_attachment", json!({"file_key": "file_v3_csv"})),
        )
        .await;
    assert_failed(&result, ToolErrorCode::Upstream);
}

#[tokio::test]
async fn sandbox_acquire_failure_is_sandbox() {
    // §3.3：沙箱创建/执行失败（Docker 不可用等）→ code=sandbox。
    let f = fixture();
    f.sandbox.fail_acquire(SandboxError::new(
        SandboxErrorKind::Unavailable,
        "Cannot connect to the Docker daemon",
    ));
    let result = f
        .gateway()
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    assert_failed(&result, ToolErrorCode::Sandbox);
}

#[tokio::test]
async fn sandbox_exec_failure_is_sandbox() {
    let f = fixture();
    f.sandbox
        .fail_exec(SandboxError::new(SandboxErrorKind::NotFound, "容器没了"));
    let result = f
        .gateway()
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    assert_failed(&result, ToolErrorCode::Sandbox);
}

#[tokio::test]
async fn missing_sandbox_is_sandbox() {
    let f = fixture();
    let result = f
        .without_sandbox()
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    assert_failed(&result, ToolErrorCode::Sandbox);
}

#[tokio::test]
async fn missing_platform_is_upstream() {
    let f = fixture();
    let result = f
        .without_platform()
        .call(&f.ctx, &req("read_group_history"))
        .await;
    assert_failed(&result, ToolErrorCode::Upstream);
}

// --- §3.2「永远不抛异常给调用方」---------------------------------------------

#[tokio::test]
async fn a_panicking_tool_becomes_a_result() {
    // 工具实现自己写崩了也不能把 worker 带走：panic 被接住，翻成 upstream。
    let f = fixture();
    let exploding: ToolImpl = Arc::new(
        |_env: ToolEnv, _ctx: ToolContext, _args: Map<String, Value>| {
            Box::pin(async { panic!("工具实现自己写崩了") })
        },
    );
    let gateway = f.raw().with_tool("list_files", exploding);
    register(&gateway);

    let result = gateway.call(&f.ctx, &req("list_files")).await;

    assert_failed(&result, ToolErrorCode::Upstream);
    assert!(
        result.error.unwrap().message.contains("工具实现自己写崩了"),
        "panic 的原话要带到消息里"
    );
}

#[tokio::test]
async fn cancelling_the_call_yields_no_result() {
    // 取消是调用方的意思（旧实现里 CancelledError 原样外抛）：
    // 不该伪造出一条 ToolResult，Rust 这边就是这个 future 被丢掉、什么都不产出。
    let f = fixture();
    let hang: ToolImpl = Arc::new(
        |_env: ToolEnv, _ctx: ToolContext, _args: Map<String, Value>| {
            Box::pin(async {
                std::future::pending::<()>().await;
                unreachable!()
            })
        },
    );
    let gateway = Arc::new(f.raw().with_tool("list_files", hang));
    register(&gateway);

    let ctx = f.ctx.clone();
    let handle = {
        let gateway = gateway.clone();
        tokio::spawn(async move { gateway.call(&ctx, &req("list_files")).await })
    };
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    handle.abort();

    let joined = handle.await;
    assert!(joined.unwrap_err().is_cancelled(), "取消后不该有结果");
}

// --- 执行顺序（§3.2 的注释）--------------------------------------------------

#[tokio::test]
async fn token_is_checked_before_the_tool_lookup() {
    // 带错 token 调一个不存在的工具 → denied，不是 not_found。
    // 反过来的话，攻击者能靠错误码探出「有哪些工具」。
    let f = fixture();
    let forged = f.ctx_with_token(&"f".repeat(32));
    let result = f.gateway().call(&forged, &req("no_such_tool")).await;
    assert_failed(&result, ToolErrorCode::Denied);
}

#[tokio::test]
async fn tool_lookup_happens_before_argument_validation() {
    // 不存在的工具 + 一堆乱参数 → not_found，不是 invalid_args。
    let f = fixture();
    let result = f
        .gateway()
        .call(&f.ctx, &req_args("no_such_tool", json!({"nonsense": 1})))
        .await;
    assert_failed(&result, ToolErrorCode::NotFound);
}

#[tokio::test]
async fn arguments_are_validated_before_the_tool_runs() {
    // 参数不合 schema 时工具压根不该被执行 —— 别为一个必然失败的调用起容器。
    let f = fixture();
    let gateway = f.gateway();
    let result = gateway
        .call(
            &f.ctx,
            &req_args(
                "run_python",
                json!({"code": "print(1)", "timeout_sec": 9999}),
            ),
        )
        .await;

    assert_failed(&result, ToolErrorCode::InvalidArgs);
    assert!(f.sandbox.exec_requests().is_empty());
    assert_eq!(f.sandbox.box_count(), 0);
    assert!(gateway.sandbox_id_of(&f.ctx.task_id).await.is_none());
}

// --- 截断与 ToolResult 的形状 ------------------------------------------------

#[tokio::test]
async fn content_is_clipped_to_the_cap() {
    let f = fixture();
    let flood = tool_impl(|| Ok(ToolOutcome::new("x".repeat(MAX_TOOL_CONTENT_CHARS * 3))));
    let gateway = f.raw().with_tool("list_files", flood);
    register(&gateway);

    let result = gateway.call(&f.ctx, &req("list_files")).await;

    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.content.chars().count(), MAX_TOOL_CONTENT_CHARS);
    assert!(result.content.ends_with("[内容已截断]"));
}

#[tokio::test]
async fn error_message_is_also_clipped() {
    let f = fixture();
    let shout = tool_impl(|| {
        Err(ToolFailure::upstream(
            "长".repeat(MAX_TOOL_CONTENT_CHARS * 2),
        ))
    });
    let gateway = f.raw().with_tool("list_files", shout);
    register(&gateway);

    let result = gateway.call(&f.ctx, &req("list_files")).await;

    assert_eq!(result.content.chars().count(), MAX_TOOL_CONTENT_CHARS);
    // error.message 不截断：日志与证据要看全文。
    assert_eq!(
        result.error.unwrap().message.chars().count(),
        MAX_TOOL_CONTENT_CHARS * 2
    );
}

#[tokio::test]
async fn result_echoes_call_id_and_name() {
    let f = fixture();
    let request = ToolCallRequest {
        call_id: "call-xyz".into(),
        name: "list_files".into(),
        arguments: Map::new(),
    };
    let result = f.gateway().call(&f.ctx, &request).await;

    assert_eq!(
        (result.call_id.as_str(), result.name.as_str()),
        ("call-xyz", "list_files")
    );
    assert!(result.ok, "{:?}", result.error);
}

#[tokio::test]
async fn a_tool_that_is_in_the_catalog_but_has_no_impl_is_not_found() {
    // 「目录里有、表里没有」→ not_found（消息说"在本次运行里没有实现"），不是崩掉。
    let f = fixture();
    let gateway = f.raw().with_tools(std::collections::HashMap::new());
    register(&gateway);

    let result = gateway.call(&f.ctx, &req("list_files")).await;

    assert_failed(&result, ToolErrorCode::NotFound);
    assert!(
        result.error.unwrap().message.contains("没有实现"),
        "要能分辨「不在目录里」和「没有实现」"
    );
    assert_eq!(
        error_code(&gateway.call(&f.ctx, &req("list_files")).await),
        Some(ToolErrorCode::NotFound)
    );
}
