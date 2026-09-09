# aite/contracts/evidence.py
from datetime import datetime
from enum import StrEnum
from typing import Any

from pydantic import BaseModel


class EvidenceKind(StrEnum):
    task_created = "task_created"
    event_received = "event_received"
    model_call = "model_call"        # payload: 模型名、messages hash、usage、finish_reason（不存全文）
    checklist_op = "checklist_op"
    tool_call = "tool_call"          # payload: name、arguments
    tool_result = "tool_result"      # payload: ok、error、content_hash、duration_ms
    artifact = "artifact"            # payload: title、mime、sha256、size
    delivered = "delivered"
    failed = "failed"
    cancelled = "cancelled"

class EvidenceEvent(BaseModel):
    task_id: str
    seq: int                         # 从 0 开始
    kind: EvidenceKind
    payload_hash: str                # sha256(canonical_json(payload)) hex
    payload_ref: str | None          # payload 内联时 None；>64KB 时为 payloads/{seq}.json 相对路径
    payload: dict[str, Any] | None   # 内联 payload
    prev_hash: str                   # 第 0 条为 GENESIS
    hash: str                        # sha256(prev_hash + payload_hash) hex
    created_at: datetime

GENESIS = "0" * 64

def canonical_json(payload: dict[str, Any]) -> str:
    import json
    return json.dumps(payload, sort_keys=True, ensure_ascii=False, separators=(",", ":"))

def chain_hash(prev_hash: str, payload_hash: str) -> str:
    import hashlib
    return hashlib.sha256((prev_hash + payload_hash).encode("utf-8")).hexdigest()

def payload_hash_of(payload: dict[str, Any]) -> str:
    import hashlib
    return hashlib.sha256(canonical_json(payload).encode("utf-8")).hexdigest()

# 测试向量（tests/contracts 必须逐字节校验）：
#   payload {"a": 1}      canonical '{"a":1}'
#   payload_hash          015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862
#   hash(GENESIS, ·)      cdae94bd29b7de0c2a7af392f4d984ed5a41b15bdd872f7b9726132508047adc
#   payload {"b": "文"}   canonical '{"b":"文"}'
#   payload_hash          1e8763171f38ca61b0bb0f996142a149ce16ba66c664d341d03c91f54ae4ea10
#   hash(上一条 hash, ·)  11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e
# 落盘：{evidence_dir}/{task_id}/events.jsonl（一行一个 EvidenceEvent）+ manifest.json
#   manifest = {task_id, session_id, task_no, created_by, model, contract_version, root_hash(最后一条 hash), event_count}
