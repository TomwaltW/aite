//! 会话 / 任务 / 轮次（对应旧 aite/contracts/session.py）。core 内部类型，不跨进程。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::events::{Anchor, Attachment, str_enum};
use crate::outbound::ChecklistState;

str_enum! {
    /// P0 只创建 task
    SessionKind { Channel => "channel", Task => "task", Dm => "dm" }
}

str_enum! {
    SessionStatus { Active => "active", Idle => "idle", Archived => "archived" }
}

str_enum! {
    /// 与立项方案 §4.5 状态机一致
    TaskStatus {
        Created => "created",
        Planning => "planning",
        Answering => "answering",
        Working => "working",
        AwaitingApproval => "awaiting_approval",
        Delivered => "delivered",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

/// SessionStore.list_active_tasks 的口径：status in (created, planning, working)。
pub const ACTIVE_TASK_STATUSES: [TaskStatus; 3] = [
    TaskStatus::Created,
    TaskStatus::Planning,
    TaskStatus::Working,
];

impl TaskStatus {
    pub fn is_active(self) -> bool {
        ACTIVE_TASK_STATUSES.contains(&self)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Delivered | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }
}

str_enum! {
    TurnRole { User => "user", Assistant => "assistant", SystemNote => "system_note" }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub session_id: String,
    /// 由调用方分配；重复 (session_id, seq) 报错
    pub seq: u64,
    pub role: TurnRole,
    #[serde(default)]
    pub platform_user_id: Option<String>,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    /// "c1", "c2", ...（worker 分配）
    pub id: String,
    pub text: String,
    #[serde(default = "ChecklistItem::default_state")]
    pub state: ChecklistState,
    #[serde(default)]
    pub note: Option<String>,
}

impl ChecklistItem {
    fn default_state() -> ChecklistState {
        ChecklistState::Todo
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// uuid4
    pub id: String,
    pub tenant_id: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub kind: SessionKind,
    pub anchor: Anchor,
    #[serde(default = "Session::default_status")]
    pub status: SessionStatus,
    pub created_by: String,
    /// 创建时冻结的模型名/系统指令 hash/发起人显示名（initiator_name）
    #[serde(default)]
    pub config_snapshot: Map<String, Value>,
    pub created_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
}

impl Session {
    fn default_status() -> SessionStatus {
        SessionStatus::Active
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// uuid4
    pub id: String,
    pub session_id: String,
    /// encode_task_no(n)
    pub task_no: String,
    #[serde(default = "Task::default_status")]
    pub status: TaskStatus,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub checklist: Vec<ChecklistItem>,
    #[serde(default)]
    pub card_id: Option<String>,
    #[serde(default)]
    pub sandbox_id: Option<String>,
    /// 随机 32 hex，Tool Gateway 校验用
    pub session_token: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub cost: f64,
    #[serde(default = "Task::default_max_steps")]
    pub max_steps: u32,
    #[serde(default = "Task::default_max_wall_sec")]
    pub max_wall_sec: u32,
    #[serde(default)]
    pub result_summary: String,
    #[serde(default)]
    pub evidence_root_hash: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Task {
    fn default_status() -> TaskStatus {
        TaskStatus::Created
    }
    fn default_max_steps() -> u32 {
        40
    }
    fn default_max_wall_sec() -> u32 {
        1200
    }
}

const B32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"; // Crockford base32

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("task_no 计数必须 >= 1，得到 {0}")]
pub struct TaskNoError(pub u64);

/// 租户内递增计数 → "#A.."。向量：1→"#A1"，17→"#AH"，32→"#A10"，1000→"#AZ8"。n<1 报错。
pub fn encode_task_no(n: u64) -> Result<String, TaskNoError> {
    if n < 1 {
        return Err(TaskNoError(n));
    }
    let mut n = n;
    let mut digits = Vec::new();
    while n > 0 {
        digits.push(B32[(n % 32) as usize] as char);
        n /= 32;
    }
    digits.reverse();
    let mut s = String::from("#A");
    s.extend(digits);
    Ok(s)
}
