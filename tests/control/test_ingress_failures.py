"""事件入口的失败面：路由半路炸了，那条事件去哪了（owner: T24）。

`tests/control/test_ingress.py` 验的是 `Ingress` 这层薄片本身（异常不漏回 adapter、
慢回调被叫出来）。这一份往下走一层，问的是**代价**：异常被兜住之后，那条事件
还有没有人管。

§3.3 最后一行写的是：

    任何未捕获异常 → task `failed` + 回帖 + evidence `failed`；进程不退出

「进程不退出」这半一直成立。另一半在两种情况下**一件都做不到**：

1. 异常发生在 `seen_event`（去重那一步）—— 此刻连 task 都还没建，没有 task 可标
   failed、没有帖可回、没有证据链可写。事件被静默丢掉。
2. 异常发生在 `seen_event` **之后** —— 更糟：去重记录已经落库了，平台再重推同一条，
   R2 会把它当重复丢掉。这条事件从此**救不回来**。

这两条都先钉成用例。判断和代价写在回执里 —— 钉住现状本身就是交付的一部分，
不是每条都得跟一个改动。
"""
import asyncio
from pathlib import Path

import pytest
from control_fakes import FakePlatform, make_event

from aite.contracts import NormalizedEvent
from aite.control import InProcessControlPlane
from aite.ingress import Ingress

CHAT = "oc_chat"
ROOT = "om_1"


class DiskHiccup(RuntimeError):
    """磁盘抖了一下。真机上是只读目录、盘满、SQLite 锁超时那一类。"""


class FlakyStore:
    """把某一个 store 方法变成会抛的（抛完即恢复），其余原样转发。

    §3.4 的老规矩：替身写在自己这份文件里，不动 `control_fakes.py`。
    这里也刻意不用 `unittest.mock` —— 包一层真 store 才验得了「抛在第几步」，
    而这正是这一组的全部内容。
    """

    def __init__(self, inner, *, fail_on: str, times: int = 1) -> None:
        self._inner = inner
        self._fail_on = fail_on
        self._left = times
        self.attempts: list[str] = []

    def __getattr__(self, name: str):
        target = getattr(self._inner, name)
        if name != self._fail_on:
            return target

        async def _boom(*args, **kwargs):
            self.attempts.append(name)
            if self._left > 0:
                self._left -= 1
                raise DiskHiccup(f"{name} 撞上了只读磁盘")
            return await target(*args, **kwargs)

        return _boom


def _ingress(plane: InProcessControlPlane) -> Ingress:
    return Ingress(plane)


# --------------------------------------------------------------------------
# 1 抛在 seen_event：事件被静默丢掉
# --------------------------------------------------------------------------

async def test_a_failure_in_seen_event_drops_the_event_silently(
    make_plane, store, platform, config
):
    """**当前行为，钉住而非认可**：`seen_event` 抛异常 → 什么都没发生。

    没有 task（连 `create_task` 都没走到）、没有会话、群里没有任何一句话、
    evidence 目录是空的。用户看到的就是「@ 了 Aite，它一点反应都没有」。

    §3.3 承诺的三件事（task failed / 回帖 / evidence failed）在这条路上一件都做不到 ——
    它们全都需要一个已经存在的 task，而异常抛在建 task 之前。
    """
    plane = make_plane(store=FlakyStore(store, fail_on="seen_event"))
    ingress = _ingress(plane)

    await ingress.on_event(make_event(text="磁盘抖的时候发的那句话"))   # 不抛

    assert ingress.counters["ingress.errors"] == 1
    assert ingress.counters["events.handled"] == 0
    # 三件承诺，一件都没有
    assert await store.list_active_tasks(CHAT) == [], "连 task 都没建出来，无从标 failed"
    assert platform.texts == [], "没有 task 就没有帖可回"
    assert platform.cards == [] and platform.reactions == []
    evidence_root = Path(config.storage.evidence_dir)
    assert [p for p in evidence_root.glob("*") if p.is_dir()] == [], (
        "没有 task_id 就没有证据链目录"
    )


