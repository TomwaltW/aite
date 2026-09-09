"""FakeToolGateway 自测：契约那套顺序、六个错误码、以及「过滤归 Gateway」这道分工。

对应 §3.2 ToolGateway 的注释与 §2.2 B5。
"""
from __future__ import annotations

import pytest

from aite.contracts import GATEWAY_TOOLS, ToolCallRequest, ToolContext, ToolErrorCode
from aite.contracts.ports import DocumentContent
from aite.testing import FakePlatform, FakeSandbox, FakeToolGateway, history_message
from aite.testing.jsonschema_mini import SchemaError, validate

CTX = ToolContext(
    tenant_id="default",
    workspace_id="cli_app",
    chat_id="oc_1",
    session_id="s1",
    task_id="t1",
    session_token="tok",
    thread_id="om_1",
    attachments_message_id="om_1",
)


def req(name, **arguments):
    return ToolCallRequest(call_id="c1", name=name, arguments=arguments)


def make_gateway(**kw):
    platform = kw.pop("platform", None) or FakePlatform()
    sandbox = kw.pop("sandbox", None) or FakeSandbox()
    return FakeToolGateway(platform=platform, sandbox=sandbox, **kw), platform, sandbox


def test_catalog_is_gateway_tools_verbatim():
    """§3.2：P0 的 catalog = GATEWAY_TOOLS 原样。"""
    gw, _, _ = make_gateway()
    assert [t.name for t in gw.catalog(CTX)] == [t.name for t in GATEWAY_TOOLS]


async def test_unknown_tool_is_not_found():
    gw, _, _ = make_gateway()
    r = await gw.call(CTX, req("no_such_tool"))
    assert r.ok is False and r.error.code is ToolErrorCode.not_found


async def test_bad_args_is_invalid_args():
    gw, _, _ = make_gateway()
    r = await gw.call(CTX, req("read_document"))                 # 少了必填的 url_or_token
    assert r.ok is False and r.error.code is ToolErrorCode.invalid_args
    assert "url_or_token" in r.error.message


async def test_wrong_token_is_denied_before_anything_else():
    """顺序是「先校验 session_token」——连工具存不存在都还没查。"""
    gw, _, _ = make_gateway(expected_token="right")
    r = await gw.call(CTX, req("no_such_tool"))
    assert r.ok is False and r.error.code is ToolErrorCode.denied


async def test_platform_failure_becomes_upstream_not_an_exception():
    """§3.2：永远不抛异常给调用方。"""
    gw, _, _ = make_gateway()
    r = await gw.call(CTX, req("read_document", url_or_token="不存在"))
    assert r.ok is False and r.error.code is ToolErrorCode.upstream


async def test_sandbox_failure_becomes_sandbox_code():
    sandbox = FakeSandbox(exec_script=[{"error": "Docker 不可用"}])
    gw, _, _ = make_gateway(sandbox=sandbox)
    r = await gw.call(CTX, req("run_python", code="print(1)"))
    assert r.ok is False and r.error.code is ToolErrorCode.sandbox


async def test_read_group_history_drops_non_human():
    """PlatformPort.read_history 不过滤，过滤在这一层 —— 05_history_summary 验的就是它。"""
    platform = FakePlatform(history=[
        history_message("om_1", "人说的", sender_name="Alice"),
        history_message("om_2", "机器人播报", sender_kind="bot"),
        history_message("om_3", "应用推的", sender_kind="app"),
    ])
    gw, _, _ = make_gateway(platform=platform)
    r = await gw.call(CTX, req("read_group_history", limit=50))
    assert r.ok is True
    assert "[om_1] Alice: 人说的" in r.content
    assert "机器人播报" not in r.content and "应用推的" not in r.content
    assert r.data == {"count": 1, "dropped": 2}


async def test_read_group_history_applies_schema_default_limit():
    """limit 不给时用 schema 里的 default=50，而不是报参数缺失。"""
    platform = FakePlatform(history=[history_message(f"om_{i}", str(i)) for i in range(60)])
    gw, _, _ = make_gateway(platform=platform)
    r = await gw.call(CTX, req("read_group_history"))
    assert r.ok is True and len(r.content.splitlines()) == 50


