"""第 4 组：宽限期超时（C-TΩ-1 退出序列的「超时 → 取消」那一支）。

造一个收不完的任务：模型停在第 2 次 `chat` 上，没人放行。把 `shutdown_grace_sec`
压到 50ms，于是 `plane.join()` 必然超时，接下来要看的是：

* `run_app` 仍然在有限时间内返回 —— 不许挂死；
* 被取消的那条 `run_forever` 真的死了 —— 不许留一条还在空转的协程；
* 沙箱还回去了；
* `store.close()` 照样走到。

第 3 条在纯 `SandboxPort` 上今天做不到，见本文件末尾那条 xfail 与回执 D-1 / D-2。
"""
import asyncio

import pytest
from app_under_test import build_app
from integration_fakes import (
    CHAT,
    ClosableFakeSandbox,
    GatedPlatform,
    RecordingModel,
    final_step,
    make_event,
    manifest_path,
    read_task_from_disk,
    running_app,
    settle,
    tool_step,
    wait_until,
)

from aite.contracts import TaskStatus
from aite.testing import FakeSandbox

#: 宽限期压到 50ms：`plane.join()` 必然超时，走取消那一支。
TINY_GRACE_SEC = 0.05

#: 第 1 步先把沙箱建起来（`run_python` 会让 Gateway acquire），
#: 第 2 步停在 chat 里 —— hold_ticks 大到没人放行就等于永远收不完。
STUCK_SCRIPT = [
    tool_step("run_python", {"code": "print('起个沙箱')", "timeout_sec": 30}),
    final_step("永远到不了这一句。", hold_ticks=10**9),
]


async def _drive_until_stuck(app, platform, model, sandbox):
    """把任务推到「沙箱已建、模型停在第 2 步」这个确定的状态。"""
    await platform.emit(make_event(text="干个收不完的活"))
    task = (await app.store.list_active_tasks(CHAT))[0]
    await wait_until(
        lambda: sandbox.calls.count("acquire") == 1 and model.holds >= 1,
        what="沙箱建好、模型停在第 2 步",
    )
    assert sandbox.alive, "run_python 之后应当有一个活着的沙箱"
    return task


async def test_grace_timeout_cancels_the_stuck_task_and_still_returns(config):
    platform = GatedPlatform()
    model = RecordingModel(STUCK_SCRIPT)
    sandbox = ClosableFakeSandbox()
    app = build_app(config, platform=platform, model=model, sandbox=sandbox)

    started = asyncio.get_running_loop().time()
    async with running_app(app, shutdown_grace_sec=TINY_GRACE_SEC, exit_timeout=5.0) as run:
        task = await _drive_until_stuck(app, platform, model, sandbox)
        sandbox_id = sandbox.alive[0]
        run.stop.set()
    elapsed = asyncio.get_running_loop().time() - started

    # 不许挂死：宽限期 50ms，整轮下来远不该到秒级
    assert elapsed < 3.0, f"run_app 花了 {elapsed:.3f}s 才返回，宽限期只有 {TINY_GRACE_SEC}s"

    # 被取消的协程真的死了：再让出一批 tick，模型那个自旋不许再往前走一格
    frozen = model.hold_ticks_yielded
    await settle(200)
    assert model.hold_ticks_yielded == frozen, "run_forever 被取消了，卡住的 worker 却还在跑"
    assert model.turns_served == 1, "第 2 步不该出牌"

    # 任务确实没交付
    assert platform.count("send_text") == 0
    assert platform.count("send_file") == 0

    # 沙箱还回去了 —— 走的是 C-TΩ-1 退出序列里的 `sandbox.aclose()`
    assert sandbox.aclose_calls == 1
    assert sandbox.released_ids == [sandbox_id]
    assert sandbox.alive == []

    # store.close() 照样走到
    with pytest.raises(RuntimeError):
        await app.store.get_task(task.id)


async def test_grace_timeout_survives_a_protocol_only_sandbox(config):
    """沙箱只实现 §3.2 冻结的 `SandboxPort`（没有 `aclose`）时，收尾不许炸。

    C-TΩ-1 的退出序列写着 `sandbox.aclose()`，但 `aclose` **不在** `SandboxPort` 里 ——
    只有 `DockerSandbox` 有。裸调的话，任何别的沙箱实现都会让收尾崩在这一步，
    而收尾崩了 = `store.close()` 走不到 = SQLite 连接泄漏。
    """
    platform = GatedPlatform()
    model = RecordingModel(STUCK_SCRIPT)
    sandbox = FakeSandbox()
    assert not hasattr(sandbox, "aclose"), "FakeSandbox 就该只有协议面上的方法"

    app = build_app(config, platform=platform, model=model, sandbox=sandbox)
    async with running_app(app, shutdown_grace_sec=TINY_GRACE_SEC, exit_timeout=5.0) as run:
        task = await _drive_until_stuck(app, platform, model, sandbox)
        run.stop.set()

    # 收尾整段走完了：平台停了、库关了
    assert platform.stopped is True
    with pytest.raises(RuntimeError):
        await app.store.get_task(task.id)

    # 被取消的协程也死透了
    frozen = model.hold_ticks_yielded
    await settle(200)
    assert model.hold_ticks_yielded == frozen


@pytest.mark.xfail(
    strict=True,
    reason=(
        "已知缺陷（回执 D-2）：宽限期超时时 run_app 取消 run_forever，但 asyncio 取消"
        "不会走到 AgentWorker._cancel() —— 那条路只由 is_cancelled() 触发。于是沙箱不还、"
        "任务状态停在 working（重启后一直挂在 !status 上）、证据链也没 finalize。"
        "C-TΩ-1 括号里那句「worker 的 cancel 路径会还沙箱、把卡片置 cancelled」不成立："
        "DockerSandbox 靠协议外的 aclose() 兜住了沙箱，任务状态与证据则谁都没兜。"
    ),
)
async def test_cancelled_task_should_land_on_cancelled_and_return_its_sandbox(config):
    """这一条钉的是「应该怎样」。今天它是红的 —— 修好之后请把 xfail 摘掉。"""
    platform = GatedPlatform()
    model = RecordingModel(STUCK_SCRIPT)
    sandbox = FakeSandbox()          # 只有 SandboxPort，没有 aclose 兜底
    app = build_app(config, platform=platform, model=model, sandbox=sandbox)

    async with running_app(app, shutdown_grace_sec=TINY_GRACE_SEC, exit_timeout=5.0) as run:
        task = await _drive_until_stuck(app, platform, model, sandbox)
        run.stop.set()

    finished = await read_task_from_disk(config, task.id)
    assert finished is not None
    # 三条一起断言，免得修的人只看见最先炸的那一条
    observed = {
        "沙箱还回去了": sandbox.alive == [],
        "任务落到 cancelled 终态": finished.status is TaskStatus.cancelled,
        "证据链 finalize 了（有 manifest.json）": manifest_path(config, task.id).is_file(),
    }
    assert all(observed.values()), f"被取消的任务没善终：{observed}"
