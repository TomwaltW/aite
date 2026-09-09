"""Agent Loop（§3.6 W1–W9，owner: T2）。

一步 = 一次 `ModelPort.chat`。模型只能通过工具驱动进度面与产出：
checklist_* 与 final 由 worker 本地处理，其余交 `ToolGateway.call`（W2）。
"""
import asyncio
import hashlib
import logging
import mimetypes
import time
from collections.abc import Callable
from datetime import UTC, datetime
from pathlib import Path

from ..contracts import (
    ALL_MODEL_TOOLS,
    FINAL_TOOL,
    LOCAL_TOOL_NAMES,
    AiteConfig,
    ChecklistItem,
    EvidenceKind,
    EvidenceWriter,
    Message,
    ModelPort,
    OutboundFile,
    OutboundText,
    PlatformPort,
    SandboxPort,
    SandboxSpec,
    Session,
    SessionStore,
    Task,
    TaskStatus,
    ToolCallRequest,
    ToolContext,
    ToolGateway,
)
from .card import MAX_ITEM_CHARS, MAX_TITLE_CHARS, CardCoalescer, clip, render_card
from .context import build_context, load_system_prompt

log = logging.getLogger("aite.worker")

# §3.3 模型调用异常 / 5xx：重试 2 次
MODEL_RETRY_DELAYS: tuple[float, ...] = (2.0, 5.0)
# §3.3 同一任务连续 N 次 → failed
MAX_CONSECUTIVE_INVALID_ARGS = 3
MAX_CONSECUTIVE_SANDBOX_ERRORS = 2

NUDGE_TEXT = "请调用 final 交付结果，或调用一个工具继续。"


