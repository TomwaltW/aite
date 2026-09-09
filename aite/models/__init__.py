"""模型客户端（owner: T4）。

`OpenAICompatModel` 是 §3.2 `ModelPort` 的 live 实现；脚本化替身在
`aite.testing.FakeModel`。P0 只配一个主力模型，多模型路由 / 快模型 verifier 是 P1（§1）。
"""
from .openai_compat import (
    ModelConfigError,
    OpenAICompatModel,
    cost_of,
    resolve_api_key,
    to_openai_messages,
    to_openai_tools,
    turn_from_response,
    usage_from_openai,
)

__all__ = [
    "ModelConfigError",
    "OpenAICompatModel",
    "cost_of",
    "resolve_api_key",
    "to_openai_messages",
    "to_openai_tools",
    "turn_from_response",
    "usage_from_openai",
]
