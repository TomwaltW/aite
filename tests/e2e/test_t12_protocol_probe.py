"""协议出牌观测装置自测（owner: T12，dev-spec §14.2 / §3.3）。

装置本身要先站得住，段 B 拿真模型跑出来的那份报告才有人信。所以这里不打桩、
**用真的 `aite.control` 控制面 + 真的 `aite/worker` agent loop 跑**，只把模型换成
脚本化替身来造各种出牌 —— 报告里那几条「§3.3 兜底触发了」是系统真触发的，
不是装置自己编的。

覆盖派单要的五个问题：

    每步调了什么      test_report_lists_every_tool_call_in_order
    参数合不合 schema  test_schema_violation_*
    协议外的名字       test_out_of_protocol_tool_name_*
    几步收敛           test_steps_to_final_*, test_hit_max_steps_*
    §3.3 兜底逐条      test_fallback_*

外加两条「以前 live 根本起不来」的回归：`ModelProbe` 补上了 `Deps` 要的 `calls` /
`call_count`（test_live_model_object_survives_the_runner），`settle()` 不再把
「正在等模型回包」当成系统空闲（test_settle_waits_for_a_slow_model）。
"""
from __future__ import annotations

import asyncio
import json
import time
from types import SimpleNamespace
from typing import Any

import pytest

from aite.contracts import ALL_MODEL_TOOLS, Message, ModelTurn, ToolCallRequest, Usage
from aite.contracts.config import ModelConfig
from aite.evals import ModelProbe, run_scenario
from aite.evals.__main__ import LIVE_TIMEOUT_SCALE, _scale_timeouts, main
from aite.evals.protocol_probe import analyze, render_digest
from aite.evals.scenario import Scenario
from aite.evals.wiring import build_deps, model_busy
from aite.models import OpenAICompatModel
from aite.testing import FakeModel

NUDGE = "请调用 final 交付结果，或调用一个工具继续。"


def scenario(name: str, script: list[dict[str, Any]], **kw: Any) -> Scenario:
    """一条消息进来 + 一份模型脚本；断言留空，这里只看报告。"""
    return Scenario(
        name=name,
        model_script=script,
        events=[{"event_id": "e1", "text": "帮我算一下", "mentioned": True}],
        **kw,
    )


def final(reply: str = "算好了") -> dict[str, Any]:
    return {"tool_calls": [{"name": "final", "arguments": {"reply": reply}}]}


async def report_of(sc: Scenario) -> dict[str, Any]:
    """把场景真跑一遍（真控制面 + 真 worker），返回协议报告。"""
    r = await run_scenario(sc, collect_protocol=True)
    assert r.phase not in ("wiring", "error"), f"接不上被测系统：{r.reason}"
    assert r.protocol, "开了 collect_protocol 却没拿到报告"
    return r.protocol


# --- ModelProbe 本身：透明包装 -------------------------------------------------

async def test_probe_forwards_chat_and_counts_it():
    probe = ModelProbe(FakeModel([final()]))
    turn = await probe.chat([Message(role="user", content="hi")], ALL_MODEL_TOOLS,
                            max_tokens=16, temperature=0.0)
    assert [tc.name for tc in turn.message.tool_calls or []] == ["final"]
    assert probe.call_count == 1 and len(probe.calls) == 1
    assert probe.tool_names_emitted() == ["final"]


def test_probe_passes_unknown_attributes_through():
    inner = FakeModel([final()], name="scripted")
    probe = ModelProbe(inner)
    assert probe.name == "scripted"
    assert probe.script is inner.script            # 透传
    probe.release_holds()                          # 透传的方法也能调
    assert inner.holds_released is True


def test_probe_records_the_exception_and_reraises():
    async def run() -> None:
        probe = ModelProbe(FakeModel([{"error": "上游 503"}]))
        with pytest.raises(Exception, match="上游 503"):
            await probe.chat([Message(role="user", content="hi")], ALL_MODEL_TOOLS,
                             max_tokens=16, temperature=0.0)
        assert probe.call_count == 1
        assert probe.observations[0].error is not None and "上游 503" in probe.observations[0].error
        assert probe.awaiting_retry is True

    asyncio.run(run())


