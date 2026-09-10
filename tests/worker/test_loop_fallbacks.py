"""T20：同一张牌原样连发 → 第 3 次提示换招、第 5 次 failed（§3.3 那张表接不住的那种）。

T17 实测 `04_csv_to_chart`：替身沙箱对不含 `savefig` 的代码一律回 `exit_code=0` + 空
stdout，模型自己诊断了几步之后对**逐字节相同**的 `run_python` 连发 33 次，一路烧到
`max_steps` 才停（`evals/live-report-2026-09-10.md` §4 缺口 2）。§3.3 的四条兜底一条都
接不住：参数完全合法、工具返回 `ok=True`、没超时、每次都有 tool_call。

这里的 FakeGateway 对没预置的工具正好返回 `ok=True` + 一句没用的话，跟那个形状同构。
"""
import pytest
from worker_fakes import final_turn, tool_turn

from aite.contracts import TaskStatus, ToolCallRequest
from aite.worker.loop import (
    MAX_CONSECUTIVE_REPEATS,
    REPEAT_NUDGE_AT,
    _call_signature,
)

#: T17 里那张牌：参数合法、工具成功、内容没用。
SPIN = ("run_python", {"code": "print(open('/work/in/file_sales_csv').read())"})
SPIN_TURN = tool_turn(SPIN)


def _systems(messages, needle: str) -> list[str]:
    return [m.content for m in messages if m.role == "system" and needle in (m.content or "")]


def _gateway_names(gateway) -> list[str]:
    return [req.name for _ctx, req in gateway.calls]


# ---- 兜底开火 -------------------------------------------------------------


async def test_repeated_gateway_call_fails_at_the_fifth(run_task, platform):
    """连续第 5 次原样重复 → failed，不再等 max_steps。"""
    _, task, model = await run_task(None, repeat=SPIN_TURN, step_seconds=0.6)

    assert len(model.calls) == MAX_CONSECUTIVE_REPEATS == 5   # 第 5 步之后不再调模型
    assert task.status is TaskStatus.failed
    assert "原样重复调用" in platform.texts[-1].text
    assert "run_python" in platform.texts[-1].text
    assert task.task_no in platform.texts[-1].text


async def test_failing_early_saves_the_fifth_sandbox_run(run_task, gateway):
    """第 5 次在**执行之前**就收 —— 前 4 次结果一模一样，没必要再起一次沙箱执行。"""
    await run_task(None, repeat=SPIN_TURN, step_seconds=0.6)

    assert _gateway_names(gateway) == ["run_python"] * 4


async def test_repeat_failure_closes_the_card(run_task, platform):
    """W4：终态照样把卡片收干净，只此一张。"""
    await run_task(None, repeat=SPIN_TURN, step_seconds=0.6)

    assert len(platform.cards) == 1
    assert platform.card_updates[-1][1].status == "failed"


# ---- 提示换招 -------------------------------------------------------------


async def test_third_repeat_nudges_the_model_to_change_tack(run_task):
    """第 3 次回一条 system 提示：给结论 + 两个具体的下一步，而不是「你重复了」。"""
    _, _task, model = await run_task(None, repeat=SPIN_TURN, step_seconds=0.6)

    nudges = _systems(model.calls[-1], "这条路走不通")
    assert len(nudges) == 1
    assert "run_python" in nudges[0]
    assert str(REPEAT_NUDGE_AT) in nudges[0]
    assert "换个做法" in nudges[0] and "final" in nudges[0]


async def test_nudge_does_not_end_the_task(run_task, platform):
    """提示只是提示：模型换招后照常交付。"""
    script = [SPIN_TURN, SPIN_TURN, SPIN_TURN, final_turn("换了个法子，拿到了")]
    _, task, model = await run_task(script, step_seconds=0.6)

    assert task.status is TaskStatus.delivered
    assert platform.texts[-1].text == "换了个法子，拿到了"
    assert len(_systems(model.calls[-1], "这条路走不通")) == 1


# ---- 「连续」的边界 -------------------------------------------------------


async def test_different_arguments_are_not_a_repeat(run_task, platform):
    """参数不同就是另一张牌。正常探测会连着调同一个工具，不该被算成打转。"""
    script = [tool_turn(("run_python", {"code": f"print({i})"})) for i in range(6)]
    script.append(final_turn("探完了"))
    _, task, model = await run_task(script, step_seconds=0.6)

    assert len(model.calls) == 7
    assert task.status is TaskStatus.delivered
    assert platform.texts[-1].text == "探完了"


