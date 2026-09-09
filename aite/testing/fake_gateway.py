"""FakeToolGateway —— ToolGateway 的替身（dev-spec §3.2）。

真实现归 T3；这一份是给 tests/e2e 和 evals runner 用的，好让 T3 还没合进来时
场景仍然能跑出「工具被调过、返回了什么」。

严格按契约注释的顺序办事：
    校验 session_token → 查工具存在 → 按 ToolSpec.parameters 校验 arguments
    → 执行（带超时）→ 截断 content → 返回
并且**永远不抛异常给调用方**，所有失败都走 ToolResult(ok=False, error=...)。

`read_group_history` 在这里过滤掉非真人消息（§3.6 W1 / §6 T3），
PlatformPort.read_history 则不过滤 —— 05_history_summary 验的就是这道分工。
"""
from __future__ import annotations

import asyncio
import time
from typing import Any

from ..contracts import (
    DEFAULT_TOOL_TIMEOUT_SEC,
    GATEWAY_TOOLS,
    MAX_TOOL_CONTENT_CHARS,
    SandboxSpec,
    ToolCallRequest,
    ToolContext,
    ToolError,
    ToolErrorCode,
    ToolResult,
    ToolSpec,
)
from ..contracts.sandbox import ExecRequest
from .fake_platform import FakePlatform
from .fake_sandbox import FakeSandbox, FakeSandboxError
from .jsonschema_mini import SchemaError, validate
from .recorder import CallLog

_SPECS: dict[str, ToolSpec] = {t.name: t for t in GATEWAY_TOOLS}


