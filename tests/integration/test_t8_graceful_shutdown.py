"""第 3 组：优雅退出（C-TΩ-1 的退出序列）。

冻结下来的顺序是：

    platform.stop()            ← 先闭嘴，不再收新事件
    plane.join() 限时 shutdown_grace_sec
    （超时才取消 run_forever）
    sandbox.aclose()（有沙箱才调）
    store.close()

所以这一组要钉住的不是「退出了」，而是**退出的顺序**：闭嘴要发生在收尾之前，
收尾期间到达的事件一条都不许被处理，而在跑的任务必须善终。

怎么把「正在跑」做成确定的：`FakeModel` 的 `hold_ticks` 让 worker 停在
「下一次 chat」上 —— 上一步的副作用（卡片、库里的状态）都已经落地，别的协程
（我们的收尾）终于插得进来。放行由 `release_holds()` 显式给出，不靠睡时间。
"""
import pytest
from app_under_test import build_app
from integration_fakes import (
    CHAT,
    ClosableFakeSandbox,
    GatedPlatform,
    RecordingModel,
    evidence_task_dirs,
    final_step,
    make_event,
    read_manifest,
    read_task_from_disk,
    running_app,
    settle,
    tool_step,
    wait_until,
)

from aite.contracts import TaskStatus
from aite.testing import FakeSandbox

#: 第 2 步停在 chat 里不返回，直到测试显式 release_holds()。
#: hold_ticks 给得足够大 = 「没人放行就一直停着」，但它有上限，所以永远不会挂死。
HOLDING_SCRIPT = [
    tool_step("checklist_add", {"items": ["干活"]}),
    final_step("收尾时把手上的活干完了。", hold_ticks=10**6),
]


async def test_stop_closes_the_platform_before_draining_and_the_store_last(config):
    """在跑的任务善终；`platform.stop()` 在它交付之前；收尾期间的新事件被丢掉。"""
    platform = GatedPlatform()
    model = RecordingModel(HOLDING_SCRIPT)
    sandbox = ClosableFakeSandbox()
    app = build_app(config, platform=platform, model=model, sandbox=sandbox)

    async with running_app(app) as run:
        await platform.emit(make_event(event_id="e1", text="干个长活"))
        task = (await app.store.list_active_tasks(CHAT))[0]

        # 等到任务真的在跑：卡片发出去了（W3），模型停在第 2 步上
        await wait_until(lambda: model.holds >= 1, what="worker 停在第 2 步的 chat 上")
        assert platform.card_count == 1
        assert (await app.store.get_task(task.id)).status is TaskStatus.working

        run.stop.set()
        await wait_until(lambda: platform.stopped, what="run_app 调 platform.stop()")

        # 收尾期间平台上又来了一条消息 —— 连接已经断了，它不该被处理
        await platform.emit(make_event(event_id="e2", text="收尾期间的新消息", message_id="om_2"))
        assert platform.dropped_after_stop == 1

        model.release_holds()                 # 手上的活可以收了

    # ── 退出之后 ────────────────────────────────────────────────
    methods = platform.calls.methods()
    assert "stop" in methods
    last_send_text = len(methods) - 1 - methods[::-1].index("send_text")
    assert methods.index("stop") < last_send_text, (
        f"platform.stop() 必须发生在在跑任务的交付之前，实际顺序：{methods}"
    )

    # 在跑的任务善终了：状态、证据、manifest 一个不少
    finished = await read_task_from_disk(config, task.id)
    assert finished is not None and finished.status is TaskStatus.delivered
    assert read_manifest(config, task.id)["root_hash"] == finished.evidence_root_hash

    # 第二条事件一点痕迹都没留下
    assert evidence_task_dirs(config) == [task.id]
    assert model.call_count == 2               # 只有脚本里那两步，e2 没惊动模型
    assert app.ingress.counters["events.handled"] == 1  # ingress 只处理过 e1

    # store.close() 调过了：再用它查任何东西都该报「未初始化」
    with pytest.raises(RuntimeError):
        await app.store.get_task(task.id)

    # 有沙箱就调 aclose（C-TΩ-1 退出序列倒数第二步）
    assert sandbox.aclose_calls == 1


async def test_stop_with_nothing_running_returns_at_once(config):
    """队列是空的时候，收尾不许干等宽限期。

    这里刻意**不传** `shutdown_grace_sec`，走 C-TΩ-1 的默认 20.0：
    `exit_timeout=2.0` 还能过，就说明 `plane.join()` 是等队列而不是等钟。
    """
    platform = GatedPlatform()
    app = build_app(
        config,
        platform=platform,
        model=RecordingModel([final_step("没人叫我。")]),
        # 纯 FakeSandbox：它实现的就是 §3.2 冻结的 SandboxPort，**没有** aclose。
        # run_app 不许因为协议面之外的方法不存在就炸掉。
        sandbox=FakeSandbox(),
    )
    async with running_app(app, exit_timeout=2.0):
        pass

    assert platform.stopped is True
    assert platform.outbound_count == 0
    with pytest.raises(RuntimeError):
        await app.store.get_task("whatever")


async def test_events_arriving_after_shutdown_change_nothing(config):
    """退出之后平台再推事件（比如 adapter 没停干净），系统状态一动不动。"""
    platform = GatedPlatform()
    model = RecordingModel([final_step("干完了。")])
    app = build_app(config, platform=platform, model=model, sandbox=FakeSandbox())

    async with running_app(app):
        await platform.emit(make_event(event_id="e1", text="干个活"))
        task = (await app.store.list_active_tasks(CHAT))[0]
        await wait_until(lambda: platform.count("send_text") == 1, what="任务交付")

    before = platform.outbound_count
    await platform.emit(make_event(event_id="e2", text="人走了才到", message_id="om_2"))
    await settle()

    assert platform.dropped_after_stop == 1
    assert platform.outbound_count == before
    assert model.call_count == 1
    assert evidence_task_dirs(config) == [task.id]
