"""OpenAICompatModel 自测：契约 ↔ OpenAI 双向翻译、cost、密钥只从环境变量取。

翻译逻辑用注入的假 client 单测；最后几条不注入 client，验的是
「AsyncOpenAI 真被按 base_url / 环境变量里的密钥建起来了」——不然翻译再对，
接线错了也照样打不通。

为什么不用 respx 拦 HTTP：本机 openai>=1.40 解析到的是 3.10，它自带 vendored 的
httpx2（site-packages/httpx2），而 respx 只 patch httpx，拦不到 SDK 的实际流量。
所以这里改成直接查建出来的 client 上的 base_url / api_key。
"""
from __future__ import annotations

import inspect
import json
from types import SimpleNamespace

import pytest

from aite.contracts import ALL_MODEL_TOOLS, Message, ToolCallRequest, Usage
from aite.contracts.config import ModelConfig
from aite.contracts.ports import ModelPort
from aite.models import (
    ModelConfigError,
    OpenAICompatModel,
    cost_of,
    resolve_api_key,
    to_openai_messages,
    to_openai_tools,
    turn_from_response,
    usage_from_openai,
)

CFG = ModelConfig(
    provider="openai_compat",
    base_url="https://dashscope.example.com/compatible-mode/v1",
    api_key_env="AITE_MODEL_API_KEY",
    model="qwen-plus",
    price_in_per_mtok=0.8,
    price_out_per_mtok=2.0,
)


def response(*, content=None, tool_calls=None, finish_reason="stop", usage=None):
    return {
        "id": "chatcmpl-1",
        "model": "qwen-plus",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": content, "tool_calls": tool_calls},
                "finish_reason": finish_reason,
            }
        ],
        "usage": usage,
    }


class StubCompletions:
    def __init__(self, payload):
        self.payload = payload
        self.seen: list[dict] = []

    async def create(self, **kwargs):
        self.seen.append(kwargs)
        return self.payload


def stub_client(payload):
    completions = StubCompletions(payload)
    return SimpleNamespace(chat=SimpleNamespace(completions=completions)), completions


# --- 契约 -> OpenAI ---------------------------------------------------------

def test_signature_matches_model_port():
    got = inspect.signature(OpenAICompatModel.chat)
    want = inspect.signature(ModelPort.chat)
    assert [(p.name, p.kind) for p in got.parameters.values()] == [
        (p.name, p.kind) for p in want.parameters.values()
    ]
    assert inspect.iscoroutinefunction(OpenAICompatModel.chat)


def test_assistant_tool_calls_are_serialised_as_json_arguments():
    msgs = [
        Message(role="system", content="你是 Aite"),
        Message(role="user", content="画个图"),
        Message(
            role="assistant",
            tool_calls=[ToolCallRequest(call_id="c1", name="run_python", arguments={"code": "x=1"})],
        ),
        Message(role="tool", content="exit_code=0", tool_call_id="c1", name="run_python"),
    ]
    out = to_openai_messages(msgs)
    assert [m["role"] for m in out] == ["system", "user", "assistant", "tool"]
    tc = out[2]["tool_calls"][0]
    assert tc == {
        "id": "c1",
        "type": "function",
        "function": {"name": "run_python", "arguments": json.dumps({"code": "x=1"}, ensure_ascii=False)},
    }
    assert out[2]["content"] == ""             # 带 tool_calls 时用空串，不用 null
    assert out[3] == {"role": "tool", "content": "exit_code=0", "tool_call_id": "c1", "name": "run_python"}


def test_chinese_arguments_are_not_escaped():
    msgs = [
        Message(
            role="assistant",
            tool_calls=[ToolCallRequest(call_id="c1", name="final", arguments={"reply": "好了"})],
        )
    ]
    assert "好了" in to_openai_messages(msgs)[0]["tool_calls"][0]["function"]["arguments"]


def test_tool_message_without_call_id_is_rejected():
    with pytest.raises(ValueError, match="tool_call_id"):
        to_openai_messages([Message(role="tool", content="x")])


def test_all_model_tools_translate_to_functions():
    out = to_openai_tools(ALL_MODEL_TOOLS)
    assert len(out) == len(ALL_MODEL_TOOLS) == 10
    assert {t["function"]["name"] for t in out} == {t.name for t in ALL_MODEL_TOOLS}
    assert all(t["type"] == "function" for t in out)
    assert all(t["function"]["parameters"]["type"] == "object" for t in out)


# --- OpenAI -> 契约 ---------------------------------------------------------

def test_tool_call_response_becomes_model_turn():
    payload = response(
        tool_calls=[
            {"id": "call_a", "type": "function",
             "function": {"name": "final", "arguments": '{"reply": "好了"}'}}
        ],
        finish_reason="tool_calls",
    )
    turn = turn_from_response(payload)
    assert turn.finish_reason == "tool_calls"
    assert turn.message.tool_calls == [
        ToolCallRequest(call_id="call_a", name="final", arguments={"reply": "好了"})
    ]


def test_text_only_response_keeps_the_fallback_shape():
    """§3.3 的兜底：本层如实返回 content 有值、tool_calls 为空，兜底归 worker。"""
    turn = turn_from_response(response(content="北京今天晴。"))
    assert turn.message.content == "北京今天晴。"
    assert turn.message.tool_calls is None


