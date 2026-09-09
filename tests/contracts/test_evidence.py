"""evidence 哈希链契约测试（aite/contracts/evidence.py）。

逐字节校验 spec §3.1 evidence.py 末尾给出的两组测试向量：
  payload {"a": 1}      canonical '{"a":1}'
  payload_hash          015abd7f...f862
  hash(GENESIS, ·)      cdae94bd...7adc
  payload {"b": "文"}   canonical '{"b":"文"}'
  payload_hash          1e876317...4a10
  hash(上一条 hash, ·)  11dbc980...8f5e

外加 spec §6 T0 行要求的 EvidenceEvent JSON round-trip，以及 EvidenceKind / 字段名
的冻结校验。所有期望值都是照 spec 写死的字面量，不从被测代码反推。
"""

import hashlib
import json
from datetime import UTC, datetime

import pytest
from pydantic import ValidationError

from aite.contracts.evidence import (
    GENESIS,
    EvidenceEvent,
    EvidenceKind,
    canonical_json,
    chain_hash,
    payload_hash_of,
)

# ---- spec §3.1 冻结的测试向量（写死，不允许从代码反推）----
PAYLOAD_A = {"a": 1}
CANONICAL_A = '{"a":1}'
PAYLOAD_HASH_A = "015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
CHAIN_HASH_A = "cdae94bd29b7de0c2a7af392f4d984ed5a41b15bdd872f7b9726132508047adc"

PAYLOAD_B = {"b": "文"}
CANONICAL_B = '{"b":"文"}'
PAYLOAD_HASH_B = "1e8763171f38ca61b0bb0f996142a149ce16ba66c664d341d03c91f54ae4ea10"
CHAIN_HASH_B = "11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e"

CREATED_AT_ISO = "2026-09-09T12:00:00Z"
CREATED_AT = datetime(2026, 9, 9, 12, 0, 0, tzinfo=UTC)

# ---- 两条 event 的**冻结字面量**。刻意不调用被测函数生成，
#      这样下面「重算 hash 与存档值相等」才是真校验，而不是自证。----
FROZEN_EVENT_0: dict[str, object] = {
    "task_id": "t-0001",
    "seq": 0,
    "kind": "task_created",
    "payload_hash": PAYLOAD_HASH_A,
    "payload_ref": None,
    "payload": PAYLOAD_A,
    "prev_hash": GENESIS,
    "hash": CHAIN_HASH_A,
    "created_at": CREATED_AT_ISO,
}
FROZEN_EVENT_1: dict[str, object] = {
    "task_id": "t-0001",
    "seq": 1,
    "kind": "model_call",
    "payload_hash": PAYLOAD_HASH_B,
    "payload_ref": None,
    "payload": PAYLOAD_B,
    "prev_hash": CHAIN_HASH_A,
    "hash": CHAIN_HASH_B,
    "created_at": CREATED_AT_ISO,
}

# spec §3.1 EvidenceKind 的 10 个成员（名字与取值都冻结）
EXPECTED_KINDS: list[tuple[str, str]] = [
    ("task_created", "task_created"),
    ("event_received", "event_received"),
    ("model_call", "model_call"),
    ("checklist_op", "checklist_op"),
    ("tool_call", "tool_call"),
    ("tool_result", "tool_result"),
    ("artifact", "artifact"),
    ("delivered", "delivered"),
    ("failed", "failed"),
    ("cancelled", "cancelled"),
]

# spec §3.1 EvidenceEvent 的字段名与顺序
EXPECTED_FIELDS: list[str] = [
    "task_id",
    "seq",
    "kind",
    "payload_hash",
    "payload_ref",
    "payload",
    "prev_hash",
    "hash",
    "created_at",
]


def _frozen_chain() -> list[EvidenceEvent]:
    """把两条冻结字面量喂给 EvidenceEvent（只做反序列化，不算任何 hash）。"""
    return [EvidenceEvent.model_validate(FROZEN_EVENT_0), EvidenceEvent.model_validate(FROZEN_EVENT_1)]


# =============== canonical_json ===============


def test_genesis_is_64_zeros() -> None:
    """验 GENESIS 常量：64 个 '0' 字符，长度 64。"""
    assert GENESIS == "0" * 64
    assert len(GENESIS) == 64
    assert set(GENESIS) == {"0"}


