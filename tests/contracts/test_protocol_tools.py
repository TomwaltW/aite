# tests/contracts/test_protocol_tools.py
"""模型侧工具目录的冻结测试（dev-spec-2026-09-09 §3.1 protocol.py / §6 T0 行）。

验收口径来自 §6 T0："ALL_MODEL_TOOLS 名字唯一且每个 parameters.type == 'object'"。
本文件把它扩成机器判据：名字集合与顺序冻结、组成关系、本地/网关分区、
parameters 与 description 逐字冻结、跨契约默认值一致。
断言全部写死期望值，不做"能跑就算过"的软校验。
"""

from typing import Any

from aite.contracts import (
    ALL_MODEL_TOOLS,
    CHECKLIST_TOOLS,
    FINAL_TOOL,
    GATEWAY_TOOLS,
    LOCAL_TOOL_NAMES,
    ExecRequest,
    ToolSpec,
)

# §3.1 冻结的 10 个工具名：4 个 checklist + final + 5 个 gateway。
# 这里只锁名字集合；schema 的逐字冻结在文件末尾的 FROZEN_PARAMETERS。
FROZEN_TOOL_NAMES: set[str] = {
    "checklist_add",
    "checklist_check",
    "checklist_fail",
    "checklist_note",
    "final",
    "read_group_history",
    "read_document",
    "download_attachment",
    "run_python",
    "list_files",
}

# 目录顺序也是冻结的：ALL_MODEL_TOOLS = CHECKLIST_TOOLS + [FINAL_TOOL] + GATEWAY_TOOLS，
# worker 把它整份喂给模型（§7 W1），顺序变化会改变模型看到的提示。
FROZEN_TOOL_ORDER: list[str] = [
    "checklist_add",
    "checklist_check",
    "checklist_fail",
    "checklist_note",
    "final",
    "read_group_history",
    "read_document",
    "download_attachment",
    "run_python",
    "list_files",
]

FROZEN_CHECKLIST_NAMES: set[str] = {
    "checklist_add",
    "checklist_check",
    "checklist_fail",
    "checklist_note",
}

FROZEN_GATEWAY_NAMES: set[str] = {
    "read_group_history",
    "read_document",
    "download_attachment",
    "run_python",
    "list_files",
}


def _names(tools: list[ToolSpec]) -> list[str]:
    """取出工具名列表，保持原顺序。"""
    return [t.name for t in tools]


def test_all_model_tools_names_are_unique() -> None:
    """§6 T0 第一条：名字唯一 —— 同名工具会让 worker 的分派歧义。"""
    names = _names(ALL_MODEL_TOOLS)
    assert len(names) == len(set(names))
    assert len(names) == 10


def test_all_model_tools_parameters_are_json_schema_objects() -> None:
    """§6 T0 第二条：每个 parameters.type == "object"，且带 properties 键。

    OpenAI-compatible 的 function calling 只接受 object 顶层 schema；
    没有 properties 的 object 在部分服务端会被拒。
    """
    assert ALL_MODEL_TOOLS, "工具目录不能为空"
    for tool in ALL_MODEL_TOOLS:
        params: dict[str, Any] = tool.parameters
        assert isinstance(params, dict), f"{tool.name}: parameters 必须是 dict"
        assert params["type"] == "object", f"{tool.name}: parameters.type 必须是 'object'"
        assert "properties" in params, f"{tool.name}: parameters 缺少 'properties' 键"
        assert isinstance(params["properties"], dict), f"{tool.name}: properties 必须是 dict"


def test_frozen_tool_name_set_matches_spec() -> None:
    """名字冻结：与 §3.1 写死的 10 个名字逐字比对（集合相等，多一个少一个都红）。"""
    assert set(_names(ALL_MODEL_TOOLS)) == FROZEN_TOOL_NAMES
    assert set(_names(CHECKLIST_TOOLS)) == FROZEN_CHECKLIST_NAMES
    assert set(_names(GATEWAY_TOOLS)) == FROZEN_GATEWAY_NAMES
    assert FINAL_TOOL.name == "final"


def test_frozen_tool_order_matches_spec() -> None:
    """目录顺序冻结：喂给模型的工具顺序必须与 §3.1 的定义顺序一致。"""
    assert _names(ALL_MODEL_TOOLS) == FROZEN_TOOL_ORDER