async def test_the_process_keeps_working_after_the_disk_settles(make_plane, store, platform):
    """抖完就好：下一条事件照常接得住。「活着但废了」不算过关。"""
    plane = make_plane(store=FlakyStore(store, fail_on="seen_event", times=1))
    ingress = _ingress(plane)

    await ingress.on_event(make_event(event_id="ev-boom", text="撞上磁盘的那条"))
    await ingress.on_event(make_event(event_id="ev-ok", text="抖完之后的那条", message_id="om_2"))

    assert ingress.counters["ingress.errors"] == 1
    assert ingress.counters["events.handled"] == 1
    active = await store.list_active_tasks(CHAT)
    assert [t.title for t in active] == ["抖完之后的那条"]


# --------------------------------------------------------------------------
# 2 抛在 seen_event 之后：这条事件从此救不回来
# --------------------------------------------------------------------------

async def test_a_failure_after_seen_event_makes_the_event_unrecoverable(
    make_plane, store, platform
):
    """比第 1 条更糟的那一半，也是「重试」这条路走不通的根本原因。

    `seen_event` 是**先落库再往下走**的：它成功那一刻去重记录就已经 commit 了。
    如果紧接着的 `create_session` / `create_task` 炸了，事件同样被丢掉 —— 但这一次
    平台把它重推上来也没用，R2 认得它，会当成重复直接丢。

    所以「磁盘抖动多半是瞬时的，重试一次就好」这个直觉在这里不成立：
    能重试的只有**整条 `handle_event`**，而重跑第一步就会撞上 R2。
    """
    plane = make_plane(store=FlakyStore(store, fail_on="create_session"))
    ingress = _ingress(plane)
    ev = make_event(event_id="ev-lost", text="卡在建会话上的那句话")

    await ingress.on_event(ev)
    assert ingress.counters["ingress.errors"] == 1
    assert await store.list_active_tasks(CHAT) == []

    # 去重记录已经落库了 —— 平台重推也救不回来
    assert await store.seen_event("ev-lost") is True, (
        "seen_event 是先落库的，所以这条事件在库里已经算「见过」了"
    )
    await ingress.on_event(ev)                       # 平台重推同一条
    assert await store.list_active_tasks(CHAT) == [], "重推被 R2 当成重复丢掉了"
    assert plane.counters["events.duplicate"] == 1
    assert platform.texts == []


# --------------------------------------------------------------------------
# 3 计数器要看得见（本轨的那条小改）
# --------------------------------------------------------------------------

async def test_dropped_events_are_counted_on_the_plane(make_plane, store):
    """异常escape `handle_event` 时，控制面自己也记一笔。

    `Ingress.counters["ingress.errors"]` 早就在数了，但那个计数器挂在 ingress 上，
    而 `!status` 是控制面回的 —— 控制面拿不到 ingress。所以在这里也记一笔，
    两个计数器数的是同一批事件，只是待在不同的层上。
    """
    plane = make_plane(store=FlakyStore(store, fail_on="seen_event"))

    with pytest.raises(DiskHiccup):
        await plane.handle_event(make_event())       # 直接调控制面：异常必须还往上抛

    assert plane.counters["events.dropped"] == 1
    assert plane.counters["events.duplicate"] == 0


async def test_status_says_out_loud_that_events_were_dropped(make_plane, store, platform):
    """`!status` 是人在问「现在到底什么情况」的地方，丢过事件就得在这里说出来。

    不说的话，唯一的痕迹是一行 `ingress.handle_failed` 日志 —— 一人公司没有告警，
    没人会去翻。而用户能想到的动作恰恰就是 `!status`。
    """
    plane = make_plane(store=FlakyStore(store, fail_on="seen_event"))
    ingress = _ingress(plane)

    await ingress.on_event(make_event(event_id="ev-boom", text="被磁盘吃掉的那句"))
    await ingress.on_event(
        make_event(event_id="ev-status", text="!status", message_id="om_2")
    )

    body = platform.texts[-1].text
    assert "1" in body and "没接住" in body, (
        f"!status 该把丢掉的事件数说出来，实际回的是：{body!r}"
    )