def test_canonical_json_vector_a_byte_exact() -> None:
    """验向量一的 canonical 串逐字节相等：无空格分隔符、key 排序。"""
    assert canonical_json(PAYLOAD_A) == CANONICAL_A
    # 逐字节：canonical 串按 utf-8 编码后与字面量字节序列一致
    assert canonical_json(PAYLOAD_A).encode("utf-8") == b'{"a":1}'


def test_canonical_json_vector_b_not_ascii_escaped() -> None:
    """验向量二：ensure_ascii=False，中文原样落在串里，不写成 \\u6587。"""
    assert canonical_json(PAYLOAD_B) == CANONICAL_B
    assert "文" in canonical_json(PAYLOAD_B)
    assert "\\u" not in canonical_json(PAYLOAD_B)
    # 逐字节：'文' 的 utf-8 三字节 e6 96 87 直接出现在输出里
    assert canonical_json(PAYLOAD_B).encode("utf-8") == b'{"b":"\xe6\x96\x87"}'


def test_canonical_json_separators_have_no_spaces() -> None:
    """验分隔符是 (",", ":")：多 key 场景不出现 ', ' 或 ': '。"""
    assert canonical_json({"a": 1, "b": 2}) == '{"a":1,"b":2}'
    assert ", " not in canonical_json({"a": 1, "b": 2})
    assert ": " not in canonical_json({"a": 1, "b": 2})


def test_canonical_json_sorts_keys_regardless_of_insertion_order() -> None:
    """验 key 排序性质：字面顺序不同的同一 dict，canonical 串与 hash 都必须相同。"""
    unsorted = {"b": 1, "a": 2}
    sorted_ = {"a": 2, "b": 1}
    assert canonical_json(unsorted) == canonical_json(sorted_) == '{"a":2,"b":1}'
    assert payload_hash_of(unsorted) == payload_hash_of(sorted_)
    assert chain_hash(GENESIS, payload_hash_of(unsorted)) == chain_hash(GENESIS, payload_hash_of(sorted_))


def test_canonical_json_sorts_nested_keys_too() -> None:
    """嵌套 dict 的 key 也必须排序（sort_keys 是递归的），否则同一 payload 会有两个 hash。"""
    assert canonical_json({"z": {"b": 1, "a": 2}}) == '{"z":{"a":2,"b":1}}'
    assert payload_hash_of({"z": {"b": 1, "a": 2}}) == payload_hash_of({"z": {"a": 2, "b": 1}})


# =============== payload_hash_of / chain_hash ===============


def test_payload_hash_of_vectors() -> None:
    """验两组向量的 payload_hash 逐字符相等（sha256 hex，64 位小写）。"""
    assert payload_hash_of(PAYLOAD_A) == PAYLOAD_HASH_A
    assert payload_hash_of(PAYLOAD_B) == PAYLOAD_HASH_B
    assert len(payload_hash_of(PAYLOAD_A)) == 64
    assert len(payload_hash_of(PAYLOAD_B)) == 64


def test_payload_hash_is_sha256_of_canonical_utf8_bytes() -> None:
    """验 payload_hash 的定义：sha256(canonical 串的 utf-8 字节)，两个向量都对得上。"""
    assert hashlib.sha256(b'{"a":1}').hexdigest() == PAYLOAD_HASH_A
    assert hashlib.sha256(b'{"b":"\xe6\x96\x87"}').hexdigest() == PAYLOAD_HASH_B


def test_chain_hash_first_link_from_genesis() -> None:
    """验链首：prev = GENESIS，payload 为向量一时的 hash。"""
    assert chain_hash(GENESIS, PAYLOAD_HASH_A) == CHAIN_HASH_A


def test_chain_hash_second_link_from_previous_hash() -> None:
    """验链上第二条：prev = 上一条的 hash，payload 为向量二时的 hash。"""
    assert chain_hash(CHAIN_HASH_A, PAYLOAD_HASH_B) == CHAIN_HASH_B


def test_chain_hash_is_sha256_of_concatenated_hex() -> None:
    """验 chain_hash 的定义就是 sha256(prev_hash + payload_hash)，输入是 128 字符 hex 串。"""
    expected = hashlib.sha256((GENESIS + PAYLOAD_HASH_A).encode("utf-8")).hexdigest()
    assert expected == CHAIN_HASH_A
    assert len(GENESIS + PAYLOAD_HASH_A) == 128


# =============== EvidenceKind ===============


