//! `list_files`（对应旧 `aite/tools/files.py`）—— 列出沙箱 `/work` 下的文件。
//!
//! 刻意**不**建沙箱：模型经常在还没跑过任何代码时先问一句「有什么文件」，为这一问
//! 起一个容器纯属浪费。没有容器就等于 /work 是空的，直接如实回答。
//!
//! **容器被 reaper 收走之后走的也是这条路**：记账里还留着死 id，`list_files` 拿它去打
//! 会换回一条 `NotFound`。那时该做的还是「如实说 /work 空了」，而不是报沙箱失败 ——
//! 后者会把这一问算进 §3.3 的连续沙箱失败计数，两问就把任务打死。摘掉记账即可，
//! 重建留给下一次 `run_python`（上面那条「不为列目录起容器」照样成立）。
use aite_contracts::{SandboxErrorKind, ToolContext};
use serde_json::{Map, Value, json};

use super::{SANDBOX_REAPED_NOTE, ToolEnv, ToolFailure, ToolOutcome, WORKDIR, as_map};

pub async fn list_files(
    env: ToolEnv,
    _ctx: ToolContext,
    _args: Map<String, Value>,
) -> Result<ToolOutcome, ToolFailure> {
    let sandbox = env.sandbox()?;

    let Some(sandbox_id) = env.current_sandbox_id() else {
        return Ok(
            ToolOutcome::new(format!("沙箱还没启动，{WORKDIR} 下还没有文件。"))
                .with_data(as_map(json!({"files": [], "count": 0}))),
        );
    };

    let paths = match sandbox.list_files(&sandbox_id).await {
        Ok(paths) => paths,
        Err(e) if e.kind == SandboxErrorKind::NotFound => {
            tracing::warn!(
                sandbox_id = %sandbox_id,
                error = %e,
                "gateway.sandbox_gone 容器已不在，{WORKDIR} 按空目录回答"
            );
            env.forget_sandbox();
            return Ok(ToolOutcome::new(SANDBOX_REAPED_NOTE)
                .with_data(as_map(json!({"files": [], "count": 0}))));
        }
        Err(e) => return Err(ToolFailure::sandbox(format!("列 {WORKDIR} 失败：{e}"))),
    };

    if paths.is_empty() {
        return Ok(ToolOutcome::new(format!("{WORKDIR} 下还没有文件。"))
            .with_data(as_map(json!({"files": [], "count": 0}))));
    }

    Ok(ToolOutcome::new(format!(
        "{WORKDIR} 下有 {} 个文件：\n{}",
        paths.len(),
        paths.join("\n")
    ))
    .with_data(as_map(json!({"files": paths, "count": paths.len()}))))
}