async def test_read_group_history_thread_only_uses_ctx_thread():
    platform = FakePlatform(history=[
        history_message("om_1", "顶层"),
        history_message("om_2", "话题里", thread_id="om_1"),
    ])
    gw, _, _ = make_gateway(platform=platform)
    r = await gw.call(CTX, req("read_group_history", thread_only=True))
    assert "om_2" in r.content and "om_1]" not in r.content


async def test_read_document_returns_title_in_content():
    platform = FakePlatform(
        documents={"tok": DocumentContent(title="Q3 交付计划", text="## 里程碑", url="u")}
    )
    gw, _, _ = make_gateway(platform=platform)
    r = await gw.call(CTX, req("read_document", url_or_token="tok"))
    assert r.ok is True and "Q3 交付计划" in r.content
    assert r.data["title"] == "Q3 交付计划"


async def test_download_attachment_lands_in_work_in():
    platform = FakePlatform(files={("om_1", "fk_csv"): b"month,amount"})
    gw, _, sandbox = make_gateway(platform=platform)
    r = await gw.call(CTX, req("download_attachment", file_key="fk_csv"))
    assert r.ok is True and r.data["path"] == "/work/in/fk_csv"
    sid = next(iter(sandbox.boxes))
    assert await sandbox.get_file(sid, "/work/in/fk_csv") == b"month,amount"


async def test_run_python_reuses_the_same_sandbox_per_task():
    sandbox = FakeSandbox()
    gw, _, _ = make_gateway(sandbox=sandbox)
    await gw.call(CTX, req("run_python", code="a=1"))
    await gw.call(CTX, req("run_python", code="b=2"))
    assert sandbox.calls.count("acquire") == 1


async def test_run_python_reports_exit_code_and_files():
    sandbox = FakeSandbox(exec_script=[{"match": "savefig", "stdout": "done",
                                        "writes": {"/work/out.png": "builtin:png"}}])
    gw, _, _ = make_gateway(sandbox=sandbox)
    r = await gw.call(CTX, req("run_python", code="plt.savefig('/work/out.png')"))
    assert r.ok is True
    assert "exit_code=0" in r.content and "done" in r.content
    assert r.data["files_out"] == ["/work/out.png"]


async def test_list_files_shows_what_was_downloaded():
    platform = FakePlatform(files={("om_1", "fk"): b"x"})
    gw, _, _ = make_gateway(platform=platform)
    await gw.call(CTX, req("download_attachment", file_key="fk"))
    r = await gw.call(CTX, req("list_files"))
    assert r.content == "/work/in/fk"


async def test_content_is_truncated_to_contract_limit():
    platform = FakePlatform(
        history=[history_message(f"om_{i}", "长" * 200) for i in range(200)]
    )
    gw, _, _ = make_gateway(platform=platform)
    r = await gw.call(CTX, req("read_group_history", limit=200))
    assert len(r.content) <= 12000                # MAX_TOOL_CONTENT_CHARS


async def test_results_are_recorded_for_assertions():
    gw, _, _ = make_gateway()
    await gw.call(CTX, req("list_files"))
    assert gw.count("list_files") == 1
    assert len(gw.results_of("list_files")) == 1


# --- 迷你 schema 校验器本身 --------------------------------------------------

def test_mini_schema_rejects_bool_as_integer():
    """bool 是 int 的子类，不排掉的话 limit: true 会被当成合法整数。"""
    schema = {"type": "object", "properties": {"limit": {"type": "integer"}}}
    with pytest.raises(SchemaError, match="期望 integer"):
        validate({"limit": True}, schema)


def test_mini_schema_enforces_bounds_and_item_counts():
    items = {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 2}
    schema = {"type": "object", "properties": {"items": items}, "required": ["items"]}
    assert validate({"items": ["a"]}, schema) == {"items": ["a"]}
    with pytest.raises(SchemaError, match="最多 2 项"):
        validate({"items": ["a", "b", "c"]}, schema)
    with pytest.raises(SchemaError, match="缺少必填参数"):
        validate({}, schema)
