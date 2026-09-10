"""第 7 组：断线重连后那一批重推，进程级贯通（owner: T24）。

§2.4 M2 的原话是两句：

    拔网线 30 秒再插回 → 服务自动重连；
    **断网期间群里发的 @ 在重连后被处理且只处理一次**

前半句和 adapter 那一层已经验过（`tests/adapters/feishu/test_feishu_reconnect.py`：
退避序列、`feishu.reconnected`、adapter 自己不去重）。后半句在进程级一条都没有 ——
去重本身只被 `evals/p0/10_duplicate_event.yaml` 验过，而那条走的是评测的 in-process
装配、投的是**顺序**的两条一模一样的事件。

真机上重连那一刻的形状不长这样。它是**一批**：几条不同的 @、夹着几条重复的、
可能还有断网前就已经处理过的，而且是**同时**进来的 —— `FeishuWSConnection._handle`
每收一帧就 `asyncio.run_coroutine_threadsafe(self._on_raw(...), loop)` 起一个独立协程，
从收帧到 `on_event` 这一路没有任何串行化。所以「一批同时到达」不是假想的极端，
它就是重连那一刻的常态形状。

这一组把那个形状造出来，从 `build_app` + `run_app` 走。驱动口径照抄第 6 组
（`test_t13_cold_start_to_delivery.py`）：等的是 `AppWorker.in_flight` 空掉，
不是 `send_text` 返回 —— 后者返回时 evidence 还没 finalize、沙箱还没还。

**替身写在本文件里，不进 `integration_fakes.py`**（§3.4：重复远比冲突便宜）。
落盘一律走 `tmp_path`，仓库的 data/ 一个字节都不许写。
"""
from __future__ import annotations

import asyncio
from typing import Any

import pytest
from app_under_test import AiteApp, build_app
from integration_fakes import (
    ClosableFakeSandbox,
    GatedPlatform,
    RecordingModel,
    evidence_task_dirs,
    final_step,
    make_event,
    read_events,
    running_app,
    settle,
    tool_step,
    wait_until,
)

from aite.contracts import AiteConfig, EvidenceKind, NormalizedEvent, TaskStatus

#: 断网**之前**就办完的那个活。重连时平台会把它一起重推上来。
BEFORE_EVENT = "ev-before"
BEFORE_MSG = "om_before"

#: 断网期间群里发的三个 @，各占一个话题。
DURING = (("ev-a", "om_a"), ("ev-b", "om_b"), ("ev-c", "om_c"))


def script_for(n_tasks: int) -> list[dict]:
    """给 n 个任务各配一份两步的脚本：先加一项清单（逼出 W3 那张卡），再 final。

    `FakeModel` 的游标是**全脚本共用**的，多任务会串 —— 这里不串是因为
    `run_forever` 一次只跑一个任务（`await self._run_task(...)` 跑完才取下一个），
    模型调用天然是串行的。所以第 k 个被派发的任务吃的就是第 k 段。
    多建了任务的话脚本会被吃穿（`ScriptExhausted`），红得很响，这正是我们要的。
    """
    steps: list[dict] = []
    for i in range(n_tasks):
        steps.append(tool_step("checklist_add", {"items": [f"第 {i + 1} 个活"]}))
        steps.append(final_step(f"第 {i + 1} 个活干完了。"))
    return steps


# --------------------------------------------------------------------------
# 替身
# --------------------------------------------------------------------------

