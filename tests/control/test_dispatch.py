"""run_forever：派发队列里的任务给 worker，并跑沙箱 reaper（W7：每 60s 一次）。"""
import asyncio
import contextlib

import pytest
from control_fakes import ParkedSleep, ScriptedModel, final_turn, make_event

from aite.contracts import TaskStatus
from aite.control import REAPER_INTERVAL_SEC


@contextlib.asynccontextmanager
async def _running(plane):
    runner = asyncio.ensure_future(plane.run_forever())
    try:
        yield runner
    finally:
        runner.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await runner


async def test_run_forever_dispatches_queued_tasks(make_plane, store, platform, clock):
    model = ScriptedModel([final_turn("答完了")], clock=clock)
    plane = make_plane(model, sleep=ParkedSleep(limit=1))

    await plane.handle_event(make_event(text="一个问题"))
    task_id = (await store.list_active_tasks("oc_chat"))[0].id

    async with _running(plane):
        await asyncio.wait_for(plane.join(), timeout=2)

    assert (await store.get_task(task_id)).status is TaskStatus.delivered
    assert platform.texts[-1].text == "答完了"


async def test_reaper_calls_reap_idle_on_the_configured_cadence(make_plane, sandbox, config):
    sleeper = ParkedSleep(limit=3)          # 转 2 圈后停住
    sandbox.reap_returns = ["sb_1"]
    plane = make_plane(sleep=sleeper)

    async with _running(plane):
        await asyncio.wait_for(sleeper.reached.wait(), timeout=2)

    assert sleeper.calls[:2] == [REAPER_INTERVAL_SEC, REAPER_INTERVAL_SEC] == [60.0, 60.0]
    assert sandbox.reap_calls == [config.sandbox.idle_sec] * 2 == [300, 300]
    assert plane.counters["sandbox.reaped"] == 2


async def test_reaper_survives_a_failing_sandbox(make_plane, sandbox):
    async def boom(idle_sec):
        raise RuntimeError("docker 不在")

    sandbox.reap_idle = boom
    sleeper = ParkedSleep(limit=3)
    plane = make_plane(sleep=sleeper)

    async with _running(plane):
        await asyncio.wait_for(sleeper.reached.wait(), timeout=2)

    assert len(sleeper.calls) == 3          # 抛异常没打断循环


async def test_dispatch_failure_does_not_kill_the_loop(make_plane, store, platform, clock):
    """§3.3：任何未捕获异常 → task failed + 回帖，进程不退出。第一个炸了第二个照跑。"""
    n = {"send": 0}
    ok_send = platform.send_text

    async def flaky(msg):
        n["send"] += 1
        if n["send"] == 1:                  # 交付第一个任务时平台抽风
            raise RuntimeError("平台抽风")
        return await ok_send(msg)

    platform.send_text = flaky
    model = ScriptedModel([final_turn("第一份结果"), final_turn("第二份结果")], clock=clock)
    plane = make_plane(model, sleep=ParkedSleep(limit=1))

    await plane.handle_event(make_event(event_id="e1", text="会炸的", message_id="om_1"))
    first_id = (await store.list_active_tasks("oc_chat"))[0].id

    async with _running(plane):
        await asyncio.wait_for(plane.join(), timeout=2)
        assert (await store.get_task(first_id)).status is TaskStatus.failed

        await plane.handle_event(make_event(event_id="e2", text="第二件事", message_id="om_2"))
        second_id = (await store.list_active_tasks("oc_chat"))[0].id
        await asyncio.wait_for(plane.join(), timeout=2)

    assert (await store.get_task(second_id)).status is TaskStatus.delivered
    assert platform.texts[-1].text == "第二份结果"


@pytest.mark.parametrize("qsize", [0, 1, 3])
async def test_run_pending_drains_without_a_worker(plane, qsize):
    """没配 model 的控制面：run_pending 不该抛，只是什么都不做。"""
    for i in range(qsize):
        await plane.handle_event(make_event(event_id=f"e{i}", text="活儿", message_id=f"om_{i}"))
    assert plane.pending == qsize
    await plane.run_pending()
    assert plane.pending == 0
