//! 连接器与 `mcp_call`：按 scope 配置的外部连接（CT25）。
//!
//! 主人：FF8（W4）。CC4 ④ 预埋的空文件：**不进 `default_tools()`、不进目录**。
//! 主人轨在这里实现一个 `crate::registry::GatewayTool`，再在 `app/src/features/` 对应的文件里登记。