def test_broken_tool_arguments_degrade_instead_of_raising():
    """参数不是合法 JSON 时不炸整轮，走 §3.3 的 invalid_args 那条路。"""
    payload = response(
        tool_calls=[{"id": "c1", "type": "function",
                     "function": {"name": "final", "arguments": "{不是 JSON"}}],
        finish_reason="tool_calls",
    )
    turn = turn_from_response(payload)
    assert turn.message.tool_calls[0].arguments == {}
    assert turn.raw["arg_parse_errors"][0]["call_id"] == "c1"


def test_empty_choices_is_an_error():
    with pytest.raises(ValueError, match="没有 choices"):
        turn_from_response({"choices": []})


def test_usage_maps_including_cached_tokens():
    u = usage_from_openai(
        {"prompt_tokens": 1200, "completion_tokens": 300, "prompt_tokens_details": {"cached_tokens": 800}}
    )
    assert u == Usage(input_tokens=1200, output_tokens=300, cached_tokens=800)
    assert usage_from_openai(None) == Usage()


def test_cost_uses_price_per_mtok():
    cost = cost_of(Usage(input_tokens=1_000_000, output_tokens=500_000), CFG)
    assert cost == pytest.approx(0.8 + 1.0)


def test_pydantic_like_response_objects_are_accepted():
    """SDK 返回的是 pydantic 对象而不是 dict —— model_dump() 那条路要走得通。"""

    class Fake:
        def model_dump(self):
            return response(content="来自对象")

    assert turn_from_response(Fake()).message.content == "来自对象"


# --- 密钥与配置 -------------------------------------------------------------

def test_api_key_comes_from_the_configured_env_var(monkeypatch):
    monkeypatch.setenv("AITE_MODEL_API_KEY", "sk-secret")
    assert resolve_api_key(CFG) == "sk-secret"


def test_missing_api_key_names_the_var_not_the_value(monkeypatch):
    monkeypatch.delenv("AITE_MODEL_API_KEY", raising=False)
    with pytest.raises(ModelConfigError) as exc:
        resolve_api_key(CFG)
    assert "AITE_MODEL_API_KEY" in str(exc.value)


def test_custom_api_key_env_is_honoured(monkeypatch):
    cfg = CFG.model_copy(update={"api_key_env": "ZHIPU_KEY"})
    monkeypatch.setenv("ZHIPU_KEY", "sk-zhipu")
    assert resolve_api_key(cfg) == "sk-zhipu"


def test_blank_base_url_is_reported_before_any_network(monkeypatch):
    monkeypatch.setenv("AITE_MODEL_API_KEY", "sk-x")
    model = OpenAICompatModel(ModelConfig(model="qwen-plus"))
    with pytest.raises(ModelConfigError, match="base_url"):
        model._ensure_client()


# --- chat() 全程 ------------------------------------------------------------

async def test_chat_sends_tools_and_accumulates_cost():
    payload = response(
        tool_calls=[{"id": "c1", "type": "function",
                     "function": {"name": "final", "arguments": '{"reply": "好"}'}}],
        finish_reason="tool_calls",
        usage={"prompt_tokens": 1_000_000, "completion_tokens": 0},
    )
    client, completions = stub_client(payload)
    model = OpenAICompatModel(CFG, client=client)

    turn = await model.chat(
        [Message(role="user", content="嗨")], ALL_MODEL_TOOLS, max_tokens=2048, temperature=0.3
    )

    sent = completions.seen[0]
    assert sent["model"] == "qwen-plus"
    assert sent["max_tokens"] == 2048 and sent["temperature"] == 0.3
    assert sent["tool_choice"] == "auto" and len(sent["tools"]) == 10
    assert turn.message.tool_calls[0].name == "final"
    assert model.total_cost == pytest.approx(0.8)
    assert turn.raw["cost_cny"] == pytest.approx(0.8)

    await model.chat([Message(role="user", content="再来")], ALL_MODEL_TOOLS,
                     max_tokens=2048, temperature=0.3)
    assert model.total_cost == pytest.approx(1.6)      # 累加，不是覆盖


async def test_chat_without_tools_omits_the_tools_field():
    client, completions = stub_client(response(content="纯聊天"))
    model = OpenAICompatModel(CFG, client=client)
    await model.chat([Message(role="user", content="嗨")], [], max_tokens=100, temperature=0.0)
    assert "tools" not in completions.seen[0] and "tool_choice" not in completions.seen[0]


def test_name_is_the_configured_model():
    assert OpenAICompatModel(CFG).name == "qwen-plus"


def test_real_client_is_built_from_base_url_and_env_key(monkeypatch):
    """接线是真的：不注入 client 时，AsyncOpenAI 按 base_url 建、密钥取自环境变量。"""
    monkeypatch.setenv("AITE_MODEL_API_KEY", "sk-from-env")
    model = OpenAICompatModel(CFG)
    client = model._ensure_client()

    assert str(client.base_url).rstrip("/") == CFG.base_url
    assert client.api_key == "sk-from-env"
    assert model._ensure_client() is client        # 只建一次，之后复用


def test_real_client_refuses_to_build_without_the_env_key(monkeypatch):
    monkeypatch.delenv("AITE_MODEL_API_KEY", raising=False)
    with pytest.raises(ModelConfigError, match="AITE_MODEL_API_KEY"):
        OpenAICompatModel(CFG)._ensure_client()


def test_repr_never_leaks_the_key(monkeypatch):
    monkeypatch.setenv("AITE_MODEL_API_KEY", "sk-super-secret")
    text = repr(OpenAICompatModel(CFG))
    assert "sk-super-secret" not in text and "qwen-plus" in text
