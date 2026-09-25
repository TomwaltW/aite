//! `submit_internal` 的落点：例程 / 跟进 / 环境观察 / fork / 外部事件 / 管理台发起的任务，
//! 走和用户消息同一条派发。主人：DD3（契约里的 `ControlPlane::submit_internal` 由 T0 加）。
//!
//! CC2 预埋的桩：不进路由链，今天没有调用点。
use aite_contracts::{IngressError, Task};

use crate::plane::InProcessControlPlane;

/// 在 `chat_id`（可选 `thread_id` 话题里）以 `text` 起一个任务。桩：什么都不做，返回 `None`。
#[allow(dead_code)] // 主人：DD3
pub(crate) async fn submit_internal(
    plane: &InProcessControlPlane,
    chat_id: &str,
    thread_id: Option<&str>,
    text: &str,
) -> Result<Option<Task>, IngressError> {
    let _ = (plane, chat_id, thread_id, text);
    Ok(None)
}
