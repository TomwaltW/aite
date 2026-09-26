//! 网关工具的插件注册表（CC4 ②，对标 Claude Tag 的 CT25 / CT16）。
//!
//! 外部 crate 实现 [`GatewayTool`]，登记进 [`ToolRegistry`]，再经
//! `P0ToolGateway::with_registry` 交给网关。`app` 那边在 `features/<x>.rs` 的 `wire` 里登记，
//! 出错走唯一的通道：`register` 当场回 `Err` → `wire` 用 `?` 往上抛 → `wire_all` 原样传出 →
//! `build_app_with_features` 映射成 `StartupError`（见 `app/src/features/mod.rs`）。
//!
//! 目录（`catalog(ctx)`）：P0 五个内建工具（`gateway_tools()` 原顺序，始终在前），
//! 后面接已登记且 `enabled(ctx)` 为真的（按登记顺序）。**没登记任何东西时逐字等于 `gateway_tools()`**。
//! `call` 查工具那一步与目录用同一个过滤：谓词为假的工具 → `not_found`。
//!
//! 实现的座子复用 `tools` 模块已经 `pub` 的 [`ToolEnv`] / [`ToolOutcome`] / [`ToolFailure`]，
//! 外部 crate 不需要任何新依赖就能实现。**`aite-gateway` 永远不许反过来依赖实现方**（否则成环）。
use std::fmt;
use std::sync::Arc;

use aite_contracts::ports::BoxFuture;
use aite_contracts::{ToolContext, ToolSpec, all_model_tools};
use serde_json::{Map, Value};

use crate::tools::{ToolEnv, ToolFailure, ToolOutcome};

/// 一个可以登记进网关的工具。
///
/// ```text
/// spec()     进模型目录的规格；arguments 按 spec().parameters 校验、补默认值之后才交给 call
/// enabled()  这次上下文里要不要露出来（默认恒真）；为假时目录里没有它、调用回 not_found
/// call()     干活；失败一律 Err(ToolFailure{code, message})，由网关翻成 ToolResult{ok:false}
/// ```
///
/// `call` 返回 `'static` 的 future：网关要把它放进 `tokio::time::timeout`（超时预算
/// `DEFAULT_TOOL_TIMEOUT_SEC`）与 panic 保护圈。
pub trait GatewayTool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn enabled(&self, ctx: &ToolContext) -> bool {
        let _ = ctx;
        true
    }

    fn call(
        &self,
        env: ToolEnv,
        ctx: ToolContext,
        args: Map<String, Value>,
    ) -> BoxFuture<'static, Result<ToolOutcome, ToolFailure>>;
}

/// 登记被拒（重名）。消息是人话，写清哪个名字撞了谁。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RegistryError(pub String);

/// `features/<x>.rs` 的 `wire` 返回 `Result<(), String>`，`?` 靠它。
impl From<RegistryError> for String {
    fn from(e: RegistryError) -> Self {
        e.0
    }
}

/// 已登记的外部工具，按登记顺序。
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn GatewayTool>>,
}

impl fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.names())
            .finish()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个工具。与已登记的重名、与 `all_model_tools()` 里 10 个名字（含本地工具
    /// `checklist_*` / `final`）重名 → **当场** `Err`。
    pub fn register(&mut self, tool: Arc<dyn GatewayTool>) -> Result<(), RegistryError> {
        let name = tool.spec().name;
        if all_model_tools().iter().any(|t| t.name == name) {
            return Err(RegistryError(format!(
                "工具 {name:?} 登记失败：与内建工具重名（all_model_tools() 里已经有这个名字）"
            )));
        }
        if self.tools.iter().any(|t| t.spec().name == name) {
            return Err(RegistryError(format!(
                "工具 {name:?} 登记失败：已经有一个同名的工具登记过了"
            )));
        }
        self.tools.push(tool);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// 已登记的名字，按登记顺序。
    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.spec().name).collect()
    }

    pub(crate) fn tools(&self) -> &[Arc<dyn GatewayTool>] {
        &self.tools
    }
}
