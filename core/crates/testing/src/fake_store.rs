//! FakeSessionStore / FakeEvidenceWriter —— 两个 Port 的内存替身
//! （对应旧 `aite/testing/fake_store.py`）。真实现（SQLite + 落盘 evidence）归 R3。
//!
//! 这两份只保证语义对：
//!
//! * `append_turn` 对重复的 (session_id, seq) 报 `StoreError::DuplicateTurn`
//! * `next_task_no` 原子递增并走契约的 `encode_task_no`
//! * `seen_event` 首次 false、之后 true
//! * `list_active_tasks` 只回 created / planning / working 且属于本 chat 的
//! * evidence 链用契约里的 `payload_hash_of` / `chain_hash` 真算，`verify` 重算整条链
//!
//! 存取全走深拷贝（Rust 里就是 clone）：存进去之后在外面改对象，不该改到库里那份。
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;

use aite_contracts::{
    ACTIVE_TASK_STATUSES, EvidenceError, EvidenceEvent, EvidenceKind, EvidenceWriter, GENESIS,
    Session, SessionStore, StoreError, Task, Turn, chain_hash, encode_task_no, payload_hash_of,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Map, Value, json};

use crate::kwargs;
use crate::recorder::CallLog;

#[derive(Default)]
struct StoreState {
    sessions: BTreeMap<String, Session>,
    session_order: Vec<String>,
    tasks: BTreeMap<String, Task>,
    /// 插入序 —— `task_list()` 与 `newest_task()` 要靠它（对应 Python dict 的插入序）
    task_order: Vec<String>,
    turns: BTreeMap<String, Vec<Turn>>,
    seen: BTreeSet<String>,
    counters: BTreeMap<String, u64>,
    initialized: bool,
    closed: bool,
}

/// `SessionStore` 的内存替身。
pub struct FakeSessionStore {
    pub calls: CallLog,
    state: Mutex<StoreState>,
}

impl Default for FakeSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSessionStore {
    pub fn new() -> Self {
        Self {
            calls: CallLog::new(),
            state: Mutex::new(StoreState::default()),
        }
    }

    pub fn initialized(&self) -> bool {
        self.state.lock().expect("FakeStore 锁").initialized
    }

    pub fn closed(&self) -> bool {
        self.state.lock().expect("FakeStore 锁").closed
    }

    pub fn session_count(&self) -> usize {
        self.state.lock().expect("FakeStore 锁").sessions.len()
    }

    pub fn task_count(&self) -> usize {
        self.state.lock().expect("FakeStore 锁").tasks.len()
    }

    pub fn seen_count(&self) -> usize {
        self.state.lock().expect("FakeStore 锁").seen.len()
    }

    pub fn session_ids(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("FakeStore 锁")
            .sessions
            .keys()
            .cloned()
            .collect()
    }

    /// 按 created_at 正序的任务列表（`check: task` 用）。
    pub fn task_list(&self) -> Vec<Task> {
        let state = self.state.lock().expect("FakeStore 锁");
        let mut out: Vec<Task> = state
            .task_order
            .iter()
            .filter_map(|id| state.tasks.get(id))
            .cloned()
            .collect();
        out.sort_by_key(|t| t.created_at);
        out
    }

    /// 最后建出来的那个任务 —— 直接读字段、不走 store 的方法，
    /// 别让「等待」这个动作本身给 `Deps::activity()` 添活动。
    pub fn newest_task(&self) -> Option<Task> {
        let state = self.state.lock().expect("FakeStore 锁");
        state
            .task_order
            .last()
            .and_then(|id| state.tasks.get(id))
            .cloned()
    }

    /// task_id -> session_token，同步、不记账（`real_stack::token_resolver_of` 用）。
    pub fn session_token_of(&self, task_id: &str) -> Option<String> {
        self.state
            .lock()
            .expect("FakeStore 锁")
            .tasks
            .get(task_id)
            .map(|t| t.session_token.clone())
    }

    /// 任务落在几个不同会话上。
    pub fn distinct_session_ids_of_tasks(&self) -> BTreeSet<String> {
        self.state
            .lock()
            .expect("FakeStore 锁")
            .tasks
            .values()
            .map(|t| t.session_id.clone())
            .collect()
    }

    /// 快照：所有任务（不记账）。
    pub fn tasks_snapshot(&self) -> Vec<Task> {
        self.state
            .lock()
            .expect("FakeStore 锁")
            .tasks
            .values()
            .cloned()
            .collect()
    }
}