class ReplayPlatform(GatedPlatform):
    """会断线、会重推的平台替身。

    与 `GatedPlatform` 的 `stopped` 是两码事：`stopped` 是**本进程在收尾**（不再收新事件），
    这里的 `online` 是**长连接断了**（平台还在收消息，只是送不到本进程）。真机上
    重连由 adapter 负责，控制面看得见的只有「一段时间没有事件，然后一批一起来」。
    """

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.online = True
        #: 断线期间平台替用户攒着的事件（真机上是飞书那边的重推队列）
        self.backlog: list[NormalizedEvent] = []
        self.replays = 0
        self._inflight = 0
        #: 同时压在 `on_event` 里的事件数的峰值。
        #: 「同时到达」这件事光靠 `gather` 是断言不了的 —— 万一 `handle_event` 一路
        #: 不 yield，gather 出来的也是一条跑完再一条。有了这个峰值，并发用例才算
        #: 真的在验并发；对照组也才证得了它确实是串行的。
        self.max_concurrent = 0

    async def emit(self, ev: NormalizedEvent) -> None:
        if not self.online:
            self.backlog.append(ev)
            return
        self._inflight += 1
        self.max_concurrent = max(self.max_concurrent, self._inflight)
        try:
            await super().emit(ev)
        finally:
            self._inflight -= 1

    def unplug(self) -> None:
        """拔网线。此后 `emit` 的事件都进 backlog，一条也到不了 `on_event`。"""
        self.online = False

    async def replug(
        self, extra: list[NormalizedEvent] | None = None, *, together: bool = True
    ) -> list[NormalizedEvent]:
        """插回网线，把这一批重推上来，返回实际推了哪些。

        `extra` 是「平台多推的那些」—— 重复的、断网前就处理过的。至少一次投递的
        长连接在重连边界上本来就会这样，adapter 明确不去重（§3.3），一律交给 R2。

        `together=True` 是重连那一刻的真形状：整批同时进 `on_event`，不是一条
        await 完再下一条。`False` 留给对照组。
        """
        self.online = True
        self.replays += 1
        batch = list(self.backlog) + list(extra or [])
        self.backlog.clear()
        if together:
            await asyncio.gather(*(self.emit(ev) for ev in batch))
        else:
            for ev in batch:
                await self.emit(ev)
        return batch


class GatedModel(RecordingModel):
    """第一次 `chat` 挂在闸门上不返回，其余照常出牌。

    用来造「网断的时候，有个任务正跑在半路上」：任务已经被 worker 领走、卡片也发了，
    就卡在下一次模型往返上。`hold_ticks` 做不到这件事 —— 它让的是固定几个 tick，
    我要的是「挂到我说放行为止」。
    """

    def __init__(self, script: list[dict], **kwargs: Any) -> None:
        super().__init__(script, **kwargs)
        self.gate = asyncio.Event()
        self.gate_reached = asyncio.Event()
        self._armed = True

    async def chat(self, messages: list, tools: list, **kwargs: Any):
        if self._armed:
            self._armed = False
            self.gate_reached.set()
            await self.gate.wait()
        return await super().chat(messages, tools, **kwargs)

    def release(self) -> None:
        self.gate.set()


# --------------------------------------------------------------------------
# 驱动
# --------------------------------------------------------------------------

def make_app(config: AiteConfig, *, n_tasks: int, gated: bool = False) -> AiteApp:
    model_cls = GatedModel if gated else RecordingModel
    return build_app(
        config,
        platform=ReplayPlatform(),
        model=model_cls(script_for(n_tasks)),
        sandbox=ClosableFakeSandbox(),
    )


async def settled(app: AiteApp, platform: ReplayPlatform, *, deliveries: int) -> None:
    """等到这一批全部收完 —— 交付数到位，且 worker 手上什么都不剩。

    只等 `send_text` 是不够的（那时 `_finish` 还没走完），只等 `in_flight` 空也不够
    （队列里可能还压着没派发的）。三件一起等。
    """
    await wait_until(
        lambda: platform.count("send_text") == deliveries,
        what=f"{deliveries} 次交付回帖",
    )
    await app.plane.join()
    await wait_until(lambda: not app.worker.in_flight, what="worker.run() 全部返回")


def mention(event_id: str, message_id: str, *, text: str | None = None) -> NormalizedEvent:
    return make_event(
        event_id=event_id,
        message_id=message_id,
        text=text or f"{event_id} 的活",
        mentioned=True,
    )


def reply_targets(platform: ReplayPlatform) -> set[str | None]:
    """每条交付回帖回到了哪条消息上。任务与话题 root 一一对应，所以这个集合
    就是「哪些事件真的变成了任务并交付」—— 而且不吃派发顺序。"""
    return {m.reply_to for m in platform.sent_texts}


