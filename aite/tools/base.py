"""5 个 Gateway 工具的公共底座（owner: T3）。

工具实现只做「干活」，不构造 `ToolResult`：失败一律 `raise ToolFailure(code, message)`，
由 `P0ToolGateway.call` 统一翻成 `ToolResult(ok=False, error=...)`。§3.2 写明
「永远不抛异常给调用方」—— 那是对 `call` 的要求，工具层往外抛是这个约定的实现方式，
不是违背它。

错误码按 §3.3 / 派单里的对应表分工：
    平台 / 外部系统失败  → upstream
    沙箱创建、执行、读写  → sandbox
    工具自己判定超时      → timeout（`run_python` 见 python_exec.py）
`not_found` / `invalid_args` / `denied` 由 Gateway 在调工具**之前**判掉，工具层不产出。
"""
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any, Protocol

from ..contracts import (
    ArtifactRef,
    PlatformPort,
    SandboxPort,
    ToolContext,
    ToolErrorCode,
)

__all__ = [
    "ToolEnv",
    "ToolFailure",
    "ToolImpl",
    "ToolOutcome",
    "require_platform",
    "require_sandbox",
]


class ToolFailure(Exception):
    """工具执行失败。`code` 直接就是要写进 `ToolError` 的那个码。"""

    def __init__(self, code: ToolErrorCode, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


@dataclass(slots=True)
class ToolOutcome:
    """工具成功时的产出。`content` 由 Gateway 截断到 MAX_TOOL_CONTENT_CHARS。"""

    content: str
    data: dict[str, Any] | None = None
    artifacts: list[ArtifactRef] = field(default_factory=list)


@dataclass(slots=True)
class ToolEnv:
    """一次 `call` 里工具能碰到的东西。

    沙箱不直接给 sandbox_id，而是给两个闭包：Gateway 按 task_id 记着容器，
    `acquire_sandbox()` 是「有就复用、没有才建」，`current_sandbox_id()` 是
    「有就给、没有就 None」—— `list_files` 用后者，免得为了列一个空目录白建个容器。
    """

    platform: PlatformPort | None
    sandbox: SandboxPort | None
    acquire_sandbox: Callable[[], Awaitable[str]]
    current_sandbox_id: Callable[[], str | None]


class ToolImpl(Protocol):
    """工具实现的形状。"""

    async def __call__(
        self, env: ToolEnv, ctx: ToolContext, args: dict[str, Any]
    ) -> ToolOutcome: ...


def require_platform(env: ToolEnv) -> PlatformPort:
    if env.platform is None:
        raise ToolFailure(ToolErrorCode.upstream, "本次运行没有接入平台，这个工具用不了")
    return env.platform


def require_sandbox(env: ToolEnv) -> SandboxPort:
    if env.sandbox is None:
        raise ToolFailure(ToolErrorCode.sandbox, "本次运行没有可用沙箱，这个工具用不了")
    return env.sandbox
