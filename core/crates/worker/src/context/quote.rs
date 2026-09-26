//! 引用消息块（钉钉 / 企微的引用快照）。主人：EE13。
//!
//! CC3 预埋的块：本轨恒 `None`，在 `build_messages` 里按 `context/mod.rs` 模块文档的顺序真调。
//! 启用它**只改这个文件**。
use aite_contracts::Message;

use super::BlockCtx;

pub async fn block(ctx: &BlockCtx<'_>) -> Option<Message> {
    let _ = ctx;
    None
}
