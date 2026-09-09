"""R6 排队的 steer 消息，worker 每步开始前合并进上下文。"""
from worker_fakes import (
    FakePlatform,
    ScriptedModel,
    final_turn,
    make_event,
    tool_turn,
)

from aite.control import InProcessControlPlane
from aite.evidence import FileEvidenceWriter


async def test_steer_message_reaches_the_next_step(store, config, clock, sandbox, gateway):
    platform = FakePlatform()
    plane: InProcessControlPlane | None = None

    async def inject(step: int) -> None:
        # 第一步跑起来之后，话题里又来一句（不带 @）：R6 判定为 steer
        if step == 1:
            await plane.handle_event(
                make_event(
                    event_id="ev2", text="顺便加上同比", mentioned=False,
                    message_id="om_2", thread_id="om_1",
                )
            )

    model = ScriptedModel(
        [
            tool_turn(("checklist_add", {"items": ["取数"]})),
            tool_turn(("checklist_check", {"id": "c1"})),
            final_turn("加上同比之后的结果。"),
        ],
        clock=clock,
        step_seconds=0.6,
        on_call=inject,
    )
    plane = InProcessControlPlane(
        store=store,
        platform=platform,
        evidence=FileEvidenceWriter(config.storage.evidence_dir),
        config=config,
        model=model,
        gateway=gateway,
        sandbox=sandbox,
        clock=clock,
        sleep=clock.sleep,
    )

    await plane.handle_event(make_event(text="按月画个图"))
    task = (await store.list_active_tasks("oc_chat"))[0]
    await plane.run_pending()

    # 第 2 步（下标 1）之前注入，所以第 3 步（下标 2）的上下文里必须有它
    step3 = model.calls[2]
    assert any(m.role == "user" and m.content == "顺便加上同比" for m in step3)
    assert not any(m.role == "user" and m.content == "顺便加上同比" for m in model.calls[1])

    assert plane.pending_steer(task.id) == []            # 已被消费
    assert len(await store.list_active_tasks("oc_chat")) == 0   # 没有第二个 task
    assert [t.content for t in await store.list_turns(task.session_id)] == ["按月画个图", "顺便加上同比"]
