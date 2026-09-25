//! 频道会话（自动回应顶层非 @ 消息）。主人：FF1。
//!
//! CC2 预埋的桩：现在一律 `Pass`，不做 I/O、不计数、不打日志（替身的调用记录因此不变）。
//! 启用它**只改这个文件**；它在链上的位置见 `routing/mod.rs` 的头注释。
use aite_contracts::{IngressError, NormalizedEvent};

use crate::plane::InProcessControlPlane;
use crate::routing::Flow;

/// R7 与 R8 之间。
pub(crate) async fn hook(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
) -> Result<Flow, IngressError> {
    let _ = (plane, ev);
    Ok(Flow::Pass)
}
