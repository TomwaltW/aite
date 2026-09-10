"""T18：崩溃恢复（§2.4 M6 的 `kill -9` 那一半）。

`test_t8_graceful_shutdown.py` 验的是**优雅**停机 —— platform 先停、任务收尾、
store 最后关。M6 说的是 `systemctl restart` / 杀进程：没有收尾、没有 `store.close()`、
写到一半的事务、可能还有半行 JSON 落在 `events.jsonl` 上。这一整轨没人验过。

**怎么模拟崩溃**：不真 `kill -9`（测试里不好控），而是「不走收尾路径」——
`build_app` + `store.init()` 把状态造出来，然后**一步收尾都不走**：
不 `platform.stop()`、不 `plane.join()`、不 finalize evidence，只把 fd 关掉
（等价于 OS 回收进程的 fd），再在同一个 .db 文件上重开一套组装。
`run_app` 的退出序列是被跳过的那部分 —— 那正是崩溃时不会发生的事。
"""
import asyncio
import json
import os
import sqlite3
import stat
from pathlib import Path

import pytest
from app_under_test import build_app
from integration_fakes import (
    CHAT,
    GatedPlatform,
    RecordingModel,
    final_step,
    make_event,
    running_app,
    settle,
    wait_until,
)

from aite.contracts import EvidenceKind, TaskStatus
from aite.control import SqliteSessionStore
from aite.control.store import ORPHAN_RESULT_SUMMARY
from aite.evidence import FileEvidenceWriter
from aite.testing import FakeSandbox

ROOT = "om_1"


def make_app(config, *, reply: str = "干完了。"):
    return build_app(
        config,
        platform=GatedPlatform(),
        model=RecordingModel([final_step(reply)]),
        sandbox=FakeSandbox(),
    )


async def crash_with_a_task_in_flight(config) -> tuple[str, str]:
    """造一个「正在跑」的任务，然后崩溃。返回 (task_id, task_no)。

    收尾一步都不走：platform 没 stop、任务没收尾、evidence 没 finalize。
    """
    app = make_app(config)
    await app.store.init()                       # run_app 起飞时做的那一步
    await app.plane.handle_event(make_event(event_id="e1", text="画个图", message_id=ROOT))

    task = (await app.store.list_active_tasks(CHAT))[0]
    task.status = TaskStatus.working             # 崩溃那一刻它正在干活
    await app.store.update_task(task)

    await app.store.close()                      # 只关 fd，等价 OS 回收；收尾序列全跳过
    return task.id, task.task_no


# --------------------------------------------------------------------------
# 僵尸任务
# --------------------------------------------------------------------------

async def test_crash_leaves_a_zombie_task_hanging_on_status(config):
    """**钉住现状（这是个真问题）**：崩溃时在跑的任务，重开后还是 `working`。

    `list_active_tasks` 认 created/planning/working（§3.2），起飞路径上又没有人收拾，
    于是这个永远跑不动的任务会一直挂在群里的 `!status` 上。M6 之后就是这个样子。
    """
    task_id, task_no = await crash_with_a_task_in_flight(config)

    app2 = make_app(config)
    await app2.store.init()
    try:
        still_there = await app2.store.list_active_tasks(CHAT)
        assert [t.id for t in still_there] == [task_id]
        assert still_there[0].status is TaskStatus.working      # 没人动过它

        # 群里 `!status` 真的会把它列出来 —— 这才是用户看得见的那一面
        await app2.plane.handle_event(
            make_event(event_id="e2", text="!status", message_id="om_s")
        )
        assert task_no in app2.platform.texts()[-1]
    finally:
        await app2.store.close()


