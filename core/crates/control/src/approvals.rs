//! 审批信箱的写入侧：卡片「批准 / 驳回」与 `!approve` / `!reject` 经这里把决定交给
//! DD3 在 `dispatch.rs` 里建的每任务信箱。主人：EE7。
//!
//! CC2 预埋的桩：不进路由链，今天没有调用点。
use aite_contracts::{IngressError, NormalizedEvent};

use crate::plane::InProcessControlPlane;

/// 对 `task_no` 下一个审批决定（`approve` 为真 = 批准）。桩：什么都不做。
#[allow(dead_code)] // 主人：EE7
pub(crate) async fn decide(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
    task_no: &str,
    approve: bool,
) -> Result<(), IngressError> {
    let _ = (plane, ev, task_no, approve);
    Ok(())
}
