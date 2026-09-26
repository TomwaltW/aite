//! 用量与限额（CT11 / §3 D24）。主人：EE3。
//!
//! CC3 预埋的两个挂点，**在 `agent_loop` 里真调**：每步开头 `before_step`（`Some(文案)` = 以该文案
//! fail），每次 `chat` 之后 `after_model_call`。本轨恒 `None` / 空实现，不写任何预设或限额逻辑。
use aite_contracts::{Task, Usage};

/// 每步开头。`Some(文案)` = 以该文案判任务失败。
pub async fn before_step(task: &Task) -> Option<String> {
    let _ = task;
    None
}

/// 每次模型调用之后（记用量）。
pub async fn after_model_call(task: &Task, usage: &Usage) {
    let _ = (task, usage);
}
