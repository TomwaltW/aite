//! T18：崩溃恢复里属于 store 的那几条（移植 `tests/integration/test_t18_crash_recovery.py`
//! 中与 SessionStore 相关的用例；evidence 那一半在 aite-evidence 的 torn_tail 测试里）。
//!
//! **怎么模拟崩溃**：不真 `kill -9`（测试里不好控），而是「不走收尾路径」——
//! 把状态造出来，然后一步收尾都不走，只把 fd 关掉（等价于 OS 回收进程的 fd），
//! 再在同一个 .db 文件上重开一个 store。

mod common;

use std::sync::Arc;

use aite_contracts::{SessionStore, StoreError, TaskStatus};
use aite_store::{ORPHAN_RESULT_SUMMARY, SqliteSessionStore};
use common::{CHAT, ROOT, make_turn, open_store, seed_task};
use tempfile::TempDir;

fn db_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("data").join("aite.db")
}

/// 造一个「正在跑」的任务，然后崩溃。返回 (task_id, task_no)。
/// 收尾一步都不走：没有 recover、没有 finalize，只关 fd。
async fn crash_with_a_task_in_flight(db: &std::path::Path) -> (String, String) {
    let store = open_store(db).await;
    let task_no = seed_task(&store, "ses_1", "tsk_1", CHAT, "画个图").await;
    store
        .append_turn(&make_turn("ses_1", 0, "画个图"))
        .await
        .unwrap();

    let mut task = store.list_active_tasks(CHAT).await.unwrap().remove(0);
    task.status = TaskStatus::Working; // 崩溃那一刻它正在干活
    store.update_task(&task).await.unwrap();

    store.close().await.unwrap(); // 只关 fd，等价 OS 回收；收尾序列全跳过
    (task.id, task_no)
}

#[tokio::test]
async fn crash_leaves_a_zombie_task_hanging_on_status() {
    // 钉住现状（这是个真问题）：崩溃时在跑的任务，重开后还是 working ——
    // `list_active_tasks` 认 created/planning/working，起飞路径上又没有人收拾。
    let tmp = TempDir::new().unwrap();
    let (task_id, _) = crash_with_a_task_in_flight(&db_path(&tmp)).await;

    let store2 = open_store(db_path(&tmp)).await;
    let still_there = store2.list_active_tasks(CHAT).await.unwrap();
    assert_eq!(
        still_there.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        [task_id]
    );
    assert_eq!(still_there[0].status, TaskStatus::Working, "没人动过它");
    store2.close().await.unwrap();
}

#[tokio::test]
async fn recover_orphan_tasks_clears_the_zombie_after_crash() {
    let tmp = TempDir::new().unwrap();
    let (task_id, task_no) = crash_with_a_task_in_flight(&db_path(&tmp)).await;

    let store2 = open_store(db_path(&tmp)).await;
    let orphans = store2.recover_orphan_tasks().await.unwrap();
    assert_eq!(
        orphans.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        [task_id]
    );
    assert_eq!(orphans[0].status, TaskStatus::Failed);
    assert_eq!(orphans[0].task_no, task_no, "回帖要用它，得带得出来");
    assert_eq!(orphans[0].result_summary, ORPHAN_RESULT_SUMMARY);
    assert!(store2.list_active_tasks(CHAT).await.unwrap().is_empty());
    store2.close().await.unwrap();
}

#[tokio::test]
async fn journal_mode_is_delete_and_crash_leaves_no_residue() {
    // P0 的 journal 模式是 `delete`（不是 WAL）：崩溃后不留 `-wal` / `-journal`。
    // 哪天有人改成 WAL，部署面要跟着管两个新文件。
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);

    let store = open_store(&db).await;
    assert_eq!(store.pragma("journal_mode").await.unwrap(), "delete");
    store.close().await.unwrap();

    crash_with_a_task_in_flight(&db).await;

    let prefix = format!("{}-", db.file_name().unwrap().to_string_lossy());
    let residue: Vec<String> = std::fs::read_dir(db.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(&prefix))
        .collect();
    assert!(residue.is_empty(), "崩溃后有残留：{residue:?}");
}

