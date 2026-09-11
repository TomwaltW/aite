//! FakeSessionStore / FakeEvidenceWriter 自测
//! （移植自 `tests/e2e/test_t4_fake_store.py`，12 条）。
use aite_contracts::{
    Anchor, EvidenceKind, EvidenceWriter, GENESIS, Session, SessionKind, SessionStatus,
    SessionStore, StoreError, Task, TaskStatus, Turn, TurnRole, chain_hash, payload_hash_of,
};
use aite_testing::{FakeEvidenceWriter, FakeSessionStore};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 9, 9, 0, 0).unwrap()
}

fn anchor(message_id: &str, thread_id: Option<&str>) -> Anchor {
    Anchor {
        platform: "fake".into(),
        chat_id: "oc_1".into(),
        message_id: message_id.into(),
        thread_id: thread_id.map(String::from),
        task_no: None,
    }
}

fn session(sid: &str, thread_id: Option<&str>, chat_id: &str) -> Session {
    Session {
        id: sid.into(),
        tenant_id: "default".into(),
        workspace_id: "cli_app".into(),
        chat_id: chat_id.into(),
        kind: SessionKind::Task,
        anchor: anchor("om_1", thread_id),
        status: SessionStatus::Active,
        created_by: "ou_alice".into(),
        config_snapshot: serde_json::Map::new(),
        created_at: now(),
        last_active_at: now(),
        archived_at: None,
    }
}

fn task(tid: &str, sid: &str, status: TaskStatus, no: &str) -> Task {
    Task {
        id: tid.into(),
        session_id: sid.into(),
        task_no: no.into(),
        status,
        title: String::new(),
        checklist: Vec::new(),
        card_id: None,
        sandbox_id: None,
        session_token: "tok".into(),
        model: String::new(),
        steps: 0,
        tokens_in: 0,
        tokens_out: 0,
        cost: 0.0,
        max_steps: 40,
        max_wall_sec: 1200,
        result_summary: String::new(),
        evidence_root_hash: None,
        created_by: "ou_alice".into(),
        created_at: now(),
        updated_at: now(),
    }
}

fn turn(sid: &str, seq: u64, content: &str) -> Turn {
    Turn {
        session_id: sid.into(),
        seq,
        role: TurnRole::User,
        platform_user_id: Some("ou_alice".into()),
        content: content.into(),
        attachments: Vec::new(),
        created_at: now(),
    }
}

#[tokio::test]
async fn init_is_idempotent() {
    let s = FakeSessionStore::new();
    s.init().await.unwrap();
    s.init().await.unwrap();
    assert!(s.initialized());
    s.close().await.unwrap();
    assert!(s.closed());
}

#[tokio::test]
async fn find_session_by_thread() {
    let s = FakeSessionStore::new();
    s.create_session(&session("s1", Some("om_1"), "oc_1"))
        .await
        .unwrap();
    assert_eq!(
        s.find_session_by_thread("oc_1", "om_1")
            .await
            .unwrap()
            .unwrap()
            .id,
        "s1"
    );
    assert!(
        s.find_session_by_thread("oc_1", "om_other")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.find_session_by_thread("oc_other", "om_1")
            .await
            .unwrap()
            .is_none()
    );
}

/// 契约：重复的 (session_id, seq) 报 `StoreError::DuplicateTurn`。
#[tokio::test]
async fn append_turn_rejects_duplicate_seq() {
    let s = FakeSessionStore::new();
    s.create_session(&session("s1", Some("om_1"), "oc_1"))
        .await
        .unwrap();
    let t = turn("s1", 0, "你好");
    s.append_turn(&t).await.unwrap();
    let err = s.append_turn(&t).await.unwrap_err();
    assert!(
        matches!(err, StoreError::DuplicateTurn { ref session_id, seq } if session_id == "s1" && seq == 0)
    );
}

#[tokio::test]
async fn list_turns_is_seq_ordered_and_limited() {
    let s = FakeSessionStore::new();
    s.create_session(&session("s1", Some("om_1"), "oc_1"))
        .await
        .unwrap();
    for i in 0..5u64 {
        s.append_turn(&turn("s1", i, &i.to_string())).await.unwrap();
    }
    let rows = s.list_turns("s1", 2).await.unwrap();
    assert_eq!(
        rows.iter().map(|t| t.content.clone()).collect::<Vec<_>>(),
        vec!["3".to_string(), "4".to_string()]
    );
    assert_eq!(s.next_turn_seq("s1").await.unwrap(), 5);
    assert_eq!(s.next_turn_seq("s_none").await.unwrap(), 0);
}

/// 向量来自契约：1 → #A1、17 → #AH、32 → #A10。
#[tokio::test]
async fn next_task_no_uses_contract_encoding_and_is_per_tenant() {
    let s = FakeSessionStore::new();
    let mut got = Vec::new();
    for _ in 0..17 {
        got.push(s.next_task_no("default").await.unwrap());
    }
    assert_eq!(got[0], "#A1");
    assert_eq!(got[16], "#AH");
    assert_eq!(s.next_task_no("other").await.unwrap(), "#A1");
}

#[tokio::test]
async fn seen_event_is_false_once_then_true() {
    let s = FakeSessionStore::new();
    assert!(!s.seen_event("e1").await.unwrap());
    assert!(s.seen_event("e1").await.unwrap());
    assert_eq!(s.seen_count(), 1);
}

