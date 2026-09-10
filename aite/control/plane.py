"""ControlPlane 的实现（owner: T2，契约见 aite/contracts/ports.py §3.2）。

`handle_event` 是 §3.5 路由的唯一入口，R1–R8 **按编号顺序求值、命中即停**。
读这个文件时对着 §3.5 那张表看，`_route` 里每个分支都标了规则号。
"""
import asyncio
import contextlib
import logging
import secrets
import time
import uuid
from collections import defaultdict
from collections.abc import Callable
from datetime import UTC, datetime

from ..contracts import (
    AiteConfig,
    Anchor,
    EventKind,
    EvidenceKind,
    EvidenceWriter,
    ModelPort,
    NormalizedEvent,
    OutboundText,
    PlatformPort,
    SandboxPort,
    SenderKind,
    Session,
    SessionKind,
    SessionStatus,
    SessionStore,
    Task,
    TaskStatus,
    ToolGateway,
    Turn,
)
from ..worker.card import MAX_TITLE_CHARS, clip, render_card
from ..worker.loop import AgentWorker
from .commands import UNKNOWN_COMMAND_TEXT, normalize_task_no, parse_command

log = logging.getLogger("aite.control")

# W7：沙箱 reaper 的节奏
REAPER_INTERVAL_SEC = 60.0

NO_SUCH_TASK_TEXT = "没有这个任务"
NO_ACTIVE_TASK_TEXT = "本群没有活跃任务"
RESTART_EMPTY_TEXT = "已重开会话，请直接说要做什么。"

_R4_KINDS = (
    EventKind.message_edited,
    EventKind.message_deleted,
    EventKind.bot_added,
    EventKind.member_changed,
)

#: `event_received` 的 `route`：这条事件是**建了任务**的那一条（R6 新建 / R7）。
ROUTE_NEW_TASK = "new_task"
#: `event_received` 的 `route`：这条是**任务跑到一半**排进来的追问（R6 steer）。
ROUTE_STEER = "steer"


def _event_payload(ev: NormalizedEvent, *, route: str, **extra: object) -> dict:
    """`event_received` 的 payload。

    `EvidenceKind` 是冻结契约（`aite/contracts/evidence.py`），加不了新 kind，所以
    「建任务的那条事件」和「中途追问的那条」共用 `event_received`。共用就得分得开 ——
    不然时间线上两条长得一模一样，读的人分不出哪条是任务的起点、哪条是用户改了主意。
    分辨靠 `route`：两个写入点都带，缺这个字段的是本次改动之前写的老链。
    """
    return {
        "event_id": ev.event_id,
        "kind": ev.kind.value,
        "chat_id": ev.chat_id,
        "sender_id": ev.sender_id,
        "message_id": ev.anchor.message_id,
        "mentioned": ev.mentioned,
        "route": route,
        **extra,
    }


def _steer_payload(ev: NormalizedEvent) -> dict:
    """追问那条额外带上用户说的话（截断口径同卡片标题）。

    为什么这一条要带 `text` 而建任务那条不带：建任务那条紧挨着的 `task_created` 已经把
    用户原话截进 `title` 了，再抄一遍是噪音；追问这条旁边什么都没有，不带的话
    `model_call` 只留 `messages_hash`（W8：模型消息全文不进证据），
    从证据里反推不出「用户中途把要求改成了什么」—— 而这恰恰是解释「为什么最后交的是
    季度图而不是月度图」的关键一环。

    口径不是这里发明的：用户原话以**截断后的形式**进证据，`task_created.title` 一直
    就是这么做的。W8 禁的是**模型**消息全文，不是用户那句话。
    """
    return _event_payload(ev, route=ROUTE_STEER, text=clip(ev.text, MAX_TITLE_CHARS))


