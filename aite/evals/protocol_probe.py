"""真模型出牌观测（owner: T12，dev-spec §14.2 模型实测 / §3.3 兜底）。

`--model live` 落地了但一次都没被真跑过。跑起来之后 `evals/p0/*.yaml` 里那些照
`model_script` 台词写的 `expect` 必然大面积红 —— 那是预期的，不是 bug（§3.8 的
判据钉的是 scripted 路径的 P0 验收）。真正要看的是另一件事：**真模型能不能按
`aite/contracts/protocol.py` 那套协议出牌，接不住的地方 §3.3 的兜底够不够**。
这个文件就是看这件事的眼睛。

装置分两半：

* `ModelProbe`：`ModelPort` 形状的包装器，包住任意模型（`FakeModel` 或
  `OpenAICompatModel`），一次 `chat` 记一条 `Observation` —— 出了什么牌、参数合不合
  `ToolSpec.parameters`、抛没抛异常、离上一次隔了多久。
* `analyze()`：把观测跟 evidence 链、出站消息、任务终态对起来，回答五个问题：
  每步调了什么 / 参数合不合 schema / 调没调协议外的名字 / 几步收敛 / 兜底触没触发。

**它只看，不改行为。** §3.3 的兜底全在 `aite/worker/loop.py`（T2 的格子），这里一行
都不碰 —— 观测装置改了被观测的东西，测出来的就不是真模型的出牌了。

顺带修掉一个 bug：`Deps.activity()` 读 `model.calls`、`Deps.stats()` 读
`model.call_count`，这两样 `OpenAICompatModel` 都没有，所以 `--model live` 以前连
第一次网络调用都到不了就 `AttributeError` 崩掉。探针补上了这两个口子。

出牌怎么切分成「轮」：worker 一个 task 一份 `messages`，全程只 append，且**每一步的
第一件事就是把模型刚才那张回牌 append 进去**（见 `AgentWorker._loop`）。所以续接的
判据有两条，缺一不可：

1. 这次调用的前 N 条 == 上次调用的全部；
2. 第 N 条（新长出来的头一条）就是上次那张 assistant 回牌。

只比前缀不够 —— 同一会话里的第二个 task，上下文是第一个 task 的**前缀扩展**
（多一条 user turn），只看第 1 条会把两个 task 并成一轮，02_thread_followup 就是。

重试（§3.3 模型 5xx）原样再投同一份 messages：一条都没长，且上一发是抛出去的 ——
单独认这一种，落在同一轮里。
"""
from __future__ import annotations

import hashlib
import json
import time
from collections import Counter
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from ..contracts import ALL_MODEL_TOOLS, Message, ModelTurn, ToolSpec
from ..testing.jsonschema_mini import SchemaError, validate
from ..testing.recorder import CallLog

if TYPE_CHECKING:                                     # 避免 wiring <-> protocol_probe 循环 import
    from .wiring import Deps

#: 摘要里参数值截到多长 —— run_python 的 code 动辄几百字，全打出来没法读
MAX_ARG_CHARS = 200
#: 纯文本步骤在摘要里截到多长
MAX_TEXT_CHARS = 160
#: 同一张牌连着出几次才算「卡住了」。2 次多半还是正常探测（真模型实测里
#: 连着两次 `list_files()` 是常态），3 次起才是原地打转。
MIN_REPEAT_RUN = 3

PROTOCOL_TOOL_NAMES: frozenset[str] = frozenset(t.name for t in ALL_MODEL_TOOLS)


def _one_line(exc: BaseException) -> str:
    """异常 → 一行能读的原因。只取首行，绝不带栈。"""
    head = str(exc).strip().splitlines()
    return f"{type(exc).__name__}: {head[0]}" if head else type(exc).__name__


def _clip(text: str, limit: int) -> str:
    text = text.replace("\n", "⏎")
    return text if len(text) <= limit else text[: limit - 1] + "…"


def _row(m: Message) -> dict[str, Any]:
    """一条 message 的摘要，进报告用。原文可能很长，这里只留看得出形状的部分。"""
    row: dict[str, Any] = {"role": m.role, "content": _clip(m.content or "", MAX_TEXT_CHARS)}
    if m.tool_calls:
        row["tool_calls"] = [tc.name for tc in m.tool_calls]
    if m.name:
        row["name"] = m.name
    return row


