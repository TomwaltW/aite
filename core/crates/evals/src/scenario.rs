//! 场景文件（`evals/p0/*.yaml`）的形状与加载（对应旧 `aite/evals/scenario.py`）。
//!
//! 一个场景 = 「喂什么」+「模型怎么出牌」+「该看到什么」：
//!
//! ```yaml
//! name: 03_checklist_progress
//! title: 卡片进度原地更新
//! verifies: send_card x1、update_card>=3、无第二条卡片
//! platform:            # FakePlatform 的初始数据：群历史 / 云文档 / 附件
//! sandbox:             # FakeSandbox 的 exec 脚本
//! events:              # 按顺序投给 ControlPlane 的归一化事件
//! model_script:        # FakeModel 每一步出什么牌
//! expect:              # 断言清单，语义见 checks.rs
//! ```
//!
//! 字段默认值给得很足，场景 yaml 里只写关心的那几个 —— 场景文件是给人读的，
//! 不该被十几行样板淹掉。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aite_contracts::{
    Anchor, Attachment, AttachmentKind, CardAction, ChatType, DocumentContent, EventKind,
    HistoryMessage, NormalizedEvent, SenderKind,
};
use aite_testing::{ExecScriptStep, ScriptStep, as_bytes};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 所有时间戳从这里起算，场景之间可复现。
pub fn base_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 9, 9, 0, 0).unwrap()
}

pub const DEFAULT_CHAT_ID: &str = "oc_demo";
pub const DEFAULT_WORKSPACE_ID: &str = "cli_fake_app";

/// `after` 的等待上限（秒）。等不到就让场景以 dispatch 失败收场，绝不接着往下投 ——
/// 「等不到就接着投」测出来的绿是假的。
pub const DEFAULT_AFTER_TIMEOUT_SEC: f64 = 5.0;

/// 场景文件本身写错了（缺字段、名字对不上目录等）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ScenarioError(pub String);

/// 投这条事件之前先等什么（冻结契约 C-T5T6-1）。判据实现在 `runner.rs`：
///
///   none     不等，紧接上一条投（默认值）
///   idle     等系统静默：在跑的任务都收了、待处理队列空了，再投
///   running  等上一条事件起的那个任务真的被 worker 领走、开始跑了，再投
///
/// 真实平台上两条消息之间必然隔着人打字的时间，评测里默认却是同一轮调度。
/// 需要那段时间的场景把它显式写出来 —— 而不是反过来改控制面的路由去迁就场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EventAfter {
    #[default]
    None,
    Idle,
    Running,
}

impl EventAfter {
    pub fn as_str(self) -> &'static str {
        match self {
            EventAfter::None => "none",
            EventAfter::Idle => "idle",
            EventAfter::Running => "running",
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_sender_id() -> String {
    "ou_alice".to_string()
}
fn default_sender_name() -> Option<String> {
    Some("Alice".to_string())
}
fn default_chat_id() -> String {
    DEFAULT_CHAT_ID.to_string()
}
fn default_workspace_id() -> String {
    DEFAULT_WORKSPACE_ID.to_string()
}
fn default_tenant_id() -> String {
    "default".to_string()
}
fn default_after_timeout() -> f64 {
    DEFAULT_AFTER_TIMEOUT_SEC
}
fn default_timeout_sec() -> f64 {
    10.0
}
fn default_event_kind() -> EventKind {
    EventKind::Message
}
fn default_sender_kind() -> SenderKind {
    SenderKind::Human
}
fn default_chat_type() -> ChatType {
    ChatType::Group
}
fn default_attachment_kind() -> AttachmentKind {
    AttachmentKind::File
}
fn default_human() -> String {
    "human".to_string()
}
fn default_someone() -> String {
    "ou_someone".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentSpec {
    #[serde(default = "default_attachment_kind")]
    pub kind: AttachmentKind,
    pub file_key: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default)]
    pub mime: Option<String>,
}

/// 一条投给 ControlPlane 的事件。默认是「群里 @Aite 的一条真人消息」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSpec {
    pub event_id: String,
    #[serde(default = "default_event_kind")]
    pub kind: EventKind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub raw_text: Option<String>,
    #[serde(default = "default_true")]
    pub mentioned: bool,
    #[serde(default = "default_sender_id")]
    pub sender_id: String,
    #[serde(default = "default_sender_kind")]
    pub sender_kind: SenderKind,
    #[serde(default = "default_sender_name")]
    pub sender_name: Option<String>,
    #[serde(default = "default_chat_id")]
    pub chat_id: String,
    #[serde(default = "default_chat_type")]
    pub chat_type: ChatType,
    #[serde(default = "default_workspace_id")]
    pub workspace_id: String,
    #[serde(default = "default_tenant_id")]
    pub tenant_id: String,
    /// 触发消息 id；不写就按 event_id 派生
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub task_no: Option<String>,
    #[serde(default)]
    pub attachments: Vec<AttachmentSpec>,
    #[serde(default)]
    pub card_action: Option<CardAction>,
    /// 相对 BASE_TIME 的秒偏移；不写就用事件在列表里的下标
    #[serde(default)]
    pub at_sec: Option<i64>,
    /// 投这条之前先等什么（C-T5T6-1）
    #[serde(default)]
    pub after: EventAfter,
    /// 上面那个等待的上限（秒）
    #[serde(default = "default_after_timeout")]
    pub after_timeout_sec: f64,
}

