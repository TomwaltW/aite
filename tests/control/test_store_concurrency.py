"""T18：§3.2 那三条带并发含义的 SessionStore 约定，压在真并发下。

已有的 `test_persistence.py` 验的全是顺序场景（写完、关掉、重开、读到），
只有 `test_task_no_is_atomic_per_tenant` 摸到一点并发。这里把三条约定各压一遍：

    append_turn   「seq 由调用方分配，重复 (session_id, seq) 报错」
    next_task_no  「原子递增 + encode_task_no」
    seen_event    「首次调用记录并返回 False，之后 True」

P0 是单 worker，但**事件投递不是单路的**：`on_event` 必须 1s 内返回（§3.3 第一条），
平台重连后会一次重推一批，`handle_event` 之间没有串行化保证。

除了同一实例内的并发，这里还压**两个 store 实例指向同一个 .db 文件** ——
`SqliteSessionStore` 的 `asyncio.Lock` 是每实例一个，跨实例完全不互斥，
只剩 SQLite 自己的文件锁兜底。这正是 `systemctl restart` 时新旧进程叠在一起的形状。
"""
import asyncio
from datetime import UTC, datetime

import pytest
from control_fakes import make_event

from aite.contracts import TaskStatus, Turn, encode_task_no
from aite.control import DuplicateTurnError, SqliteSessionStore
from aite.control.store import ORPHAN_RESULT_SUMMARY

CHAT = "oc_chat"


def make_turn(session_id: str, seq: int) -> Turn:
    return Turn(
        session_id=session_id, seq=seq, role="user", platform_user_id="ou_1",
        content=f"第 {seq} 问", created_at=datetime.now(UTC),
    )


@pytest.fixture
async def second_store(config):
    """指向同一个 .db 文件的第二个 store 实例（锁与第一个互不相识）。"""
    s = SqliteSessionStore(config.storage.sqlite_path)
    await s.init()
    yield s
    await s.close()


# ---- 1. append_turn 同 seq ------------------------------------------------

async def test_concurrent_same_seq_lets_exactly_one_win(store):
    """并发写同一 (session_id, seq)：必须恰好一个赢，其余全部报错。

    静默写重的话，`list_turns` 会把同一轮喂给模型两次；静默覆盖的话，
    先到的那轮正文就没了 —— 两种都是「重复报错」这条约定要挡的。
    """
    results = await asyncio.gather(
        *[store.append_turn(make_turn("s1", 0)) for _ in range(16)],
        return_exceptions=True,
    )
    winners = [r for r in results if r is None]
    losers = [r for r in results if isinstance(r, DuplicateTurnError)]
    assert len(winners) == 1, f"应当恰好一个成功，实际 {len(winners)}"
    assert len(losers) == 15, f"其余都该报 DuplicateTurnError，实际 {results}"

    turns = await store.list_turns("s1")
    assert len(turns) == 1 and turns[0].seq == 0      # 库里只留下一条，没写重


async def test_concurrent_distinct_seq_loses_nothing(store):
    """并发写**不同** seq：一条都不许丢，顺序也要对。"""
    n = 64
    errs = [
        r for r in await asyncio.gather(
            *[store.append_turn(make_turn("s2", i)) for i in range(n)],
            return_exceptions=True,
        )
        if r is not None
    ]
    assert errs == []

    turns = await store.list_turns("s2", limit=n)
    assert [t.seq for t in turns] == list(range(n))


async def test_cross_instance_same_seq_still_raises(store, second_store):
    """两个 store 实例（各自一把锁）撞同一个 seq —— 只剩 SQLite 的主键约束兜底。"""
    results = await asyncio.gather(
        store.append_turn(make_turn("s3", 0)),
        second_store.append_turn(make_turn("s3", 0)),
        return_exceptions=True,
    )
    assert sum(1 for r in results if r is None) == 1
    assert sum(1 for r in results if isinstance(r, DuplicateTurnError)) == 1
    assert len(await store.list_turns("s3")) == 1


async def test_duplicate_turn_error_does_not_eat_neighbours(store):
    """报错那一路的 rollback 不许把并发的别人写的数据回滚掉。

    `append_turn` 撞主键时会 `rollback()`，而 rollback 回滚的是**整条连接**的当前事务。
    真把边上的写卷进去的话，丢的是别的会话的对话轮 —— 库里静悄悄少一行，没有任何报错。
    """
    await store.append_turn(make_turn("s4", 0))
    results = await asyncio.gather(
        *([store.append_turn(make_turn("s4", 0)) for _ in range(8)]          # 全部撞车
          + [store.append_turn(make_turn("s5", i)) for i in range(8)]),      # 无辜的邻居
        return_exceptions=True,
    )
    assert sum(1 for r in results if isinstance(r, DuplicateTurnError)) == 8
    assert [t.seq for t in await store.list_turns("s5")] == list(range(8))   # 邻居一条不少


# ---- 2. next_task_no 原子性 ----------------------------------------------

