"""scripts/evidence_show.py 的测试（owner: T10）。

口径：证据一律用真的 `FileEvidenceWriter` 往 tmp_path 里写，不手搓 jsonl ——
手搓的 jsonl 只能证明「我的渲染和我的假数据自洽」，证明不了它读得懂 T2 真写出来的东西。
"""
import json
import os
import re

import evidence_show as es
import pytest

from aite.contracts import EvidenceKind, canonical_json, payload_hash_of
from aite.evidence.writer import FileEvidenceWriter

MODEL_PRICE_IN = 2.4
MODEL_PRICE_OUT = 9.6


async def write_full_task(root, task_id: str = "tsk_demo") -> FileEvidenceWriter:
    """一个走完全程的任务：建任务 → 两次模型调用 → 清单 → 一次失败的工具 → 产物 → 交付。"""
    w = FileEvidenceWriter(root)

    async def add(kind, payload):
        return await w.append(task_id, kind, payload)

    await add(EvidenceKind.task_created, {
        "session_id": "ses_1", "task_no": "#A1", "chat_id": "oc_1",
        "created_by": "ou_1", "title": "把这个 CSV 画成月度趋势图",
    })
    await add(EvidenceKind.event_received, {
        "event_id": "evt_1", "kind": "message", "chat_id": "oc_1",
        "sender_id": "ou_1", "message_id": "om_1", "mentioned": True,
    })
    await add(EvidenceKind.model_call, {
        "model": "qwen-max", "step": 0, "messages_hash": "a" * 64,
        "usage": {"input_tokens": 1000, "output_tokens": 100, "cached_tokens": 0},
        "finish_reason": "tool_calls",
    })
    await add(EvidenceKind.tool_call, {
        "call_id": "c1", "name": "checklist_add",
        "arguments": {"items": ["读取 CSV", "按月汇总", "画趋势图"]},
    })
    await add(EvidenceKind.checklist_op, {
        "op": "add", "ids": ["c1", "c2", "c3"], "items": ["读取 CSV", "按月汇总", "画趋势图"],
    })
    await add(EvidenceKind.tool_result, {
        "call_id": "c1", "name": "checklist_add", "ok": True, "error": None,
        "content_hash": "b" * 64, "duration_ms": 0,
    })
    await add(EvidenceKind.checklist_op, {"op": "check", "id": "c2", "state": "done"})
    await add(EvidenceKind.tool_call, {
        "call_id": "c2", "name": "run_python",
        "arguments": {"code": "print(1)", "timeout_sec": 60},
    })
    await add(EvidenceKind.tool_result, {
        "call_id": "c2", "name": "run_python", "ok": False, "error": "timeout",
        "content_hash": "c" * 64, "duration_ms": 60001,
    })
    await add(EvidenceKind.model_call, {
        "model": "qwen-max", "step": 1, "messages_hash": "d" * 64,
        "usage": {"input_tokens": 2000, "output_tokens": 300, "cached_tokens": 1024},
        "finish_reason": "tool_calls",
    })
    await add(EvidenceKind.artifact, {
        "title": "月度趋势", "mime": "image/png", "sha256": "e" * 64, "size": 4096,
    })
    await add(EvidenceKind.delivered, {"artifacts": 1, "missing": [], "steps": 2})
    await w.finalize(task_id, {
        "session_id": "ses_1", "task_no": "#A1", "created_by": "ou_1", "model": "qwen-max",
    })
    return w


@pytest.fixture
async def full_task(tmp_path):
    await write_full_task(tmp_path)
    return tmp_path / "tsk_demo"


def load(task_dir):
    return es.load_timeline(task_dir, price_in=MODEL_PRICE_IN, price_out=MODEL_PRICE_OUT)


# ---------------------------------------------------------------- 正常任务


async def test_full_task_renders_every_event_and_verifies(full_task):
    tl = load(full_task)

    assert tl.ok
    assert tl.finalized
    assert len(tl.rows) == 12
    assert tl.checked == 12
    assert tl.issues == []
    assert tl.manifest_problems == []
    # root_hash 就是 manifest 里那个
    assert tl.root_hash == json.loads((full_task / "manifest.json").read_text())["root_hash"]


