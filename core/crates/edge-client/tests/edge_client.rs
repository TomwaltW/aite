//! `EdgeClient` 的往返与错误映射（假 edge = 真 tonic server + tempdir 里的 UDS）。
//!
//! 验的东西分四组：
//! 1. 每个方法的往返（domain → pb → RPC → pb → domain，参数不丢、返回对得上）
//! 2. 错误映射（UNAVAILABLE → retryable、NOT_FOUND 的两种前缀、INVALID_ARGUMENT、4xx 不重试）
//! 3. `max_message_mb` 两个方向都设（8MB 的 send_file / download_file 不炸）
//! 4. edge 不在时 RPC 是 retryable 且不 panic、edge 起来后自动恢复
//!
//! `IngressServer` 那组在 `ingress.rs`。不碰网络、不碰 Docker、不靠真实 sleep。
mod common;

use std::time::Duration;

use aite_contracts::{
    CONTRACT_VERSION, CardStatus, ChecklistCard, ChecklistItemView, ChecklistState, EdgeConfig,
    ExecRequest, OutboundFile, OutboundText, PlatformPort, ReactionKind, SandboxErrorKind,
    SandboxSpec,
};
use aite_edge_client::EdgeClient;
use common::FakeEdge;
use tonic::Code;

fn config(edge: &FakeEdge) -> EdgeConfig {
    EdgeConfig {
        edge_socket: edge.socket().display().to_string(),
        core_socket: edge.dir().join("aite-core.sock").display().to_string(),
        handle_event_deadline_ms: 1000,
        max_message_mb: 16,
    }
}

async fn client(edge: &FakeEdge) -> EdgeClient {
    EdgeClient::connect(&config(edge), edge.dir())
        .await
        .expect("懒连接不该失败")
}

/// 假 edge 收到的调用，去掉契约闸门那几发 `GetStatus`。
fn method_calls(edge: &FakeEdge) -> Vec<String> {
    edge.calls()
        .into_iter()
        .filter(|m| m != "GetStatus")
        .collect()
}

fn card() -> ChecklistCard {
    ChecklistCard {
        task_id: "task-1".into(),
        task_no: "#A17".into(),
        title: "跑一遍对账".into(),
        initiator: "张三".into(),
        started_at: "9:02".into(),
        status: CardStatus::Working,
        items: vec![ChecklistItemView {
            id: "c1".into(),
            text: "核对单据".into(),
            state: ChecklistState::Doing,
            note: None,
        }],
        footer: "预计 2 分钟".into(),
        actions: Vec::new(),
    }
}

// --- 1. 往返 ---------------------------------------------------------------

#[tokio::test]
async fn platform_round_trip_covers_every_method() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    let platform = client.platform();

    let sent = platform
        .send_text(&OutboundText::new("oc_demo", "你好"))
        .await
        .expect("send_text");
    assert_eq!(sent.message_id, "om_text_oc_demo");

    let card_sent = platform
        .send_card("oc_demo", Some("om_root"), &card())
        .await
        .expect("send_card");
    assert_eq!(card_sent.card_id.as_deref(), Some("om_card_#A17"));

    platform
        .update_card("om_card_#A17", &card())
        .await
        .expect("update_card");

    let file_sent = platform
        .send_file(&OutboundFile {
            chat_id: "oc_demo".into(),
            reply_to: None,
            name: "out.png".into(),
            mime: "image/png".into(),
            data: vec![1, 2, 3],
        })
        .await
        .expect("send_file");
    assert_eq!(file_sent.message_id, "om_file");
    assert_eq!(edge.state().last_send_file_bytes, 3);

    platform
        .add_reaction("om_root", ReactionKind::Ack)
        .await
        .expect("add_reaction");

    let history = platform
        .read_history("oc_demo", 7, Some("om_root"))
        .await
        .expect("read_history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].message_id, "om_1");
    assert_eq!(history[0].sender_kind, "human");
    assert_eq!(history[0].thread_id.as_deref(), Some("om_root"));
    let asked = edge.state().last_read_history.clone().expect("记下请求");
    assert_eq!((asked.chat_id.as_str(), asked.limit), ("oc_demo", 7));
    assert_eq!(asked.thread_id.as_deref(), Some("om_root"));

    let doc = platform
        .read_document("https://feishu.cn/docx/abc")
        .await
        .expect("read_document");
    assert_eq!(doc.title, "退款流程 SOP");
    assert_eq!(doc.url, "https://feishu.cn/docx/abc");

    let blob = platform
        .download_file("om_root", "file_v3_csv")
        .await
        .expect("download_file");
    assert_eq!(blob, vec![7u8; 4]);

    // 过滤掉 `GetStatus`：第一发通了的 RPC 会顺手补比一次 `contract_version`
    // （`gate.rs` 的契约闸门 —— 起飞之后唯一的复查点）。那是基础设施的一发，
    // 不属于「每个 PlatformPort 方法各往返一遍」这条断言要管的事。
    assert_eq!(
        method_calls(&edge),
        vec![
            "SendText",
            "SendCard",
            "UpdateCard",
            "SendFile",
            "AddReaction",
            "ReadHistory",
            "ReadDocument",
            "DownloadFile",
        ]
    );
}

