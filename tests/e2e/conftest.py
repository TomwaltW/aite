"""T4 自己的 fixture（§3.4：各 track 的 fixture 放自己目录，不动 tests/conftest.py）。

这里最要紧的是 `DemoPlane`：一个**只为验 runner 本身而存在**的最小 ControlPlane。
它不是 T2 的实现，也不打算变成 T2 的实现 —— 只覆盖 R1/R2/R6/R7 加「一步 final」
这条最短路径，好证明「场景 yaml → 事件 → 替身 → expect 断言」这条链是通的，
而不是一个永远报 not implemented 的空壳。真实现见 aite/control/（T2）。
"""
from __future__ import annotations

import asyncio
from datetime import UTC, datetime
from uuid import uuid4

import pytest

from aite.contracts import (
    ALL_MODEL_TOOLS,
    Message,
    NormalizedEvent,
    OutboundText,
    SenderKind,
    Session,
    SessionKind,
    Task,
    TaskStatus,
    Turn,
)
from aite.evals.scenario import load_suite
from aite.evals.wiring import Deps
from aite.testing import FakePlatform, FakeSandbox, FakeToolGateway

SUITE_DIR = "evals/p0"


class DemoPlane:
    """最小 ControlPlane：R1 丢非真人 → R2 去重 → R6 续接 / R7 新建 → 一步 final。"""

    def __init__(self, deps: Deps) -> None:
        self.deps = deps
        self.queue: asyncio.Queue = asyncio.Queue()

    async def handle_event(self, ev: NormalizedEvent) -> None:
        d = self.deps
        if ev.sender_kind is not SenderKind.human:            # R1
            return
        if await d.store.seen_event(ev.event_id):             # R2
            return

        now = datetime.now(UTC)
        session = None
        if ev.anchor.thread_id:                               # R6
            session = await d.store.find_session_by_thread(ev.chat_id, ev.anchor.thread_id)
        if session is None:                                   # R7
            anchor = ev.anchor.model_copy(
                update={"thread_id": ev.anchor.thread_id or ev.anchor.message_id}
            )
            session = Session(
                id=uuid4().hex,
                tenant_id=ev.tenant_id,
                workspace_id=ev.workspace_id,
                chat_id=ev.chat_id,
                kind=SessionKind.task,
                anchor=anchor,
                created_by=ev.sender_id,
                created_at=now,
                last_active_at=now,
            )
            await d.store.create_session(session)
            await d.platform.add_reaction(ev.anchor.message_id, "ack")

        seq = len(await d.store.list_turns(session.id))
        await d.store.append_turn(
            Turn(
                session_id=session.id,
                seq=seq,
                role="user",
                platform_user_id=ev.sender_id,
                content=ev.text,
                attachments=list(ev.attachments),
                created_at=now,
            )
        )
        task = Task(
            id=uuid4().hex,
            session_id=session.id,
            task_no=await d.store.next_task_no(ev.tenant_id),
            session_token=uuid4().hex,
            created_by=ev.sender_id,
            created_at=now,
            updated_at=now,
            max_steps=d.config.worker.max_steps,
            max_wall_sec=d.config.worker.max_wall_sec,
        )
        await d.store.create_task(task)
        await self.queue.put((session, task, ev))

    async def run_forever(self) -> None:
        while True:
            session, task, ev = await self.queue.get()
            await self._run(session, task, ev)

    async def _run(self, session: Session, task: Task, ev: NormalizedEvent) -> None:
        d = self.deps
        turn = await d.model.chat(
            [Message(role="user", content=ev.text)],
            ALL_MODEL_TOOLS,
            max_tokens=d.config.model.max_tokens,
            temperature=d.config.model.temperature,
        )
        for tc in turn.message.tool_calls or []:
            if tc.name != "final":
                continue
            await d.platform.send_text(
                OutboundText(
                    chat_id=ev.chat_id,
                    text=str(tc.arguments.get("reply", "")),
                    reply_to=ev.anchor.message_id,
                )
            )
            task.status = TaskStatus.delivered
            task.updated_at = datetime.now(UTC)
            await d.store.update_task(task)


@pytest.fixture
def demo_plane_factory():
    """给 run_scenario 用的 plane 工厂。"""
    return DemoPlane


@pytest.fixture
def suite():
    """evals/p0 下的 10 个场景，按文件名排序。"""
    return load_suite(SUITE_DIR)


@pytest.fixture
def scenarios_by_name(suite):
    return {sc.name: sc for sc in suite}


@pytest.fixture
def platform():
    return FakePlatform()


@pytest.fixture
def sandbox():
    return FakeSandbox()


@pytest.fixture
def gateway(platform, sandbox):
    return FakeToolGateway(platform=platform, sandbox=sandbox)
