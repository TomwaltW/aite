//! `read_document`（对应旧 `aite/tools/documents.py`）—— 读一篇飞书云文档，返回 markdown 文本。
//!
//! 真正把文档转成 markdown 的是 `PlatformPort::read_document`（edge 侧的 adapter）；
//! 这一层只负责把它包成给模型看的文本，并把平台侧的任何失败翻成 upstream（§3.3）。
use aite_contracts::ToolContext;
use serde_json::{Map, Value, json};

use super::{ToolEnv, ToolFailure, ToolOutcome, as_map, str_arg};

pub async fn read_document(
    env: ToolEnv,
    _ctx: ToolContext,
    args: Map<String, Value>,
) -> Result<ToolOutcome, ToolFailure> {
    let platform = env.platform()?;
    let reference = str_arg(&args, "url_or_token").to_string();

    let doc = platform
        .read_document(&reference)
        .await
        .map_err(|e| ToolFailure::upstream(format!("读取文档失败（{reference}）：{e}")))?;

    let content = format!("# {}\n\n来源：{}\n\n{}", doc.title, doc.url, doc.text);
    Ok(ToolOutcome::new(content).with_data(as_map(json!({
        "title": doc.title,
        "url": doc.url,
        "chars": doc.text.chars().count(),
    }))))
}
