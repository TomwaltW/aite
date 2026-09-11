//! domain ↔ pb 互转。方向约定：`From<domain> for pb` 永不失败；`TryFrom<pb> for domain` 可能失败
//! （枚举 Unspecified、必填子消息缺失、时间戳缺失）。
use aite_contracts::{
    Anchor, Attachment, AttachmentKind, CardAction, CardActionKind, CardStatus, ChatType,
    ChecklistCard, ChecklistItemView, ChecklistState, DocumentContent, EventKind, ExecLanguage,
    ExecRequest, ExecResult, FileEntry, HistoryMessage, NormalizedEvent, OutboundFile,
    OutboundText, PlatformCapabilities, ReactionKind, SandboxNetwork, SandboxSpec, SendResult,
    SenderKind,
};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Map, Number, Value};

use crate::pb;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    #[error("{field}: 枚举取值未指定（UNSPECIFIED）")]
    Unspecified { field: &'static str },
    #[error("{field}: 必填字段缺失")]
    Missing { field: &'static str },
    #[error("{field}: 取值非法：{value}")]
    Invalid { field: &'static str, value: String },
}

// ---------------------------------------------------------------- WKT

pub fn struct_to_map(s: Option<prost_types::Struct>) -> Map<String, Value> {
    s.map(|s| {
        s.fields
            .into_iter()
            .map(|(k, v)| (k, prost_value_to_json(v)))
            .collect()
    })
    .unwrap_or_default()
}

pub fn map_to_struct(m: &Map<String, Value>) -> prost_types::Struct {
    prost_types::Struct {
        fields: m
            .iter()
            .map(|(k, v)| (k.clone(), json_to_prost_value(v)))
            .collect(),
    }
}

pub fn prost_value_to_json(v: prost_types::Value) -> Value {
    use prost_types::value::Kind;
    match v.kind {
        None | Some(Kind::NullValue(_)) => Value::Null,
        Some(Kind::NumberValue(n)) => {
            if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
                Value::Number(Number::from(n as i64))
            } else {
                Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Some(Kind::StringValue(s)) => Value::String(s),
        Some(Kind::BoolValue(b)) => Value::Bool(b),
        Some(Kind::StructValue(s)) => Value::Object(struct_to_map(Some(s))),
        Some(Kind::ListValue(l)) => {
            Value::Array(l.values.into_iter().map(prost_value_to_json).collect())
        }
    }
}

pub fn json_to_prost_value(v: &Value) -> prost_types::Value {
    use prost_types::value::Kind;
    let kind = match v {
        Value::Null => Kind::NullValue(0),
        Value::Bool(b) => Kind::BoolValue(*b),
        Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => Kind::StringValue(s.clone()),
        Value::Array(items) => Kind::ListValue(prost_types::ListValue {
            values: items.iter().map(json_to_prost_value).collect(),
        }),
        Value::Object(m) => Kind::StructValue(map_to_struct(m)),
    };
    prost_types::Value { kind: Some(kind) }
}

pub fn ts_to_chrono(
    ts: Option<prost_types::Timestamp>,
    field: &'static str,
) -> Result<DateTime<Utc>, ConvertError> {
    let ts = ts.ok_or(ConvertError::Missing { field })?;
    Utc.timestamp_opt(ts.seconds, ts.nanos as u32)
        .single()
        .ok_or_else(|| ConvertError::Invalid {
            field,
            value: format!("{}.{}", ts.seconds, ts.nanos),
        })
}

pub fn chrono_to_ts(t: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: t.timestamp(),
        nanos: t.timestamp_subsec_nanos() as i32,
    }
}

// ---------------------------------------------------------------- enums

macro_rules! enum_pair {
    ($domain:ty, $pbty:ty, $field:literal, { $($d:ident <=> $p:ident),+ $(,)? }) => {
        impl From<$domain> for $pbty {
            fn from(v: $domain) -> Self { match v { $(<$domain>::$d => <$pbty>::$p),+ } }
        }
        impl TryFrom<$pbty> for $domain {
            type Error = ConvertError;
            fn try_from(v: $pbty) -> Result<Self, ConvertError> {
                match v { $(<$pbty>::$p => Ok(<$domain>::$d),)+ _ => Err(ConvertError::Unspecified { field: $field }) }
            }
        }
    };
}

enum_pair!(SenderKind, pb::SenderKind, "sender_kind", { Human <=> Human, Bot <=> Bot, App <=> App, System <=> System });
enum_pair!(EventKind, pb::EventKind, "kind", {
    Message <=> Message, MessageEdited <=> MessageEdited, MessageDeleted <=> MessageDeleted,
    CardAction <=> CardAction, BotAdded <=> BotAdded, MemberChanged <=> MemberChanged,
});
enum_pair!(ChatType, pb::ChatType, "chat_type", { Group <=> Group, P2p <=> P2p });
enum_pair!(AttachmentKind, pb::AttachmentKind, "attachments[].kind", { Image <=> Image, File <=> File });
enum_pair!(CardActionKind, pb::CardActionKind, "card_action.action", { Stop <=> Stop, Evidence <=> Evidence });
enum_pair!(ChecklistState, pb::ChecklistState, "items[].state", { Todo <=> Todo, Doing <=> Doing, Done <=> Done, Failed <=> Failed });
enum_pair!(CardStatus, pb::CardStatus, "status", { Working <=> Working, Delivered <=> Delivered, Failed <=> Failed, Cancelled <=> Cancelled });
enum_pair!(ReactionKind, pb::ReactionKind, "kind", { Ack <=> Ack, Done <=> Done, Fail <=> Fail });

fn enum_i32<D, P>(raw: i32, field: &'static str) -> Result<D, ConvertError>
where
    P: TryFrom<i32>,
    D: TryFrom<P, Error = ConvertError>,
{
    let p = P::try_from(raw).map_err(|_| ConvertError::Invalid {
        field,
        value: raw.to_string(),
    })?;
    D::try_from(p)
}

// ---------------------------------------------------------------- events

impl From<Anchor> for pb::Anchor {
    fn from(a: Anchor) -> Self {
        pb::Anchor {
            platform: a.platform,
            chat_id: a.chat_id,
            message_id: a.message_id,
            thread_id: a.thread_id,
            task_no: a.task_no,
        }
    }
}
impl From<pb::Anchor> for Anchor {
    fn from(a: pb::Anchor) -> Self {
        Anchor {
            platform: a.platform,
            chat_id: a.chat_id,
            message_id: a.message_id,
            thread_id: a.thread_id,
            task_no: a.task_no,
        }
    }
}

impl From<Attachment> for pb::Attachment {
    fn from(a: Attachment) -> Self {
        pb::Attachment {
            kind: pb::AttachmentKind::from(a.kind) as i32,
            file_key: a.file_key,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            mime: a.mime,
        }
    }
}
impl TryFrom<pb::Attachment> for Attachment {
    type Error = ConvertError;
    fn try_from(a: pb::Attachment) -> Result<Self, ConvertError> {
        Ok(Attachment {
            kind: enum_i32::<AttachmentKind, pb::AttachmentKind>(a.kind, "attachments[].kind")?,
            file_key: a.file_key,
            message_id: a.message_id,
            name: a.name,
            size: a.size,
            mime: a.mime,
        })
    }
}

impl From<CardAction> for pb::CardAction {
    fn from(c: CardAction) -> Self {
        pb::CardAction {
            card_id: c.card_id,
            action: pb::CardActionKind::from(c.action) as i32,
            task_id: c.task_id,
            value: Some(map_to_struct(&c.value)),
        }
    }
}
impl TryFrom<pb::CardAction> for CardAction {
    type Error = ConvertError;
    fn try_from(c: pb::CardAction) -> Result<Self, ConvertError> {
        Ok(CardAction {
            card_id: c.card_id,
            action: enum_i32::<CardActionKind, pb::CardActionKind>(c.action, "card_action.action")?,
            task_id: c.task_id,
            value: struct_to_map(c.value),
        })
    }
}

impl From<NormalizedEvent> for pb::NormalizedEvent {
    fn from(e: NormalizedEvent) -> Self {
        pb::NormalizedEvent {
            event_id: e.event_id,
            kind: pb::EventKind::from(e.kind) as i32,
            platform: e.platform,
            tenant_id: e.tenant_id,
            workspace_id: e.workspace_id,
            chat_id: e.chat_id,
            chat_type: pb::ChatType::from(e.chat_type) as i32,
            sender_id: e.sender_id,
            sender_kind: pb::SenderKind::from(e.sender_kind) as i32,
            sender_name: e.sender_name,
            text: e.text,
            raw_text: e.raw_text,
            mentioned: e.mentioned,
            anchor: Some(e.anchor.into()),
            attachments: e.attachments.into_iter().map(Into::into).collect(),
            card_action: e.card_action.map(Into::into),
            occurred_at: Some(chrono_to_ts(e.occurred_at)),
            raw: Some(map_to_struct(&e.raw)),
        }
    }
}
impl TryFrom<pb::NormalizedEvent> for NormalizedEvent {
    type Error = ConvertError;
    fn try_from(e: pb::NormalizedEvent) -> Result<Self, ConvertError> {
        Ok(NormalizedEvent {
            event_id: e.event_id,
            kind: enum_i32::<EventKind, pb::EventKind>(e.kind, "kind")?,
            platform: e.platform,
            tenant_id: if e.tenant_id.is_empty() {
                "default".to_string()
            } else {
                e.tenant_id
            },
            workspace_id: e.workspace_id,
            chat_id: e.chat_id,
            chat_type: enum_i32::<ChatType, pb::ChatType>(e.chat_type, "chat_type")?,
            sender_id: e.sender_id,
            sender_kind: enum_i32::<SenderKind, pb::SenderKind>(e.sender_kind, "sender_kind")?,
            sender_name: e.sender_name,
            text: e.text,
            raw_text: e.raw_text,
            mentioned: e.mentioned,
            anchor: e
                .anchor
                .ok_or(ConvertError::Missing { field: "anchor" })?
                .into(),
            attachments: e
                .attachments
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            card_action: e.card_action.map(TryInto::try_into).transpose()?,
            occurred_at: ts_to_chrono(e.occurred_at, "occurred_at")?,
            raw: struct_to_map(e.raw),
        })
    }
}

// ---------------------------------------------------------------- outbound

impl From<OutboundText> for pb::OutboundText {
    fn from(t: OutboundText) -> Self {
        pb::OutboundText {
            chat_id: t.chat_id,
            text: t.text,
            reply_to: t.reply_to,
            in_thread: t.in_thread,
        }
    }
}
impl From<pb::OutboundText> for OutboundText {
    fn from(t: pb::OutboundText) -> Self {
        OutboundText {
            chat_id: t.chat_id,
            text: t.text,
            reply_to: t.reply_to,
            in_thread: t.in_thread,
        }
    }
}

impl From<ChecklistItemView> for pb::ChecklistItemView {
    fn from(i: ChecklistItemView) -> Self {
        pb::ChecklistItemView {
            id: i.id,
            text: i.text,
            state: pb::ChecklistState::from(i.state) as i32,
            note: i.note,
        }
    }
}
impl TryFrom<pb::ChecklistItemView> for ChecklistItemView {
    type Error = ConvertError;
    fn try_from(i: pb::ChecklistItemView) -> Result<Self, ConvertError> {
        Ok(ChecklistItemView {
            id: i.id,
            text: i.text,
            state: enum_i32::<ChecklistState, pb::ChecklistState>(i.state, "items[].state")?,
            note: i.note,
        })
    }
}

impl From<ChecklistCard> for pb::ChecklistCard {
    fn from(c: ChecklistCard) -> Self {
        pb::ChecklistCard {
            task_id: c.task_id,
            task_no: c.task_no,
            title: c.title,
            initiator: c.initiator,
            started_at: c.started_at,
            status: pb::CardStatus::from(c.status) as i32,
            items: c.items.into_iter().map(Into::into).collect(),
            footer: c.footer,
            actions: c
                .actions
                .into_iter()
                .map(|a| pb::CardActionKind::from(a) as i32)
                .collect(),
        }
    }
}
impl TryFrom<pb::ChecklistCard> for ChecklistCard {
    type Error = ConvertError;
    fn try_from(c: pb::ChecklistCard) -> Result<Self, ConvertError> {
        Ok(ChecklistCard {
            task_id: c.task_id,
            task_no: c.task_no,
            title: c.title,
            initiator: c.initiator,
            started_at: c.started_at,
            status: enum_i32::<CardStatus, pb::CardStatus>(c.status, "status")?,
            items: c
                .items
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            footer: c.footer,
            actions: c
                .actions
                .into_iter()
                .map(|a| enum_i32::<CardActionKind, pb::CardActionKind>(a, "actions[]"))
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<OutboundFile> for pb::OutboundFile {
    fn from(f: OutboundFile) -> Self {
        pb::OutboundFile {
            chat_id: f.chat_id,
            reply_to: f.reply_to,
            name: f.name,
            mime: f.mime,
            data: f.data,
        }
    }
}
impl From<pb::OutboundFile> for OutboundFile {
    fn from(f: pb::OutboundFile) -> Self {
        OutboundFile {
            chat_id: f.chat_id,
            reply_to: f.reply_to,
            name: f.name,
            mime: f.mime,
            data: f.data,
        }
    }
}

impl From<SendResult> for pb::SendResult {
    fn from(r: SendResult) -> Self {
        pb::SendResult {
            message_id: r.message_id,
            card_id: r.card_id,
        }
    }
}
impl From<pb::SendResult> for SendResult {
    fn from(r: pb::SendResult) -> Self {
        SendResult {
            message_id: r.message_id,
            card_id: r.card_id,
        }
    }
}

impl From<HistoryMessage> for pb::HistoryMessage {
    fn from(h: HistoryMessage) -> Self {
        pb::HistoryMessage {
            message_id: h.message_id,
            sender_id: h.sender_id,
            sender_kind: h.sender_kind,
            sender_name: h.sender_name,
            text: h.text,
            thread_id: h.thread_id,
            created_at: Some(chrono_to_ts(h.created_at)),
        }
    }
}
impl TryFrom<pb::HistoryMessage> for HistoryMessage {
    type Error = ConvertError;
    fn try_from(h: pb::HistoryMessage) -> Result<Self, ConvertError> {
        Ok(HistoryMessage {
            message_id: h.message_id,
            sender_id: h.sender_id,
            sender_kind: h.sender_kind,
            sender_name: h.sender_name,
            text: h.text,
            thread_id: h.thread_id,
            created_at: ts_to_chrono(h.created_at, "created_at")?,
        })
    }
}

impl From<DocumentContent> for pb::DocumentContent {
    fn from(d: DocumentContent) -> Self {
        pb::DocumentContent {
            title: d.title,
            text: d.text,
            url: d.url,
        }
    }
}
impl From<pb::DocumentContent> for DocumentContent {
    fn from(d: pb::DocumentContent) -> Self {
        DocumentContent {
            title: d.title,
            text: d.text,
            url: d.url,
        }
    }
}

// ---------------------------------------------------------------- capabilities / sandbox

impl From<PlatformCapabilities> for pb::PlatformCapabilities {
    fn from(c: PlatformCapabilities) -> Self {
        pb::PlatformCapabilities {
            platform: c.platform,
            supports_thread: c.supports_thread,
            supports_history: c.supports_history,
            supports_passive_listen: c.supports_passive_listen,
            supports_card_edit: c.supports_card_edit,
            card_edit_window_sec: c.card_edit_window_sec as i32,
            inbound_file_in_group: c.inbound_file_in_group,
            proactive_requires_prior_message: c.proactive_requires_prior_message,
            outbound_rate_per_min: c.outbound_rate_per_min as i32,
        }
    }
}
impl From<pb::PlatformCapabilities> for PlatformCapabilities {
    fn from(c: pb::PlatformCapabilities) -> Self {
        PlatformCapabilities {
            platform: c.platform,
            supports_thread: c.supports_thread,
            supports_history: c.supports_history,
            supports_passive_listen: c.supports_passive_listen,
            supports_card_edit: c.supports_card_edit,
            card_edit_window_sec: c.card_edit_window_sec.max(0) as u32,
            inbound_file_in_group: c.inbound_file_in_group,
            proactive_requires_prior_message: c.proactive_requires_prior_message,
            outbound_rate_per_min: c.outbound_rate_per_min.max(0) as u32,
        }
    }
}

impl From<SandboxSpec> for pb::SandboxSpec {
    fn from(s: SandboxSpec) -> Self {
        pb::SandboxSpec {
            image: s.image,
            cpu: s.cpu,
            mem_mb: s.mem_mb as i32,
            network: s.network.as_str().to_string(),
            workdir: s.workdir,
        }
    }
}
impl TryFrom<pb::SandboxSpec> for SandboxSpec {
    type Error = ConvertError;
    fn try_from(s: pb::SandboxSpec) -> Result<Self, ConvertError> {
        let network = match s.network.as_str() {
            "" | "none" => SandboxNetwork::None,
            other => {
                return Err(ConvertError::Invalid {
                    field: "network",
                    value: other.to_string(),
                });
            }
        };
        Ok(SandboxSpec {
            image: s.image,
            cpu: if s.cpu > 0.0 { s.cpu } else { 1.0 },
            mem_mb: if s.mem_mb > 0 { s.mem_mb as u32 } else { 1024 },
            network,
            workdir: if s.workdir.is_empty() {
                "/work".to_string()
            } else {
                s.workdir
            },
        })
    }
}

impl From<ExecRequest> for pb::ExecRequest {
    fn from(r: ExecRequest) -> Self {
        pb::ExecRequest {
            language: r.language.as_str().to_string(),
            code: r.code,
            timeout_sec: r.timeout_sec as i32,
        }
    }
}
impl TryFrom<pb::ExecRequest> for ExecRequest {
    type Error = ConvertError;
    fn try_from(r: pb::ExecRequest) -> Result<Self, ConvertError> {
        let language = match r.language.as_str() {
            "" | "python" => ExecLanguage::Python,
            other => {
                return Err(ConvertError::Invalid {
                    field: "language",
                    value: other.to_string(),
                });
            }
        };
        Ok(ExecRequest {
            language,
            code: r.code,
            timeout_sec: if r.timeout_sec > 0 {
                r.timeout_sec as u32
            } else {
                120
            },
        })
    }
}

impl From<FileEntry> for pb::FileEntry {
    fn from(f: FileEntry) -> Self {
        pb::FileEntry {
            path: f.path,
            size: f.size,
        }
    }
}
impl From<pb::FileEntry> for FileEntry {
    fn from(f: pb::FileEntry) -> Self {
        FileEntry {
            path: f.path,
            size: f.size,
        }
    }
}

impl From<ExecResult> for pb::ExecResult {
    fn from(r: ExecResult) -> Self {
        pb::ExecResult {
            exit_code: r.exit_code,
            stdout: r.stdout,
            stderr: r.stderr,
            duration_ms: r.duration_ms as i64,
            truncated: r.truncated,
            files_out: r.files_out.into_iter().map(Into::into).collect(),
        }
    }
}
impl From<pb::ExecResult> for ExecResult {
    fn from(r: pb::ExecResult) -> Self {
        ExecResult {
            exit_code: r.exit_code,
            stdout: r.stdout,
            stderr: r.stderr,
            duration_ms: r.duration_ms.max(0) as u64,
            truncated: r.truncated,
            files_out: r.files_out.into_iter().map(Into::into).collect(),
        }
    }
}
