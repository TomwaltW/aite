"""evals/p0 下的 10 个场景文件本身：名字、结构、断言可解析（dev-spec §3.8）。"""
from __future__ import annotations

import pytest

from aite.contracts import ALL_MODEL_TOOLS, EventKind, SenderKind
from aite.evals.checks import REGISTRY
from aite.evals.scenario import Scenario, load_scenario, load_suite

#: §3.8 那张表，逐字抄下来。场景增删改名都要先过这一条。
SPEC_38_SCENARIOS = [
    "01_simple_qa",
    "02_thread_followup",
    "03_checklist_progress",
    "04_csv_to_chart",
    "05_history_summary",
    "06_read_document",
    "07_commands",
    "08_step_limit",
    "09_bot_ignored",
    "10_duplicate_event",
]

TOOL_NAMES = {t.name for t in ALL_MODEL_TOOLS}


def test_suite_is_exactly_the_ten_spec_scenarios(suite):
    assert [sc.name for sc in suite] == SPEC_38_SCENARIOS


def test_every_scenario_documents_what_it_verifies(suite):
    for sc in suite:
        assert sc.title, f"{sc.name} 缺 title"
        assert sc.verifies, f"{sc.name} 缺 verifies"
        assert "§3.8" in sc.spec_ref, f"{sc.name} 的 spec_ref 没指回 §3.8：{sc.spec_ref!r}"


def test_every_scenario_has_events_and_expectations(suite):
    for sc in suite:
        assert sc.events, f"{sc.name} 一个事件都不投"
        assert sc.expect, f"{sc.name} 一条断言都没有"


def test_every_expect_uses_a_known_check(suite):
    for sc in suite:
        for i, spec in enumerate(sc.expect):
            assert "check" in spec, f"{sc.name}.expect[{i}] 没有 check 键"
            assert spec["check"] in REGISTRY, (
                f"{sc.name}.expect[{i}] 用了未知 check={spec['check']!r}，可用：{sorted(REGISTRY)}"
            )


def test_model_scripts_only_call_contract_tools(suite):
    """场景不许发明契约外的工具名 —— 那样跑到 TΩ 才会以 not_found 收场。"""
    for sc in suite:
        for step in sc.model_script:
            for tc in step.tool_calls:
                assert tc.name in TOOL_NAMES, f"{sc.name} 用了契约外的工具 {tc.name!r}"


def test_events_build_into_valid_normalized_events(suite):
    for sc in suite:
        for ev in sc.build_events():
            assert ev.platform == "fake"
            assert ev.anchor.message_id
            assert ev.anchor.chat_id == ev.chat_id


def test_09_is_the_only_non_human_sender(suite):
    """§3.5 R1 只该在 09 被验到；别的场景混进非真人发送者会让结论变糊。"""
    non_human = {
        sc.name for sc in suite
        for ev in sc.build_events() if ev.sender_kind is not SenderKind.human
    }
    assert non_human == {"09_bot_ignored"}


def test_10_really_sends_the_same_event_id_twice(scenarios_by_name):
    ids = [e.event_id for e in scenarios_by_name["10_duplicate_event"].events]
    assert len(ids) == 2 and len(set(ids)) == 1


def test_02_second_message_is_in_thread_without_at(scenarios_by_name):
    """R6 的形状：话题内、不带 @，thread_id 指向第一条消息。"""
    first, second = scenarios_by_name["02_thread_followup"].events
    assert first.mentioned is True and first.thread_id is None
    assert second.mentioned is False
    assert second.thread_id == f"om_{first.event_id}"


def test_04_attachment_is_downloadable_from_the_fixture(scenarios_by_name):
    """附件的 (message_id, file_key) 必须和事件对得上，否则 download_attachment 必然 upstream。"""
    sc = scenarios_by_name["04_csv_to_chart"]
    ev = sc.build_events()[0]
    assert ev.attachments, "04 没带附件"
    key = (ev.attachments[0].message_id, ev.attachments[0].file_key)
    assert key in sc.build_files(), f"附件 {key} 在 platform.files 里没有对应内容"


def test_05_history_mixes_bot_messages_in(scenarios_by_name):
    kinds = {h.sender_kind for h in scenarios_by_name["05_history_summary"].platform.history}
    assert "human" in kinds and kinds - {"human"}, "05 的历史里没有非真人消息，过滤就无从验起"


def test_06_document_key_matches_what_the_model_asks_for(scenarios_by_name):
    sc = scenarios_by_name["06_read_document"]
    asked = {
        tc.arguments["url_or_token"]
        for step in sc.model_script for tc in step.tool_calls if tc.name == "read_document"
    }
    assert asked and asked <= set(sc.build_documents())


def test_07_uses_bang_commands(scenarios_by_name):
    texts = [e.text for e in scenarios_by_name["07_commands"].events]
    assert any(t.startswith("!status") for t in texts)
    assert any(t.startswith("!stop") for t in texts)


def test_07_keeps_the_task_alive_until_the_commands_arrive(scenarios_by_name):
    """07 靠两件事保证命令到达时任务还活着，少一件断言就成了摆设。

    一是命令事件的 `after: running`（C-T5T6-1）——- 等 e1 起的任务真的被 worker
    领走再投；二是模型脚本里有一步卡住不返回（hold_ticks）——- 替身瞬时返回，
    worker 一旦被调度上就会一口气跑到步数上限，没有这个让出点，命令永远赶不上。
    老版本写的是「max_steps: 200 保证它不会自己结束」，那句是错的：200 步毫秒级就跑完了。
    """
    sc = scenarios_by_name["07_commands"]
    e1, e2, e3 = sc.events
    assert e1.after == "none"
    assert (e2.after, e3.after) == ("running", "running")
    assert any(step.hold_ticks > 0 for step in sc.model_script), "07 没有一步是卡住的"


def test_08_model_never_finishes(scenarios_by_name):
    """08 靠 repeat: inf 把模型钉死，否则任务会自己 delivered，测不到上限。"""
    sc = scenarios_by_name["08_step_limit"]
    assert sc.config["worker"]["max_steps"] == 3
    assert sc.model_script[-1].repeat == "inf"
    assert all(tc.name != "final" for step in sc.model_script for tc in step.tool_calls)


def test_09_has_an_empty_model_script(scenarios_by_name):
    """模型一次都不该被调用；脚本留空，真被调了就以「脚本已用尽」失败。"""
    assert scenarios_by_name["09_bot_ignored"].model_script == []


def test_name_must_match_filename(tmp_path):
    p = tmp_path / "99_wrong.yaml"
    p.write_text("name: something_else\n", encoding="utf-8")
    with pytest.raises(Exception, match="与文件名"):
        load_scenario(p)


def test_missing_suite_dir_is_reported(tmp_path):
    with pytest.raises(Exception, match="场景目录不存在"):
        load_suite(tmp_path / "nope")


def test_defaults_keep_scenario_files_small():
    """场景 yaml 只写关心的字段，其余靠默认值 —— 这条钉住默认值本身。"""
    sc = Scenario.model_validate({"name": "x", "events": [{"event_id": "e1", "text": "hi"}]})
    ev = sc.build_events()[0]
    assert ev.kind is EventKind.message
    assert ev.sender_kind is SenderKind.human
    assert ev.chat_type == "group" and ev.mentioned is True
    assert ev.anchor.message_id == "om_e1" and ev.anchor.thread_id is None
