//! `download_attachment`（对应旧 `aite/tools/attachments.py`）—— 把本次消息的附件下到沙箱 `/work/in/`。
//!
//! 两段两种错误码，别混（§3.3）：
//! - 从平台下载失败（含「压根没有附件上下文」）→ upstream。
//!   §3.3 原文：「群里发的附件 adapter 下载失败 → `download_attachment` 工具返回 upstream」。
//! - 下来了但写不进沙箱 → sandbox。
//!
//! 文件名只从 `file_key` 推。§3.1 冻结的 schema 里只有 `file_key` 一个参数，拿不到原始
//! 文件名；而 file_key 是平台给的不透明串，直接当路径用会带上 `/`、`..` 之类，
//! 所以先过一遍白名单清洗（`_safe_name` 五步，逐步对齐旧实现）。
use aite_contracts::ToolContext;
use serde_json::{Map, Value, json};

use super::{ToolEnv, ToolFailure, ToolOutcome, as_map, str_arg};

const MAX_NAME: usize = 120;
const INBOX: &str = "/work/in";

/// 只留字母数字、点、下划线、连字符和汉字，其余一律折成下划线
/// （旧实现的正则 `[^0-9A-Za-z._一-鿿-]+`，连续多个折成一个）。
fn safe_name(file_key: &str) -> String {
    // ① PurePosixPath(file_key).name（空则原串）
    let trimmed = file_key.trim_end_matches('/');
    let base = trimmed.rsplit('/').next().unwrap_or("");
    let base = if base.is_empty() { file_key } else { base };

    // ② 白名单之外折成 _
    let mut folded = String::with_capacity(base.len());
    let mut in_run = false;
    for ch in base.chars() {
        if is_safe_char(ch) {
            folded.push(ch);
            in_run = false;
        } else if !in_run {
            folded.push('_');
            in_run = true;
        }
    }

    // ③ 去掉首尾的 . 和 _
    let stripped = folded.trim_matches(|c| c == '.' || c == '_');

    // ④ 空则 attachment，再截到 120 个字符
    let name = if stripped.is_empty() {
        "attachment"
    } else {
        stripped
    };
    name.chars().take(MAX_NAME).collect()
}

fn is_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') || ('一'..='鿿').contains(&ch)
}

pub async fn download_attachment(
    env: ToolEnv,
    ctx: ToolContext,
    args: Map<String, Value>,
) -> Result<ToolOutcome, ToolFailure> {
    let platform = env.platform()?;
    let sandbox = env.sandbox()?;
    let file_key = str_arg(&args, "file_key").to_string();

    let Some(message_id) = ctx
        .attachments_message_id
        .as_deref()
        .filter(|m| !m.is_empty())
    else {
        return Err(ToolFailure::upstream(
            "本次消息没有附件（attachments_message_id 为空），没有东西可下载",
        ));
    };

    let blob = platform
        .download_file(message_id, &file_key)
        .await
        .map_err(|e| ToolFailure::upstream(format!("下载附件失败（{file_key}）：{e}")))?;

    // ⑤ /work/in/<清洗后的名字>
    let path = format!("{INBOX}/{}", safe_name(&file_key));
    let write = async {
        let sandbox_id = env.acquire_sandbox().await?;
        sandbox
            .put_file(&sandbox_id, &path, &blob)
            .await
            .map_err(|e| ToolFailure::sandbox(e.to_string()))
    };
    write
        .await
        .map_err(|e| ToolFailure::sandbox(format!("把附件写进沙箱失败（{path}）：{e}")))?;

    Ok(
        ToolOutcome::new(format!("附件已下载到沙箱：{path}（{} 字节）", blob.len())).with_data(
            as_map(json!({
                "path": path,
                "size": blob.len(),
                "file_key": file_key,
            })),
        ),
    )
}