#[tokio::test]
async fn list_active_tasks_only_returns_active_of_this_chat() {
    let s = FakeSessionStore::new();
    s.create_session(&session("s1", Some("om_1"), "oc_1"))
        .await
        .unwrap();
    s.create_session(&session("s2", Some("om_2"), "oc_2"))
        .await
        .unwrap();
    s.create_task(&task("t_created", "s1", TaskStatus::Created, "#A1"))
        .await
        .unwrap();
    s.create_task(&task("t_working", "s1", TaskStatus::Working, "#A2"))
        .await
        .unwrap();
    s.create_task(&task("t_done", "s1", TaskStatus::Delivered, "#A3"))
        .await
        .unwrap();
    s.create_task(&task("t_other_chat", "s2", TaskStatus::Working, "#A4"))
        .await
        .unwrap();
    let ids: Vec<String> = s
        .list_active_tasks("oc_1")
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, vec!["t_created".to_string(), "t_working".to_string()]);
}

/// 存进去之后在外面改对象，不该改到库里那份。
#[tokio::test]
async fn stored_models_are_copies() {
    let s = FakeSessionStore::new();
    let mut t = task("t1", "s1", TaskStatus::Created, "#A1");
    s.create_task(&t).await.unwrap();
    t.status = TaskStatus::Failed;
    assert_eq!(
        s.get_task("t1").await.unwrap().unwrap().status,
        TaskStatus::Created
    );
}

/// 起飞时收残局：所有活跃态任务改 failed 并回报。
#[tokio::test]
async fn recover_orphan_tasks_fails_the_active_ones() {
    let s = FakeSessionStore::new();
    s.create_session(&session("s1", Some("om_1"), "oc_1"))
        .await
        .unwrap();
    s.create_task(&task("t_working", "s1", TaskStatus::Working, "#A1"))
        .await
        .unwrap();
    s.create_task(&task("t_done", "s1", TaskStatus::Delivered, "#A2"))
        .await
        .unwrap();
    let recovered = s.recover_orphan_tasks().await.unwrap();
    assert_eq!(
        recovered.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        ["t_working"]
    );
    assert_eq!(
        s.get_task("t_working").await.unwrap().unwrap().status,
        TaskStatus::Failed
    );
    assert_eq!(
        s.get_task("t_done").await.unwrap().unwrap().status,
        TaskStatus::Delivered
    );
}

#[tokio::test]
async fn evidence_chain_matches_contract_hashing() {
    let w = FakeEvidenceWriter::new();
    let p0 = aite_testing::kwargs! {"a" => json!(1)};
    let p1 = aite_testing::kwargs! {"b" => json!("文")};
    let e0 = w
        .append("t1", EvidenceKind::TaskCreated, p0.clone())
        .await
        .unwrap();
    let e1 = w
        .append("t1", EvidenceKind::Delivered, p1.clone())
        .await
        .unwrap();

    assert_eq!(e0.prev_hash, GENESIS);
    assert_eq!(e0.seq, 0);
    assert_eq!(e0.payload_hash, payload_hash_of(&p0));
    assert_eq!(e0.hash, chain_hash(GENESIS, &e0.payload_hash));
    assert_eq!(e1.prev_hash, e0.hash);
    assert_eq!(e1.seq, 1);
    assert!(w.verify("t1"));
    assert_eq!(
        w.kinds("t1"),
        vec!["task_created".to_string(), "delivered".to_string()]
    );
}

#[tokio::test]
async fn tampering_breaks_verify() {
    let w = FakeEvidenceWriter::new();
    w.append(
        "t1",
        EvidenceKind::TaskCreated,
        aite_testing::kwargs! {"a" => json!(1)},
    )
    .await
    .unwrap();
    assert!(w.verify("t1"));
    w.tamper("t1", 0, "a", json!(2));
    assert!(!w.verify("t1"));
}

#[tokio::test]
async fn finalize_returns_last_hash() {
    let w = FakeEvidenceWriter::new();
    w.append(
        "t1",
        EvidenceKind::TaskCreated,
        aite_testing::kwargs! {"a" => json!(1)},
    )
    .await
    .unwrap();
    let last = w
        .append(
            "t1",
            EvidenceKind::Delivered,
            aite_testing::kwargs! {"b" => json!(2)},
        )
        .await
        .unwrap();
    let root = w
        .finalize("t1", aite_testing::kwargs! {"task_no" => json!("#A1")})
        .await
        .unwrap();
    assert_eq!(root, last.hash);
    let manifest = w.manifest("t1").unwrap();
    assert_eq!(manifest["event_count"], json!(2));
    assert_eq!(manifest["task_no"], json!("#A1"));
    // 空链 finalize 回 GENESIS
    assert_eq!(
        w.finalize("t_empty", serde_json::Map::new()).await.unwrap(),
        GENESIS
    );
    assert!(w.task_dir("t1").ends_with("t1"));
}

// 让 `use` 行里那个别名不至于变成未使用导入（Rust 没有 Python 那种运行期协议检查，
// 这条只是把「两个替身都实现了契约 trait」钉在编译期）。
#[allow(dead_code)]
fn implements_the_two_ports() {
    fn takes_store<T: SessionStore>(_: &T) {}
    fn takes_writer<T: EvidenceWriter>(_: &T) {}
    takes_store(&FakeSessionStore::new());
    takes_writer(&FakeEvidenceWriter::new());
}
