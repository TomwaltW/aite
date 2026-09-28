//! CC5：schema 迁移器（`schema_version` 表 + 有序、事务化的迁移清单）。
//!
//! 夹具一律用**抄下来的** P0 建表 SQL 造库（[`P0_SCHEMA`]），不引用 lib 里的迁移 1 ——
//! 以后谁改了迁移 1，这里的「P0 老库」不会跟着悄悄变。

mod common;

use std::path::{Path, PathBuf};

use aite_contracts::{SessionStore, StoreError};
use aite_store::{LATEST_SCHEMA_VERSION, SqliteSessionStore};
use chrono::{DateTime, SecondsFormat, Utc};
use common::{CHAT, ROOT, make_session, make_task, make_turn, open_store};
use rusqlite::{Connection, params};
use tempfile::TempDir;

/// P0 的建表 SQL，逐字抄自 `8458435` 的 `core/crates/store/src/lib.rs` `SCHEMA`。
const P0_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    tenant_id   TEXT NOT NULL,
    chat_id     TEXT NOT NULL,
    thread_id   TEXT,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_thread ON sessions (chat_id, thread_id);

CREATE TABLE IF NOT EXISTS turns (
    session_id  TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    data        TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
);

CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    task_no     TEXT NOT NULL,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_session ON tasks (session_id);

CREATE TABLE IF NOT EXISTS task_counters (
    tenant_id   TEXT PRIMARY KEY,
    n           INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS seen_events (
    event_id    TEXT PRIMARY KEY
);
";

fn db_path(dir: &TempDir) -> PathBuf {
    dir.path().join("aite.db")
}

/// 与 store 的冗余列同一格式（定长 6 位小数 + `Z`）。
fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// `schema_version` 表里的全部版本号，正序。
fn version_rows(db: &Path) -> Vec<i64> {
    let c = Connection::open(db).unwrap();
    let mut stmt = c
        .prepare("SELECT version FROM schema_version ORDER BY version")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<i64>, _>>()
        .unwrap()
}

fn all_versions() -> Vec<i64> {
    (1..=LATEST_SCHEMA_VERSION).collect()
}

fn sqlite_master_names(db: &Path) -> Vec<String> {
    let c = Connection::open(db).unwrap();
    let mut stmt = c.prepare("SELECT name FROM sqlite_master").unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<String>, _>>()
        .unwrap()
}

/// 用裸连接按 P0 schema 造一份「跑过一阵子」的老库：一个会话、一个花过钱的任务、
/// 两轮 turn、计数器、两条去重键。返回写进去的会话与任务。
fn build_p0_db(db: &Path) -> (aite_contracts::Session, aite_contracts::Task) {
    let c = Connection::open(db).unwrap();
    c.execute_batch(P0_SCHEMA).unwrap();

    let s = make_session("ses_1", CHAT, Some(ROOT));
    c.execute(
        "INSERT INTO sessions (id, tenant_id, chat_id, thread_id, status, created_at, data) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            s.id,
            s.tenant_id,
            s.chat_id,
            s.anchor.thread_id,
            s.status.as_str(),
            stamp(s.created_at),
            serde_json::to_string(&s).unwrap()
        ],
    )
    .unwrap();

    let mut t = make_task("tsk_1", "ses_1", "#A1", "老任务");
    t.cost = 0.375;
    t.tokens_in = 1200;
    t.tokens_out = 345;
    c.execute(
        "INSERT INTO tasks (id, session_id, task_no, status, created_at, data) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            t.id,
            t.session_id,
            t.task_no,
            t.status.as_str(),
            stamp(t.created_at),
            serde_json::to_string(&t).unwrap()
        ],
    )
    .unwrap();

    for (seq, text) in [(0u64, "第一问"), (1, "第二问")] {
        let turn = make_turn("ses_1", seq, text);
        c.execute(
            "INSERT INTO turns (session_id, seq, data) VALUES (?1, ?2, ?3)",
            params![
                turn.session_id,
                turn.seq as i64,
                serde_json::to_string(&turn).unwrap()
            ],
        )
        .unwrap();
    }
    c.execute(
        "INSERT INTO task_counters (tenant_id, n) VALUES ('default', 1)",
        [],
    )
    .unwrap();
    c.execute_batch("INSERT INTO seen_events (event_id) VALUES ('ev-1'), ('ev-2');")
        .unwrap();
    (s, t)
}