def test_deps_always_carries_a_probe():
    """live 模式的两个 AttributeError（activity/stats 要 calls/call_count）就死在这。"""
    deps = build_deps(scenario("x", [final()]))
    assert isinstance(deps.model, ModelProbe)
    assert deps.activity() == 0 and deps.stats()["model_calls"] == 0


def test_build_deps_does_not_double_wrap():
    probe = ModelProbe(FakeModel([final()]))
    deps = build_deps(scenario("x", []), model=probe)
    assert deps.model is probe


# --- 问题 1：每步调了什么 -------------------------------------------------------

async def test_report_lists_every_tool_call_in_order():
    sc = scenario("t12_steps", [
        {"tool_calls": [{"name": "checklist_add", "arguments": {"items": ["读数据", "画图"]}}]},
        {"tool_calls": [{"name": "checklist_check", "arguments": {"id": "c1"}}]},
        final(),
    ], config={"worker": {"card_update_min_interval_ms": 0}})
    report = await report_of(sc)

    assert report["runs"][0]["tools"] == ["checklist_add", "checklist_check", "final"]
    first = report["runs"][0]["steps_detail"][0]["tool_calls"][0]
    assert first["name"] == "checklist_add"
    assert first["arguments"] == {"items": '["读数据", "画图"]'}
    assert first["in_protocol"] is True and first["schema_ok"] is True


async def test_report_records_a_text_only_step_with_its_text():
    report = await report_of(scenario("t12_text", [{"text": "北京今天晴。"}]))
    step = report["runs"][0]["steps_detail"][0]
    assert step["tool_calls"] == [] and step["text"] == "北京今天晴。"


# --- 问题 2：参数合不合 schema --------------------------------------------------

async def test_schema_violation_is_reported_with_the_offending_arguments():
    """checklist_add 的 items 是 minItems=1 的数组，空数组不合 schema。"""
    sc = scenario("t12_badargs", [
        {"tool_calls": [{"name": "checklist_add", "arguments": {"items": []}}], "repeat": "inf"},
    ], config={"worker": {"max_steps": 6, "card_update_min_interval_ms": 0}})
    report = await report_of(sc)

    bad = report["schema_violations"]
    assert bad, "空 items 该被判不合 schema"
    assert bad[0]["name"] == "checklist_add"
    assert "至少 1 项" in bad[0]["error"]
    assert bad[0]["arguments"] == {"items": "[]"}


async def test_schema_check_catches_a_wrong_type_the_worker_lets_through():
    """run_python 的 timeout_sec 是 integer；给字符串该判红。"""
    sc = scenario("t12_badtype", [
        {"tool_calls": [{"name": "run_python",
                         "arguments": {"code": "print(1)", "timeout_sec": "5"}}]},
        final(),
    ], config={"worker": {"card_update_min_interval_ms": 0}})
    report = await report_of(sc)

    bad = [v for v in report["schema_violations"] if v["name"] == "run_python"]
    assert len(bad) == 1 and "期望 integer" in bad[0]["error"]


async def test_a_clean_run_has_no_schema_violations():
    report = await report_of(scenario("t12_clean", [final()]))
    assert report["schema_violations"] == [] and report["unknown_tools"] == {}


# --- 问题 3：协议外的工具名 -----------------------------------------------------

async def test_out_of_protocol_tool_name_is_named_and_counted():
    sc = scenario("t12_unknown", [
        {"tool_calls": [{"name": "web_search", "arguments": {"q": "北京天气"}}]},
        final(),
    ], config={"worker": {"card_update_min_interval_ms": 0}})
    report = await report_of(sc)

    assert report["unknown_tools"] == {"web_search": 1}
    call = report["runs"][0]["steps_detail"][0]["tool_calls"][0]
    assert call["in_protocol"] is False and "schema_ok" not in call
    # 协议外的名字进 Gateway 查无此人：§3.3 走 not_found，不是 invalid_args
    assert report["fallbacks"]["not_found"] == {"web_search": 1}


# --- 问题 4：几步收敛 -----------------------------------------------------------

