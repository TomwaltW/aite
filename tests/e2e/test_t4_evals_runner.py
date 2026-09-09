"""评测 runner 自测（dev-spec §6 T4）。

两件事最要紧：

1. **别的轨没合入时，每个场景报的是人话原因，不是异常栈** —— 这是 §6 T4 的原文要求。
2. **这套 harness 不是空壳**：用 tests/e2e/conftest.py 里的 DemoPlane（一个只覆盖
   R1/R2/R6/R7 + 一步 final 的最小实现）把 01/02/09/10 真跑绿，证明
   「场景 yaml → 事件 → 替身 → expect 断言」整条链是通的。
"""
from __future__ import annotations

import json

import pytest

from aite.evals import run_scenario, run_suite
from aite.evals.__main__ import main
from aite.evals.checks import CheckError, run_checks
from aite.evals.scenario import Scenario
from aite.evals.wiring import Deps, build_deps

#: DemoPlane 覆盖得到的场景（它只实现最短路径，卡片/工具/命令那几条覆盖不到）
DEMO_PASSABLE = ["01_simple_qa", "02_thread_followup", "09_bot_ignored", "10_duplicate_event"]


# --- 优雅降级 ---------------------------------------------------------------

async def test_every_scenario_reports_a_reason_not_a_stack(suite):
    """并行期的核心要求：跑得完、每条都有原因、原因里没有 Traceback。"""
    result = await run_suite(suite, suite="evals/p0")
    assert result.total == 10
    for r in result.results:
        assert r.passed is False or r.phase == "ok"
        if not r.passed:
            assert r.reason, f"{r.name} 失败了却没给原因"
            assert "Traceback" not in r.reason
            assert "\n" not in r.reason, f"{r.name} 的原因是多行的，八成把栈塞进去了：{r.reason!r}"
            assert r.phase in {"wiring", "dispatch", "drive", "assert", "error"}


async def test_summary_is_json_serialisable(suite):
    result = await run_suite(suite, suite="evals/p0")
    payload = json.loads(json.dumps(result.to_json(), ensure_ascii=False))
    assert payload["total"] == 10
    assert payload["contract_version"] == "p0.1"
    assert len(payload["scenarios"]) == 10
    assert payload["passed"] + payload["failed"] == 10


async def test_wiring_failure_is_attributed_to_the_missing_track(suite):
    """T2 还没合入时，原因要指得出是哪一环 —— 而不是一句「失败」。"""
    result = await run_suite(suite[:1], suite="evals/p0")
    r = result.results[0]
    if not r.passed:
        assert r.phase in {"wiring", "drive"}
        assert any(k in r.reason for k in ("ControlPlane", "run_forever", "NotImplementedError"))


async def test_broken_plane_factory_is_caught_not_raised(scenarios_by_name):
    def boom(deps: Deps):
        raise RuntimeError("接线炸了")

    r = await run_scenario(scenarios_by_name["01_simple_qa"], plane_factory=boom)
    assert r.passed is False and "接线炸了" in r.reason and "Traceback" not in r.reason


async def test_handle_event_failure_is_reported_as_dispatch(scenarios_by_name):
    class Angry:
        async def handle_event(self, ev):
            raise ValueError("我不接这个事件")

        async def run_forever(self):
            while True:
                await _forever()

    async def _forever():
        import asyncio

        await asyncio.sleep(3600)

    r = await run_scenario(scenarios_by_name["01_simple_qa"], plane_factory=lambda d: Angry())
    assert r.phase == "dispatch" and "我不接这个事件" in r.reason


async def test_a_plane_that_never_settles_times_out_with_a_reason(scenarios_by_name):
    sc = scenarios_by_name["01_simple_qa"].model_copy(update={"timeout_sec": 0.3})

    class Busy:
        def __init__(self, deps):
            self.deps = deps

        async def handle_event(self, ev):
            return None

        async def run_forever(self):
            import asyncio

            while True:                       # 一直在动，永远静不下来
                await self.deps.platform.add_reaction("om_x", "ack")
                await asyncio.sleep(0.01)

    r = await run_scenario(sc, plane_factory=Busy)
    assert r.phase == "drive" and "没有停下来" in r.reason


# --- harness 不是空壳 --------------------------------------------------------

@pytest.mark.parametrize("name", DEMO_PASSABLE)
async def test_demo_plane_makes_scenarios_actually_pass(scenarios_by_name, demo_plane_factory, name):
    """最小 ControlPlane 一接上，这几条就该真绿 —— 证明断言不是永远失败的摆设。"""
    r = await run_scenario(scenarios_by_name[name], plane_factory=demo_plane_factory)
    assert r.passed is True, f"{name} 没过：{r.reason} / {r.failures}"
    assert r.phase == "ok"
    assert r.settled_by in {"quiesce", "drain"}


