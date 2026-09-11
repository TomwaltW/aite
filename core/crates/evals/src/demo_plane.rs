//! `DemoPlane` —— 一个**只为验 runner 本身而存在**的最小 ControlPlane
//! （对应旧 `tests/e2e/conftest.py` 里的同名类）。
//!
//! 它不是 R4 的实现，也不打算变成 R4 的实现 —— 只覆盖 R1（丢非真人）/ R2（去重）/
//! R6（话题续接）/ R7（新建会话）加「一步 final」这条最短路径，好证明
//! 「场景 yaml → 事件 → 替身 → expect 断言」这条链是通的，而不是一个永远报
//! `not implemented` 的空壳。真实现见 `aite-control`（R4）。
//!
//! 覆盖得到的场景：01_simple_qa / 02_thread_followup / 09_bot_ignored / 10_duplicate_event。
//! 卡片、工具、命令那几条它覆盖不到 —— 那些要真 worker。
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use aite_contracts::{
    ControlPlane, IngressError, NormalizedEvent, OutboundText, PlatformPort, ReactionKind, Role,
    SenderKind, Session, SessionKind, SessionStatus, SessionStore, Task, TaskStatus, Turn,
    TurnRole, all_model_tools,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Map, Value, json};

use crate::deps::Deps;

struct Queued {
    session: Session,
    task: Task,
    event: NormalizedEvent,
}

pub struct DemoPlane {
    config: aite_contracts::AiteConfig,
    platform: Arc<aite_testing::FakePlatform>,
    model: Arc<crate::protocol_probe::ModelProbe>,
    store: Arc<aite_testing::FakeSessionStore>,
    queue: Mutex<VecDeque<Queued>>,
    running: Mutex<usize>,
    counters: Mutex<Map<String, Value>>,
}

impl DemoPlane {
    pub fn new(deps: &Deps) -> Self {
        Self {
            config: deps.config.clone(),
            platform: deps.platform.clone(),
            model: deps.model.clone(),
            store: deps.store.clone(),
            queue: Mutex::new(VecDeque::new()),
            running: Mutex::new(0),
            counters: Mutex::new(Map::new()),
        }
    }

    /// 给 `PlaneFactory` 用的工厂。
    pub fn factory() -> crate::runner::PlaneFactory {
        Arc::new(|deps: &Deps| Ok(Arc::new(DemoPlane::new(deps)) as Arc<dyn ControlPlane>))
    }

    fn bump(&self, key: &str) {
        let mut c = self.counters.lock().expect("DemoPlane 锁");
        let n = c.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
        c.insert(key.to_string(), json!(n));
    }

    fn new_id() -> String {
        // uuid crate 在 workspace 里，但这里只要一个够用的唯一串：时间 + 计数器
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!(
            "{:016x}{:08x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or_default(),
            N.fetch_add(1, Ordering::SeqCst)
        )
    }

    async fn run_one(&self, item: Queued) {
        let Queued {
            session: _session,
            mut task,
            event,
        } = item;
        // worker 领走的第一件事：把任务推出 created（after: running 的判据看的就是这个）
        task.status = TaskStatus::Planning;
        task.updated_at = Utc::now();
        let _ = self.store.update_task(&task).await;

        let messages = vec![aite_contracts::Message::text(
            Role::User,
            event.text.clone(),
        )];
        let turn = match aite_contracts::ModelPort::chat(
            &*self.model,
            &messages,
            all_model_tools(),
            self.config.model.max_tokens,
            self.config.model.temperature,
        )
        .await
        {
            Ok(turn) => turn,
            Err(_) => {
                task.status = TaskStatus::Failed;
                task.updated_at = Utc::now();
                let _ = self.store.update_task(&task).await;
                return;
            }
        };

        for tc in turn.message.tool_calls.iter().flatten() {
            if tc.name != "final" {
                continue;
            }
            let reply = tc
                .arguments
                .get("reply")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            let _ = self
                .platform
                .send_text(&OutboundText {
                    chat_id: event.chat_id.clone(),
                    text: reply,
                    reply_to: Some(event.anchor.message_id.clone()),
                    in_thread: true,
                })
                .await;
            task.status = TaskStatus::Delivered;
            task.updated_at = Utc::now();
            let _ = self.store.update_task(&task).await;
        }
    }

