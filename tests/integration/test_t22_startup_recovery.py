"""T22：起飞时把上一条命的残局收干净（§2.4 M6 的后半程）。

T18 查出了病、开了药方（`SqliteSessionStore.recover_orphan_tasks`），但**没有一个
生产调用点** —— 药在瓶子里。这一轨把它接进 `run_app`，测的就是接线本身：

* 收拾发生在 `platform.start()` **之前**（真机上 `FeishuPlatform.start()` 是个跑到
  `stop()` 才返回的重连循环，排在它后面的代码永远轮不到）；
* §3.3 对失败的三件事一件不少：task `failed`（store 做）+ 回帖 + evidence `failed`，
  外加把停在「进行中」的那张卡置成 failed（W4）；
* **一个孤儿收不掉，不许连累别人，更不许让进程起不来** —— 崩溃现场的
  `events.jsonl` 常常带半行 JSON，session 行也可能残了，这些都不是不起飞的理由。

外加「想退退不出去」那条：`store.init()` 成功之后到起飞完成之间抛异常，那条
aiosqlite 的**非 daemon** 线程必须被收掉，否则 `threading._shutdown` join 它会永久
阻塞 —— 跟「进程不退出」正相反，是想退退不出去，Ctrl-C 都救不回来。

崩溃怎么模拟：照 T18 的办法，**一步收尾都不走**（不 `platform.stop()`、不 `plane.join()`、
不 finalize evidence），只把 fd 关掉，再在同一个 .db 文件上重开一套组装。
"""
import asyncio
import threading

import pytest
from app_under_test import build_app
from integration_fakes import (
    CHAT,
    GatedPlatform,
    RecordingModel,
    evidence_task_dirs,
    final_step,
    make_event,
    manifest_path,
    read_events,
    read_manifest,
    read_task_from_disk,
    running_app,
    wait_until,
)

from aite.contracts import ChecklistItem, EvidenceKind, TaskStatus
from aite.control.store import ORPHAN_RESULT_SUMMARY, SqliteSessionStore
from aite.testing import FakeSandbox
from aite.worker.card import render_card

ROOT = "om_1"


def make_app(config, *, reply: str = "干完了。"):
    return build_app(
        config,
        platform=GatedPlatform(),
        model=RecordingModel([final_step(reply)]),
        sandbox=FakeSandbox(),
    )


async def crash_with_a_task_in_flight(config, *, text: str = "画个图", root: str = ROOT):
    """造一个「崩溃那一刻正在干活」的任务，返回 (app, task)。

    比 T18 那份多一样，因为这一轨要看的正是它：卡片已经发出去停在「进行中」。
    evidence 由 `handle_event` 自然写下 task_created / event_received 两条，
    finalize 没走 —— 崩溃现场就长这样。
    """
    app = make_app(config)
    await app.store.init()                        # run_app 起飞时做的那一步
    # 上一次崩溃留下的任务也还是 active，靠「这一轮新冒出来的那个」认人，
    # 别拿 list_active_tasks()[0] —— 那会一直指着最早那个。
    before = {t.id for t in await app.store.list_active_tasks(CHAT)}
    await app.plane.handle_event(make_event(event_id=f"e-{root}", text=text, message_id=root))

    task = next(t for t in await app.store.list_active_tasks(CHAT) if t.id not in before)
    session = await app.store.get_session(task.session_id)

    # worker 领走任务后做的第一件事：发卡片（W3）
    task.status = TaskStatus.working
    task.title = "画个图"
    task.checklist = [ChecklistItem(id="c1", text="画", state="doing")]
    result = await app.platform.send_card(
        session.chat_id,
        root,
        render_card(task, session, initiator=session.created_by, status="working"),
    )
    task.card_id = result.card_id
    await app.store.update_task(task)

    await app.store.close()                       # 只关 fd，等价 OS 回收；收尾序列全跳过
    return app, task


