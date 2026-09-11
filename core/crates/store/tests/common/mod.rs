//! 各测试文件共用的造数工具。
//!
//! spec §3.2「并行期间各轨要的替身一律在自己 crate 的 tests/ 里私写」—— 这里不依赖
//! aite-testing（那是 R7 的），也不依赖 ControlPlane（那是 R4 的）：Python 里用
//! `plane.handle_event(...)` 顺手造出来的会话与任务，这里直接往 store 里写。
#![allow(dead_code)]

use aite_contracts::{
    Anchor, Session, SessionKind, SessionStatus, SessionStore, Task, TaskStatus, Turn, TurnRole,
};
use aite_store::SqliteSessionStore;
use chrono::{DateTime, TimeZone, Utc};

pub const CHAT: &str = "oc_chat";
pub const ROOT: &str = "om_1";

/// 固定基准时间：`created_at` 是冗余列也是排序键，用固定值才能钉住顺序。
pub fn at(offset_sec: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + offset_sec, 0).unwrap()
}

pub fn make_session(id: &str, chat_id: &str, thread_id: Option<&str>) -> Session {
    Session {
        id: id.to_string(),
        tenant_id: "default".to_string(),
        workspace_id: "cli_ws".to_string(),
        chat_id: chat_id.to_string(),
        kind: SessionKind::Task,
        anchor: Anchor {
            platform: "feishu".to_string(),
            chat_id: chat_id.to_string(),
            message_id: thread_id.unwrap_or(ROOT).to_string(),
            thread_id: thread_id.map(str::to_string),
            task_no: None,
        },
        status: SessionStatus::Active,
        created_by: "ou_1".to_string(),
        config_snapshot: Default::default(),
        created_at: at(0),
        last_active_at: at(0),
        archived_at: None,
    }
}

pub fn make_task(id: &str, session_id: &str, task_no: &str, title: &str) -> Task {
    Task {
        id: id.to_string(),
        session_id: session_id.to_string(),
        task_no: task_no.to_string(),
        status: TaskStatus::Created,
        title: title.to_string(),
        checklist: Vec::new(),
        card_id: None,
        sandbox_id: None,
        session_token: "0".repeat(32),
        model: "scripted-p0".to_string(),
        steps: 0,
        tokens_in: 0,
        tokens_out: 0,
        cost: 0.0,
        max_steps: 40,
        max_wall_sec: 1200,
        result_summary: String::new(),
        evidence_root_hash: None,
        created_by: "ou_1".to_string(),
        created_at: at(0),
        updated_at: at(0),
    }
}

pub fn make_turn(session_id: &str, seq: u64, content: &str) -> Turn {
    Turn {
        session_id: session_id.to_string(),
        seq,
        role: TurnRole::User,
        platform_user_id: Some("ou_1".to_string()),
        content: content.to_string(),
        attachments: Vec::new(),
        created_at: at(seq as i64),
    }
}

/// Python 的 `make_turn(session_id, seq)`：正文是「第 N 问」。
pub fn numbered_turn(session_id: &str, seq: u64) -> Turn {
    make_turn(session_id, seq, &format!("第 {seq} 问"))
}

/// 打开并建表。
pub async fn open_store(path: impl AsRef<std::path::Path>) -> SqliteSessionStore {
    let store = SqliteSessionStore::open(path).expect("open");
    store.init().await.expect("init");
    store
}

/// 建一个会话 + 一个任务，返回 (session_id, task_id)。
pub async fn seed_task(
    store: &SqliteSessionStore,
    session_id: &str,
    task_id: &str,
    chat_id: &str,
    title: &str,
) -> String {
    let session = make_session(session_id, chat_id, Some(ROOT));
    store
        .create_session(&session)
        .await
        .expect("create_session");
    let task_no = store.next_task_no("default").await.expect("next_task_no");
    let task = make_task(task_id, session_id, &task_no, title);
    store.create_task(&task).await.expect("create_task");
    task_no
}
