//! `IngressServer`（core 侧监听、edge 调 `HandleEvent`）。
//!
//! 契约（edge.proto 头注释 / §3.3）：
//! - handler 被调到，返回 Ok 就回 `HandleEventResponse`
//! - 事件转不回 domain → `INVALID_ARGUMENT`（edge 不重推，计 ingress.invalid）
//! - handler 失败 → `INTERNAL`（edge 让平台重推）
//! - 1s 内返回（edge 用 1s deadline 调；handler 自己慢那条由调用方的 deadline 兜，在 Go 侧）
mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aite_contracts::{
    Anchor, ChatType, EdgeConfig, EventHandler, EventKind, IngressError, NormalizedEvent,
    PlatformPort, SenderKind,
};
use aite_edge_client::EdgeClient;
use aite_proto::pb;
use aite_proto::pb::ingress_service_client::IngressServiceClient;
use chrono::{TimeZone, Utc};
use common::FakeEdge;
use serde_json::Map;
use tonic::Code;
use tonic::transport::Endpoint;

fn event(event_id: &str) -> NormalizedEvent {
    NormalizedEvent {
        event_id: event_id.to_string(),
        kind: EventKind::Message,
        platform: "feishu".into(),
        tenant_id: "default".into(),
        workspace_id: "cli_fake_app".into(),
        chat_id: "oc_demo".into(),
        chat_type: ChatType::Group,
        sender_id: "ou_alice".into(),
        sender_kind: SenderKind::Human,
        sender_name: Some("Alice".into()),
        text: "今天北京天气怎么样".into(),
        raw_text: None,
        mentioned: true,
        anchor: Anchor {
            platform: "feishu".into(),
            chat_id: "oc_demo".into(),
            message_id: format!("om_{event_id}"),
            thread_id: None,
            task_no: None,
        },
        attachments: Vec::new(),
        card_action: None,
        occurred_at: Utc.with_ymd_and_hms(2026, 9, 9, 9, 0, 0).single().unwrap(),
        raw: Map::new(),
    }
}

/// 记账 + 可脚本化失败的 handler。
#[derive(Default)]
struct Seen {
    events: Vec<String>,
    fail_with: Option<String>,
}

fn handler(seen: Arc<Mutex<Seen>>) -> EventHandler {
    Arc::new(move |ev: NormalizedEvent| {
        let seen = seen.clone();
        Box::pin(async move {
            let mut state = seen.lock().unwrap();
            state.events.push(ev.event_id.clone());
            match state.fail_with.clone() {
                Some(reason) => Err(IngressError::Other(reason)),
                None => Ok(()),
            }
        })
    })
}

struct Started {
    client: EdgeClient,
    seen: Arc<Mutex<Seen>>,
    socket: std::path::PathBuf,
}

/// 起一个只开 IngressServer 的 core 侧（edge 的三套服务这组用不到）。
async fn start_ingress(edge: &FakeEdge) -> Started {
    let socket = edge.dir().join("aite-core.sock");
    let cfg = EdgeConfig {
        edge_socket: edge.socket().display().to_string(),
        core_socket: socket.display().to_string(),
        handle_event_deadline_ms: 1000,
        max_message_mb: 16,
    };
    let client = EdgeClient::connect(&cfg, edge.dir())
        .await
        .expect("connect");
    let seen = Arc::new(Mutex::new(Seen::default()));
    client
        .platform()
        .start(handler(seen.clone()))
        .await
        .expect("start ingress");
    Started {
        client,
        seen,
        socket,
    }
}

async fn ingress_client(
    socket: &std::path::Path,
) -> IngressServiceClient<tonic::transport::Channel> {
    let channel = Endpoint::from_shared(format!("unix://{}", socket.display()))
        .expect("地址")
        .connect()
        .await
        .expect("连 core 的 ingress");
    IngressServiceClient::new(channel)
}

#[tokio::test]
async fn handle_event_reaches_the_handler() {
    let edge = FakeEdge::start().await;
    let core = start_ingress(&edge).await;
    assert!(core.client.edge_platform().started());

    let mut caller = ingress_client(&core.socket).await;
    caller
        .handle_event(pb::NormalizedEvent::from(event("e1")))
        .await
        .expect("HandleEvent 该成功");

    assert_eq!(core.seen.lock().unwrap().events, vec!["e1"]);
}

#[tokio::test]
async fn an_invalid_event_is_invalid_argument() {
    // 枚举 UNSPECIFIED / anchor 缺失：edge 不该重推，所以是 INVALID_ARGUMENT 不是 INTERNAL。
    let edge = FakeEdge::start().await;
    let core = start_ingress(&edge).await;
    let mut caller = ingress_client(&core.socket).await;

    let mut broken = pb::NormalizedEvent::from(event("e2"));
    broken.sender_kind = pb::SenderKind::Unspecified as i32;
    let status = caller.handle_event(broken).await.expect_err("该被拒");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().starts_with("invalid_event: "),
        "{}",
        status.message()
    );

    let mut no_anchor = pb::NormalizedEvent::from(event("e3"));
    no_anchor.anchor = None;
    let status = caller.handle_event(no_anchor).await.expect_err("该被拒");
    assert_eq!(status.code(), Code::InvalidArgument);

    assert!(
        core.seen.lock().unwrap().events.is_empty(),
        "非法事件不该进 handler"
    );
}