def _prefix_key(messages: list[Message], n: int) -> str:
    """前 n 条 message 的指纹。用来判「这次调用是不是上次那一轮的续接」。"""
    h = hashlib.sha256()
    for m in messages[:n]:
        h.update(m.model_dump_json().encode("utf-8"))
    return h.hexdigest()


# --------------------------------------------------------------------------
# 观测记录
# --------------------------------------------------------------------------

@dataclass
class ToolCallObservation:
    """模型出的一张牌，外加「这张牌合不合法」的判定。"""

    call_id: str
    name: str
    arguments: dict[str, Any]
    #: 名字在不在 ALL_MODEL_TOOLS 里
    in_protocol: bool
    #: 按 ToolSpec.parameters 校验的结果；协议外的工具没有 spec，记 None
    schema_ok: bool | None
    schema_error: str | None = None

    def to_json(self) -> dict[str, Any]:
        row: dict[str, Any] = {
            "name": self.name,
            "call_id": self.call_id,
            "arguments": {k: _clip(_stringify(v), MAX_ARG_CHARS) for k, v in self.arguments.items()},
            "in_protocol": self.in_protocol,
        }
        if self.schema_ok is not None:
            row["schema_ok"] = self.schema_ok
        if self.schema_error:
            row["schema_error"] = self.schema_error
        return row


def _stringify(value: Any) -> str:
    if isinstance(value, str):
        return value
    try:
        return json.dumps(value, ensure_ascii=False)
    except (TypeError, ValueError):
        return repr(value)


@dataclass
class Observation:
    """一次 `ModelPort.chat` 的全部可见事实。失败的那次也记 —— 重试要靠它数。"""

    index: int                      # 第几次 chat（全局，含失败重试）
    run: int                        # 第几轮对话；worker 一个 task 一轮
    attempt: int                    # 本轮里的第几次调用（含失败重试）
    n_messages: int
    delta: list[dict[str, Any]] = field(default_factory=list)   # 相对上一次新增的 message
    elapsed_ms: int = 0
    gap_ms: int = 0                 # 距上一次 chat 返回过了多久（看 §3.3 的 2s / 5s 重试间隔）
    error: str | None = None
    text: str = ""
    finish_reason: str = ""
    tool_calls: list[ToolCallObservation] = field(default_factory=list)
    usage: dict[str, int] = field(default_factory=dict)

    @property
    def ok(self) -> bool:
        return self.error is None

    @property
    def text_only(self) -> bool:
        """§3.3 那条「既无 tool_call 也无 final，只返回文本」的出牌。"""
        return self.ok and not self.tool_calls

    def to_json(self) -> dict[str, Any]:
        row: dict[str, Any] = {
            "index": self.index,
            "run": self.run,
            "attempt": self.attempt,
            "n_messages": self.n_messages,
            "elapsed_ms": self.elapsed_ms,
            "gap_ms": self.gap_ms,
        }
        if self.error:
            row["error"] = self.error
            return row
        row["finish_reason"] = self.finish_reason
        if self.text:
            row["text"] = _clip(self.text, MAX_TEXT_CHARS)
        row["tool_calls"] = [tc.to_json() for tc in self.tool_calls]
        if self.usage:
            row["usage"] = self.usage
        if self.delta:
            row["delta"] = self.delta
        return row


# --------------------------------------------------------------------------
# 探针
# --------------------------------------------------------------------------

