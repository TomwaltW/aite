"""ToolGateway 的 P0 实现（owner: T3，契约见 aite/contracts/ports.py §3.2）。

`call` 的顺序照 §3.2 的注释逐字执行：

    校验 session_token → 查工具存在 → 按 ToolSpec.parameters 校验 arguments
    → 执行（带超时）→ 截断 content → 返回

以及那条硬要求：**永远不抛异常给调用方**。`call` 里唯一会往外传的是
`asyncio.CancelledError`（那是调用方自己在取消，吞掉它才是 bug），
其余一切 —— 包括工具实现里的意外 —— 都变成 `ToolResult(ok=False, error=...)`。

session_token 的校验是**失败关闭**的：查不到这个 task_id 该有的 token 就一律 denied。
Gateway 不认识 SessionStore（那是 T2 的），所以期望值有两个来源：
`register_task()` 现场登记，或者构造时传一个 `token_resolver`（TΩ 组装时把它接到
`SessionStore.get_task().session_token` 上）。两个都没有 = 谁也别想调工具，
而不是「没登记就放过」。
"""
import asyncio
import secrets
import time
from collections.abc import Callable
from typing import Any

from ..contracts import (
    DEFAULT_TOOL_TIMEOUT_SEC,
    GATEWAY_TOOLS,
    MAX_TOOL_CONTENT_CHARS,
    PlatformPort,
    SandboxConfig,
    SandboxPort,
    SandboxSpec,
    ToolCallRequest,
    ToolContext,
    ToolError,
    ToolErrorCode,
    ToolResult,
    ToolSpec,
)
from ..tools import TOOL_IMPLS, ToolEnv, ToolFailure, ToolImpl, ToolOutcome
from .schema import SchemaViolation, validate_arguments

__all__ = ["P0ToolGateway"]

#: task_id → 期望的 session_token。
TokenResolver = Callable[[str], str | None]

#: `run_python` 的外层超时比它自己的 timeout_sec 多留一点余量。
#: 代码的时限由容器内的 coreutils `timeout` 精确执行（超时退出码 124，工具翻成 timeout）；
#: 外层这道 `wait_for` 只在 Docker daemon 卡住、连 exec 都不返回时才该开火。
#: 余量为 0 的话两个时限同时到，赢的通常是外层，模型就永远拿不到超时前的部分输出。
DEFAULT_RUN_PYTHON_GRACE_SEC = 5.0


def _spec_from_config(cfg: SandboxConfig) -> SandboxSpec:
    return SandboxSpec(image=cfg.image, cpu=cfg.cpu, mem_mb=cfg.mem_mb)


def _clip(text: str, limit: int = MAX_TOOL_CONTENT_CHARS) -> str:
    if len(text) <= limit:
        return text
    marker = "\n…[内容已截断]"
    return text[: max(0, limit - len(marker))] + marker


