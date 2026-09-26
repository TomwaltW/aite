//! 沙箱记账的键（CC4 ①）。
//!
//! P0 是「一个任务一个沙箱」，键就是 task_id。CT10「按话题一个沙箱」要换键 —— 那时 DD6 加
//! `Session` 变体 + builder 开关（`features/sandbox.rs` 推一条 `gateway_options`），调用点一个不改。
//!
//! **键只管 Gateway 自己的记账**（`Inner` 的两张表、`ToolEnv`）。传给 `SandboxPort::acquire` 的第一个
//! 参数仍然逐字是 task_id：edge 按它打 `aite.task` 标签，B8 07 的 `release min:1` 和真容器那组都看它。

/// Gateway 按什么给沙箱记账。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SandboxKey {
    /// 一个任务一个沙箱（P0 与今天唯一的变体）。
    Task(String),
}

impl SandboxKey {
    /// 这次调用该用哪个键。今天只有按任务。
    pub fn for_task(task_id: &str) -> Self {
        Self::Task(task_id.to_string())
    }
}
