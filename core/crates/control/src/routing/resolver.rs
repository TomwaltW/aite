//! 追问归属解析（`#A` 号、引用、消息索引、同一发起人 30 分钟内）。主人：DD3。
//!
//! CC2 预埋的桩：`thread_session` 没命中时调，现在一律返回 `None`（= 今天的行为）。
use aite_contracts::{IngressError, NormalizedEvent, Session};

use crate::plane::InProcessControlPlane;

/// 话题查不到会话时的第二条路。`Some` = 这条消息归这个会话（接着走 R5 / R6）。
pub(crate) async fn resolve(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
) -> Result<Option<Session>, IngressError> {
    let _ = (plane, ev);
    Ok(None)
}