def reopen(config, dead, *, reply: str = "这次答上了。"):
    """在同一个 .db 上重开一套组装，并把上一条命发出的卡片认领过来。"""
    app = make_app(config, reply=reply)
    app.platform.adopt_cards(dead.platform)
    return app


# --------------------------------------------------------------------------
# 主线：孤儿被收干净
# --------------------------------------------------------------------------

async def test_takeoff_closes_the_orphan_in_the_group(config):
    """起飞就把孤儿收了：库里 failed、群里有话、卡片不再绿、证据链收口。"""
    dead, orphan = await crash_with_a_task_in_flight(config)
    app = reopen(config, dead)

    async with running_app(app):
        # 1) 状态（store 做的那一件）
        closed = await read_task_from_disk(config, orphan.id)
        assert closed is not None
        assert closed.status is TaskStatus.failed
        assert closed.result_summary == ORPHAN_RESULT_SUMMARY

        # 2) 回帖：回到任务原来那条话题里，带得出任务号
        assert app.platform.count("send_text") == 1
        notice = app.platform.sent_texts[0]
        assert orphan.task_no in notice.text
        assert ORPHAN_RESULT_SUMMARY in notice.text
        assert (notice.chat_id, notice.reply_to, notice.in_thread) == (CHAT, ROOT, True)

        # 3) 卡片：原地 PATCH 同一条，置 failed —— 群里那张绿不了的卡是人最先看见的
        assert app.platform.count("send_card") == 0, "收残局不许新发卡片，只能原地更新"
        snapshots = app.platform.card_snapshots(orphan.card_id)
        assert snapshots[-1].status == "failed"
        assert snapshots[-1].task_no == orphan.task_no

        # 4) evidence：failed 事件 + manifest，root_hash 回写进库
        kinds = [e.kind for e in read_events(config, orphan.id)]
        assert kinds == [
            EvidenceKind.task_created,      # 崩溃前 handle_event 写下的两条
            EvidenceKind.event_received,
            EvidenceKind.failed,            # 收残局补上的这一条
        ]
        payload = read_events(config, orphan.id)[-1].payload
        assert payload["reason"] == ORPHAN_RESULT_SUMMARY
        assert payload["by"] == "startup_recovery"
        assert read_manifest(config, orphan.id)["root_hash"] == closed.evidence_root_hash

        # 5) 用户看得见的那一面：`!status` 不再挂着它
        await app.platform.emit(make_event(event_id="e-st", text="!status", message_id="om_s"))
        assert orphan.task_no not in app.platform.texts()[-1]


async def test_orphans_are_closed_before_the_platform_starts(config):
    """收拾必须排在 `platform.start()` 之前。

    真机上 `FeishuPlatform.start()` 是个跑到 `stop()` 才返回的重连循环
    （`adapters/feishu/platform.py:173` 的 `while not self._stopping`），
    排在它后面的收拾逻辑一辈子等不到执行；而出站走的是另一条 HTTP 路
    （同文件 253-314 行 → `api.request`），不必等长连接。
    """
    dead, orphan = await crash_with_a_task_in_flight(config)
    app = reopen(config, dead)

    async with running_app(app):
        methods = app.platform.calls.methods()
        assert "start" in methods, methods
        start_at = methods.index("start")
        assert methods.index("update_card") < start_at, f"卡片收在 start() 之后了：{methods}"
        assert methods.index("send_text") < start_at, f"回帖发在 start() 之后了：{methods}"


