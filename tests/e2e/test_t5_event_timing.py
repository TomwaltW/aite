"""事件投递时序（冻结契约 C-T5T6-1）：`EventSpec.after` / `after_timeout_sec`。

投递循环原来是零间隔连着投的：两条消息落在同一个事件循环 tick 里，系统根本没机会
消化第一条。02 就是这么红的 —— e2 到达时 e1 的任务还在跑，控制面按 §3.5 R6 把它当
steer 合进当前任务，第二个 task 就没了。R6 本身没错，错的是评测没有时序模型。

这份测试钉四件事：

1. `after` 默认是 `none`，且 `none` 的行为与时序机制引入前一致；
2. `after: idle` 真的等到了静默才投（不等就会被并成 steer）；
3. `after: running` 判的是「worker 真的领走了」，不是「store 里有一行 task」；
4. 等不到就以 `phase="dispatch"` 失败收场，reason 说得清等的是什么、等了多久。
"""
from __future__ import annotations

import asyncio
from datetime import UTC, datetime
from uuid import uuid4

import pytest
from pydantic import ValidationError

from aite.contracts import NormalizedEvent, Task, TaskStatus
from aite.evals import run_scenario
from aite.evals.scenario import EventSpec, Scenario
from aite.evals.wiring import Deps, build_control_plane, newest_task

#: 01 在基线上跑出来的 stats，逐字抄下来。`after: none` 的行为一变，这条先红
BASELINE_01_STATS = {
    "platform_calls": 5,
    "model_calls": 1,
    "gateway_calls": 0,
    "sandbox_calls": 0,
    "sessions": 1,
    "tasks": 1,
}

#: 时序机制只动了 02（07 归 T6，他会给自己的场景加 after）。其余八个必须保持 none
UNTOUCHED_SCENARIOS = [
    "01_simple_qa",
    "03_checklist_progress",
    "04_csv_to_chart",
    "05_history_summary",
    "06_read_document",
    "08_step_limit",
    "09_bot_ignored",
    "10_duplicate_event",
]


# --- 测试专用的 plane ---------------------------------------------------------

class SlowPickupPlane:
    """建好任务后隔几个 tick 才领走 —— 专门把「任务建好了」和「worker 开跑了」拉开。

    `status_on_arrival` 记下每条事件到达时最新任务的状态，测试据此判断投递到底等没等：
    没等的话看到的是 `created`（还躺在队列里），等到了才是 `planning` 之后的状态。
    """

    pickup_delay_ms = 30
    picks_up = True

    def __init__(self, deps: Deps) -> None:
        self.deps = deps
        self.queue: asyncio.Queue[Task] = asyncio.Queue()
        self.status_on_arrival: list[tuple[str, str | None]] = []

    async def handle_event(self, ev: NormalizedEvent) -> None:
        seen = newest_task(self.deps)
        self.status_on_arrival.append((ev.event_id, None if seen is None else str(seen.status)))
        now = datetime.now(UTC)
        task = Task(
            id=uuid4().hex,
            session_id=uuid4().hex,
            task_no=await self.deps.store.next_task_no(ev.tenant_id),
            session_token=uuid4().hex,
            created_by=ev.sender_id,
            created_at=now,
            updated_at=now,
            max_steps=self.deps.config.worker.max_steps,
            max_wall_sec=self.deps.config.worker.max_wall_sec,
        )
        await self.deps.store.create_task(task)
        await self.queue.put(task)

    async def run_forever(self) -> None:
        while True:
            task = await self.queue.get()
            await asyncio.sleep(self.pickup_delay_ms / 1000)
            if not self.picks_up:                     # 任务永远停在 created
                continue
            task.status = TaskStatus.planning         # worker 领走的第一件事（§4.5）
            await self.deps.store.update_task(task)
            task.status = TaskStatus.delivered
            await self.deps.store.update_task(task)


class NeverPicksUpPlane(SlowPickupPlane):
    """任务只建不领 —— 用来验 `after: running` 等不到时的失败姿态。"""

    picks_up = False


class NeverQuietPlane(SlowPickupPlane):
    """一刻不停地调替身 —— 用来验 `after: idle` 等不到时的失败姿态。"""

    async def run_forever(self) -> None:
        while True:
            await self.deps.store.get_task("没这个任务")   # 每一圈都往 CallLog 里加一笔
            await asyncio.sleep(0.001)


def timing_scenario(*, after: str, after_timeout_sec: float = 5.0) -> Scenario:
    """两条事件的最小场景：e2 按 `after` 决定投之前等什么。"""
    return Scenario(
        name="t5_timing",
        events=[
            EventSpec(event_id="e1", text="起个活"),
            EventSpec(event_id="e2", text="再来一句", after=after, after_timeout_sec=after_timeout_sec),
        ],
    )


def capturing_factory(box: list):
    """接真 ControlPlane，同时把它扣在 box 里 —— 测试要读它的 counters。"""

    async def factory(deps: Deps):
        plane = await build_control_plane(deps)
        box.append(plane)
        return plane

    return factory


# --- 1. 默认值与 none 的行为 --------------------------------------------------

def test_after_defaults_to_none_with_a_five_second_budget():
    """契约 C-T5T6-1 的默认值。改这两个默认值会连坐 T6 的 07。"""
    spec = EventSpec(event_id="e1")
    assert spec.after == "none"
    assert spec.after_timeout_sec == 5.0


def test_unknown_after_values_are_rejected_at_load_time():
    """只认 none / idle / running；写错了要在加载场景时就炸，不是投到一半才发现。"""
    with pytest.raises(ValidationError):
        EventSpec(event_id="e1", after="soon")