def _sha256(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


class _ToolOutcome:
    """本地工具的执行结果，形状对齐 ToolResult 里 worker 关心的那几项。"""

    __slots__ = ("ok", "content", "error_code")

    def __init__(self, ok: bool, content: str, error_code: str | None = None) -> None:
        self.ok = ok
        self.content = content
        self.error_code = error_code


class AgentWorker:
    """Worker（T2）。一个实例可以跑多个任务，每个任务的状态都在 run() 的栈上。"""

    def __init__(
        self,
        *,
        store: SessionStore,
        platform: PlatformPort,
        model: ModelPort,
        evidence: EvidenceWriter,
        config: AiteConfig,
        gateway: ToolGateway | None = None,
        sandbox: SandboxPort | None = None,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], object] = asyncio.sleep,
    ) -> None:
        self._store = store
        self._platform = platform
        self._model = model
        self._evidence = evidence
        self._config = config
        self._gateway = gateway
        self._sandbox = sandbox
        self._clock = clock
        self._sleep = sleep

    # ---- 入口 ------------------------------------------------------------

    async def run(
        self,
        task: Task,
        session: Session,
        *,
        initiator: str | None = None,
        drain_steer: Callable[[], list[str]] | None = None,
        is_cancelled: Callable[[], bool] | None = None,
    ) -> Task:
        card = CardCoalescer(
            self._platform,
            min_interval_ms=self._config.worker.card_update_min_interval_ms,
            clock=self._clock,
        )
        ctx = _RunContext(task=task, session=session, card=card, initiator=initiator or session.created_by)
        try:
            return await self._loop(ctx, drain_steer=drain_steer, is_cancelled=is_cancelled)
        except Exception as exc:  # §3.3 任何未捕获异常 → task failed + 回帖 + evidence，进程不退出
            log.exception("worker.unhandled task=%s", task.id)
            return await self._fail(ctx, f"任务 {task.task_no} 执行出错：{exc}")

    # ---- 主循环 ----------------------------------------------------------

    async def _loop(
        self,
        ctx: "_RunContext",
        *,
        drain_steer: Callable[[], list[str]] | None,
        is_cancelled: Callable[[], bool] | None,
    ) -> Task:
        task = ctx.task

        messages = await self._build_messages(ctx)
        if not task.title:
            task.title = clip(_last_user_text(messages) or "处理中", MAX_TITLE_CHARS)
        task.status = TaskStatus.planning
        task.model = self._model.name
        await self._save(task)

        started = self._clock()
        while True:
            if is_cancelled is not None and is_cancelled():
                return await self._cancel(ctx)
            if task.steps >= task.max_steps:
                return await self._fail(ctx, f"任务 {task.task_no}：已达步数上限，请缩小任务或 !new 重开")
            if self._clock() - started >= task.max_wall_sec:
                return await self._fail(ctx, f"任务 {task.task_no}：已达时间上限，请缩小任务或 !new 重开")

            # R6 排队进来的 steer 消息，在每步开始前合并进上下文
            if drain_steer is not None:
                for text in drain_steer():
                    messages.append(Message(role="user", content=text))

            step_index = task.steps
            try:
                turn = await self._chat(messages)
            except Exception:
                log.exception("worker.model_failed task=%s", task.id)
                return await self._fail(ctx, f"模型服务暂不可用，任务 {task.task_no} 已终止")

            task.steps += 1
            task.tokens_in += turn.usage.input_tokens
            task.tokens_out += turn.usage.output_tokens
            task.cost += self._price(turn.usage.input_tokens, turn.usage.output_tokens)
            await self._evidence.append(
                task.id,
                EvidenceKind.model_call,
                {
                    "model": self._model.name,
                    "step": step_index,
                    "messages_hash": _sha256("\n".join(m.model_dump_json() for m in messages)),
                    "usage": turn.usage.model_dump(),
                    "finish_reason": turn.finish_reason,
                },
            )

            messages.append(turn.message)
            calls = turn.message.tool_calls or []

            if not calls:
                text = (turn.message.content or "").strip()
                # §3.3 只返回文本：仅第一步兜底成 final，之后提示它用工具
                if step_index == 0 and text:
                    return await self._deliver(ctx, text, [], answering=True)
                messages.append(Message(role="system", content=NUDGE_TEXT))
                await ctx.card.maybe_flush()
                continue

            for call in calls:
                if call.name != FINAL_TOOL.name:
                    await self._ensure_card(ctx)

                if call.name == FINAL_TOOL.name:
                    await self._evidence.append(
                        task.id,
                        EvidenceKind.tool_call,
                        {"call_id": call.call_id, "name": call.name, "arguments": call.arguments},
                    )
                    reply, artifacts, bad = _parse_final(call)
                    if bad is not None:
                        await self._local_result(ctx, call, False, bad.content, bad.error_code)
                        messages.append(self._tool_message(call, bad.content))
                        ctx.invalid_args += 1
                        break
                    return await self._deliver(ctx, reply, artifacts, answering=not ctx.card.sent)

                outcome = await self._run_tool(ctx, call)
                messages.append(self._tool_message(call, outcome.content))

                if outcome.error_code == "invalid_args":
                    ctx.invalid_args += 1
                elif outcome.ok:
                    ctx.invalid_args = 0
                if outcome.error_code == "sandbox":
                    ctx.sandbox_errors += 1
                elif outcome.ok:
                    ctx.sandbox_errors = 0

                if ctx.invalid_args >= MAX_CONSECUTIVE_INVALID_ARGS:
                    return await self._fail(
                        ctx, f"任务 {task.task_no}：模型连续 {MAX_CONSECUTIVE_INVALID_ARGS} 次给出不合法的工具参数，已终止"
                    )
                if ctx.sandbox_errors >= MAX_CONSECUTIVE_SANDBOX_ERRORS:
                    return await self._fail(
                        ctx, f"任务 {task.task_no}：沙箱连续 {MAX_CONSECUTIVE_SANDBOX_ERRORS} 次不可用，已终止"
                    )

            if ctx.invalid_args >= MAX_CONSECUTIVE_INVALID_ARGS:
                return await self._fail(
                    ctx, f"任务 {task.task_no}：模型连续 {MAX_CONSECUTIVE_INVALID_ARGS} 次给出不合法的工具参数，已终止"
                )
            await self._refresh_card(ctx)
            await self._save(task)

    # ---- 上下文与模型 ----------------------------------------------------

    async def _build_messages(self, ctx: "_RunContext") -> list[Message]:
        session = ctx.session
        turns = await self._store.list_turns(session.id)
        try:
            history = await self._platform.read_history(
                session.chat_id, limit=self._config.feishu.history_window
            )
        except Exception:
            log.exception("worker.read_history_failed chat=%s", session.chat_id)
            history = []
        attachments = next((t.attachments for t in reversed(turns) if t.attachments), [])
        ctx.attachments_message_id = attachments[0].message_id if attachments else session.anchor.message_id
        prompt = load_system_prompt(self._config.worker.system_prompt_path)
        return build_context(prompt, turns, history, attachments)

    async def _chat(self, messages: list[Message]):
        last: Exception | None = None
        for delay in (0.0, *MODEL_RETRY_DELAYS):
            if delay:
                await self._sleep(delay)
            try:
                return await self._model.chat(
                    messages,
                    ALL_MODEL_TOOLS,
                    max_tokens=self._config.model.max_tokens,
                    temperature=self._config.model.temperature,
                )
            except Exception as exc:
                last = exc
        raise last

    def _price(self, tokens_in: int, tokens_out: int) -> float:
        m = self._config.model
        return (tokens_in * m.price_in_per_mtok + tokens_out * m.price_out_per_mtok) / 1_000_000

    def _tool_message(self, call: ToolCallRequest, content: str) -> Message:
        return Message(role="tool", content=content, tool_call_id=call.call_id, name=call.name)

    # ---- 工具分发（W2）---------------------------------------------------

    async def _run_tool(self, ctx: "_RunContext", call: ToolCallRequest) -> _ToolOutcome:
        if call.name in LOCAL_TOOL_NAMES:
            return await self._run_local_tool(ctx, call)
        return await self._run_gateway_tool(ctx, call)

    async def _run_local_tool(self, ctx: "_RunContext", call: ToolCallRequest) -> _ToolOutcome:
        task = ctx.task
        args = call.arguments or {}
        await self._evidence.append(
            task.id, EvidenceKind.tool_call, {"call_id": call.call_id, "name": call.name, "arguments": args}
        )

        if call.name == "checklist_add":
            items = args.get("items")
            if not isinstance(items, list) or not items or not all(isinstance(x, str) and x.strip() for x in items):
                return await self._local_result(ctx, call, False, "items 必须是 1–8 个非空字符串", "invalid_args")
            added = []
            for text in items[:8]:
                item = ChecklistItem(id=f"c{len(task.checklist) + 1}", text=clip(text, MAX_ITEM_CHARS))
                task.checklist.append(item)
                added.append(item.id)
            await self._checklist_evidence(task, "add", {"ids": added, "items": [i.text for i in task.checklist]})
            return await self._local_result(ctx, call, True, f"已添加 {len(added)} 项：{', '.join(added)}")

        if call.name in ("checklist_check", "checklist_fail"):
            item = next((i for i in task.checklist if i.id == args.get("id")), None)
            if item is None:
                return await self._local_result(ctx, call, False, f"没有这一项：{args.get('id')!r}", "invalid_args")
            if call.name == "checklist_check":
                item.state = "done"
            else:
                reason = args.get("reason")
                if not isinstance(reason, str) or not reason.strip():
                    return await self._local_result(ctx, call, False, "reason 必填", "invalid_args")
                item.state = "failed"
                item.note = clip(reason, 40)
            await self._checklist_evidence(task, call.name[10:], {"id": item.id, "state": item.state})
            return await self._local_result(ctx, call, True, f"{item.id} 已标记为 {item.state}")

        if call.name == "checklist_note":
            text = args.get("text")
            if not isinstance(text, str) or not text.strip():
                return await self._local_result(ctx, call, False, "text 必填", "invalid_args")
            ctx.note = clip(text, 40)
            await self._checklist_evidence(task, "note", {"text": ctx.note})
            return await self._local_result(ctx, call, True, "备注已更新")

        return await self._local_result(ctx, call, False, f"未知工具：{call.name}", "not_found")

    async def _local_result(
        self, ctx: "_RunContext", call: ToolCallRequest, ok: bool, content: str, code: str | None = None
    ) -> _ToolOutcome:
        await self._evidence.append(
            ctx.task.id,
            EvidenceKind.tool_result,
            {
                "call_id": call.call_id,
                "name": call.name,
                "ok": ok,
                "error": code,
                "content_hash": _sha256(content),
                "duration_ms": 0,
            },
        )
        return _ToolOutcome(ok, content, code)

    async def _checklist_evidence(self, task: Task, op: str, payload: dict) -> None:
        await self._evidence.append(task.id, EvidenceKind.checklist_op, {"op": op, **payload})

    async def _run_gateway_tool(self, ctx: "_RunContext", call: ToolCallRequest) -> _ToolOutcome:
        task = ctx.task
        await self._evidence.append(
            task.id,
            EvidenceKind.tool_call,
            {"call_id": call.call_id, "name": call.name, "arguments": call.arguments},
        )
        if self._gateway is None:
            return await self._local_result(ctx, call, False, f"工具不可用：{call.name}", "not_found")

        result = await self._gateway.call(self._tool_context(ctx), call)
        code = result.error.code.value if result.error is not None else None
        await self._evidence.append(
            task.id,
            EvidenceKind.tool_result,
            {
                "call_id": result.call_id,
                "name": result.name,
                "ok": result.ok,
                "error": code,
                "content_hash": _sha256(result.content),
                "duration_ms": result.duration_ms,
            },
        )
        content = result.content
        if not result.ok and result.error is not None and not content:
            content = f"[{result.error.code.value}] {result.error.message}"
        return _ToolOutcome(result.ok, content, code)

    def _tool_context(self, ctx: "_RunContext") -> ToolContext:
        s, t = ctx.session, ctx.task
        return ToolContext(
            tenant_id=s.tenant_id,
            workspace_id=s.workspace_id,
            chat_id=s.chat_id,
            session_id=s.id,
            task_id=t.id,
            session_token=t.session_token,
            thread_id=s.anchor.thread_id,
            attachments_message_id=ctx.attachments_message_id,
        )

    # ---- 卡片（W3 / W4）--------------------------------------------------

    async def _ensure_card(self, ctx: "_RunContext") -> None:
        if ctx.card.sent:
            return
        ctx.task.status = TaskStatus.working
        card = render_card(ctx.task, ctx.session, initiator=ctx.initiator, status="working", note=ctx.note)
        await ctx.card.ensure_card(ctx.session.chat_id, ctx.thread_root, card)
        ctx.task.card_id = ctx.card.card_id
        await self._save(ctx.task)

    async def _refresh_card(self, ctx: "_RunContext", status: str = "working") -> None:
        if not ctx.card.sent:
            return
        await ctx.card.update(
            render_card(ctx.task, ctx.session, initiator=ctx.initiator, status=status, note=ctx.note)
        )

    async def _close_card(self, ctx: "_RunContext", status: str) -> None:
        if not ctx.card.sent:
            return
        await ctx.card.force_flush(
            render_card(ctx.task, ctx.session, initiator=ctx.initiator, status=status, note=ctx.note)
        )

    # ---- 收尾 ------------------------------------------------------------

    async def _deliver(self, ctx: "_RunContext", reply: str, artifacts: list[dict], *, answering: bool) -> Task:
        """W5：产物逐个 get_file → send_file → evidence artifact；然后 send_text；delivered；finalize。"""
        task, session = ctx.task, ctx.session
        # Answering 路径（W3：第一步就 final，没发过卡片）在状态机上是独立的一格
        task.status = TaskStatus.answering if answering else TaskStatus.working
        await self._save(task)

        missing: list[str] = []
        for art in artifacts:
            path, title = str(art.get("path", "")), str(art.get("title") or art.get("path", ""))
            data = await self._fetch_artifact(task, path)
            if data is None:
                missing.append(title or path)
                continue
            mime = art.get("mime") or mimetypes.guess_type(path)[0] or "application/octet-stream"
            await self._platform.send_file(
                OutboundFile(
                    chat_id=session.chat_id,
                    reply_to=ctx.thread_root,
                    name=Path(path).name or title,
                    mime=mime,
                    data=data,
                )
            )
            await self._evidence.append(
                task.id,
                EvidenceKind.artifact,
                {"title": title, "mime": mime, "sha256": hashlib.sha256(data).hexdigest(), "size": len(data)},
            )

        text = reply.strip()
        if missing:
            text += "\n" + "\n".join(f"产物 {m} 未找到" for m in missing)
        await self._platform.send_text(
            OutboundText(chat_id=session.chat_id, text=text, reply_to=ctx.thread_root, in_thread=True)
        )

        task.status = TaskStatus.delivered
        task.result_summary = clip(reply, 200)
        await self._evidence.append(
            task.id,
            EvidenceKind.delivered,
            {"artifacts": len(artifacts) - len(missing), "missing": missing, "steps": task.steps},
        )
        await self._close_card(ctx, "delivered")
        await self._finish(ctx)
        return task

    async def _fail(self, ctx: "_RunContext", text: str) -> Task:
        task = ctx.task
        task.status = TaskStatus.failed
        task.result_summary = clip(text, 200)
        try:
            await self._platform.send_text(
                OutboundText(
                    chat_id=ctx.session.chat_id, text=text, reply_to=ctx.thread_root, in_thread=True
                )
            )
        except Exception:
            log.exception("worker.fail_notice_failed task=%s", task.id)
        await self._evidence.append(task.id, EvidenceKind.failed, {"reason": text, "steps": task.steps})
        await self._close_card(ctx, "failed")
        await self._finish(ctx)
        return task

    async def _cancel(self, ctx: "_RunContext") -> Task:
        task = ctx.task
        task.status = TaskStatus.cancelled
        await self._evidence.append(task.id, EvidenceKind.cancelled, {"steps": task.steps})
        await self._close_card(ctx, "cancelled")
        await self._finish(ctx)
        return task

    async def _finish(self, ctx: "_RunContext") -> None:
        task, session = ctx.task, ctx.session
        task.evidence_root_hash = await self._evidence.finalize(
            task.id,
            {
                "session_id": session.id,
                "task_no": task.task_no,
                "created_by": task.created_by,
                "model": task.model or self._model.name,
            },
        )
        # delivered / failed / cancelled 三条路都汇到这里，沙箱在这里还。
        # 不还的话容器要挂到 reaper 的 idle_sec 空闲超时才被收，任务结束了还占着。
        await self._release_gateway_sandbox(task.id)
        await self._save(task)

    async def _release_gateway_sandbox(self, task_id: str) -> None:
        """任务落终态：让 Gateway 释放沙箱并撤掉 session_token（幂等）。"""
        fn = getattr(self._gateway, "release_task", None)
        if not callable(fn):
            return
        try:
            await fn(task_id)
        except Exception:
            log.exception("worker.gateway_release_failed task=%s", task_id)

    async def _save(self, task: Task) -> None:
        task.updated_at = datetime.now(UTC)
        await self._store.update_task(task)

    async def _fetch_artifact(self, task: Task, path: str) -> bytes | None:
        """§3.3：path 不在 /work 下或取不到 → 跳过该产物，任务仍 delivered。"""
        if not path.startswith("/work/") or self._sandbox is None:
            return None
        try:
            if task.sandbox_id is None:
                # 产物是 run_python 在 Gateway 那个沙箱里写出来的，得先问它要。
                # 不问就直接 acquire 的话拿到的是个全新的空容器，产物必然找不到。
                task.sandbox_id = self._gateway_sandbox_id(task.id)
            if task.sandbox_id is None:
                cfg = self._config.sandbox
                task.sandbox_id = await self._sandbox.acquire(
                    task.id, SandboxSpec(image=cfg.image, cpu=cfg.cpu, mem_mb=cfg.mem_mb)
                )
            return await self._sandbox.get_file(task.sandbox_id, path)
        except Exception:
            log.exception("worker.artifact_failed task=%s path=%s", task.id, path)
            return None

    def _gateway_sandbox_id(self, task_id: str) -> str | None:
        """Gateway 手上这个 task 的沙箱。ToolGateway 协议（冻结）里没有这个访问器，
        实现类有 `sandbox_id_of` 就用它，没有就当没有沙箱。"""
        fn = getattr(self._gateway, "sandbox_id_of", None)
        if not callable(fn):
            return None
        try:
            return fn(task_id)
        except Exception:
            log.exception("worker.gateway_sandbox_id_failed task=%s", task_id)
            return None


