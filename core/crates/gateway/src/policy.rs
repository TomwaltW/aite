//! 调用前策略钩子（CC4 ③）。
//!
//! 位置固定在 `call` 的第三步：
//!
//! ```text
//! 校验 session_token → 查工具（含 enabled(ctx) 谓词）→ before_call → 校验 arguments → 执行
//! ```
//!
//! 这样 token 错与工具不存在的错误码不受策略影响（`tests/errors.rs` 的三条顺序测试）。
//! `Deny(reason)` → `ToolResult{ok:false, error.code = denied}`，message 用 reason（空串时给
//! [`DEFAULT_DENY_MESSAGE`]）。策略实现 panic 落进 `call` 的 `catch_unwind` 保护圈，回 upstream。
//!
//! 默认 [`AllowAll`]。DD6（W2）先把待审批的工具 Deny；EE7（W3）改成「暂停等审批」——
//! `before_call` 是 async，就是为了让它能在这里等。
use aite_contracts::{ToolCallRequest, ToolContext};
use async_trait::async_trait;

/// 策略说了什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    /// 拒绝，附带给模型看的理由
    Deny(String),
}

/// reason 为空时的那句话（`ToolResult` 的 message / content 不许空）。
pub const DEFAULT_DENY_MESSAGE: &str = "这次工具调用被策略拒绝了";

/// 调用前策略。
#[async_trait]
pub trait ToolPolicy: Send + Sync {
    async fn before_call(&self, ctx: &ToolContext, req: &ToolCallRequest) -> PolicyDecision;
}

/// 默认策略：一律放行（与 CC4 之前的行为逐字一致）。
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAll;

#[async_trait]
impl ToolPolicy for AllowAll {
    async fn before_call(&self, _ctx: &ToolContext, _req: &ToolCallRequest) -> PolicyDecision {
        PolicyDecision::Allow
    }
}
