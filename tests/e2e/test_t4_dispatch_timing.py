"""投递时序：冻结契约 C-T5T6-1 的 `after` / `after_timeout_sec`（T5 与 T6 共用）。

这里只测**从 yaml 外面看得见的行为**，一律不碰 runner / wiring 的内部（函数名、
私有属性、异常类型都不碰）：写了 `after: running`，事件就等到任务真被领走才投；
写了 `after: idle`，就等到系统静默才投；等不到就以 `phase="dispatch"` 停下、
不许接着往下投。判据换个实现照样得成立。

用的 plane 是本文件自带的最小实现，刻意把「建任务」和「领任务」分在两个协程里 ——
那正是契约要区分的两件事：store 里有一行 task ≠ worker 领走了它。
"""
from __future__ import annotations

import asyncio
from datetime import UTC, datetime
from uuid import uuid4

from aite.contracts import (
    NormalizedEvent,
    Session,
    SessionKind,
    Task,
    TaskStatus,
)
from aite.evals import run_scenario
from aite.evals.scenario import EventSpec, Scenario
from aite.evals.wiring import Deps


class SplitPlane:
    """最小 plane：`handle_event` 只建 task（created）并入队，`run_forever` 才领走。

    领走时把任务推到 planning —— 与 §3.6 里 worker 一进主循环干的事一样，也是
    「worker 真的领走了」在外面唯一看得见的痕迹。三个开关用来摆出要测的时序：

        take=False    队列里的任务永远没人领（验 after: running 等不到时的姿态）
        settle_ticks  领走后隔几个 tick 才收尾（让 after: idle 有得可等）
        finish        收尾时把任务落到 delivered
    """

    def __init__(
        self, deps: Deps, *, take: bool = True, settle_ticks: int = 0, finish: bool = True
    ) -> None:
        self.deps = deps
        self.take = take
        self.settle_ticks = settle_ticks
        self.finish = finish
        self.queue: asyncio.Queue[str] = asyncio.Queue()
        #: 每条事件到达时，store 里各任务的状态 —— 测试就看这个判断投递时机
        self.seen: list[tuple[str, list[str]]] = []

    async def handle_event(self, ev: NormalizedEvent) -> None:
        now = datetime.now(UTC)
        self.seen.append(
            (ev.event_id, [str(t.status) for t in self.deps.store.tasks.values()])
        )
        session = Session(
            id=uuid4().hex,
            tenant_id=ev.tenant_id,
            workspace_id=ev.workspace_id,
            chat_id=ev.chat_id,
            kind=SessionKind.task,
            anchor=ev.anchor.model_copy(
                update={"thread_id": ev.anchor.thread_id or ev.anchor.message_id}
            ),
            created_by=ev.sender_id,
            created_at=now,
            last_active_at=now,
        )
        await self.deps.store.create_session(session)
        task = Task(
            id=uuid4().hex,
            session_id=session.id,
            task_no=await self.deps.store.next_task_no(ev.tenant_id),
            session_token=uuid4().hex,
            created_by=ev.sender_id,
            created_at=now,
            updated_at=now,
        )
        await self.deps.store.create_task(task)
        self.queue.put_nowait(task.id)

    async def run_forever(self) -> None:
        while True:
            task_id = await self.queue.get()
            if not self.take:                       # 领是领了，但永远不推进
                await asyncio.sleep(3600)
            task = await self.deps.store.get_task(task_id)
            assert task is not None
            task.status = TaskStatus.planning       # ← 「被 worker 领走了」
            await self.deps.store.update_task(task)
            for _ in range(self.settle_ticks):
                await asyncio.sleep(0)
            if self.finish:
                task.status = TaskStatus.delivered
                await self.deps.store.update_task(task)


def _two_events(after: str, *, after_timeout_sec: float = 5.0) -> Scenario:
    """一条事件起个任务，第二条按 `after` 决定什么时候投。"""
    return Scenario(
        name="timing",
        events=[
            EventSpec(event_id="e1", text="起个活"),
            EventSpec(
                event_id="e2",
                text="第二条",
                after=after,
                after_timeout_sec=after_timeout_sec,
            ),
        ],
        timeout_sec=2.0,
    )


