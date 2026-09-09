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

from ..contracts import AiteConfig
from ..testing import (
    FakeEvidenceWriter,
    FakeModel,
    FakePlatform,
    FakeSandbox,
    FakeSessionStore,
    FakeToolGateway,
)
from .scenario import Scenario

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
    model: FakeModel
    sandbox: FakeSandbox
    gateway: FakeToolGateway
    store: FakeSessionStore
    evidence: FakeEvidenceWriter

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


def build_deps(sc: Scenario, *, model: FakeModel | None = None) -> Deps:
    """按场景里的 fixture 造一套替身。"""
    config = AiteConfig.model_validate({"platform": "fake", **sc.config})
    platform = FakePlatform(
        history=sc.build_history(), documents=sc.build_documents(), files=sc.build_files()
    )
    sandbox = FakeSandbox(exec_script=list(sc.sandbox.exec_script))
    gateway = FakeToolGateway(platform=platform, sandbox=sandbox, sandbox_image=config.sandbox.image)
    return Deps(
        config=config,
        platform=platform,
        model=model if model is not None else FakeModel(list(sc.model_script)),
        sandbox=sandbox,
        gateway=gateway,
        store=FakeSessionStore(),
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
        if now_activity != last:
            last, quiet_since = now_activity, loop.time()
            continue
        if (loop.time() - quiet_since) * 1000 >= idle_ms:
            return "quiesce"
    return "timeout" if loop.time() >= deadline else "quiesce"


async def stop_loop(loop_task: asyncio.Task | None) -> None:
    if loop_task is None or loop_task.done():
        return
    loop_task.cancel()
    try:
        await loop_task
    except (asyncio.CancelledError, Exception):       # 收尾阶段的异常不该盖掉场景结论
        pass