async def test_recover_orphan_tasks_clears_the_zombie_after_crash(config):
    """收拾办法：起飞时调 `recover_orphan_tasks()` —— 之后 `!status` 就干净了。

    它把任务**交回给调用方**而不是自己闷头改完，是因为 §3.3 还要求回帖 +
    evidence `failed` 事件，那两件 store 做不了（接线建议见 T18 回执）。
    """
    task_id, task_no = await crash_with_a_task_in_flight(config)

    app2 = make_app(config)
    await app2.store.init()
    try:
        orphans = await app2.store.recover_orphan_tasks()
        assert [t.id for t in orphans] == [task_id]
        assert orphans[0].status is TaskStatus.failed
        assert orphans[0].task_no == task_no                 # 回帖要用它，得带得出来
        assert orphans[0].result_summary == ORPHAN_RESULT_SUMMARY

        assert await app2.store.list_active_tasks(CHAT) == []

        await app2.plane.handle_event(
            make_event(event_id="e2", text="!status", message_id="om_s")
        )
        assert task_no not in app2.platform.texts()[-1]   # 不再挂在群里
    finally:
        await app2.store.close()


async def test_crash_does_not_block_new_work_in_the_same_thread(config):
    """M6 正题：杀进程重启后**在旧线程追问，仍能续接**。

    收拾掉僵尸只是把摊子收干净，用户要的是接着问下去 —— 命中同一会话、
    模型上下文里带着崩溃前那一轮、任务号接着往下发。
    """
    old_id, old_no = await crash_with_a_task_in_flight(config)

    app2 = make_app(config, reply="这次答上了。")
    async with running_app(app2):
        # 起飞时 run_app 已经替你收过一遍了（T22，aite/app.py 的 _recover_orphans），
        # 所以这里再问就该是干净的 —— 僵尸已经落到 failed 上。
        assert await app2.store.recover_orphan_tasks() == []
        assert (await app2.store.get_task(old_id)).status is TaskStatus.failed

        await app2.platform.emit(
            make_event(event_id="e9", text="接着上面那个问题", mentioned=False,
                       message_id="om_9", thread_id=ROOT)
        )
        # handle_event 在 emit 里同步走完，任务当场就在库里
        new_task = (await app2.store.list_active_tasks(CHAT))[0]
        assert new_task.task_no == "#A2" != old_no          # 新号，不复用僵尸那个

        # 认那句新回复，别数 send_text 的条数：T22 之后起飞会先给僵尸回一帖
        # （「已终止，请重新发起」），计数里早就有它了，数条数会在新任务开跑前就满足。
        await wait_until(lambda: "这次答上了。" in app2.platform.texts(), what="新任务交付")

        old_task = await app2.store.get_task(old_id)
        assert new_task.session_id == old_task.session_id   # 同一个会话，话题锚点没断
        turns = await app2.store.list_turns(new_task.session_id)
        assert [t.content for t in turns] == ["画个图", "接着上面那个问题"]
        # 关键的一条：模型真的看见了崩溃前那一轮，不只是库里躺着一行
        assert any("画个图" in text for text in app2.model.prompt_texts())


# --------------------------------------------------------------------------
# 半行 evidence
# --------------------------------------------------------------------------

def tear_last_line(writer: FileEvidenceWriter, task_id: str) -> None:
    """往 events.jsonl 末尾写半行 JSON —— 进程在 write 中途没的那一刻的样子。"""
    with writer.events_path(task_id).open("a", encoding="utf-8") as fh:
        fh.write('{"task_id": "' + task_id + '", "seq": 2, "kind": "model_ca')


async def test_verify_rejects_a_torn_last_line(tmp_path):
    """残行必须让 `verify()` 报 False。现成测试验过「完整篡改」，没验过「写了一半」。"""
    writer = FileEvidenceWriter(tmp_path / "evidence")
    await writer.append("t-torn", EvidenceKind.task_created, {"a": 1})
    await writer.append("t-torn", EvidenceKind.model_call, {"b": 2})
    assert writer.verify("t-torn") is True

    tear_last_line(writer, "t-torn")
    assert writer.verify("t-torn") is False
    assert FileEvidenceWriter(tmp_path / "evidence").verify("t-torn") is False   # 换实例也一样