#[async_trait]
impl SessionStore for FakeSessionStore {
    async fn init(&self) -> Result<(), StoreError> {
        self.calls.record("init", kwargs! {});
        self.state.lock().expect("FakeStore 锁").initialized = true; // 幂等
        Ok(())
    }

    async fn close(&self) -> Result<(), StoreError> {
        self.calls.record("close", kwargs! {});
        self.state.lock().expect("FakeStore 锁").closed = true;
        Ok(())
    }

    async fn get_session(&self, session_id: &str) -> Result<Option<Session>, StoreError> {
        self.calls
            .record("get_session", kwargs! {"session_id" => json!(session_id)});
        Ok(self
            .state
            .lock()
            .expect("FakeStore 锁")
            .sessions
            .get(session_id)
            .cloned())
    }

    async fn find_session_by_thread(
        &self,
        chat_id: &str,
        thread_id: &str,
    ) -> Result<Option<Session>, StoreError> {
        self.calls.record(
            "find_session_by_thread",
            kwargs! {"chat_id" => json!(chat_id), "thread_id" => json!(thread_id)},
        );
        let state = self.state.lock().expect("FakeStore 锁");
        Ok(state
            .session_order
            .iter()
            .filter_map(|id| state.sessions.get(id))
            .find(|s| s.chat_id == chat_id && s.anchor.thread_id.as_deref() == Some(thread_id))
            .cloned())
    }

    async fn create_session(&self, s: &Session) -> Result<(), StoreError> {
        self.calls.record(
            "create_session",
            kwargs! {"session_id" => json!(s.id), "thread_id" => json!(s.anchor.thread_id)},
        );
        let mut state = self.state.lock().expect("FakeStore 锁");
        if state.sessions.contains_key(&s.id) {
            return Err(StoreError::Other(format!("session {} 已存在", s.id)));
        }
        state.sessions.insert(s.id.clone(), s.clone());
        state.session_order.push(s.id.clone());
        state.turns.entry(s.id.clone()).or_default();
        Ok(())
    }

    async fn update_session(&self, s: &Session) -> Result<(), StoreError> {
        self.calls.record(
            "update_session",
            kwargs! {"session_id" => json!(s.id), "status" => json!(s.status.as_str())},
        );
        let mut state = self.state.lock().expect("FakeStore 锁");
        if !state.sessions.contains_key(&s.id) {
            state.session_order.push(s.id.clone());
        }
        state.sessions.insert(s.id.clone(), s.clone());
        Ok(())
    }

    async fn append_turn(&self, t: &Turn) -> Result<(), StoreError> {
        self.calls.record(
            "append_turn",
            kwargs! {
                "session_id" => json!(t.session_id),
                "seq" => json!(t.seq),
                "role" => json!(t.role.as_str()),
            },
        );
        let mut state = self.state.lock().expect("FakeStore 锁");
        let rows = state.turns.entry(t.session_id.clone()).or_default();
        if rows.iter().any(|r| r.seq == t.seq) {
            return Err(StoreError::DuplicateTurn {
                session_id: t.session_id.clone(),
                seq: t.seq,
            });
        }
        rows.push(t.clone());
        Ok(())
    }

    async fn list_turns(&self, session_id: &str, limit: u32) -> Result<Vec<Turn>, StoreError> {
        self.calls.record(
            "list_turns",
            kwargs! {"session_id" => json!(session_id), "limit" => json!(limit)},
        );
        let state = self.state.lock().expect("FakeStore 锁");
        let mut rows = state.turns.get(session_id).cloned().unwrap_or_default();
        rows.sort_by_key(|r| r.seq);
        let start = rows.len().saturating_sub(limit as usize);
        Ok(rows.split_off(start))
    }

    async fn next_turn_seq(&self, session_id: &str) -> Result<u64, StoreError> {
        self.calls
            .record("next_turn_seq", kwargs! {"session_id" => json!(session_id)});
        let state = self.state.lock().expect("FakeStore 锁");
        Ok(state
            .turns
            .get(session_id)
            .and_then(|rows| rows.iter().map(|r| r.seq).max().map(|m| m + 1))
            .unwrap_or(0))
    }

