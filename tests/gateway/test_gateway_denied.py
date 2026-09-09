"""session_token 校验的边角（owner: T3）。

单独一份是因为这条闸是 Gateway 唯一的鉴权面：`Task.session_token` 是随机 32 hex，
校验一旦有缝，任何拿到 chat_id / task_id 的调用方就能借别的任务的壳跑 run_python。
B5 只要求「不匹配 → denied」，这里把「不匹配」的各种长相都摆上。
"""
from aite.contracts import ToolCallRequest, ToolErrorCode


def req(name: str = "list_files") -> ToolCallRequest:
    return ToolCallRequest(call_id="call-1", name=name)


async def code_of(gateway, ctx) -> ToolErrorCode | None:
    result = await gateway.call(ctx, req())
    return result.error.code if result.error else None


async def test_the_registered_token_is_accepted(gateway, ctx):
    """先证明这道闸不是「一律拒」，否则下面每条都恒真。"""
    assert (await gateway.call(ctx, req())).ok is True


async def test_prefix_of_the_real_token_is_denied(gateway, ctx):
    """前缀相同不算匹配 —— 长度也得咬住。"""
    short = ctx.model_copy(update={"session_token": ctx.session_token[:16]})
    assert await code_of(gateway, short) == ToolErrorCode.denied


async def test_token_with_trailing_whitespace_is_denied(gateway, ctx):
    padded = ctx.model_copy(update={"session_token": ctx.session_token + " "})
    assert await code_of(gateway, padded) == ToolErrorCode.denied


async def test_case_flipped_token_is_denied(gateway, ctx):
    flipped = ctx.model_copy(update={"session_token": ctx.session_token.upper()})
    assert await code_of(gateway, flipped) == ToolErrorCode.denied


async def test_non_ascii_token_is_denied_not_upstream(gateway, ctx):
    """compare_digest 收 str 时只接受纯 ASCII，直接传会抛 TypeError。

    那样就被最外层兜成 upstream 了 —— 一个伪造的 token 必须是 denied，
    不能表现成「外部系统出错」，否则 §3.3 的重试逻辑会去重试一个鉴权失败。
    """
    weird = ctx.model_copy(update={"session_token": "口" * 32})
    assert await code_of(gateway, weird) == ToolErrorCode.denied


async def test_token_resolver_exception_does_not_leak(make_gateway, ctx):
    """resolver 是外部接来的（TΩ 把它接到 SessionStore 上），它炸了也不能抛出去。"""

    def boom(task_id: str) -> str | None:
        raise RuntimeError("SQLite 锁了")

    gateway = make_gateway(register=False, token_resolver=boom)
    result = await gateway.call(ctx, req())

    assert result.ok is False
    assert result.error.code == ToolErrorCode.upstream      # 确实是外部系统出错
    assert "SQLite" in result.error.message
