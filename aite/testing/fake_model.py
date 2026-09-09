"""FakeModel —— ModelPort 的脚本化替身（dev-spec §3.2）。

一条脚本一步出牌，worker 每调一次 `chat` 消费一步。三种出牌都能造：

* `tool_calls`：一步里可以有多个 tool_call，worker 按顺序处理（§3.6 W2）
* `text`（无 tool_call）：§3.3 那条兜底 —— `steps==0` 时视为 `final(reply=文本)`，
  `steps>0` 时回 system 提示并计 1 步。**这条逻辑归 T2 的 worker**，替身只负责
  把这种牌造出来让 T2/TΩ 验。
* `error`：模型调用直接抛异常，演 §3.3 的「模型调用异常 / 5xx」

`repeat: inf` 让最后一步无限重复，08_step_limit 用它把模型卡在 checklist_note 上。
"""
from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field, field_validator

from ..contracts import Message, ModelTurn, ToolCallRequest, ToolSpec, Usage
from .recorder import CallLog


class FakeModelError(RuntimeError):
    """脚本里 `error:` 那一步抛的错，用来演模型侧异常。"""


class ScriptExhausted(RuntimeError):
    """脚本用完了还被继续调用 —— 场景写漏了，报告里要能一眼看出是这个原因。"""


class ScriptedToolCall(BaseModel):
    name: str
    arguments: dict[str, Any] = Field(default_factory=dict)
    call_id: str | None = None


class ScriptStep(BaseModel):
    """一步出牌。tool_calls 与 text 可以同时有（模型边说话边调工具）。"""

    text: str = ""
    tool_calls: list[ScriptedToolCall] = Field(default_factory=list)
    usage: Usage = Field(default_factory=Usage)
    finish_reason: str | None = None
    #: 这一步重复几次；"inf" = 永远重复（脚本到此为止，后面的步骤不会再被消费）
    repeat: int | Literal["inf"] = 1
    #: 非空则这一步抛 FakeModelError(error)
    error: str | None = None

    @field_validator("repeat")
    @classmethod
    def _check_repeat(cls, v: int | str) -> int | str:
        if v == "inf":
            return v
        if not isinstance(v, int) or v < 1:
            raise ValueError(f"repeat 必须是 >=1 的整数或 'inf'，收到 {v!r}")
        return v


class FakeModel:
    """ModelPort 的替身。`name` 与契约的 `ModelPort.name` 对齐。"""

    def __init__(
        self,
        script: list[ScriptStep | dict[str, Any]] | None = None,
        *,
        name: str = "scripted",
    ) -> None:
        self.name = name
        self.script: list[ScriptStep] = [
            s if isinstance(s, ScriptStep) else ScriptStep.model_validate(s) for s in (script or [])
        ]
        self.calls = CallLog()
        self._cursor = 0          # 指向下一个「还没用尽」的步骤
        self._served_here = 0     # 当前步骤已经出过几次牌
        self.turns_served = 0

    # ---- ModelPort ------------------------------------------------------

    async def chat(
        self,
        messages: list[Message],
        tools: list[ToolSpec],
        *,
        max_tokens: int,
        temperature: float,
    ) -> ModelTurn:
        call = self.calls.record(
            "chat",
            n_messages=len(messages),
            roles=[m.role for m in messages],
            tools=[t.name for t in tools],
            max_tokens=max_tokens,
            temperature=temperature,
        )
        step = self._take()
        if step.error:
            call.error = step.error
            raise FakeModelError(step.error)

        idx = self.turns_served
        tool_calls = [
            ToolCallRequest(
                call_id=tc.call_id or f"call_{idx}_{j}",
                name=tc.name,
                arguments=dict(tc.arguments),
            )
            for j, tc in enumerate(step.tool_calls)
        ]
        finish = step.finish_reason or ("tool_calls" if tool_calls else "stop")
        turn = ModelTurn(
            message=Message(role="assistant", content=step.text, tool_calls=tool_calls or None),
            usage=step.usage.model_copy(),
            finish_reason=finish,
            raw={"scripted_step": self._cursor, "served": self.turns_served},
        )
        self.turns_served += 1
        call.result = [tc.name for tc in tool_calls] or f"text:{step.text[:30]}"
        return turn

    # ---- 内部 -----------------------------------------------------------

    def _take(self) -> ScriptStep:
        if self._cursor >= len(self.script):
            raise ScriptExhausted(
                f"模型脚本已用尽：共 {len(self.script)} 步，这是第 {self.turns_served + 1} 次调用。"
                f"给 model_script 补步骤，或给最后一步写 repeat: inf"
            )
        step = self.script[self._cursor]
        if step.repeat == "inf":
            self._served_here += 1
            return step
        self._served_here += 1
        if self._served_here >= step.repeat:
            self._cursor += 1
            self._served_here = 0
        return step

    # ---- 给断言用 --------------------------------------------------------

    @property
    def call_count(self) -> int:
        return self.calls.count("chat")

    def tool_names_emitted(self) -> list[str]:
        """模型出过的所有 tool_call 名字（含本地 checklist_* 与 final）。"""
        names: list[str] = []
        for c in self.calls.of("chat"):
            if isinstance(c.result, list):
                names.extend(c.result)
        return names

    def __repr__(self) -> str:
        return f"<FakeModel {self.name} {self.turns_served}/{len(self.script)} steps served>"