    async fn create_task(&self, t: &Task) -> Result<(), StoreError> {
        self.calls.record(
            "create_task",
            kwargs! {
                "task_id" => json!(t.id),
                "session_id" => json!(t.session_id),
                "task_no" => json!(t.task_no),
            },
        );
        let mut state = self.state.lock().expect("FakeStore 锁");
        if state.tasks.contains_key(&t.id) {
            return Err(StoreError::Other(format!("task {} 已存在", t.id)));
        }
        state.tasks.insert(t.id.clone(), t.clone());
        state.task_order.push(t.id.clone());
        Ok(())
    }

    async fn update_task(&self, t: &Task) -> Result<(), StoreError> {
        self.calls.record(
            "update_task",
            kwargs! {"task_id" => json!(t.id), "status" => json!(t.status.as_str())},
        );
        let mut state = self.state.lock().expect("FakeStore 锁");
        if !state.tasks.contains_key(&t.id) {
            state.task_order.push(t.id.clone());
        }
        state.tasks.insert(t.id.clone(), t.clone());
        Ok(())
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, StoreError> {
        self.calls
            .record("get_task", kwargs! {"task_id" => json!(task_id)});
        Ok(self
            .state
            .lock()
            .expect("FakeStore 锁")
            .tasks
            .get(task_id)
            .cloned())
    }

    async fn list_active_tasks(&self, chat_id: &str) -> Result<Vec<Task>, StoreError> {
        self.calls
            .record("list_active_tasks", kwargs! {"chat_id" => json!(chat_id)});
        let state = self.state.lock().expect("FakeStore 锁");
        let mut out: Vec<Task> = state
            .task_order
            .iter()
            .filter_map(|id| state.tasks.get(id))
            .filter(|t| {
                state
                    .sessions
                    .get(&t.session_id)
                    .is_some_and(|s| s.chat_id == chat_id)
                    && ACTIVE_TASK_STATUSES.contains(&t.status)
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    async fn next_task_no(&self, tenant_id: &str) -> Result<String, StoreError> {
        let n = {
            let mut state = self.state.lock().expect("FakeStore 锁");
            let n = state.counters.get(tenant_id).copied().unwrap_or(0) + 1;
            state.counters.insert(tenant_id.to_string(), n);
            n
        };
        self.calls.record(
            "next_task_no",
            kwargs! {"tenant_id" => json!(tenant_id), "n" => json!(n)},
        );
        encode_task_no(n).map_err(|e| StoreError::Other(e.to_string()))
    }

    async fn seen_event(&self, event_id: &str) -> Result<bool, StoreError> {
        let already = {
            let mut state = self.state.lock().expect("FakeStore 锁");
            !state.seen.insert(event_id.to_string())
        };
        self.calls.record(
            "seen_event",
            kwargs! {"event_id" => json!(event_id), "seen" => json!(already)},
        );
        Ok(already)
    }

    async fn recover_orphan_tasks(&self) -> Result<Vec<Task>, StoreError> {
        self.calls.record("recover_orphan_tasks", kwargs! {});
        let mut state = self.state.lock().expect("FakeStore 锁");
        let ids: Vec<String> = state
            .task_order
            .iter()
            .filter(|id| {
                state
                    .tasks
                    .get(*id)
                    .is_some_and(|t| ACTIVE_TASK_STATUSES.contains(&t.status))
            })
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(task) = state.tasks.get_mut(&id) {
                task.status = aite_contracts::TaskStatus::Failed;
                task.result_summary = "上一次运行没有正常收尾".to_string();
                task.updated_at = Utc::now();
                out.push(task.clone());
            }
        }
        Ok(out)
    }
}

// --------------------------------------------------------------------------

/// `EvidenceWriter` 的内存替身；hash 链用契约里的函数真算。
pub struct FakeEvidenceWriter {
    pub calls: CallLog,
    state: Mutex<EvidenceState>,
}

#[derive(Default)]
struct EvidenceState {
    chains: BTreeMap<String, Vec<EvidenceEvent>>,
    order: Vec<String>,
    manifests: BTreeMap<String, Map<String, Value>>,
}

impl Default for FakeEvidenceWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeEvidenceWriter {
    pub fn new() -> Self {
        Self {
            calls: CallLog::new(),
            state: Mutex::new(EvidenceState::default()),
        }
    }

    /// 某个任务的整条链（拷贝）。
    pub fn chain(&self, task_id: &str) -> Vec<EvidenceEvent> {
        self.state
            .lock()
            .expect("FakeEvidence 锁")
            .chains
            .get(task_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 全部任务的链，按首次写入的先后。
    pub fn chains(&self) -> Vec<(String, Vec<EvidenceEvent>)> {
        let state = self.state.lock().expect("FakeEvidence 锁");
        state
            .order
            .iter()
            .filter_map(|id| state.chains.get(id).map(|c| (id.clone(), c.clone())))
            .collect()
    }

    pub fn task_ids(&self) -> Vec<String> {
        self.state.lock().expect("FakeEvidence 锁").order.clone()
    }

    pub fn total_events(&self) -> usize {
        self.state
            .lock()
            .expect("FakeEvidence 锁")
            .chains
            .values()
            .map(Vec::len)
            .sum()
    }

    pub fn manifest(&self, task_id: &str) -> Option<Map<String, Value>> {
        self.state
            .lock()
            .expect("FakeEvidence 锁")
            .manifests
            .get(task_id)
            .cloned()
    }

    pub fn kinds(&self, task_id: &str) -> Vec<String> {
        self.chain(task_id)
            .iter()
            .map(|e| e.kind.as_str().to_string())
            .collect()
    }

    /// 测试用：篡改某一条的 payload，好验 `verify` 抓不抓得住。
    pub fn tamper(&self, task_id: &str, seq: usize, key: &str, value: Value) {
        let mut state = self.state.lock().expect("FakeEvidence 锁");
        if let Some(chain) = state.chains.get_mut(task_id)
            && let Some(ev) = chain.get_mut(seq)
            && let Some(payload) = ev.payload.as_mut()
        {
            payload.insert(key.to_string(), value);
        }
    }
}

#[async_trait]
impl EvidenceWriter for FakeEvidenceWriter {
    async fn append(
        &self,
        task_id: &str,
        kind: EvidenceKind,
        payload: Map<String, Value>,
    ) -> Result<EvidenceEvent, EvidenceError> {
        self.calls.record(
            "append",
            kwargs! {"task_id" => json!(task_id), "kind" => json!(kind.as_str())},
        );
        let mut state = self.state.lock().expect("FakeEvidence 锁");
        if !state.chains.contains_key(task_id) {
            state.order.push(task_id.to_string());
        }
        let chain = state.chains.entry(task_id.to_string()).or_default();
        let prev = chain
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| GENESIS.to_string());
        let ph = payload_hash_of(&payload);
        let ev = EvidenceEvent {
            task_id: task_id.to_string(),
            seq: chain.len() as u64,
            kind,
            payload_hash: ph.clone(),
            payload_ref: None,
            payload: Some(payload),
            prev_hash: prev.clone(),
            hash: chain_hash(&prev, &ph),
            created_at: Utc::now(),
        };
        chain.push(ev.clone());
        Ok(ev)
    }

    async fn finalize(
        &self,
        task_id: &str,
        manifest_extra: Map<String, Value>,
    ) -> Result<String, EvidenceError> {
        let (root, count) = {
            let state = self.state.lock().expect("FakeEvidence 锁");
            let chain = state.chains.get(task_id);
            (
                chain
                    .and_then(|c| c.last())
                    .map(|e| e.hash.clone())
                    .unwrap_or_else(|| GENESIS.to_string()),
                chain.map(Vec::len).unwrap_or(0),
            )
        };
        self.calls.record(
            "finalize",
            kwargs! {"task_id" => json!(task_id), "root_hash" => json!(root)},
        );
        let mut manifest = kwargs! {
            "task_id" => json!(task_id),
            "root_hash" => json!(root),
            "event_count" => json!(count),
        };
        for (k, v) in manifest_extra {
            manifest.insert(k, v);
        }
        self.state
            .lock()
            .expect("FakeEvidence 锁")
            .manifests
            .insert(task_id.to_string(), manifest);
        Ok(root)
    }

    fn verify(&self, task_id: &str) -> bool {
        self.calls
            .record("verify", kwargs! {"task_id" => json!(task_id)});
        let state = self.state.lock().expect("FakeEvidence 锁");
        let mut prev = GENESIS.to_string();
        for (i, ev) in state
            .chains
            .get(task_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            if ev.seq != i as u64 || ev.prev_hash != prev {
                return false;
            }
            if let Some(payload) = &ev.payload
                && payload_hash_of(payload) != ev.payload_hash
            {
                return false;
            }
            if chain_hash(&prev, &ev.payload_hash) != ev.hash {
                return false;
            }
            prev = ev.hash.clone();
        }
        true
    }

    fn task_dir(&self, task_id: &str) -> PathBuf {
        PathBuf::from("memory://evidence").join(task_id)
    }
}
