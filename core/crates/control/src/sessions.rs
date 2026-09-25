//! 会话与任务的建立、话题查找、transcript 追加、回帖。CC2 从 `plane.rs` 原样搬来。
use serde_json::{Map, Value};

use aite_contracts::{
    Anchor, EvidenceKind, IngressError, NormalizedEvent, OutboundText, ReactionKind, Session,
    SessionKind, SessionStatus, Task, TaskStatus, Turn, TurnRole,
};
use rand::Rng;

use crate::card::{MAX_TITLE_CHARS, clip};
use crate::evidence_log::{ROUTE_NEW_TASK, event_payload};
use crate::lock;
use crate::plane::InProcessControlPlane;

/// 32 hex（Python 的 `secrets.token_hex(16)`）。
pub(crate) fn token_hex_16() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// 发起人显示名：`config_snapshot["initiator_name"]`，空则退回 `created_by`。
pub(crate) fn initiator_of(session: &Session) -> String {
    session
        .config_snapshot
        .get("initiator_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| session.created_by.clone())
}

impl InProcessControlPlane {
    pub(crate) async fn new_session(
        &self,
        ev: &NormalizedEvent,
        thread_id: &str,
        text: &str,
        react: bool,
    ) -> Result<Session, IngressError> {
        let now = self.now();
        let mut config_snapshot = Map::new();
        config_snapshot.insert(
            "model".into(),
            Value::String(self.config.model.model.clone()),
        );
        config_snapshot.insert(
            "initiator_name".into(),
            Value::String(
                ev.sender_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| ev.sender_id.clone()),
            ),
        );
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            tenant_id: ev.tenant_id.clone(),
            workspace_id: ev.workspace_id.clone(),
            chat_id: ev.chat_id.clone(),
            kind: SessionKind::Task,
            anchor: Anchor {
                platform: ev.platform.clone(),
                chat_id: ev.chat_id.clone(),
                message_id: ev.anchor.message_id.clone(),
                thread_id: Some(thread_id.to_string()),
                task_no: None,
            },
            status: SessionStatus::Active,
            created_by: ev.sender_id.clone(),
            config_snapshot,
            created_at: now,
            last_active_at: now,
            archived_at: None,
        };
        self.store.create_session(&session).await?;
        if react {
            // ack 失败不影响建任务（Python 的 contextlib.suppress）
            if let Err(e) = self
                .platform
                .add_reaction(&ev.anchor.message_id, ReactionKind::Ack)
                .await
            {
                tracing::warn!(target: "aite.control", error = %e, "control.ack_failed");
            }
        }
        if !text.is_empty() {
            self.append_turn(&session, ev, TurnRole::User, text).await?;
            self.start_task(&session, ev, Some(text)).await?;
        }
        Ok(session)
    }

    /// 六步顺序严格：造 Task → `create_task` → `task_created` 证据 → `event_received`
    /// 证据 → `_owned` → 入队。证据链前两条永远是 `task_created, event_received`。
    pub(crate) async fn start_task(
        &self,
        session: &Session,
        ev: &NormalizedEvent,
        text: Option<&str>,
    ) -> Result<Task, IngressError> {
        let now = self.now();
        let task_no = self.store.next_task_no(&session.tenant_id).await?;
        let task = Task {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            task_no,
            status: TaskStatus::Created,
            title: clip(text.unwrap_or(&ev.text), MAX_TITLE_CHARS),
            checklist: Vec::new(),
            card_id: None,
            sandbox_id: None,
            session_token: token_hex_16(),
            model: self.config.model.model.clone(),
            steps: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            max_steps: self.config.worker.max_steps,
            max_wall_sec: self.config.worker.max_wall_sec,
            result_summary: String::new(),
            evidence_root_hash: None,
            created_by: ev.sender_id.clone(),
            created_at: now,
            updated_at: now,
        };
        self.store.create_task(&task).await?;

        let mut created = Map::new();
        created.insert("session_id".into(), Value::String(session.id.clone()));
        created.insert("task_no".into(), Value::String(task.task_no.clone()));
        created.insert("chat_id".into(), Value::String(session.chat_id.clone()));
        created.insert("created_by".into(), Value::String(task.created_by.clone()));
        created.insert("title".into(), Value::String(task.title.clone()));
        self.evidence
            .append(&task.id, EvidenceKind::TaskCreated, created)
            .await?;
        self.evidence
            .append(
                &task.id,
                EvidenceKind::EventReceived,
                event_payload(ev, ROUTE_NEW_TASK, None),
            )
            .await?;

        lock(&self.shared.owned).insert(task.id.clone());
        self.shared.queue.put(task.id.clone());
        Ok(task)
    }

    pub(crate) async fn thread_session(
        &self,
        ev: &NormalizedEvent,
        fallback_to_message_id: bool,
    ) -> Result<Option<Session>, IngressError> {
        if let Some(thread_id) = ev.anchor.thread_id.as_deref().filter(|t| !t.is_empty())
            && let Some(hit) = self
                .store
                .find_session_by_thread(&ev.chat_id, thread_id)
                .await?
        {
            return Ok(Some(hit));
        }
        if fallback_to_message_id {
            // 被编辑的可能正是话题 root 那条消息
            return Ok(self
                .store
                .find_session_by_thread(&ev.chat_id, &ev.anchor.message_id)
                .await?);
        }
        Ok(None)
    }

    /// seq 由调用方分配（§3.2）。只用协议里有的 `list_turns` 推下一个 seq，
    /// 这样换任何 SessionStore 实现都成立。
    ///
    /// 读 seq 和写 turn 之间不许有别人插进来。同一个话题里的两条事件**是会同时**进
    /// `handle_event` 的 —— 重连那一刻整批一起上来，而这中间隔着两次真会挂起的存储往返。
    /// 两条都读到「还没有 turn」就都写 seq=0，第二条撞上 (session_id, seq) 唯一约束报
    /// `DuplicateTurn`；这一报发生在 `seen_event` **已经落库之后**，于是事件既没变成任务，
    /// 也永远不会被重推第二次（R2 认得它了）—— 用户那句话就此消失。M2 后半句在这里破。
    ///
    /// P0 是单进程单副本，进程内一把锁就够。
    pub(crate) async fn append_turn(
        &self,
        session: &Session,
        ev: &NormalizedEvent,
        role: TurnRole,
        content: &str,
    ) -> Result<(), IngressError> {
        let _seq_guard = self.turn_seq_lock.lock().await;
        let recent = self.store.list_turns(&session.id, 1).await?;
        let seq = recent.last().map(|t| t.seq + 1).unwrap_or(0);
        self.store
            .append_turn(&Turn {
                session_id: session.id.clone(),
                seq,
                role,
                platform_user_id: Some(ev.sender_id.clone()),
                content: content.to_string(),
                attachments: ev.attachments.clone(),
                created_at: ev.occurred_at,
            })
            .await?;
        Ok(())
    }

    pub(crate) async fn reply(&self, ev: &NormalizedEvent, text: &str) -> Result<(), IngressError> {
        self.platform
            .send_text(&OutboundText {
                chat_id: ev.chat_id.clone(),
                text: text.to_string(),
                reply_to: Some(ev.anchor.message_id.clone()),
                in_thread: true,
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hex_16_is_32_hex_chars() {
        let token = token_hex_16();
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token, token_hex_16(), "两次取不该一样");
    }
}
