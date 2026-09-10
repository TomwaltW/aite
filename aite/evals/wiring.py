"""把场景接到「被测系统」上，并在别的轨还没合进来时优雅降级。

T4 与 T1/T2/T3 并行，`aite/control/` 这些目录对 T4 不可见（§3.4）。所以这里不写死
任何一个类名，而是**运行时发现**：找 `aite.control` 下结构上满足 §3.2 `ControlPlane`
的东西，按参数名把替身喂进去。找不到 / 构造不出来 / 还是空实现 —— 都变成一句能读的
失败原因，绝不让异常栈冒到 runner 外面（§6 T4：「每个场景必须报出失败原因而不是异常栈」）。

给 TΩ 的接线提示（按优先级）：
1. `aite.control` 暴露 `build_control_plane(**deps)` 工厂 —— 最省事，参数名随便起，
   这里按名字喂 config / platform / model / sandbox / gateway / store / evidence。
2. 否则找 `aite.control.plane` 里带 `handle_event` + `run_forever` 的类，按同一套
   参数名调构造函数。
3. 跑起来之后，优先调 plane 的 `drain()` / `run_until_idle()`（如果有）等队列排空；
   没有就让 `run_forever()` 后台跑，轮询到「所有替身都不再被调用」为止再取消。
   §3.2 没有「跑到空闲为止」这个接口，第 3 条是本 runner 自己的兜底。
"""
from __future__ import annotations

import asyncio
import importlib
import inspect
from dataclasses import dataclass
from typing import Any

from ..contracts import AiteConfig, Task, TaskStatus
from ..testing import (
    FakeEvidenceWriter,
    FakeModel,
    FakePlatform,
    FakeSandbox,
    FakeSessionStore,
    FakeToolGateway,
)
from ..testing.fake_store import ACTIVE_STATUSES
from .protocol_probe import ModelProbe
from .real_stack import build_docker_stack
from .scenario import EventAfter, Scenario

#: `--sandbox` 认哪几档。默认 `fake` —— 默认路径逐字节不变是硬约束。
SANDBOXES = ("fake", "docker")

#: 去哪儿找 ControlPlane
CANDIDATE_MODULES = ("aite.control", "aite.control.plane")
#: 工厂函数名，按顺序试
CANDIDATE_FACTORIES = ("build_control_plane", "make_control_plane", "create_control_plane")
#: 队列排空的接口名，按顺序试
DRAIN_METHODS = ("drain", "run_until_idle", "process_pending", "run_once")

#: 依赖名 -> 构造函数里可能用的参数名
PARAM_ALIASES: dict[str, tuple[str, ...]] = {
    "config": ("config", "cfg", "settings", "aite_config"),
    "platform": ("platform", "platform_port", "adapter"),
    "model": ("model", "model_port", "llm"),
    "sandbox": ("sandbox", "sandbox_port"),
    "gateway": ("gateway", "tool_gateway", "tools"),
    "store": ("store", "session_store", "sessions"),
    "evidence": ("evidence", "evidence_writer", "evidence_port"),
}


class PhaseError(RuntimeError):
    """带阶段标记的失败。runner 把 phase 一起写进报告，方便看是哪一环断的。"""

    def __init__(self, phase: str, message: str) -> None:
        super().__init__(message)
        self.phase = phase


def brief(exc: BaseException) -> str:
    """异常 → 一行能读的原因。只取消息首行，绝不带栈。"""
    text = str(exc).strip().splitlines()
    head = text[0] if text else ""
    return f"{type(exc).__name__}: {head}" if head else type(exc).__name__