async def test_a_new_question_in_the_old_thread_still_lands(config):
    """M6 正题：收干净之后，在旧线程追问照样续接。

    T14 修的是「孤儿吞掉追问」那一半，这里验的是两半合起来还成立 ——
    收残局没有把会话连坐掉，任务号也接着往下发。
    """
    dead, orphan = await crash_with_a_task_in_flight(config)
    app = reopen(config, dead)

    async with running_app(app):
        await app.platform.emit(
            make_event(event_id="e9", text="接着上面那个问题", mentioned=False,
                       message_id="om_9", thread_id=ROOT)
        )
        # handle_event 在 emit 里同步走完，任务当场就在库里
        new_task = (await app.store.list_active_tasks(CHAT))[0]
        assert new_task.task_no == "#A2" != orphan.task_no      # 新号，不复用孤儿那个
        assert new_task.session_id == orphan.session_id         # 同一会话，话题锚点没断

        await wait_until(lambda: app.platform.count("send_text") == 2, what="新任务交付")
        assert any("画个图" in text for text in app.model.prompt_texts())


async def test_nothing_to_recover_stays_quiet(config):
    """库里没有残局时，起飞一句话都不许说。"""
    app = make_app(config)
    async with running_app(app):
        assert app.platform.outbound_count == 0
    assert evidence_task_dirs(config) == []


# --------------------------------------------------------------------------
# 一个孤儿收不掉，不许连累别人
# --------------------------------------------------------------------------

def break_evidence_for(app, *task_ids: str) -> None:
    """让这几个 task 的 `evidence.append` 一律抛。

    真机上最常见的触发原因是崩溃留下的半行 JSON（`_read_events` 撞上就抛，T18 钉过
    这条现状，写入侧自愈归 T21）。这里**不**用残行来演，改成直接注入失败 ——
    要钉的是「起飞这一层碰上写不进去的证据怎么降级」，那是本轨自己的账；
    绑着 evidence 面的当期实现写，等那边一修好这条测试就变成空转。
    """
    broken = set(task_ids)
    real = app.evidence.append

    async def append(task_id, kind, payload):
        if task_id in broken:
            raise RuntimeError(f"{task_id} 的证据目录写不进去")
        return await real(task_id, kind, payload)

    app.evidence.append = append


async def test_a_broken_evidence_chain_still_gets_the_group_told(config):
    """evidence 写不进去，但群里那两件事照做，进程照常起飞。

    证据链残着是既成事实，用户那头却不能因此一点交代都没有 —— 卡片绿着挂在群里
    比什么都糟。
    """
    dead, orphan = await crash_with_a_task_in_flight(config)

    app = reopen(config, dead)
    break_evidence_for(app, orphan.id)
    async with running_app(app):
        closed = await read_task_from_disk(config, orphan.id)
        assert closed is not None and closed.status is TaskStatus.failed

        # append 都没成，manifest 自然也不该有：没写进去的东西不许假装收了口
        assert not manifest_path(config, orphan.id).exists()
        assert not closed.evidence_root_hash

        # 但人看得见的两件事一件没少
        assert app.platform.count("send_text") == 1
        assert orphan.task_no in app.platform.texts()[0]
        assert app.platform.card_snapshots(orphan.card_id)[-1].status == "failed"


async def test_one_hopeless_orphan_does_not_block_the_others(config):
    """两个孤儿，头一个怎么收都收不掉，第二个照样收干净。"""
    dead_a, orphan_a = await crash_with_a_task_in_flight(config, text="画个图", root=ROOT)
    dead_b, orphan_b = await crash_with_a_task_in_flight(config, text="写个稿", root="om_2")

    app = reopen(config, dead_a)
    app.platform.adopt_cards(dead_b.platform)
    # 头一个：证据写不进去，卡片也 PATCH 不动（fail_next 抛完即清，只砸第一次调用）
    break_evidence_for(app, orphan_a.id)
    app.platform.fail_next["update_card"] = RuntimeError("飞书那头 500 了")

    async with running_app(app):
        for orphan in (orphan_a, orphan_b):
            closed = await read_task_from_disk(config, orphan.id)
            assert closed is not None and closed.status is TaskStatus.failed, orphan.task_no

        # 两个都回了帖 —— 头一个的 evidence 和卡片全军覆没也没吞掉它自己那条
        texts = app.platform.texts()
        assert len(texts) == 2
        assert {orphan_a.task_no, orphan_b.task_no} == {t.split("：")[0].split()[-1] for t in texts}

        # 第二个的证据链是完整收口的：头一个的塌方没有蔓延
        assert read_manifest(config, orphan_b.id)["event_count"] == 3
        assert app.platform.card_snapshots(orphan_b.card_id)[-1].status == "failed"

        assert await app.store.list_active_tasks(CHAT) == []


