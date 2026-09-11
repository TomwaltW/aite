//! FakePlatform —— `PlatformPort` 的官方替身（对应旧 `aite/testing/fake_platform.py`）。
//!
//! 记账是它的主业：send_card / update_card / send_text / send_file / add_reaction 的
//! 调用次数与参数全进 CallLog，评测场景的断言都靠它。
//!
//! 三条刻意做严的地方：
//!
//! * `update_card` 只认已经存在的 card_id。发第二条卡片再更新、或者拿消息 id 当卡片 id
//!   传进来，都会直接报错 —— 03 要验的正是「原地更新，不新发消息」，这种错必须在替身
//!   这一层就炸掉，而不是等断言绕着弯地发现。
//! * `read_history` **不做** sender_kind 过滤（契约注释写死：过滤归 Gateway 的
//!   read_group_history）。05_history_summary 靠这条才验得出 Gateway 有没有过滤。
//! * 查不到的文档 / 附件一律报错并列出已备的键。
use std::collections::BTreeMap;
use std::sync::Mutex;

use aite_contracts::{
    ChecklistCard, DocumentContent, EventHandler, HistoryMessage, NormalizedEvent, OutboundFile,
    OutboundText, PlatformCapabilities, PlatformError, PlatformPort, ReactionKind, SendResult,
};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;

use crate::kwargs;
use crate::recorder::CallLog;

/// fake 平台的能力位。除了「限速给大」与 `supports_passive_listen=true` 以外与 FEISHU_P0 一致
/// —— 事件由测试直接注入，不存在「收不到」这回事。
pub fn fake_p0() -> PlatformCapabilities {
    PlatformCapabilities {
        platform: "fake".to_string(),
        supports_thread: true,
        supports_history: true,
        supports_passive_listen: true,
        supports_card_edit: true,
        card_edit_window_sec: 1_209_600,
        inbound_file_in_group: true,
        proactive_requires_prior_message: false,
        outbound_rate_per_min: 6000,
    }
}

/// 替身自己判定「这个调用姿势不对」时报的错，用来把违规钉在现场。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FakePlatformError(pub String);

impl From<FakePlatformError> for PlatformError {
    fn from(e: FakePlatformError) -> Self {
        PlatformError::new("fake_platform", e.0, false)
    }
}

#[derive(Default)]
struct State {
    history: Vec<HistoryMessage>,
    documents: BTreeMap<String, DocumentContent>,
    files: BTreeMap<(String, String), Vec<u8>>,
    sent_texts: Vec<OutboundText>,
    sent_files: Vec<OutboundFile>,
    reactions: Vec<(String, ReactionKind)>,
    /// card_id -> 该卡片的全部快照（[0] 是 send_card 那次，其后每次 update_card 追加一份）
    cards: BTreeMap<String, Vec<ChecklistCard>>,
    /// 卡片发出的先后（`card_snapshots()` 要按这个顺序摊平）
    card_order: Vec<String>,
    /// 方法名 -> 下一次调用要报的错（报完即清），用来演失败面
    fail_next: BTreeMap<String, String>,
    next_id: u64,
    started: bool,
    stopped: bool,
    on_event: Option<EventHandler>,
}

/// `PlatformPort` 的替身。构造后可用 `with_history / with_documents / with_files` 喂数据。
pub struct FakePlatform {
    pub calls: CallLog,
    state: Mutex<State>,
}

impl Default for FakePlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl FakePlatform {
    pub fn new() -> Self {
        Self {
            calls: CallLog::new(),
            state: Mutex::new(State {
                next_id: 1,
                ..State::default()
            }),
        }
    }

    pub fn with_history(self, history: Vec<HistoryMessage>) -> Self {
        self.state.lock().expect("FakePlatform 锁").history = history;
        self
    }

    pub fn with_documents(self, documents: BTreeMap<String, DocumentContent>) -> Self {
        self.state.lock().expect("FakePlatform 锁").documents = documents;
        self
    }

    pub fn with_files(self, files: BTreeMap<(String, String), Vec<u8>>) -> Self {
        self.state.lock().expect("FakePlatform 锁").files = files;
        self
    }