class ModelProbe:
    """`ModelPort` 的透明包装：照原样转发 `chat`，顺手把每一步出牌记下来。

    未知属性一律透传给被包的模型（`FakeModel.release_holds()` / `.script` /
    `OpenAICompatModel.total_cost` 这些照常能用），所以套上它不改任何调用方。
    """

    def __init__(self, inner: Any, *, tools: list[ToolSpec] | None = None) -> None:
        self.inner = inner
        self.name: str = getattr(inner, "name", type(inner).__name__)
        self.calls = CallLog()
        self.observations: list[Observation] = []
        self._specs: dict[str, ToolSpec] = {t.name: t for t in (tools or ALL_MODEL_TOOLS)}
        self._prev_len = 0
        self._prev_key = ""
        self._prev_reply: str | None = None      # 上一次返回的那条 assistant 的指纹
        self._run = -1
        self._attempt = 0
        self._last_return: float | None = None
        self._in_flight = 0
        self._last_failed = False

    # ---- ModelPort ------------------------------------------------------

    async def chat(
        self,
        messages: list[Message],
        tools: list[ToolSpec],
        *,
        max_tokens: int,
        temperature: float,
    ) -> ModelTurn:
        obs = self._open(messages)
        call = self.calls.record(
            "chat",
            n_messages=len(messages),
            roles=[m.role for m in messages],
            tools=[t.name for t in tools],
            max_tokens=max_tokens,
            temperature=temperature,
        )
        # 记在调用前：模型抛异常时这条观测也得留下，不然重试次数就数不出来了
        self.observations.append(obs)
        self._remember(messages)

        started = time.monotonic()
        self._in_flight += 1
        try:
            turn = await self.inner.chat(
                messages, tools, max_tokens=max_tokens, temperature=temperature
            )
        except Exception as exc:
            obs.elapsed_ms = int((time.monotonic() - started) * 1000)
            obs.error = _one_line(exc)
            call.error = obs.error
            self._prev_reply = None            # 抛了就没有回牌被 append，重试认「一条没长」
            self._last_failed = True
            self._last_return = time.monotonic()
            raise
        finally:
            self._in_flight -= 1

        obs.elapsed_ms = int((time.monotonic() - started) * 1000)
        self._last_failed = False
        self._prev_reply = turn.message.model_dump_json()
        self._absorb(obs, turn)
        call.result = [tc.name for tc in obs.tool_calls] or f"text:{_clip(obs.text, 30)}"
        self._last_return = time.monotonic()
        return turn

    # ---- 内部 -----------------------------------------------------------

    def _is_continuation(self, messages: list[Message]) -> bool:
        """这次调用是不是上一轮的续接。判据见模块 docstring。"""
        if self._prev_len == 0 or len(messages) < self._prev_len:
            return False
        if _prefix_key(messages, self._prev_len) != self._prev_key:
            return False
        if len(messages) == self._prev_len:
            # 一条都没长 —— 只可能是 §3.3 的重试：上一发抛了，messages 原样再投一次
            return bool(self.observations) and self.observations[-1].error is not None
        return self._prev_reply is not None and (
            messages[self._prev_len].model_dump_json() == self._prev_reply
        )

    def _open(self, messages: list[Message]) -> Observation:
        """开一条观测，顺便判定这次调用属于哪一轮。"""
        if self._is_continuation(messages):
            self._attempt += 1
            delta_from = self._prev_len
        else:
            self._run += 1
            self._attempt = 0
            delta_from = 0
        now = time.monotonic()
        return Observation(
            index=len(self.observations),
            run=self._run,
            attempt=self._attempt,
            n_messages=len(messages),
            delta=[_row(m) for m in messages[delta_from:]],
            gap_ms=0 if self._last_return is None else int((now - self._last_return) * 1000),
        )

    def _remember(self, messages: list[Message]) -> None:
        self._prev_len = len(messages)
        self._prev_key = _prefix_key(messages, self._prev_len)

    def _absorb(self, obs: Observation, turn: ModelTurn) -> None:
        obs.text = turn.message.content or ""
        obs.finish_reason = turn.finish_reason
        obs.usage = turn.usage.model_dump()
        for tc in turn.message.tool_calls or []:
            spec = self._specs.get(tc.name)
            schema_ok: bool | None = None
            schema_error: str | None = None
            if spec is not None:
                try:
                    validate(tc.arguments, spec.parameters)
                    schema_ok = True
                except SchemaError as exc:
                    schema_ok, schema_error = False, str(exc)
            obs.tool_calls.append(
                ToolCallObservation(
                    call_id=tc.call_id,
                    name=tc.name,
                    arguments=dict(tc.arguments),
                    in_protocol=spec is not None,
                    schema_ok=schema_ok,
                    schema_error=schema_error,
                )
            )

    # ---- 给断言 / Deps 用 -------------------------------------------------

    @property
    def in_flight(self) -> int:
        """有几次 chat 还没返回。

        `Deps.activity()` 是个「变没变」的探测器，看不见长时间的 await —— 真模型一次
        调用动辄几秒，期间一个替身都不会被碰。`wiring.settle()` 靠这个数才不会把
        「正在等模型回包」当成「系统不干活了」。
        """
        return self._in_flight

    @property
    def awaiting_retry(self) -> bool:
        """上一发是抛出去的：worker 可能正睡在 §3.3 的 2s / 5s 退避里，等着再来一发。"""
        return self._last_failed

    @property
    def call_count(self) -> int:
        return self.calls.count("chat")

    def tool_names_emitted(self) -> list[str]:
        """模型出过的所有 tool_call 名字（含本地 checklist_* 与 final）。"""
        return [tc.name for o in self.observations for tc in o.tool_calls]

    def __getattr__(self, item: str) -> Any:
        # dataclass/属性都找不到时才走到这里；透传给被包的模型
        return getattr(self.inner, item)

    def __repr__(self) -> str:
        return f"<ModelProbe {self.name} {len(self.observations)} chats>"