async def test_steps_to_final_counts_the_step_that_delivered():
    sc = scenario("t12_converge", [
        {"tool_calls": [{"name": "checklist_add", "arguments": {"items": ["一"]}}]},
        {"tool_calls": [{"name": "checklist_check", "arguments": {"id": "c1"}}]},
        final(),
    ], config={"worker": {"card_update_min_interval_ms": 0}})
    report = await report_of(sc)

    run = report["runs"][0]
    assert run["reached_final"] is True and run["steps_to_final"] == 3
    assert report["reached_final"] == 1
    assert report["tasks"][-1]["status"] == "delivered"


async def test_hit_max_steps_is_named_when_the_model_never_finals():
    sc = scenario("t12_maxsteps", [
        {"tool_calls": [{"name": "checklist_note", "arguments": {"text": "再想想"}}],
         "repeat": "inf"},
    ], config={"worker": {"max_steps": 3, "card_update_min_interval_ms": 0}})
    report = await report_of(sc)

    assert report["runs"][0]["reached_final"] is False
    assert report["runs"][0]["steps_to_final"] is None
    assert report["hit_max_steps"] == [report["tasks"][-1]["task_no"]]
    assert report["tasks"][-1]["status"] == "failed"


async def test_two_tasks_in_one_session_are_two_runs(scenarios_by_name):
    """一个 session 里第二个 task 的上下文是第一个的前缀扩展 —— 别并成一轮。"""
    r = await run_scenario(scenarios_by_name["02_thread_followup"], collect_protocol=True)
    assert r.passed, r.failures
    assert len(r.protocol["runs"]) == 2
    assert [run["steps_to_final"] for run in r.protocol["runs"]] == [1, 1]


# --- 问题 5：§3.3 兜底逐条 ------------------------------------------------------

async def test_fallback_text_only_at_step_zero_becomes_final():
    """§3.3：模型既无 tool_call 也无 final、只回文本，且 steps==0 → 视为 final。"""
    report = await report_of(scenario("t12_fb_final", [{"text": "北京今天晴。"}]))
    fb = report["fallbacks"]

    assert len(fb["text_only_as_final"]) == 1
    assert fb["text_only_as_final"][0]["step"] == 0
    assert fb["text_only_nudge"] == []
    assert report["tasks"][-1]["status"] == "delivered"
    assert report["outbound_texts"] == ["北京今天晴。"]


async def test_fallback_text_only_after_step_zero_gets_a_system_nudge():
    """§3.3：steps>0 时不兜成 final，回一条 system 提示并计 1 步。"""
    sc = scenario("t12_fb_nudge", [
        {"tool_calls": [{"name": "checklist_note", "arguments": {"text": "想想"}}]},
        {"text": "我觉得应该没问题"},
        final(),
    ], config={"worker": {"card_update_min_interval_ms": 0}})
    report = await report_of(sc)
    fb = report["fallbacks"]

    assert fb["text_only_as_final"] == []
    assert len(fb["text_only_nudge"]) == 1
    hit = fb["text_only_nudge"][0]
    assert hit["step"] == 1 and hit["nudge"] == NUDGE
    assert report["tasks"][-1]["status"] == "delivered"


async def test_fallback_invalid_args_three_in_a_row_fails_the_task():
    """§3.3：参数不合 schema → invalid_args，同一任务连续 3 次 → failed。"""
    sc = scenario("t12_fb_invalid", [
        {"tool_calls": [{"name": "checklist_check", "arguments": {"id": "c99"}}], "repeat": "inf"},
    ], config={"worker": {"max_steps": 10, "card_update_min_interval_ms": 0}})
    report = await report_of(sc)
    ia = report["fallbacks"]["invalid_args"]

    assert ia["max_consecutive"] == 3, "连续 3 次就该收手，多一次都是漏兜"
    assert ia["by_tool"] == {"checklist_check": 3}
    assert report["tasks"][-1]["status"] == "failed"
    assert any("连续 3 次" in t for t in report["outbound_texts"])


