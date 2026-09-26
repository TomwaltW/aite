//! 附件清单（W1）。CC3 从 `context.rs` 原样搬来。
use aite_contracts::{Attachment, Message, Role};

pub const ATTACHMENT_HEADER: &str =
    "本次消息带了以下附件（尚未下载，需要时调用 download_attachment）：";

/// 附件清单：名字 / 大小 / file_key，不下载。
pub fn attachments_message(attachments: &[Attachment]) -> Option<Message> {
    if attachments.is_empty() {
        return None;
    }
    let lines: Vec<String> = attachments
        .iter()
        .map(|a| {
            let size = match a.size {
                Some(n) => format!("{n} 字节"),
                None => "大小未知".to_string(),
            };
            let name = a
                .name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(&a.file_key);
            format!(
                "- {}（{}，{}，file_key={}）",
                name, a.kind, size, a.file_key
            )
        })
        .collect();
    Some(Message::text(
        Role::System,
        format!("{ATTACHMENT_HEADER}\n{}", lines.join("\n")),
    ))
}