def test_the_eight_green_scenarios_still_dispatch_back_to_back(suite):
    """硬约束 1：本轨只该动 02。其余八个一条 `after` 都不许多出来（07 归 T6）。"""
    for sc in suite:
        if sc.name not in UNTOUCHED_SCENARIOS:
            continue
        assert [e.after for e in sc.events] == ["none"] * len(sc.events), (
            f"{sc.name} 被加了投递等待，8 个绿场景的 stats 会跟着变"
        )


async def test_after_none_keeps_a_green_scenario_byte_for_byte(scenarios_by_name):
    """拿 01 做对照：全程 `after: none`，stats 必须和基线逐字一致。"""
    r = await run_scenario(scenarios_by_name["01_simple_qa"])
    assert r.passed is True, f"01 没过：{r.reason} / {r.failures}"
    assert r.stats == BASELINE_01_STATS


# --- 2. after: idle 真的等到了静默 --------------------------------------------

async def test_02_without_the_wait_is_merged_into_a_steer(scenarios_by_name):
    """不等就投 = 改动前的样子：R6 看到活跃 task，把 e2 排成 steer，只有一个 task。

    这是 T2 对 R6 的正确实现，不是缺陷 —— 所以要改的是 runner 怎么投。
    """
    sc = scenarios_by_name["02_thread_followup"].model_copy(deep=True)
    assert sc.events[1].after == "idle", "02 的 e2 应该是 after: idle"
    sc.events[1].after = "none"

    box: list = []
    r = await run_scenario(sc, plane_factory=capturing_factory(box))

    assert r.passed is False and r.phase == "assert"
    assert r.stats["tasks"] == 1 and r.stats["sessions"] == 1
    assert box[0].counters["events.steer"] == 1


async def test_02_with_after_idle_starts_a_second_task(scenarios_by_name):
    """等 e1 那个任务收了再投，R6 就走「没有活跃 task → 新建 task 继续」那一支。"""
    box: list = []
    r = await run_scenario(scenarios_by_name["02_thread_followup"], plane_factory=capturing_factory(box))

    assert r.passed is True, f"02 没过：{r.reason} / {r.failures}"
    assert r.stats["tasks"] == 2 and r.stats["sessions"] == 1
    assert box[0].counters["events.steer"] == 0


# --- 3. after: running 判的是「worker 真的领走了」 -----------------------------

async def test_after_none_sees_the_task_still_queued():
    """对照组：不等的话 e2 到达时任务还躺在队列里，状态是 created。"""
    plane_box: list = []

    def factory(deps: Deps):
        plane_box.append(SlowPickupPlane(deps))
        return plane_box[-1]

    r = await run_scenario(timing_scenario(after="none"), plane_factory=factory)

    assert r.phase == "ok", f"{r.reason} / {r.failures}"
    assert plane_box[0].status_on_arrival == [("e1", None), ("e2", "created")]


async def test_after_running_waits_until_the_worker_actually_picks_it_up():
    """`created` 不算 running —— 任务建好但还在队列里时，投递必须继续等。"""
    plane_box: list = []

    def factory(deps: Deps):
        plane_box.append(SlowPickupPlane(deps))
        return plane_box[-1]

    r = await run_scenario(timing_scenario(after="running"), plane_factory=factory)

    assert r.phase == "ok", f"{r.reason} / {r.failures}"
    arrivals = plane_box[0].status_on_arrival
    assert arrivals[0] == ("e1", None)
    assert arrivals[1][0] == "e2"
    assert arrivals[1][1] != str(TaskStatus.created), "e2 投早了：任务还没被领走"


# --- 4. 等不到就失败，不许继续投 ----------------------------------------------

async def test_running_that_never_lands_fails_at_dispatch():
    """任务只建不领 → 场景以 dispatch 失败收场，reason 点名等的是什么、等了多久。"""
    r = await run_scenario(
        timing_scenario(after="running", after_timeout_sec=0.05),
        plane_factory=lambda deps: NeverPicksUpPlane(deps),
    )

    assert r.passed is False
    assert r.phase == "dispatch"
    assert "\n" not in r.reason and "Traceback" not in r.reason
    assert "e2" in r.reason                            # 卡在哪条事件
    assert "after=running" in r.reason                 # 等的是什么
    assert "created" in r.reason                       # 为什么没等到
    assert "0.05" in r.reason                          # 等了多久


async def test_idle_that_never_settles_fails_at_dispatch():
    """系统一直在动 → 同样是 dispatch 失败，而不是「等不到就接着投」。"""
    r = await run_scenario(
        timing_scenario(after="idle", after_timeout_sec=0.05),
        plane_factory=lambda deps: NeverQuietPlane(deps),
    )

    assert r.passed is False
    assert r.phase == "dispatch"
    assert "\n" not in r.reason and "Traceback" not in r.reason
    assert "e2" in r.reason
    assert "after=idle" in r.reason
    assert "0.05" in r.reason


async def test_a_failed_wait_stops_the_dispatch_loop():
    """等不到就不许把后面的事件投出去 —— 投出去测出来的绿是假的。"""
    sc = Scenario(
        name="t5_timing_halts",
        events=[
            EventSpec(event_id="e1", text="起个活"),
            EventSpec(event_id="e2", text="等不到", after="running", after_timeout_sec=0.05),
            EventSpec(event_id="e3", text="这条不该被投出去"),
        ],
    )
    plane_box: list = []

    def factory(deps: Deps):
        plane_box.append(NeverPicksUpPlane(deps))
        return plane_box[-1]

    r = await run_scenario(sc, plane_factory=factory)

    assert r.phase == "dispatch"
    assert [eid for eid, _ in plane_box[0].status_on_arrival] == ["e1"]
