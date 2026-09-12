//! aite-store —— SessionStore 的 SQLite 实现（对应旧 `aite/control/store.py`）。owner: R3
//!
//! 落库策略与 Python 版逐字一致：每张表存一列 `data`（domain 类型的 JSON 全文）
//! 加若干用于查询的冗余列。serde 负责序列化/反序列化，datetime、枚举、嵌套的
//! Anchor / Attachment 都不会在手写的列映射里走样；代价只是查询字段要在写入时冗余出来。
//!
//! 并发结构（对应 Python 的「单连接 + 每实例一把 asyncio.Lock，方法整体持锁」）：
//! 单个 `rusqlite::Connection` 放在 `Mutex` 里，每个方法整体在
//! `tokio::task::spawn_blocking` 里持锁执行 —— trait 是 async 的，而 rusqlite 是阻塞 IO，
//! 不能在 async 上下文里直接做（spec §7.6）。
//!
//! 与 Python 版的两处有意差异：
//! - `PRAGMA busy_timeout = 5000`（spec D8）：Rust 里跨实例并发是真线程，不是 asyncio 交错，
//!   两个实例指向同一个 .db 时会真撞上 SQLite 的文件锁。
//! - 不显式 commit / rollback：rusqlite 默认 autocommit，一条写语句就是一个事务，
//!   天然满足「每次写完立即 commit」与「撞主键那一路不牵连邻居」。
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aite_contracts::{
    ACTIVE_TASK_STATUSES, Session, SessionStatus, SessionStore, StoreError, Task, TaskStatus, Turn,
    encode_task_no,
};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};

/// 上一条命没跑完的任务被收拾掉时写进 `result_summary` 的话（与 Python 版逐字一致）。
pub const ORPHAN_RESULT_SUMMARY: &str = "进程重启前该任务仍在执行，已终止。请重新发起。";

/// spec D8：跨实例并发是真线程，撞上文件锁时等一会儿而不是立刻报 SQLITE_BUSY。
pub const BUSY_TIMEOUT_MS: u64 = 5000;

/// 建表 SQL，逐字照 `aite/control/store.py` 的 `_SCHEMA`。幂等。
const SCHEMA: &str = "
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

fn sq(e: rusqlite::Error) -> StoreError {
    StoreError::Sqlite(e.to_string())
}

/// 冗余列 `created_at` 的格式。定长（永远 6 位小数 + `Z`）是有意的：这列是
/// `ORDER BY created_at DESC, id DESC` 的排序键，小数位数飘的话字典序就不等于时间序。
fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// 撞主键 / 唯一约束？`seen_event` 与 `append_turn` 靠它把「重复」和「写不进去」分开 ——
/// 只读目录那一路必须继续往上抛，不许被当成「已经有了」。
fn is_constraint_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// SessionStore（对应 Python `SqliteSessionStore`）。
pub struct SqliteSessionStore {
    path: String,
    /// `None` = 已经 close 过；此后任何方法都回 `StoreError::NotInitialized`。
    conn: Arc<Mutex<Option<Connection>>>,
}

