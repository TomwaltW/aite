//! `catalog` 与工具目录的完整性（对应旧 `tests/gateway/test_gateway_catalog.py`，5 条）。
//!
//! §3.2：「P0 = gateway_tools() 原样」。§3.1 又写明这 5 个工具的名字与 schema 已冻结，
//! 所以「目录里有的都实现了、实现了的都在目录里」得有人常驻盯着 —— 少一个的表现不是报错，
//! 是模型调它时拿到 not_found，然后在日志里当成模型的问题。
mod common;

use aite_contracts::{ToolErrorCode, ToolGateway, gateway_tools};
use common::{assert_failed, fixture, req_args};
use serde_json::json;

#[tokio::test]
async fn catalog_is_gateway_tools_verbatim() {
    let f = fixture();
    assert_eq!(f.gateway().catalog(&f.ctx), gateway_tools().to_vec());
}

#[tokio::test]
async fn catalog_hands_back_a_fresh_vec() {
    let f = fixture();
    let gateway = f.gateway();
    let before = gateway_tools().len();

    let mut catalog = gateway.catalog(&f.ctx);
    catalog.push(catalog[0].clone());

    assert_eq!(gateway_tools().len(), before);
    assert_eq!(gateway.catalog(&f.ctx).len(), before);
}

#[tokio::test]
async fn catalog_does_not_depend_on_ctx() {
    // P0 不按 ctx 裁剪（scope / access bundle 是 P1）。
    let f = fixture();
    let gateway = f.gateway();
    let mut other = f.ctx.clone();
    other.chat_id = "oc_other".into();
    other.task_id = "task-other".into();

    assert_eq!(gateway.catalog(&other), gateway.catalog(&f.ctx));
}

#[tokio::test]
async fn every_frozen_tool_has_an_implementation() {
    // 目录里的每个名字都能走过「查实现」那一步：带个不认识的参数，
    // 有实现就止步于 invalid_args，没实现才会是 not_found。
    let f = fixture();
    let gateway = f.gateway();
    for tool in gateway_tools() {
        let result = gateway
            .call(&f.ctx, &req_args(&tool.name, json!({"__nope__": 1})))
            .await;
        assert_failed(&result, ToolErrorCode::InvalidArgs);
    }
}

#[tokio::test]
async fn the_five_p0_tool_names_are_exactly_these() {
    // 名字冻结（§3.1）。改一个名字 = 改契约，必须显式过这条。
    let names: Vec<&str> = gateway_tools().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "read_group_history",
            "read_document",
            "download_attachment",
            "run_python",
            "list_files",
        ]
    );
}