#[tokio::test]
async fn sandbox_round_trip_covers_every_method() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    let sandbox = client.sandbox();

    let spec = SandboxSpec::new("aite-sandbox:p0");
    let sandbox_id = sandbox.acquire("task-1", &spec).await.expect("acquire");
    assert_eq!(sandbox_id, "sb-task-1-1024"); // task_id 与 spec 都过去了

    let result = sandbox
        .exec(&sandbox_id, &ExecRequest::python("print(1)", 30))
        .await
        .expect("exec");
    assert_eq!((result.exit_code, result.stdout.as_str()), (0, "hi\n"));
    assert_eq!(result.files_out[0].path, "/work/out.png");
    let asked = edge.state().last_exec.clone().expect("记下 exec");
    let asked_req = asked.req.expect("req");
    assert_eq!(
        (asked_req.code.as_str(), asked_req.timeout_sec),
        ("print(1)", 30)
    );

    sandbox
        .put_file(&sandbox_id, "/work/in/a.csv", b"month,amount\n")
        .await
        .expect("put_file");
    assert_eq!(
        edge.state().last_put_file.clone().expect("记下 put_file"),
        (sandbox_id.clone(), "/work/in/a.csv".to_string(), 13)
    );

    assert_eq!(
        sandbox
            .get_file(&sandbox_id, "/work/out.png")
            .await
            .expect("get_file"),
        b"png-bytes".to_vec()
    );
    assert_eq!(
        sandbox.list_files(&sandbox_id).await.expect("list_files"),
        vec!["/work/in/a.csv", "/work/out.png"]
    );
    sandbox.touch(&sandbox_id).await.expect("touch");
    sandbox.release(&sandbox_id).await.expect("release");
    assert_eq!(
        sandbox.reap_idle(300).await.expect("reap_idle"),
        vec!["sb-idle-300"]
    );
    // close_all 是本地 no-op：沙箱记账在 edge，不该多打一次 RPC
    sandbox.close_all().await.expect("close_all");

    // 同上：`GetStatus` 是契约闸门的补比，不算 SandboxPort 的往返。
    assert_eq!(
        method_calls(&edge),
        vec![
            "Acquire",
            "Exec",
            "PutFile",
            "GetFile",
            "ListFiles",
            "Touch",
            "Release",
            "ReapIdle",
        ]
    );
}

#[tokio::test]
async fn status_reports_the_contract_version() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;

    let status = client.status().await.expect("status");

    assert_eq!(status.contract_version, CONTRACT_VERSION);
    assert_eq!(status.platform, "fake");
    assert!(status.platform_connected && status.sandbox_ok);
}

#[tokio::test]
async fn capabilities_are_cached_after_the_first_call() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    let platform = client.edge_platform();

    let first = platform.refresh_capabilities().await.expect("capabilities");
    let second = platform.refresh_capabilities().await.expect("capabilities");

    assert_eq!(first, second);
    assert_eq!(edge.call_count("GetCapabilities"), 1, "只问一次");
    assert_eq!(platform.capabilities(), first);
}

// --- 2. 错误映射 -----------------------------------------------------------

#[tokio::test]
async fn unavailable_is_retryable() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    edge.fail("SendText", Code::Unavailable, "transport_error: 飞书断线");

    let error = client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await
        .expect_err("该失败");

    assert!(error.retryable, "{error:?}");
    assert_eq!(error.code, "transport_error");
    assert_eq!(error.message, "飞书断线");
}

#[tokio::test]
async fn platform_4xx_is_not_retryable_and_keeps_http_status() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    edge.fail("ReadDocument", Code::NotFound, "230002: 文档不存在");

    let error = client
        .platform()
        .read_document("https://feishu.cn/docx/nope")
        .await
        .expect_err("该失败");

    assert!(!error.retryable);
    assert_eq!(error.code, "230002");
    assert_eq!(error.http_status, Some(404));
}

#[tokio::test]
async fn sandbox_not_found_and_file_not_found_are_two_kinds() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    let sandbox = client.sandbox();

    edge.fail("Exec", Code::NotFound, "sandbox_not_found: sb-9 不在");
    let error = sandbox
        .exec("sb-9", &ExecRequest::python("print(1)", 5))
        .await
        .expect_err("该失败");
    assert_eq!(error.kind, SandboxErrorKind::NotFound);

    edge.fail(
        "GetFile",
        Code::NotFound,
        "file_not_found: /work/out.png 不在",
    );
    let error = sandbox
        .get_file("sb-1", "/work/out.png")
        .await
        .expect_err("该失败");
    // worker 据此「跳过该产物」，所以这两种必须分得开
    assert_eq!(error.kind, SandboxErrorKind::FileNotFound);
}