async def test_demo_plane_dedups_and_creates_one_task(scenarios_by_name, demo_plane_factory):
    r = await run_scenario(scenarios_by_name["10_duplicate_event"], plane_factory=demo_plane_factory)
    assert r.passed is True
    assert r.stats["tasks"] == 1 and r.stats["model_calls"] == 1


async def test_demo_plane_ignores_bots_entirely(scenarios_by_name, demo_plane_factory):
    r = await run_scenario(scenarios_by_name["09_bot_ignored"], plane_factory=demo_plane_factory)
    assert r.passed is True
    assert r.stats["sessions"] == 0 and r.stats["model_calls"] == 0


async def test_failing_expectation_names_the_gap(scenarios_by_name, demo_plane_factory):
    """故意把断言改错，看报告说不说得清「期望什么、实际什么」。"""
    sc = scenarios_by_name["01_simple_qa"].model_copy(
        update={"expect": [{"check": "platform_calls", "method": "send_text", "equals": 7}]}
    )
    r = await run_scenario(sc, plane_factory=demo_plane_factory)
    assert r.passed is False and r.phase == "assert"
    assert r.failures == ["platform.send_text 期望 == 7，实际 1"]


# --- 断言 DSL 本身 -----------------------------------------------------------

def _empty_deps() -> Deps:
    return build_deps(Scenario(name="x"))


def test_unknown_check_is_reported_not_raised():
    failures = run_checks(_empty_deps(), [{"check": "no_such_check"}])
    assert len(failures) == 1 and "未知的 check" in failures[0]


def test_check_without_comparator_is_reported():
    failures = run_checks(_empty_deps(), [{"check": "platform_calls", "method": "send_text"}])
    assert len(failures) == 1 and "equals / min / max" in failures[0]


def test_malformed_expect_entry_is_reported():
    failures = run_checks(_empty_deps(), ["不是 mapping"])
    assert len(failures) == 1 and "不是带 check 键的 mapping" in failures[0]


def test_compare_reports_min_and_max_separately():
    deps = _empty_deps()
    assert run_checks(deps, [{"check": "model_calls", "equals": 0}]) == []
    assert run_checks(deps, [{"check": "model_calls", "min": 1}]) == ["ModelPort.chat 次数 期望 >= 1，实际 0"]


def test_check_error_is_raised_for_bad_dsl_usage():
    with pytest.raises(CheckError):
        from aite.evals.checks import REGISTRY

        REGISTRY["text"](_empty_deps(), {"check": "text", "where": "nowhere", "contains": "x"})


# --- CLI --------------------------------------------------------------------

def test_cli_list_prints_the_ten_names(capsys):
    assert main(["run", "evals/p0", "--list"]) == 0
    assert json.loads(capsys.readouterr().out) == [
        "01_simple_qa", "02_thread_followup", "03_checklist_progress", "04_csv_to_chart",
        "05_history_summary", "06_read_document", "07_commands", "08_step_limit",
        "09_bot_ignored", "10_duplicate_event",
    ]


def test_cli_run_ends_with_passed_k_of_10(capsys):
    code = main(["run", "evals/p0", "--platform", "fake", "--model", "scripted"])
    out = capsys.readouterr().out
    last = out.strip().splitlines()[-1]
    assert last.startswith("passed ") and last.endswith("/10")
    passed = int(last.split()[1].split("/")[0])
    assert code == (0 if passed == 10 else 1)
    assert "Traceback" not in out


def test_cli_run_summary_is_json_before_the_last_line(capsys):
    main(["run", "evals/p0"])
    out = capsys.readouterr().out
    body = "\n".join(out.strip().splitlines()[:-1])
    payload = json.loads(body)
    assert payload["suite"] == "evals/p0" and payload["total"] == 10


def test_cli_only_filters_scenarios(capsys):
    main(["run", "evals/p0", "--only", "01_simple_qa", "--list"])
    assert json.loads(capsys.readouterr().out) == ["01_simple_qa"]


def test_cli_rejects_unknown_scenario(capsys):
    assert main(["run", "evals/p0", "--only", "99_nope"]) == 2
    assert "没有这些场景" in capsys.readouterr().err


def test_cli_rejects_missing_suite(capsys, tmp_path):
    assert main(["run", str(tmp_path / "nope")]) == 2
    assert "场景加载失败" in capsys.readouterr().err


def test_cli_writes_json_file(tmp_path, capsys):
    out = tmp_path / "summary.json"
    main(["run", "evals/p0", "--json", str(out)])
    capsys.readouterr()
    assert json.loads(out.read_text(encoding="utf-8"))["total"] == 10
