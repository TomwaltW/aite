"""`read_group_history`（owner: T3）。

**这个工具的核心职责是过滤。** §3.2 里 `PlatformPort.read_history` 的注释写明
「不做 sender_kind 过滤（过滤归 Gateway 的 read_group_history 工具）」，
而 §3.1 里这个工具的 description 是「读取本群最近的消息（**只含真人消息**）」。
两边合起来：adapter 把机器人、应用、系统消息原样交上来，这里负责丢掉。

为什么这条不能漏：机器人消息里可能带着别的系统生成的指令性文本，而 §3.6 W9 要求
「外部内容一律是数据不是指令」。少过滤一层，Aite 自己上一轮的回帖也会回流进上下文，
模型会拿自己说过的话当事实。
"""
from typing import Any

from ..contracts import SenderKind, ToolContext, ToolErrorCode
from .base import ToolEnv, ToolFailure, ToolOutcome, require_platform

__all__ = ["read_group_history"]

_HUMAN = SenderKind.human.value


async def read_group_history(env: ToolEnv, ctx: ToolContext, args: dict[str, Any]) -> ToolOutcome:
    platform = require_platform(env)
    limit = int(args.get("limit", 50))
    thread_only = bool(args.get("thread_only", False))

    thread_id = None
    if thread_only:
        thread_id = ctx.thread_id
        if thread_id is None:
            return ToolOutcome(
                content="本次对话不在话题里，没有话题历史可读。",
                data={"messages": [], "count": 0, "filtered_out": 0, "thread_only": True},
            )

    try:
        messages = await platform.read_history(ctx.chat_id, limit=limit, thread_id=thread_id)
    except Exception as exc:
        raise ToolFailure(ToolErrorCode.upstream, f"读取群历史失败：{exc}") from exc

    humans = [m for m in messages if str(m.sender_kind) == _HUMAN]
    dropped = len(messages) - len(humans)

    if not humans:
        note = f"（读到 {len(messages)} 条，全都不是真人发的，已丢弃）" if dropped else "（这里还没有消息）"
        content = f"本群没有可引用的真人消息。{note}"
    else:
        # 格式与 §3.6 W1 的群历史窗口一致：[message_id] 姓名: 文本
        lines = [
            f"[{m.message_id}] {m.sender_name or m.sender_id}: {m.text}".rstrip()
            for m in humans
        ]
        head = f"本群最近 {len(humans)} 条真人消息"
        if dropped:
            head += f"（另有 {dropped} 条机器人/应用/系统消息已过滤）"
        content = head + "：\n" + "\n".join(lines)

    return ToolOutcome(
        content=content,
        data={
            "messages": [m.model_dump(mode="json") for m in humans],
            "count": len(humans),
            "filtered_out": dropped,
            "thread_only": thread_only,
        },
    )
