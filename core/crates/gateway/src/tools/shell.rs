//! `run_shell`：在沙箱里跑一段 shell（`ExecLanguage::Bash` 要等 T0 加进契约）。
//!
//! 主人：DD6（W2）。CC4 ④ 预埋的空文件：**不进 `default_tools()`、不进目录**。
//! 主人轨在这里实现一个 `crate::registry::GatewayTool`，再在 `app/src/features/` 对应的文件里登记。
