//! 管理台（配置页、配对码）的监听。主人：DD12。
//!
//! CC4 ⑤ 预埋的空壳：`wire` 在组装前改 `FeatureCtx`（登记工具、推选项、写覆盖槽），`start` 在
//! 组装后起循环 / 监听。**只改这个文件**，`mod.rs` 不动。`wire` 里不许做任何 I/O
//! （`build_app` 只组装）；`start` 是同步函数，要起循环就 `tokio::spawn`，不许阻塞。
use super::{FeatureCtx, StartCtx};

pub(crate) fn wire(_: &mut FeatureCtx) -> Result<(), String> {
    Ok(())
}

pub(crate) fn start(_: &StartCtx) {}