async def test_a_different_call_in_between_resets_the_run(run_task, platform):
    """中间插进别的牌 → 清零。换招了说明模型还在推进，不算卡住。

    这里一共出了 5 张一样的 run_python，只是被 list_files 断成 2 + 3；不清零的话
    第 5 张就该 failed 了。
    """
    script = [
        SPIN_TURN,
        SPIN_TURN,
        tool_turn(("list_files", {})),
        SPIN_TURN,
        SPIN_TURN,
        SPIN_TURN,
        final_turn("绕过去了"),
    ]
    _, task, model = await run_task(script, step_seconds=0.6)

    assert len(model.calls) == 7
    assert task.status is TaskStatus.delivered
    assert platform.texts[-1].text == "绕过去了"


async def test_two_identical_calls_in_one_step_count_twice(run_task, platform, gateway):
    """一步出两张一样的牌算 2 次 —— 那比隔了一步再重复更卡。"""
    double = tool_turn(SPIN, SPIN)
    _, task, model = await run_task(None, repeat=double, step_seconds=0.6)

    assert len(model.calls) == 3                       # 1,2 / 3,4 / 第 5 张开火
    assert _gateway_names(gateway) == ["run_python"] * 4
    assert task.status is TaskStatus.failed
    assert "原样重复调用" in platform.texts[-1].text


async def test_counter_survives_a_text_only_step(run_task, platform):
    """只回文本的那一步不出牌，也就不打断连发（观测侧 _repeat_loops 也只摊平 tool_calls）。"""
    from worker_fakes import text_turn

    script = [SPIN_TURN, SPIN_TURN, text_turn("我再想想"), SPIN_TURN, SPIN_TURN, SPIN_TURN]
    _, task, model = await run_task(script, step_seconds=0.6)

    assert len(model.calls) == 6
    assert task.status is TaskStatus.failed
    assert "原样重复调用" in platform.texts[-1].text


# ---- 不抢别人的活 ---------------------------------------------------------


async def test_local_tool_spin_is_still_the_step_limit_case(run_task, platform, config):
    """连发 `checklist_note` 归 `max_steps`，不归这条兜底。

    §3.8 `08_step_limit` 的 `verifies` 就是「模型只会重复 checklist_note → max_steps 处
    failed」，回帖里那句「上限」是它的判据之一。本地工具照样进计数（口径要和
    `protocol_probe._repeat_loops()` 对得上），但不由它开火。
    """
    loop_turn = tool_turn(("checklist_note", {"text": "再想想"}))
    _, task, model = await run_task(None, repeat=loop_turn, step_seconds=0.6)

    assert config.worker.max_steps == 40
    assert len(model.calls) == 40
    assert task.status is TaskStatus.failed
    assert "上限" in platform.texts[-1].text
    assert "原样重复调用" not in platform.texts[-1].text


async def test_repeated_invalid_final_is_reported_as_invalid_args(run_task, platform):
    """`final` 也是本地工具：原样重发不合法的 final 由 invalid_args 在第 3 次接住，
    诊断比「原地打转」更具体。"""
    bad_final = tool_turn(("final", {"reply": "   "}))
    _, task, model = await run_task(None, repeat=bad_final, step_seconds=0.6)

    assert len(model.calls) == 3
    assert task.status is TaskStatus.failed
    assert "不合法的工具参数" in platform.texts[-1].text


# ---- 与观测侧的口径 -------------------------------------------------------


@pytest.mark.parametrize(
    "name, args",
    [
        ("run_python", {"code": "print(1)"}),
        ("run_python", {"timeout": 30, "code": "print(1)"}),      # 顶层 key 顺序无关
        ("list_files", {}),
        ("checklist_add", {"items": ["甲", "乙"]}),               # 非 ASCII 不转义
        ("final", {"reply": "好", "artifacts": [{"path": "/work/a.png"}]}),
    ],
)
def test_signature_matches_protocol_probe(name, args):
    """兜底侧和观测侧对「同一张牌」必须是同一个口径。

    不一致的话，报告里说「打转 33 次」而系统说「没打转」，排查时两边会打架。
    这里直接拿 `protocol_probe` 那行公式当判据（那个文件归 T23，只读）。
    """
    from aite.evals.protocol_probe import _stringify

    expected = f"{name}:{_stringify(dict(sorted(args.items())))}"
    assert _call_signature(ToolCallRequest(call_id="c1", name=name, arguments=args)) == expected


def test_signature_survives_unserializable_arguments():
    """真模型给回来的参数是 JSON 解出来的，理论上一定可序列化；万一不是，
    指纹只能退化，不能把整个 worker 掀了。"""
    weird = ToolCallRequest(call_id="c1", name="run_python", arguments={"code": {1, 2}})

    assert _call_signature(weird).startswith("run_python:")
