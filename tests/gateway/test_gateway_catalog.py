"""`catalog` 与工具目录的完整性（owner: T3）。

§3.2：「P0 = GATEWAY_TOOLS 原样」。§3.1 又写明这 5 个工具的名字与 schema 已冻结，
所以「目录里有的都实现了、实现了的都在目录里」得有人常驻盯着 —— 少一个的表现
不是报错，是模型调它时拿到 not_found，然后在日志里当成模型的问题。
"""
from aite.contracts import GATEWAY_TOOLS
from aite.tools import TOOL_IMPLS


def test_catalog_is_gateway_tools_verbatim(gateway, ctx):
    assert gateway.catalog(ctx) == GATEWAY_TOOLS


def test_catalog_hands_back_a_fresh_list(gateway, ctx):
    """返回的是新列表：调用方 append 不该污染模块级常量。"""
    before = len(GATEWAY_TOOLS)
    catalog = gateway.catalog(ctx)
    catalog.append(catalog[0])
    assert len(GATEWAY_TOOLS) == before
    assert gateway.catalog(ctx) is not catalog


def test_catalog_does_not_depend_on_ctx(gateway, ctx):
    """P0 不按 ctx 裁剪（scope / access bundle 是 P1）。"""
    other = ctx.model_copy(update={"chat_id": "oc_other", "task_id": "task-other"})
    assert gateway.catalog(other) == gateway.catalog(ctx)


def test_every_frozen_tool_has_an_implementation():
    assert set(TOOL_IMPLS) == {t.name for t in GATEWAY_TOOLS}


def test_the_five_p0_tool_names_are_exactly_these():
    """名字冻结（§3.1）。改一个名字 = 改契约，必须显式过这条。"""
    assert [t.name for t in GATEWAY_TOOLS] == [
        "read_group_history",
        "read_document",
        "download_attachment",
        "run_python",
        "list_files",
    ]
