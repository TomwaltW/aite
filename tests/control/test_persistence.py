"""B6：进程重启后仍能续接。

判据原文：建会话 + 两轮 turn → 用**同一 SQLite 文件**新建第二个 ControlPlane 实例
→ 同线程追问命中同一 session_id，list_turns 含此前两轮。
"""
import asyncio
from datetime import UTC, datetime

import pytest
from control_fakes import FakeClock, FakePlatform, make_config, make_event

from aite.contracts import TaskStatus, Turn, encode_task_no
from aite.control import DuplicateTurnError, InProcessControlPlane, SqliteSessionStore
from aite.evidence import FileEvidenceWriter

CHAT = "oc_chat"
ROOT = "om_1"


def _plane(store, config) -> InProcessControlPlane:
    clock = FakeClock()
    return InProcessControlPlane(
        store=store,
        platform=FakePlatform(),
        evidence=FileEvidenceWriter(config.storage.evidence_dir),
        config=config,
        clock=clock,
        sleep=clock.sleep,
    )


async def _deliver_active(store, chat_id=CHAT):
    """把活跃任务标成 delivered，模拟上一轮已经答完。"""
    for t in await store.list_active_tasks(chat_id):
        t.status = TaskStatus.delivered
        await store.update_task(t)


async def test_b6_restart_resumes_same_session_and_turns(tmp_path):
    config = make_config(tmp_path)
    db = config.storage.sqlite_path

    # —— 第一个实例：建会话 + 两轮 turn ——
    store1 = SqliteSessionStore(db)
    await store1.init()
    plane1 = _plane(store1, config)
    await plane1.handle_event(make_event(event_id="ev1", text="第一问", message_id=ROOT))
    session_id = (await store1.list_active_tasks(CHAT))[0].session_id
    await _deliver_active(store1)
    await plane1.handle_event(
        make_event(event_id="ev2", text="第二问", mentioned=False, message_id="om_2", thread_id=ROOT)
    )
    first_task_no = (await store1.list_active_tasks(CHAT))[0].task_no
    assert [t.content for t in await store1.list_turns(session_id)] == ["第一问", "第二问"]
    await _deliver_active(store1)
    await store1.close()

    # —— 换一个实例，同一个文件 ——
    store2 = SqliteSessionStore(db)
    await store2.init()
    plane2 = _plane(store2, config)
    await plane2.handle_event(
        make_event(event_id="ev3", text="第三问", mentioned=False, message_id="om_3", thread_id=ROOT)
    )

    hit = await store2.find_session_by_thread(CHAT, ROOT)
    assert hit is not None and hit.id == session_id            # 命中同一 session

    turns = await store2.list_turns(session_id)
    assert [t.content for t in turns] == ["第一问", "第二问", "第三问"]   # 含此前两轮
    assert [t.seq for t in turns] == [0, 1, 2]

    new_task = (await store2.list_active_tasks(CHAT))[0]
    assert new_task.session_id == session_id
    assert new_task.task_no != first_task_no                   # 任务号计数器也是持久的
    await store2.close()


async def test_seen_event_survives_restart(tmp_path):
    """R2 的去重键落在库里：换实例后重推同一 event 仍然被丢。"""
    config = make_config(tmp_path)
    store1 = SqliteSessionStore(config.storage.sqlite_path)
    await store1.init()
    await _plane(store1, config).handle_event(make_event(event_id="ev-dup", text="活儿"))
    await store1.close()

    store2 = SqliteSessionStore(config.storage.sqlite_path)
    await store2.init()
    plane2 = _plane(store2, config)
    await plane2.handle_event(make_event(event_id="ev-dup", text="活儿"))

    assert plane2.counters["events.duplicate"] == 1
    assert len(await store2.list_active_tasks(CHAT)) == 1
    await store2.close()


async def test_task_no_is_atomic_per_tenant(store):
    """next_task_no 必须原子递增：32 个并发调用拿到 1..32，一个不重不漏。"""
    nos = await asyncio.gather(*[store.next_task_no("default") for _ in range(32)])
    assert set(nos) == {encode_task_no(i) for i in range(1, 33)}
    assert await store.next_task_no("other-tenant") == "#A1"   # 计数器按租户隔离


async def test_duplicate_turn_seq_raises(store):
    t = Turn(
        session_id="s1", seq=0, role="user", platform_user_id="ou_1",
        content="hi", created_at=datetime.now(UTC),
    )
    await store.append_turn(t)
    with pytest.raises(DuplicateTurnError):
        await store.append_turn(t)


async def test_list_active_tasks_scope(store, plane):
    """口径：status in (created, planning, working)，且按 chat 隔离。"""
    await plane.handle_event(make_event(event_id="e1", text="甲", message_id="om_a", chat_id=CHAT))
    await plane.handle_event(
        make_event(event_id="e2", text="乙", message_id="om_b", chat_id="oc_other")
    )
    assert [t.title for t in await store.list_active_tasks(CHAT)] == ["甲"]

    task = (await store.list_active_tasks(CHAT))[0]
    for done in (TaskStatus.delivered, TaskStatus.failed, TaskStatus.cancelled, TaskStatus.answering):
        task.status = done
        await store.update_task(task)
        assert await store.list_active_tasks(CHAT) == []
    for live in (TaskStatus.created, TaskStatus.planning, TaskStatus.working):
        task.status = live
        await store.update_task(task)
        assert len(await store.list_active_tasks(CHAT)) == 1


async def test_next_turn_seq(store):
    assert await store.next_turn_seq("s9") == 0
    await store.append_turn(
        Turn(session_id="s9", seq=0, role="user", platform_user_id=None, content="x",
             created_at=datetime.now(UTC))
    )
    assert await store.next_turn_seq("s9") == 1