#[tokio::test]
async fn a_failing_handler_becomes_internal() {
    let edge = FakeEdge::start().await;
    let core = start_ingress(&edge).await;
    core.seen.lock().unwrap().fail_with = Some("SQLite 抖了".into());
    let mut caller = ingress_client(&core.socket).await;

    let status = caller
        .handle_event(pb::NormalizedEvent::from(event("e4")))
        .await
        .expect_err("该失败");

    // edge 据此计 ingress.errors 并让平台重推；重复由 ControlPlane 靠 event_id 去重。
    assert_eq!(status.code(), Code::Internal);
    assert!(
        status.message().contains("ingress_failed"),
        "{}",
        status.message()
    );
    assert_eq!(core.seen.lock().unwrap().events, vec!["e4"]);
}

#[tokio::test]
async fn handle_event_returns_well_within_one_second() {
    let edge = FakeEdge::start().await;
    let core = start_ingress(&edge).await;
    let mut caller = ingress_client(&core.socket).await;

    let started = Instant::now();
    caller
        .handle_event(pb::NormalizedEvent::from(event("e5")))
        .await
        .expect("HandleEvent");
    let elapsed = started.elapsed();

    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
}

#[tokio::test]
async fn stop_closes_the_listener_and_start_is_idempotent() {
    let edge = FakeEdge::start().await;
    let core = start_ingress(&edge).await;
    let platform = core.client.edge_platform();

    // 再 start 一次不该炸、也不该换掉监听
    platform
        .start(handler(core.seen.clone()))
        .await
        .expect("start 幂等");
    assert!(platform.started());

    platform.stop().await.expect("stop");
    assert!(!platform.started());
    assert!(!core.socket.exists(), "socket 文件该收掉");

    // 关了就连不上了
    let failed = Endpoint::from_shared(format!("unix://{}", core.socket.display()))
        .expect("地址")
        .connect()
        .await;
    assert!(failed.is_err(), "停了之后不该还能连上");

    // 停完能再起（残留 socket 也不挡）
    platform
        .start(handler(core.seen.clone()))
        .await
        .expect("重新 start");
    let mut caller = ingress_client(&core.socket).await;
    caller
        .handle_event(pb::NormalizedEvent::from(event("e6")))
        .await
        .expect("重新起来还能收");
    assert_eq!(core.seen.lock().unwrap().events, vec!["e6"]);
    platform.stop().await.expect("stop");
}

#[tokio::test]
async fn start_clears_a_stale_socket_file() {
    // 上一条命留下的 socket 文件会让 bind EADDRINUSE —— start 要自己清掉。
    let edge = FakeEdge::start().await;
    let socket = edge.dir().join("stale").join("aite-core.sock");
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    std::fs::write(&socket, b"stale").unwrap();

    let cfg = EdgeConfig {
        edge_socket: edge.socket().display().to_string(),
        core_socket: socket.display().to_string(),
        ..EdgeConfig::default()
    };
    let client = EdgeClient::connect(&cfg, edge.dir())
        .await
        .expect("connect");
    let seen = Arc::new(Mutex::new(Seen::default()));
    client
        .platform()
        .start(handler(seen.clone()))
        .await
        .expect("残留 socket 不该挡住 start");

    let mut caller = ingress_client(&socket).await;
    caller
        .handle_event(pb::NormalizedEvent::from(event("e7")))
        .await
        .expect("HandleEvent");
    assert_eq!(seen.lock().unwrap().events, vec!["e7"]);
    client.platform().stop().await.expect("stop");
}

#[tokio::test]
async fn start_creates_the_socket_parent_directory() {
    let edge = FakeEdge::start().await;
    let socket = edge.dir().join("data").join("run").join("aite-core.sock");
    let cfg = EdgeConfig {
        edge_socket: edge.socket().display().to_string(),
        core_socket: socket.display().to_string(),
        ..EdgeConfig::default()
    };

    let client = EdgeClient::connect(&cfg, edge.dir())
        .await
        .expect("connect");
    client
        .platform()
        .start(handler(Arc::new(Mutex::new(Seen::default()))))
        .await
        .expect("父目录该自己建");

    assert!(socket.exists());
    client.platform().stop().await.expect("stop");
}

#[tokio::test]
async fn start_also_caches_capabilities_from_edge() {
    let edge = FakeEdge::start().await;
    let core = start_ingress(&edge).await;

    assert_eq!(edge.call_count("GetCapabilities"), 1);
    assert_eq!(core.client.platform().capabilities().platform, "feishu");
}

#[tokio::test]
async fn start_survives_an_edge_that_is_not_up_yet() {
    // §2.1：启动顺序无关。edge 不在也要能把投递面起起来（capabilities 问不到就先用默认值）。
    let edge = FakeEdge::cold();
    let core = start_ingress(&edge).await;

    assert!(core.client.edge_platform().started());
    let mut caller = ingress_client(&core.socket).await;
    caller
        .handle_event(pb::NormalizedEvent::from(event("e8")))
        .await
        .expect("edge 不在也该能收事件");
    assert_eq!(core.seen.lock().unwrap().events, vec!["e8"]);
}