impl std::fmt::Debug for SqliteSessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSessionStore")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteSessionStore {
    /// 打开（或新建）一个 .db。`:memory:` 也认；文件路径会先把父目录建出来。
    /// 建表在 `init()` 里，这里只负责连上。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let p: PathBuf = path.as_ref().to_path_buf();
        let path_str = p.to_string_lossy().into_owned();
        if path_str != ":memory:"
            && let Some(parent) = p.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError::Other(format!("建不出 {} 的父目录：{e}", p.display())))?;
        }
        let conn = Connection::open(&path_str).map_err(sq)?;
        conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
            .map_err(sq)?;
        Ok(Self {
            path: path_str,
            conn: Arc::new(Mutex::new(Some(conn))),
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// 排障 / 测试用：跑一条 `PRAGMA <name>` 并把第一列当文本取回来
    /// （`journal_mode` 必须是 `delete`、崩溃后 `integrity_check` 必须是 `ok`，T18 钉住）。
    /// 名字只允许字母数字和下划线 —— 它是拼进 SQL 的。
    pub async fn pragma(&self, name: &str) -> Result<String, StoreError> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(StoreError::Other(format!("非法的 pragma 名：{name:?}")));
        }
        let sql = format!("PRAGMA {name}");
        self.with_conn(move |c| c.query_row(&sql, [], |r| r.get::<_, String>(0)).map_err(sq))
            .await
    }

    /// 同 [`Self::pragma`]，但取 INTEGER 那一列。
    ///
    /// `busy_timeout` 这类 pragma 回的是数字，用取文本的那个版本会直接报类型错 ——
    /// 于是 D8 那条「`PRAGMA busy_timeout = 5000`」一直没有测试钉得住（RΩ 补）。
    pub async fn pragma_int(&self, name: &str) -> Result<i64, StoreError> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(StoreError::Other(format!("非法的 pragma 名：{name:?}")));
        }
        let sql = format!("PRAGMA {name}");
        self.with_conn(move |c| c.query_row(&sql, [], |r| r.get::<_, i64>(0)).map_err(sq))
            .await
    }

    /// 所有方法的公共骨架：进 blocking 线程池 → 持锁 → 拿连接（已 close 则报
    /// `NotInitialized`）→ 干活。
    async fn with_conn<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let cell = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let guard = cell.lock().unwrap_or_else(|e| e.into_inner());
            let conn = guard.as_ref().ok_or(StoreError::NotInitialized)?;
            f(conn)
        })
        .await
        .map_err(|e| StoreError::Other(format!("spawn_blocking 失败：{e}")))?
    }
}

fn parse_session(json: &str) -> Result<Session, StoreError> {
    Ok(serde_json::from_str(json)?)
}

fn parse_task(json: &str) -> Result<Task, StoreError> {
    Ok(serde_json::from_str(json)?)
}

fn parse_turn(json: &str) -> Result<Turn, StoreError> {
    Ok(serde_json::from_str(json)?)
}

