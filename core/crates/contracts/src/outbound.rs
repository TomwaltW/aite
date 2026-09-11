//! 出站形状 + 平台读接口的返回形状（对应旧 aite/contracts/outbound.py 与 ports.py 里的
//! HistoryMessage / DocumentContent；跨进程形态见 proto/aite/v1/outbound.proto）。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::{CardActionKind, str_enum};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundText {
    pub chat_id: String,
    /// markdown（飞书 post/markdown 由 adapter 转）
    pub text: String,
    /// 回复哪条消息
    #[serde(default)]
    pub reply_to: Option<String>,
    /// reply_to 存在时是否进话题（默认 true）
    #[serde(default = "default_true")]
    pub in_thread: bool,
}

impl OutboundText {
    pub fn new(chat_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            chat_id: chat_id.into(),
            text: text.into(),
            reply_to: None,
            in_thread: true,
        }
    }
}

str_enum! {
    ChecklistState { Todo => "todo", Doing => "doing", Done => "done", Failed => "failed" }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItemView {
    pub id: String,
    pub text: String,
    pub state: ChecklistState,
    #[serde(default)]
    pub note: Option<String>,
}

str_enum! {
    CardStatus { Working => "working", Delivered => "delivered", Failed => "failed", Cancelled => "cancelled" }
}

fn default_actions() -> Vec<CardActionKind> {
    vec![CardActionKind::Stop]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistCard {
    pub task_id: String,
    /// "#A17"
    pub task_no: String,
    /// 任务一句话（≤40 字，worker 截断）
    pub title: String,
    /// 发起人显示名
    pub initiator: String,
    /// "9:02"
    pub started_at: String,
    pub status: CardStatus,
    #[serde(default)]
    pub items: Vec<ChecklistItemView>,
    /// "预计 2 分钟 · 已用 ¥0.12"
    #[serde(default)]
    pub footer: String,
    /// 默认 [stop]
    #[serde(default = "default_actions")]
    pub actions: Vec<CardActionKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundFile {
    pub chat_id: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendResult {
    pub message_id: String,
    /// 卡片消息的 id，后续 update_card 用
    #[serde(default)]
    pub card_id: Option<String>,
}

str_enum! {
    /// adapter 负责映射到平台 emoji（飞书：ack=OnIt, done=DONE, fail=CRY）
    ReactionKind { Ack => "ack", Done => "done", Fail => "fail" }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub message_id: String,
    pub sender_id: String,
    /// "human" | "bot" | "app" | "system"（沿用旧契约的字符串形态；Gateway 用 == "human" 过滤）
    pub sender_kind: String,
    #[serde(default)]
    pub sender_name: Option<String>,
    pub text: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentContent {
    pub title: String,
    /// markdown
    pub text: String,
    pub url: String,
}
