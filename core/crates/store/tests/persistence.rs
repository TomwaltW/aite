//! B6：进程重启后仍能续接（移植 `tests/control/test_persistence.py` 6 条）。
//!
//! 判据原文：建会话 + 两轮 turn → 用**同一 SQLite 文件**新建第二个实例
//! → 同线程追问命中同一 session_id，list_turns 含此前两轮。
//!
//! Python 那边靠 `InProcessControlPlane.handle_event` 顺手把会话/任务造出来；
//! ControlPlane 是 R4 的活，这里改成直接往 store 里写 —— 被验的是 store 的约定，
//! 不是路由。

mod common;

use aite_contracts::{SessionStore, StoreError, TaskStatus, encode_task_no};
use common::{CHAT, ROOT, make_session, make_task, make_turn, open_store, seed_task};
use tempfile::TempDir;

fn db_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("data").join("aite.db")
}

async fn contents(store: &aite_store::SqliteSessionStore, session_id: &str) -> Vec<String> {
    store
        .list_turns(session_id, 200)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.content)
        .collect()
}

#[tokio::test]
async fn b6_restart_resumes_same_session_and_turns() {
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);

    // —— 第一个实例：建会话 + 两轮 turn ——
    let store1 = open_store(&db).await;
    let session = make_session("ses_1", CHAT, Some(ROOT));
    store1.create_session(&session).await.unwrap();
    let first_task_no = store1.next_task_no("default").await.unwrap();
    store1
        .create_task(&make_task("tsk_1", "ses_1", &first_task_no, "第一问"))
        .await
        .unwrap();
    store1
        .append_turn(&make_turn("ses_1", 0, "第一问"))
        .await
        .unwrap();
    store1
        .append_turn(&make_turn("ses_1", 1, "第二问"))
        .await
        .unwrap();
    assert_eq!(contents(&store1, "ses_1").await, ["第一问", "第二问"]);
    store1.close().await.unwrap();

    // —— 换一个实例，同一个文件 ——
    let store2 = open_store(&db).await;
    let hit = store2.find_session_by_thread(CHAT, ROOT).await.unwrap();
    assert_eq!(hit.expect("命中同一 session").id, "ses_1");

    let seq = store2.next_turn_seq("ses_1").await.unwrap();
    assert_eq!(seq, 2);
    store2
        .append_turn(&make_turn("ses_1", seq, "第三问"))
        .await
        .unwrap();
    let turns = store2.list_turns("ses_1", 200).await.unwrap();
    assert_eq!(
        turns.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(),
        ["第一问", "第二问", "第三问"]
    );
    assert_eq!(turns.iter().map(|t| t.seq).collect::<Vec<_>>(), [0, 1, 2]);

    // 任务号计数器也是持久的
    let new_task_no = store2.next_task_no("default").await.unwrap();
    assert_ne!(new_task_no, first_task_no);
    store2
        .create_task(&make_task("tsk_2", "ses_1", &new_task_no, "第三问"))
        .await
        .unwrap();
    let active = store2.list_active_tasks(CHAT).await.unwrap();
    assert_eq!(active.last().unwrap().session_id, "ses_1");
    store2.close().await.unwrap();
}

#[tokio::test]
async fn seen_event_survives_restart() {
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);

    let store1 = open_store(&db).await;
    assert!(!store1.seen_event("ev-dup").await.unwrap());
    store1.close().await.unwrap();

    let store2 = open_store(&db).await;
    assert!(
        store2.seen_event("ev-dup").await.unwrap(),
        "换实例后重推同一 event 仍然被丢"
    );
    store2.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn task_no_is_atomic_per_tenant() {
    let tmp = TempDir::new().unwrap();
    let store = std::sync::Arc::new(open_store(db_path(&tmp)).await);

    let mut handles = Vec::new();
    for _ in 0..32 {
        let s = store.clone();
        handles.push(tokio::spawn(async move { s.next_task_no("default").await }));
    }
    let mut nos = Vec::new();
    for h in handles {
        nos.push(h.await.unwrap().unwrap());
    }
    let got: std::collections::BTreeSet<_> = nos.into_iter().collect();
    let want: std::collections::BTreeSet<_> =
        (1..=32).map(|i| encode_task_no(i).unwrap()).collect();
    assert_eq!(got, want);

    // 计数器按租户隔离
    assert_eq!(store.next_task_no("other-tenant").await.unwrap(), "#A1");
}

#[tokio::test]
async fn duplicate_turn_seq_raises() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;

    let t = make_turn("s1", 0, "hi");
    store.append_turn(&t).await.unwrap();
    match store.append_turn(&t).await {
        Err(StoreError::DuplicateTurn { session_id, seq }) => {
            assert_eq!((session_id.as_str(), seq), ("s1", 0));
        }
        other => panic!("应当报 DuplicateTurn，实际 {other:?}"),
    }
}

#[tokio::test]
async fn list_active_tasks_scope() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;

    seed_task(&store, "ses_a", "tsk_a", CHAT, "甲").await;
    seed_task(&store, "ses_b", "tsk_b", "oc_other", "乙").await;
    assert_eq!(
        store
            .list_active_tasks(CHAT)
            .await
            .unwrap()
            .iter()
            .map(|t| t.title.clone())
            .collect::<Vec<_>>(),
        ["甲"]
    );

    let mut task = store.list_active_tasks(CHAT).await.unwrap().remove(0);
    for done in [
        TaskStatus::Delivered,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::Answering,
    ] {
        task.status = done;
        store.update_task(&task).await.unwrap();
        assert!(
            store.list_active_tasks(CHAT).await.unwrap().is_empty(),
            "{done} 不该算活跃"
        );
    }
    for live in [
        TaskStatus::Created,
        TaskStatus::Planning,
        TaskStatus::Working,
    ] {
        task.status = live;
        store.update_task(&task).await.unwrap();
        assert_eq!(
            store.list_active_tasks(CHAT).await.unwrap().len(),
            1,
            "{live} 该算活跃"
        );
    }
}

#[tokio::test]
async fn next_turn_seq() {
    let tmp = TempDir::new().unwrap();
    let store = open_store(db_path(&tmp)).await;

    assert_eq!(store.next_turn_seq("s9").await.unwrap(), 0);
    store.append_turn(&make_turn("s9", 0, "x")).await.unwrap();
    assert_eq!(store.next_turn_seq("s9").await.unwrap(), 1);
}