    /// 下一次调 `method` 时报这个错（报完即清）。
    pub fn fail_next(&self, method: &str, message: impl Into<String>) {
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .fail_next
            .insert(method.to_string(), message.into());
    }

    fn take_fail(&self, method: &str) -> Result<(), PlatformError> {
        let taken = self
            .state
            .lock()
            .expect("FakePlatform 锁")
            .fail_next
            .remove(method);
        match taken {
            None => Ok(()),
            Some(msg) => Err(PlatformError::new("fake_fail_next", msg, false)),
        }
    }

    /// 假 id：msg / card / file **共用一个计数器**（清单 §11 第 23 条）。
    fn next_id(&self, prefix: &str) -> String {
        let mut state = self.state.lock().expect("FakePlatform 锁");
        let n = state.next_id;
        state.next_id += 1;
        format!("{prefix}-{n}")
    }

    // ---- 事件注入（测试侧用，不属于 PlatformPort）------------------------

    /// 把一个归一化事件投给已注册的 on_event —— 相当于平台推了一条消息过来。
    pub async fn emit(&self, ev: &NormalizedEvent) -> Result<(), FakePlatformError> {
        let handler = {
            let state = self.state.lock().expect("FakePlatform 锁");
            state.on_event.clone()
        };
        let Some(handler) = handler else {
            return Err(FakePlatformError(
                "还没有 start()，没有 on_event 可以投递".to_string(),
            ));
        };
        self.calls.record(
            "emit",
            kwargs! {"event_id" => json!(ev.event_id), "kind" => json!(ev.kind.as_str())},
        );
        handler(ev.clone())
            .await
            .map_err(|e| FakePlatformError(e.to_string()))
    }

    // ---- 给断言用的便捷视图 ---------------------------------------------

    pub fn count(&self, method: &str) -> usize {
        self.calls.count(method)
    }

    /// 发出过几张卡片（不是更新了几次）。
    pub fn card_count(&self) -> usize {
        self.state.lock().expect("FakePlatform 锁").cards.len()
    }

    pub fn update_count(&self) -> usize {
        self.calls.count("update_card")
    }

    /// 所有出站动作之和 —— 09_bot_ignored 要它等于 0。
    pub fn outbound_count(&self) -> usize {
        [
            "send_text",
            "send_card",
            "update_card",
            "send_file",
            "add_reaction",
        ]
        .iter()
        .map(|m| self.calls.count(m))
        .sum()
    }

    pub fn texts(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .sent_texts
            .iter()
            .map(|m| m.text.clone())
            .collect()
    }

    pub fn sent_files(&self) -> Vec<OutboundFile> {
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .sent_files
            .clone()
    }

    pub fn reactions(&self) -> Vec<(String, ReactionKind)> {
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .reactions
            .clone()
    }

    /// 某张卡片的全部快照；不给 card_id 就按卡片发出的顺序摊平。
    pub fn card_snapshots(&self, card_id: Option<&str>) -> Vec<ChecklistCard> {
        let state = self.state.lock().expect("FakePlatform 锁");
        match card_id {
            Some(id) => state.cards.get(id).cloned().unwrap_or_default(),
            None => state
                .card_order
                .iter()
                .filter_map(|id| state.cards.get(id))
                .flat_map(|snaps| snaps.iter().cloned())
                .collect(),
        }
    }

    pub fn card_ids(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .cards
            .keys()
            .cloned()
            .collect()
    }

    pub fn started(&self) -> bool {
        self.state.lock().expect("FakePlatform 锁").started
    }

    pub fn stopped(&self) -> bool {
        self.state.lock().expect("FakePlatform 锁").stopped
    }
}

#[async_trait]
impl PlatformPort for FakePlatform {
    fn capabilities(&self) -> PlatformCapabilities {
        fake_p0()
    }