def test_all_model_tools_composition_is_checklist_plus_final_plus_gateway() -> None:
    """组成关系：ALL_MODEL_TOOLS == CHECKLIST_TOOLS + [FINAL_TOOL] + GATEWAY_TOOLS，逐个相等。"""
    expected: list[ToolSpec] = [*CHECKLIST_TOOLS, FINAL_TOOL, *GATEWAY_TOOLS]
    assert len(ALL_MODEL_TOOLS) == len(expected) == 10
    for got, want in zip(ALL_MODEL_TOOLS, expected, strict=True):
        # ToolSpec 是 pydantic BaseModel，== 是逐字段比较（name/description/parameters）
        assert got == want
    assert len(CHECKLIST_TOOLS) == 4
    assert len(GATEWAY_TOOLS) == 5


def test_local_tool_names_is_checklist_plus_final() -> None:
    """LOCAL_TOOL_NAMES 恰好 = 4 个 checklist 工具名 + "final"（§7 W2 靠它分派本地/网关）。"""
    assert LOCAL_TOOL_NAMES == FROZEN_CHECKLIST_NAMES | {"final"}
    assert len(LOCAL_TOOL_NAMES) == 5


def test_local_and_gateway_tool_names_are_disjoint() -> None:
    """本地工具名与 Gateway 工具名不相交 —— 否则一个调用会被两条路径同时认领。"""
    gateway_names = set(_names(GATEWAY_TOOLS))
    assert LOCAL_TOOL_NAMES & gateway_names == set()
    # 两边合起来正好铺满整份目录，没有第三类工具
    assert LOCAL_TOOL_NAMES | gateway_names == FROZEN_TOOL_NAMES


def test_required_fields_are_declared_in_properties() -> None:
    """每个 ToolSpec 的 required（若有）必须都出现在 properties 里 —— 防止 schema 自相矛盾。"""
    for tool in ALL_MODEL_TOOLS:
        params: dict[str, Any] = tool.parameters
        required = params.get("required", [])
        assert isinstance(required, list), f"{tool.name}: required 必须是 list"
        properties: dict[str, Any] = params["properties"]
        for field in required:
            assert field in properties, f"{tool.name}: required 里的 {field!r} 未在 properties 中声明"


def test_checklist_tools_required_fields_are_frozen() -> None:
    """4 个 checklist 工具的 required 逐字冻结（worker 本地处理时按这些字段取参）。"""
    required_by_name = {t.name: t.parameters.get("required") for t in CHECKLIST_TOOLS}
    assert required_by_name == {
        "checklist_add": ["items"],
        "checklist_check": ["id"],
        "checklist_fail": ["id", "reason"],
        "checklist_note": ["text"],
    }


def test_gateway_tools_required_fields_are_frozen() -> None:
    """5 个 Gateway 工具的 required 逐字冻结；list_files 无参数、无 required 键。"""
    required_by_name = {t.name: t.parameters.get("required") for t in GATEWAY_TOOLS}
    assert required_by_name == {
        "read_group_history": None,       # limit / thread_only 都有 default，全可选
        "read_document": ["url_or_token"],
        "download_attachment": ["file_key"],
        "run_python": ["code"],
        "list_files": None,
    }
    # list_files 是空 object：有 properties 键但里面没字段
    list_files = next(t for t in GATEWAY_TOOLS if t.name == "list_files")
    assert list_files.parameters == {"type": "object", "properties": {}}


def test_final_tool_artifacts_item_schema_is_frozen() -> None:
    """FINAL_TOOL 的 artifacts 数组元素 schema：required 恰为 ["path", "title"]。

    这条对应 §3.1 的 ArtifactRef（path 必须以 /work/ 开头 + title），
    模型交付时少给任一字段都无法落成 OutboundFile。
    """
    params: dict[str, Any] = FINAL_TOOL.parameters
    assert params["type"] == "object"
    assert params["required"] == ["reply"]
    assert set(params["properties"]) == {"reply", "artifacts"}
    assert params["properties"]["reply"] == {"type": "string"}

    artifacts = params["properties"]["artifacts"]
    assert artifacts["type"] == "array"
    item = artifacts["items"]
    assert item["type"] == "object"
    assert item["required"] == ["path", "title"]
    assert item["properties"] == {"path": {"type": "string"}, "title": {"type": "string"}}


def test_every_tool_has_non_empty_description() -> None:
    """每个工具都要有描述 —— 这是模型选工具的唯一依据，空描述等于目录残缺。"""
    for tool in ALL_MODEL_TOOLS:
        assert tool.description.strip(), f"{tool.name}: description 不能为空"