# --------------------------------------------------------------------------
# 分析：把观测跟系统的反应对起来
# --------------------------------------------------------------------------

@dataclass
class _Run:
    """一轮对话（≈ 一个 task）的观测集合。"""

    run: int
    obs: list[Observation]

    @property
    def steps(self) -> list[Observation]:
        """真出了牌的那几次（失败重试不算步）。"""
        return [o for o in self.obs if o.ok]


def _group(observations: list[Observation]) -> list[_Run]:
    runs: dict[int, _Run] = {}
    for o in observations:
        runs.setdefault(o.run, _Run(run=o.run, obs=[])).obs.append(o)
    return [runs[k] for k in sorted(runs)]


def _final_index(run: _Run) -> int | None:
    """第几步调了 final（0 起）。没调过返回 None。"""
    for i, o in enumerate(run.steps):
        if any(tc.name == "final" for tc in o.tool_calls):
            return i
    return None


def _retry_bursts(run: _Run) -> list[dict[str, Any]]:
    """§3.3「模型调用异常 / 5xx → 重试 2 次（2s / 5s）」的实际发生情况。

    一串连着抛的调用算一次 burst。**退避间隔记在「重试那一发」头上**：`gap_ms` 是
    「离上一次调用返回过了多久」，所以第 k 次失败之后的退避，体现在第 k+1 次调用的
    gap 上 —— 2000 / 5000 附近才说明 `MODEL_RETRY_DELAYS` 真按 §3.3 走了。

    burst 后面还跟着一次成功调用 = worker 重试成功；后面什么都没有 = 重试用尽，
    worker 已经把任务判 failed（这一轮不会再有调用）。
    """
    bursts: list[dict[str, Any]] = []
    obs, i = run.obs, 0
    while i < len(obs):
        if obs[i].ok:
            i += 1
            continue
        j = i
        while j < len(obs) and not obs[j].ok:
            j += 1
        # obs[i:j] 全是抛出去的；obs[i+1:j+1] 是「每次失败之后 worker 又发的那一下」
        followers = obs[i + 1 : j + 1]
        bursts.append(
            {
                "first_index": obs[i].index,
                "failures": j - i,
                "retried": len(followers),
                "delays_ms": [o.gap_ms for o in followers],
                "errors": [o.error for o in obs[i:j]],
                "recovered": j < len(obs),
            }
        )
        i = j
    return bursts


def _fallback_text_only(run: _Run) -> tuple[list[dict], list[dict]]:
    """区分 §3.3 纯文本兜底的两条分支。

    * `steps==0` 且文本非空 → 视为 `final(reply=文本)`，这一轮到此为止。
    * 其余（`steps>0`，或第一步就回了空文本）→ 回 system 提示并计 1 步。

    判据照抄 `AgentWorker._loop` 的那一行（`step_index == 0 and text`），因为
    `run.steps` 已经滤掉了抛出去的重试，`enumerate` 的下标就是 worker 眼里的
    `step_index`。**不能只看「下一步的 delta 里有没有 system」** —— 兜底之后
    worker 立刻撞上 `max_steps`（或被取消）时压根没有下一次调用，那条 system
    提示虽然进了 messages 却再也发不出去，光看 delta 会把 nudge 误报成 final。
    T17 拿真模型跑 08_step_limit（`max_steps: 3`）就撞了这一下：第 3 步纯文本
    走的是 nudge 分支、任务以「已达步数上限」failed，报告却说它兜成了 final。

    system 提示的原文仍从下一步的 delta 里取，取不到就记 None —— 那正是
    「提示生成了但没能再投给模型」这一种。
    """
    as_final: list[dict[str, Any]] = []
    nudged: list[dict[str, Any]] = []
    steps = run.steps
    for i, o in enumerate(steps):
        if not o.text_only:
            continue
        nxt = steps[i + 1] if i + 1 < len(steps) else None
        system_rows = [] if nxt is None else [r for r in nxt.delta if r["role"] == "system"]
        row = {"run": run.run, "step": i, "text": _clip(o.text, MAX_TEXT_CHARS)}
        if i == 0 and o.text.strip():
            as_final.append(row)
        else:
            nudged.append({**row, "nudge": system_rows[-1]["content"] if system_rows else None})
    return as_final, nudged


