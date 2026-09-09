"""SessionStore 的 SQLite 实现骨架（owner: T2，契约见 aite/contracts/ports.py §3.2）。"""
from ..contracts import Session, Task, Turn


class SqliteSessionStore:
    """SessionStore（T2）。"""

    async def init(self) -> None:
        raise NotImplementedError("T2")

    async def get_session(self, session_id: str) -> Session | None:
        raise NotImplementedError("T2")

    async def find_session_by_thread(self, chat_id: str, thread_id: str) -> Session | None:
        raise NotImplementedError("T2")

    async def create_session(self, s: Session) -> None:
        raise NotImplementedError("T2")

    async def update_session(self, s: Session) -> None:
        raise NotImplementedError("T2")

    async def append_turn(self, t: Turn) -> None:
        raise NotImplementedError("T2")

    async def list_turns(self, session_id: str, *, limit: int = 200) -> list[Turn]:
        raise NotImplementedError("T2")

    async def create_task(self, t: Task) -> None:
        raise NotImplementedError("T2")

    async def update_task(self, t: Task) -> None:
        raise NotImplementedError("T2")

    async def get_task(self, task_id: str) -> Task | None:
        raise NotImplementedError("T2")

    async def list_active_tasks(self, chat_id: str) -> list[Task]:
        raise NotImplementedError("T2")

    async def next_task_no(self, tenant_id: str) -> str:
        raise NotImplementedError("T2")

    async def seen_event(self, event_id: str) -> bool:
        raise NotImplementedError("T2")
