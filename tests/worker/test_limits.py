"""T2 独有验收：脚本化模型死循环 → 第 40 步 `failed` 且 `send_text` 含「上限」（W6 / §3.3）。"""
from worker_fakes import final_turn, text_turn, tool_turn

from aite.contracts import TaskStatus

LOOP_TURN = tool_turn(("checklist_note", {"text": "再想想"}))


async def test_step_limit_fails_at_max_steps(run_task, platform, config):
    _, task, model = await run_task(None, repeat=LOOP_TURN, step_seconds=0.6)

    assert config.worker.max_steps == 40
    assert len(model.calls) == 40                  # 第 40 步之后不再调模型
    assert task.steps == 40
    assert task.status is TaskStatus.failed
    assert "上限" in platform.texts[-1].text
    assert task.task_no in platform.texts[-1].text


async def test_step_limit_closes_the_card(run_task, platform):
    _, _task, _ = await run_task(None, repeat=LOOP_TURN, step_seconds=0.6)

    assert len(platform.cards) == 1
    assert platform.card_updates[-1][1].status == "failed"   # W4：结束时必定再更新一次


async def test_wall_clock_limit(run_task, platform, config):
    """每步 700s、上限 1200s → 第 2 步之后超时收工。"""
    _, task, model = await run_task(None, repeat=LOOP_TURN, step_seconds=700.0)

    assert config.worker.max_wall_sec == 1200
    assert len(model.calls) == 2
    assert task.status is TaskStatus.failed
    assert "上限" in platform.texts[-1].text


async def test_model_failure_retries_twice_then_fails(run_task, platform):
    """§3.3：模型异常重试 2 次（共 3 次调用）仍失败 → task failed + 「模型服务暂不可用」。"""
    _, task, model = await run_task([final_turn("不会走到这")], fail_times=3)

    assert len(model.calls) == 3
    assert task.status is TaskStatus.failed
    assert "模型服务暂不可用" in platform.texts[-1].text
    assert task.task_no in platform.texts[-1].text


async def test_model_failure_recovers_within_retries(run_task, platform):
    _, task, model = await run_task([final_turn("重试后成功了")], fail_times=2)

    assert len(model.calls) == 3
    assert task.status is TaskStatus.delivered
    assert platform.texts[-1].text == "重试后成功了"


async def test_repeated_invalid_args_fails(run_task, platform):
    """连续 3 次不合 schema 的工具参数 → failed（§3.3）。"""
    bad = tool_turn(("checklist_add", {"items": []}))
    _, task, model = await run_task(None, repeat=bad, step_seconds=0.6)

    assert len(model.calls) == 3
    assert task.status is TaskStatus.failed
    assert "不合法的工具参数" in platform.texts[-1].text


async def test_text_only_after_first_step_is_nudged_not_delivered(run_task, platform):
    """§3.3：steps>0 时只回文本 → 提示它调 final，计 1 步，不当成交付。"""
    script = [
        tool_turn(("checklist_add", {"items": ["先想想"]})),
        text_turn("我觉得可以这样做"),
        final_turn("结论在这里"),
    ]
    _, task, model = await run_task(script, step_seconds=0.6)

    assert task.status is TaskStatus.delivered
    assert len(model.calls) == 3
    assert platform.texts[-1].text == "结论在这里"
    nudges = [m for m in model.calls[-1] if m.role == "system" and "请调用 final" in m.content]
    assert len(nudges) == 1