def _repeat_loops(run: _Run) -> list[dict[str, Any]]:
    """同一张牌连着出好几次 —— 真模型卡住时的典型姿态。

    T17 实测 04_csv_to_chart：沙箱对每条不含 `savefig` 的代码都回
    `exit_code=0` + 空 stdout，模型先合理地诊断了几步，然后对**逐字节相同**的
    `run_python` 连发 31 次，一路烧到 `max_steps` 才停。

    §3.3 的两条计数兜底都接不住这种：`invalid_args` 要参数不合 schema，
    `sandbox` 要工具报错，而这里工具**返回的是 `ok=True`**，只是内容为空。
    唯一接住它的是 `max_steps`，代价是烧满整整 max_steps 次模型调用。所以单独
    标出来 —— 不然几十行一模一样的 `run_python(code)` 只能靠肉眼数。

    签名把一轮里的 tool_call 按顺序摊平算（一步出多张牌时逐张算），只认**连续**
    相同：中间插进别的调用说明模型还在换招，不算卡住。
    """
    flat: list[tuple[int, ToolCallObservation, str]] = []
    for i, o in enumerate(run.steps):
        for tc in o.tool_calls:
            flat.append((i, tc, f"{tc.name}:{_stringify(dict(sorted(tc.arguments.items())))}"))

    loops: list[dict[str, Any]] = []
    i = 0
    while i < len(flat):
        j = i + 1
        while j < len(flat) and flat[j][2] == flat[i][2]:
            j += 1
        if j - i >= MIN_REPEAT_RUN:
            step, tc, _ = flat[i]
            loops.append(
                {
                    "run": run.run,
                    "name": tc.name,
                    "count": j - i,
                    "first_step": step,
                    "last_step": flat[j - 1][0],
                    "arguments": {
                        k: _clip(_stringify(v), MAX_ARG_CHARS) for k, v in tc.arguments.items()
                    },
                }
            )
        i = j
    return loops


def _tool_results(deps: Deps) -> list[dict[str, Any]]:
    """从 evidence 链里把每次工具执行的判定捞出来（含 error code）。

    为什么不自己数：worker 对本地工具做的是就地校验（`checklist_add` 的 items、
    `final.reply` 之类），Gateway 做的才是 `ToolSpec.parameters` 全校验。系统究竟
    把哪一次判成了 `invalid_args`，只有 evidence 里的 `tool_result` 说了算。
    """
    rows: list[dict[str, Any]] = []
    for task_id, chain in deps.evidence.chains.items():
        for ev in chain:
            if str(ev.kind) != "tool_result" or ev.payload is None:
                continue
            rows.append(
                {
                    "task_id": task_id,
                    "seq": ev.seq,
                    "name": ev.payload.get("name"),
                    "ok": bool(ev.payload.get("ok")),
                    "error": ev.payload.get("error"),
                }
            )
    return rows


def _invalid_args(rows: list[dict[str, Any]]) -> dict[str, Any]:
    """§3.3「参数不合 schema → 回 invalid_args，连续 3 次 failed」的实际计数。"""
    worst = 0
    streak = 0
    by_task: dict[str, int] = {}
    for r in rows:
        if r["error"] == "invalid_args":
            streak += 1
            by_task[r["task_id"]] = by_task.get(r["task_id"], 0) + 1
        elif r["ok"]:
            streak = 0
        worst = max(worst, streak)
    return {
        "count": sum(by_task.values()),
        "max_consecutive": worst,
        "by_tool": dict(Counter(r["name"] for r in rows if r["error"] == "invalid_args")),
    }