@dataclass
class Deps:
    """一个场景要用到的全套替身。"""

    config: AiteConfig
    platform: FakePlatform
    #: 永远是探针（见 build_deps）：它记账每一步出牌，也补上了 live 模型客户端没有的
    #: `calls` / `call_count` —— 下面 activity() / stats() 要这两样
    model: ModelProbe
    #: `--sandbox fake`（默认）是 `FakeSandbox` / `FakeToolGateway`；`--sandbox docker`
    #: 是真 `DockerSandbox` / `P0ToolGateway` 各套一层 `real_stack` 的探针。两档下
    #: 断言读到的记账面（`.calls` / `count()` / `results_of()`）同名同义。
    sandbox: FakeSandbox | Any
    gateway: FakeToolGateway | Any
    store: FakeSessionStore
    evidence: FakeEvidenceWriter

    async def aclose(self) -> None:
        """收摊。真沙箱那一档要靠它把容器和 docker 客户端收干净（`FakeSandbox` 没有
        `aclose`，默认档在这里什么都不做）。

        任务正常收尾时容器已经由 worker 的 `release_task` 还掉了，这里管的是没走到
        终态的那些 —— 场景超时、步数上限打断、断言前就炸了。不收的话容器要挂到
        `reap_idle` 的 `idle_sec`（默认 300s）才被捡走，而 `tests/sandbox` 那几条
        「`docker ps -a --filter label=aite.task` 为空」的断言是全机器共享的。
        """
        fn = getattr(self.sandbox, "aclose", None)
        if not callable(fn):
            return
        try:
            await fn()
        except Exception:                             # 收摊失败不该盖掉场景结论
            pass

    def activity(self) -> int:
        """所有替身被调用的总次数 —— 用来判断系统是不是已经不干活了。"""
        return sum(
            len(x.calls)
            for x in (self.platform, self.model, self.sandbox, self.gateway, self.store, self.evidence)
        )

    def stats(self) -> dict[str, int]:
        return {
            "platform_calls": len(self.platform.calls),
            "model_calls": self.model.call_count,
            "gateway_calls": self.gateway.calls.count("call"),
            "sandbox_calls": len(self.sandbox.calls),
            "sessions": len(self.store.sessions),
            "tasks": len(self.store.tasks),
        }


def build_deps(sc: Scenario, *, model: Any = None, sandbox_kind: str = "fake") -> Deps:
    """按场景里的 fixture 造一套替身。

    `sandbox_kind` 选沙箱与 Gateway 这一段接谁（`--sandbox`）：

    * `fake`（默认）—— `FakeSandbox` 照场景的 `exec_script` 演，`FakeToolGateway`
      自己实现工具。**这一档逐字节不变**：`scripts/check.sh` 的 B8、CI、T4 的 200 条
      都吃这条路。
    * `docker` —— 真 `DockerSandbox` + 真 `P0ToolGateway`，代码在容器里真跑。
      接线的两件麻烦事（断言面、`session_token`）写在 `real_stack.py` 的模块 docstring 里。
      场景的 `exec_script` 在这一档下一律忽略（同上，理由见
      `real_stack.scenarios_with_exec_script`）。

    `model` 不给就按场景的 `model_script` 造 `FakeModel`；`--model live` 会从这里塞进来
    一个 `OpenAICompatModel`。**两种都统一套一层 `ModelProbe`**：

    * 这是 `--model live` 能跑起来的前提 —— `activity()` 读 `model.calls`、`stats()` 读
      `model.call_count`，真模型客户端两样都没有（§3.2 `ModelPort` 里也确实没这两条，
      它们是替身的记账面）。套探针之前，live 连第一次网络调用都到不了就 AttributeError。
    * 顺带把每一步出牌记下来，`protocol_probe.analyze()` 据此出 §14.2 的实测报告。

    探针是透明的：未知属性透传给被包的模型，`tool_names_emitted()` / `call_count` 按
    实际观测到的出牌算，scripted 路径的断言一条都不用改。
    """
    if sandbox_kind not in SANDBOXES:
        raise PhaseError("wiring", f"不认识的 --sandbox {sandbox_kind!r}，只认 {list(SANDBOXES)}")

    config = AiteConfig.model_validate({"platform": "fake", **sc.config})
    platform = FakePlatform(
        history=sc.build_history(), documents=sc.build_documents(), files=sc.build_files()
    )
    store = FakeSessionStore()
    if sandbox_kind == "docker":
        # token_resolver 要认得这个 store，所以 store 得先建出来（见 real_stack）。
        sandbox, gateway = build_docker_stack(platform=platform, store=store, config=config)
    else:
        sandbox = FakeSandbox(exec_script=list(sc.sandbox.exec_script))
        gateway = FakeToolGateway(
            platform=platform, sandbox=sandbox, sandbox_image=config.sandbox.image
        )
    inner = model if model is not None else FakeModel(list(sc.model_script))
    return Deps(
        config=config,
        platform=platform,
        model=inner if isinstance(inner, ModelProbe) else ModelProbe(inner),
        sandbox=sandbox,
        gateway=gateway,
        store=store,
        evidence=FakeEvidenceWriter(),
    )


