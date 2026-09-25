//! 话题静音（👎 reaction / `!mute`）与解除（回复 / @）。主人：EE9。
//!
//! CC2 预埋的桩：现在一律 `Pass`，不做 I/O、不计数、不打日志（替身的调用记录因此不变）。
//! 启用它**只改这个文件**；它在链上的位置见 `routing/mod.rs` 的头注释。
use aite_contracts::{IngressError, NormalizedEvent};

use crate::plane::InProcessControlPlane;
use crate::routing::Flow;

/// R3 之后、R4 判定之前（与 `routing/external.rs` 同一位置）：收 👎 这类 reaction。
pub(crate) async fn pre_r4(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
) -> Result<Flow, IngressError> {
    let _ = (plane, ev);
    Ok(Flow::Pass)
}

/// R5 之前：收「回复 / @ 解除静音」这类消息侧逻辑。
pub(crate) async fn pre_r5(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
) -> Result<Flow, IngressError> {
    let _ = (plane, ev);
    Ok(Flow::Pass)
}
