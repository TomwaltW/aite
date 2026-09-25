//! 准入闸：本群是否启用、外部群模式、私聊开关、群名单、成员白名单。主人：EE8。
//!
//! CC2 预埋的桩：现在一律 `Pass`，不做 I/O、不计数、不打日志（替身的调用记录因此不变）。
//! 启用它**只改这个文件**；它在链上的位置见 `routing/mod.rs` 的头注释。
use aite_contracts::{IngressError, NormalizedEvent};

use crate::plane::InProcessControlPlane;
use crate::routing::Flow;

/// R2 之后、R3 之前。未启用的群 / 私聊里 `!connect <码>`、`!help`、`!about` 要返回 `Pass` 放行到命令路由。
pub(crate) async fn hook(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
) -> Result<Flow, IngressError> {
    let _ = (plane, ev);
    Ok(Flow::Pass)
}
