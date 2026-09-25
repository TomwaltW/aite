//! 本地工具：checklist_* 与 final 由 worker 自己处理，不经 Gateway（W2）。
//!
//! CC3 起每个本地工具一个文件，各带 `pub const ENABLED: bool`。`checklist` / `final` 恒启用；
//! 预埋的两个桩（`post_update` → DD4、`request_approval` → EE7）`ENABLED = false` 时**不进目录、
//! 名字也不进本地路由集合** —— 被模型点名时照旧送 gateway、回 NotFound，与 CC3 之前逐字节一致。
//! 启用一个桩**只改它自己的文件**：本文件按 `ENABLED` 汇总额外本地工具的名字与 spec。
use aite_contracts::{EvidenceKind, ToolCallRequest, ToolErrorCode, ToolSpec};
use serde_json::json;

use crate::r#loop::{AgentWorker, RunContext, ToolOutcome};
use crate::{RunError, texts};

pub mod checklist;
#[path = "final.rs"]
pub mod r#final;
pub mod post_update;
pub mod request_approval;

pub(crate) use r#final::{is_truthy, parse_final, plain_string};

/// 启用的**额外**本地工具（契约里 `is_local_tool` 之外的）名字。今天为空。
pub fn enabled_names() -> Vec<&'static str> {
    let mut out = Vec::new();
    if post_update::ENABLED {
        out.push(post_update::NAME);
    }
    if request_approval::ENABLED {
        out.push(request_approval::NAME);
    }
    out
}

/// 启用的额外本地工具的 spec（进模型目录，排在 final 之后、gateway 目录之前）。今天为空。
pub fn enabled_specs() -> Vec<ToolSpec> {
    let mut out = Vec::new();
    if post_update::ENABLED {
        out.push(post_update::spec());
    }
    if request_approval::ENABLED {
        out.push(request_approval::spec());
    }
    out
}

/// 这个名字由 worker 本地处理吗（契约的本地工具 + 启用的额外本地工具）。
pub fn is_local(name: &str) -> bool {
    aite_contracts::is_local_tool(name) || enabled_names().contains(&name)
}

impl AgentWorker {
    pub(crate) async fn run_local_tool(
        &self,
        ctx: &mut RunContext,
        call: &ToolCallRequest,
    ) -> Result<ToolOutcome, RunError> {
        let args = &call.arguments;
        self.append_evidence(
            &ctx.task.id,
            EvidenceKind::ToolCall,
            json!({"call_id": call.call_id, "name": call.name, "arguments": crate::redact::redact_args(args)}),
        )
        .await?;
        match call.name.as_str() {
            "checklist_add" | "checklist_check" | "checklist_fail" | "checklist_note" => {
                self.run_checklist(ctx, call).await
            }
            name if post_update::ENABLED && name == post_update::NAME => {
                post_update::run(self, ctx, call).await
            }
            name if request_approval::ENABLED && name == request_approval::NAME => {
                request_approval::run(self, ctx, call).await
            }
            other => {
                let content = texts::unknown_tool(other);
                self.local_result(ctx, call, false, &content, Some(ToolErrorCode::NotFound))
                    .await
            }
        }
    }
}
