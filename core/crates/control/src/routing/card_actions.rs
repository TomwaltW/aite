//! 卡片回传的新动作（审批通过 / 驳回等）。主人：EE7（T0c 先加 log+drop 分支）。
//!
//! CC2 预埋的桩：现在一律 `Pass`，不做 I/O、不计数、不打日志。启用它**只改这个文件**。
use aite_contracts::{CardAction, IngressError, NormalizedEvent};

use crate::plane::InProcessControlPlane;
use crate::routing::Flow;

/// R3 里、现有 Stop / Evidence 分支之前。`Handled` = 这个动作归这里处理，R3 不再往下走。
pub(crate) async fn hook(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
    action: &CardAction,
) -> Result<Flow, IngressError> {
    let _ = (plane, ev, action);
    Ok(Flow::Pass)
}
