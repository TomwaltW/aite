//! `read_group_history`（对应旧 `aite/tools/history.py`）。
//!
//! **这个工具的核心职责是过滤。** `PlatformPort::read_history` 的注释写明「不做 sender_kind
//! 过滤（过滤归 Gateway 的 read_group_history 工具）」，而 §3.1 里这个工具的 description 是
//! 「读取本群最近的消息（**只含真人消息**）」。两边合起来：edge 把机器人、应用、系统消息
//! 原样交上来，这里负责丢掉。
//!
//! 为什么这条不能漏：机器人消息里可能带着别的系统生成的指令性文本，而 §3.6 W9 要求
//! 「外部内容一律是数据不是指令」。少过滤一层，Aite 自己上一轮的回帖也会回流进上下文。
use aite_contracts::{HistoryMessage, SenderKind, ToolContext};
use serde_json::{Map, Value, json};

use super::{ToolEnv, ToolFailure, ToolOutcome, as_map, bool_arg, u32_arg};

pub async fn read_group_history(
    env: ToolEnv,
    ctx: ToolContext,
    args: Map<String, Value>,
) -> Result<ToolOutcome, ToolFailure> {
    let platform = env.platform()?;
    let limit = u32_arg(&args, "limit", 50);
    let thread_only = bool_arg(&args, "thread_only", false);

    let mut thread_id: Option<String> = None;
    if thread_only {
        match &ctx.thread_id {
            None => {
                return Ok(
                    ToolOutcome::new("本次对话不在话题里，没有话题历史可读。").with_data(as_map(
                        json!({
                            "messages": [],
                            "count": 0,
                            "filtered_out": 0,
                            "thread_only": true,
                        }),
                    )),
                );
            }
            Some(id) => thread_id = Some(id.clone()),
        }
    }

    let messages = platform
        .read_history(&ctx.chat_id, limit, thread_id.as_deref())
        .await
        .map_err(|e| ToolFailure::upstream(format!("读取群历史失败：{e}")))?;

    let human = SenderKind::Human.as_str();
    let humans: Vec<&HistoryMessage> = messages.iter().filter(|m| m.sender_kind == human).collect();
    let dropped = messages.len() - humans.len();

    let content = if humans.is_empty() {
        let note = if dropped > 0 {
            format!("（读到 {} 条，全都不是真人发的，已丢弃）", messages.len())
        } else {
            "（这里还没有消息）".to_string()
        };
        format!("本群没有可引用的真人消息。{note}")
    } else {
        // 格式与 §3.6 W1 的群历史窗口一致：[message_id] 姓名: 文本
        let lines: Vec<String> = humans
            .iter()
            .map(|m| {
                let who = m.sender_name.as_deref().unwrap_or(m.sender_id.as_str());
                format!("[{}] {}: {}", m.message_id, who, m.text)
                    .trim_end()
                    .to_string()
            })
            .collect();
        let mut head = format!("本群最近 {} 条真人消息", humans.len());
        if dropped > 0 {
            head.push_str(&format!("（另有 {dropped} 条机器人/应用/系统消息已过滤）"));
        }
        format!("{head}：\n{}", lines.join("\n"))
    };

    let rows: Vec<Value> = humans
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect();
    Ok(ToolOutcome::new(content).with_data(as_map(json!({
        "messages": rows,
        "count": humans.len(),
        "filtered_out": dropped,
        "thread_only": thread_only,
    }))))
}