def _tasks(deps: Deps) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for t in deps.store.task_list:
        out.append(
            {
                "task_no": t.task_no,
                "status": str(t.status),
                "steps": t.steps,
                "max_steps": t.max_steps,
                "hit_max_steps": t.steps >= t.max_steps,
                "summary": _clip(t.result_summary or "", MAX_TEXT_CHARS),
            }
        )
    return out


def analyze(deps: Deps) -> dict[str, Any]:
    """把一个场景跑完之后的观测汇成一份协议报告。没装探针就返回空 dict。"""
    probe = deps.model
    if not isinstance(probe, ModelProbe):
        return {}

    runs = _group(probe.observations)
    tool_names: Counter[str] = Counter()
    unknown: Counter[str] = Counter()
    violations: list[dict[str, Any]] = []
    as_final: list[dict[str, Any]] = []
    nudged: list[dict[str, Any]] = []
    retries: list[dict[str, Any]] = []
    loops: list[dict[str, Any]] = []
    run_rows: list[dict[str, Any]] = []

    for run in runs:
        steps = run.steps
        final_at = _final_index(run)
        for i, o in enumerate(steps):
            for tc in o.tool_calls:
                tool_names[tc.name] += 1
                if not tc.in_protocol:
                    unknown[tc.name] += 1
                elif tc.schema_ok is False:
                    violations.append(
                        {
                            "run": run.run,
                            "step": i,
                            "name": tc.name,
                            "error": tc.schema_error,
                            "arguments": {
                                k: _clip(_stringify(v), MAX_ARG_CHARS) for k, v in tc.arguments.items()
                            },
                        }
                    )
        run_af, run_nudge = _fallback_text_only(run)
        as_final.extend(run_af)
        nudged.extend(run_nudge)
        loops.extend(_repeat_loops(run))
        for burst in _retry_bursts(run):
            retries.append({"run": run.run, **burst})
        run_rows.append(
            {
                "run": run.run,
                "steps": len(steps),
                "attempts": len(run.obs),
                "reached_final": final_at is not None,
                "steps_to_final": None if final_at is None else final_at + 1,
                "tools": [tc.name for o in steps for tc in o.tool_calls],
                "steps_detail": [o.to_json() for o in run.obs],
            }
        )

    results = _tool_results(deps)
    tasks = _tasks(deps)
    return {
        "model": probe.name,
        "chat_attempts": len(probe.observations),
        "chat_ok": sum(1 for o in probe.observations if o.ok),
        "runs": run_rows,
        "reached_final": sum(1 for r in run_rows if r["reached_final"]),
        "tool_names": dict(tool_names),
        "unknown_tools": dict(unknown),
        "schema_violations": violations,
        # 故意不放进 fallbacks：§3.3 里没有哪一条接得住原地打转，它是「缺兜底」
        # 的证据，不是「兜底触发了」的记录。
        "repeat_loops": loops,
        "fallbacks": {
            "text_only_as_final": as_final,
            "text_only_nudge": nudged,
            "invalid_args": _invalid_args(results),
            "model_retry": retries,
            "not_found": dict(Counter(r["name"] for r in results if r["error"] == "not_found")),
            "sandbox_errors": sum(1 for r in results if r["error"] == "sandbox"),
        },
        "tasks": tasks,
        "hit_max_steps": [t["task_no"] for t in tasks if t["hit_max_steps"]],
        "outbound_texts": [_clip(t, MAX_TEXT_CHARS) for t in deps.platform.texts()],
    }


# --------------------------------------------------------------------------
# 人话摘要
# --------------------------------------------------------------------------

def _verdict(report: dict[str, Any]) -> str:
    runs = report.get("runs") or []
    if not runs:
        return "模型一次都没被调用"
    reached = report["reached_final"]
    steps = [r["steps_to_final"] for r in runs if r["steps_to_final"]]
    tail = f"，到 final 用了 {steps} 步" if steps else ""
    return f"{len(runs)} 轮 / final {reached} 轮{tail}"