async def test_append_after_a_torn_line_recovers_on_a_fresh_writer(tmp_path):
    """崩溃留下残行后，重启的新进程能接着写 —— §3.3 的收尾链不再自锁。

    T18 时这里是个真 bug：`verify()` 有 try/except 挡住坏行，`_read_events()` 没有，
    于是 `append()` / `finalize()` 直接抛 pydantic `ValidationError`。按 §3.3 本该
    「task failed + 回帖 + evidence failed 事件」，可**写那条 failed 事件本身就是
    炸的那个操作**，越出事越写不进去。T21 在写入侧加了 `_heal_torn_tail`：
    `append` / `finalize` 进门先把没写完的末段收拾掉，再接着往下写。
    """
    root = tmp_path / "evidence"
    writer = FileEvidenceWriter(root)
    await writer.append("t-cold", EvidenceKind.task_created, {"a": 1})
    tear_last_line(writer, "t-cold")

    fresh = FileEvidenceWriter(root)                  # 重启后的新进程：_tip 缓存是空的
    failed = await fresh.append("t-cold", EvidenceKind.failed, {"why": "crash"})
    assert failed.seq == 1                            # 残行没写完，不占号
    assert fresh.counters["evidence.torn_tail_dropped"] == 1   # 收拾这件事留了痕
    assert fresh.verify("t-cold") is True             # 链是通的，不是「不炸但坏着」

    # finalize 走的是同一条收拾路径（它也可能是崩溃后第一个被调到的）
    other = FileEvidenceWriter(root)
    assert await other.finalize("t-cold", {}) == failed.hash
    manifest = json.loads(other.manifest_path("t-cold").read_text(encoding="utf-8"))
    assert manifest["event_count"] == 2               # 那半行从来不是一条事件


async def test_append_after_a_torn_line_keeps_the_chain_on_a_hot_writer(tmp_path):
    """`_tip` 还热时也不再坏链 —— 磁盘满那一路：`write` 只落一半，进程还活着。

    T18 时这里是个真 bug：这条路不读文件所以不报错，两条 JSON 挤在同一行，
    `verify()` 从此永远 False 而写的人什么都不知道。T21 的挡法不依赖 `_tip`
    是冷是热 —— `append` 每次都真去看文件末尾那一个字节。
    """
    writer = FileEvidenceWriter(tmp_path / "evidence")
    await writer.append("t-hot", EvidenceKind.task_created, {"a": 1})
    tear_last_line(writer, "t-hot")

    second = await writer.append("t-hot", EvidenceKind.model_call, {"b": 2})
    assert second.seq == 1
    assert writer.counters["evidence.torn_tail_dropped"] == 1
    assert writer.verify("t-hot") is True

    lines = writer.events_path("t-hot").read_text(encoding="utf-8").splitlines()
    assert len(lines) == 2
    assert all(line.count('"task_id"') == 1 for line in lines)     # 一行一条，不再挤


# --------------------------------------------------------------------------
# journal 模式 / 崩溃后的文件残留
# --------------------------------------------------------------------------

async def test_journal_mode_is_delete_and_crash_leaves_no_residue(config):
    """P0 的 journal 模式是 `delete`（不是 WAL）：崩溃后不留 `-wal` / `-journal`。

    delete 模式下 journal 只在事务进行中存在，commit 完就删。`store.py` 也没设过
    `PRAGMA journal_mode`，所以走的是 SQLite 默认值 —— 这条钉住它，
    哪天有人改成 WAL，部署面要跟着管两个新文件（见回执）。
    """
    db = Path(config.storage.sqlite_path)

    store = SqliteSessionStore(db)
    await store.init()
    async with store._conn.execute("PRAGMA journal_mode") as cur:
        assert (await cur.fetchone())[0] == "delete"
    await store.close()

    await crash_with_a_task_in_flight(config)

    residue = [p.name for p in db.parent.iterdir() if p.name.startswith(db.name + "-")]
    assert residue == [], f"崩溃后有残留：{residue}"


async def test_data_written_before_the_crash_survives(config):
    """崩溃前 commit 过的东西一个字都不许丢，计数器也接着往下走。"""
    task_id, task_no = await crash_with_a_task_in_flight(config)
    assert task_no == "#A1"

    store = SqliteSessionStore(config.storage.sqlite_path)
    await store.init()
    try:
        task = await store.get_task(task_id)
        assert task is not None and task.status is TaskStatus.working

        session = await store.find_session_by_thread(CHAT, ROOT)
        assert session is not None                                   # 话题锚点还在
        assert [t.content for t in await store.list_turns(task.session_id)] == ["画个图"]
        assert await store.next_task_no("default") == "#A2"          # 号接着发，不回退
    finally:
        await store.close()

    # 独立连接（完全另一条 fd）也读得到同样的东西 —— 不是缓存里的幻觉
    raw = sqlite3.connect(config.storage.sqlite_path)
    try:
        assert raw.execute("SELECT COUNT(*) FROM turns").fetchone()[0] == 1
    finally:
        raw.close()