# --------------------------------------------------------------------------
# 发现并构造 ControlPlane
# --------------------------------------------------------------------------

def _is_plane_class(obj: Any, module_name: str) -> bool:
    if not inspect.isclass(obj) or getattr(obj, "_is_protocol", False):
        return False
    if obj.__module__ != module_name:                 # 只认这个模块自己定义的，不认 import 进来的
        return False
    return all(callable(getattr(obj, name, None)) for name in ("handle_event", "run_forever"))


def _kwargs_for(target: Any, deps: Deps) -> dict[str, Any]:
    """按参数名把替身喂进去；喂不满必填参数就抛，消息里点名缺哪个。"""
    sig = inspect.signature(target)
    params = sig.parameters
    takes_kwargs = any(p.kind is inspect.Parameter.VAR_KEYWORD for p in params.values())
    if takes_kwargs:
        return {name: getattr(deps, name) for name in PARAM_ALIASES}

    kwargs: dict[str, Any] = {}
    for dep_name, aliases in PARAM_ALIASES.items():
        for alias in aliases:
            p = params.get(alias)
            if p is not None and p.kind in (p.POSITIONAL_OR_KEYWORD, p.KEYWORD_ONLY):
                kwargs[alias] = getattr(deps, dep_name)
                break

    unfilled = [
        name
        for name, p in params.items()
        if name not in kwargs
        and name != "self"
        and p.default is inspect.Parameter.empty
        and p.kind in (p.POSITIONAL_OR_KEYWORD, p.KEYWORD_ONLY)
    ]
    if unfilled:
        raise PhaseError(
            "wiring",
            f"{getattr(target, '__qualname__', target)} 的必填参数 {unfilled} 不在已知别名表里"
            f"（可用别名见 aite/evals/wiring.py 的 PARAM_ALIASES）",
        )
    return kwargs


async def _instantiate(target: Any, deps: Deps) -> Any:
    result = target(**_kwargs_for(target, deps))
    if inspect.isawaitable(result):
        result = await result
    return result


async def build_control_plane(deps: Deps) -> Any:
    """找到并构造一个 ControlPlane。失败时抛 PhaseError('wiring', 人话原因)。"""
    tried: list[str] = []
    for modname in CANDIDATE_MODULES:
        try:
            mod = importlib.import_module(modname)
        except Exception as exc:
            tried.append(f"import {modname} 失败（{brief(exc)}）")
            continue

        for fname in CANDIDATE_FACTORIES:
            fn = getattr(mod, fname, None)
            if callable(fn):
                try:
                    return await _instantiate(fn, deps)
                except Exception as exc:
                    tried.append(f"{modname}.{fname}() 构造失败（{brief(exc)}）")

        classes = [obj for obj in vars(mod).values() if _is_plane_class(obj, mod.__name__)]
        classes.sort(key=lambda c: (0 if "ControlPlane" in c.__name__ else 1, c.__name__))
        if not classes:
            tried.append(f"{modname} 里没有同时带 handle_event / run_forever 的类")
        for cls in classes:
            try:
                return await _instantiate(cls, deps)
            except Exception as exc:
                tried.append(f"{modname}.{cls.__name__}() 构造失败（{brief(exc)}）")

    raise PhaseError(
        "wiring",
        "接不上 ControlPlane（T2 尚未合入，或构造面对不上）：" + "；".join(tried),
    )


# --------------------------------------------------------------------------
# 驱动到静默
# --------------------------------------------------------------------------

def model_busy(deps: Deps) -> bool:
    """模型这条路上还有活没干完？—— `settle()` 的静默判据要减掉这一段。

    `Deps.activity()` 是个「变没变」的探测器，看不见长时间的 await。脚本化替身瞬时
    返回，这从来没露过馅；**真模型一次 chat 动辄几秒**，期间一个替身都不会被碰 ——
    照 150ms 的静默判据，系统在第一次回包之前就被判定「不干活了」，任务当场被
    `stop_loop` 取消。`--model live` 以前就是这么跑不起来的（另一半是 `Deps` 读
    `model.calls` / `model.call_count`，见 `build_deps`）。

    两种情况算忙：

    1. 有 chat 还没返回（`ModelProbe.in_flight`）；
    2. 上一发抛了、且还有任务没落终态 —— worker 正睡在 §3.3 的 2s / 5s 退避里等着
       重试。重试用尽时 worker 把任务判 failed，任务一落终态这里就不再算忙，所以
       **不用在评测侧写死退避时长**（那个数是 worker 的，抄一份迟早对不上）。

    对 scripted 路径是恒等变换：替身瞬时返回，`in_flight` 只在同一个 tick 内非零。
    """
    model = deps.model
    if getattr(model, "in_flight", 0):
        return True
    if not getattr(model, "awaiting_retry", False):
        return False
    return any(t.status in ACTIVE_STATUSES for t in deps.store.tasks.values())