class InProcessControlPlane:
    """ControlPlane（T2）。进程内队列 + 单 worker，P0 只跑单副本。"""

    def __init__(
        self,
        *,
        store: SessionStore,
        platform: PlatformPort,
        evidence: EvidenceWriter,
        config: AiteConfig,
        model: ModelPort | None = None,
        gateway: ToolGateway | None = None,
        sandbox: SandboxPort | None = None,
        worker: AgentWorker | None = None,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], object] = asyncio.sleep,
        reaper_interval_sec: float = REAPER_INTERVAL_SEC,
    ) -> None:
        self._store = store
        self._platform = platform
        self._evidence = evidence
        self._config = config
        self._model = model
        self._sandbox = sandbox
        self._gateway = gateway
        self._clock = clock
        self._sleep = sleep
        self._reaper_interval = reaper_interval_sec
        self._worker = worker
        if self._worker is None and model is not None:
            self._worker = AgentWorker(
                store=store,
                platform=platform,
                model=model,
                evidence=evidence,
                config=config,
                gateway=gateway,
                sandbox=sandbox,
                clock=clock,
                sleep=sleep,
            )
        self._queue: asyncio.Queue[str] = asyncio.Queue()
        self._steer: dict[str, list[str]] = defaultdict(list)
        self._cancelled: set[str] = set()
        self._running: set[str] = set()
        #: 本进程接手过、还没收尾的任务（排在队列里的 + 正在 worker 手上的）。
        #: R6 只把追问排给这里面的任务 —— 见 `_steer_target`。
        self._owned: set[str] = set()
        #: 分配 turn.seq 的临界区。为什么要有它见 `_append_turn`。
        self._turn_seq_lock = asyncio.Lock()
        self.counters: dict[str, int] = defaultdict(int)

    async def init(self) -> None:
        await self._store.init()

    # ---- §3.5 路由 -------------------------------------------------------

    async def handle_event(self, ev: NormalizedEvent) -> None:
        """R1–R8 的唯一入口。按编号顺序求值，命中即停。

        路由半路抛出去的异常照旧往上传（`Ingress.on_event` 兜住它、不让长连接的读循环
        被带走，§3.3 第一行），这里只在路过时记一笔。为什么记在这儿而不是复用
        `Ingress.counters["ingress.errors"]`：`!status` 是控制面回的，控制面拿不到
        ingress。两个计数器数的是同一批事件，只是待在不同的层上。

        为什么这一笔非记不可：异常抛在 `seen_event` 那一步时，此刻连 task 都还没建，
        §3.3 承诺的「task failed + 回帖 + evidence failed」一件都做不到，事件就是
        静默没了。P0 认这个代价（判断与备选见回执），但不能连**认了多少条**都不知道。
        """
        try:
            await self._route(ev)
        except asyncio.CancelledError:
            raise
        except Exception:
            # 日志由 Ingress 打（那里有完整的 event_id / kind），这里不打第二遍。
            self.counters["events.dropped"] += 1
            raise

    async def _route(self, ev: NormalizedEvent) -> None:
        # R1：非真人一律丢弃。机器人/应用/系统消息永远不触发任务（含 Aite 自己发的）
        if ev.sender_kind != SenderKind.human:
            self.counters["events.nonhuman"] += 1
            return

        # R2：重连后平台重推的重复事件
        if await self._store.seen_event(ev.event_id):
            self.counters["events.duplicate"] += 1
            return

        # R3：卡片回传，不建会话
        if ev.kind == EventKind.card_action:
            await self._on_card_action(ev)
            return

        # R4：编辑/删除/成员变动，不触发任务
        if ev.kind in _R4_KINDS:
            await self._on_non_message(ev)
            return

        session = await self._thread_session(ev)
        text = ev.text.strip()

        # R5：`!` 命令，先于 R6/R7 判定；只接受 @ 过的或已在话题内的消息
        if text.startswith("!") and (ev.mentioned or session is not None):
            await self._on_command(ev, session, text)
            return

        # R6：话题内续接，不要求 mentioned
        if ev.chat_type == "group" and session is not None:
            await self._continue_session(ev, session)
            return

        # R7：@ 了 Aite → 新建 task session，本条消息成为话题 root
        if ev.mentioned:
            await self._new_session(ev, thread_id=ev.anchor.message_id, text=ev.text, react=True)
            return

        # R8：其余丢弃（P0 无群会话、无 DM 主动监听）
        self.counters["events.ignored"] += 1

    # ---- R3 --------------------------------------------------------------

    async def _on_card_action(self, ev: NormalizedEvent) -> None:
        action = ev.card_action
        if action is None:
            self.counters["events.bad_card_action"] += 1
            return
        if action.action == "stop":
            task = await self._resolve_task(ev.chat_id, action.task_id, card_id=action.card_id)
            if task is None:
                await self._reply(ev, NO_SUCH_TASK_TEXT)
                return
            await self.cancel_task(task, reply_to=ev.anchor.message_id, chat_id=ev.chat_id)
            return
        if action.action == "evidence":
            task_id = action.task_id or ""
            # EvidenceWriter 协议里没有路径访问器；实现类有 task_dir 就用它，否则按配置拼
            task_dir = getattr(self._evidence, "task_dir", None)
            where = (
                str(task_dir(task_id))
                if callable(task_dir)
                else f"{self._config.storage.evidence_dir}/{task_id}"
            )
            await self._reply(ev, f"任务 {task_id} 的证据目录：{where}")

    # ---- R4 --------------------------------------------------------------

    async def _on_non_message(self, ev: NormalizedEvent) -> None:
        if ev.kind == EventKind.message_edited:
            # 编辑即使加上 @Aite 也不启动任务，只在命中的会话里留一条 system_note
            session = await self._thread_session(ev, fallback_to_message_id=True)
            if session is not None:
                await self._append_turn(
                    session, ev, role="system_note", content=f"[用户修改了消息] 新内容：{ev.text}"
                )
            self.counters["events.edited"] += 1
            return
        if ev.kind == EventKind.message_deleted:
            self.counters["events.deleted"] += 1
            return
        self.counters[f"events.{ev.kind.value}"] += 1

    # ---- R5 --------------------------------------------------------------

    async def _on_command(self, ev: NormalizedEvent, session: Session | None, text: str) -> None:
        name, rest = parse_command(text)
        self.counters[f"commands{name}"] += 1

        if name == "!status":
            tasks = await self._store.list_active_tasks(ev.chat_id)
            if not tasks:
                await self._reply(ev, NO_ACTIVE_TASK_TEXT + self._dropped_note())
                return
            lines = [f"{t.task_no} {t.status.value} {t.title or '(无标题)'}" for t in tasks]
            await self._reply(ev, "本群活跃任务：\n" + "\n".join(lines) + self._dropped_note())
            return

        if name == "!stop":
            task = await self._resolve_stop_target(ev.chat_id, rest)
            if task is None:
                await self._reply(ev, NO_SUCH_TASK_TEXT)
                return
            await self.cancel_task(task, reply_to=ev.anchor.message_id, chat_id=ev.chat_id)
            return

        if name == "!restart":
            stopped = 0
            if session is not None:
                # 归档的会话不该留着还在跑的任务：它们的结果会落进一个已经不存在的会话里，
                # 而且会继续出现在 !status 里。§3.3 的 cancel 路径正好是它们该有的收尾。
                for t in await self._store.list_active_tasks(ev.chat_id):
                    if t.session_id == session.id:
                        await self.cancel_task(t, notify=False)
                        stopped += 1
                session.status = SessionStatus.archived
                session.archived_at = datetime.now(UTC)
                await self._store.update_session(session)
            # 用其余文本新建：还在原话题里的话就继续用同一个 thread_id
            thread_id = (session.anchor.thread_id if session else None) or ev.anchor.message_id
            await self._new_session(ev, thread_id=thread_id, text=rest, react=bool(rest))
            note = f"终止了 {stopped} 个进行中的任务。" if stopped else ""
            if not rest:
                await self._reply(ev, note + RESTART_EMPTY_TEXT)
            elif note:
                await self._reply(ev, "已重开会话，" + note)
            return

        if name == "!new":
            # 强制新建，即使已经在别的话题里：本条消息成为新话题的 root
            await self._new_session(ev, thread_id=ev.anchor.message_id, text=rest, react=bool(rest))
            if not rest:
                await self._reply(ev, RESTART_EMPTY_TEXT)
            return

        await self._reply(ev, UNKNOWN_COMMAND_TEXT)

    def _dropped_note(self) -> str:
        """丢过事件才多说这一句，平时一个字都不加。

        这条警告是**进程级**的（不分群）：`events.dropped` 数的是路由抛出去的事件，
        而抛在 `seen_event` 上时连 `chat_id` 归谁都还没走到，分不了群。措辞上说清楚
        它是「本进程启动以来」，免得被当成本群的账。

        为什么挂在 `!status` 上：这是人在问「现在到底什么情况」的地方。不挂的话，
        唯一的痕迹是一行 `ingress.handle_failed` 日志 —— 没有告警的话没人会去翻，
        而用户能想到的动作恰恰就是 `!status`。
        """
        n = self.counters["events.dropped"]
        if not n:
            return ""
        return (
            f"\n⚠ 本进程启动以来有 {n} 条事件没接住（多半是存储异常），"
            "可能有消息没被处理。翻日志看 ingress.handle_failed。"
        )

    async def _resolve_stop_target(self, chat_id: str, raw: str) -> Task | None:
        tasks = await self._store.list_active_tasks(chat_id)
        if not raw:
            # 只有一个活跃任务时允许省略任务号
            return tasks[0] if len(tasks) == 1 else None
        want = normalize_task_no(raw)
        return next((t for t in tasks if t.task_no == want), None)

    async def _resolve_task(self, chat_id: str, task_id: str | None, *, card_id: str | None) -> Task | None:
        tasks = await self._store.list_active_tasks(chat_id)
        if task_id:
            return next((t for t in tasks if t.id == task_id), None)
        return next((t for t in tasks if t.card_id and t.card_id == card_id), None)

    # ---- R6 / R7 ---------------------------------------------------------

    async def _continue_session(self, ev: NormalizedEvent, session: Session) -> None:
        await self._append_turn(session, ev, role="user", content=ev.text)
        session.last_active_at = datetime.now(UTC)
        await self._store.update_session(session)

        active = [
            t
            for t in await self._store.list_active_tasks(session.chat_id)
            if t.session_id == session.id and t.id not in self._cancelled
        ]
        target = self._steer_target(active)
        if target is not None:
            # 先写证据再排队，和 `_start_task` 一个顺序：证据链宁可少一条也不撒谎，
            # 不能出现「任务收到了这句追问，但链上查不到它是什么时候来的」。
            await self._evidence.append(target.id, EvidenceKind.event_received, _steer_payload(ev))
            # worker 每步开始前会把这些合并进上下文
            self._steer[target.id].append(ev.text)
            self.counters["events.steer"] += 1
            return
        if active:
            # 库里还是 active，可本进程既没在跑它也没排过它 —— 上个进程被杀之后留在
            # working 上的孤儿，没人会来 drain 它的 steer。按 R6 的「否则新建 task
            # 继续」走：不然用户这句追问排进一个永不消费的队列，看起来就是 Aite
            # 没反应（§2.4 M6「杀进程重启后在旧线程追问仍能续接」会红）。
            self.counters["events.orphan_task"] += 1
        await self._start_task(session, ev)

    async def _new_session(
        self, ev: NormalizedEvent, *, thread_id: str, text: str, react: bool
    ) -> Session:
        now = datetime.now(UTC)
        session = Session(
            id=str(uuid.uuid4()),
            tenant_id=ev.tenant_id,
            workspace_id=ev.workspace_id,
            chat_id=ev.chat_id,
            kind=SessionKind.task,
            anchor=Anchor(
                platform=ev.platform,
                chat_id=ev.chat_id,
                message_id=ev.anchor.message_id,
                thread_id=thread_id,
            ),
            status=SessionStatus.active,
            created_by=ev.sender_id,
            config_snapshot={
                "model": self._config.model.model,
                "initiator_name": ev.sender_name or ev.sender_id,
            },
            created_at=now,
            last_active_at=now,
        )
        await self._store.create_session(session)
        if react:
            with contextlib.suppress(Exception):
                await self._platform.add_reaction(ev.anchor.message_id, "ack")
        if text:
            await self._append_turn(session, ev, role="user", content=text)
            await self._start_task(session, ev, text=text)
        return session

    async def _start_task(self, session: Session, ev: NormalizedEvent, text: str | None = None) -> Task:
        now = datetime.now(UTC)
        task = Task(
            id=str(uuid.uuid4()),
            session_id=session.id,
            task_no=await self._store.next_task_no(session.tenant_id),
            status=TaskStatus.created,
            title=clip(text if text is not None else ev.text, MAX_TITLE_CHARS),
            session_token=secrets.token_hex(16),
            model=self._config.model.model,
            max_steps=self._config.worker.max_steps,
            max_wall_sec=self._config.worker.max_wall_sec,
            created_by=ev.sender_id,
            created_at=now,
            updated_at=now,
        )
        await self._store.create_task(task)
        await self._evidence.append(
            task.id,
            EvidenceKind.task_created,
            {
                "session_id": session.id,
                "task_no": task.task_no,
                "chat_id": session.chat_id,
                "created_by": task.created_by,
                "title": task.title,
            },
        )
        await self._evidence.append(
            task.id, EvidenceKind.event_received, _event_payload(ev, route=ROUTE_NEW_TASK)
        )
        self._owned.add(task.id)
        self._queue.put_nowait(task.id)
        return task

    # ---- 任务派发 --------------------------------------------------------

    async def run_forever(self) -> None:
        """派发队列里的任务给 worker，同时跑沙箱 reaper（W7）。"""
        reaper = asyncio.ensure_future(self._reaper_loop())
        try:
            while True:
                task_id = await self._queue.get()
                try:
                    await self._run_task(task_id)
                except Exception:  # §3.3 任何未捕获异常都不能让进程退出
                    log.exception("control.dispatch_failed task=%s", task_id)
                finally:
                    self._queue.task_done()
        finally:
            reaper.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await reaper

    async def run_pending(self) -> None:
        """把当前排队的任务跑完就返回。给测试和一次性回放用，不起 reaper。"""
        while not self._queue.empty():
            task_id = self._queue.get_nowait()
            try:
                await self._run_task(task_id)
            except Exception:
                log.exception("control.dispatch_failed task=%s", task_id)
            finally:
                self._queue.task_done()

    @property
    def pending(self) -> int:
        return self._queue.qsize()

    async def join(self) -> None:
        """等队列里已入队的任务都跑完。`run_forever` 在另一条协程上跑时用。"""
        await self._queue.join()

    async def _run_task(self, task_id: str) -> None:
        try:
            await self._dispatch_task(task_id)
        finally:
            # 这个任务是跑完了、还是压根没被领走（已取消 / 记录没了 / 没配 worker），
            # 排在它名下的 steer 都不会再有人来 drain。清理只放在 worker 那圈 finally
            # 里的话，`_dispatch_task` 上面那几条提前 return 会把队列留在内存里 ——
            # 一条泄漏，用户那句追问也石沉大海。最容易撞上的是「任务还在队列里排着
            # 就被 !stop 掉」。
            self._steer.pop(task_id, None)
            self._owned.discard(task_id)

    async def _dispatch_task(self, task_id: str) -> None:
        task = await self._store.get_task(task_id)
        if task is None:
            return
        session = await self._store.get_session(task.session_id)
        if session is None:
            return
        if self._worker is None:
            log.warning("control.no_worker task=%s", task_id)
            return
        if task_id in self._cancelled:
            return
        # 开跑前排进来的 steer 已经在 transcript 里了：`_continue_session` 先
        # append_turn 再排队，而 worker 的 `_build_messages` 是现在才去 list_turns。
        # 不清掉的话第一步开头会把同一句话再注入一遍，上下文里出现两条一样的用户发言。
        self._steer.pop(task_id, None)
        self._running.add(task_id)
        try:
            await self._worker.run(
                task,
                session,
                initiator=str(session.config_snapshot.get("initiator_name") or session.created_by),
                drain_steer=lambda: self._drain_steer(task_id),
                is_cancelled=lambda: task_id in self._cancelled,
            )
        finally:
            self._running.discard(task_id)

    def pending_steer(self, task_id: str) -> list[str]:
        """排队中的 steer 消息（R6）。worker 每步开始前会把它们合并进上下文。"""
        return list(self._steer.get(task_id, []))

    def _steer_target(self, active: list[Task]) -> Task | None:
        """这句话该排给哪个任务（R6）。没有能收的就返回 None，调用方按「新建 task」走。

        两条判据：

        1. **只认本进程接手过的**（`_owned`）。`list_active_tasks` 读的是库，杀进程
           重启后上一批任务还挂在 `working` 上，谁也不会再领它们。
        2. **优先给正在跑的那个**。它的 messages 在任务开跑那一刻就定型了，不合并
           进去就看不到这句话；还躺在队列里的任务不需要 steer —— 它开跑时
           `_build_messages` 会从 transcript 里读到（见 `_dispatch_task`）。

        多个候选时取最后创建的：§3.2 `SessionStore.list_active_tasks` 只承诺
        `status in (created, planning, working)`，**没承诺返回顺序**，所以按
        `created_at` 自己排，不吃 SqliteSessionStore 那句 `ORDER BY`。
        """
        owned = [t for t in active if t.id in self._owned]
        if not owned:
            return None
        running = [t for t in owned if t.id in self._running]
        return max(running or owned, key=lambda t: (t.created_at, t.id))

    def _drain_steer(self, task_id: str) -> list[str]:
        pending = self._steer.pop(task_id, [])
        return list(pending)

    async def _reaper_loop(self) -> None:
        while True:
            await self._sleep(self._reaper_interval)
            if self._sandbox is None:
                continue
            try:
                released = await self._sandbox.reap_idle(self._config.sandbox.idle_sec)
            except Exception:
                log.exception("control.reap_failed")
                continue
            if released:
                self.counters["sandbox.reaped"] += len(released)
                log.info("control.reaped n=%d", len(released))

    # ---- 取消（!stop / 卡片 stop 按钮，§3.3）-----------------------------

    async def cancel_task(
        self,
        task: Task,
        *,
        reply_to: str | None = None,
        chat_id: str | None = None,
        notify: bool = True,
    ) -> Task:
        running = task.id in self._running
        self._cancelled.add(task.id)
        task.status = TaskStatus.cancelled
        task.updated_at = datetime.now(UTC)
        await self._store.update_task(task)
        if self._sandbox is not None and task.sandbox_id:
            with contextlib.suppress(Exception):
                await self._sandbox.release(task.sandbox_id)

        session = await self._store.get_session(task.session_id)
        # 任务正在 worker 手里跑：证据、收卡片、还沙箱都交给它下一步开头做，
        # 那边拿到的步数才是准的，也免得同一条链上写两次 cancelled。
        # 反过来，没在跑的任务 worker 不会再收尾，Gateway 手上那个沙箱只能在这里还。
        if not running:
            await self._release_gateway_sandbox(task.id)
            await self._evidence.append(
                task.id, EvidenceKind.cancelled, {"by": "stop", "steps": task.steps}
            )
            if session is not None and task.card_id:
                with contextlib.suppress(Exception):
                    await self._platform.update_card(
                        task.card_id,
                        render_card(
                            task,
                            session,
                            initiator=str(
                                session.config_snapshot.get("initiator_name") or session.created_by
                            ),
                            status="cancelled",
                        ),
                    )
            await self._finalize_evidence(task, session)
        if notify:
            await self._platform.send_text(
                OutboundText(
                    chat_id=chat_id or (session.chat_id if session else ""),
                    text=f"任务 {task.task_no} 已停止。",
                    reply_to=reply_to,
                    in_thread=True,
                )
            )
        return task

    async def _finalize_evidence(self, task: Task, session: Session | None) -> None:
        """给证据链收口 —— 与 worker 的 `AgentWorker._finish()` 同一件事，manifest 字段照抄。

        没在跑的任务 worker 不会再收尾，这一步只能在这里做；不做的话没有 manifest.json、
        `Task.evidence_root_hash` 留空，§2.4 的 `evidence_show.py` 和卡片上的证据按钮
        都拿不到 root_hash。

        `finalize` 每个任务只该走一次：worker 收过尾的任务 `evidence_root_hash` 已经有值，
        再 finalize 一遍会把它算进这条 cancelled 之后，manifest 就和 worker 写的那份对不上了。
        """
        if task.evidence_root_hash:
            return
        # Task.model 建任务时取的是 config.model.model（默认空串），worker 开跑才覆盖成
        # 真模型名。没被领走就被停的任务遇上空配置就还是空的，退回本进程装着的模型名 ——
        # 对齐 `_finish()` 里那句 `task.model or self._model.name`。
        task.evidence_root_hash = await self._evidence.finalize(
            task.id,
            {
                "session_id": session.id if session is not None else task.session_id,
                "task_no": task.task_no,
                "created_by": task.created_by,
                "model": task.model or str(getattr(self._model, "name", "") or ""),
            },
        )
        # 上面那次 update_task 在 finalize 之前，root_hash 得靠这一次才进库。
        task.updated_at = datetime.now(UTC)
        await self._store.update_task(task)

    async def _release_gateway_sandbox(self, task_id: str) -> None:
        """让 Gateway 释放它给这个 task 建的沙箱并撤掉 token。

        沙箱是 Gateway 按 task_id 自己记着的（`Task.sandbox_id` 只记 worker 兜底建的
        那种），协议 ToolGateway 是冻结的、没有这个方法，实现类有 `release_task`
        就用它。
        """
        fn = getattr(self._gateway, "release_task", None)
        if not callable(fn):
            return
        try:
            await fn(task_id)
        except Exception:
            log.exception("control.gateway_release_failed task=%s", task_id)

    # ---- 小工具 ----------------------------------------------------------

    async def _thread_session(
        self, ev: NormalizedEvent, *, fallback_to_message_id: bool = False
    ) -> Session | None:
        thread_id = ev.anchor.thread_id
        if thread_id:
            hit = await self._store.find_session_by_thread(ev.chat_id, thread_id)
            if hit is not None:
                return hit
        if fallback_to_message_id:
            # 被编辑的可能正是话题 root 那条消息
            return await self._store.find_session_by_thread(ev.chat_id, ev.anchor.message_id)
        return None

    async def _append_turn(self, session: Session, ev: NormalizedEvent, *, role: str, content: str) -> None:
        # seq 由调用方分配（§3.2）。只用协议里有的 list_turns 推下一个 seq，
        # 这样换任何 SessionStore 实现都成立。
        #
        # 读 seq 和写 turn 之间不许有别人插进来。同一个话题里的两条事件**是会同时**进
        # `handle_event` 的 —— `FeishuWSConnection._handle` 每收一帧就
        # `run_coroutine_threadsafe` 起一个独立协程，重连那一刻整批一起上来，
        # 而这中间隔着两次真会挂起的 SQLite 往返。两条都读到「还没有 turn」就都写 seq=0，
        # 第二条撞上 (session_id, seq) 唯一约束抛 `DuplicateTurnError`；这一抛发生在
        # `seen_event` **已经落库之后**，于是事件既没变成任务，也永远不会被重推第二次
        # （R2 认得它了）—— 用户那句话就此消失。M2 后半句在这里破。
        #
        # P0 是单进程单副本（见本类 docstring），进程内一把锁就够。
        # `SqliteSessionStore` 本来就是单连接串行的，这把锁不额外增加争用；
        # 临界区只有两次 SQLite 往返，`on_event` 的 1s 预算（§3.3）吃得下。
        async with self._turn_seq_lock:
            recent = await self._store.list_turns(session.id, limit=1)
            seq = recent[-1].seq + 1 if recent else 0
            await self._store.append_turn(
                Turn(
                    session_id=session.id,
                    seq=seq,
                    role=role,
                    platform_user_id=ev.sender_id,
                    content=content,
                    attachments=ev.attachments,
                    created_at=ev.occurred_at,
                )
            )

    async def _reply(self, ev: NormalizedEvent, text: str) -> None:
        await self._platform.send_text(
            OutboundText(
                chat_id=ev.chat_id, text=text, reply_to=ev.anchor.message_id, in_thread=True
            )
        )