async def test_an_orphan_without_a_session_does_not_block_takeoff(config):
    """session 行没了：回帖和卡片没有收件人，只写 evidence，起飞照常。"""
    dead, orphan = await crash_with_a_task_in_flight(config)

    store = SqliteSessionStore(config.storage.sqlite_path)
    await store.init()
    try:
        await store._conn.execute("DELETE FROM sessions WHERE id = ?", (orphan.session_id,))
        await store._conn.commit()
    finally:
        await store.close()

    app = reopen(config, dead)
    async with running_app(app):
        closed = await read_task_from_disk(config, orphan.id)
        assert closed is not None and closed.status is TaskStatus.failed

        # 硬发只会发到别处去，所以一句话都不说
        assert app.platform.count("send_text") == 0
        assert app.platform.count("update_card") == 0

        # evidence 只要 task_id 就够，照写照收口
        assert [e.kind for e in read_events(config, orphan.id)][-1] is EvidenceKind.failed
        assert read_manifest(config, orphan.id)["root_hash"] == closed.evidence_root_hash


async def test_a_broken_store_query_does_not_block_takeoff(config):
    """连「查出残局」这一步都炸了，进程照样起得来。"""
    app = make_app(config)

    async def boom() -> list:
        raise RuntimeError("库读不动了")

    app.store.recover_orphan_tasks = boom            # type: ignore[method-assign]

    async with running_app(app) as run:
        assert app.platform.started is True
        assert not run.runner.done()


# --------------------------------------------------------------------------
# 「想退退不出去」
# --------------------------------------------------------------------------

class ExplodingInitStore(SqliteSessionStore):
    """`init()` 把连接建好之后立刻抛 —— 「init 成功了但起飞没成」那个窗口。"""

    def __init__(self, path) -> None:
        super().__init__(path)
        self.close_calls = 0

    async def init(self) -> None:
        await super().init()                          # 连接真建起来了，工作线程已经在跑
        raise RuntimeError("起飞半路炸了")

    async def close(self) -> None:
        self.close_calls += 1
        await super().close()


def sqlite_worker_threads() -> set[threading.Thread]:
    """aiosqlite 的连接工作线程 —— 非 daemon，活着就能把进程钉在退出那一步。"""
    return {t for t in threading.enumerate() if "_connection_worker_thread" in t.name}


async def test_store_is_closed_when_takeoff_explodes_after_init(config):
    """init 成功之后抛出的异常必须走到 `store.close()`，否则进程想退退不出去。

    断言两层：`close()` 真的被调了，以及那条**非 daemon** 的 aiosqlite 工作线程
    真的没了 —— 后者才是「进程能退」的直接证据，`threading._shutdown` join 的
    正是它。
    """
    app = make_app(config)
    app.store = ExplodingInitStore(config.storage.sqlite_path)
    before = sqlite_worker_threads()

    with pytest.raises(RuntimeError, match="起飞半路炸了"):
        await asyncio.wait_for(run_and_return(app), timeout=10.0)

    assert app.store.close_calls == 1
    assert app.store._db is None
    leaked = {t for t in sqlite_worker_threads() - before if t.is_alive()}
    assert not leaked, f"aiosqlite 的非 daemon 线程漏了：{[t.name for t in leaked]}"


async def run_and_return(app) -> None:
    from app_under_test import run_app

    await run_app(app, stop=asyncio.Event(), shutdown_grace_sec=1.0)