impl EventSpec {
    pub fn new(event_id: &str, text: &str) -> Self {
        serde_json::from_value(serde_json::json!({"event_id": event_id, "text": text}))
            .expect("EventSpec 默认值")
    }

    pub fn build(&self, index: usize) -> NormalizedEvent {
        let message_id = self
            .message_id
            .clone()
            .unwrap_or_else(|| format!("om_{}", self.event_id));
        NormalizedEvent {
            event_id: self.event_id.clone(),
            kind: self.kind,
            platform: "fake".to_string(),
            tenant_id: self.tenant_id.clone(),
            workspace_id: self.workspace_id.clone(),
            chat_id: self.chat_id.clone(),
            chat_type: self.chat_type,
            sender_id: self.sender_id.clone(),
            sender_kind: self.sender_kind,
            sender_name: self.sender_name.clone(),
            text: self.text.clone(),
            raw_text: self.raw_text.clone(),
            mentioned: self.mentioned,
            anchor: Anchor {
                platform: "fake".to_string(),
                chat_id: self.chat_id.clone(),
                message_id: message_id.clone(),
                thread_id: self.thread_id.clone(),
                task_no: self.task_no.clone(),
            },
            attachments: self
                .attachments
                .iter()
                .map(|a| Attachment {
                    kind: a.kind,
                    file_key: a.file_key.clone(),
                    message_id: message_id.clone(),
                    name: a.name.clone(),
                    size: a.size,
                    mime: a.mime.clone(),
                })
                .collect(),
            card_action: self.card_action.clone(),
            occurred_at: base_time() + Duration::seconds(self.at_sec.unwrap_or(index as i64)),
            raw: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySpec {
    pub message_id: String,
    pub text: String,
    #[serde(default = "default_someone")]
    pub sender_id: String,
    #[serde(default = "default_human")]
    pub sender_kind: String,
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub at_sec: Option<i64>,
}

impl HistorySpec {
    pub fn build(&self, index: usize) -> HistoryMessage {
        HistoryMessage {
            message_id: self.message_id.clone(),
            sender_id: self.sender_id.clone(),
            sender_kind: self.sender_kind.clone(),
            sender_name: Some(
                self.sender_name
                    .clone()
                    .unwrap_or_else(|| self.sender_id.clone()),
            ),
            text: self.text.clone(),
            thread_id: self.thread_id.clone(),
            created_at: base_time() + Duration::seconds(-600 + self.at_sec.unwrap_or(index as i64)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSpec {
    /// read_document 的 url_or_token 用哪个键命中
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub url: Option<String>,
}

impl DocumentSpec {
    pub fn build(&self) -> DocumentContent {
        DocumentContent {
            title: self.title.clone(),
            text: self.text.clone(),
            url: self.url.clone().unwrap_or_else(|| self.key.clone()),
        }
    }
}

/// 群消息里的一个附件：`download_file(message_id, file_key)` 能取到它。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    pub message_id: String,
    pub file_key: String,
    #[serde(default)]
    pub content: Value,
}

impl FileSpec {
    pub fn data(&self) -> Result<Vec<u8>, ScenarioError> {
        let value = if self.content.is_null() {
            Value::String(String::new())
        } else {
            self.content.clone()
        };
        as_bytes(&value).map_err(|e| ScenarioError(e.0))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformFixture {
    pub history: Vec<HistorySpec>,
    pub documents: Vec<DocumentSpec>,
    pub files: Vec<FileSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxFixture {
    pub exec_script: Vec<ExecScriptStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub verifies: String,
    #[serde(default)]
    pub spec_ref: String,
    /// 覆盖 AiteConfig 的片段，例如 `{"worker": {"max_steps": 3}}`
    #[serde(default)]
    pub config: Map<String, Value>,
    #[serde(default)]
    pub platform: PlatformFixture,
    #[serde(default)]
    pub sandbox: SandboxFixture,
    #[serde(default)]
    pub model_script: Vec<ScriptStep>,
    #[serde(default)]
    pub events: Vec<EventSpec>,
    #[serde(default)]
    pub expect: Vec<Value>,
    /// 单个场景的驱动上限（秒）
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: f64,
    /// 来源文件，加载时填
    #[serde(default)]
    pub source: String,
}

impl Scenario {
    /// 只有名字的空场景（测试与 `build_deps` 的最小入口）。
    pub fn named(name: &str) -> Self {
        serde_json::from_value(serde_json::json!({"name": name})).expect("Scenario 默认值")
    }

    /// (投递时序, 归一化事件) 逐条配对 —— runner 按 `EventSpec.after` 决定投之前等什么。
    pub fn dispatch_plan(&self) -> Vec<(EventSpec, NormalizedEvent)> {
        self.events
            .iter()
            .enumerate()
            .map(|(i, e)| (e.clone(), e.build(i)))
            .collect()
    }

    pub fn build_events(&self) -> Vec<NormalizedEvent> {
        self.events
            .iter()
            .enumerate()
            .map(|(i, e)| e.build(i))
            .collect()
    }

    pub fn build_history(&self) -> Vec<HistoryMessage> {
        self.platform
            .history
            .iter()
            .enumerate()
            .map(|(i, h)| h.build(i))
            .collect()
    }

    pub fn build_documents(&self) -> BTreeMap<String, DocumentContent> {
        self.platform
            .documents
            .iter()
            .map(|d| (d.key.clone(), d.build()))
            .collect()
    }

    pub fn build_files(&self) -> Result<BTreeMap<(String, String), Vec<u8>>, ScenarioError> {
        let mut out = BTreeMap::new();
        for f in &self.platform.files {
            out.insert((f.message_id.clone(), f.file_key.clone()), f.data()?);
        }
        Ok(out)
    }

    /// 按倍数放大两个等待上限。只在内存里改，场景文件一个字都不动。
    pub fn scaled(&self, k: f64) -> Scenario {
        let mut out = self.clone();
        out.timeout_sec *= k;
        for e in &mut out.events {
            e.after_timeout_sec *= k;
        }
        out
    }
}

/// YAML 值 → JSON 值。两边都是无标签的动态树，逐节点搬。
fn yaml_to_json(value: serde_yaml::Value) -> Result<Value, ScenarioError> {
    Ok(match value {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => Value::Bool(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        }
        serde_yaml::Value::String(s) => Value::String(s),
        serde_yaml::Value::Sequence(items) => Value::Array(
            items
                .into_iter()
                .map(yaml_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        serde_yaml::Value::Mapping(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => s,
                    serde_yaml::Value::Number(n) => n.to_string(),
                    serde_yaml::Value::Bool(b) => b.to_string(),
                    other => {
                        return Err(ScenarioError(format!(
                            "YAML 的键必须是标量，收到 {other:?}"
                        )));
                    }
                };
                out.insert(key, yaml_to_json(v)?);
            }
            Value::Object(out)
        }
        serde_yaml::Value::Tagged(t) => yaml_to_json(t.value)?,
    })
}

pub fn load_scenario(path: &Path) -> Result<Scenario, ScenarioError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ScenarioError(format!("{}: 读不出来（{e}）", path.display())))?;
    let raw: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|e| ScenarioError(format!("{}: YAML 解析失败（{e}）", path.display())))?;
    let mut json = yaml_to_json(raw)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    let Value::Object(map) = &mut json else {
        return Err(ScenarioError(format!(
            "{}: 顶层必须是 mapping，实际是 {}",
            path.display(),
            kind_of(&json)
        )));
    };
    match map.get("name") {
        None => {
            map.insert("name".to_string(), Value::String(stem.clone()));
        }
        Some(Value::String(name)) if *name == stem => {}
        Some(other) => {
            return Err(ScenarioError(format!(
                "{}: name={other} 与文件名 {stem:?} 不一致",
                path.display()
            )));
        }
    }

    let mut sc: Scenario = serde_json::from_value(json)
        .map_err(|e| ScenarioError(format!("{}: {e}", path.display())))?;
    sc.source = path.display().to_string();
    Ok(sc)
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "mapping",
    }
}

/// 按文件名排序加载一个目录下的全部场景（*.yaml / *.yml）。
pub fn load_suite(suite_dir: &Path) -> Result<Vec<Scenario>, ScenarioError> {
    if !suite_dir.is_dir() {
        return Err(ScenarioError(format!(
            "场景目录不存在：{}",
            suite_dir.display()
        )));
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(suite_dir)
        .map_err(|e| ScenarioError(format!("{}: 读不出来（{e}）", suite_dir.display())))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("yaml") | Some("yml")
            )
        })
        .collect();
    if paths.is_empty() {
        return Err(ScenarioError(format!(
            "{} 下没有 *.yaml 场景文件",
            suite_dir.display()
        )));
    }
    paths.sort();
    paths.iter().map(|p| load_scenario(p)).collect()
}