async def test_fallback_model_error_is_retried_twice_with_2s_and_5s_backoff():
    """§3.3：模型调用异常 / 5xx → 重试 2 次（2s / 5s）。

    这条**故意不缩短退避**：要验的就是那两个真实数字，跑起来 ~7s。缩了就只证明了
    「重试了两次」，证明不了「按 2s / 5s 退避」—— 而真机上模型抖动时等多久，
    正是 §14.2 想知道的事。
    """
    sc = scenario("t12_fb_retry", [{"error": "上游 503 Service Unavailable", "repeat": "inf"}],
                  timeout_sec=20.0)
    report = await report_of(sc)

    bursts = report["fallbacks"]["model_retry"]
    assert len(bursts) == 1
    burst = bursts[0]
    assert burst["failures"] == 3 and burst["retried"] == 2 and burst["recovered"] is False
    assert all("上游 503" in e for e in burst["errors"])
    slow, slower = burst["delays_ms"]
    assert 1800 <= slow <= 3200, f"第一次退避该在 2s 上下，实际 {slow}ms"
    assert 4800 <= slower <= 6200, f"第二次退避该在 5s 上下，实际 {slower}ms"
    assert report["tasks"][-1]["status"] == "failed"
    assert any("模型服务暂不可用" in t for t in report["outbound_texts"])


async def test_fallback_model_error_recovering_on_retry_is_marked_recovered():
    sc = scenario("t12_fb_retry_ok", [{"error": "上游 503"}, final("缓过来了")], timeout_sec=20.0)
    report = await report_of(sc)

    burst = report["fallbacks"]["model_retry"][0]
    assert burst["failures"] == 1 and burst["retried"] == 1 and burst["recovered"] is True
    assert report["runs"][0]["reached_final"] is True
    assert report["tasks"][-1]["status"] == "delivered"


# --- settle()：别把「在等模型回包」当成系统空闲 -----------------------------------

def test_model_busy_is_false_for_an_idle_probe():
    deps = build_deps(scenario("x", [final()]))
    assert model_busy(deps) is False


async def test_settle_waits_for_a_slow_model():
    """真模型一次调用远超 150ms 的静默窗口 —— 以前任务会在第一次回包前被取消。"""

    class SlowModel:
        name = "slow"

        async def chat(self, messages, tools, *, max_tokens, temperature):
            await asyncio.sleep(0.6)               # 比 settle 的 idle_ms=150 长得多
            return ModelTurn(
                message=Message(
                    role="assistant",
                    content="",
                    tool_calls=[ToolCallRequest(call_id="c1", name="final",
                                                arguments={"reply": "慢但是到了"})],
                ),
                usage=Usage(),
                finish_reason="tool_calls",
            )

    started = time.monotonic()
    r = await run_scenario(scenario("t12_slow", []), model=SlowModel(), collect_protocol=True)
    elapsed = time.monotonic() - started

    assert r.stats["model_calls"] == 1, "在等模型回包期间被判成空闲、任务被取消了"
    assert elapsed >= 0.6, f"没等模型回包就收工了，只用了 {elapsed:.3f}s"
    assert r.protocol["runs"][0]["tools"] == ["final"]
    assert r.protocol["outbound_texts"] == ["慢但是到了"]


# --- live 模型对象走完 runner 全程 -----------------------------------------------

async def test_live_model_object_survives_the_runner():
    """`OpenAICompatModel` 没有 calls / call_count —— 套上探针之前，runner 必崩。"""
    payload = {
        "id": "chatcmpl-1",
        "model": "qwen-plus",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "",
                        "tool_calls": [{"id": "call_1", "type": "function", "function": {
                            "name": "final",
                            "arguments": json.dumps({"reply": "真客户端也走得通"}, ensure_ascii=False)}}]},
            "finish_reason": "tool_calls",
        }],
        "usage": {"prompt_tokens": 12, "completion_tokens": 3},
    }

    class Completions:
        async def create(self, **kwargs):
            return payload

    live = OpenAICompatModel(
        ModelConfig(provider="openai_compat", base_url="https://example.invalid/v1", model="qwen-plus"),
        client=SimpleNamespace(chat=SimpleNamespace(completions=Completions())),
    )
    r = await run_scenario(scenario("t12_live", []), model=live, collect_protocol=True)

    assert r.phase == "ok", f"live 模型跑不完：{r.reason} / {r.failures}"
    assert r.stats["model_calls"] == 1
    assert r.protocol["model"] == "qwen-plus"
    assert r.protocol["runs"][0]["tools"] == ["final"]
    assert r.protocol["outbound_texts"] == ["真客户端也走得通"]


