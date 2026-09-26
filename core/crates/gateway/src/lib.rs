//! aite-gateway —— ToolGateway + 五个 Gateway 工具（对应旧 `aite/gateway/**` 与 `aite/tools/**`）。owner: R6
//!
//! 对外三件东西：
//! - `P0ToolGateway`（`impl aite_contracts::ToolGateway`）：RΩ 组装时按 §3.2 的构造表接
//! - `schema::validate_arguments`：按 `ToolSpec.parameters` 校验并补 default
//! - `tools::{ToolEnv, ToolOutcome, ToolFailure, ToolImpl}`：工具实现的座子（换实现用 `with_tool`）
//!
//! 工具目录是 §3.1 冻结的 `gateway_tools()` 原样，名字与 schema 不在这里定义。
mod gateway;
pub mod sandbox_key;
pub mod schema;
pub mod tools;

pub use gateway::{DEFAULT_RUN_PYTHON_GRACE_SEC, P0ToolGateway, TokenResolver};
pub use sandbox_key::SandboxKey;
pub use schema::{SchemaViolation, validate_arguments};
pub use tools::{ToolEnv, ToolFailure, ToolImpl, ToolOutcome};