class FakeToolGateway:
    """ToolGateway 的替身，背后接 FakePlatform + FakeSandbox。"""

    def __init__(
        self,
        *,
        platform: FakePlatform,
        sandbox: FakeSandbox,
        expected_token: str | None = None,
        timeout_sec: int = DEFAULT_TOOL_TIMEOUT_SEC,
        sandbox_image: str = "aite-sandbox:p0",
    ) -> None:
        self.platform = platform
        self.sandbox = sandbox
        #: None = 不校验（大多数场景不关心）；给了就必须与 ctx.session_token 一致
        self.expected_token = expected_token
        self.timeout_sec = timeout_sec
        self.sandbox_image = sandbox_image
        self.calls = CallLog()
        self.results: list[ToolResult] = []
        self._sandbox_of: dict[str, str] = {}

    # ---- ToolGateway ----------------------------------------------------

    def catalog(self, ctx: ToolContext) -> list[ToolSpec]:
        self.calls.record("catalog", task_id=ctx.task_id)
        return list(GATEWAY_TOOLS)

    async def call(self, ctx: ToolContext, req: ToolCallRequest) -> ToolResult:
        call = self.calls.record("call", name=req.name, task_id=ctx.task_id, arguments=req.arguments)
        started = time.monotonic()

        def done(
            ok: bool,
            content: str,
            *,
            code: ToolErrorCode | None = None,
            message: str = "",
            data: dict[str, Any] | None = None,
        ) -> ToolResult:
            result = ToolResult(
                call_id=req.call_id,
                name=req.name,
                ok=ok,
                content=content[:MAX_TOOL_CONTENT_CHARS],
                data=data,
                error=None if code is None else ToolError(code=code, message=message),
                duration_ms=int((time.monotonic() - started) * 1000),
            )
            call.result = f"ok={ok}" + ("" if code is None else f" {code}")
            self.results.append(result)
            return result

        if self.expected_token is not None and ctx.session_token != self.expected_token:
            return done(False, "会话令牌不匹配", code=ToolErrorCode.denied, message="session_token 不匹配")

        spec = _SPECS.get(req.name)
        if spec is None:
            return done(
                False,
                f"没有名为 {req.name} 的工具",
                code=ToolErrorCode.not_found,
                message=f"未知工具 {req.name}，可用：{sorted(_SPECS)}",
            )

        try:
            args = validate(dict(req.arguments), spec.parameters)
        except SchemaError as exc:
            return done(False, str(exc), code=ToolErrorCode.invalid_args, message=str(exc))

        timeout = args.get("timeout_sec", self.timeout_sec) if req.name == "run_python" else self.timeout_sec
        try:
            content, data = await asyncio.wait_for(self._dispatch(ctx, req.name, args), timeout=timeout)
        except TimeoutError:
            return done(
                False, f"{req.name} 执行超时（{timeout}s）", code=ToolErrorCode.timeout, message="timeout"
            )
        except FakeSandboxError as exc:
            return done(False, str(exc), code=ToolErrorCode.sandbox, message=str(exc))
        except Exception as exc:  # 平台侧/其他外部错误一律 upstream，绝不外抛
            return done(False, str(exc), code=ToolErrorCode.upstream, message=f"{type(exc).__name__}: {exc}")

        return done(True, content, data=data)

    # ---- 工具实现 --------------------------------------------------------

    async def _dispatch(self, ctx: ToolContext, name: str, args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        handler = getattr(self, f"_tool_{name}")
        return await handler(ctx, args)

    async def _tool_read_group_history(
        self, ctx: ToolContext, args: dict[str, Any]
    ) -> tuple[str, dict[str, Any]]:
        limit = args.get("limit", 50)
        thread_only = args.get("thread_only", False)
        rows = await self.platform.read_history(
            ctx.chat_id, limit=limit, thread_id=ctx.thread_id if thread_only else None
        )
        human = [h for h in rows if h.sender_kind == "human"]      # 过滤归 Gateway，不归 adapter
        lines = [f"[{h.message_id}] {h.sender_name or h.sender_id}: {h.text}" for h in human]
        return "\n".join(lines), {"count": len(human), "dropped": len(rows) - len(human)}

    async def _tool_read_document(self, ctx: ToolContext, args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        doc = await self.platform.read_document(args["url_or_token"])
        return f"# {doc.title}\n\n{doc.text}", {"title": doc.title, "url": doc.url}

    async def _tool_download_attachment(
        self, ctx: ToolContext, args: dict[str, Any]
    ) -> tuple[str, dict[str, Any]]:
        message_id = ctx.attachments_message_id
        if not message_id:
            raise RuntimeError("本次消息没有附件（ToolContext.attachments_message_id 为空）")
        file_key = args["file_key"]
        data = await self.platform.download_file(message_id, file_key)
        sandbox_id = await self._sandbox_for(ctx)
        path = f"/work/in/{file_key}"
        await self.sandbox.put_file(sandbox_id, path, data)
        return f"已下载到 {path}（{len(data)} 字节）", {"path": path, "size": len(data)}

    async def _tool_run_python(self, ctx: ToolContext, args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        sandbox_id = await self._sandbox_for(ctx)
        res = await self.sandbox.exec(
            sandbox_id, ExecRequest(code=args["code"], timeout_sec=args.get("timeout_sec", 120))
        )
        body = f"exit_code={res.exit_code}\n--- stdout ---\n{res.stdout}"
        if res.stderr:
            body += f"\n--- stderr ---\n{res.stderr}"
        return body, {
            "exit_code": res.exit_code,
            "files_out": [f.path for f in res.files_out],
            "duration_ms": res.duration_ms,
        }

    async def _tool_list_files(self, ctx: ToolContext, args: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        sandbox_id = await self._sandbox_for(ctx)
        files = await self.sandbox.list_files(sandbox_id)
        return "\n".join(files) if files else "(空)", {"files": files}

    # ---- 沙箱归属（与 ToolGateway 实现类同名同义）--------------------------
    #
    # 冻结的 ToolGateway 协议里只有 catalog / call，沙箱归属没进协议 —— 但产物是
    # 在这里建的沙箱里写出来的，worker 取产物、控制面 !stop 都得问得到它。真实现
    # （aite/gateway/tool_gateway.py）给了这两个方法，替身缺了就不等价：worker 会
    # 以为没有沙箱、自己 acquire 一个空的，04_csv_to_chart 的产物就此丢掉。

    def sandbox_id_of(self, task_id: str) -> str | None:
        """这个 task 现在用的沙箱（还没建就是 None）。"""
        return self._sandbox_of.get(task_id)

    async def release_task(self, task_id: str) -> None:
        """释放这个 task 的沙箱（幂等）。"""
        sandbox_id = self._sandbox_of.pop(task_id, None)
        if sandbox_id is not None:
            await self.sandbox.release(sandbox_id)

    # ---- 内部 -----------------------------------------------------------

    async def _sandbox_for(self, ctx: ToolContext) -> str:
        """每个 task 一个沙箱，用到才建。"""
        sandbox_id = self._sandbox_of.get(ctx.task_id)
        if sandbox_id is None:
            sandbox_id = await self.sandbox.acquire(ctx.task_id, SandboxSpec(image=self.sandbox_image))
            self._sandbox_of[ctx.task_id] = sandbox_id
        return sandbox_id

    # ---- 给断言用 --------------------------------------------------------

    def count(self, name: str) -> int:
        return sum(1 for c in self.calls.of("call") if c.kwargs.get("name") == name)

    def results_of(self, name: str) -> list[ToolResult]:
        return [r for r in self.results if r.name == name]