#[tokio::test]
async fn migrates_p0_db_in_place() {
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    let (session, task) = build_p0_db(&db);
    assert!(
        !sqlite_master_names(&db).contains(&"schema_version".to_string()),
        "夹具必须是没有版本表的 P0 库"
    );

    let store = open_store(&db).await;

    assert_eq!(version_rows(&db), all_versions());
    // 老数据一个字不丢
    assert_eq!(store.get_session("ses_1").await.unwrap(), Some(session));
    assert_eq!(store.get_task("tsk_1").await.unwrap(), Some(task.clone()));
    assert_eq!(
        store
            .list_turns("ses_1", 10)
            .await
            .unwrap()
            .iter()
            .map(|t| t.content.clone())
            .collect::<Vec<_>>(),
        ["第一问", "第二问"]
    );
    assert_eq!(
        store
            .find_session_by_thread(CHAT, ROOT)
            .await
            .unwrap()
            .map(|s| s.id),
        Some("ses_1".to_string())
    );
    assert!(store.seen_event("ev-1").await.unwrap(), "老去重键还认");
    assert!(store.seen_event("ev-2").await.unwrap(), "老去重键还认");
    assert_eq!(
        store.next_task_no("default").await.unwrap(),
        "#A2",
        "号接着发"
    );
    assert_eq!(store.pragma("journal_mode").await.unwrap(), "delete");
    store.close().await.unwrap();
}

#[tokio::test]
async fn migration_idempotent() {
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    build_p0_db(&db);

    let store = open_store(&db).await; // 第一次：真迁
    assert_eq!(version_rows(&db), all_versions());

    // 同一实例第二次 init：零写入，文件字节完全相同
    let before = std::fs::read(&db).unwrap();
    store.init().await.expect("同一实例第二次 init");
    assert_eq!(std::fs::read(&db).unwrap(), before, "第二次 init 写了库");

    // 同一文件、第二个实例（前一个还开着 —— app 测试里读盘就是这个形状）。
    // 旁边另有连接正持着写锁（RESERVED）：已是最新版本的 init 只读不写，必须立刻过，
    // 而不是去抢写锁、在 busy_timeout 里干等 5 秒再报 SQLITE_BUSY。
    let writer = Connection::open(&db).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let other = SqliteSessionStore::open(&db).unwrap();
    let before = std::fs::read(&db).unwrap();
    let t0 = std::time::Instant::now();
    other
        .init()
        .await
        .expect("第二个实例 init（旁边有人持写锁）");
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(2),
        "已是最新版本的 init 去抢写锁了：{:?}",
        t0.elapsed()
    );
    assert_eq!(
        std::fs::read(&db).unwrap(),
        before,
        "第二个实例的 init 写了库"
    );
    writer.execute_batch("ROLLBACK;").unwrap();
    drop(writer);

    assert_eq!(version_rows(&db), all_versions());
    let task = other.get_task("tsk_1").await.unwrap().expect("任务还在");
    assert_eq!(task.task_no, "#A1");
    assert_eq!(other.list_turns("ses_1", 10).await.unwrap().len(), 2);
    other.close().await.unwrap();
    store.close().await.unwrap();
}

#[tokio::test]
async fn refuses_newer_schema_version() {
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    open_store(&db).await.close().await.unwrap();

    let newer = LATEST_SCHEMA_VERSION + 1;
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            params![newer, stamp(Utc::now())],
        )
        .unwrap();

    let before = std::fs::read(&db).unwrap();
    let store = SqliteSessionStore::open(&db).unwrap();
    let err = store.init().await.expect_err("库比程序新，必须拒绝");
    assert!(matches!(err, StoreError::Other(_)), "{err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains(&newer.to_string()) && msg.contains(&LATEST_SCHEMA_VERSION.to_string()),
        "报错要带上两个版本号：{msg}"
    );
    assert_eq!(std::fs::read(&db).unwrap(), before, "拒绝之前写了库");
    store.close().await.unwrap();
}