    async fn start(&self, on_event: EventHandler) -> Result<(), PlatformError> {
        self.calls.record("start", kwargs! {});
        let mut state = self.state.lock().expect("FakePlatform 锁");
        state.on_event = Some(on_event);
        state.started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<(), PlatformError> {
        self.calls.record("stop", kwargs! {});
        self.state.lock().expect("FakePlatform 锁").stopped = true;
        Ok(())
    }

    async fn send_text(&self, msg: &OutboundText) -> Result<SendResult, PlatformError> {
        let idx = self.calls.record(
            "send_text",
            kwargs! {
                "chat_id" => json!(msg.chat_id),
                "reply_to" => json!(msg.reply_to),
                "in_thread" => json!(msg.in_thread),
                "text" => json!(msg.text),
            },
        );
        self.take_fail("send_text")?;
        let message_id = self.next_id("msg");
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .sent_texts
            .push(msg.clone());
        self.calls.set_result(idx, json!(message_id));
        Ok(SendResult {
            message_id,
            card_id: None,
        })
    }

    async fn send_card(
        &self,
        chat_id: &str,
        reply_to: Option<&str>,
        card: &ChecklistCard,
    ) -> Result<SendResult, PlatformError> {
        let idx = self.calls.record(
            "send_card",
            kwargs! {
                "chat_id" => json!(chat_id),
                "reply_to" => json!(reply_to),
                "task_id" => json!(card.task_id),
                "status" => json!(card.status.as_str()),
            },
        );
        self.take_fail("send_card")?;
        let card_id = self.next_id("card");
        {
            let mut state = self.state.lock().expect("FakePlatform 锁");
            state.cards.insert(card_id.clone(), vec![card.clone()]);
            state.card_order.push(card_id.clone());
        }
        self.calls.set_result(idx, json!(card_id));
        // send_card 返回的 message_id == card_id（后续 update_card 认的就是它）
        Ok(SendResult {
            message_id: card_id.clone(),
            card_id: Some(card_id),
        })
    }

    async fn update_card(&self, card_id: &str, card: &ChecklistCard) -> Result<(), PlatformError> {
        let idx = self.calls.record(
            "update_card",
            kwargs! {
                "card_id" => json!(card_id),
                "task_id" => json!(card.task_id),
                "status" => json!(card.status.as_str()),
            },
        );
        {
            let state = self.state.lock().expect("FakePlatform 锁");
            if !state.cards.contains_key(card_id) {
                let known: Vec<String> = state.cards.keys().cloned().collect();
                drop(state);
                self.calls.set_error(idx, "unknown card_id");
                return Err(FakePlatformError(format!(
                    "update_card 的 card_id={card_id:?} 不存在。已发出的卡片：{known:?}。\
                     卡片必须原地更新，不能新发消息。"
                ))
                .into());
            }
        }
        self.take_fail("update_card")?;
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .cards
            .entry(card_id.to_string())
            .or_default()
            .push(card.clone());
        Ok(())
    }

    async fn send_file(&self, msg: &OutboundFile) -> Result<SendResult, PlatformError> {
        let idx = self.calls.record(
            "send_file",
            kwargs! {
                "chat_id" => json!(msg.chat_id),
                "reply_to" => json!(msg.reply_to),
                "name" => json!(msg.name),
                "mime" => json!(msg.mime),
                "size" => json!(msg.data.len()),
            },
        );
        self.take_fail("send_file")?;
        let message_id = self.next_id("file");
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .sent_files
            .push(msg.clone());
        self.calls.set_result(idx, json!(message_id));
        Ok(SendResult {
            message_id,
            card_id: None,
        })
    }

    async fn add_reaction(
        &self,
        message_id: &str,
        kind: ReactionKind,
    ) -> Result<(), PlatformError> {
        self.calls.record(
            "add_reaction",
            kwargs! {"message_id" => json!(message_id), "kind" => json!(kind.as_str())},
        );
        self.take_fail("add_reaction")?;
        self.state
            .lock()
            .expect("FakePlatform 锁")
            .reactions
            .push((message_id.to_string(), kind));
        Ok(())
    }

    async fn read_history(
        &self,
        chat_id: &str,
        limit: u32,
        thread_id: Option<&str>,
    ) -> Result<Vec<HistoryMessage>, PlatformError> {
        let idx = self.calls.record(
            "read_history",
            kwargs! {
                "chat_id" => json!(chat_id),
                "limit" => json!(limit),
                "thread_id" => json!(thread_id),
            },
        );
        self.take_fail("read_history")?;
        let mut rows: Vec<HistoryMessage> = {
            let state = self.state.lock().expect("FakePlatform 锁");
            state
                .history
                .iter()
                .filter(|h| match thread_id {
                    None => true,
                    Some(tid) => h.thread_id.as_deref() == Some(tid),
                })
                .cloned()
                .collect()
        };
        rows.sort_by_key(|h| h.created_at); // 契约要求时间正序
        let start = rows.len().saturating_sub(limit as usize);
        let rows = rows.split_off(start);
        self.calls.set_result(
            idx,
            json!(
                rows.iter()
                    .map(|h| h.message_id.clone())
                    .collect::<Vec<_>>()
            ),
        );
        Ok(rows)
    }

    async fn read_document(&self, url_or_token: &str) -> Result<DocumentContent, PlatformError> {
        let idx = self.calls.record(
            "read_document",
            kwargs! {"url_or_token" => json!(url_or_token)},
        );
        self.take_fail("read_document")?;
        let state = self.state.lock().expect("FakePlatform 锁");
        match state.documents.get(url_or_token) {
            Some(doc) => Ok(doc.clone()),
            None => {
                let known: Vec<&String> = state.documents.keys().collect();
                let message = format!("没有这篇文档：{url_or_token:?}（已备：{known:?}）");
                drop(state);
                self.calls.set_error(idx, "not found");
                Err(FakePlatformError(message).into())
            }
        }
    }

    async fn download_file(
        &self,
        message_id: &str,
        file_key: &str,
    ) -> Result<Vec<u8>, PlatformError> {
        let idx = self.calls.record(
            "download_file",
            kwargs! {"message_id" => json!(message_id), "file_key" => json!(file_key)},
        );
        self.take_fail("download_file")?;
        let state = self.state.lock().expect("FakePlatform 锁");
        let key = (message_id.to_string(), file_key.to_string());
        match state.files.get(&key) {
            Some(data) => Ok(data.clone()),
            None => {
                let known: Vec<&(String, String)> = state.files.keys().collect();
                let message = format!(
                    "没有这个附件：message_id={message_id:?} file_key={file_key:?}（已备：{known:?}）"
                );
                drop(state);
                self.calls.set_error(idx, "not found");
                Err(FakePlatformError(message).into())
            }
        }
    }
}

/// 造一条群历史，字段默认值齐全，场景 yaml 里只写关心的那几个。
pub fn history_message(message_id: &str, text: &str) -> HistoryMessage {
    HistoryMessage {
        message_id: message_id.to_string(),
        sender_id: "ou_someone".to_string(),
        sender_kind: "human".to_string(),
        sender_name: Some("ou_someone".to_string()),
        text: text.to_string(),
        thread_id: None,
        created_at: Utc.with_ymd_and_hms(2026, 9, 9, 9, 0, 0).unwrap(),
    }
}

/// 逐字段可调的版本（Rust 没有关键字参数，给一个 builder 风格的入口）。
pub struct HistoryBuilder(HistoryMessage);

impl HistoryBuilder {
    pub fn new(message_id: &str, text: &str) -> Self {
        Self(history_message(message_id, text))
    }
    pub fn sender_id(mut self, v: &str) -> Self {
        self.0.sender_id = v.to_string();
        if self.0.sender_name.as_deref() == Some("ou_someone") {
            self.0.sender_name = Some(v.to_string());
        }
        self
    }
    pub fn sender_kind(mut self, v: &str) -> Self {
        self.0.sender_kind = v.to_string();
        self
    }
    pub fn sender_name(mut self, v: &str) -> Self {
        self.0.sender_name = Some(v.to_string());
        self
    }
    pub fn thread_id(mut self, v: &str) -> Self {
        self.0.thread_id = Some(v.to_string());
        self
    }
    pub fn created_at(mut self, v: DateTime<Utc>) -> Self {
        self.0.created_at = v;
        self
    }
    pub fn build(self) -> HistoryMessage {
        self.0
    }
}