# --------------------------------------------------------------------------
# 1 重推的一批：重复只算一次，不同的一个不丢，断网前处理过的也认得
# --------------------------------------------------------------------------

async def test_a_replayed_batch_dedups_without_dropping_anything_new(config):
    """M2 后半句的主干，一次把三条判据放在同一批里验。

    这一批的形状照重连那一刻造：三个断网期间发的 @（各占一个话题），其中一个被平台
    重推了三遍、另一个两遍，外加一条**断网之前就已经办完**的事件也被一起推了回来。
    """
    app = make_app(config, n_tasks=4)
    platform: ReplayPlatform = app.platform          # type: ignore[assignment]

    async with running_app(app):
        # ── 断网之前：正常办完一个活 ──────────────────────────────
        before = mention(BEFORE_EVENT, BEFORE_MSG, text="断网前的活")
        await platform.emit(before)
        await settled(app, platform, deliveries=1)

        # ── 拔网线：这三条一条也到不了本进程 ──────────────────────
        platform.unplug()
        for event_id, message_id in DURING:
            await platform.emit(mention(event_id, message_id))
        assert len(platform.backlog) == 3
        await settle()
        assert platform.count("send_text") == 1, "断网期间不该有任何新交付"

        # ── 插回网线：整批一次性重推，夹着重复和断网前那条 ────────
        batch = await platform.replug(
            extra=[
                mention("ev-a", "om_a"),          # 重复 #1
                mention("ev-a", "om_a"),          # 重复 #2
                mention("ev-b", "om_b"),          # 重复 #1
                mention(BEFORE_EVENT, BEFORE_MSG, text="断网前的活"),   # 断网前就办完的
            ]
        )
        assert len(batch) == 7, "这一批该是 3 条新的 + 3 条重复 + 1 条断网前的"

        await settled(app, platform, deliveries=4)
        await settle()   # 再给系统一把机会去做那件不该做的事（多建一个 task）

        # ── 逐条判据 ──────────────────────────────────────────────
        # 同 event_id 重复 → 只建一个 task、只交付一次
        # 不同 event_id 一个不丢 → 三个话题各交付一次
        # 断网前处理过的 → 被丢掉，没有第五个任务
        assert reply_targets(platform) == {BEFORE_MSG, "om_a", "om_b", "om_c"}, (
            f"每个话题各该有且只有一次交付，实际回到了 {reply_targets(platform)}"
        )
        assert platform.count("send_text") == 4
        assert platform.count("send_card") == 4, (
            f"一个任务一张卡，4 个任务就是 4 张，实际 {platform.count('send_card')} 张"
        )
        assert platform.card_count == 4

        sessions = {t.session_id for t in await _all_tasks(app, config)}
        tasks = await _all_tasks(app, config)
        assert len(tasks) == 4, f"4 个 event_id 就该有 4 个任务，实际 {len(tasks)} 个"
        assert len(sessions) == 4, "四条 @ 各占一个话题，会话也该是 4 个"
        assert all(t.status is TaskStatus.delivered for t in tasks), (
            f"这一批该全部交付，实际 {[(t.task_no, t.status.value) for t in tasks]}"
        )

        # 计数器对得上：4 条重复（ev-a×2、ev-b×1、断网前那条×1）
        assert app.plane.counters["events.duplicate"] == 4, (
            f"重复事件该被 R2 数走 4 条，实际 {app.plane.counters['events.duplicate']}"
        )
        assert app.ingress.counters["events.handled"] == 8   # 1 + 7
        assert app.ingress.counters["ingress.errors"] == 0

        # ack 也只加一次 —— 重复事件在 R2 就被拦下，走不到 R7 的 add_reaction
        acked = [m for m, _ in platform.reactions]
        assert sorted(acked) == sorted([BEFORE_MSG, "om_a", "om_b", "om_c"]), (
            f"@ 一条只该 ack 一次，实际 {platform.reactions}"
        )

        # 证据目录也是一个任务一个，重复事件不该凭空多出一份链
        assert sorted(evidence_task_dirs(config)) == sorted(t.id for t in tasks)