/// 取一列 TEXT 的全部行。
fn query_texts(
    conn: &Connection,
    sql: &str,
    ps: &[&dyn rusqlite::ToSql],
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(sql).map_err(sq)?;
    let rows = stmt
        .query_map(ps, |r| r.get::<_, String>(0))
        .map_err(sq)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sq)?;
    Ok(rows)
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn init(&self) -> Result<(), StoreError> {
        self.with_conn(|c| c.execute_batch(SCHEMA).map_err(sq))
            .await
    }

    async fn close(&self) -> Result<(), StoreError> {
        let cell = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
            match guard.take() {
                // 幂等：已经关过就什么都不做（Python 版同此）
                None => Ok(()),
                Some(conn) => conn.close().map_err(|(_, e)| sq(e)),
            }
        })
        .await
        .map_err(|e| StoreError::Other(format!("spawn_blocking 失败：{e}")))?
    }

    async fn get_session(&self, session_id: &str) -> Result<Option<Session>, StoreError> {
        let id = session_id.to_string();
        self.with_conn(move |c| {
            let row: Option<String> = c
                .query_row(
                    "SELECT data FROM sessions WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sq)?;
            row.as_deref().map(parse_session).transpose()
        })
        .await
    }

    /// 话题命中。已归档的会话不再命中 —— `!restart` 靠这个换新会话（§3.5 R5）。
    async fn find_session_by_thread(
        &self,
        chat_id: &str,
        thread_id: &str,
    ) -> Result<Option<Session>, StoreError> {
        let (chat_id, thread_id) = (chat_id.to_string(), thread_id.to_string());
        self.with_conn(move |c| {
            let row: Option<String> = c
                .query_row(
                    "SELECT data FROM sessions WHERE chat_id = ?1 AND thread_id = ?2 AND status != ?3 \
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    params![chat_id, thread_id, SessionStatus::Archived.as_str()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sq)?;
            row.as_deref().map(parse_session).transpose()
        })
        .await
    }

    async fn create_session(&self, s: &Session) -> Result<(), StoreError> {
        let s = s.clone();
        self.with_conn(move |c| {
            let data = serde_json::to_string(&s)?;
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
                    data
                ],
            )
            .map_err(sq)?;
            Ok(())
        })
        .await
    }

    async fn update_session(&self, s: &Session) -> Result<(), StoreError> {
        let s = s.clone();
        self.with_conn(move |c| {
            let data = serde_json::to_string(&s)?;
            c.execute(
                "UPDATE sessions SET tenant_id = ?1, chat_id = ?2, thread_id = ?3, status = ?4, \
                 data = ?5 WHERE id = ?6",
                params![
                    s.tenant_id,
                    s.chat_id,
                    s.anchor.thread_id,
                    s.status.as_str(),
                    data,
                    s.id
                ],
            )
            .map_err(sq)?;
            Ok(())
        })
        .await
    }

    /// seq 由调用方分配；重复 (session_id, seq) → `StoreError::DuplicateTurn`。
    async fn append_turn(&self, t: &Turn) -> Result<(), StoreError> {
        let t = t.clone();
        self.with_conn(move |c| {
            let data = serde_json::to_string(&t)?;
            match c.execute(
                "INSERT INTO turns (session_id, seq, data) VALUES (?1, ?2, ?3)",
                params![t.session_id, t.seq as i64, data],
            ) {
                Ok(_) => Ok(()),
                Err(e) if is_constraint_violation(&e) => Err(StoreError::DuplicateTurn {
                    session_id: t.session_id.clone(),
                    seq: t.seq,
                }),
                Err(e) => Err(sq(e)),
            }
        })
        .await
    }

    /// 按 seq 正序返回**最近** limit 轮：取最近而不是最早，丢掉新消息会让追问失去意义。
    async fn list_turns(&self, session_id: &str, limit: u32) -> Result<Vec<Turn>, StoreError> {
        let sid = session_id.to_string();
        self.with_conn(move |c| {
            let mut rows = query_texts(
                c,
                "SELECT data FROM turns WHERE session_id = ?1 ORDER BY seq DESC LIMIT ?2",
                params![sid, limit as i64],
            )?;
            rows.reverse();
            rows.iter().map(|s| parse_turn(s)).collect()
        })
        .await
    }

    async fn next_turn_seq(&self, session_id: &str) -> Result<u64, StoreError> {
        let sid = session_id.to_string();
        self.with_conn(move |c| {
            let max: i64 = c
                .query_row(
                    "SELECT COALESCE(MAX(seq), -1) FROM turns WHERE session_id = ?1",
                    params![sid],
                    |r| r.get(0),
                )
                .map_err(sq)?;
            Ok((max + 1).max(0) as u64)
        })
        .await
    }

    async fn create_task(&self, t: &Task) -> Result<(), StoreError> {
        let t = t.clone();
        self.with_conn(move |c| {
            let data = serde_json::to_string(&t)?;
            c.execute(
                "INSERT INTO tasks (id, session_id, task_no, status, created_at, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    t.id,
                    t.session_id,
                    t.task_no,
                    t.status.as_str(),
                    stamp(t.created_at),
                    data
                ],
            )
            .map_err(sq)?;
            Ok(())
        })
        .await
    }

    async fn update_task(&self, t: &Task) -> Result<(), StoreError> {
        let t = t.clone();
        self.with_conn(move |c| {
            let data = serde_json::to_string(&t)?;
            c.execute(
                "UPDATE tasks SET session_id = ?1, task_no = ?2, status = ?3, data = ?4 WHERE id = ?5",
                params![t.session_id, t.task_no, t.status.as_str(), data, t.id],
            )
            .map_err(sq)?;
            Ok(())
        })
        .await
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, StoreError> {
        let id = task_id.to_string();
        self.with_conn(move |c| {
            let row: Option<String> = c
                .query_row("SELECT data FROM tasks WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(sq)?;
            row.as_deref().map(parse_task).transpose()
        })
        .await
    }

    /// status in (created, planning, working)；chat_id 经 sessions 表反查。
    async fn list_active_tasks(&self, chat_id: &str) -> Result<Vec<Task>, StoreError> {
        let chat_id = chat_id.to_string();
        self.with_conn(move |c| {
            let rows = query_texts(
                c,
                "SELECT t.data FROM tasks t JOIN sessions s ON s.id = t.session_id \
                 WHERE s.chat_id = ?1 AND t.status IN (?2, ?3, ?4) \
                 ORDER BY t.created_at ASC, t.id ASC",
                params![
                    chat_id,
                    ACTIVE_TASK_STATUSES[0].as_str(),
                    ACTIVE_TASK_STATUSES[1].as_str(),
                    ACTIVE_TASK_STATUSES[2].as_str()
                ],
            )?;
            rows.iter().map(|s| parse_task(s)).collect()
        })
        .await
    }

    /// 租户内原子递增 + `encode_task_no`。UPSERT … RETURNING 是单条语句，天然原子。
    async fn next_task_no(&self, tenant_id: &str) -> Result<String, StoreError> {
        let tenant_id = tenant_id.to_string();
        self.with_conn(move |c| {
            let mut stmt = c
                .prepare_cached(
                    "INSERT INTO task_counters (tenant_id, n) VALUES (?1, 1) \
                     ON CONFLICT(tenant_id) DO UPDATE SET n = n + 1 RETURNING n",
                )
                .map_err(sq)?;
            let mut rows = stmt.query(params![tenant_id]).map_err(sq)?;
            let n: i64 = match rows.next().map_err(sq)? {
                Some(row) => row.get(0).map_err(sq)?,
                None => return Err(StoreError::Other("task_counters UPSERT 没回行".into())),
            };
            // 步到底，让隐式事务干净收尾（RETURNING 的行没取完就 reset 是另一回事）
            while rows.next().map_err(sq)?.is_some() {}
            drop(rows);
            encode_task_no(n.max(0) as u64).map_err(|e| StoreError::Other(e.to_string()))
        })
        .await
    }

    /// 首次调用记录并返回 false，之后 true。**写失败必须 Err**，不许假装 false ——
    /// 静默返回 false 的话，重连风暴里每条重复事件都会被当成新的，同一个活干很多遍。
    async fn seen_event(&self, event_id: &str) -> Result<bool, StoreError> {
        let event_id = event_id.to_string();
        self.with_conn(move |c| {
            match c.execute(
                "INSERT INTO seen_events (event_id) VALUES (?1)",
                params![event_id],
            ) {
                Ok(_) => Ok(false),
                Err(e) if is_constraint_violation(&e) => Ok(true),
                Err(e) => Err(sq(e)),
            }
        })
        .await
    }

    /// 把上一条命遗留的活跃任务收干净，返回被收拾的那些。
    ///
    /// **故意不在 `init()` 里自动调用**：`init()` 的契约是「建表，幂等」，塞状态变更是扩契约；
    /// 而 §3.3 要求失败要「task failed + 回帖 + evidence failed 事件」，后两件 store 做不了。
    /// 自动改状态而没人回帖等于把任务悄悄埋掉 —— 用户以为还在跑，比挂着更糟。
    async fn recover_orphan_tasks(&self) -> Result<Vec<Task>, StoreError> {
        self.with_conn(move |c| {
            let rows = query_texts(
                c,
                "SELECT data FROM tasks WHERE status IN (?1, ?2, ?3) ORDER BY created_at ASC, id ASC",
                params![
                    ACTIVE_TASK_STATUSES[0].as_str(),
                    ACTIVE_TASK_STATUSES[1].as_str(),
                    ACTIVE_TASK_STATUSES[2].as_str()
                ],
            )?;
            let tx = c.unchecked_transaction().map_err(sq)?;
            let now = Utc::now();
            let mut orphans = Vec::with_capacity(rows.len());
            for json in &rows {
                let mut t = parse_task(json)?;
                t.status = TaskStatus::Failed;
                t.result_summary = ORPHAN_RESULT_SUMMARY.to_string();
                t.updated_at = now;
                let data = serde_json::to_string(&t)?;
                tx.execute(
                    "UPDATE tasks SET status = ?1, data = ?2 WHERE id = ?3",
                    params![t.status.as_str(), data, t.id],
                )
                .map_err(sq)?;
                orphans.push(t);
            }
            tx.commit().map_err(sq)?;
            Ok(orphans)
        })
        .await
    }
}