def render_digest(rows: list[tuple[str, dict[str, Any]]]) -> str:
    """把若干场景的报告渲成一段能读的文字。写 stderr，不污染 stdout 的 JSON 摘要。"""
    out: list[str] = ["=== 协议出牌报告（§14.2 模型实测）==="]
    for name, report in rows:
        if not report:
            out.append(f"\n--- {name} ---\n  （没装探针）")
            continue
        fb = report["fallbacks"]
        out.append(f"\n--- {name} · {report['model']} · {_verdict(report)} ---")
        for run in report["runs"]:
            head = (
                f"  轮 {run['run']}：{run['steps']} 步 / {run['attempts']} 次调用 · "
                + (f"final@{run['steps_to_final']}" if run["reached_final"] else "未 final")
            )
            out.append(head)
            for o in run["steps_detail"]:
                out.append("    " + _step_line(o))
        if report["unknown_tools"]:
            out.append(f"  协议外工具名：{report['unknown_tools']}")
        for v in report["schema_violations"]:
            out.append(f"  参数不合 schema：轮{v['run']} 步{v['step']} {v['name']} -> {v['error']}")
        for hit in fb["text_only_as_final"]:
            out.append(f"  §3.3 兜底[纯文本→final]：轮{hit['run']} 步{hit['step']} {hit['text']!r}")
        for hit in fb["text_only_nudge"]:
            tail = "（提示没能再投出去：兜底之后就没有下一步了）" if hit["nudge"] is None else repr(hit["nudge"])
            out.append(f"  §3.3 兜底[纯文本→system 提示]：轮{hit['run']} 步{hit['step']} {tail}")
        for loop in report.get("repeat_loops") or []:
            out.append(
                f"  ⚠ 原地打转：轮{loop['run']} 步{loop['first_step']}–{loop['last_step']} "
                f"连着 {loop['count']} 次一模一样的 {loop['name']}({', '.join(loop['arguments'])})"
                f" —— §3.3 没有哪条兜底接得住，只有 max_steps"
            )
        ia = fb["invalid_args"]
        if ia["count"]:
            out.append(
                f"  §3.3 兜底[invalid_args]：{ia['count']} 次，最长连续 {ia['max_consecutive']} 次"
                f"（上限 3 触发 failed）{ia['by_tool']}"
            )
        for burst in fb["model_retry"]:
            out.append(
                f"  §3.3 兜底[模型异常重试]：轮{burst['run']} 抛了 {burst['failures']} 次 / "
                f"又重试 {burst['retried']} 次，退避 {burst['delays_ms']}ms，"
                + ("最终成功" if burst["recovered"] else "重试用尽")
                + f"；错误：{burst['errors']}"
            )
        if fb["not_found"]:
            out.append(f"  工具名查无此人（not_found）：{fb['not_found']}")
        if fb["sandbox_errors"]:
            out.append(f"  沙箱错误：{fb['sandbox_errors']} 次（连续 2 次 failed）")
        if report["hit_max_steps"]:
            # 带上终态：`hit_max_steps` 只是「步数用满了」，跟「因为撞上限而失败」不是
            # 一回事 —— T17 实测 08_step_limit 就出过 steps==max_steps 却正常 delivered
            # 的一轮（最后一步刚好调了 final），光看任务号会误读成跑飞了。
            hits = {t["task_no"]: t["status"] for t in report["tasks"] if t["hit_max_steps"]}
            out.append(f"  步数用满 max_steps 的任务：{hits}")
        out.append(f"  任务终态：{[(t['task_no'], t['status']) for t in report['tasks']] or '(没建任务)'}")
    return "\n".join(out)


def _step_line(o: dict[str, Any]) -> str:
    if "error" in o:
        return f"#{o['index']} ✗ {o['error']}（gap {o['gap_ms']}ms）"
    if not o["tool_calls"]:
        return f"#{o['index']} 纯文本 {o.get('text', '')!r}（finish={o['finish_reason']}）"
    parts = []
    for tc in o["tool_calls"]:
        mark = "" if tc.get("schema_ok", True) else " ✗schema"
        mark += "" if tc["in_protocol"] else " ✗协议外"
        parts.append(f"{tc['name']}({', '.join(tc['arguments'])}){mark}")
    return f"#{o['index']} " + " ".join(parts)