def test_evidence_kind_members_and_values_frozen() -> None:
    """验 EvidenceKind 的 10 个成员名与取值逐个冻结，且不多不少。"""
    assert [(m.name, m.value) for m in EvidenceKind] == EXPECTED_KINDS


def test_evidence_kind_is_str_enum() -> None:
    """验 StrEnum 语义：成员直接等于自己的字符串，落盘时不会写成 'EvidenceKind.xxx'。"""
    assert EvidenceKind.tool_result == "tool_result"
    assert isinstance(EvidenceKind.tool_result, str)
    assert json.dumps({"kind": EvidenceKind.tool_result}) == '{"kind": "tool_result"}'


# =============== EvidenceEvent（从冻结字面量反序列化）===============


def test_evidence_event_field_names_and_order_frozen() -> None:
    """验字段名与顺序：不多一个、不少一个，落盘 jsonl 的形状才是稳定的。"""
    assert list(EvidenceEvent.model_fields) == EXPECTED_FIELDS


def test_evidence_event_chain_matches_vectors() -> None:
    """验真实链：两条 EvidenceEvent 的 prev_hash / payload_hash / hash 与冻结向量逐字符相等。"""
    e0, e1 = _frozen_chain()

    assert e0.task_id == "t-0001"
    assert e0.seq == 0
    assert e0.kind == EvidenceKind.task_created
    assert e0.prev_hash == GENESIS
    assert e0.payload_hash == PAYLOAD_HASH_A
    assert e0.hash == CHAIN_HASH_A
    assert e0.payload_ref is None
    assert e0.payload == PAYLOAD_A
    assert e0.created_at == CREATED_AT

    assert e1.seq == 1
    assert e1.kind == EvidenceKind.model_call
    assert e1.prev_hash == e0.hash == CHAIN_HASH_A
    assert e1.payload_hash == PAYLOAD_HASH_B
    assert e1.hash == CHAIN_HASH_B


def test_evidence_event_hash_recomputes_from_stored_fields() -> None:
    """验校验流程：拿存档里的 prev_hash / payload_hash 重算 chain_hash，必须等于存档的 hash。

    事件本身是冻结字面量（不是用 chain_hash 算出来的），所以这里是真校验而非自证。
    """
    for ev in _frozen_chain():
        assert chain_hash(ev.prev_hash, ev.payload_hash) == ev.hash


def test_evidence_event_payload_hash_recomputes_from_stored_payload() -> None:
    """验校验流程：拿存档里的内联 payload 重算 payload_hash，必须等于存档的 payload_hash。"""
    for ev in _frozen_chain():
        assert ev.payload is not None
        assert payload_hash_of(ev.payload) == ev.payload_hash


def test_evidence_event_chain_is_linked() -> None:
    """验链式相接：第 n 条的 prev_hash 是第 n-1 条的 hash；链首的 prev 是 GENESIS。"""
    events = _frozen_chain()
    assert events[0].prev_hash == GENESIS
    for prev_ev, cur_ev in zip(events[:-1], events[1:], strict=True):
        assert cur_ev.prev_hash == prev_ev.hash
        assert cur_ev.seq == prev_ev.seq + 1


# =============== JSON round-trip（spec §6 T0 行要求）===============


def test_evidence_event_json_round_trip() -> None:
    """验 JSON round-trip：dump 再 validate，字段值全等（events.jsonl 一行一条的落盘前提）。"""
    for frozen in (FROZEN_EVENT_0, FROZEN_EVENT_1):
        original = EvidenceEvent.model_validate(frozen)
        restored = EvidenceEvent.model_validate_json(original.model_dump_json())
        assert restored == original
        assert restored.model_dump() == original.model_dump()


def test_evidence_event_json_keeps_hex_and_payload_verbatim() -> None:
    """验 dump 出来的 JSON 里三个 hex 串与 payload 原样保留，kind 是纯字符串。"""
    dumped = json.loads(EvidenceEvent.model_validate(FROZEN_EVENT_1).model_dump_json())
    assert dumped["prev_hash"] == CHAIN_HASH_A
    assert dumped["payload_hash"] == PAYLOAD_HASH_B
    assert dumped["hash"] == CHAIN_HASH_B
    assert dumped["payload"] == {"b": "文"}
    assert dumped["kind"] == "model_call"
    assert dumped["payload_ref"] is None
    assert set(dumped) == set(EXPECTED_FIELDS)


