"""ModelPort 的 OpenAI-compatible 实现（owner: T4，契约见 aite/contracts/ports.py §3.2）。

指向百炼 / 智谱这类 OpenAI 兼容端点：`base_url`、`model`、`max_tokens`、`temperature`
全部从 `AiteConfig.model`（ModelConfig）读，**密钥只从 `api_key_env` 指定的环境变量取**
（默认 `AITE_MODEL_API_KEY`），代码和配置文件里都不出现取值。

这一层只做翻译，不做策略：

* 契约的 `Message` / `ToolSpec` ↔ OpenAI 的 messages / tools
* OpenAI 的 choices / usage ↔ 契约的 `ModelTurn` / `Usage`
* `cost_of()` 按 `price_in_per_mtok` / `price_out_per_mtok` 折算人民币，只给卡片上的「已用 ¥」用

**不在这里做**的两件事，避免和 T2 抢格子：
* 重试（§3.3「模型调用异常 / 5xx → worker 重试 2 次」是 worker 的事）
* §3.3 那条兜底 —— 模型既无 tool_call 也无 final 只回文本时，`steps==0` 视为
  `final(reply=文本)`、`steps>0` 回 system 提示并计 1 步。本层如实返回
  「content 有值、tool_calls 为空」的 ModelTurn，怎么兜底归 worker。
  想造这种出牌来验，用 `aite.testing.FakeModel` 的纯 text 步骤。
"""
from __future__ import annotations

import json
import os
from typing import Any

from ..contracts import Message, ModelTurn, ToolCallRequest, ToolSpec, Usage
from ..contracts.config import ModelConfig


class ModelConfigError(RuntimeError):
    """配置不全（缺 base_url / model / 密钥环境变量）时抛，消息里绝不带密钥取值。"""


# --------------------------------------------------------------------------
# 契约 -> OpenAI
# --------------------------------------------------------------------------

def to_openai_messages(messages: list[Message]) -> list[dict[str, Any]]:
    """契约 Message 列表翻成 OpenAI 的 messages。"""
    out: list[dict[str, Any]] = []
    for m in messages:
        row: dict[str, Any] = {"role": m.role, "content": m.content}
        if m.role == "assistant" and m.tool_calls:
            row["tool_calls"] = [
                {
                    "id": tc.call_id,
                    "type": "function",
                    "function": {"name": tc.name, "arguments": json.dumps(tc.arguments, ensure_ascii=False)},
                }
                for tc in m.tool_calls
            ]
            # 兼容那些不接受 content=null 的国内网关：带 tool_calls 时空内容写空串而不是 null
            row["content"] = m.content or ""
        if m.role == "tool":
            if not m.tool_call_id:
                raise ValueError(f"role=tool 的消息必须带 tool_call_id：{m!r}")
            row["tool_call_id"] = m.tool_call_id
            if m.name:
                row["name"] = m.name
        out.append(row)
    return out


def to_openai_tools(tools: list[ToolSpec]) -> list[dict[str, Any]]:
    """契约 ToolSpec 列表翻成 OpenAI 的 tools（function calling）。"""
    return [
        {
            "type": "function",
            "function": {"name": t.name, "description": t.description, "parameters": t.parameters},
        }
        for t in tools
    ]


# --------------------------------------------------------------------------
# OpenAI -> 契约
# --------------------------------------------------------------------------

def _as_dict(payload: Any) -> dict[str, Any]:
    """SDK 返回的是 pydantic 对象，先摊成普通 dict，翻译逻辑就能脱离 SDK 单测。"""
    if isinstance(payload, dict):
        return payload
    for attr in ("model_dump", "to_dict", "dict"):
        fn = getattr(payload, attr, None)
        if callable(fn):
            return fn()
    raise TypeError(f"看不懂的响应类型：{type(payload).__name__}")


def usage_from_openai(raw: dict[str, Any] | None) -> Usage:
    if not raw:
        return Usage()
    details = raw.get("prompt_tokens_details") or {}
    return Usage(
        input_tokens=raw.get("prompt_tokens") or 0,
        output_tokens=raw.get("completion_tokens") or 0,
        cached_tokens=(details.get("cached_tokens") if isinstance(details, dict) else 0) or 0,
    )