# --- 报告的形状与 CLI ----------------------------------------------------------

async def test_report_is_json_serialisable_and_digest_is_text():
    report = await report_of(scenario("t12_json", [final()]))
    assert json.loads(json.dumps(report, ensure_ascii=False))["model"] == "scripted"
    digest = render_digest([("t12_json", report)])
    assert "协议出牌报告" in digest and "final" in digest


def test_analyze_without_a_probe_returns_nothing():
    deps = build_deps(scenario("x", []))
    deps.model = FakeModel([])                     # type: ignore[assignment]
    assert analyze(deps) == {}


def test_cli_without_the_flag_adds_no_protocol_field(capsys):
    main(["run", "evals/p0", "--only", "01_simple_qa"])
    body = "\n".join(capsys.readouterr().out.strip().splitlines()[:-1])
    assert "protocol" not in json.loads(body)["scenarios"][0]


def test_cli_protocol_report_writes_stderr_digest_and_keeps_stdout_shape(capsys):
    code = main(["run", "evals/p0", "--only", "01_simple_qa", "--protocol-report"])
    captured = capsys.readouterr()
    assert code == 0
    assert captured.out.strip().splitlines()[-1] == "passed 1/1"
    body = "\n".join(captured.out.strip().splitlines()[:-1])
    assert json.loads(body)["scenarios"][0]["protocol"]["runs"][0]["tools"] == ["final"]
    assert "协议出牌报告" in captured.err


def test_cli_protocol_report_path_writes_a_json_file(tmp_path, capsys):
    path = tmp_path / "protocol.json"
    assert main(["run", "evals/p0", "--only", "01_simple_qa", "--protocol-report", str(path)]) == 0
    capsys.readouterr()
    payload = json.loads(path.read_text(encoding="utf-8"))
    assert payload["model"] == "scripted" and payload["contract_version"] == "p0.1"
    assert payload["scenarios"][0]["name"] == "01_simple_qa"
    assert payload["scenarios"][0]["runs"][0]["tools"] == ["final"]


def test_timeout_scale_multiplies_both_waits():
    sc = scenario("t12_scale", [final()], timeout_sec=10.0)
    scaled = _scale_timeouts(sc, 12.0)
    assert scaled.timeout_sec == 120.0
    assert [e.after_timeout_sec for e in scaled.events] == [60.0]
    assert sc.timeout_sec == 10.0, "原场景对象不该被改"


def test_cli_scripted_run_keeps_stderr_empty(capsys):
    """scripted 默认 scale=1.0 —— 一个字都不往 stderr 写，check.sh 的 B8 靠这个干净。"""
    main(["run", "evals/p0", "--only", "01_simple_qa"])
    assert capsys.readouterr().err == ""


def test_cli_live_defaults_to_a_bigger_timeout_scale(capsys, tmp_path):
    """真模型一次调用就顶穿 yaml 里的 10s —— live 默认放大，且必须说出来。"""
    cfg = tmp_path / "live.yaml"
    cfg.write_text("platform: fake\nmodel:\n  base_url: ''\n  model: ''\n", encoding="utf-8")
    code = main(["run", "evals/p0", "--only", "09_bot_ignored", "--model", "live",
                 "--config", str(cfg)])
    err = capsys.readouterr().err
    assert f"x{LIVE_TIMEOUT_SCALE}" in err
    assert code in (0, 1)          # 09 不调模型，配置空不空都跑得完


def test_cli_digest_goes_to_stderr_not_stdout(capsys):
    main(["run", "evals/p0", "--only", "01_simple_qa", "--protocol-report"])
    captured = capsys.readouterr()
    assert "协议出牌报告" in captured.err
    assert "协议出牌报告" not in captured.out