def test_evidence_event_created_at_survives_json_round_trip() -> None:
    """验 created_at 带时区往返不丢：UTC 时刻在 round-trip 后仍相等。"""
    original = EvidenceEvent.model_validate(FROZEN_EVENT_0)
    restored = EvidenceEvent.model_validate_json(original.model_dump_json())
    assert restored.created_at == CREATED_AT
    assert restored.created_at.utcoffset() == CREATED_AT.utcoffset()


def test_evidence_event_offloaded_payload_round_trip() -> None:
    """验 payload >64KB 的形态：payload 为 None、payload_ref 指向 payloads/{seq}.json，往返不丢。"""
    offloaded = dict(FROZEN_EVENT_1)
    offloaded["payload"] = None
    offloaded["payload_ref"] = "payloads/1.json"
    ev = EvidenceEvent.model_validate(offloaded)
    assert ev.payload is None
    assert ev.payload_ref == "payloads/1.json"
    # payload 卸载到外部文件后，hash 链仍然只依赖 payload_hash，校验照常成立
    assert chain_hash(ev.prev_hash, ev.payload_hash) == CHAIN_HASH_B
    restored = EvidenceEvent.model_validate_json(ev.model_dump_json())
    assert restored == ev


def test_evidence_event_missing_required_field_raises() -> None:
    """验字段都是必填：少任何一个都必须 ValidationError（不能靠默认值糊过去）。"""
    for field in EXPECTED_FIELDS:
        incomplete = {k: v for k, v in FROZEN_EVENT_0.items() if k != field}
        with pytest.raises(ValidationError):
            EvidenceEvent.model_validate(incomplete)


def test_evidence_event_rejects_unknown_kind() -> None:
    """验 kind 受 EvidenceKind 约束：spec 之外的取值必须报错。"""
    bad = dict(FROZEN_EVENT_0)
    bad["kind"] = "not_a_kind"
    with pytest.raises(ValidationError):
        EvidenceEvent.model_validate(bad)


# =============== 篡改可检测 ===============


def test_tampered_payload_changes_payload_hash_and_chain() -> None:
    """篡改可检测：把 seq 0 的 payload 改一个字，重算的 payload_hash 与存档值不同，链尾也随之变。"""
    e0, e1 = _frozen_chain()

    tampered_payload = {"a": 2}  # 原为 {"a": 1}，改了一个字
    tampered_hash = payload_hash_of(tampered_payload)

    assert canonical_json(tampered_payload) == '{"a":2}'
    # 存档里的 payload_hash 与重算值对不上 —— 这就是校验失败的判据
    assert tampered_hash != e0.payload_hash
    assert tampered_hash != PAYLOAD_HASH_A

    # 篡改传导到链尾：seq 1 的 hash 会随之改变，无法与原 CHAIN_HASH_B 相等
    tampered_link0 = chain_hash(GENESIS, tampered_hash)
    assert tampered_link0 != CHAIN_HASH_A
    tampered_link1 = chain_hash(tampered_link0, e1.payload_hash)
    assert tampered_link1 != CHAIN_HASH_B


def test_tampered_chinese_payload_is_detectable() -> None:
    """篡改可检测（中文向量）：{"b": "文"} 改成 {"b": "汉"}，canonical 与 hash 都必须变。"""
    tampered = {"b": "汉"}
    assert canonical_json(tampered) == '{"b":"汉"}'
    assert canonical_json(tampered) != CANONICAL_B
    assert payload_hash_of(tampered) != PAYLOAD_HASH_B
    assert chain_hash(CHAIN_HASH_A, payload_hash_of(tampered)) != CHAIN_HASH_B


def test_tampered_prev_hash_breaks_the_link() -> None:
    """篡改可检测（改链接）：把 seq 1 的 prev_hash 换成 GENESIS，重算 hash 与存档值对不上。"""
    tampered = dict(FROZEN_EVENT_1)
    tampered["prev_hash"] = GENESIS
    ev = EvidenceEvent.model_validate(tampered)
    assert chain_hash(ev.prev_hash, ev.payload_hash) != ev.hash
    assert chain_hash(GENESIS, PAYLOAD_HASH_B) != CHAIN_HASH_B


def test_deleting_a_middle_event_breaks_the_chain() -> None:
    """删条可检测：把 seq 1 直接挂到 GENESIS 上（伪装 seq 0 被删），hash 与存档值必然不同。"""
    e1 = EvidenceEvent.model_validate(FROZEN_EVENT_1)
    assert chain_hash(GENESIS, e1.payload_hash) != e1.hash