# --------------------------------------------------------------------------
# 2 一批**同时**到达：去重仍然只放行一次
# --------------------------------------------------------------------------

async def test_the_same_event_arriving_all_at_once_still_creates_one_task(config):
    """六份一模一样的重推**同时**进 `on_event`。

    T18 已经证过 `SqliteSessionStore.seen_event` 在并发下 `False` 恰好一次，但从
    `Ingress.on_event` 到建 task 这一整段没人在并发下验过 —— 中间还隔着
    `_thread_session`、`create_session`、`create_task`、两条 evidence 和一次入队，
    每一处都是可能被并发插进来的 await 点。

    这条与顺序到达的对照组（`10_duplicate_event`）分工明确：那边验行为，
    这边验「同时来也一样」。
    """
    app = make_app(config, n_tasks=1)
    platform: ReplayPlatform = app.platform          # type: ignore[assignment]

    async with running_app(app):
        platform.unplug()
        await platform.emit(mention("ev-storm", "om_storm"))
        await platform.replug(extra=[mention("ev-storm", "om_storm") for _ in range(5)])

        await settled(app, platform, deliveries=1)
        await settle()

        tasks = await _all_tasks(app, config)
        assert len(tasks) == 1, f"六份重推只该建一个任务，实际 {len(tasks)} 个"
        assert platform.count("send_text") == 1
        assert platform.count("send_card") == 1
        assert platform.reactions == [("om_storm", "ack")]
        assert app.plane.counters["events.duplicate"] == 5
        # 模型只该被调两次（checklist_add + final）。多一次就是多跑了一个任务。
        assert app.model.call_count == 2, f"模型被调了 {app.model.call_count} 次"
        assert platform.max_concurrent >= 2, (
            f"这一批压根没同时压在 on_event 里（峰值 {platform.max_concurrent}），"
            "那这条用例验的就不是并发了"
        )


async def test_a_concurrent_batch_of_distinct_events_loses_none(config):
    """去重不能误伤：五个不同的 `event_id` 同时到达，五个任务一个不少。

    并发下最容易出的错是「拿一把锁把整段 `handle_event` 串起来」——- 那样去重是稳了，
    但 §3.3 的「`on_event` 1s 内返回」会被重连风暴直接吃掉。这条盯的是另一头：
    没有误伤。
    """
    app = make_app(config, n_tasks=5)
    platform: ReplayPlatform = app.platform          # type: ignore[assignment]

    async with running_app(app):
        platform.unplug()
        for i in range(5):
            await platform.emit(mention(f"ev-{i}", f"om_{i}"))
        await platform.replug()

        await settled(app, platform, deliveries=5)
        await settle()

        tasks = await _all_tasks(app, config)
        assert len(tasks) == 5, f"五个不同的事件该有五个任务，实际 {len(tasks)} 个"
        assert reply_targets(platform) == {f"om_{i}" for i in range(5)}
        assert app.plane.counters["events.duplicate"] == 0, "一条都不该被当成重复"
        assert {t.task_no for t in tasks} == {"#A1", "#A2", "#A3", "#A4", "#A5"}, (
            f"任务号该是连着的五个，实际 {sorted(t.task_no for t in tasks)} —— "
            "并发下 next_task_no 发重号的话这里先红"
        )
        assert platform.max_concurrent >= 2, (
            f"这一批压根没同时压在 on_event 里（峰值 {platform.max_concurrent}）"
        )


# --------------------------------------------------------------------------
# 3 断网之前处理过的，跨断线仍然认得
# --------------------------------------------------------------------------

