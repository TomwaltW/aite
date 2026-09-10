"""SessionStore 的 SQLite 实现（owner: T2，契约见 aite/contracts/ports.py §3.2）。

落库策略：每张表存一列 `data`（模型的 JSON 全文）+ 若干用于查询的冗余列。
好处是 pydantic 负责序列化/反序列化，datetime、枚举、嵌套的 Anchor / Attachment
都不会在手写的列映射里走样；代价只是查询字段要在写入时同步冗余出来。
"""
import asyncio
import sqlite3
from datetime import UTC, datetime
from pathlib import Path

import aiosqlite

from ..contracts import (
    Session,
    SessionStatus,
    Task,
    TaskStatus,
    Turn,
    encode_task_no,
)

# §3.2 list_active_tasks 的口径
ACTIVE_TASK_STATUSES: tuple[TaskStatus, ...] = (
    TaskStatus.created,
    TaskStatus.planning,
    TaskStatus.working,
)

# 上一条命没跑完的任务被收拾掉时写进 result_summary 的话。
ORPHAN_RESULT_SUMMARY = "进程重启前该任务仍在执行，已终止。请重新发起。"

_SCHEMA = """
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
"""


class DuplicateTurnError(ValueError):
    """同一 (session_id, seq) 被写了两次。§3.2：seq 由调用方分配，重复则报错。"""