# —— schema 逐字冻结 ——
# §3.1 对 Gateway 目录写的是"名字与 schema 冻结"。上面的 required 冻结只锁了必填字段名，
# 锁不住可选参数的类型/边界/默认值：把 read_group_history 的 default 从 50 改成 100、
# 或把 run_python 的 maximum 从 300 放到 3000，前面每一条都还是绿的。
# 下面这份是从 spec §3.1 的 protocol.py 代码块逐字抄下来的 parameters 全文，整份比对。
FROZEN_PARAMETERS: dict[str, dict[str, Any]] = {
    "checklist_add": {
        "type": "object",
        "properties": {
            "items": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 8}
        },
        "required": ["items"],
    },
    "checklist_check": {
        "type": "object",
        "properties": {"id": {"type": "string"}},
        "required": ["id"],
    },
    "checklist_fail": {
        "type": "object",
        "properties": {"id": {"type": "string"}, "reason": {"type": "string"}},
        "required": ["id", "reason"],
    },
    "checklist_note": {
        "type": "object",
        "properties": {"text": {"type": "string"}},
        "required": ["text"],
    },
    "final": {
        "type": "object",
        "properties": {
            "reply": {"type": "string"},
            "artifacts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}, "title": {"type": "string"}},
                    "required": ["path", "title"],
                },
            },
        },
        "required": ["reply"],
    },
    "read_group_history": {
        "type": "object",
        "properties": {
            "limit": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50},
            "thread_only": {"type": "boolean", "default": False},
        },
    },
    "read_document": {
        "type": "object",
        "properties": {"url_or_token": {"type": "string"}},
        "required": ["url_or_token"],
    },
    "download_attachment": {
        "type": "object",
        "properties": {"file_key": {"type": "string"}},
        "required": ["file_key"],
    },
    "run_python": {
        "type": "object",
        "properties": {
            "code": {"type": "string"},
            "timeout_sec": {"type": "integer", "minimum": 1, "maximum": 300, "default": 120},
        },
        "required": ["code"],
    },
    "list_files": {"type": "object", "properties": {}},
}

# 描述也是冻结面的一部分：它是模型选工具的唯一依据（§7 W1 把整份目录喂给模型），
# 改一个字就改了模型的行为，属于契约变更而不是文案润色。逐字抄自 spec §3.1。
FROZEN_DESCRIPTIONS: dict[str, str] = {
    "checklist_add": "添加待办项，仅在任务开始或发现新步骤时调用；每项 ≤20 字",
    "checklist_check": "把某项标记为完成",
    "checklist_fail": "把某项标记为失败并说明原因",
    "checklist_note": "在卡片上写一条 ≤40 字的备注（不新增消息）",
    "final": "交付最终结果。reply 为 markdown；artifacts 为沙箱 /work/ 下要发回线程的文件。调用后任务结束。",
    "read_group_history": "读取本群最近的消息（只含真人消息）",
    "read_document": "读取一篇飞书云文档，返回 markdown 文本",
    "download_attachment": "把本次消息里的附件下载到沙箱 /work/in/ 下，返回路径",
    "run_python": "在隔离沙箱里执行 Python（无网络）。工作目录 /work，输出文件写到 /work/ 下",
    "list_files": "列出沙箱 /work 下的文件",
}


def test_every_tool_parameters_schema_is_frozen_verbatim() -> None:
    """整份 parameters 逐字冻结：可选参数的类型、minimum/maximum、default 一并锁死。"""
    got = {t.name: t.parameters for t in ALL_MODEL_TOOLS}
    assert got == FROZEN_PARAMETERS


def test_every_tool_description_is_frozen_verbatim() -> None:
    """description 逐字冻结 —— 非空还不够，改字就是改模型的选工具依据。"""
    got = {t.name: t.description for t in ALL_MODEL_TOOLS}
    assert got == FROZEN_DESCRIPTIONS


def test_run_python_default_timeout_agrees_with_sandbox_exec_request() -> None:
    """跨契约一致性：模型看到的 run_python 默认超时，必须等于 ExecRequest 的默认超时。

    两边都写死 120（§3.1 protocol.py / sandbox.py）。不一致的话模型按 120 规划，
    沙箱按另一个值执行，超时行为对不上，而两边各自的冻结测试都不会红。
    """
    schema_default = FROZEN_PARAMETERS["run_python"]["properties"]["timeout_sec"]["default"]
    assert schema_default == 120
    assert ExecRequest(code="pass").timeout_sec == 120
    assert schema_default == ExecRequest(code="pass").timeout_sec
    # 上界 300 也是冻结值：sandbox 侧没有对应常量，只能在这里锁住
    assert FROZEN_PARAMETERS["run_python"]["properties"]["timeout_sec"]["maximum"] == 300


def test_catalog_entries_are_toolspec_instances() -> None:
    """目录里放的必须是 ToolSpec 本身，不是 dict 或子类实例 —— 下游按字段取参。"""
    for tool in ALL_MODEL_TOOLS:
        assert type(tool) is ToolSpec, f"{tool!r}: 不是 ToolSpec 实例"
    assert isinstance(LOCAL_TOOL_NAMES, set)
