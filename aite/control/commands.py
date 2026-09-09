"""`!` 命令的解析（§3.5 R5 / §3.3）。

只做解析，不碰状态 —— 执行在 plane.py 里。
"""

UNKNOWN_COMMAND_TEXT = "未知命令，可用：!status !stop <任务号> !restart !new"

KNOWN_COMMANDS = ("!status", "!stop", "!restart", "!new")


def parse_command(text: str) -> tuple[str, str]:
    """`"!stop #A17"` → `("!stop", "#A17")`。命令名统一小写，参数原样 strip。"""
    head, _, rest = text.strip().partition(" ")
    return head.lower(), rest.strip()


def normalize_task_no(raw: str) -> str:
    """`"a17"` / `"A17"` / `"#a17"` 一律归成 `"#A17"`，对齐 `encode_task_no` 的输出形状。"""
    s = raw.strip().upper()
    if not s:
        return ""
    return s if s.startswith("#") else "#" + s
