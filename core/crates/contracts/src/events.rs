//! 入站事件归一化形状（对应旧 aite/contracts/events.py；跨进程形态见 proto/aite/v1/events.proto）。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($(#[$vmeta])* $variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s { $($s => Ok($name::$variant),)+ other => Err(format!("{}: 未知取值 {other:?}", stringify!($name))) }
            }
        }
    };
}
pub(crate) use str_enum;

str_enum! {
    /// 发送者类型。机器人/应用/系统消息永远不触发任务（R1）。
    SenderKind { Human => "human", Bot => "bot", App => "app", System => "system" }
}

str_enum! {
    EventKind {
        Message => "message",
        MessageEdited => "message_edited",
        MessageDeleted => "message_deleted",
        CardAction => "card_action",
        BotAdded => "bot_added",
        MemberChanged => "member_changed",
    }
}

str_enum! {
    ChatType { Group => "group", P2p => "p2p" }
}

str_enum! {
    AttachmentKind { Image => "image", File => "file" }
}

str_enum! {
    /// 卡片按钮 / 卡片回传动作；出站 ChecklistCard.actions 与入站 CardAction.action 共用。
    CardActionKind { Stop => "stop", Evidence => "evidence" }
}

/// 任务锚点：会话靠它绑定到平台线程。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub platform: String,
    pub chat_id: String,
    /// 触发消息 id
    pub message_id: String,
    /// 飞书：话题 root 消息 id；顶层消息则为 None
    #[serde(default)]
    pub thread_id: Option<String>,
    /// "#A17"，P0 只展示不路由
    #[serde(default)]
    pub task_no: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    /// 平台文件 key，配合 message_id 下载
    pub file_key: String,
    pub message_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default)]
    pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardAction {
    /// = 卡片所在消息的 message_id
    pub card_id: String,
    pub action: CardActionKind,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub value: Map<String, Value>,
}

fn default_tenant() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedEvent {
    /// 平台 message_id 或 event_id，去重键
    pub event_id: String,
    pub kind: EventKind,
    pub platform: String,
    #[serde(default = "default_tenant")]
    pub tenant_id: String,
    /// 飞书 app_id
    pub workspace_id: String,
    pub chat_id: String,
    pub chat_type: ChatType,
    /// 飞书 open_id
    pub sender_id: String,
    pub sender_kind: SenderKind,
    #[serde(default)]
    pub sender_name: Option<String>,
    /// 去掉 @Aite 后、strip 过的纯文本；非文本消息为 ""
    pub text: String,
    #[serde(default)]
    pub raw_text: Option<String>,
    /// 是否 @ 了 Aite
    pub mentioned: bool,
    pub anchor: Anchor,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub card_action: Option<CardAction>,
    pub occurred_at: DateTime<Utc>,
    /// 原始事件，仅供审计；任何逻辑不得依赖 raw
    #[serde(default)]
    pub raw: Map<String, Value>,
}
