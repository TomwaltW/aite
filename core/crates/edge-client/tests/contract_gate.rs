//! `contract_version` 门禁在**起飞之后**还管不管用（审核记账 4.1 第 1 行）。
//!
//! 病灶：`build_app` 的第 4 步最多拨 5 次 × 1s 去问 edge 的 `contract_version`，答上来就
//! 严格比、不等就拒绝起飞；始终答不上来就记一行 `aite.edge_unreachable` 照常起飞
//! （§2.1「启动顺序无关，连不上不退出」）。问题不在这个折中，在**此后整个进程生命周期
//! 再也没有第二次比对**。三条常态路径都落进这个缺口：
//!
//! 1. `docker-compose.yml` 刻意不写 `depends_on`（§2.1 那条「谁先起都行」），
//!    core 先起完、edge 超过 ~4s 才就绪，是完全正常的一次 `docker compose up`；
//! 2. `acceptance-M.md` §M6 三遍里的**「只重启 edge」**那一遍 —— 这条路上门禁**必然**
//!    不生效，没有任何随机性；
//! 3. 真机排障时手动重启 aite-edge。
//!
//! 这一组钉住下沉之后的三条：**连上就比一次**、**比出不一致就闸断**、**对上了自己解开**。
//!
//! 不碰网络、不碰 Docker、不靠真实 sleep：假 edge 是真 tonic server + tempdir 里的 UDS，
//! 时序全靠「显式改假 edge 的答案 + 一次真实的 RPC 往返」推进。
mod common;

use aite_contracts::{EdgeConfig, OutboundText, SandboxSpec};
use aite_edge_client::{ContractState, EdgeClient};
use common::FakeEdge;

/// edge 换了新契约之后报的那个版本。
const OTHER: &str = "p0-2099-01-01";

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