async def test_summary_numbers_match_the_events(full_task):
    s = load(full_task).stats

    assert s.events == 12
    assert s.model_calls == 2
    assert s.tokens_in == 3000
    assert s.tokens_out == 400
    assert s.tool_calls == 2
    assert s.tool_failed == 1          # run_python 那次 timeout
    assert s.artifacts == 1
    assert s.terminal == "delivered"
    assert s.unreadable == 0
    # 与 aite/worker/loop.py 的 _price 同式：(in*price_in + out*price_out)/1e6
    assert s.cost == pytest.approx((3000 * MODEL_PRICE_IN + 400 * MODEL_PRICE_OUT) / 1e6)


async def test_rendered_line_count_matches_event_count(full_task):
    tl = load(full_task)
    text = es.render_text(tl, tl.rows, filtered=False)

    body = text.split("── 汇总")[0].splitlines()
    # 表头 2 行（列名 + 分隔线）之后，一个事件一行：[标记] seq +耗时
    event_lines = [ln for ln in body if re.match(r"^[★✗ ]\s+\d+\s+\+", ln)]
    assert len(event_lines) == len(tl.rows) == 12
    assert "hash 链   OK" in text
    assert "12 条全部闭合" in text


async def test_terminal_event_is_marked(full_task):
    tl = load(full_task)
    text = es.render_text(tl, tl.rows, filtered=False)

    delivered = [ln for ln in text.splitlines() if "delivered " in ln and "已交付" in ln]
    assert len(delivered) == 1
    assert delivered[0].startswith(es.MARK_TERMINAL)


async def test_checklist_check_shows_the_item_text(full_task):
    """payload 里只有 id 和 state，文本要从前面的 add 事件里补回来。"""
    tl = load(full_task)
    row = next(r for r in tl.rows if r.kind == "checklist_op" and r.fields.get("op") == "check")

    assert "c2" in row.detail
    assert "按月汇总" in row.detail


async def test_model_call_line_carries_usage_and_cost(full_task):
    tl = load(full_task)
    row = next(r for r in tl.rows if r.kind == "model_call")

    assert row.fields["model"] == "qwen-max"
    assert row.fields["finish_reason"] == "tool_calls"
    assert row.fields["total_tokens"] == 1100
    assert "in=1000" in row.detail and "out=100" in row.detail
    assert "¥" in row.detail


async def test_tool_result_shows_error_code_and_duration(full_task):
    tl = load(full_task)
    row = next(r for r in tl.rows if r.kind == "tool_result" and not r.fields["ok"])

    assert "FAIL[timeout]" in row.detail
    assert "60001ms" in row.detail


# ---------------------------------------------------------------- 篡改


async def test_tampered_payload_is_caught_and_points_at_the_break(tmp_path):
    await write_full_task(tmp_path)
    task_dir = tmp_path / "tsk_demo"
    path = task_dir / "events.jsonl"
    lines = path.read_text(encoding="utf-8").splitlines()

    obj = json.loads(lines[3])                      # seq=3 的 tool_call
    original_hash = obj["payload_hash"]
    obj["payload"]["arguments"]["items"] = ["偷偷改成别的"]
    lines[3] = json.dumps(obj, ensure_ascii=False)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    tl = load(task_dir)

    assert not tl.ok
    assert len(tl.issues) == 1
    issue = tl.issues[0]
    assert issue.seq == 3                            # 断在第几条
    assert issue.line == 4                           # events.jsonl 的第几行
    assert "payload_hash" in issue.problem
    assert issue.expected == original_hash           # 期望
    assert issue.actual == payload_hash_of(obj["payload"])   # 实际
    # 只报一处，不级联：改一条不该把后面 8 条都染红
    assert [r.seq for r in tl.rows if r.broken] == [3]


async def test_tampered_chain_exits_nonzero_and_says_so(tmp_path, capsys):
    await write_full_task(tmp_path)
    path = tmp_path / "tsk_demo" / "events.jsonl"
    lines = path.read_text(encoding="utf-8").splitlines()
    obj = json.loads(lines[2])
    obj["payload"]["usage"]["input_tokens"] = 999999
    lines[2] = json.dumps(obj, ensure_ascii=False)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    code = es.main(["--dir", str(tmp_path / "tsk_demo")])
    out = capsys.readouterr().out

    assert code == 1
    assert "hash 链   断了" in out
    assert "seq=2" in out
    assert "这份证据不可信" in out


