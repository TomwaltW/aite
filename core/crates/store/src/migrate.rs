//! schema 迁移器：`schema_version` 表 + 按版本号严格递增的迁移清单。
//!
//! 说的是**本 crate 自己建的 `schema_version` 表**（一次迁移一行，当前版本 = `MAX(version)`），
//! 不是 SQLite 的两个同名 pragma：
//! - `PRAGMA user_version` 不用 —— 版本记在表里，带着每一步的落地时间，排障时看得见；
//! - `PRAGMA schema_version` 是 SQLite 内置的 schema cookie，**绝不许写**（写错会把库弄坏），
//!   `app/src/preflight.rs` 拿它当只读探针。表名与它同名只是巧合，两者不在一个命名空间里。
//!
//! `init()` 的流程（[`run`]）：
//!
//! ```text
//! 1. 快路径：普通 SELECT 读版本，不开写事务。= 最新 → 直接返回，零写入
//!    （app 测试在 app 连接还活着时另开 store 调 init()；已建完表的只读库上 init() 照样成功）
//! 2. > 最新 → 任何写之前拒绝（StoreError::Other，带两个版本号）
//! 3. < 最新 → BEGIN IMMEDIATE → 事务里重读版本（防两个实例同时起飞）→ 逐条迁移 + 写版本行 → COMMIT
//!    任一步失败 → 回滚（drop 事务），版本不变，错误原样上抛
//! ```
//!
//! **DD1 加迁移 3+ 的做法**：在 [`MIGRATIONS`] 末尾追加一条，同时把 [`LATEST_SCHEMA_VERSION`] 改成它的版本号。
//! 已发布的迁移一个字都不许改（老库已经跑过它，改了新旧库就分叉了）。
use chrono::Utc;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use aite_contracts::StoreError;

use crate::{sq, stamp};

/// 本程序认识的最新 schema 版本。库比它新 → `init()` 拒绝打开；比它旧 → 迁上来。
pub const LATEST_SCHEMA_VERSION: i64 = 2;

/// 一条迁移：在调用方给的事务里跑（`Transaction` 解引用成 `&Connection`），不自己 commit。
struct Migration {
    version: i64,
    apply: fn(&Connection) -> Result<(), StoreError>,
}

/// 迁移清单，版本号严格递增、从 1 起连续。
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        apply: m1_p0_schema,
    },
    Migration {
        version: 2,
        apply: m2_query_columns_and_message_index,
    },
];

/// 迁移 1 = P0 的建表 SQL，逐字照 `aite/control/store.py` 的 `_SCHEMA`。
/// 全是 `IF NOT EXISTS`，所以在 P0 老库上是空操作 —— 老库只多出一行版本记录。
const SCHEMA_V1: &str = "
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

fn m1_p0_schema(c: &Connection) -> Result<(), StoreError> {
    c.execute_batch(SCHEMA_V1).map_err(sq)
}

