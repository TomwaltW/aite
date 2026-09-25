//! CC2 的路由与入口：③ 入口把存储 / 证据错误传出去；⑥ 能力位与 p2p；⑦ R6 追问补 ack；
//! ⑧ `!new` 之后同话题的回复；⑩ `append_turn` 撞号重试。
mod support;

use aite_contracts::IngressError;
use aite_control::Ingress;
use support::{Harness, ev};

/// ③：`handler()` 对存储错误返回 `Err`（gRPC 入口翻成 INTERNAL，`proto/src/status.rs`：
/// 非 `Invalid` 一律 INTERNAL；control 不依赖 aite-proto，断言到这里为止），计数照打。
#[tokio::test]
async fn ingress_store_failure_returns_internal() {
    let h = Harness::new();
    h.store.fail_next("seen_event", 1);
    let plane = h.plane();
    let ingress = Ingress::new(plane.clone());

    let out = (ingress.handler())(ev().build()).await;
    assert!(
        matches!(out, Err(IngressError::Store(_))),
        "存储错误该交回给 gRPC 那一层，实际是 {out:?}"
    );
    assert_eq!(ingress.counter("ingress.errors"), 1);
    assert_eq!(plane.counter("events.dropped"), 1);
}