def _status_at(plane: SplitPlane, event_id: str) -> list[str]:
    return next(states for eid, states in plane.seen if eid == event_id)


# --- 默认值 ------------------------------------------------------------------

def test_after_defaults_to_none_and_five_seconds():
    """C-T5T6-1 的默认值。改了它，现在绿着的 8 个场景会一起变行为。"""
    spec = EventSpec(event_id="e1")
    assert spec.after == "none"
    assert spec.after_timeout_sec == 5.0


# --- after: none / running ---------------------------------------------------

async def test_without_after_the_second_event_lands_before_anyone_takes_the_task():
    """默认（none）不等：第二条到达时第一条起的任务还躺在队列里，状态是 created。

    这不是缺陷，是基准 —— 07_commands 当初正是把这种时序当成了「任务在跑」。
    """
    planes: list[SplitPlane] = []
    r = await run_scenario(
        _two_events("none"),
        plane_factory=lambda d: planes.append(SplitPlane(d)) or planes[-1],
    )
    assert r.passed is True, r.reason
    assert _status_at(planes[0], "e2") == ["created"]


async def test_after_running_waits_until_the_task_is_actually_taken():
    """写了 after: running，第二条就要等到任务真的被领走（离开 created）才投。"""
    planes: list[SplitPlane] = []
    r = await run_scenario(
        _two_events("running"),
        plane_factory=lambda d: planes.append(SplitPlane(d, finish=False)) or planes[-1],
    )
    assert r.passed is True, r.reason
    assert _status_at(planes[0], "e2") == ["planning"]


async def test_after_running_that_never_happens_fails_the_dispatch_phase():
    """等不到就失败，不许接着投 —— 「等不到就接着投」测出来的绿是假的。"""
    planes: list[SplitPlane] = []
    r = await run_scenario(
        _two_events("running", after_timeout_sec=0.2),
        plane_factory=lambda d: planes.append(SplitPlane(d, take=False)) or planes[-1],
    )
    assert r.passed is False
    assert r.phase == "dispatch"
    assert r.reason and "Traceback" not in r.reason and "\n" not in r.reason
    assert [eid for eid, _ in planes[0].seen] == ["e1"], "等不到却还是把 e2 投出去了"


# --- after: idle -------------------------------------------------------------

async def test_after_idle_waits_until_the_running_task_is_done():
    """写了 after: idle，第二条要等在跑的任务收了、队列空了才投。"""
    planes: list[SplitPlane] = []
    r = await run_scenario(
        _two_events("idle"),
        plane_factory=lambda d: planes.append(SplitPlane(d, settle_ticks=5)) or planes[-1],
    )
    assert r.passed is True, r.reason
    assert _status_at(planes[0], "e2") == ["delivered"]


async def test_after_idle_judges_by_activity_not_by_task_status():
    """idle 判的是**系统活动静默**（`wiring.settle()` 那一套），不是「任务落到终态」。

    并轨时 `after` 的判据以契约 owner 那份为准：连续一段没有替身被调用就算静默 ——
    收尾判「系统不干活了」用的也是这套，同一件事不该有两个说法。

    `SplitPlane(finish=False)` 把任务撂在 planning 却一个替身都不碰，于是这里等到的是
    「静默」、e2 照投。真实系统里 worker 跑任务必然要调模型/网关，这种「活着但一动不动」
    的形态只在这个人造 plane 里出现，所以它钉的是判据本身，不是某个场景的期望。

    idle 等不到时的失败姿态（dispatch 失败、不往下投）由 `test_t5_event_timing.py::
    test_idle_that_never_settles_fails_at_dispatch` 钉住，那里的 plane 一刻不停在动。
    """
    planes: list[SplitPlane] = []
    r = await run_scenario(
        _two_events("idle", after_timeout_sec=0.4),
        plane_factory=lambda d: planes.append(SplitPlane(d, finish=False)) or planes[-1],
    )
    assert r.passed is True, r.reason
    assert _status_at(planes[0], "e2") == ["planning"]