class SqliteSessionStore:
    """SessionStore（T2）。"""

    def __init__(self, path: str | Path = "data/aite.db") -> None:
        self._path = str(path)
        self._db: aiosqlite.Connection | None = None
        # 单连接 + 单锁：P0 是进程内单副本，正确性比并发度重要得多。
        self._lock = asyncio.Lock()

    # ---- 生命周期 ---------------------------------------------------------

    async def init(self) -> None:
        """建表，幂等。"""
        if self._db is None:
            if self._path != ":memory:":
                Path(self._path).parent.mkdir(parents=True, exist_ok=True)
            self._db = await aiosqlite.connect(self._path)
        await self._db.executescript(_SCHEMA)
        await self._db.commit()

    async def close(self) -> None:
        """关连接。契约里没有这条，但测试和 TΩ 需要能收干净。幂等。"""
        if self._db is not None:
            await self._db.close()
            self._db = None

    @property
    def _conn(self) -> aiosqlite.Connection:
        if self._db is None:
            raise RuntimeError("SqliteSessionStore 未初始化：先 await store.init()")
        return self._db

    # ---- session ---------------------------------------------------------

    async def get_session(self, session_id: str) -> Session | None:
        async with self._lock:
            async with self._conn.execute("SELECT data FROM sessions WHERE id = ?", (session_id,)) as cur:
                row = await cur.fetchone()
        return Session.model_validate_json(row[0]) if row else None

    async def find_session_by_thread(self, chat_id: str, thread_id: str) -> Session | None:
        """话题命中。已归档的会话不再命中 —— `!restart` 靠这个换新会话（§3.5 R5）。"""
        sql = (
            "SELECT data FROM sessions WHERE chat_id = ? AND thread_id = ? AND status != ? "
            "ORDER BY created_at DESC, id DESC LIMIT 1"
        )
        async with self._lock:
            async with self._conn.execute(sql, (chat_id, thread_id, SessionStatus.archived.value)) as cur:
                row = await cur.fetchone()
        return Session.model_validate_json(row[0]) if row else None

    async def create_session(self, s: Session) -> None:
        async with self._lock:
            await self._conn.execute(
                "INSERT INTO sessions (id, tenant_id, chat_id, thread_id, status, created_at, data) "
                "VALUES (?, ?, ?, ?, ?, ?, ?)",
                (s.id, s.tenant_id, s.chat_id, s.anchor.thread_id, s.status.value,
                 s.created_at.isoformat(), s.model_dump_json()),
            )
            await self._conn.commit()

    async def update_session(self, s: Session) -> None:
        async with self._lock:
            await self._conn.execute(
                "UPDATE sessions SET tenant_id = ?, chat_id = ?, thread_id = ?, status = ?, data = ? "
                "WHERE id = ?",
                (s.tenant_id, s.chat_id, s.anchor.thread_id, s.status.value, s.model_dump_json(), s.id),
            )
            await self._conn.commit()

    # ---- turn ------------------------------------------------------------

    async def append_turn(self, t: Turn) -> None:
        """seq 由调用方分配；重复 (session_id, seq) 报 DuplicateTurnError。"""
        async with self._lock:
            try:
                await self._conn.execute(
                    "INSERT INTO turns (session_id, seq, data) VALUES (?, ?, ?)",
                    (t.session_id, t.seq, t.model_dump_json()),
                )
            except sqlite3.IntegrityError as exc:
                await self._conn.rollback()
                raise DuplicateTurnError(f"turn 已存在：session={t.session_id} seq={t.seq}") from exc
            await self._conn.commit()

    async def list_turns(self, session_id: str, *, limit: int = 200) -> list[Turn]:
        """按 seq 正序返回**最近** limit 轮。

        取最近而不是最早：这是给 worker 拼上下文用的，丢掉新消息会让追问失去意义；
        W1 的「前 2 轮 + 最近 30 轮」截断在这个窗口之上再做一次。
        """
        sql = "SELECT data FROM turns WHERE session_id = ? ORDER BY seq DESC LIMIT ?"
        async with self._lock:
            async with self._conn.execute(sql, (session_id, limit)) as cur:
                rows = await cur.fetchall()
        return [Turn.model_validate_json(r[0]) for r in reversed(rows)]

    async def next_turn_seq(self, session_id: str) -> int:
        """下一个可用 seq。契约没有这一条，但 seq「由调用方分配」总得有处可查。"""
        async with self._lock:
            async with self._conn.execute(
                "SELECT COALESCE(MAX(seq), -1) FROM turns WHERE session_id = ?", (session_id,)
            ) as cur:
                row = await cur.fetchone()
        return int(row[0]) + 1

    # ---- task ------------------------------------------------------------

    async def create_task(self, t: Task) -> None:
        async with self._lock:
            await self._conn.execute(
                "INSERT INTO tasks (id, session_id, task_no, status, created_at, data) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (t.id, t.session_id, t.task_no, t.status.value, t.created_at.isoformat(), t.model_dump_json()),
            )
            await self._conn.commit()

    async def update_task(self, t: Task) -> None:
        async with self._lock:
            await self._conn.execute(
                "UPDATE tasks SET session_id = ?, task_no = ?, status = ?, data = ? WHERE id = ?",
                (t.session_id, t.task_no, t.status.value, t.model_dump_json(), t.id),
            )
            await self._conn.commit()

    async def get_task(self, task_id: str) -> Task | None:
        async with self._lock:
            async with self._conn.execute("SELECT data FROM tasks WHERE id = ?", (task_id,)) as cur:
                row = await cur.fetchone()
        return Task.model_validate_json(row[0]) if row else None

    async def list_active_tasks(self, chat_id: str) -> list[Task]:
        """status in (created, planning, working)；chat_id 经 sessions 表反查。"""
        placeholders = ", ".join("?" for _ in ACTIVE_TASK_STATUSES)
        sql = (
            "SELECT t.data FROM tasks t JOIN sessions s ON s.id = t.session_id "
            f"WHERE s.chat_id = ? AND t.status IN ({placeholders}) "
            "ORDER BY t.created_at ASC, t.id ASC"
        )
        params = (chat_id, *(s.value for s in ACTIVE_TASK_STATUSES))
        async with self._lock:
            async with self._conn.execute(sql, params) as cur:
                rows = await cur.fetchall()
        return [Task.model_validate_json(r[0]) for r in rows]

    async def recover_orphan_tasks(self) -> list[Task]:
        """把上一条命遗留的活跃任务收干净，返回被收拾的那些。

        `systemctl restart` / `kill -9` 之后，崩溃瞬间处于 created/planning/working 的任务
        既没有人接着跑，也没有人把它标完 —— 而 `list_active_tasks` 正认这三个状态（§3.2），
        不收拾就永远挂在 `!status` 上，群里看着像还在干活（§2.4 M6 的重启场景）。

        **故意不在 `init()` 里自动调用**，两个理由：
        - `init()` 的契约是「建表，幂等」（§3.2），塞状态变更是扩契约；
        - §3.3 要求失败要「task failed + 回帖 + evidence `failed` 事件」，
          后两件 store 做不了。自动改状态而没人回帖，等于把任务悄悄埋掉 ——
          用户以为还在跑，比挂着更糟。所以这里只把状态收干净并**把任务交回给调用方**，
          由起飞处（`aite/app.py`）拿着返回值去回帖 + 写 evidence。
        """
        placeholders = ", ".join("?" for _ in ACTIVE_TASK_STATUSES)
        sql = (
            f"SELECT data FROM tasks WHERE status IN ({placeholders}) "
            "ORDER BY created_at ASC, id ASC"
        )
        params = tuple(s.value for s in ACTIVE_TASK_STATUSES)
        async with self._lock:
            async with self._conn.execute(sql, params) as cur:
                rows = await cur.fetchall()
            orphans = [Task.model_validate_json(r[0]) for r in rows]
            now = datetime.now(UTC)
            for t in orphans:
                t.status = TaskStatus.failed
                t.result_summary = ORPHAN_RESULT_SUMMARY
                t.updated_at = now
                await self._conn.execute(
                    "UPDATE tasks SET status = ?, data = ? WHERE id = ?",
                    (t.status.value, t.model_dump_json(), t.id),
                )
            await self._conn.commit()
        return orphans

    async def next_task_no(self, tenant_id: str) -> str:
        """租户内原子递增 + encode_task_no。UPSERT ... RETURNING 是单条语句，天然原子。"""
        async with self._lock:
            async with self._conn.execute(
                "INSERT INTO task_counters (tenant_id, n) VALUES (?, 1) "
                "ON CONFLICT(tenant_id) DO UPDATE SET n = n + 1 RETURNING n",
                (tenant_id,),
            ) as cur:
                row = await cur.fetchone()
            await self._conn.commit()
        return encode_task_no(int(row[0]))

    # ---- 去重 ------------------------------------------------------------

    async def seen_event(self, event_id: str) -> bool:
        """首次调用记录并返回 False，之后 True。"""
        async with self._lock:
            try:
                await self._conn.execute("INSERT INTO seen_events (event_id) VALUES (?)", (event_id,))
            except sqlite3.IntegrityError:
                await self._conn.rollback()
                return True
            await self._conn.commit()
        return False