/// 迁移 2（CC5）：
/// - `seen_events.seen_at` + 索引：存量行回填成**迁移时刻**（`stamp()` 的定长格式，字典序 = 时间序；
///   不留 NULL —— NULL 永远不满足 `<`，会永远剪不掉）。SQLite 的 `ADD COLUMN` 不许拿
///   `CURRENT_TIMESTAMP` 当默认值，所以先加可空列再回填。
/// - `tasks` 的四个查询列 `chat_id / cost / tokens_in / tokens_out` + `(chat_id, created_at)` 索引：
///   `cost` / `tokens_*` 从 `data` 回填（缺省 0）；`chat_id` 不在 `Task` 的 JSON 里，按 `session_id`
///   去 sessions 表取，会话行没了就留 NULL（起飞照常，见 app 的 `startup_recovery`）。
/// - `message_index`：「平台消息 → 会话 / 任务」索引，锚点与引用解析用。
fn m2_query_columns_and_message_index(c: &Connection) -> Result<(), StoreError> {
    c.execute_batch("ALTER TABLE seen_events ADD COLUMN seen_at TEXT;")
        .map_err(sq)?;
    c.execute(
        "UPDATE seen_events SET seen_at = ?1",
        params![stamp(Utc::now())],
    )
    .map_err(sq)?;
    c.execute_batch(
        "
CREATE INDEX IF NOT EXISTS idx_seen_events_seen_at ON seen_events (seen_at);

ALTER TABLE tasks ADD COLUMN chat_id TEXT;
ALTER TABLE tasks ADD COLUMN cost REAL NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN tokens_in INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN tokens_out INTEGER NOT NULL DEFAULT 0;
UPDATE tasks SET
    chat_id    = (SELECT s.chat_id FROM sessions s WHERE s.id = tasks.session_id),
    cost       = CASE WHEN json_valid(data) THEN COALESCE(json_extract(data, '$.cost'), 0) ELSE 0 END,
    tokens_in  = CASE WHEN json_valid(data) THEN COALESCE(json_extract(data, '$.tokens_in'), 0) ELSE 0 END,
    tokens_out = CASE WHEN json_valid(data) THEN COALESCE(json_extract(data, '$.tokens_out'), 0) ELSE 0 END;
CREATE INDEX IF NOT EXISTS idx_tasks_chat_created ON tasks (chat_id, created_at);

CREATE TABLE IF NOT EXISTS message_index (
    chat_id     TEXT NOT NULL,
    message_id  TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    task_id     TEXT,
    outbound    INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (chat_id, message_id)
);
",
    )
    .map_err(sq)
}

/// 读当前版本：`schema_version` 表不存在（任何 P0 的库）或为空 = 0。
///
/// 先查 `sqlite_master` 判表在不在，**不**把「读版本出错」一律当 0 —— 其它任何错误
/// （`file is not a database` 之类）都经 `sq()` 原样上抛，preflight 的对拍测试要那句原文。
fn current_version(c: &Connection) -> Result<i64, StoreError> {
    let exists: bool = c
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version')",
            [],
            |r| r.get(0),
        )
        .map_err(sq)?;
    if !exists {
        return Ok(0);
    }
    // 空表时 MAX 回一行 NULL
    let v: Option<i64> = c
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .map_err(sq)?;
    Ok(v.unwrap_or(0))
}

/// 库比本程序新：不认识的列 / 表语义没法保证，拒绝打开（在任何写之前）。
fn refuse_newer(v: i64) -> Result<(), StoreError> {
    if v > LATEST_SCHEMA_VERSION {
        return Err(StoreError::Other(format!(
            "库的 schema 版本 {v} 比本程序支持的 {LATEST_SCHEMA_VERSION} 新，拒绝打开"
        )));
    }
    Ok(())
}

/// `init()` 的全部内容。流程见模块头。
pub(crate) fn run(c: &Connection) -> Result<(), StoreError> {
    // 1–2. 快路径：普通 SELECT，不开写事务
    let v = current_version(c)?;
    refuse_newer(v)?;
    if v == LATEST_SCHEMA_VERSION {
        return Ok(());
    }

    // 3. IMMEDIATE：一开始就拿写锁（deferred 读后再升级会撞 BUSY）。
    //    `with_conn` 只给 `&Connection`，所以用 `new_unchecked`。
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate).map_err(sq)?;
    // 事务里重读：两个实例同时起飞时，后拿到锁的那个看到的已经是迁完的版本
    let v = current_version(&tx)?;
    refuse_newer(v)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version     INTEGER PRIMARY KEY,
            applied_at  TEXT NOT NULL
        );",
    )
    .map_err(sq)?;
    for m in MIGRATIONS.iter().filter(|m| m.version > v) {
        (m.apply)(&tx)?;
        tx.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            params![m.version, stamp(Utc::now())],
        )
        .map_err(sq)?;
    }
    // 任一步 `?` 提前返回 → `tx` 被 drop → 回滚，版本不变
    tx.commit().map_err(sq)
}