async def test_cut_line_breaks_the_chain_at_the_next_event(tmp_path):
    """有人把中间一行删了：seq 不连续 + prev_hash 接不住，两条都要报。"""
    await write_full_task(tmp_path)
    task_dir = tmp_path / "tsk_demo"
    path = task_dir / "events.jsonl"
    lines = path.read_text(encoding="utf-8").splitlines()
    del lines[5]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    tl = load(task_dir)

    assert not tl.ok
    problems = {i.problem for i in tl.issues}
    assert "seq 不连续" in problems
    assert "prev_hash 接不住上一条" in problems
    assert min(i.seq for i in tl.issues) == 6        # 删掉 seq=5 之后，从 6 开始对不上


async def test_manifest_root_hash_mismatch_is_reported(tmp_path):
    await write_full_task(tmp_path)
    task_dir = tmp_path / "tsk_demo"
    m = json.loads((task_dir / "manifest.json").read_text(encoding="utf-8"))
    m["root_hash"] = "0" * 64
    (task_dir / "manifest.json").write_text(json.dumps(m), encoding="utf-8")

    tl = load(task_dir)

    assert not tl.ok
    assert tl.issues == []                            # 事件本身没问题
    assert any("root_hash" in p for p in tl.manifest_problems)


async def test_garbage_line_does_not_crash(tmp_path):
    await write_full_task(tmp_path)
    task_dir = tmp_path / "tsk_demo"
    path = task_dir / "events.jsonl"
    lines = path.read_text(encoding="utf-8").splitlines()
    lines[4] = "{ 这不是 json"
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    tl = load(task_dir)

    assert not tl.ok
    assert any("不是合法的 EvidenceEvent" in i.problem for i in tl.issues)
    assert len(tl.rows) == 11                          # 剩下的照渲染


# ---------------------------------------------------------------- 未 finalize


async def test_unfinalized_task_still_renders(tmp_path):
    w = FileEvidenceWriter(tmp_path)
    await w.append("tsk_live", EvidenceKind.task_created, {"task_no": "#A2", "title": "还在跑"})
    await w.append("tsk_live", EvidenceKind.model_call, {
        "model": "qwen-max", "step": 0, "messages_hash": "f" * 64,
        "usage": {"input_tokens": 10, "output_tokens": 1, "cached_tokens": 0},
        "finish_reason": "tool_calls",
    })

    tl = load(tmp_path / "tsk_live")
    text = es.render_text(tl, tl.rows, filtered=False)

    assert not tl.finalized
    assert tl.ok                     # 没 finalize 不等于损坏
    assert len(tl.rows) == 2
    assert "未 finalize" in text
    assert "无终态事件" in text


async def test_unfinalized_task_exits_zero(tmp_path, capsys):
    w = FileEvidenceWriter(tmp_path)
    await w.append("tsk_live", EvidenceKind.task_created, {"task_no": "#A2", "title": "还在跑"})

    code = es.main(["--dir", str(tmp_path / "tsk_live")])

    assert code == 0
    assert "未 finalize" in capsys.readouterr().out


# ---------------------------------------------------------------- 外置 payload


async def test_external_payload_is_followed(tmp_path):
    w = FileEvidenceWriter(tmp_path)
    big = {"call_id": "c9", "name": "run_python", "ok": True, "error": None,
           "content_hash": "9" * 64, "duration_ms": 12, "blob": "x" * 70000}
    ev = await w.append("tsk_big", EvidenceKind.tool_result, big)
    assert ev.payload_ref == "payloads/0.json" and ev.payload is None   # 前提：真外置了

    tl = load(tmp_path / "tsk_big")

    assert tl.ok
    assert tl.rows[0].fields["name"] == "run_python"          # 跟过去读到了
    assert tl.rows[0].fields["duration_ms"] == 12
    assert "payloads/0.json" in tl.rows[0].note
    assert tl.stats.unreadable == 0