    fn take(&self) -> Option<Queued> {
        let mut q = self.queue.lock().expect("DemoPlane 锁");
        let item = q.pop_front();
        if item.is_some() {
            *self.running.lock().expect("DemoPlane 锁") += 1;
        }
        item
    }

    fn done(&self) {
        let mut running = self.running.lock().expect("DemoPlane 锁");
        *running = running.saturating_sub(1);
    }
}

#[async_trait]
impl ControlPlane for DemoPlane {
    async fn handle_event(&self, ev: NormalizedEvent) -> Result<(), IngressError> {
        // R1：机器人/应用/系统消息永远不触发任务
        if ev.sender_kind != SenderKind::Human {
            self.bump("events.nonhuman");
            return Ok(());
        }
        // R2：去重
        if self.store.seen_event(&ev.event_id).await? {
            self.bump("events.duplicate");
            return Ok(());
        }

        let now = Utc::now();
        // R6：话题内续接
        let mut session = match &ev.anchor.thread_id {
            Some(tid) => self.store.find_session_by_thread(&ev.chat_id, tid).await?,
            None => None,
        };
        // R7：新建会话 + ack 表情
        if session.is_none() {
            let mut anchor = ev.anchor.clone();
            if anchor.thread_id.is_none() {
                anchor.thread_id = Some(anchor.message_id.clone());
            }
            let s = Session {
                id: Self::new_id(),
                tenant_id: ev.tenant_id.clone(),
                workspace_id: ev.workspace_id.clone(),
                chat_id: ev.chat_id.clone(),
                kind: SessionKind::Task,
                anchor,
                status: SessionStatus::Active,
                created_by: ev.sender_id.clone(),
                config_snapshot: Map::new(),
                created_at: now,
                last_active_at: now,
                archived_at: None,
            };
            self.store.create_session(&s).await?;
            self.platform
                .add_reaction(&ev.anchor.message_id, ReactionKind::Ack)
                .await?;
            session = Some(s);
        }
        let session = session.expect("上面要么找到要么新建");

        let seq = self.store.next_turn_seq(&session.id).await?;
        self.store
            .append_turn(&Turn {
                session_id: session.id.clone(),
                seq,
                role: TurnRole::User,
                platform_user_id: Some(ev.sender_id.clone()),
                content: ev.text.clone(),
                attachments: ev.attachments.clone(),
                created_at: now,
            })
            .await?;

        let task = Task {
            id: Self::new_id(),
            session_id: session.id.clone(),
            task_no: self.store.next_task_no(&ev.tenant_id).await?,
            status: TaskStatus::Created,
            title: String::new(),
            checklist: Vec::new(),
            card_id: None,
            sandbox_id: None,
            session_token: Self::new_id(),
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
        self.bump("events.handled");
        self.queue.lock().expect("DemoPlane 锁").push_back(Queued {
            session,
            task,
            event: ev,
        });
        Ok(())
    }

    async fn run_forever(&self) {
        loop {
            match self.take() {
                Some(item) => {
                    self.run_one(item).await;
                    self.done();
                }
                None => tokio::task::yield_now().await,
            }
        }
    }

    async fn run_pending(&self) {
        while let Some(item) = self.take() {
            self.run_one(item).await;
            self.done();
        }
    }

    fn pending(&self) -> usize {
        self.queue.lock().expect("DemoPlane 锁").len() + *self.running.lock().expect("DemoPlane 锁")
    }

    async fn join(&self) {
        while self.pending() > 0 {
            tokio::task::yield_now().await;
        }
    }

    async fn cancel_task(
        &self,
        mut task: Task,
        _reply_to: Option<String>,
        _chat_id: Option<String>,
        _notify: bool,
    ) -> Task {
        task.status = TaskStatus::Cancelled;
        task.updated_at = Utc::now();
        let _ = self.store.update_task(&task).await;
        task
    }

    fn counters(&self) -> Map<String, Value> {
        self.counters.lock().expect("DemoPlane 锁").clone()
    }
}
