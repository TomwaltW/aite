"""`validate_arguments` 的单元测试（owner: T3）。

§3.2 的第三步是「按 ToolSpec.parameters 校验 arguments」。这一份直接拿 §3.1 冻结的
那 5 个 schema 当输入，别自己编 schema —— 编的话测的是校验器，不是契约。
"""
import pytest

from aite.contracts import GATEWAY_TOOLS
from aite.gateway import SchemaViolation, validate_arguments

SCHEMAS = {t.name: t.parameters for t in GATEWAY_TOOLS}


def check(tool: str, args: dict) -> dict:
    return validate_arguments(SCHEMAS[tool], args)


# --- 填默认值 ---------------------------------------------------------------

def test_defaults_come_from_the_schema():
    """§3.1：limit 默认 50、thread_only 默认 False。"""
    assert check("read_group_history", {}) == {"limit": 50, "thread_only": False}


def test_run_python_default_timeout_is_120():
    assert check("run_python", {"code": "print(1)"}) == {"code": "print(1)", "timeout_sec": 120}


def test_list_files_takes_no_arguments():
    assert check("list_files", {}) == {}


def test_given_values_win_over_defaults():
    assert check("read_group_history", {"limit": 3}) == {"limit": 3, "thread_only": False}


# --- 必填 -------------------------------------------------------------------

@pytest.mark.parametrize(
    ("tool", "args"),
    [
        ("read_document", {}),
        ("download_attachment", {}),
        ("run_python", {}),
        ("run_python", {"timeout_sec": 10}),
    ],
)
def test_missing_required_is_rejected(tool, args):
    with pytest.raises(SchemaViolation, match="必填"):
        check(tool, args)


# --- 类型 -------------------------------------------------------------------

@pytest.mark.parametrize(
    ("tool", "args"),
    [
        ("read_group_history", {"limit": "50"}),      # 不做隐式转换
        ("read_group_history", {"limit": 12.5}),
        ("read_group_history", {"limit": None}),
        ("read_group_history", {"thread_only": "true"}),
        ("read_document", {"url_or_token": 42}),
        ("run_python", {"code": ["print(1)"]}),
        ("run_python", {"code": "x", "timeout_sec": "30"}),
    ],
)
def test_wrong_types_are_rejected(tool, args):
    with pytest.raises(SchemaViolation):
        check(tool, args)


def test_bool_is_not_an_integer():
    """bool 是 int 的子类，不显式排掉的话 True 会被当成合法的 limit。"""
    with pytest.raises(SchemaViolation):
        check("read_group_history", {"limit": True})


def test_integer_is_not_a_boolean():
    with pytest.raises(SchemaViolation):
        check("read_group_history", {"thread_only": 1})


# --- 上下界 -----------------------------------------------------------------

@pytest.mark.parametrize("limit", [0, -1, 201, 10_000])
def test_limit_outside_1_200_is_rejected(limit):
    with pytest.raises(SchemaViolation):
        check("read_group_history", {"limit": limit})


@pytest.mark.parametrize("limit", [1, 50, 200])
def test_limit_on_the_boundary_is_accepted(limit):
    assert check("read_group_history", {"limit": limit})["limit"] == limit


@pytest.mark.parametrize("timeout_sec", [0, -5, 301])
def test_run_python_timeout_outside_1_300_is_rejected(timeout_sec):
    with pytest.raises(SchemaViolation):
        check("run_python", {"code": "x", "timeout_sec": timeout_sec})


@pytest.mark.parametrize("timeout_sec", [1, 120, 300])
def test_run_python_timeout_on_the_boundary_is_accepted(timeout_sec):
    assert check("run_python", {"code": "x", "timeout_sec": timeout_sec})["timeout_sec"] == timeout_sec


# --- 多余参数（本层策略，比 JSON Schema 默认更严）---------------------------

@pytest.mark.parametrize(
    ("tool", "args"),
    [
        ("read_group_history", {"limt": 5}),                       # 打错字
        ("list_files", {"path": "/work"}),                          # 这个工具不收参数
        ("run_python", {"code": "x", "language": "python"}),
    ],
)
def test_unknown_arguments_are_rejected(tool, args):
    with pytest.raises(SchemaViolation, match="不认识的参数"):
        check(tool, args)


def test_unknown_argument_message_lists_what_is_available():
    """模型收到的错误要能照着改，所以把可用参数名带上。"""
    with pytest.raises(SchemaViolation, match="limit"):
        check("read_group_history", {"limt": 5})


# --- arguments 本身的形状 ---------------------------------------------------

@pytest.mark.parametrize("args", [None, [], "limit=5", 7])
def test_arguments_must_be_an_object(args):
    with pytest.raises(SchemaViolation):
        validate_arguments(SCHEMAS["list_files"], args)


# --- 嵌套关键字（GATEWAY_TOOLS 没用到，但 CHECKLIST_TOOLS 用了，校验器得撑得住）---

NESTED = {
    "type": "object",
    "properties": {
        "items": {
            "type": "array",
            "items": {"type": "string"},
            "minItems": 1,
            "maxItems": 3,
        },
        "who": {
            "type": "object",
            "properties": {"id": {"type": "string"}, "vip": {"type": "boolean", "default": False}},
            "required": ["id"],
        },
        "mode": {"type": "string", "enum": ["fast", "slow"]},
    },
    "required": ["items"],
}


def test_nested_array_and_object_are_validated():
    out = validate_arguments(NESTED, {"items": ["a", "b"], "who": {"id": "ou_1"}, "mode": "fast"})
    assert out == {"items": ["a", "b"], "who": {"id": "ou_1", "vip": False}, "mode": "fast"}


@pytest.mark.parametrize(
    "args",
    [
        {"items": []},                              # minItems
        {"items": ["a", "b", "c", "d"]},            # maxItems
        {"items": [1, 2]},                          # items 的类型
        {"items": ["a"], "who": {}},                # 嵌套必填
        {"items": ["a"], "mode": "medium"},         # enum
    ],
)
def test_nested_violations_are_rejected(args):
    with pytest.raises(SchemaViolation):
        validate_arguments(NESTED, args)