# --------------------------------------------------------------------------
# 存储层失败 → 进程不退出（§3.3 最后一行）
# --------------------------------------------------------------------------

async def test_store_failure_does_not_kill_the_process(config):
    """db 目录只读（写不出 journal）时投事件：进程必须还活着。

    §3.3 最后一行：「任何未捕获异常 → task failed + 回帖 + evidence failed；进程不退出」。
    这里成立的是「进程不退出」那半 —— `Ingress.on_event` 把异常兜住并计 `ingress.errors`。
    另一半不成立：异常发生在 `seen_event`，此刻连 task 都还没建，
    所以没有 task 可以标 failed、也没有帖可回，事件被静默丢掉（见回执）。
    """
    db = Path(config.storage.sqlite_path)
    app = make_app(config)

    async with running_app(app) as handle:
        await app.platform.emit(make_event(event_id="ok-1", text="正常的活"))
        await wait_until(lambda: app.platform.count("send_text") == 1, what="第一个任务交付")

        os.chmod(db.parent, stat.S_IRUSR | stat.S_IXUSR)      # 只读目录：建不出 -journal
        try:
            before = app.ingress.counters["ingress.errors"]
            await app.platform.emit(make_event(event_id="boom", text="这条会撞上只读磁盘",
                                               message_id="om_b"))
            await settle()

            assert app.ingress.counters["ingress.errors"] == before + 1   # 异常被兜住了
            assert not handle.runner.done()                               # 进程还活着
            assert app.platform.started                                   # 也没自己闭嘴
        finally:
            os.chmod(db.parent, stat.S_IRWXU)

        # 磁盘恢复后还能接着干活 —— 不是活着但废了
        await app.platform.emit(make_event(event_id="ok-2", text="恢复后的活",
                                           message_id="om_c"))
        await wait_until(lambda: app.platform.count("send_text") == 2, what="恢复后仍能交付")


async def test_store_failure_surfaces_as_an_exception_not_a_silent_false(config):
    """存储层写失败必须**抛**出来，不许假装成 `seen_event() -> False`。

    静默返回 False 的话，重连风暴里每条重复事件都会被当成新的 —— 同一个活干很多遍。
    """
    db = Path(config.storage.sqlite_path)
    store = SqliteSessionStore(db)
    await store.init()
    try:
        assert await store.seen_event("before-failure") is False

        os.chmod(db.parent, stat.S_IRUSR | stat.S_IXUSR)
        try:
            with pytest.raises(sqlite3.OperationalError):
                await store.seen_event("during-failure")
            with pytest.raises(sqlite3.OperationalError):
                await store.next_task_no("default")
        finally:
            os.chmod(db.parent, stat.S_IRWXU)

        assert await store.seen_event("after-recovery") is False      # 恢复后照常
        assert await store.seen_event("before-failure") is True       # 之前记的还在
    finally:
        await store.close()


async def test_crash_during_concurrent_writes_keeps_the_db_readable(config):
    """一堆并发写正在飞的时候崩溃：重开后库必须是完好的（不是半个事务）。"""
    store = SqliteSessionStore(config.storage.sqlite_path)
    await store.init()
    await asyncio.gather(*[store.seen_event(f"ev-{i}") for i in range(50)])
    nos = await asyncio.gather(*[store.next_task_no("default") for _ in range(50)])
    await store.close()                       # 崩溃：没有任何收尾

    reopened = SqliteSessionStore(config.storage.sqlite_path)
    await reopened.init()
    try:
        async with reopened._conn.execute("PRAGMA integrity_check") as cur:
            assert (await cur.fetchone())[0] == "ok"
        assert await reopened.seen_event("ev-0") is True              # 去重键全在
        assert await reopened.next_task_no("default") not in nos      # 号不回退、不重发
    finally:
        await reopened.close()
