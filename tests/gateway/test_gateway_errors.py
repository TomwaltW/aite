"""§2.2 B5：Gateway 的错误面（owner: T3）。

B5 原文四条：
    未知工具 → not_found；参数不合 schema → invalid_args；
    工具超时 → timeout；session_token 不匹配 → denied

外加派单里对应表剩下的两条（upstream / sandbox）、§3.2 那句「永远不抛异常给调用方」，
以及 §3.2 注释里的**执行顺序**：
    校验 session_token → 查工具存在 → 校验 arguments → 执行（带超时）→ 截断 content
顺序错了错误码就会串味（比如带错 token 调不存在的工具应该是 denied 而不是 not_found），
所以顺序本身也单独钉两条。
"""
import asyncio

import pytest

from aite.contracts import (
    MAX_TOOL_CONTENT_CHARS,
    ExecResult,
    ToolCallRequest,
    ToolErrorCode,
)
from aite.tools import ToolOutcome


def req(name: str, **arguments) -> ToolCallRequest:
    return ToolCallRequest(call_id="call-1", name=name, arguments=arguments)


def assert_failed(result, code: ToolErrorCode, *, name: str = "call-1") -> None:
    assert result.ok is False
    assert result.error is not None
    assert result.error.code == code
    assert result.error.message
    assert result.content, "失败时 content 也要有话说，模型可能只读 content"
    assert result.call_id == name


# --- B5 四条 ---------------------------------------------------------------

async def test_unknown_tool_is_not_found(gateway, ctx):
    result = await gateway.call(ctx, req("read_the_room"))
    assert_failed(result, ToolErrorCode.not_found)
    assert result.name == "read_the_room"


async def test_bad_arguments_are_invalid_args(gateway, ctx):
    result = await gateway.call(ctx, req("read_group_history", limit=9999))
    assert_failed(result, ToolErrorCode.invalid_args)


async def test_missing_required_argument_is_invalid_args(gateway, ctx):
    result = await gateway.call(ctx, req("read_document"))
    assert_failed(result, ToolErrorCode.invalid_args)


async def test_wrong_session_token_is_denied(gateway, ctx):
    forged = ctx.model_copy(update={"session_token": "f" * 32})
    result = await gateway.call(forged, req("list_files"))
    assert_failed(result, ToolErrorCode.denied)


async def test_unregistered_task_is_denied(make_gateway, ctx):
    """失败关闭：没登记过 token 的 task 一律 denied，不是「没登记就放过」。"""
    gateway = make_gateway(register=False)
    result = await gateway.call(ctx, req("list_files"))
    assert_failed(result, ToolErrorCode.denied)


async def test_empty_session_token_is_denied(gateway, ctx):
    result = await gateway.call(ctx.model_copy(update={"session_token": ""}), req("list_files"))
    assert_failed(result, ToolErrorCode.denied)


async def test_token_of_another_task_is_denied(gateway, ctx):
    """token 对得上但 task_id 不是它的 —— 一样不放过。"""
    other = ctx.model_copy(update={"task_id": "task-someone-else"})
    result = await gateway.call(other, req("list_files"))
    assert_failed(result, ToolErrorCode.denied)


async def test_tool_timeout_is_timeout(make_gateway, ctx, platform):
    """默认预算（DEFAULT_TOOL_TIMEOUT_SEC）那条路：平台迟迟不返回。"""
    gateway = make_gateway(default_timeout_sec=0.2)
    platform.delay_sec = 30

    result = await gateway.call(ctx, req("read_group_history"))

    assert_failed(result, ToolErrorCode.timeout)
    assert result.duration_ms < 5_000


async def test_run_python_timeout_from_inside_the_sandbox(gateway, ctx, sandbox):
    """沙箱内 coreutils timeout 把退出码打成 124 → 工具翻成 timeout。"""
    sandbox.exec_result = ExecResult(
        exit_code=124, stdout="", stderr="[aite] 执行超过 5s 上限，已在沙箱内终止", duration_ms=5010
    )

    result = await gateway.call(ctx, req("run_python", code="import time; time.sleep(99)", timeout_sec=5))

    assert_failed(result, ToolErrorCode.timeout)
    assert "5s" in result.error.message


async def test_run_python_timeout_from_the_outer_budget(make_gateway, ctx, sandbox):
    """Docker daemon 卡住、连 exec 都不返回时，外层 wait_for 兜底。"""
    gateway = make_gateway(run_python_grace_sec=0.05)
    sandbox.exec_delay_sec = 30

    result = await gateway.call(ctx, req("run_python", code="print(1)", timeout_sec=1))

    assert_failed(result, ToolErrorCode.timeout)
    assert result.duration_ms < 5_000


async def test_run_python_budget_follows_the_request_not_the_default(make_gateway, ctx, sandbox):
    """§3.3：run_python 用请求里的 timeout_sec，不是 60s 的默认值。"""
    gateway = make_gateway(default_timeout_sec=0.05, run_python_grace_sec=0.5)
    sandbox.exec_delay_sec = 0.2      # 比默认预算长，比 timeout_sec 短

    result = await gateway.call(ctx, req("run_python", code="print(1)", timeout_sec=2))

    assert result.ok is True, result.error


# --- 派单对应表剩下的两条 ---------------------------------------------------

