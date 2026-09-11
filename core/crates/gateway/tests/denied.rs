//! session_token 校验的边角（对应旧 `tests/gateway/test_gateway_denied.py`，6 条）。
//!
//! 单独一份是因为这条闸是 Gateway 唯一的鉴权面：`Task.session_token` 是随机 32 hex，
//! 校验一旦有缝，任何拿到 chat_id / task_id 的调用方就能借别的任务的壳跑 run_python。
//! B5 只要求「不匹配 → denied」，这里把「不匹配」的各种长相都摆上。
mod common;

use std::sync::Arc;

use aite_contracts::{ToolErrorCode, ToolGateway};
use aite_gateway::TokenResolver;
use common::{SESSION_TOKEN, error_code, fixture, req};

#[tokio::test]
async fn the_registered_token_is_accepted() {
    // 先证明这道闸不是「一律拒」，否则下面每条都恒真。
    let f = fixture();
    assert!(f.gateway().call(&f.ctx, &req("list_files")).await.ok);
}

#[tokio::test]
async fn prefix_of_the_real_token_is_denied() {
    // 前缀相同不算匹配 —— 长度也得咬住。
    let f = fixture();
    let ctx = f.ctx_with_token(&SESSION_TOKEN[..16]);
    let result = f.gateway().call(&ctx, &req("list_files")).await;
    assert_eq!(error_code(&result), Some(ToolErrorCode::Denied));
}

#[tokio::test]
async fn token_with_trailing_whitespace_is_denied() {
    let f = fixture();
    let ctx = f.ctx_with_token(&format!("{SESSION_TOKEN} "));
    let result = f.gateway().call(&ctx, &req("list_files")).await;
    assert_eq!(error_code(&result), Some(ToolErrorCode::Denied));
}

#[tokio::test]
async fn case_flipped_token_is_denied() {
    let f = fixture();
    let ctx = f.ctx_with_token(&SESSION_TOKEN.to_uppercase());
    let result = f.gateway().call(&ctx, &req("list_files")).await;
    assert_eq!(error_code(&result), Some(ToolErrorCode::Denied));
}

#[tokio::test]
async fn non_ascii_token_is_denied_not_upstream() {
    // 旧实现里 compare_digest 收非 ASCII str 会抛 TypeError，一不小心就被兜成 upstream；
    // Rust 这边比的是字节，但这条仍然要钉：伪造的 token 必须是 denied，
    // 否则 §3.3 的重试逻辑会去重试一个鉴权失败。
    let f = fixture();
    let ctx = f.ctx_with_token(&"口".repeat(32));
    let result = f.gateway().call(&ctx, &req("list_files")).await;
    assert_eq!(error_code(&result), Some(ToolErrorCode::Denied));
}

#[tokio::test]
async fn token_resolver_failure_does_not_leak() {
    // resolver 是外部接来的（RΩ 把它接到 SessionStore 上），它炸了也不能把调用打断。
    let f = fixture();
    let boom: TokenResolver = Arc::new(|_task_id| Err("SQLite 锁了".to_string()));
    let gateway = f.raw().with_token_resolver(boom);

    let result = gateway.call(&f.ctx, &req("list_files")).await;

    assert!(!result.ok);
    let error = result.error.unwrap();
    assert_eq!(error.code, ToolErrorCode::Upstream); // 确实是外部系统出错
    assert!(error.message.contains("SQLite"), "{}", error.message);
}