async def test_events_handled_before_the_drop_are_still_known_after_it(config):
    """`seen_event` 是落库的，所以断线本身不该让去重失忆。

    这条单独拆出来，是因为它和第 1 条的失败原因不一样：第 1 条红 = 这一批里的去重
    坏了；这条红 = 去重的记忆没跨过断线（比如哪天有人把 `seen_events` 改成内存表，
    或者重连时顺手清了什么）。真机上表现是同一句话被干两遍。
    """
    app = make_app(config, n_tasks=2)
    platform: ReplayPlatform = app.platform          # type: ignore[assignment]

    async with running_app(app):
        await platform.emit(mention(BEFORE_EVENT, BEFORE_MSG, text="断网前的活"))
        await settled(app, platform, deliveries=1)
        first = (await _all_tasks(app, config))[0]

        platform.unplug()
        await settle()
        # 重连：平台只重推了断网前那条（边界上的至少一次投递）
        await platform.replug(extra=[mention(BEFORE_EVENT, BEFORE_MSG, text="断网前的活")])
        await settle()

        assert await _all_tasks(app, config) == [first], "断网前办过的活不该被重推出第二遍"
        assert platform.count("send_text") == 1
        assert app.plane.counters["events.duplicate"] == 1

        # 系统没被这条重推带偏：紧接着来的新事件照样接得住
        await platform.emit(mention("ev-after", "om_after"))
        await settled(app, platform, deliveries=2)
        assert len(await _all_tasks(app, config)) == 2


# --------------------------------------------------------------------------
# 4 断网期间在跑的任务，重连之后照常收尾
# --------------------------------------------------------------------------

async def test_a_task_running_across_the_reconnect_finishes_normally(config):
    """M2 说的是「服务自动重连」，不是「重启」—— 进程一直活着，在跑的任务不该受影响。

    形状：任务 A 已经被 worker 领走、卡片发了，正卡在下一次模型往返上；这时网断了，
    群里又攒了两条；重连把那两条推上来；然后放行 A。A 要正常交付，两条新的也要
    各自办完，谁也别把谁挤掉。
    """
    app = make_app(config, n_tasks=3, gated=True)
    platform: ReplayPlatform = app.platform          # type: ignore[assignment]
    model: GatedModel = app.model                    # type: ignore[assignment]

    async with running_app(app):
        await platform.emit(mention("ev-long", "om_long", text="一个跑得久的活"))
        await asyncio.wait_for(model.gate_reached.wait(), timeout=5.0)

        running = await _all_tasks(app, config)
        assert len(running) == 1
        task_a = running[0]
        assert app.worker.in_flight, "闸门拦住的时候任务应该正在 worker 手上"

        # ── 网断了，群里又发了两条 ────────────────────────────────
        platform.unplug()
        await platform.emit(mention("ev-x", "om_x"))
        await platform.emit(mention("ev-y", "om_y"))
        await settle()
        assert app.worker.in_flight, "断线不该把在跑的任务弄没了"
        assert task_a.id in app.worker.in_flight

        # ── 重连，一批推上来（A 还挂着，它们只能先排队）──────────
        await platform.replug(extra=[mention("ev-x", "om_x")])   # 夹一条重复
        await settle()
        assert app.worker.in_flight and task_a.id in app.worker.in_flight, (
            "重连推进来的一批不该把正在跑的任务顶掉"
        )
        assert app.plane.pending == 2, (
            f"两个新任务该排在队列里等 A 跑完，实际队列里 {app.plane.pending} 个"
        )

        # ── 放行 A ────────────────────────────────────────────────
        model.release()
        await settled(app, platform, deliveries=3)
        await settle()

        tasks = await _all_tasks(app, config)
        assert len(tasks) == 3, f"A + 两条新的 = 3 个任务，实际 {len(tasks)} 个"
        assert all(t.status is TaskStatus.delivered for t in tasks)
        assert reply_targets(platform) == {"om_long", "om_x", "om_y"}
        assert app.plane.counters["events.duplicate"] == 1

        # A 的证据链自己收了口：断线没有在它中间插进任何东西
        events = read_events(config, task_a.id)
        kinds = [e.kind for e in events]
        assert kinds[0] is EvidenceKind.task_created
        assert kinds[1] is EvidenceKind.event_received
        assert kinds[-1] is EvidenceKind.delivered, (
            f"跨断线的任务该正常交付收口，实际最后一条是 {kinds[-1]}"
        )
        assert [k for k in kinds if k is EvidenceKind.event_received] == [
            EvidenceKind.event_received
        ], "重连推进来的那一批不该往 A 的证据链上写东西 ——- 它们是别的话题"

        from_disk = await app.store.get_task(task_a.id)
        assert from_disk is not None and from_disk.status is TaskStatus.delivered


