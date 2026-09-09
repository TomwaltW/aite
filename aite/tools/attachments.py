"""`download_attachment`（owner: T3）—— 把本次消息的附件下到沙箱 `/work/in/`。

两段两种错误码，别混（§3.3）：
- 从平台下载失败（含「压根没有附件上下文」）→ upstream。
  §3.3 原文：「群里发的附件 adapter 下载失败 → `download_attachment` 工具返回 upstream」。
- 下来了但写不进沙箱 → sandbox。

文件名只从 `file_key` 推。§3.1 冻结的 schema 里只有 `file_key` 一个参数，拿不到原始
文件名；而 file_key 是平台给的不透明串，直接当路径用会带上 `/`、`..` 之类，
所以先过一遍白名单清洗。
"""
import re
from pathlib import PurePosixPath
from typing import Any

from ..contracts import ToolContext, ToolErrorCode
from .base import ToolEnv, ToolFailure, ToolOutcome, require_platform, require_sandbox

__all__ = ["download_attachment"]

#: 只留字母数字、点、下划线、连字符和汉字，其余一律折成下划线。
_UNSAFE = re.compile(r"[^0-9A-Za-z._一-鿿-]+")
_MAX_NAME = 120
_INBOX = "/work/in"


def _safe_name(file_key: str) -> str:
    name = PurePosixPath(file_key).name or file_key
    name = _UNSAFE.sub("_", name).strip("._")
    return (name or "attachment")[:_MAX_NAME]


async def download_attachment(
    env: ToolEnv, ctx: ToolContext, args: dict[str, Any]
) -> ToolOutcome:
    platform = require_platform(env)
    sandbox = require_sandbox(env)
    file_key = args["file_key"]

    message_id = ctx.attachments_message_id
    if not message_id:
        raise ToolFailure(
            ToolErrorCode.upstream,
            "本次消息没有附件（attachments_message_id 为空），没有东西可下载",
        )

    try:
        blob = await platform.download_file(message_id, file_key)
    except Exception as exc:
        raise ToolFailure(ToolErrorCode.upstream, f"下载附件失败（{file_key}）：{exc}") from exc

    path = f"{_INBOX}/{_safe_name(file_key)}"
    try:
        sandbox_id = await env.acquire_sandbox()
        await sandbox.put_file(sandbox_id, path, blob)
    except Exception as exc:
        raise ToolFailure(ToolErrorCode.sandbox, f"把附件写进沙箱失败（{path}）：{exc}") from exc

    return ToolOutcome(
        content=f"附件已下载到沙箱：{path}（{len(blob)} 字节）",
        data={"path": path, "size": len(blob), "file_key": file_key},
    )
