"""B7：§3.1 的两个 hash 测试向量逐字节相等；篡改任一 payload 后 verify 返回 False。"""
import json

import pytest

from aite.contracts import GENESIS, EvidenceEvent, EvidenceKind
from aite.evidence import FileEvidenceWriter

# §3.1 evidence.py 末尾的测试向量，逐字符抄下来
VEC1_PAYLOAD = {"a": 1}
VEC1_PAYLOAD_HASH = "015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
VEC1_HASH = "cdae94bd29b7de0c2a7af392f4d984ed5a41b15bdd872f7b9726132508047adc"
VEC2_PAYLOAD = {"b": "文"}
VEC2_PAYLOAD_HASH = "1e8763171f38ca61b0bb0f996142a149ce16ba66c664d341d03c91f54ae4ea10"
VEC2_HASH = "11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e"

TASK = "task-vec"


@pytest.fixture
def writer(tmp_path) -> FileEvidenceWriter:
    return FileEvidenceWriter(tmp_path / "evidence")


async def _write_vectors(writer) -> tuple[EvidenceEvent, EvidenceEvent]:
    a = await writer.append(TASK, EvidenceKind.task_created, VEC1_PAYLOAD)
    b = await writer.append(TASK, EvidenceKind.model_call, VEC2_PAYLOAD)
    return a, b


async def test_hash_vectors_match_spec(writer):
    a, b = await _write_vectors(writer)

    assert a.seq == 0 and a.prev_hash == GENESIS
    assert a.payload_hash == VEC1_PAYLOAD_HASH
    assert a.hash == VEC1_HASH

    assert b.seq == 1 and b.prev_hash == VEC1_HASH
    assert b.payload_hash == VEC2_PAYLOAD_HASH
    assert b.hash == VEC2_HASH

    assert writer.verify(TASK) is True


async def test_payload_is_inlined_and_ref_is_none(writer):
    """payload_ref 在契约里没有默认值，必须显式传 None —— 落盘后确实是 null。"""
    ev = await writer.append(TASK, EvidenceKind.task_created, VEC1_PAYLOAD)
    assert ev.payload_ref is None and ev.payload == VEC1_PAYLOAD

    line = json.loads(writer.events_path(TASK).read_text(encoding="utf-8").splitlines()[0])
    assert line["payload_ref"] is None
    assert line["payload"] == VEC1_PAYLOAD


async def test_one_line_per_event(writer):
    await _write_vectors(writer)
    lines = writer.events_path(TASK).read_text(encoding="utf-8").splitlines()
    assert len(lines) == 2
    assert [EvidenceEvent.model_validate_json(x).seq for x in lines] == [0, 1]


# ---- 篡改 ----------------------------------------------------------------


def _rewrite(writer, index: int, mutate) -> None:
    path = writer.events_path(TASK)
    lines = path.read_text(encoding="utf-8").splitlines()
    row = json.loads(lines[index])
    mutate(row)
    lines[index] = json.dumps(row, ensure_ascii=False)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


@pytest.mark.parametrize("index", [0, 1])
async def test_tampering_any_payload_fails_verify(writer, index):
    await _write_vectors(writer)
    assert writer.verify(TASK) is True

    _rewrite(writer, index, lambda row: row.update(payload={"a": 999}))

    assert writer.verify(TASK) is False


async def test_tampering_payload_hash_fails_verify(writer):
    """只改 payload_hash（让它和 payload 对上是不可能的）→ 链断。"""
    await _write_vectors(writer)
    _rewrite(writer, 0, lambda row: row.update(payload_hash=VEC2_PAYLOAD_HASH))
    assert writer.verify(TASK) is False


async def test_tampering_chain_hash_fails_verify(writer):
    await _write_vectors(writer)
    _rewrite(writer, 1, lambda row: row.update(prev_hash=GENESIS))
    assert writer.verify(TASK) is False


async def test_dropping_a_line_fails_verify(writer):
    """删掉中间一条 → seq 不连续，也是 False。"""
    await _write_vectors(writer)
    await writer.append(TASK, EvidenceKind.delivered, {"c": 3})
    path = writer.events_path(TASK)
    lines = path.read_text(encoding="utf-8").splitlines()
    path.write_text(lines[0] + "\n" + lines[2] + "\n", encoding="utf-8")
    assert writer.verify(TASK) is False


def test_verify_missing_task_is_false(writer):
    assert writer.verify("没有这个任务") is False


# ---- 大 payload 外置 ------------------------------------------------------


async def test_large_payload_goes_to_payload_ref(writer):
    big = {"blob": "x" * 70_000}
    ev = await writer.append(TASK, EvidenceKind.tool_result, big)

    assert ev.payload is None
    assert ev.payload_ref == "payloads/0.json"
    assert (writer.task_dir(TASK) / ev.payload_ref).exists()
    assert ev.payload_hash  # 仍然按原 payload 算
    assert writer.verify(TASK) is True


async def test_chain_continues_after_new_writer_instance(writer, tmp_path):
    """进程重启：新实例从已落盘的文件接着写，链不断。"""
    await writer.append(TASK, EvidenceKind.task_created, VEC1_PAYLOAD)

    other = FileEvidenceWriter(tmp_path / "evidence")
    second = await other.append(TASK, EvidenceKind.model_call, VEC2_PAYLOAD)

    assert second.seq == 1 and second.prev_hash == VEC1_HASH and second.hash == VEC2_HASH
    assert other.verify(TASK) is True
