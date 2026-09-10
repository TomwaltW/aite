"""跑场景：造替身 → 接上 ControlPlane → 投事件 → 等静默 → 跑断言。

铁律（§6 T4）：**每个场景必须报出失败原因而不是异常栈**。所以这一层把所有异常
都收干净，转成 `ScenarioResult.reason` 的一行人话，并标出断在哪个阶段：

    wiring    接不上被测系统（别的轨还没合入时就停在这里）
    dispatch  handle_event 抛了；或者 EventSpec.after 要等的东西没等到
    drive     run_forever 起不来 / 中途炸了 / 超时
    assert    跑到了，但断言没过
    ok        全过

投递不是零间隔连着投：每条事件按 `EventSpec.after`（契约 C-T5T6-1）先等系统消化上一条，
判据在 `aite/evals/wiring.py`。默认 `after="none"` 就是原来那样紧接着投。

需要看栈的时候加 `--traceback`，栈只写 stderr，不污染 stdout 的 JSON 摘要。
"""
from __future__ import annotations

import inspect
import time
import traceback
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from ..contracts import CONTRACT_VERSION
from .checks import run_checks
from .protocol_probe import analyze
from .scenario import Scenario
from .wiring import (
    Deps,
    PhaseError,
    brief,
    build_control_plane,
    build_deps,
    settle,
    start_loop,
    stop_loop,
    wait_before_dispatch,
)

#: 测试 / TΩ 可以绕过运行时发现，直接给一个造 ControlPlane 的工厂
PlaneFactory = Callable[[Deps], Any]


@dataclass
class ScenarioResult:
    name: str
    passed: bool
    phase: str
    reason: str | None = None
    failures: list[str] = field(default_factory=list)
    duration_ms: int = 0
    stats: dict[str, int] = field(default_factory=dict)
    settled_by: str = ""
    traceback: str | None = None
    #: §14.2 的协议出牌报告。只有开了 --protocol-report 才非空 —— 默认 JSON 摘要
    #: 一个字段都不多，T4 的 CI 读的还是原来那份。
    protocol: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> dict[str, Any]:
        row: dict[str, Any] = {
            "name": self.name,
            "passed": self.passed,
            "phase": self.phase,
            "duration_ms": self.duration_ms,
        }
        if self.reason:
            row["reason"] = self.reason
        if self.failures:
            row["failures"] = self.failures
        if self.settled_by:
            row["settled_by"] = self.settled_by
        if self.stats:
            row["stats"] = self.stats
        if self.protocol:
            row["protocol"] = self.protocol
        return row


@dataclass
class SuiteResult:
    suite: str
    platform: str
    model: str
    results: list[ScenarioResult]

    @property
    def passed(self) -> int:
        return sum(1 for r in self.results if r.passed)

    @property
    def total(self) -> int:
        return len(self.results)

    def to_json(self) -> dict[str, Any]:
        return {
            "suite": self.suite,
            "platform": self.platform,
            "model": self.model,
            "contract_version": CONTRACT_VERSION,
            "total": self.total,
            "passed": self.passed,
            "failed": self.total - self.passed,
            "scenarios": [r.to_json() for r in self.results],
        }


async def _execute(sc: Scenario, deps: Deps, plane_factory: PlaneFactory | None = None) -> str:
    """把场景真跑一遍。任何失败都以 PhaseError 抛出，带阶段标记。"""
    if plane_factory is None:
        plane = await build_control_plane(deps)
    else:
        plane = plane_factory(deps)
        if inspect.isawaitable(plane):
            plane = await plane

    try:
        await deps.store.init()
        await deps.platform.start(plane.handle_event)
    except PhaseError:
        raise
    except Exception as exc:
        raise PhaseError("wiring", f"接线失败：{brief(exc)}") from exc

    loop_task = await start_loop(plane)
    try:
        for spec, ev in sc.build_dispatch_plan():
            # after="none"（默认）一步都不多走 —— 投递路径与时序机制引入前完全一致
            if spec.after != "none":
                await wait_before_dispatch(
                    plane,
                    deps,
                    loop_task,
                    after=spec.after,
                    label=ev.event_id,
                    timeout_sec=spec.after_timeout_sec,
                )
            try:
                await deps.platform.emit(ev)
            except Exception as exc:
                raise PhaseError(
                    "dispatch", f"handle_event({ev.event_id}) 抛了：{brief(exc)}"
                ) from exc
        settled_by = await settle(plane, deps, loop_task, timeout_sec=sc.timeout_sec)
    finally:
        await stop_loop(loop_task)

    if settled_by == "timeout":
        raise PhaseError("drive", f"{sc.timeout_sec}s 内没有停下来（系统一直在动）")
    return settled_by


def _protocol(deps: Deps, wanted: bool) -> dict[str, Any]:
    """协议报告。装置本身出错也只算「报告没出来」，绝不改场景的成败结论。"""
    if not wanted:
        return {}
    try:
        return analyze(deps)
    except Exception as exc:
        return {"analyze_error": brief(exc)}


async def run_scenario(
    sc: Scenario,
    *,
    model: Any = None,
    plane_factory: PlaneFactory | None = None,
    want_traceback: bool = False,
    collect_protocol: bool = False,
) -> ScenarioResult:
    started = time.monotonic()
    deps = build_deps(sc, model=model)
    tb: str | None = None
    try:
        settled_by = await _execute(sc, deps, plane_factory)
    except PhaseError as exc:
        tb = traceback.format_exc() if want_traceback else None
        return ScenarioResult(
            name=sc.name,
            passed=False,
            phase=exc.phase,
            reason=str(exc),
            duration_ms=int((time.monotonic() - started) * 1000),
            stats=deps.stats(),
            traceback=tb,
            protocol=_protocol(deps, collect_protocol),
        )
    except Exception as exc:                       # 兜底：绝不让异常栈冒出去
        tb = traceback.format_exc() if want_traceback else None
        return ScenarioResult(
            name=sc.name,
            passed=False,
            phase="error",
            reason=brief(exc),
            duration_ms=int((time.monotonic() - started) * 1000),
            stats=deps.stats(),
            traceback=tb,
            protocol=_protocol(deps, collect_protocol),
        )

    try:
        failures = run_checks(deps, sc.expect)
    except Exception as exc:                       # 断言 DSL 本身写错也要有人话
        failures = [f"断言执行出错：{brief(exc)}"]

    return ScenarioResult(
        name=sc.name,
        passed=not failures,
        phase="ok" if not failures else "assert",
        reason=None if not failures else f"{len(failures)} 条断言没过",
        failures=failures,
        duration_ms=int((time.monotonic() - started) * 1000),
        stats=deps.stats(),
        settled_by=settled_by,
        protocol=_protocol(deps, collect_protocol),
    )


async def run_suite(
    scenarios: list[Scenario],
    *,
    suite: str,
    platform: str = "fake",
    model_name: str = "scripted",
    model_factory: Any = None,
    plane_factory: PlaneFactory | None = None,
    want_traceback: bool = False,
    collect_protocol: bool = False,
) -> SuiteResult:
    results = []
    for sc in scenarios:
        model = model_factory() if model_factory is not None else None
        results.append(
            await run_scenario(
                sc,
                model=model,
                plane_factory=plane_factory,
                want_traceback=want_traceback,
                collect_protocol=collect_protocol,
            )
        )
    return SuiteResult(suite=suite, platform=platform, model=model_name, results=results)
