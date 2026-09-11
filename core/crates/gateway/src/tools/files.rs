//! `list_files`（对应旧 `aite/tools/files.py`）—— 列出沙箱 `/work` 下的文件。
//!
//! 刻意**不**建沙箱：模型经常在还没跑过任何代码时先问一句「有什么文件」，为这一问
//! 起一个容器纯属浪费。没有容器就等于 /work 是空的，直接如实回答。
use aite_contracts::ToolContext;
use serde_json::{Map, Value, json};

use super::{ToolEnv, ToolFailure, ToolOutcome, WORKDIR, as_map};

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

    let paths = sandbox
        .list_files(&sandbox_id)
        .await
        .map_err(|e| ToolFailure::sandbox(format!("列 {WORKDIR} 失败：{e}")))?;

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