async def test_concurrent_next_task_no_never_collides(store):
    """并发要 N 个号 → N 个互不相同的号，形状都合 encode_task_no。

    撞号不是小事：`!stop #A3` 是人在群里按号停任务的，两个任务同号就会停错那个。
    """
    n = 200
    nos = await asyncio.gather(*[store.next_task_no("default") for _ in range(n)])
    assert len(set(nos)) == n, f"撞号了：{n - len(set(nos))} 个重复"
    assert set(nos) == {encode_task_no(i) for i in range(1, n + 1)}   # 恰好 1..N，不跳号


async def test_cross_instance_next_task_no_never_collides(store, second_store):
    """两个 store 实例同时发号（重启窗口里新旧进程叠在一起的形状）。"""
    n = 50
    nos = await asyncio.gather(
        *([store.next_task_no("default") for _ in range(n)]
          + [second_store.next_task_no("default") for _ in range(n)])
    )
    assert set(nos) == {encode_task_no(i) for i in range(1, 2 * n + 1)}


async def test_concurrent_task_no_stays_isolated_per_tenant(store):
    """并发压力下租户之间也不许串号。"""
    calls = [store.next_task_no("t-a") for _ in range(32)]
    calls += [store.next_task_no("t-b") for _ in range(32)]
    nos = await asyncio.gather(*calls)
    expected = {encode_task_no(i) for i in range(1, 33)}
    assert sorted(nos) == sorted(list(expected) * 2)   # 两个租户各自跑完 1..32


# ---- 3. seen_event 并发去重 ----------------------------------------------

async def test_concurrent_seen_event_returns_false_exactly_once(store):
    """同一 event_id 并发问 N 次 → 只有一次 False。

    两个协程都拿到 False 就会建两个 task：同一个问题在群里被干两遍、回两次帖。
    R2 与场景 10_duplicate_event 靠的就是这条，但那条测的是顺序投两次。
    """
    n = 64
    answers = await asyncio.gather(*[store.seen_event("ev-storm") for _ in range(n)])
    assert answers.count(False) == 1, f"有 {answers.count(False)} 路都以为自己是第一个"
    assert answers.count(True) == n - 1


async def test_cross_instance_seen_event_returns_false_exactly_once(store, second_store):
    """跨实例的同一 event_id：仍然只许一路拿到 False。"""
    answers = await asyncio.gather(
        *([store.seen_event("ev-x") for _ in range(8)]
          + [second_store.seen_event("ev-x") for _ in range(8)])
    )
    assert answers.count(False) == 1


async def test_concurrent_distinct_events_all_pass_once(store):
    """不同 event_id 并发：每个都该拿到一次 False，不许被误判成重复。"""
    ids = [f"ev-{i}" for i in range(64)]
    assert await asyncio.gather(*[store.seen_event(i) for i in ids]) == [False] * 64
    assert await asyncio.gather(*[store.seen_event(i) for i in ids]) == [True] * 64


# ---- recover_orphan_tasks（崩溃收尾用，见 test_t18_crash_recovery.py）----

async def test_recover_orphan_tasks_clears_the_active_three(store, plane):
    """created/planning/working 三个状态都要被收走，收完 list_active_tasks 清零。"""
    await plane.handle_event(make_event(event_id="e1", text="活儿", message_id="om_a"))
    task = (await store.list_active_tasks(CHAT))[0]

    for live in (TaskStatus.created, TaskStatus.planning, TaskStatus.working):
        task.status = live
        await store.update_task(task)

        orphans = await store.recover_orphan_tasks()
        assert [t.id for t in orphans] == [task.id]
        assert orphans[0].status is TaskStatus.failed
        assert orphans[0].result_summary == ORPHAN_RESULT_SUMMARY
        assert await store.list_active_tasks(CHAT) == []          # !status 上不再挂着

        reread = await store.get_task(task.id)
        assert reread.status is TaskStatus.failed                 # 落盘了，不只是内存里
        assert reread.result_summary == ORPHAN_RESULT_SUMMARY


async def test_recover_orphan_tasks_leaves_finished_tasks_alone(store, plane):
    """已经收尾的任务不许被动：delivered 的答案摘要不能被覆写成「已终止」。"""
    await plane.handle_event(make_event(event_id="e1", text="活儿", message_id="om_a"))
    task = (await store.list_active_tasks(CHAT))[0]

    for done in (TaskStatus.delivered, TaskStatus.failed, TaskStatus.cancelled,
                 TaskStatus.answering):
        task.status = done
        task.result_summary = "原来的摘要"
        await store.update_task(task)

        assert await store.recover_orphan_tasks() == []
        reread = await store.get_task(task.id)
        assert reread.status is done and reread.result_summary == "原来的摘要"


async def test_recover_orphan_tasks_is_idempotent_and_empty_safe(store):
    """空库上调、连着调两次，都不许炸 —— 起飞路径上的东西不能挑时候。"""
    assert await store.recover_orphan_tasks() == []
    assert await store.recover_orphan_tasks() == []