def sandbox_busy(deps: Deps) -> bool:
    """沙箱这条路上还有活没干完？—— 与 `model_busy` 同一个理由，换成沙箱那一头。

    `--sandbox docker` 下 `acquire` 要起容器 + 探路，`exec` 要在容器里真跑代码，
    一次一秒起步；这期间没有任何替身被碰，`Deps.activity()` 一动不动。不减掉这一段的话
    任务在第一个容器建好之前就被判「不干活了」，`stop_loop` 当场取消 —— 实测 `04` 停在
    169ms、七条断言全红，容器建好即被收走。

    对默认档是恒等变换：`FakeSandbox` 没有 `in_flight`，`getattr` 取到 0。
    """
    return bool(getattr(deps.sandbox, "in_flight", 0))


def busy(deps: Deps) -> bool:
    """系统还在等外部返回吗（模型 / 沙箱）。`settle()` 的静默判据要减掉这一段。"""
    return model_busy(deps) or sandbox_busy(deps)


def plane_id(plane: Any) -> str:
    """报告里用它指认「接到的是谁」，省得只看到一句 NotImplementedError。"""
    cls = type(plane)
    return f"{cls.__module__}.{cls.__qualname__}"


async def start_loop(plane: Any) -> asyncio.Task | None:
    """把 run_forever 挂到后台。它立刻炸掉的话，这里就把原因抛出来。"""
    fn = getattr(plane, "run_forever", None)
    if not callable(fn):
        return None
    task = asyncio.create_task(fn())
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    if task.done() and not task.cancelled():
        exc = task.exception()
        if exc is not None:
            raise PhaseError("drive", f"{plane_id(plane)}.run_forever() 起不来：{brief(exc)}")
    return task


async def settle(
    plane: Any,
    deps: Deps,
    loop_task: asyncio.Task | None,
    *,
    timeout_sec: float,
    idle_ms: int = 150,
    poll_ms: int = 5,
) -> str:
    """等系统把手上的活干完。返回用了哪种方式（drain / quiesce）。"""
    for name in DRAIN_METHODS:
        fn = getattr(plane, name, None)
        if callable(fn):
            result = fn()
            if inspect.isawaitable(result):
                await asyncio.wait_for(result, timeout=timeout_sec)
            return name

    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout_sec
    last, quiet_since = deps.activity(), loop.time()
    while loop.time() < deadline:
        if loop_task is not None and loop_task.done() and not loop_task.cancelled():
            exc = loop_task.exception()
            if exc is not None:
                raise PhaseError("drive", f"{plane_id(plane)}.run_forever() 中途异常：{brief(exc)}")
            break
        await asyncio.sleep(poll_ms / 1000)
        now_activity = deps.activity()
        if now_activity != last or busy(deps):
            last, quiet_since = now_activity, loop.time()
            continue
        if (loop.time() - quiet_since) * 1000 >= idle_ms:
            return "quiesce"
    return "timeout" if loop.time() >= deadline else "quiesce"


# --------------------------------------------------------------------------
# 投事件前的等待（冻结契约 C-T5T6-1）
# --------------------------------------------------------------------------
#
# 真实平台上两条消息之间隔着人打字的时间；投递循环默认零间隔连着投，两条消息落在
# 同一个事件循环 tick 里，系统根本没机会消化第一条。02 就是这么红的：e2 投到时 e1
# 的任务还在跑，控制面按 §3.5 R6 把它当 steer 合进当前任务，第二个 task 就没了。
#
# 要修的是 runner 怎么投，不是控制面怎么判 —— R6 的行为本身是对的。

