"""评测 runner（owner: T4，dev-spec §3.8）。命令行入口见 aite/evals/__main__.py。

分五层：
    scenario.py       evals/p0/*.yaml 的形状与加载
    wiring.py         把替身接到被测系统上；别的轨没合入时优雅降级成一句人话
    checks.py         expect 断言 DSL
    protocol_probe.py 看模型按不按协议出牌（§14.2 模型实测，--protocol-report）
    runner.py         跑一个场景 / 一整套，产出 JSON 摘要
"""
from .protocol_probe import ModelProbe, Observation, ToolCallObservation, analyze, render_digest
from .runner import ScenarioResult, SuiteResult, run_scenario, run_suite
from .scenario import Scenario, ScenarioError, load_scenario, load_suite

__all__ = [
    "ModelProbe",
    "Observation",
    "Scenario",
    "ScenarioError",
    "ScenarioResult",
    "SuiteResult",
    "ToolCallObservation",
    "analyze",
    "load_scenario",
    "load_suite",
    "render_digest",
    "run_scenario",
    "run_suite",
]
