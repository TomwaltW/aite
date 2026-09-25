//! 外部事件（GitLab webhook、飞书文档评论 / 审批）→ 按 external_topic 找例程、`submit_internal`。主人：EE11。
//!
//! CC2 预埋的桩：现在一律 `Pass`，不做 I/O、不计数、不打日志（替身的调用记录因此不变）。
//! 启用它**只改这个文件**；它在链上的位置见 `routing/mod.rs` 的头注释。
use aite_contracts::{IngressError, NormalizedEvent};

use crate::plane::InProcessControlPlane;
use crate::routing::Flow;

/// R3 之后、`R4_KINDS` 判定之前，对每个非卡片事件都调；桩内自己按 kind 过滤（`EventKind::External`）。
pub(crate) async fn hook(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
) -> Result<Flow, IngressError> {
    let _ = (plane, ev);
    Ok(Flow::Pass)
}