#[tokio::test]
async fn sandbox_invalid_argument_is_an_invalid_path() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    edge.fail(
        "PutFile",
        Code::InvalidArgument,
        "bad_path: 路径不在 /work 下",
    );

    let error = client
        .sandbox()
        .put_file("sb-1", "/etc/passwd", b"nope")
        .await
        .expect_err("该失败");

    assert_eq!(error.kind, SandboxErrorKind::InvalidPath);
}

#[tokio::test]
async fn docker_unavailable_is_unavailable() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    edge.fail("Acquire", Code::Unavailable, "docker_down: 连不上 daemon");

    let error = client
        .sandbox()
        .acquire("task-1", &SandboxSpec::new("aite-sandbox:p0"))
        .await
        .expect_err("该失败");

    assert_eq!(error.kind, SandboxErrorKind::Unavailable);
}

#[tokio::test]
async fn unimplemented_from_the_skeleton_is_not_retryable() {
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    edge.fail("SendCard", Code::Unimplemented, "unimplemented: 骨架期");

    let error = client
        .platform()
        .send_card("oc_demo", None, &card())
        .await
        .expect_err("该失败");

    assert!(!error.retryable);
}

#[tokio::test]
async fn a_reply_that_does_not_convert_is_reported_not_panicked() {
    // edge 发来少了 created_at 的 HistoryMessage：pb → domain 转不回去。
    // 这时候要的是一条人话错误，不是 panic（纪律 3）。
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    edge.state().history = Some(vec![aite_proto::pb::HistoryMessage {
        message_id: "om_1".into(),
        sender_id: "ou_zhang".into(),
        sender_kind: "human".into(),
        sender_name: None,
        text: "缺时间戳".into(),
        thread_id: None,
        created_at: None,
    }]);

    let error = client
        .platform()
        .read_history("oc_demo", 5, None)
        .await
        .expect_err("该失败");

    assert_eq!(error.code, "bad_response");
    assert!(!error.retryable);
    assert!(error.message.contains("created_at"), "{}", error.message);
}

// --- 3. max_message_mb ------------------------------------------------------

#[tokio::test]
async fn eight_megabytes_go_through_both_directions() {
    // tonic 默认解码上限是 4MB；两个方向都设了 max_message_mb 这条才过。
    let edge = FakeEdge::start().await;
    let client = client(&edge).await;
    let eight_mb = 8 * 1024 * 1024;
    edge.state().download_bytes = eight_mb;

    let sent = client
        .platform()
        .send_file(&OutboundFile {
            chat_id: "oc_demo".into(),
            reply_to: None,
            name: "big.bin".into(),
            mime: "application/octet-stream".into(),
            data: vec![3u8; eight_mb],
        })
        .await
        .expect("8MB 出站");
    assert_eq!(sent.message_id, "om_file");
    assert_eq!(edge.state().last_send_file_bytes, eight_mb);

    let blob = client
        .platform()
        .download_file("om_root", "file_big")
        .await
        .expect("8MB 入站");
    assert_eq!(blob.len(), eight_mb);
}

// --- 4. edge 不在 / 起来之后 -------------------------------------------------

#[tokio::test]
async fn rpc_without_edge_is_retryable_and_recovers_when_edge_starts() {
    let mut edge = FakeEdge::cold(); // socket 路径占着，服务没起
    let client = client(&edge).await;

    let error = client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await
        .expect_err("edge 不在该失败");
    assert!(error.retryable, "拨不通要当可重试：{error:?}");
    assert!(!client.connected());

    // edge 起来了：同一个 client 不用重建，下一发 RPC 自己恢复（tonic 每次请求重拨）
    edge.warm_up().await;
    let sent = client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await
        .expect("edge 起来后该通");
    assert_eq!(sent.message_id, "om_text_oc_demo");

    edge.stop().await;
}

#[tokio::test]
async fn connect_does_not_wait_for_edge() {
    // §2.1 启动顺序无关：edge 没起，connect 也要立刻成功返回（懒连接）。
    let edge = FakeEdge::cold();
    let started = std::time::Instant::now();

    let client = EdgeClient::connect(&config(&edge), edge.dir())
        .await
        .expect("connect 不该等 edge");

    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(client.edge_socket(), edge.socket());
}

#[tokio::test]
async fn a_relative_socket_path_resolves_against_the_repo_root() {
    let edge = FakeEdge::start().await;
    let root = edge.dir().to_path_buf();
    let cfg = EdgeConfig {
        edge_socket: "aite-edge.sock".into(), // 相对路径
        core_socket: "aite-core.sock".into(),
        ..EdgeConfig::default()
    };

    let client = EdgeClient::connect(&cfg, &root).await.expect("connect");

    assert_eq!(client.edge_socket(), edge.socket());
    // 真能打通，说明解析出来的就是假 edge 那个 socket
    client.status().await.expect("status");
}
