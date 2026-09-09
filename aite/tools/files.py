"""`list_files`（owner: T3）—— 列出沙箱 `/work` 下的文件。

刻意**不**建沙箱：模型经常在还没跑过任何代码时先问一句「有什么文件」，为这一问
起一个容器纯属浪费。没有容器就等于 /work 是空的，直接如实回答。
"""
from typing import Any

from ..contracts import ToolContext, ToolErrorCode
from ..sandbox import WORKDIR
from .base import ToolEnv, ToolFailure, ToolOutcome, require_sandbox

__all__ = ["list_files"]

_EMPTY = f"{WORKDIR} 下还没有文件。"


async def list_files(env: ToolEnv, ctx: ToolContext, args: dict[str, Any]) -> ToolOutcome:
    sandbox = require_sandbox(env)

    sandbox_id = env.current_sandbox_id()
    if sandbox_id is None:
        return ToolOutcome(content=f"沙箱还没启动，{_EMPTY}", data={"files": [], "count": 0})

    try:
        paths = await sandbox.list_files(sandbox_id)
    except Exception as exc:
        raise ToolFailure(ToolErrorCode.sandbox, f"列 {WORKDIR} 失败：{exc}") from exc

    if not paths:
        return ToolOutcome(content=_EMPTY, data={"files": [], "count": 0})

    listing = "\n".join(paths)
    return ToolOutcome(
        content=f"{WORKDIR} 下有 {len(paths)} 个文件：\n{listing}",
        data={"files": list(paths), "count": len(paths)},
    )
