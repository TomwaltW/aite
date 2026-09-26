//! `describe_access`：告诉模型本次上下文里它能用哪些工具、为什么（CT16 Access Bundle）。
//!
//! 主人：DD6（W2）。CC4 ④ 预埋的空文件：**不进 `default_tools()`、不进目录**。
//! 主人轨在这里实现一个 `crate::registry::GatewayTool`，再在 `app/src/features/` 对应的文件里登记。
