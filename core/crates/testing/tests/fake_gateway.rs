//! FakeToolGateway 自测（移植自 `tests/e2e/test_t4_fake_gateway.py`，18 条）：
//! 契约那套顺序、六个错误码、以及「过滤归 Gateway」这道分工。
use std::collections::BTreeMap;
use std::sync::Arc;

use aite_contracts::{
    DocumentContent, SandboxPort, ToolCallRequest, ToolContext, ToolErrorCode, ToolGateway,
    gateway_tools,
};
use aite_testing::{
    FakePlatform, FakeSandbox, FakeToolGateway, HistoryBuilder, history_message, validate,
};
use serde_json::{Map, Value, json};

fn ctx() -> ToolContext {
    ToolContext {
        tenant_id: "default".into(),
        workspace_id: "cli_app".into(),
        chat_id: "oc_1".into(),
        session_id: "s1".into(),
        task_id: "t1".into(),
        session_token: "tok".into(),
        thread_id: Some("om_1".into()),
        attachments_message_id: Some("om_1".into()),
    }
}

fn req(name: &str, arguments: Map<String, Value>) -> ToolCallRequest {
    ToolCallRequest {
        call_id: "c1".into(),
        name: name.into(),
        arguments,
    }
}

struct Stack {
    gw: FakeToolGateway,
    #[allow(dead_code)]
    platform: Arc<FakePlatform>,
    sandbox: Arc<FakeSandbox>,
}

fn stack(platform: FakePlatform, sandbox: FakeSandbox) -> Stack {
    let platform = Arc::new(platform);
    let sandbox = Arc::new(sandbox);
    Stack {
        gw: FakeToolGateway::new(platform.clone(), sandbox.clone()),
        platform,
        sandbox,
    }
}

fn plain() -> Stack {
    stack(FakePlatform::new(), FakeSandbox::default())
}

/// 契约：P0 的 catalog = gateway_tools() 原样。
#[test]
fn catalog_is_gateway_tools_verbatim() {
    let s = plain();
    assert_eq!(
        s.gw.catalog(&ctx())
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>(),
        gateway_tools()
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn unknown_tool_is_not_found() {
    let s = plain();
    let r = s.gw.call(&ctx(), &req("no_such_tool", Map::new())).await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().code, ToolErrorCode::NotFound);
}

#[tokio::test]
async fn bad_args_is_invalid_args() {
    let s = plain();
    // 少了必填的 url_or_token
    let r = s.gw.call(&ctx(), &req("read_document", Map::new())).await;
    assert!(!r.ok);
    let err = r.error.unwrap();
    assert_eq!(err.code, ToolErrorCode::InvalidArgs);
    assert!(err.message.contains("url_or_token"), "{}", err.message);
}

/// 顺序是「先校验 session_token」—— 连工具存不存在都还没查。
#[tokio::test]
async fn wrong_token_is_denied_before_anything_else() {
    let s = stack(FakePlatform::new(), FakeSandbox::default());
    let gw = s.gw.with_expected_token("right");
    let r = gw.call(&ctx(), &req("no_such_tool", Map::new())).await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().code, ToolErrorCode::Denied);
}

/// 契约：永远不把错误抛给调用方。
#[tokio::test]
async fn platform_failure_becomes_upstream_not_an_error() {
    let s = plain();
    let r =
        s.gw.call(
            &ctx(),
            &req(
                "read_document",
                aite_testing::kwargs! {"url_or_token" => json!("不存在")},
            ),
        )
        .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().code, ToolErrorCode::Upstream);
}

#[tokio::test]
async fn sandbox_failure_becomes_sandbox_code() {
    let sandbox = FakeSandbox::from_values(&[json!({"error": "Docker 不可用"})]).unwrap();
    let s = stack(FakePlatform::new(), sandbox);
    let r =
        s.gw.call(
            &ctx(),
            &req(
                "run_python",
                aite_testing::kwargs! {"code" => json!("print(1)")},
            ),
        )
        .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().code, ToolErrorCode::Sandbox);
}

/// PlatformPort.read_history 不过滤，过滤在这一层 —— 05_history_summary 验的就是它。
#[tokio::test]
async fn read_group_history_drops_non_human() {
    let platform = FakePlatform::new().with_history(vec![
        HistoryBuilder::new("om_1", "人说的")
            .sender_name("Alice")
            .build(),
        HistoryBuilder::new("om_2", "机器人播报")
            .sender_kind("bot")
            .build(),
        HistoryBuilder::new("om_3", "应用推的")
            .sender_kind("app")
            .build(),
    ]);
    let s = stack(platform, FakeSandbox::default());
    let r =
        s.gw.call(
            &ctx(),
            &req(
                "read_group_history",
                aite_testing::kwargs! {"limit" => json!(50)},
            ),
        )
        .await;
    assert!(r.ok);
    assert!(r.content.contains("[om_1] Alice: 人说的"), "{}", r.content);
    assert!(!r.content.contains("机器人播报"));
    assert!(!r.content.contains("应用推的"));
    let data = r.data.unwrap();
    assert_eq!(data["count"], json!(1));
    assert_eq!(data["dropped"], json!(2));
}