def turn_from_response(payload: Any) -> ModelTurn:
    """OpenAI 的 chat.completions 响应 → 契约 ModelTurn。

    tool_call 的 arguments 是 JSON 字符串；解析不出来时**不抛异常**，而是把
    arguments 置空并把原文记进 `raw["arg_parse_errors"]` —— 让它走 §3.3
    「tool_call 参数不合 schema → invalid_args，计 1 步」那条路，比整轮炸掉好。
    """
    data = _as_dict(payload)
    choices = data.get("choices") or []
    if not choices:
        raise ValueError("模型响应里没有 choices")
    choice = _as_dict(choices[0])
    msg = _as_dict(choice.get("message") or {})

    tool_calls: list[ToolCallRequest] = []
    arg_errors: list[dict[str, str]] = []
    for i, raw_tc in enumerate(msg.get("tool_calls") or []):
        tc = _as_dict(raw_tc)
        fn = _as_dict(tc.get("function") or {})
        raw_args = fn.get("arguments") or "{}"
        try:
            args = json.loads(raw_args) if isinstance(raw_args, str) else dict(raw_args)
            if not isinstance(args, dict):
                raise TypeError(f"arguments 必须是对象，实际 {type(args).__name__}")
        except (json.JSONDecodeError, TypeError, ValueError) as exc:
            args = {}
            arg_errors.append({"call_id": str(tc.get("id") or i), "error": str(exc), "raw": str(raw_args)})
        tool_calls.append(
            ToolCallRequest(call_id=str(tc.get("id") or f"call_{i}"), name=fn.get("name") or "", arguments=args)
        )

    raw_extra: dict[str, Any] = {"id": data.get("id"), "model": data.get("model")}
    if arg_errors:
        raw_extra["arg_parse_errors"] = arg_errors

    return ModelTurn(
        message=Message(
            role="assistant", content=msg.get("content") or "", tool_calls=tool_calls or None
        ),
        usage=usage_from_openai(data.get("usage")),
        finish_reason=choice.get("finish_reason") or "stop",
        raw=raw_extra,
    )


def cost_of(usage: Usage, cfg: ModelConfig) -> float:
    """按 元/百万 token 折算这一轮的花费。只用于卡片上的「已用 ¥」（§3.1 ModelConfig）。"""
    return (
        usage.input_tokens * cfg.price_in_per_mtok + usage.output_tokens * cfg.price_out_per_mtok
    ) / 1_000_000


def resolve_api_key(cfg: ModelConfig, env: dict[str, str] | None = None) -> str:
    """从 `cfg.api_key_env` 指定的环境变量取密钥；缺了就抛，且消息里只出现变量名。"""
    source = os.environ if env is None else env
    key = (source.get(cfg.api_key_env) or "").strip()
    if not key:
        raise ModelConfigError(
            f"环境变量 {cfg.api_key_env} 没设置或为空。密钥只从环境变量读，"
            f"不要写进 config/aite.yaml（§3.1 ModelConfig）"
        )
    return key


# --------------------------------------------------------------------------
# ModelPort 实现
# --------------------------------------------------------------------------

class OpenAICompatModel:
    """ModelPort（T4，live）。指向任意 OpenAI 兼容端点。

    `client` 可注入：单测里塞一个有 `chat.completions.create` 的替身即可，
    不必真连网、也不必设密钥。
    """

    def __init__(self, cfg: ModelConfig, *, client: Any = None) -> None:
        self.cfg = cfg
        self.name: str = cfg.model
        self._client = client
        #: 累计花费（元），供上层做卡片 footer 用；每次 chat 后累加
        self.total_cost: float = 0.0
        self.last_usage: Usage = Usage()

    # ---- ModelPort ------------------------------------------------------

    async def chat(
        self,
        messages: list[Message],
        tools: list[ToolSpec],
        *,
        max_tokens: int,
        temperature: float,
    ) -> ModelTurn:
        client = self._ensure_client()
        kwargs: dict[str, Any] = {
            "model": self.cfg.model,
            "messages": to_openai_messages(messages),
            "max_tokens": max_tokens,
            "temperature": temperature,
        }
        if tools:
            kwargs["tools"] = to_openai_tools(tools)
            kwargs["tool_choice"] = "auto"
        response = await client.chat.completions.create(**kwargs)
        turn = turn_from_response(response)
        self.last_usage = turn.usage
        self.total_cost += cost_of(turn.usage, self.cfg)
        turn.raw["cost_cny"] = round(self.total_cost, 6)
        return turn

    # ---- 内部 -----------------------------------------------------------

    def _ensure_client(self) -> Any:
        if self._client is not None:
            return self._client
        if not self.cfg.base_url:
            raise ModelConfigError("ModelConfig.base_url 是空的：填百炼 / 智谱的 OpenAI 兼容端点")
        if not self.cfg.model:
            raise ModelConfigError("ModelConfig.model 是空的：填要用的模型名")
        # 延迟 import：没装 openai 也不影响本包被 import（tests/e2e 注入 client 就不碰它）
        from openai import AsyncOpenAI

        self._client = AsyncOpenAI(base_url=self.cfg.base_url, api_key=resolve_api_key(self.cfg))
        return self._client

    def __repr__(self) -> str:
        return f"<OpenAICompatModel {self.name or '(未配置)'} @ {self.cfg.base_url or '(未配置)'}>"
