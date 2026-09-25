//! request_approval：请求审批并等决定。主人：EE7。
//!
//! CC3 预埋的桩：`ENABLED = false` —— 不进目录、名字不进本地路由集合（被模型点名时照旧送 gateway、
//! 回 NotFound）。启用它**只改这个文件**：把 `ENABLED` 改成 `true`、写 `spec()` 与 `run()`。
use aite_contracts::{ToolCallRequest, ToolErrorCode, ToolSpec};
use serde_json::json;

use crate::r#loop::{AgentWorker, RunContext, ToolOutcome};
use crate::{RunError, texts};

pub const ENABLED: bool = false;
pub const NAME: &str = "request_approval";

/// 进模型目录的 spec（只在 `ENABLED` 时被取）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: NAME.to_string(),
        description: String::new(),
        parameters: json!({"type": "object", "properties": {}}),
    }
}

/// 执行（只在 `ENABLED` 时被调）。桩：回「未知工具」。
pub(crate) async fn run(
    worker: &AgentWorker,
    ctx: &mut RunContext,
    call: &ToolCallRequest,
) -> Result<ToolOutcome, RunError> {
    let content = texts::unknown_tool(&call.name);
    worker
        .local_result(ctx, call, false, &content, Some(ToolErrorCode::NotFound))
        .await
}
