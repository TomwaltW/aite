"""`read_document`（owner: T3）—— 读一篇飞书云文档，返回 markdown 文本。

真正把文档转成 markdown 的是 `PlatformPort.read_document`（T1 的 adapter）；
这一层只负责把它包成给模型看的文本，并把平台侧的任何异常翻成 upstream（§3.3）。
"""
from typing import Any

from ..contracts import ToolContext, ToolErrorCode
from .base import ToolEnv, ToolFailure, ToolOutcome, require_platform

__all__ = ["read_document"]


async def read_document(env: ToolEnv, ctx: ToolContext, args: dict[str, Any]) -> ToolOutcome:
    platform = require_platform(env)
    ref = args["url_or_token"]

    try:
        doc = await platform.read_document(ref)
    except Exception as exc:
        raise ToolFailure(ToolErrorCode.upstream, f"读取文档失败（{ref}）：{exc}") from exc

    content = f"# {doc.title}\n\n来源：{doc.url}\n\n{doc.text}"
    return ToolOutcome(
        content=content,
        data={"title": doc.title, "url": doc.url, "chars": len(doc.text)},
    )