def newest_task(deps: Deps) -> Task | None:
    """store 里最后建出来的那个任务。

    `FakeSessionStore.tasks` 是插入序的 dict，`create_task` 只 insert、`update_task`
    只覆盖已有键，所以末位就是最新建的那个。直接读字段、不走 store 的方法 ——
    别让「等待」这个动作本身给 `Deps.activity()` 添活动，那会把静默判据搅浑。
    """
    tasks = list(deps.store.tasks.values())
    return tasks[-1] if tasks else None


def _raise_if_loop_died(plane: Any, loop_task: asyncio.Task | None) -> None:
    """run_forever 已经带异常收场的话，就地抛出来 —— 免得白等一个永远不会再动的系统。"""
    if loop_task is None or not loop_task.done() or loop_task.cancelled():
        return
    exc = loop_task.exception()
    if exc is not None:
        raise PhaseError("drive", f"{plane_id(plane)}.run_forever() 中途异常：{brief(exc)}")


async def _wait_idle(
    plane: Any,
    deps: Deps,
    loop_task: asyncio.Task | None,
    *,
    label: str,
    timeout_sec: float,
) -> None:
    """等系统静默：在跑的任务都收了、待处理队列空了。

    判据直接复用 `settle()` —— 场景收尾用哪套标准判「系统不干活了」，投事件前就用
    哪套，免得同一件事在两处有两个说法。
    """
    try:
        settled_by = await settle(plane, deps, loop_task, timeout_sec=timeout_sec)
    except TimeoutError as exc:                       # drain 那条路超时是抛出来的
        raise PhaseError(
            "dispatch",
            f"投 {label} 前等系统静默（after=idle）：{timeout_sec}s 内 drain 没跑完",
        ) from exc
    if settled_by == "timeout":
        raise PhaseError(
            "dispatch",
            f"投 {label} 前等系统静默（after=idle）：{timeout_sec}s 内替身一直在被调用，"
            "任务没收完或队列没排空",
        )


async def _wait_running(
    plane: Any,
    deps: Deps,
    loop_task: asyncio.Task | None,
    *,
    label: str,
    timeout_sec: float,
    poll_ms: int = 5,
) -> None:
    """等最近建的那个任务真的被 worker 领走、开始跑了。

    判据是「任务离开了 `created`」：§4.5 的状态机里只有 worker 会把任务推出 created
    （`AgentWorker._loop` 第一步就置 planning）。所以**「store 里有一行 task」不算数**
    —— 任务建好还躺在队列里时它仍然是 created，07 栽的就是这个区别。

    「最近建的那个」在正常时序下就是上一条事件起的那个；上一条没起任务（比如 `!status`）
    时它落回更早那个仍在跑的任务，也正是场景想等的那个。
    """
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout_sec
    while True:
        _raise_if_loop_died(plane, loop_task)
        task = newest_task(deps)
        if task is not None and task.status != TaskStatus.created:
            return
        if loop.time() >= deadline:
            break
        await asyncio.sleep(poll_ms / 1000)

    stuck = newest_task(deps)
    why = (
        f"任务 {stuck.task_no} 一直停在 created，没被 worker 领走"
        if stuck is not None
        else "一个任务都没建起来"
    )
    raise PhaseError("dispatch", f"投 {label} 前等任务开跑（after=running）：{timeout_sec}s 内{why}")


async def wait_before_dispatch(
    plane: Any,
    deps: Deps,
    loop_task: asyncio.Task | None,
    *,
    after: EventAfter,
    label: str,
    timeout_sec: float,
) -> None:
    """按 C-T5T6-1 的 `after` 等一等，然后才轮到 runner 投这条事件。

    等不到就以 `phase="dispatch"` 失败收场，绝不「等不到就接着投」—— 那样测出来的
    绿是假的。
    """
    if after == "none":
        return
    if after == "idle":
        await _wait_idle(plane, deps, loop_task, label=label, timeout_sec=timeout_sec)
        return
    if after == "running":
        await _wait_running(plane, deps, loop_task, label=label, timeout_sec=timeout_sec)
        return
    raise PhaseError(                                 # pydantic 拦得住，这里只是兜底
        "dispatch", f"{label} 的 after={after!r} 不认识（只认 none / idle / running）"
    )


async def stop_loop(loop_task: asyncio.Task | None) -> None:
    if loop_task is None or loop_task.done():
        return
    loop_task.cancel()
    try:
        await loop_task
    except (asyncio.CancelledError, Exception):       # 收尾阶段的异常不该盖掉场景结论
        pass