#[tokio::test]
async fn data_written_before_the_crash_survives() {
    // 崩溃前 commit 过的东西一个字都不许丢，计数器也接着往下走。
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    let (task_id, task_no) = crash_with_a_task_in_flight(&db).await;
    assert_eq!(task_no, "#A1");

    let store = open_store(&db).await;
    let task = store.get_task(&task_id).await.unwrap().expect("任务还在");
    assert_eq!(task.status, TaskStatus::Working);

    assert!(
        store
            .find_session_by_thread(CHAT, ROOT)
            .await
            .unwrap()
            .is_some(),
        "话题锚点还在"
    );
    assert_eq!(
        store
            .list_turns(&task.session_id, 200)
            .await
            .unwrap()
            .iter()
            .map(|t| t.content.clone())
            .collect::<Vec<_>>(),
        ["画个图"]
    );
    assert_eq!(
        store.next_task_no("default").await.unwrap(),
        "#A2",
        "号接着发，不回退"
    );
    store.close().await.unwrap();

    // 独立连接（完全另一条 fd）也读得到同样的东西 —— 不是缓存里的幻觉
    let raw = rusqlite::Connection::open(&db).unwrap();
    let n: i64 = raw
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn crash_during_concurrent_writes_keeps_the_db_readable() {
    // 一堆并发写正在飞的时候崩溃：重开后库必须是完好的（不是半个事务）。
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    let store = Arc::new(open_store(&db).await);

    let mut handles = Vec::new();
    for i in 0..50 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            s.seen_event(&format!("ev-{i}")).await.unwrap()
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let mut handles = Vec::new();
    for _ in 0..50 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            s.next_task_no("default").await.unwrap()
        }));
    }
    let mut nos = Vec::new();
    for h in handles {
        nos.push(h.await.unwrap());
    }
    store.close().await.unwrap(); // 崩溃：没有任何收尾

    let reopened = open_store(&db).await;
    assert_eq!(reopened.pragma("integrity_check").await.unwrap(), "ok");
    assert!(reopened.seen_event("ev-0").await.unwrap(), "去重键全在");
    let next = reopened.next_task_no("default").await.unwrap();
    assert!(!nos.contains(&next), "号不回退、不重发：{next}");
    reopened.close().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn store_failure_surfaces_as_an_exception_not_a_silent_false() {
    // 存储层写失败必须**报错**，不许假装成 `seen_event() -> false`。
    // 静默返回 false 的话，重连风暴里每条重复事件都会被当成新的 —— 同一个活干很多遍。
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    let store = open_store(&db).await;
    assert!(!store.seen_event("before-failure").await.unwrap());

    let dir = db.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();

    let seen = store.seen_event("during-failure").await;
    let no = store.next_task_no("default").await;

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(
        matches!(seen, Err(StoreError::Sqlite(_))),
        "只读目录下 seen_event 必须 Err，实际 {seen:?}"
    );
    assert!(
        matches!(no, Err(StoreError::Sqlite(_))),
        "只读目录下 next_task_no 必须 Err，实际 {no:?}"
    );

    assert!(
        !store.seen_event("after-recovery").await.unwrap(),
        "恢复后照常"
    );
    assert!(
        store.seen_event("before-failure").await.unwrap(),
        "之前记的还在"
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn closed_store_reports_not_initialized() {
    // close 之后再调任何方法都是 NotInitialized，不是 panic、也不是静默成功。
    let tmp = TempDir::new().unwrap();
    let store = SqliteSessionStore::open(db_path(&tmp)).unwrap();
    store.init().await.unwrap();
    store.close().await.unwrap();
    store.close().await.unwrap(); // 幂等

    assert!(matches!(
        store.get_session("x").await,
        Err(StoreError::NotInitialized)
    ));
    assert!(matches!(
        store.seen_event("x").await,
        Err(StoreError::NotInitialized)
    ));
    assert!(matches!(
        store.next_task_no("default").await,
        Err(StoreError::NotInitialized)
    ));
    assert!(matches!(
        store.recover_orphan_tasks().await,
        Err(StoreError::NotInitialized)
    ));
    assert!(matches!(
        store.list_turns("x", 10).await,
        Err(StoreError::NotInitialized)
    ));
}
