"""沙箱归属：Gateway 建的沙箱，worker 取产物要用它、任务收尾要还它。

冻结的 ToolGateway 协议只有 catalog / call，沙箱归属没进协议 —— Gateway 按
task_id 自己记着容器，`Task.sandbox_id` 记的则是 worker 兜底建的那种。两边不通
气的话会出两个后果，这个文件把它们各钉一条：

1. **产物丢**：run_python 把 /work/out.png 写在 Gateway 那个沙箱里，worker 取产物
   时看 `task.sandbox_id is None`，自己 acquire 一个全新的空容器，产物必然找不到。
2. **容器泄漏**：Gateway 手上那个沙箱没人 release，只能等 reaper 的 idle_sec
   空闲超时才被收 —— 任务早结束了还占着内存和 CPU 配额。
"""
from worker_fakes import FakePlatform, ScriptedModel, final_turn, make_event, tool_turn

from aite.contracts import TaskStatus
from aite.control import InProcessControlPlane
from aite.evidence import FileEvidenceWriter

PNG = b"\x89PNG\r\n\x1a\n" + b"fake"


# ---- 取产物 ---------------------------------------------------------------


async def test_artifact_is_fetched_from_the_gateway_sandbox(run_task, sandbox, gateway, platform):
    """产物在 Gateway 那个沙箱里 —— worker 得问它要，不能自己另开一个。"""
    sandbox.files["/work/out.png"] = PNG
    script = [
        tool_turn(("run_python", {"code": "plt.savefig('/work/out.png')"})),
        final_turn("图在这里。", [{"path": "/work/out.png", "title": "月度趋势"}]),
    ]

    _, task, _ = await run_task(script, step_seconds=0.6)

    held = "sb_gateway"
    assert [sid for sid, _ in sandbox.get_file_calls] == [held]   # 取的是 Gateway 那个
    assert sandbox.acquired == []                                 # worker 没有另开
    assert len(platform.files) == 1 and platform.files[0].data == PNG
    assert task.status is TaskStatus.delivered


async def test_worker_still_acquires_when_gateway_holds_nothing(run_task, sandbox, platform):
    """兜底路径不变：Gateway 没建过沙箱（比如产物是别处放的），worker 照旧自己建。"""
    sandbox.files["/work/out.txt"] = b"hi"
    script = [final_turn("给你。", [{"path": "/work/out.txt", "title": "结果"}])]

    _, task, _ = await run_task(script)

    assert len(sandbox.acquired) == 1
    assert [sid for sid, _ in sandbox.get_file_calls] == sandbox.acquired
    assert len(platform.files) == 1
    assert task.status is TaskStatus.delivered


# ---- 还沙箱 ---------------------------------------------------------------


async def test_delivered_task_returns_the_gateway_sandbox(run_task, sandbox, gateway):
    """任务交付完，Gateway 的沙箱当场还掉，不等 reaper。"""
    sandbox.files["/work/out.png"] = PNG
    script = [
        tool_turn(("run_python", {"code": "plt.savefig('/work/out.png')"})),
        final_turn("图在这里。", [{"path": "/work/out.png", "title": "月度趋势"}]),
    ]

    _, task, _ = await run_task(script, step_seconds=0.6)

    assert task.status is TaskStatus.delivered
    assert gateway.released_tasks == [task.id]
    assert sandbox.released == ["sb_gateway"]
    assert gateway.sandbox_id_of(task.id) is None


async def test_failed_task_returns_the_gateway_sandbox(run_task, sandbox, gateway):
    """failed 也走同一个收尾口，沙箱一样要还。"""
    script = [tool_turn(("run_python", {"code": "1/0"}))] * 3
    _, task, _ = await run_task(script, repeat=tool_turn(("run_python", {"code": "1/0"})))

    assert task.status is TaskStatus.failed        # 撞步数上限
    assert gateway.released_tasks == [task.id]
    assert sandbox.released == ["sb_gateway"]


async def test_cancel_mid_run_returns_the_gateway_sandbox(store, config, clock, sandbox, gateway):
    """跑到一半被 !stop：任务在 worker 手里，收尾由 worker 做，沙箱在那里还。"""
    platform = FakePlatform()
    plane: InProcessControlPlane | None = None
    task_id: str | None = None

    async def inject(step: int) -> None:
        if step == 1:                   # 第一步（run_python）已经建了沙箱，这时点 stop
            await plane.cancel_task(await store.get_task(task_id))

    model = ScriptedModel(
        [
            tool_turn(("run_python", {"code": "长活"})),
            tool_turn(("checklist_note", {"text": "还在算"})),   # 跑完这步回到循环开头才检查取消
            final_turn("不该走到这一步"),
        ],
        clock=clock,
        step_seconds=0.6,
        on_call=inject,
    )
    evidence = FileEvidenceWriter(config.storage.evidence_dir)
    plane = InProcessControlPlane(
        store=store, platform=platform, evidence=evidence, config=config,
        model=model, gateway=gateway, sandbox=sandbox, clock=clock, sleep=clock.sleep,
    )

    await plane.handle_event(make_event(text="跑个长活"))
    task_id = (await store.list_active_tasks("oc_chat"))[0].id
    await plane.run_pending()

    task = await store.get_task(task_id)
    assert task.status is TaskStatus.cancelled
    assert gateway.released_tasks == [task_id]
    assert sandbox.released == ["sb_gateway"]


async def test_cancel_before_worker_starts_returns_the_gateway_sandbox(
    store, config, clock, sandbox, gateway, platform
):
    """任务还没被 worker 领走就被停：worker 不会再收尾，得由控制面还沙箱。"""
    evidence = FileEvidenceWriter(config.storage.evidence_dir)
    plane = InProcessControlPlane(
        store=store, platform=platform, evidence=evidence, config=config,
        model=ScriptedModel([final_turn("不该跑到这")], clock=clock),
        gateway=gateway, sandbox=sandbox, clock=clock, sleep=clock.sleep,
    )

    await plane.handle_event(make_event(text="先建个任务"))
    task = (await store.list_active_tasks("oc_chat"))[0]
    gateway.hold_sandbox(task.id)          # 演上一轮工具调用留下的沙箱

    await plane.cancel_task(task)          # 还没 run_pending，任务不在 worker 手里

    assert (await store.get_task(task.id)).status is TaskStatus.cancelled
    assert gateway.released_tasks == [task.id]
    assert sandbox.released == ["sb_gateway"]
