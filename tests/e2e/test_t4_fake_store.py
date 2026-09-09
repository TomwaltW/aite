"""FakeSessionStore / FakeEvidenceWriter 自测（dev-spec §3.2）。"""
from __future__ import annotations

import inspect
from datetime import UTC, datetime

import pytest

from aite.contracts import (
    GENESIS,
    Anchor,
    EvidenceKind,
    Session,
    SessionKind,
    Task,
    TaskStatus,
    Turn,
    chain_hash,
    payload_hash_of,
)
from aite.contracts.ports import EvidenceWriter, SessionStore
from aite.testing import FakeEvidenceWriter, FakeSessionStore

NOW = datetime(2026, 9, 9, 9, 0, tzinfo=UTC)


def anchor(message_id="om_1", thread_id=None):
    return Anchor(platform="fake", chat_id="oc_1", message_id=message_id, thread_id=thread_id)


def session(sid="s1", thread_id="om_1", chat_id="oc_1"):
    return Session(
        id=sid,
        tenant_id="default",
        workspace_id="cli_app",
        chat_id=chat_id,
        kind=SessionKind.task,
        anchor=anchor(thread_id=thread_id),
        created_by="ou_alice",
        created_at=NOW,
        last_active_at=NOW,
    )


def task(tid="t1", sid="s1", status=TaskStatus.created, no="#A1"):
    return Task(
        id=tid,
        session_id=sid,
        task_no=no,
        status=status,
        session_token="tok",
        created_by="ou_alice",
        created_at=NOW,
        updated_at=NOW,
    )


def test_implements_every_store_and_evidence_method():
    for proto, impl in ((SessionStore, FakeSessionStore), (EvidenceWriter, FakeEvidenceWriter)):
        for name, obj in vars(proto).items():
            if name.startswith("_") or not inspect.isfunction(obj):
                continue
            fn = getattr(impl, name, None)
            assert fn is not None, f"{impl.__name__} 缺 {name}"
            assert inspect.iscoroutinefunction(fn) == inspect.iscoroutinefunction(obj), (
                f"{impl.__name__}.{name} 的异步形态与契约不一致"
            )


async def test_init_is_idempotent():
    s = FakeSessionStore()
    await s.init()
    await s.init()
    assert s.initialized is True


async def test_find_session_by_thread():
    s = FakeSessionStore()
    await s.create_session(session(thread_id="om_1"))
    assert (await s.find_session_by_thread("oc_1", "om_1")).id == "s1"
    assert await s.find_session_by_thread("oc_1", "om_other") is None
    assert await s.find_session_by_thread("oc_other", "om_1") is None


async def test_append_turn_rejects_duplicate_seq():
    """§3.2：重复的 (session_id, seq) 报错。"""
    s = FakeSessionStore()
    await s.create_session(session())
    t = Turn(session_id="s1", seq=0, role="user", platform_user_id="ou_alice",
             content="你好", created_at=NOW)
    await s.append_turn(t)
    with pytest.raises(ValueError, match="重复的"):
        await s.append_turn(t)


async def test_list_turns_is_seq_ordered_and_limited():
    s = FakeSessionStore()
    await s.create_session(session())
    for i in range(5):
        await s.append_turn(
            Turn(session_id="s1", seq=i, role="user", platform_user_id="u", content=str(i),
                 created_at=NOW)
        )
    assert [t.content for t in await s.list_turns("s1", limit=2)] == ["3", "4"]


async def test_next_task_no_uses_contract_encoding_and_is_per_tenant():
    """向量来自契约：1→#A1、17→#AH、32→#A10。"""
    s = FakeSessionStore()
    got = [await s.next_task_no("default") for _ in range(17)]
    assert got[0] == "#A1" and got[16] == "#AH"
    assert await s.next_task_no("other") == "#A1"


async def test_seen_event_is_false_once_then_true():
    s = FakeSessionStore()
    assert await s.seen_event("e1") is False
    assert await s.seen_event("e1") is True


async def test_list_active_tasks_only_returns_active_of_this_chat():
    s = FakeSessionStore()
    await s.create_session(session("s1", chat_id="oc_1"))
    await s.create_session(session("s2", thread_id="om_2", chat_id="oc_2"))
    await s.create_task(task("t_created", "s1", TaskStatus.created))
    await s.create_task(task("t_working", "s1", TaskStatus.working))
    await s.create_task(task("t_done", "s1", TaskStatus.delivered))
    await s.create_task(task("t_other_chat", "s2", TaskStatus.working))
    assert {t.id for t in await s.list_active_tasks("oc_1")} == {"t_created", "t_working"}


async def test_stored_models_are_copies():
    """存进去之后在外面改对象，不该改到库里那份。"""
    s = FakeSessionStore()
    t = task()
    await s.create_task(t)
    t.status = TaskStatus.failed
    assert (await s.get_task("t1")).status is TaskStatus.created


async def test_evidence_chain_matches_contract_hashing():
    w = FakeEvidenceWriter()
    e0 = await w.append("t1", EvidenceKind.task_created, {"a": 1})
    e1 = await w.append("t1", EvidenceKind.delivered, {"b": "文"})

    assert e0.prev_hash == GENESIS and e0.seq == 0
    assert e0.payload_hash == payload_hash_of({"a": 1})
    assert e0.hash == chain_hash(GENESIS, e0.payload_hash)
    assert e1.prev_hash == e0.hash and e1.seq == 1
    assert w.verify("t1") is True


async def test_tampering_breaks_verify():
    w = FakeEvidenceWriter()
    await w.append("t1", EvidenceKind.task_created, {"a": 1})
    w.chains["t1"][0].payload["a"] = 2
    assert w.verify("t1") is False


async def test_finalize_returns_last_hash():
    w = FakeEvidenceWriter()
    await w.append("t1", EvidenceKind.task_created, {"a": 1})
    last = await w.append("t1", EvidenceKind.delivered, {"b": 2})
    root = await w.finalize("t1", {"task_no": "#A1"})
    assert root == last.hash
    assert w.manifests["t1"]["event_count"] == 2
    assert w.manifests["t1"]["task_no"] == "#A1"