async def test_status_stays_quiet_when_nothing_was_dropped(plane, platform):
    """没丢过就一个字都不多说 —— 平时的 `!status` 不该被一句常驻警告污染。"""
    await plane.handle_event(make_event(text="!status"))
    assert platform.texts[-1].text == "本群没有活跃任务"


# --------------------------------------------------------------------------
# 4 并发下的 seq 分配（本轨在 ① 里修掉的那条）
# --------------------------------------------------------------------------

async def test_two_events_in_one_thread_arriving_together_both_land(make_plane, store, platform):
    """同一个话题里的两条事件**同时**进 `handle_event`，两条都要落进 transcript。

    这是重连那一刻的真形状：`FeishuWSConnection._handle` 每收一帧就
    `run_coroutine_threadsafe` 起一个独立协程，整批一起上来。

    改动前：`_append_turn` 先 `list_turns` 读 seq、再 `append_turn` 写，两条都读到
    「还没有 turn」→ 都写 seq=0 → 第二条撞唯一约束抛 `DuplicateTurnError`。
    异常被 `Ingress` 兜住，用户那句追问静默消失，而且因为 `seen_event` 已经落库，
    平台重推也救不回来（见第 2 组）。
    """
    plane = make_plane()
    ingress = _ingress(plane)

    await ingress.on_event(make_event(event_id="ev1", text="按月画个图", message_id=ROOT))
    session_id = (await store.list_active_tasks(CHAT))[0].session_id

    followups = [
        make_event(
            event_id=f"ev-f{i}",
            text=f"追问 {i}",
            mentioned=False,
            message_id=f"om_f{i}",
            thread_id=ROOT,
        )
        for i in range(6)
    ]
    await asyncio.gather(*(ingress.on_event(ev) for ev in followups))

    assert ingress.counters["ingress.errors"] == 0, (
        "有追问在路由里炸了 —— 它既没进 transcript 也不会被重推第二次"
    )
    turns = await store.list_turns(session_id)
    assert [t.content for t in turns] == ["按月画个图"] + [f"追问 {i}" for i in range(6)], (
        f"六条追问该一条不少地进 transcript，实际 {[t.content for t in turns]}"
    )
    assert [t.seq for t in turns] == list(range(7)), (
        f"seq 该是连着的，实际 {[t.seq for t in turns]} —— 撞号就说明分配不是原子的"
    )


async def test_concurrent_seq_allocation_survives_a_bigger_burst(make_plane, store):
    """把批量放大到 20 条：seq 一个不重、一个不漏。

    小批量下事件循环可能恰好不交错，看起来是绿的。这条把批量放大，让「读 seq 到写 turn
    之间被插进来」真的发生 —— 改动前这条会红掉一片。
    """
    plane = make_plane()
    ingress = _ingress(plane)
    await ingress.on_event(make_event(event_id="ev1", text="root", message_id=ROOT))
    session_id = (await store.list_active_tasks(CHAT))[0].session_id

    burst: list[NormalizedEvent] = [
        make_event(
            event_id=f"ev-{i}",
            text=f"第 {i} 句",
            mentioned=False,
            message_id=f"om_{i}",
            thread_id=ROOT,
        )
        for i in range(20)
    ]
    await asyncio.gather(*(ingress.on_event(ev) for ev in burst))

    assert ingress.counters["ingress.errors"] == 0
    turns = await store.list_turns(session_id, limit=100)
    assert len(turns) == 21
    assert [t.seq for t in turns] == list(range(21))
    assert {t.content for t in turns} == {"root"} | {f"第 {i} 句" for i in range(20)}


async def test_a_failing_store_still_does_not_take_the_platform_down(make_plane, store):
    """FlakyStore 只抖一次，但 `on_event` 在任何一条上都不许把异常漏回 adapter。

    §3.3 第一行的另一半：回调抛出去会把长连接的读循环带走。
    """
    for method in ("seen_event", "create_session", "create_task", "append_turn"):
        plane = make_plane(store=FlakyStore(store, fail_on=method), platform=FakePlatform())
        ingress = _ingress(plane)
        await ingress.on_event(make_event(event_id=f"ev-{method}", message_id=f"om_{method}"))
        assert ingress.counters["ingress.errors"] == 1, f"抛在 {method} 上时没被兜住"
