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

    let before_migrate = stamp(Utc::now());
    let store = open_store(&db).await;
    let after_migrate = stamp(Utc::now());

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

    // 迁移 2 的回填：tasks 查询列 = JSON 里的值，chat_id = 会话的 chat_id
    let raw = Connection::open(&db).unwrap();
    let cols = |raw: &Connection, id: &str| -> (Option<String>, f64, i64, i64) {
        raw.query_row(
            "SELECT chat_id, cost, tokens_in, tokens_out FROM tasks WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    };
    assert_eq!(
        cols(&raw, "tsk_1"),
        (Some(CHAT.to_string()), 0.375, 1200, 345)
    );
    // 存量去重键回填成迁移时刻，一条 NULL 都不留（NULL 永远剪不掉）
    let seen_at: Vec<Option<String>> = raw
        .prepare("SELECT seen_at FROM seen_events WHERE event_id IN ('ev-1', 'ev-2')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(seen_at.len(), 2);
    for s in &seen_at {
        let s = s.as_deref().expect("存量行 seen_at 不许是 NULL");
        assert!(
            before_migrate.as_str() <= s && s <= after_migrate.as_str(),
            "seen_at 应是迁移时刻：{s} 不在 [{before_migrate}, {after_migrate}]"
        );
    }
    let names = sqlite_master_names(&db);
    for want in [
        "schema_version",
        "message_index",
        "idx_seen_events_seen_at",
        "idx_tasks_chat_created",
    ] {
        assert!(names.contains(&want.to_string()), "少了 {want}：{names:?}");
    }

    // 写时同步：update_task 改 cost / tokens，新列跟着走；create_task 新任务也写全
    let mut t = task;
    t.cost = 1.5;
    t.tokens_in = 2000;
    t.tokens_out = 500;
    store.update_task(&t).await.unwrap();
    assert_eq!(
        cols(&raw, "tsk_1"),
        (Some(CHAT.to_string()), 1.5, 2000, 500)
    );
    let mut fresh = make_task("tsk_2", "ses_1", "#A2", "新任务");
    fresh.cost = 0.125;
    fresh.tokens_in = 7;
    fresh.tokens_out = 9;
    store.create_task(&fresh).await.unwrap();
    assert_eq!(cols(&raw, "tsk_2"), (Some(CHAT.to_string()), 0.125, 7, 9));
    drop(raw);
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

#[tokio::test]
async fn seen_events_prune() {
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    let store = open_store(&db).await;
    for id in ["old-1", "old-2", "old-3", "new-1", "new-2"] {
        assert!(!store.seen_event(id).await.unwrap());
    }
    // seen_event 自己写了 seen_at（非 NULL）；把旧那批挪到很久以前
    let raw = Connection::open(&db).unwrap();
    let nulls: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM seen_events WHERE seen_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(nulls, 0, "seen_event 必须写 seen_at");
    raw.execute(
        "UPDATE seen_events SET seen_at = ?1 WHERE event_id LIKE 'old-%'",
        params![stamp(common::at(0))],
    )
    .unwrap();
    drop(raw);

    let cutoff = Utc::now() - chrono::Duration::hours(1);
    assert_eq!(store.prune_seen_events_before(cutoff).await.unwrap(), 3);
    assert_eq!(
        store.prune_seen_events_before(cutoff).await.unwrap(),
        0,
        "再剪一次没东西可剪"
    );

    assert!(
        !store.seen_event("old-1").await.unwrap(),
        "剪掉的键重新记录"
    );
    assert!(
        store.seen_event("old-1").await.unwrap(),
        "重新记录之后又认得了"
    );
    assert!(store.seen_event("new-1").await.unwrap(), "新的一批没被剪");
    assert!(store.seen_event("new-2").await.unwrap(), "新的一批没被剪");
    store.close().await.unwrap();
}

#[tokio::test]
async fn message_index_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let db = db_path(&tmp);
    let store = open_store(&db).await;

    let t0 = Utc::now();
    // 入站：群里 @ 的那条根消息，还没建任务
    store
        .index_message(CHAT, "om_in", "ses_1", None, false)
        .await
        .unwrap();
    // 出站：我们回的那条，挂在任务上
    store
        .index_message(CHAT, "om_out", "ses_1", Some("tsk_1"), true)
        .await
        .unwrap();
    let t1 = Utc::now();

    let inbound = store
        .find_session_by_message(CHAT, "om_in")
        .await
        .unwrap()
        .expect("入站那条");
    assert_eq!(inbound.chat_id, CHAT);
    assert_eq!(inbound.message_id, "om_in");
    assert_eq!(inbound.session_id, "ses_1");
    assert_eq!(inbound.task_id, None);
    assert!(!inbound.outbound);
    assert!(t0 <= inbound.created_at && inbound.created_at <= t1);

    let outbound = store
        .find_session_by_message(CHAT, "om_out")
        .await
        .unwrap()
        .expect("出站那条");
    assert_eq!(outbound.message_id, "om_out");
    assert_eq!(outbound.session_id, "ses_1");
    assert_eq!(outbound.task_id.as_deref(), Some("tsk_1"));
    assert!(outbound.outbound);

    // 查不存在的键
    assert_eq!(
        store
            .find_session_by_message(CHAT, "om_nope")
            .await
            .unwrap(),
        None
    );
    // 跨 chat 同 message_id 互不串
    assert_eq!(
        store
            .find_session_by_message("oc_other", "om_in")
            .await
            .unwrap(),
        None
    );
    store
        .index_message("oc_other", "om_in", "ses_2", Some("tsk_9"), true)
        .await
        .unwrap();
    let other = store
        .find_session_by_message("oc_other", "om_in")
        .await
        .unwrap()
        .expect("另一个群的同号消息");
    assert_eq!(other.session_id, "ses_2");
    assert_eq!(
        store
            .find_session_by_message(CHAT, "om_in")
            .await
            .unwrap()
            .expect("原来那条")
            .session_id,
        "ses_1",
        "另一个群的索引把这边的串了"
    );

    // 重复索引：不报错、保留首条（不覆盖）
    store
        .index_message(CHAT, "om_in", "ses_X", Some("tsk_X"), true)
        .await
        .expect("重复索引不许报错");
    assert_eq!(
        store.find_session_by_message(CHAT, "om_in").await.unwrap(),
        Some(inbound),
        "重复索引要保留首条"
    );
    store.close().await.unwrap();
}
