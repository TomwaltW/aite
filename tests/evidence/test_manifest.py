"""manifest.json 的字段就是 §3.1 写死的那一组，root_hash 取最后一条的 hash。"""
import json

import pytest

from aite.contracts import CONTRACT_VERSION, GENESIS, EvidenceKind
from aite.evidence import FileEvidenceWriter

TASK = "task-manifest"
EXPECTED_FIELDS = {
    "task_id",
    "session_id",
    "task_no",
    "created_by",
    "model",
    "contract_version",
    "root_hash",
    "event_count",
}


@pytest.fixture
def writer(tmp_path) -> FileEvidenceWriter:
    return FileEvidenceWriter(tmp_path / "evidence")


async def test_manifest_shape(writer):
    last = None
    for i in range(3):
        last = await writer.append(TASK, EvidenceKind.model_call, {"step": i})

    root = await writer.finalize(
        TASK,
        {"session_id": "s-1", "task_no": "#A17", "created_by": "ou_1", "model": "scripted-p0"},
    )

    manifest = json.loads(writer.manifest_path(TASK).read_text(encoding="utf-8"))
    assert set(manifest) == EXPECTED_FIELDS          # 不多不少
    assert manifest["task_id"] == TASK
    assert manifest["session_id"] == "s-1"
    assert manifest["task_no"] == "#A17"
    assert manifest["created_by"] == "ou_1"
    assert manifest["model"] == "scripted-p0"
    assert manifest["contract_version"] == CONTRACT_VERSION == "p0.1"
    assert manifest["event_count"] == 3
    assert manifest["root_hash"] == root == last.hash


async def test_finalize_with_no_events(writer):
    assert await writer.finalize(TASK, {}) == GENESIS
    manifest = json.loads(writer.manifest_path(TASK).read_text(encoding="utf-8"))
    assert manifest["event_count"] == 0
    assert set(manifest) == EXPECTED_FIELDS


async def test_directory_layout(writer):
    await writer.append(TASK, EvidenceKind.task_created, {"a": 1})
    await writer.finalize(TASK, {})

    task_dir = writer.task_dir(TASK)
    assert task_dir.name == TASK
    assert sorted(p.name for p in task_dir.iterdir()) == ["events.jsonl", "manifest.json"]