class _RunContext:
    """一次 run() 的可变状态。放一个对象里免得在方法间传七八个参数。"""

    __slots__ = ("task", "session", "card", "initiator", "note", "invalid_args", "sandbox_errors",
                 "attachments_message_id")

    def __init__(self, *, task: Task, session: Session, card: CardCoalescer, initiator: str) -> None:
        self.task = task
        self.session = session
        self.card = card
        self.initiator = initiator
        self.note: str | None = None
        self.invalid_args = 0
        self.sandbox_errors = 0
        self.attachments_message_id: str | None = None

    @property
    def thread_root(self) -> str:
        return self.session.anchor.thread_id or self.session.anchor.message_id


def _parse_final(call: ToolCallRequest) -> tuple[str, list[dict], _ToolOutcome | None]:
    args = call.arguments or {}
    reply = args.get("reply")
    if not isinstance(reply, str) or not reply.strip():
        return "", [], _ToolOutcome(False, "final.reply 必填且不能为空", "invalid_args")
    raw = args.get("artifacts") or []
    if not isinstance(raw, list):
        return "", [], _ToolOutcome(False, "final.artifacts 必须是数组", "invalid_args")
    artifacts = [a for a in raw if isinstance(a, dict) and a.get("path")]
    return reply, artifacts, None


def _last_user_text(messages: list[Message]) -> str:
    for m in reversed(messages):
        if m.role == "user" and m.content:
            return m.content
    return ""
