//! `event_received` 证据的载荷助手（`route` 常量与两种 payload）。CC2 从 `plane.rs` 原样搬来。
use serde_json::{Map, Value};

use aite_contracts::NormalizedEvent;

use crate::card::{MAX_TITLE_CHARS, clip};

/// `event_received` 的 `route`：这条事件是**建了任务**的那一条（R6 新建 / R7）。
pub const ROUTE_NEW_TASK: &str = "new_task";
/// `event_received` 的 `route`：这条是**任务跑到一半**排进来的追问（R6 steer）。
pub const ROUTE_STEER: &str = "steer";
/// `event_received` 的 `route`：一条命令作用到了这个任务上（CC2 ⑨，另带 `command` 键）。
pub const ROUTE_COMMAND: &str = "command";

/// `event_received` 的 payload。
///
/// `EvidenceKind` 是冻结契约，加不了新 kind，所以「建任务的那条事件」和「中途追问的那条」
/// 共用 `event_received`。共用就得分得开 —— 不然时间线上两条长得一模一样，读的人分不出
/// 哪条是任务的起点、哪条是用户改了主意。分辨靠 `route`：两个写入点都带。
pub(crate) fn event_payload(
    ev: &NormalizedEvent,
    route: &str,
    text: Option<String>,
) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("event_id".into(), Value::String(ev.event_id.clone()));
    p.insert("kind".into(), Value::String(ev.kind.as_str().into()));
    p.insert("chat_id".into(), Value::String(ev.chat_id.clone()));
    p.insert("sender_id".into(), Value::String(ev.sender_id.clone()));
    p.insert(
        "message_id".into(),
        Value::String(ev.anchor.message_id.clone()),
    );
    p.insert("mentioned".into(), Value::Bool(ev.mentioned));
    p.insert("route".into(), Value::String(route.into()));
    if let Some(text) = text {
        p.insert("text".into(), Value::String(text));
    }
    p
}

/// 追问那条额外带上用户说的话（截断口径同卡片标题）。
///
/// 为什么这一条要带 `text` 而建任务那条不带：建任务那条紧挨着的 `task_created` 已经把
/// 用户原话截进 `title` 了，再抄一遍是噪音；追问这条旁边什么都没有，不带的话
/// `model_call` 只留 `messages_hash`（W8：模型消息全文不进证据），从证据里反推不出
/// 「用户中途把要求改成了什么」。
pub(crate) fn steer_payload(ev: &NormalizedEvent) -> Map<String, Value> {
    event_payload(ev, ROUTE_STEER, Some(clip(&ev.text, MAX_TITLE_CHARS)))
}