/// 等闸门走到某个状态。探针是后台 task，`await` 让它跑；不睡墙钟。
async fn wait_for_state(client: &EdgeClient, want: impl Fn(&ContractState) -> bool, what: &str) {
    for _ in 0..2000 {
        if want(&client.contract_state()) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("没等到：{what}（当前 {:?}）", client.contract_state());
}

/// 还没比成过的时候 RPC 照常放行 —— 否则 §2.1 的懒连接就废了。
#[tokio::test]
async fn an_unverified_gate_lets_rpcs_through() {
    let edge = FakeEdge::cold(); // socket 占着，服务没起
    let client = client(&edge).await;

    assert_eq!(client.contract_state(), ContractState::Unverified);
    let error = client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await
        .expect_err("edge 不在该失败");
    // 关键：失败的理由是「拨不通」（retryable），不是被自己人闸住
    assert!(error.retryable, "还没比过就闸人就错了：{error:?}");
    assert_ne!(error.code, "contract_mismatch");
}

/// **起飞之后**edge 才起来（`docker compose up` 下 core 先起 / M6 只重启了 edge）：
/// 第一发通了的 RPC 就该把契约补比一次 —— 这是原来那个缺口正中心的场景。
///
/// 为什么不等探针：它可能正卡在最长 30s 的退避 sleep 里，而「RPC 通了」这一刻
/// 已经确知连接是好的。等探针醒来的那段时间 core 是在「契约没比过」的状态下真干活的。
#[tokio::test]
async fn the_contract_is_compared_on_the_first_rpc_that_gets_through() {
    let mut edge = FakeEdge::cold();
    let client = client(&edge).await;

    // 第一发 RPC 拨不通
    let _ = client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await;
    assert_eq!(client.contract_state(), ContractState::Unverified);

    // edge 起来了，而且它是个**契约版本不一样**的 edge（M6「只重启 edge」那一遍）
    edge.set_contract_version(OTHER);
    edge.warm_up().await;

    // 这一发通了（闸门还是 Unverified，照常放行 —— §2.1 的懒连接语义）
    client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await
        .expect("edge 起来了这发该通");

    wait_for_state(
        &client,
        ContractState::is_barred,
        "第一发通了的 RPC 之后补比出不一致并闸断",
    )
    .await;
    match client.contract_state() {
        ContractState::Mismatch {
            edge_contract,
            edge_version,
        } => {
            assert_eq!(edge_contract, OTHER);
            assert_eq!(edge_version, "fake-edge");
        }
        other => panic!("该是 Mismatch，实际 {other:?}"),
    }
    assert!(
        edge.call_count("GetStatus") >= 1,
        "必须真的补发过一发 GetStatus：{:?}",
        edge.calls()
    );

    edge.stop().await;
}

/// 闸落下之后：platform 和 sandbox 两条链路都在**本地**直接失败，而且话说得清楚。
#[tokio::test]
async fn a_barred_gate_stops_both_platform_and_sandbox_rpcs() {
    let mut edge = FakeEdge::start().await;
    let client = client(&edge).await;

    // 先正常通一发（起飞体检那条路：status() 会顺手把结果记进闸门）
    client.status().await.expect("GetStatus");
    assert_eq!(client.contract_state(), ContractState::Ok);

    // 有人把 edge 换成了别的契约版本，然后再问一次
    edge.set_contract_version(OTHER);
    client.status().await.expect("GetStatus");
    assert!(client.contract_state().is_barred());

    let calls_before = edge.calls().len();

    let platform_err = client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await
        .expect_err("闸落着不该发得出去");
    assert_eq!(platform_err.code, "contract_mismatch");
    assert!(
        !platform_err.retryable,
        "两个进程版本不配，人不介入不会变好，不该当成抖动重试"
    );
    assert!(
        platform_err.message.contains("edge 换回对得上的版本"),
        "得说清楚怎么恢复：{}",
        platform_err.message
    );

    let sandbox_err = client
        .sandbox()
        .acquire("task-1", &SandboxSpec::new("python:3.11-slim"))
        .await
        .expect_err("闸落着不该发得出去");
    assert!(sandbox_err.message.contains("契约版本不一致"));

    // 拦在本地：这两发根本没到 edge 那边（只可能多出探针那几发 GetStatus）
    let after: Vec<String> = edge
        .calls()
        .into_iter()
        .skip(calls_before)
        .filter(|m| m != "GetStatus")
        .collect();
    assert!(after.is_empty(), "闸落着还有 RPC 打到 edge 上：{after:?}");

    edge.stop().await;
}

/// 自愈：edge 换回对得上的版本之后闸门自己开，**不用重启 core**。
///
/// 解锁的路只有一条：闸落着时每一下取 client 被拒都会顺手把探针叫起来
/// （RPC 全被拦在本地，不会再产生 UNAVAILABLE 去叫醒它）。探针拨通一次就重比一次。
#[tokio::test]
async fn the_gate_reopens_itself_once_edge_is_back_on_the_right_contract() {
    let mut edge = FakeEdge::start().await;
    let client = client(&edge).await;

    edge.set_contract_version(OTHER);
    client.status().await.expect("GetStatus");
    assert!(client.contract_state().is_barred());

    // 人把 edge 重新 build 对了、重启了 —— 但 core 这边没人动过
    edge.restore_contract_version();

    // 被拒的这一下顺手叫醒探针，探针重比一次版本
    let _ = client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await;
    wait_for_state(
        &client,
        |s| *s == ContractState::Ok,
        "探针重比一次之后闸门自己解开",
    )
    .await;

    // 解闸之后照常干活
    let sent = client
        .platform()
        .send_text(&OutboundText::new("oc_demo", "hi"))
        .await
        .expect("解闸之后该通");
    assert_eq!(sent.message_id, "om_text_oc_demo");

    edge.stop().await;
}

/// `GetStatus` 本身不许过闸门：拦住它，闸门就永远开不了 —— 闸落着时它是唯一还出得去的
/// 一发 RPC，`verify_contract` 靠它解锁。
///
/// 原注释还有半句「`!status` 的健康行也问不出来」：那条健康行**全仓不存在**。
/// `!status` 这条命令是真的（`control/src/plane.rs` 的 `cmd_status`），但它走
/// `status_tasks(&ev.chat_id)`，只从 store 列活跃任务，从头到尾不碰 edge。
/// 与 `app.rs:83`（W2 改掉）、`lib.rs` 那两处（X1）、`link.rs` 那两处（Y2）是同一句谎话的副本。
#[tokio::test]
async fn get_status_itself_is_never_barred() {
    let mut edge = FakeEdge::start().await;
    let client = client(&edge).await;

    edge.set_contract_version(OTHER);
    client.status().await.expect("GetStatus");
    assert!(client.contract_state().is_barred());

    let status = client.status().await.expect("闸落着也得问得出 edge 的健康");
    assert_eq!(status.contract_version, OTHER);

    edge.stop().await;
}