async def test_platform_failure_is_upstream(gateway, ctx, platform):
    platform.read_history_error = RuntimeError("飞书 500")
    result = await gateway.call(ctx, req("read_group_history"))
    assert_failed(result, ToolErrorCode.upstream)
    assert "飞书 500" in result.error.message


async def test_download_failure_is_upstream(gateway, ctx, platform):
    """§3.3：群里发的附件 adapter 下载失败 → download_attachment 返回 upstream。"""
    platform.download_error = RuntimeError("file_key 已过期")
    result = await gateway.call(ctx, req("download_attachment", file_key="file_v3_csv"))
    assert_failed(result, ToolErrorCode.upstream)


async def test_sandbox_acquire_failure_is_sandbox(gateway, ctx, sandbox):
    """§3.3：沙箱创建/执行失败（Docker 不可用等）→ code=sandbox。"""
    sandbox.acquire_error = RuntimeError("Cannot connect to the Docker daemon")
    result = await gateway.call(ctx, req("run_python", code="print(1)"))
    assert_failed(result, ToolErrorCode.sandbox)


async def test_sandbox_exec_failure_is_sandbox(gateway, ctx, sandbox):
    sandbox.exec_error = RuntimeError("容器没了")
    result = await gateway.call(ctx, req("run_python", code="print(1)"))
    assert_failed(result, ToolErrorCode.sandbox)


async def test_missing_sandbox_is_sandbox(make_gateway, ctx):
    gateway = make_gateway(sandbox=None)
    result = await gateway.call(ctx, req("run_python", code="print(1)"))
    assert_failed(result, ToolErrorCode.sandbox)


async def test_missing_platform_is_upstream(make_gateway, ctx):
    gateway = make_gateway(platform=None)
    result = await gateway.call(ctx, req("read_group_history"))
    assert_failed(result, ToolErrorCode.upstream)


# --- §3.2「永远不抛异常给调用方」---------------------------------------------

async def test_unexpected_exception_becomes_a_result(make_gateway, ctx):
    async def explode(env, tool_ctx, args):
        raise ZeroDivisionError("工具实现自己写崩了")

    gateway = make_gateway(tools={"list_files": explode})
    result = await gateway.call(ctx, req("list_files"))

    assert_failed(result, ToolErrorCode.upstream)
    assert "ZeroDivisionError" in result.error.message


async def test_base_exception_from_a_tool_still_propagates(make_gateway, ctx):
    """取消是调用方的意思，吞掉它才是 bug（§3.2 那句约束的是「失败」，不是「取消」）。"""

    async def hang(env, tool_ctx, args):
        await asyncio.sleep(30)

    gateway = make_gateway(tools={"list_files": hang})
    task = asyncio.create_task(gateway.call(ctx, req("list_files")))
    await asyncio.sleep(0.05)
    task.cancel()

    with pytest.raises(asyncio.CancelledError):
        await task


# --- 执行顺序（§3.2 的注释）--------------------------------------------------

async def test_token_is_checked_before_the_tool_lookup(gateway, ctx):
    """带错 token 调一个不存在的工具 → denied，不是 not_found。

    反过来的话，攻击者能靠错误码探出「有哪些工具」。"""
    forged = ctx.model_copy(update={"session_token": "f" * 32})
    result = await gateway.call(forged, req("no_such_tool"))
    assert_failed(result, ToolErrorCode.denied)


async def test_tool_lookup_happens_before_argument_validation(gateway, ctx):
    """不存在的工具 + 一堆乱参数 → not_found，不是 invalid_args。"""
    result = await gateway.call(ctx, req("no_such_tool", nonsense=object.__doc__))
    assert_failed(result, ToolErrorCode.not_found)


async def test_arguments_are_validated_before_the_tool_runs(gateway, ctx, sandbox):
    """参数不合 schema 时工具压根不该被执行 —— 别为一个必然失败的调用起容器。"""
    result = await gateway.call(ctx, req("run_python", code="print(1)", timeout_sec=9999))

    assert_failed(result, ToolErrorCode.invalid_args)
    assert sandbox.exec_requests == []
    assert sandbox.trees == {}


# --- 截断与 ToolResult 的形状 ------------------------------------------------

async def test_content_is_clipped_to_the_cap(make_gateway, ctx):
    async def flood(env, tool_ctx, args):
        return ToolOutcome(content="x" * (MAX_TOOL_CONTENT_CHARS * 3))

    gateway = make_gateway(tools={"list_files": flood})
    result = await gateway.call(ctx, req("list_files"))

    assert result.ok is True
    assert len(result.content) == MAX_TOOL_CONTENT_CHARS
    assert result.content.endswith("[内容已截断]")


async def test_error_message_is_also_clipped(make_gateway, ctx):
    from aite.tools import ToolFailure

    async def shout(env, tool_ctx, args):
        raise ToolFailure(ToolErrorCode.upstream, "长" * (MAX_TOOL_CONTENT_CHARS * 2))

    gateway = make_gateway(tools={"list_files": shout})
    result = await gateway.call(ctx, req("list_files"))

    assert len(result.content) == MAX_TOOL_CONTENT_CHARS


async def test_result_echoes_call_id_and_name(gateway, ctx):
    request = ToolCallRequest(call_id="call-xyz", name="list_files")
    result = await gateway.call(ctx, request)

    assert (result.call_id, result.name) == ("call-xyz", "list_files")
    assert result.duration_ms >= 0
