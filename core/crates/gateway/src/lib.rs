//! aite-gateway —— ToolGateway + 五个 Gateway 工具（对应旧 `aite/gateway/**` 与 `aite/tools/**`）。owner: R6
//!
//! 对外的东西：
//! - `P0ToolGateway`（`impl aite_contracts::ToolGateway`）：RΩ 组装时按 §3.2 的构造表接；
//!   `GatewayBuilder` 是它的别名（`with_*` 按值链式，`app` 的 `gateway_options` 槽吃它）
//! - `schema::validate_arguments`：按 `ToolSpec.parameters` 校验并补 default
//! - `tools::{ToolEnv, ToolOutcome, ToolFailure, ToolImpl}`：工具实现的座子（换内建实现用 `with_tool`）
//! - `registry::{GatewayTool, ToolRegistry}`（CC4 ②）：外部 crate 实现工具、登记进网关
//! - `policy::{ToolPolicy, PolicyDecision}`（CC4 ③）：调用前策略钩子（token → 查工具 → 策略 → 校验参数 → 执行）
//! - `SandboxKey`（CC4 ①）：沙箱记账的键
//!
//! 内建的五个工具目录是 §3.1 冻结的 `gateway_tools()` 原样，名字与 schema 不在这里定义；
//! 登记的外部工具排在它们后面。
mod gateway;
pub mod policy;
pub mod registry;
pub mod sandbox_key;
pub mod schema;
pub mod tools;

pub use gateway::{DEFAULT_RUN_PYTHON_GRACE_SEC, P0ToolGateway, TokenResolver};
pub use policy::{AllowAll, DEFAULT_DENY_MESSAGE, PolicyDecision, ToolPolicy};
pub use registry::{GatewayTool, RegistryError, ToolRegistry};

/// `app` 的 `gateway_options` 槽吃的类型（CC4 ⑤）：就是网关本身，`with_*` 按值链式。
pub type GatewayBuilder = P0ToolGateway;
pub use sandbox_key::SandboxKey;
pub use schema::{SchemaViolation, validate_arguments};
pub use tools::{ToolEnv, ToolFailure, ToolImpl, ToolOutcome};
