"""ModelPort 的 OpenAI-compatible 实现骨架（owner: T4，契约见 aite/contracts/ports.py §3.2）。"""
from ..contracts import Message, ModelTurn, ToolSpec


class OpenAICompatModel:
    """ModelPort（T4，live）。"""

    name: str = ""

    async def chat(
        self, messages: list[Message], tools: list[ToolSpec], *, max_tokens: int, temperature: float
    ) -> ModelTurn:
        raise NotImplementedError("T4")
