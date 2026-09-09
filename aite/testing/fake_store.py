"""FakeSessionStore / FakeEvidenceWriter —— 两个 Port 的内存替身（dev-spec §3.2）。

真实现（SQLite + 落盘 evidence）归 T2。这两份只保证语义对：

* `append_turn` 对重复的 (session_id, seq) 报错
* `next_task_no` 原子递增并走契约的 `encode_task_no`
* `seen_event` 首次 False、之后 True
* `list_active_tasks` 只回 created / planning / working
* evidence 链用契约里的 `payload_hash_of` / `chain_hash` 真算，`verify` 重算整条链
"""
from __future__ import annotations

from datetime import UTC, datetime

from ..contracts import (
    GENESIS,
    EvidenceEvent,
    EvidenceKind,
    Session,
    Task,
    TaskStatus,
    Turn,
    chain_hash,
    encode_task_no,
    payload_hash_of,
)
from .recorder import CallLog

ACTIVE_STATUSES = (TaskStatus.created, TaskStatus.planning, TaskStatus.working)


class FakeSessionStore:
    """SessionStore 的内存替身。"""

    def __init__(self) -> None:
        self.calls = CallLog()
        self.sessions: dict[str, Session] = {}
        self.tasks: dict[str, Task] = {}
        self.turns: dict[str, list[Turn]] = {}
        self.seen: set[str] = set()
        self._counters: dict[str, int] = {}
        self.initialized = False

    async def init(self) -> None:
        self.calls.record("init")
        self.initialized = True                      # 幂等

    async def get_session(self, session_id: str) -> Session | None:
        self.calls.record("get_session", session_id=session_id)
        return self.sessions.get(session_id)

    async def find_session_by_thread(self, chat_id: str, thread_id: str) -> Session | None:
        self.calls.record("find_session_by_thread", chat_id=chat_id, thread_id=thread_id)
        for s in self.sessions.values():
            if s.chat_id == chat_id and s.anchor.thread_id == thread_id:
                return s
        return None

    async def create_session(self, s: Session) -> None:
        self.calls.record("create_session", session_id=s.id, thread_id=s.anchor.thread_id)
        if s.id in self.sessions:
            raise ValueError(f"session {s.id} 已存在")
        self.sessions[s.id] = s.model_copy(deep=True)
        self.turns.setdefault(s.id, [])

    async def update_session(self, s: Session) -> None:
        self.calls.record("update_session", session_id=s.id, status=str(s.status))
        self.sessions[s.id] = s.model_copy(deep=True)

    async def append_turn(self, t: Turn) -> None:
        self.calls.record("append_turn", session_id=t.session_id, seq=t.seq, role=t.role)
        rows = self.turns.setdefault(t.session_id, [])
        if any(r.seq == t.seq for r in rows):
            raise ValueError(f"重复的 (session_id={t.session_id}, seq={t.seq})")
        rows.append(t.model_copy(deep=True))

    async def list_turns(self, session_id: str, *, limit: int = 200) -> list[Turn]:
        self.calls.record("list_turns", session_id=session_id, limit=limit)
        rows = sorted(self.turns.get(session_id, []), key=lambda r: r.seq)
        return rows[-limit:]

    async def create_task(self, t: Task) -> None:
        self.calls.record("create_task", task_id=t.id, session_id=t.session_id, task_no=t.task_no)
        if t.id in self.tasks:
            raise ValueError(f"task {t.id} 已存在")
        self.tasks[t.id] = t.model_copy(deep=True)

    async def update_task(self, t: Task) -> None:
        self.calls.record("update_task", task_id=t.id, status=str(t.status))
        self.tasks[t.id] = t.model_copy(deep=True)

    async def get_task(self, task_id: str) -> Task | None:
        self.calls.record("get_task", task_id=task_id)
        return self.tasks.get(task_id)

    async def list_active_tasks(self, chat_id: str) -> list[Task]:
        self.calls.record("list_active_tasks", chat_id=chat_id)
        out = []
        for t in self.tasks.values():
            session = self.sessions.get(t.session_id)
            if session is not None and session.chat_id == chat_id and t.status in ACTIVE_STATUSES:
                out.append(t.model_copy(deep=True))
        return sorted(out, key=lambda t: t.created_at)

    async def next_task_no(self, tenant_id: str) -> str:
        n = self._counters.get(tenant_id, 0) + 1
        self._counters[tenant_id] = n
        self.calls.record("next_task_no", tenant_id=tenant_id, n=n)
        return encode_task_no(n)

    async def seen_event(self, event_id: str) -> bool:
        first = event_id not in self.seen
        self.calls.record("seen_event", event_id=event_id, seen=not first)
        self.seen.add(event_id)
        return not first

    # ---- 给断言用 --------------------------------------------------------

    @property
    def session_ids(self) -> list[str]:
        return sorted(self.sessions)

    @property
    def task_list(self) -> list[Task]:
        return sorted(self.tasks.values(), key=lambda t: t.created_at)

    @property
    def distinct_session_ids_of_tasks(self) -> set[str]:
        return {t.session_id for t in self.tasks.values()}


class FakeEvidenceWriter:
    """EvidenceWriter 的内存替身；hash 链用契约里的函数真算。"""

    def __init__(self) -> None:
        self.calls = CallLog()
        self.chains: dict[str, list[EvidenceEvent]] = {}
        self.manifests: dict[str, dict] = {}

    async def append(self, task_id: str, kind: EvidenceKind, payload: dict) -> EvidenceEvent:
        self.calls.record("append", task_id=task_id, kind=str(kind))
        chain = self.chains.setdefault(task_id, [])
        prev = chain[-1].hash if chain else GENESIS
        ph = payload_hash_of(payload)
        ev = EvidenceEvent(
            task_id=task_id,
            seq=len(chain),
            kind=kind,
            payload_hash=ph,
            payload_ref=None,
            payload=payload,
            prev_hash=prev,
            hash=chain_hash(prev, ph),
            created_at=datetime.now(UTC),
        )
        chain.append(ev)
        return ev

    async def finalize(self, task_id: str, manifest_extra: dict) -> str:
        chain = self.chains.get(task_id, [])
        root = chain[-1].hash if chain else GENESIS
        self.calls.record("finalize", task_id=task_id, root_hash=root)
        self.manifests[task_id] = {"task_id": task_id, "root_hash": root, "event_count": len(chain),
                                   **manifest_extra}
        return root

    def verify(self, task_id: str) -> bool:
        self.calls.record("verify", task_id=task_id)
        prev = GENESIS
        for i, ev in enumerate(self.chains.get(task_id, [])):
            if ev.seq != i or ev.prev_hash != prev:
                return False
            if ev.payload is not None and payload_hash_of(ev.payload) != ev.payload_hash:
                return False
            if chain_hash(prev, ev.payload_hash) != ev.hash:
                return False
            prev = ev.hash
        return True

    def kinds(self, task_id: str) -> list[str]:
        return [str(e.kind) for e in self.chains.get(task_id, [])]