# --------------------------------------------------------------------------
# 5 同一话题的 root 与追问一起被重推
# --------------------------------------------------------------------------

@pytest.mark.parametrize("together", [False, True], ids=["顺序到达", "同时到达"])
async def test_a_root_and_its_thread_followup_replayed_together(config, together):
    """断网期间用户 @ 了一句、又在同一话题里追了一句，两条一起重推上来。

    这是「不同 `event_id` 一个都不丢」里最险的一种：追问那条要不要变成任务，取决于
    R6 能不能在库里找到话题会话 —— 而那个会话是 root 那条**在同一批里**刚建的。
    顺序到达时 root 先落库，追问稳稳命中 R6；同时到达时两条在 `handle_event` 里
    交错，`_append_turn` 的「读 seq → 写 turn」还会撞在一起。

    这条用例把两种到达形状放在同一份断言下 —— **两边结论必须一样**。不一样就说明
    「一句话会不会被听见」取决于网抖不抖，那是 M2 后半句真正的失败面。

    改动前「同时到达」这一支是红的：两条都读到「还没有 turn」→ 都写 seq=0 →
    第二条抛 `DuplicateTurnError` → 被 `Ingress.on_event` 兜住 → 追问静默消失，
    而且因为 `seen_event` 已经落库，平台再重推一次也救不回来。见 `_append_turn`。
    """
    app = make_app(config, n_tasks=2)
    platform: ReplayPlatform = app.platform          # type: ignore[assignment]

    async with running_app(app):
        platform.unplug()
        await platform.emit(mention("ev-root", "om_root", text="按月画个图"))
        await platform.emit(
            make_event(
                event_id="ev-follow",
                message_id="om_follow",
                thread_id="om_root",
                text="再按季度画一张",
                mentioned=False,
            )
        )
        await platform.replug(together=together)
        await settle()
        await app.plane.join()
        await wait_until(lambda: not app.worker.in_flight, what="worker.run() 全部返回")
        await settle()

        tasks = await _all_tasks(app, config)
        sessions = {t.session_id for t in tasks}
        turns = await app.store.list_turns(next(iter(sessions))) if sessions else []

        assert len(tasks) >= 1, "root 那条至少要变成一个任务"
        assert app.plane.counters["events.ignored"] == 0, (
            "追问那条掉进了 R8（丢弃）—— 说明它到的时候话题会话还没落库。"
            f"计数器：{dict(app.plane.counters)}"
        )
        assert len(sessions) == 1, (
            f"两条在同一个话题里，只该有一个会话，实际 {len(sessions)} 个 —— "
            "追问那条没命中 R6，自己另起了一个话题"
        )
        assert [t.content for t in turns] == ["按月画个图", "再按季度画一张"], (
            f"两句话都该进同一份 transcript，实际 {[t.content for t in turns]}"
        )
        assert [t.seq for t in turns] == [0, 1], (
            f"seq 该是连着的两个，实际 {[t.seq for t in turns]}"
        )
        assert app.ingress.counters["ingress.errors"] == 0, (
            "有事件在路由里炸了 —— 它既没变成任务也不会被重推第二次，等于丢了"
        )