class P0ToolGateway:
    """ToolGateway（T3）。P0 的 catalog 就是 GATEWAY_TOOLS 原样。"""

    def __init__(
        self,
        *,
        platform: PlatformPort | None = None,
        sandbox: SandboxPort | None = None,
        sandbox_spec: SandboxSpec | None = None,
        token_resolver: TokenResolver | None = None,
        default_timeout_sec: float = DEFAULT_TOOL_TIMEOUT_SEC,
        run_python_grace_sec: float = DEFAULT_RUN_PYTHON_GRACE_SEC,
        tools: dict[str, ToolImpl] | None = None,
    ) -> None:
        self._platform = platform
        self._sandbox = sandbox
        self._sandbox_spec = sandbox_spec or _spec_from_config(SandboxConfig())
        self._token_resolver = token_resolver
        self._default_timeout_sec = float(default_timeout_sec)
        self._run_python_grace_sec = float(run_python_grace_sec)

        self._specs: dict[str, ToolSpec] = {t.name: t for t in GATEWAY_TOOLS}
        self._impls: dict[str, ToolImpl] = dict(TOOL_IMPLS if tools is None else tools)

        self._tokens: dict[str, str] = {}
        self._sandbox_ids: dict[str, str] = {}
        self._acquire_locks: dict[str, asyncio.Lock] = {}

    # ------------------------------------------------------------------ ToolGateway

    def catalog(self, ctx: ToolContext) -> list[ToolSpec]:
        # P0 不按 ctx 做任何裁剪（scope / access bundle 是 P1）。返回新列表只是
        # 不想让调用方 append 到模块级常量上，元素本身就是 GATEWAY_TOOLS 里那几个。
        return list(GATEWAY_TOOLS)

    async def call(self, ctx: ToolContext, req: ToolCallRequest) -> ToolResult:
        started = time.perf_counter()
        budget = self._default_timeout_sec
        try:
            self._check_token(ctx)

            spec = self._specs.get(req.name)
            if spec is None:
                raise ToolFailure(
                    ToolErrorCode.not_found,
                    f"没有名为 {req.name!r} 的工具；可用的是 {sorted(self._specs)}",
                )
            impl = self._impls.get(req.name)
            if impl is None:
                raise ToolFailure(ToolErrorCode.not_found, f"工具 {req.name!r} 在本次运行里没有实现")

            try:
                args = validate_arguments(spec.parameters, req.arguments)
            except SchemaViolation as exc:
                raise ToolFailure(ToolErrorCode.invalid_args, str(exc)) from exc

            budget = self._budget(req.name, args)
            outcome = await asyncio.wait_for(impl(self._env(ctx), ctx, args), budget)
        except ToolFailure as exc:
            return self._failed(req, exc.code, exc.message, started)
        except TimeoutError:
            return self._failed(
                req, ToolErrorCode.timeout, f"工具 {req.name} 超过 {budget:g}s 没返回", started
            )
        except asyncio.CancelledError:
            raise                       # 调用方在取消，别把它变成一个「结果」
        # 这个宽 except 是 §3.2「永远不抛异常给调用方」的落点，不是漏网：
        # 工具实现里任何没想到的异常都必须变成一个 ToolResult，不能把 worker 打断。
        except Exception as exc:
            return self._failed(
                req,
                ToolErrorCode.upstream,
                f"工具 {req.name} 内部错误：{type(exc).__name__}: {exc}",
                started,
            )
        return self._ok(req, outcome, started)

    # ------------------------------------------------------------------ 附加（非契约）

    def register_task(self, task_id: str, session_token: str) -> None:
        """登记某个 task 的 session_token。没登记过的 task 调工具一律 denied。"""
        if not session_token:
            raise ValueError("session_token 不能为空")
        self._tokens[task_id] = session_token

    def unregister_task(self, task_id: str) -> None:
        self._tokens.pop(task_id, None)

    def sandbox_id_of(self, task_id: str) -> str | None:
        """这个 task 现在用的沙箱（还没建就是 None）。"""
        return self._sandbox_ids.get(task_id)

    async def release_task(self, task_id: str) -> None:
        """释放这个 task 的沙箱并撤掉 token（!stop / 任务收尾时用，幂等）。"""
        self.unregister_task(task_id)
        self._acquire_locks.pop(task_id, None)
        sandbox_id = self._sandbox_ids.pop(task_id, None)
        if sandbox_id is not None and self._sandbox is not None:
            await self._sandbox.release(sandbox_id)

    # ------------------------------------------------------------------ 内部

    def _check_token(self, ctx: ToolContext) -> None:
        expected = (
            self._token_resolver(ctx.task_id)
            if self._token_resolver is not None
            else self._tokens.get(ctx.task_id)
        )
        if not expected:
            raise ToolFailure(ToolErrorCode.denied, f"task {ctx.task_id} 没有登记 session_token")
        if not ctx.session_token:
            raise ToolFailure(ToolErrorCode.denied, "调用没带 session_token")
        # compare_digest 而不是 ==：token 是随机 32 hex，别给旁路留缝。
        # 先 encode 成 bytes：compare_digest 收 str 时只接受纯 ASCII，
        # 拿到一个带中文的伪造 token 会抛 TypeError，那就被外层兜成 upstream 了 —— 该是 denied。
        if not secrets.compare_digest(expected.encode("utf-8"), ctx.session_token.encode("utf-8")):
            raise ToolFailure(ToolErrorCode.denied, f"session_token 与 task {ctx.task_id} 不匹配")

    def _budget(self, name: str, args: dict[str, Any]) -> float:
        # §3.3：默认 DEFAULT_TOOL_TIMEOUT_SEC(60)；run_python 用请求里的 timeout_sec。
        if name != "run_python":
            return self._default_timeout_sec
        return float(args.get("timeout_sec", 120)) + self._run_python_grace_sec

    def _env(self, ctx: ToolContext) -> ToolEnv:
        async def acquire() -> str:
            return await self._acquire_sandbox(ctx.task_id)

        def current() -> str | None:
            return self._sandbox_ids.get(ctx.task_id)

        return ToolEnv(
            platform=self._platform,
            sandbox=self._sandbox,
            acquire_sandbox=acquire,
            current_sandbox_id=current,
        )

    async def _acquire_sandbox(self, task_id: str) -> str:
        """一个 task 一个沙箱，有就复用。并发的两个 tool_call 不会各建一个。"""
        existing = self._sandbox_ids.get(task_id)
        if existing is not None:
            return existing
        if self._sandbox is None:
            raise ToolFailure(ToolErrorCode.sandbox, "本次运行没有可用沙箱")

        lock = self._acquire_locks.setdefault(task_id, asyncio.Lock())
        async with lock:
            existing = self._sandbox_ids.get(task_id)
            if existing is not None:
                return existing
            sandbox_id = await self._sandbox.acquire(task_id, self._sandbox_spec)
            self._sandbox_ids[task_id] = sandbox_id
            return sandbox_id

    def _ok(self, req: ToolCallRequest, outcome: ToolOutcome, started: float) -> ToolResult:
        return ToolResult(
            call_id=req.call_id,
            name=req.name,
            ok=True,
            content=_clip(outcome.content),
            data=outcome.data,
            artifacts=list(outcome.artifacts),
            duration_ms=_ms(started),
        )

    def _failed(
        self, req: ToolCallRequest, code: ToolErrorCode, message: str, started: float
    ) -> ToolResult:
        return ToolResult(
            call_id=req.call_id,
            name=req.name,
            ok=False,
            content=_clip(message),     # content 也给上：模型只读 content 时不至于一片空白
            error=ToolError(code=code, message=message),
            duration_ms=_ms(started),
        )


def _ms(started: float) -> int:
    return int((time.perf_counter() - started) * 1000)