async def test_missing_external_payload_degrades_gracefully(tmp_path):
    w = FileEvidenceWriter(tmp_path)
    await w.append("tsk_big", EvidenceKind.tool_result, {
        "call_id": "c9", "name": "run_python", "ok": True, "error": None,
        "content_hash": "9" * 64, "duration_ms": 12, "blob": "x" * 70000,
    })
    await w.append("tsk_big", EvidenceKind.delivered, {"artifacts": 0, "missing": [], "steps": 1})
    (tmp_path / "tsk_big" / "payloads" / "0.json").unlink()

    tl = load(tmp_path / "tsk_big")
    text = es.render_text(tl, tl.rows, filtered=False)

    assert not tl.ok                                  # 校验不了就是校验不了
    assert len(tl.rows) == 2                          # 但不崩，剩下的照渲染
    assert "payload 缺失" in tl.rows[0].detail
    assert "payloads/0.json" in text
    # 读不到的那条不许混进汇总数字（否则 ok=False 会被当成「工具失败一次」）
    assert tl.stats.tool_calls == 0
    assert tl.stats.tool_failed == 0
    assert tl.stats.unreadable == 1


# ---------------------------------------------------------------- CLI


async def test_only_and_tail_filter_display_but_not_summary(full_task, capsys):
    code = es.main(["--dir", str(full_task), "--only", "model_call", "--tail", "1"])
    out = capsys.readouterr().out

    assert code == 0
    assert "显示 1/12 条" in out
    assert "模型调用  2 次" in out          # 汇总仍按全量
    assert "12 条全部闭合" in out           # 校验也按全量


async def test_json_output_is_machine_readable(full_task, capsys):
    code = es.main(["--dir", str(full_task), "--json"])
    data = json.loads(capsys.readouterr().out)

    assert code == 0
    assert data["task_id"] == "tsk_demo"
    assert data["finalized"] is True
    assert data["chain"]["ok"] is True
    assert data["chain"]["checked"] == 12
    assert data["summary"]["model_calls"] == 2
    assert len(data["events"]) == 12
    assert data["events"][0]["kind"] == "task_created"


async def test_list_orders_newest_first_and_flags_broken(tmp_path, capsys):
    await write_full_task(tmp_path, "tsk_old")
    await write_full_task(tmp_path, "tsk_new")
    old_events = tmp_path / "tsk_old" / "events.jsonl"
    lines = old_events.read_text(encoding="utf-8").splitlines()
    obj = json.loads(lines[0])
    obj["payload"]["title"] = "改过了"
    lines[0] = json.dumps(obj, ensure_ascii=False)
    old_events.write_text("\n".join(lines) + "\n", encoding="utf-8")
    # 两个任务是连着写的，mtime 可能一模一样 —— 把顺序钉死，别让文件系统精度决定断言
    os.utime(old_events, (1_700_000_000, 1_700_000_000))
    os.utime(tmp_path / "tsk_new" / "events.jsonl", (1_700_000_600, 1_700_000_600))

    code = es.main(["--list", "--root", str(tmp_path)])
    out = capsys.readouterr().out

    assert code == 1                                    # 有断链 → 非零
    assert [r["task_id"] for r in es.list_tasks(tmp_path)] == ["tsk_new", "tsk_old"]
    assert out.index("tsk_new") < out.index("tsk_old")   # 最近写入的排前面
    assert "证据链不可信" in out
    assert "tsk_old" in out.split("证据链不可信")[1]
    assert "tsk_new" not in out.split("证据链不可信")[1]


async def test_missing_task_exits_two(tmp_path, capsys):
    code = es.main(["--dir", str(tmp_path / "nope")])

    assert code == 2
    assert "找不到证据" in capsys.readouterr().err


async def test_bad_only_value_exits_two(full_task, capsys):
    code = es.main(["--dir", str(full_task), "--only", "model_call,不存在的kind"])

    assert code == 2
    assert "不认识的 kind" in capsys.readouterr().err


async def test_list_and_dir_together_is_rejected(tmp_path, capsys):
    code = es.main(["--list", "--dir", str(tmp_path)])

    assert code == 2
    assert "--root" in capsys.readouterr().err


# ---------------------------------------------------------------- 不许漏密钥


async def test_secretish_tool_arguments_are_redacted(tmp_path, capsys):
    """工具参数里键名带 token/secret/key 的，只打 ***，取值一个字都不许出去。"""
    secret = "s3cr3t-do-not-print-me"
    w = FileEvidenceWriter(tmp_path)
    await w.append("tsk_sec", EvidenceKind.tool_call, {
        "call_id": "c1", "name": "http_get",
        "arguments": {"url": "https://x/y", "api_key": secret, "auth_token": secret,
                      "password": secret, "harmless": "ok"},
    })

    code = es.main(["--dir", str(tmp_path / "tsk_sec")])
    captured = capsys.readouterr()

    assert code == 0
    assert secret not in captured.out
    assert secret not in captured.err
    assert "***" in captured.out
    assert "harmless=ok" in captured.out


