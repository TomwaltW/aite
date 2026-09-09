"""Gateway 工具实现（T3）。

§3.1 的 `GATEWAY_TOOLS` 冻结了这 5 个工具的名字与 schema；这里是名字到实现的映射。
`P0ToolGateway` 只按名字查这张表，所以「目录里有、表里没有」会被它当成没实现
（返回 not_found）而不是崩掉。tests/gateway 里有一条断言把两边的名字集合钉在一起。
"""
from .attachments import download_attachment
from .base import ToolEnv, ToolFailure, ToolImpl, ToolOutcome
from .documents import read_document
from .files import list_files
from .history import read_group_history
from .python_exec import run_python

#: 工具名 → 实现。键必须与 §3.1 GATEWAY_TOOLS 里的 name 一一对应。
TOOL_IMPLS: dict[str, ToolImpl] = {
    "read_group_history": read_group_history,
    "read_document": read_document,
    "download_attachment": download_attachment,
    "run_python": run_python,
    "list_files": list_files,
}

__all__ = [
    "TOOL_IMPLS",
    "ToolEnv",
    "ToolFailure",
    "ToolImpl",
    "ToolOutcome",
    "download_attachment",
    "list_files",
    "read_document",
    "read_group_history",
    "run_python",
]