async def test_a_followup_replayed_before_its_root_is_dropped(config):
    """**当前边界，不是缺陷判定**：追问被重推在它的 root 前面时，它会掉进 R8。

    R6 的前提是「库里找得到这个话题的会话」，而会话是 root 那条建的。平台按乱序把
    追问先推上来时，追问到达的那一刻话题还不存在、它自己又没 @，于是命中 R8
    「其余丢弃」—— 没有 turn、没有任务、没有回帖，用户那句话消失。

    这不是哪一行写错了：R1–R8 是按编号顺序求值的纯函数，它没有「等一等它的 root」
    这个概念。要补得靠**事件级的重排或缓冲**（按 `occurred_at` 排、或给孤儿追问一个
    短暂的等待窗口），那是路由规则本身要改，超出本轨可写面 —— 报给总管。

    真机上多常见：飞书长连接的重推**通常**是保序的，而且 `_new_session` 在
    `create_session` 之后还隔着一次真 HTTP 的 `add_reaction`（几十到几百毫秒），
    追问有充足的时间等到会话落库。所以这条是「乱序重推时才炸」，不是常态。
    """
    app = make_app(config, n_tasks=2)
    platform: ReplayPlatform = app.platform          # type: ignore[assignment]

    async with running_app(app):
        platform.unplug()
        # 平台把追问推在了前面
        await platform.emit(
            make_event(
                event_id="ev-follow",
                message_id="om_follow",
                thread_id="om_root",
                text="再按季度画一张",
                mentioned=False,
            )
        )
        await platform.emit(mention("ev-root", "om_root", text="按月画个图"))
        await platform.replug()

        await settled(app, platform, deliveries=1)
        await settle()

        tasks = await _all_tasks(app, config)
        assert len(tasks) == 1, "只有 root 那条变成了任务"
        turns = await app.store.list_turns(tasks[0].session_id)
        assert [t.content for t in turns] == ["按月画个图"], (
            f"追问没进 transcript —— 这正是这条要钉的边界，实际 {[t.content for t in turns]}"
        )
        assert app.plane.counters["events.ignored"] == 1, "追问该是掉在 R8 上"
        # 没有异常、没有回帖：丢得**安静**，这也是它难被发现的原因
        assert app.ingress.counters["ingress.errors"] == 0
        assert platform.count("send_text") == 1


# --------------------------------------------------------------------------
# 6 对照组：顺序到达与同时到达，结论必须一致
# --------------------------------------------------------------------------

@pytest.mark.parametrize("together", [False, True], ids=["顺序到达", "同时到达"])
async def test_dedup_holds_for_both_arrival_shapes(config, together):
    """同一批事件，一条一条 await 和整批 gather，结果必须一模一样。

    两种形状都真实存在：顺序到达是 adapter 单帧慢慢来的样子，同时到达是重连那一刻
    一次收下几十帧的样子。结论分叉的话，真机上就会是「平时都对，一断网就多干活」。
    """
    app = make_app(config, n_tasks=2)
    platform: ReplayPlatform = app.platform          # type: ignore[assignment]

    async with running_app(app):
        platform.unplug()
        await platform.emit(mention("ev-p", "om_p"))
        await platform.emit(mention("ev-q", "om_q"))
        await platform.replug(
            extra=[mention("ev-p", "om_p"), mention("ev-q", "om_q")], together=together
        )

        await settled(app, platform, deliveries=2)
        await settle()

        tasks = await _all_tasks(app, config)
        assert len(tasks) == 2
        assert reply_targets(platform) == {"om_p", "om_q"}
        assert platform.count("send_card") == 2
        assert app.plane.counters["events.duplicate"] == 2

        # 两种形状确实是两种形状，不是同一件事换了个写法
        if together:
            assert platform.max_concurrent >= 2, "gather 那一支没真并发起来"
        else:
            assert platform.max_concurrent == 1, "顺序那一支不该有事件重叠"


# --------------------------------------------------------------------------
# 小工具
# --------------------------------------------------------------------------

async def _all_tasks(app: AiteApp, config: AiteConfig) -> list:
    """这一趟建出来的全部任务，按任务号排。

    §3.2 的 `SessionStore` 没有「列出全部任务」的口子 —— `list_active_tasks` 只给活跃的，
    而这一组要看的多半已经 delivered 了。task_id 从 evidence 目录拿：`_start_task` 建完
    任务第一件事就是写 `task_created`，所以「盘上有几个任务目录」就是「一共建了几个任务」。
    这条本身也是断言的一部分：多建一个 task 就一定多一个证据目录。
    """
    tasks = []
    for task_id in evidence_task_dirs(config):
        task = await app.store.get_task(task_id)
        if task is not None:
            tasks.append(task)
    return sorted(tasks, key=lambda t: t.task_no)