async def test_secretish_arguments_are_redacted_in_json_too(tmp_path, capsys):
    secret = "s3cr3t-do-not-print-me"
    w = FileEvidenceWriter(tmp_path)
    await w.append("tsk_sec", EvidenceKind.tool_call, {
        "call_id": "c1", "name": "http_get", "arguments": {"api_key": secret},
    })

    es.main(["--dir", str(tmp_path / "tsk_sec"), "--json"])
    captured = capsys.readouterr()

    assert secret not in captured.out
    assert secret not in captured.err


async def test_long_arguments_are_truncated(tmp_path):
    """模型写的一大段 code 不该整段进终端 —— 截断，别把全文倒出来。"""
    code_text = "print('x')\n" * 500
    w = FileEvidenceWriter(tmp_path)
    await w.append("tsk_long", EvidenceKind.tool_call, {
        "call_id": "c1", "name": "run_python", "arguments": {"code": code_text},
    })

    tl = load(tmp_path / "tsk_long")

    assert len(tl.rows[0].detail) < 120
    assert "…" in tl.rows[0].detail


async def test_payload_hash_only_fields_never_become_text(full_task):
    """W8：证据里存的是 messages_hash，渲染时不许假装能还原全文。"""
    tl = load(full_task)
    row = next(r for r in tl.rows if r.kind == "model_call")

    assert "messages_hash" not in row.fields
    assert "a" * 64 not in row.detail


# ---------------------------------------------------------------- 契约没漂


async def test_verification_agrees_with_the_writer(tmp_path):
    """同一份证据，工具说 OK 时 FileEvidenceWriter.verify 也得说 True，反之亦然。"""
    w = await write_full_task(tmp_path)
    task_dir = tmp_path / "tsk_demo"

    assert load(task_dir).ok is True
    assert w.verify("tsk_demo") is True

    path = task_dir / "events.jsonl"
    lines = path.read_text(encoding="utf-8").splitlines()
    obj = json.loads(lines[1])
    obj["payload"]["mentioned"] = False
    lines[1] = json.dumps(obj, ensure_ascii=False)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    assert load(task_dir).ok is False
    assert w.verify("tsk_demo") is False


def test_canonical_json_is_the_contract_one():
    """花费和 hash 都建立在契约那两个函数上，这里钉一下没被本地实现替换掉。"""
    assert canonical_json({"a": 1}) == '{"a":1}'
    assert payload_hash_of({"a": 1}) == (
        "015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
    )


# ---------------------------------------------------------------- 边角


async def test_broken_manifest_is_not_reported_as_unfinalized(tmp_path):
    """manifest.json 在但读不了，和「压根没写」是两码事，别混成一句话。"""
    await write_full_task(tmp_path)
    task_dir = tmp_path / "tsk_demo"
    (task_dir / "manifest.json").write_text("not json at all", encoding="utf-8")

    tl = load(task_dir)
    text = es.render_text(tl, tl.rows, filtered=False)

    assert tl.finalized is True
    assert tl.manifest is None
    assert not tl.ok
    assert "manifest.json" in " ".join(tl.manifest_problems)
    assert "未 finalize" not in text


async def test_negative_tail_is_rejected(full_task, capsys):
    code = es.main(["--dir", str(full_task), "--tail", "-3"])

    assert code == 2
    assert "--tail" in capsys.readouterr().err


async def test_list_says_when_the_root_is_missing(tmp_path, capsys):
    code = es.main(["--list", "--root", str(tmp_path / "nowhere")])
    out = capsys.readouterr().out

    assert code == 0
    assert "这个目录不存在" in out


async def test_tail_zero_shows_nothing(full_task):
    tl = load(full_task)

    assert es._filter_rows(tl.rows, None, 0) == []
    assert len(es._filter_rows(tl.rows, None, 3)) == 3
    assert len(es._filter_rows(tl.rows, None, None)) == 12
