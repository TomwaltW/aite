"""跑到一半被 `!stop` / 卡片 stop 打断（§3.3 + W4 的收尾更新）。"""
import json

from worker_fakes import FakePlatform, ScriptedModel, final_turn, make_event, tool_turn

from aite.contracts import EvidenceKind, TaskStatus
from aite.control import InProcessControlPlane
from aite.evidence import FileEvidenceWriter


async def test_stop_mid_run_cancels_at_next_step(store, config, clock, sandbox, gateway):
    platform = FakePlatform()
    plane: InProcessControlPlane | None = None
    task_id: str | None = None

    async def inject(step: int) -> None:
        if step == 0:                       # 第一步刚开始跑，就有人点了 stop
            await plane.cancel_task(await store.get_task(task_id))

    model = ScriptedModel(
        [
            tool_turn(("checklist_add", {"items": ["取数", "画图"]})),
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
    assert len(model.calls) == 1                     # 下一步开始前就停了
    assert platform.texts == [platform.texts[0]] and "已停止" in platform.texts[0].text

    # W4：结束时必定再更新一次卡片，状态是 cancelled
    assert len(platform.cards) == 1
    assert platform.card_updates[-1][1].status == "cancelled"

    # 控制面与 worker 不重复写 cancelled
    kinds = [
        json.loads(x)["kind"]
        for x in evidence.events_path(task_id).read_text(encoding="utf-8").splitlines()
    ]
    assert kinds.count(EvidenceKind.cancelled.value) == 1
    assert evidence.verify(task_id) is True
    assert task.evidence_root_hash is not None


async def test_stop_before_dispatch_never_runs_the_worker(store, config, clock, sandbox, gateway):
    """还在队列里就被 stop：worker 一次都不该被调用。"""
    platform = FakePlatform()
    model = ScriptedModel([final_turn("不该跑")], clock=clock)
    evidence = FileEvidenceWriter(config.storage.evidence_dir)
    plane = InProcessControlPlane(
        store=store, platform=platform, evidence=evidence, config=config,
        model=model, gateway=gateway, sandbox=sandbox, clock=clock, sleep=clock.sleep,
    )

    await plane.handle_event(make_event(text="马上就后悔"))
    task = (await store.list_active_tasks("oc_chat"))[0]
    await plane.cancel_task(task)
    await plane.run_pending()

    assert model.calls == []
    assert (await store.get_task(task.id)).status is TaskStatus.cancelled
    assert platform.cards == []
