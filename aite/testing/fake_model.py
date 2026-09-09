"""FakeModel —— ModelPort 的脚本化替身（dev-spec §3.2）。

一条脚本一步出牌，worker 每调一次 `chat` 消费一步。三种出牌都能造：

* `tool_calls`：一步里可以有多个 tool_call，worker 按顺序处理（§3.6 W2）
* `text`（无 tool_call）：§3.3 那条兜底 —— `steps==0` 时视为 `final(reply=文本)`，
  `steps>0` 时回 system 提示并计 1 步。**这条逻辑归 T2 的 worker**，替身只负责
  把这种牌造出来让 T2/TΩ 验。
* `error`：模型调用直接抛异常，演 §3.3 的「模型调用异常 / 5xx」

两个「不往前走」的旋钮是正交的，别混：

* `repeat: inf` 是**无限出牌**：这一步反复出，脚本到此为止（08_step_limit 用它
  把模型钉在 checklist_note 上，好撞到 max_steps）。
* `hold_ticks` 是**这一次不返回**：`chat()` 先把控制权让回事件循环若干次再出牌
  （07_commands 用它让任务在 `!status` / `!stop` 到达时还活着）。
"""
from __future__ import annotations

import asyncio
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
    #: 出牌前先把控制权让回事件循环几次（`await asyncio.sleep(0)` x N）。0 = 不让，立刻出牌。
    #:
    #: 为什么要有它：替身全是瞬时返回的，一次 `chat()` 里没有任何真会挂起的 await 点，
    #: worker 被调度上之后就一口气把脚本跑到头 —— 投递协程根本插不进来。凡是「任务还
    #: 活着的时候才有意义」的场景（`!status` / `!stop`）就此没得可测：07_commands 当初
    #: 正是栽在这里。给某一步加上 hold_ticks，worker 就确定性地停在「下一次 chat」上，
    #: 上一步的副作用（卡片、沙箱）都已经落地，而别的协程终于有机会跑。
    #:
    #: 让出的是事件循环 tick，不是墙钟时间 —— 不 sleep 真实时间，跑多少次结果都一样。
    #: 让满 N 次就照常出牌（别的协程也可以调 `FakeModel.release_holds()` 提前放行），
    #: 所以哪怕等的那件事永远不发生，场景也只会以断言失败收场，不会挂死。
    hold_ticks: int = 0

    @field_validator("hold_ticks")
    @classmethod
    def _check_hold_ticks(cls, v: int) -> int:
        if v < 0:
            raise ValueError(f"hold_ticks 必须是 >=0 的整数，收到 {v!r}")
        return v

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
        self.holds = 0                # 进过几次 hold（见 ScriptStep.hold_ticks）
        self.hold_ticks_yielded = 0   # 这些 hold 一共让出了多少个事件循环 tick
        self.holds_released = False   # release_holds() 置位后，hold 一律立刻放行

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
        if step.hold_ticks:
            await self._hold(step.hold_ticks)
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

    # ---- 给场景驱动用 -----------------------------------------------------

    def release_holds(self) -> None:
        """让正在 hold 的那一步立刻出牌，之后的 hold 步也不再挂。

        07_commands 用不着它（那里靠 hold_ticks 的上限自己放行）。这条路留给
        「要等的事已经发生了，别再空转」的调用方：置位后当前 hold 下一个 tick 就返回。
        """
        self.holds_released = True

    # ---- 内部 -----------------------------------------------------------

    async def _hold(self, ticks: int) -> None:
        """把控制权让回事件循环最多 `ticks` 次。语义见 ScriptStep.hold_ticks。"""
        self.holds += 1
        for _ in range(ticks):
            if self.holds_released:
                return
            await asyncio.sleep(0)
            self.hold_ticks_yielded += 1

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