/// limit 不给时用 schema 里的 default=50，而不是报参数缺失。
#[tokio::test]
async fn read_group_history_applies_schema_default_limit() {
    let history: Vec<_> = (0..60)
        .map(|i| history_message(&format!("om_{i}"), &i.to_string()))
        .collect();
    let s = stack(
        FakePlatform::new().with_history(history),
        FakeSandbox::default(),
    );
    let r =
        s.gw.call(&ctx(), &req("read_group_history", Map::new()))
            .await;
    assert!(r.ok);
    assert_eq!(r.content.lines().count(), 50);
}

#[tokio::test]
async fn read_group_history_thread_only_uses_ctx_thread() {
    let platform = FakePlatform::new().with_history(vec![
        history_message("om_1", "顶层"),
        HistoryBuilder::new("om_2", "话题里")
            .thread_id("om_1")
            .build(),
    ]);
    let s = stack(platform, FakeSandbox::default());
    let r =
        s.gw.call(
            &ctx(),
            &req(
                "read_group_history",
                aite_testing::kwargs! {"thread_only" => json!(true)},
            ),
        )
        .await;
    assert!(r.content.contains("om_2"), "{}", r.content);
    assert!(!r.content.contains("om_1]"), "{}", r.content);
}

#[tokio::test]
async fn read_document_returns_title_in_content() {
    let platform = FakePlatform::new().with_documents(BTreeMap::from([(
        "tok".to_string(),
        DocumentContent {
            title: "Q3 交付计划".into(),
            text: "## 里程碑".into(),
            url: "u".into(),
        },
    )]));
    let s = stack(platform, FakeSandbox::default());
    let r =
        s.gw.call(
            &ctx(),
            &req(
                "read_document",
                aite_testing::kwargs! {"url_or_token" => json!("tok")},
            ),
        )
        .await;
    assert!(r.ok);
    assert!(r.content.contains("Q3 交付计划"));
    // 替身的排版是 `# {title}\n\n{text}`，没有来源行（真实现才有）
    assert_eq!(r.content, "# Q3 交付计划\n\n## 里程碑");
    assert_eq!(r.data.unwrap()["title"], json!("Q3 交付计划"));
}

#[tokio::test]
async fn download_attachment_lands_in_work_in() {
    let platform = FakePlatform::new().with_files(BTreeMap::from([(
        ("om_1".to_string(), "fk_csv".to_string()),
        b"month,amount".to_vec(),
    )]));
    let s = stack(platform, FakeSandbox::default());
    let r =
        s.gw.call(
            &ctx(),
            &req(
                "download_attachment",
                aite_testing::kwargs! {"file_key" => json!("fk_csv")},
            ),
        )
        .await;
    assert!(r.ok, "{}", r.content);
    assert_eq!(r.data.unwrap()["path"], json!("/work/in/fk_csv"));
    let sid = s.sandbox.box_ids()[0].clone();
    assert_eq!(
        s.sandbox.get_file(&sid, "/work/in/fk_csv").await.unwrap(),
        b"month,amount"
    );
}

/// 没有附件上下文 → upstream（不是 invalid_args）。
#[tokio::test]
async fn download_attachment_without_context_is_upstream() {
    let s = plain();
    let mut c = ctx();
    c.attachments_message_id = None;
    let r =
        s.gw.call(
            &c,
            &req(
                "download_attachment",
                aite_testing::kwargs! {"file_key" => json!("fk")},
            ),
        )
        .await;
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().code, ToolErrorCode::Upstream);
}

#[tokio::test]
async fn run_python_reuses_the_same_sandbox_per_task() {
    let s = plain();
    s.gw.call(
        &ctx(),
        &req("run_python", aite_testing::kwargs! {"code" => json!("a=1")}),
    )
    .await;
    s.gw.call(
        &ctx(),
        &req("run_python", aite_testing::kwargs! {"code" => json!("b=2")}),
    )
    .await;
    assert_eq!(s.sandbox.calls.count("acquire"), 1);
    assert_eq!(s.gw.sandbox_id_of("t1").await.as_deref(), Some("sb-1"));
}

