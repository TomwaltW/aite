"""FakeModel 自测：脚本化出牌、repeat、§3.3 那条纯文本兜底的牌造得出来。"""
from __future__ import annotations

import inspect

import pytest

from aite.contracts import ALL_MODEL_TOOLS, Message, Usage
from aite.contracts.ports import ModelPort
from aite.testing import FakeModel, FakeModelError, ScriptExhausted

MSGS = [Message(role="user", content="你好")]


async def chat(model: FakeModel):
    return await model.chat(MSGS, ALL_MODEL_TOOLS, max_tokens=4096, temperature=0.0)


def test_signature_matches_model_port():
    """§3.2 ModelPort.chat 的签名逐字对齐（参数名、keyword-only、返回标注）。"""
    assert inspect.iscoroutinefunction(FakeModel.chat)
    got = inspect.signature(FakeModel.chat)
    want = inspect.signature(ModelPort.chat)
    assert [p.name for p in got.parameters.values()] == [p.name for p in want.parameters.values()]
    assert [p.kind for p in got.parameters.values()] == [p.kind for p in want.parameters.values()]
    assert FakeModel(name="scripted").name == "scripted"


async def test_tool_call_step_becomes_assistant_message():
    m = FakeModel([{"tool_calls": [{"name": "final", "arguments": {"reply": "好了"}}]}])
    turn = await chat(m)
    assert turn.message.role == "assistant"
    assert turn.finish_reason == "tool_calls"
    assert [tc.name for tc in turn.message.tool_calls] == ["final"]
    assert turn.message.tool_calls[0].arguments == {"reply": "好了"}
    assert turn.message.tool_calls[0].call_id == "call_0_0"


async def test_bare_text_step_has_no_tool_calls():
    """§3.3 的兜底牌：既无 tool_call 也无 final，只有文本。

    怎么兜底归 T2 的 worker（steps==0 视为 final、steps>0 回 system 提示）；
    替身负责的是把这种出牌造出来。"""
    m = FakeModel([{"text": "北京今天晴。"}])
    turn = await chat(m)
    assert turn.message.tool_calls is None
    assert turn.message.content == "北京今天晴。"
    assert turn.finish_reason == "stop"


async def test_multiple_tool_calls_in_one_step():
    m = FakeModel([{"tool_calls": [{"name": "checklist_check", "arguments": {"id": "c1"}},
                                   {"name": "checklist_check", "arguments": {"id": "c2"}}]}])
    turn = await chat(m)
    assert [tc.call_id for tc in turn.message.tool_calls] == ["call_0_0", "call_0_1"]


async def test_steps_are_served_in_order():
    m = FakeModel([{"text": "一"}, {"text": "二"}, {"text": "三"}])
    assert [(await chat(m)).message.content for _ in range(3)] == ["一", "二", "三"]
    assert m.call_count == 3


async def test_repeat_n_then_moves_on():
    m = FakeModel([{"text": "重复", "repeat": 2}, {"text": "下一步"}])
    assert [(await chat(m)).message.content for _ in range(3)] == ["重复", "重复", "下一步"]


async def test_repeat_inf_never_runs_out():
    """08_step_limit 靠它把模型钉死在 checklist_note 上。"""
    m = FakeModel([{"tool_calls": [{"name": "checklist_note", "arguments": {"text": "再想想"}}],
                    "repeat": "inf"}])
    for _ in range(50):
        turn = await chat(m)
        assert turn.message.tool_calls[0].name == "checklist_note"


async def test_exhausted_script_says_so_in_plain_words():
    """脚本用尽要给一句能读的话，别让场景报告里出现异常栈。"""
    m = FakeModel([{"text": "只有一步"}])
    await chat(m)
    with pytest.raises(ScriptExhausted, match="模型脚本已用尽"):
        await chat(m)


async def test_error_step_raises_model_error():
    """§3.3「模型调用异常 / 5xx」这条路要能演。"""
    m = FakeModel([{"error": "上游 503"}])
    with pytest.raises(FakeModelError, match="上游 503"):
        await chat(m)


async def test_usage_is_carried_through():
    m = FakeModel([{"text": "x", "usage": {"input_tokens": 120, "output_tokens": 30}}])
    turn = await chat(m)
    assert turn.usage == Usage(input_tokens=120, output_tokens=30, cached_tokens=0)


async def test_calls_record_what_worker_sent():
    m = FakeModel([{"text": "x"}])
    await m.chat(MSGS, ALL_MODEL_TOOLS, max_tokens=1024, temperature=0.7)
    call = m.calls.last("chat")
    assert call.kwargs["max_tokens"] == 1024
    assert call.kwargs["temperature"] == 0.7
    assert call.kwargs["roles"] == ["user"]
    assert "final" in call.kwargs["tools"]


async def test_tool_names_emitted_counts_local_and_gateway_tools():
    m = FakeModel([
        {"tool_calls": [{"name": "checklist_add", "arguments": {"items": ["a"]}}]},
        {"tool_calls": [{"name": "run_python", "arguments": {"code": "x"}}]},
        {"tool_calls": [{"name": "final", "arguments": {"reply": "done"}}]},
    ])
    for _ in range(3):
        await chat(m)
    assert m.tool_names_emitted() == ["checklist_add", "run_python", "final"]


def test_bad_repeat_is_rejected_at_load_time():
    with pytest.raises(ValueError, match="repeat"):
        FakeModel([{"text": "x", "repeat": 0}])