#[tokio::test]
async fn run_python_reports_exit_code_and_files() {
    let sandbox = FakeSandbox::from_values(&[json!({
        "match": "savefig", "stdout": "done",
        "writes": {"/work/out.png": "builtin:png"}
    })])
    .unwrap();
    let s = stack(FakePlatform::new(), sandbox);
    let r =
        s.gw.call(
            &ctx(),
            &req(
                "run_python",
                aite_testing::kwargs! {"code" => json!("plt.savefig('/work/out.png')")},
            ),
        )
        .await;
    assert!(r.ok);
    // 排版逐字：exit_code=0\n--- stdout ---\n…（04 的断言耦合它）
    assert_eq!(r.content, "exit_code=0\n--- stdout ---\ndone");
    assert_eq!(r.data.unwrap()["files_out"], json!(["/work/out.png"]));
}

#[tokio::test]
async fn list_files_shows_what_was_downloaded() {
    let platform = FakePlatform::new().with_files(BTreeMap::from([(
        ("om_1".to_string(), "fk".to_string()),
        b"x".to_vec(),
    )]));
    let s = stack(platform, FakeSandbox::default());
    s.gw.call(
        &ctx(),
        &req(
            "download_attachment",
            aite_testing::kwargs! {"file_key" => json!("fk")},
        ),
    )
    .await;
    let r = s.gw.call(&ctx(), &req("list_files", Map::new())).await;
    assert_eq!(r.content, "/work/in/fk");
    // 空沙箱时是 (空)
    let s2 = plain();
    assert_eq!(
        s2.gw
            .call(&ctx(), &req("list_files", Map::new()))
            .await
            .content,
        "(空)"
    );
}

#[tokio::test]
async fn content_is_truncated_to_contract_limit() {
    let history: Vec<_> = (0..200)
        .map(|i| history_message(&format!("om_{i}"), &"长".repeat(200)))
        .collect();
    let s = stack(
        FakePlatform::new().with_history(history),
        FakeSandbox::default(),
    );
    let r =
        s.gw.call(
            &ctx(),
            &req(
                "read_group_history",
                aite_testing::kwargs! {"limit" => json!(200)},
            ),
        )
        .await;
    assert!(r.content.chars().count() <= 12000);
}

#[tokio::test]
async fn results_are_recorded_for_assertions() {
    let s = plain();
    s.gw.call(&ctx(), &req("list_files", Map::new())).await;
    assert_eq!(s.gw.count("list_files"), 1);
    assert_eq!(s.gw.results_of("list_files").len(), 1);
    assert_eq!(s.gw.results().len(), 1);
}

/// release_task 同时撤 token 和还沙箱，且幂等。
#[tokio::test]
async fn release_task_drops_sandbox_and_token() {
    let s = plain();
    s.gw.register_task("t1", "tok");
    s.gw.call(
        &ctx(),
        &req("run_python", aite_testing::kwargs! {"code" => json!("a=1")}),
    )
    .await;
    assert_eq!(s.sandbox.alive().len(), 1);
    s.gw.release_task("t1").await;
    s.gw.release_task("t1").await;
    assert!(s.sandbox.alive().is_empty());
    assert_eq!(s.sandbox.released_ids(), vec!["sb-1".to_string()]);
    assert!(s.gw.sandbox_id_of("t1").await.is_none());
}

// --- 迷你 schema 校验器本身 --------------------------------------------------

/// bool 不是 integer（Python 里 bool 是 int 的子类，不排掉的话 limit: true 会被当合法整数）。
#[test]
fn mini_schema_rejects_bool_as_integer() {
    let schema = json!({"type": "object", "properties": {"limit": {"type": "integer"}}});
    let err = validate(&json!({"limit": true}), &schema).unwrap_err();
    assert!(err.0.contains("期望 integer"), "{}", err.0);
}

#[test]
fn mini_schema_enforces_bounds_and_item_counts() {
    let schema = json!({
        "type": "object",
        "properties": {"items": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 2}},
        "required": ["items"]
    });
    assert_eq!(
        validate(&json!({"items": ["a"]}), &schema).unwrap(),
        json!({"items": ["a"]})
    );
    let err = validate(&json!({"items": ["a", "b", "c"]}), &schema).unwrap_err();
    assert!(err.0.contains("最多 2 项"), "{}", err.0);
    let err = validate(&json!({}), &schema).unwrap_err();
    assert!(err.0.contains("缺少必填参数"), "{}", err.0);
}

/// 这一份**不拒未知参数**（与真 Gateway 有意不同，清单 §1）。
#[test]
fn mini_schema_keeps_unknown_arguments() {
    let schema = json!({"type": "object", "properties": {"limit": {"type": "integer"}}});
    let out = validate(&json!({"limit": 3, "surprise": "x"}), &schema).unwrap();
    assert_eq!(out, json!({"limit": 3, "surprise": "x"}));
}
