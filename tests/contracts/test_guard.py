"""PreToolUse 守卫的回归测试（.claude/hooks/guard_bash.py）。

守卫是「可选但推荐」的那件：仓库里没有它时整份跳过，永远不会变成别的 track 的路障。
但只要它在，这些用例就必须成立 —— 一个有洞的守卫比没有守卫更危险，
因为它给人「契约有人看着」的错觉，而 hook 失败是**非阻塞放行且不报警**的。

用 python3（不是 sys.executable）跑：settings.json 里的 hook 命令就是 python3，
本机默认 python3 是 3.11 而 venv 是 3.12，必须验的是真正会执行守卫的那个解释器。
"""
import json
import shutil
import subprocess

import pytest

from aite.contracts.lock import REPO_ROOT

GUARD = REPO_ROOT / ".claude" / "hooks" / "guard_bash.py"
SETTINGS = REPO_ROOT / ".claude" / "settings.json"

pytestmark = pytest.mark.skipif(
    not GUARD.exists() or shutil.which("python3") is None,
    reason="仓库未安装守卫（可选项），或本机没有 python3",
)

BLOCKED = 2   # PreToolUse hook 的「拦截」退出码


def run_guard(payload: dict) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["python3", str(GUARD)],
        input=json.dumps(payload),
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
    )


def bash(cmd: str) -> subprocess.CompletedProcess:
    return run_guard({"tool_name": "Bash", "tool_input": {"command": cmd}})


# --- 必须拦下的写入姿态 -----------------------------------------------------

MUST_BLOCK = [
    ("重定向覆盖契约", "echo x > aite/contracts/gateway.py"),
    ("sed -i 改契约", 'sed -i "" s/a/b/ aite/contracts/config.py'),
    ("通配符扫到契约", "rm aite/contracts/*.py"),
    ("命令替换藏路径", "cat $(echo aite/contracts/events.py) > /tmp/x"),
    ("变量回填", "F=aite/contracts/events.py; rm $F"),
    ("git checkout 覆盖", "git checkout HEAD -- aite/contracts/events.py"),
    ("heredoc 写契约", "cat > aite/contracts/x.py <<EOF\nx\nEOF"),
    # 以下六类是审计实测出来、后来才堵上的绕过，全部不许回归
    ("全树重写：ruff format", "ruff format ."),
    ("全树重写：ruff --fix", "ruff check --fix ."),
    ("大小写不敏感（APFS）", "echo x > aite/Contracts/events.py"),
    ("xargs 管道", "echo aite/contracts/events.py | xargs rm"),
    ("key=value 藏路径", "dd if=/dev/zero of=aite/contracts/events.py"),
    ("find -delete", 'find . -name "*.py" -delete'),
    ("点分模块 relock", "python3 -m aite.contracts.lock --write"),
]


@pytest.mark.parametrize(("label", "cmd"), MUST_BLOCK, ids=[m[0] for m in MUST_BLOCK])
def test_bash_write_to_protected_is_blocked(label, cmd):
    assert bash(cmd).returncode == BLOCKED, f"{label} 没被拦：{cmd!r}"


MUST_BLOCK_TOOLS = [
    ("Write 契约", {"tool_name": "Write", "tool_input": {"file_path": "aite/contracts/events.py"}}),
    ("Edit 契约锁", {"tool_name": "Edit", "tool_input": {"file_path": ".contracts.lock"}}),
    ("Write 冻结 spec",
     {"tool_name": "Write", "tool_input": {"file_path": "docs/dev-spec-2026-09-09.md"}}),
    ("Read 守卫自身",
     {"tool_name": "Read", "tool_input": {"file_path": ".claude/hooks/guard_bash.py"}}),
    ("Read 契约锁", {"tool_name": "Read", "tool_input": {"file_path": ".contracts.lock"}}),
]


@pytest.mark.parametrize(("label", "payload"), MUST_BLOCK_TOOLS, ids=[m[0] for m in MUST_BLOCK_TOOLS])
def test_tool_write_to_protected_is_blocked(label, payload):
    assert run_guard(payload).returncode == BLOCKED, f"{label} 没被拦"


# --- 必须放行的日常操作（守卫太紧会把所有人卡死）------------------------------

MUST_PASS = [
    ("lock --check 是每轨的验收项", "python3 -m aite.contracts.lock --check"),
    ("ruff check 不是写模式", "ruff check ."),
    ("find 普通检索", 'find . -name "*.py"'),
    ("pytest", "pytest tests/contracts -q"),
    ("cat 契约（要照着写代码）", "cat aite/contracts/ports.py"),
    ("写自己轨的文件", "echo x > aite/control/plane.py"),
]


@pytest.mark.parametrize(("label", "cmd"), MUST_PASS, ids=[m[0] for m in MUST_PASS])
def test_normal_work_is_allowed(label, cmd):
    assert bash(cmd).returncode == 0, f"{label} 被误拦：{cmd!r}"


def test_read_contract_and_spec_allowed():
    """执行者必须读得到契约和 spec，否则没法照着写代码。"""
    for path in ("aite/contracts/events.py", "docs/dev-spec-2026-09-09.md"):
        payload = {"tool_name": "Read", "tool_input": {"file_path": path}}
        assert run_guard(payload).returncode == 0, f"Read {path} 被误拦"


def test_glob_over_whole_repo_allowed():
    """Glob(**/*.py) 是正常操作，不能因为可能展开到契约就整个拦掉。"""
    payload = {"tool_name": "Glob", "tool_input": {"pattern": "**/*.py"}}
    assert run_guard(payload).returncode == 0


# --- hook 接线本身 ----------------------------------------------------------

def test_hook_command_uses_python3_not_python():
    """本机没有 `python` 命令；hook 写成 python 会 command not found，
    而 hook 失败是非阻塞放行且不报警 —— 守卫会静默失效。"""
    cfg = json.loads(SETTINGS.read_text(encoding="utf-8"))
    cmds = [
        h["command"]
        for entry in cfg["hooks"]["PreToolUse"]
        for h in entry["hooks"]
        if h.get("type") == "command"
    ]
    assert cmds, "settings.json 里没有 PreToolUse 命令 hook"
    for c in cmds:
        assert "guard_bash.py" in c
        assert c.split()[0] == "python3", f"hook 命令没用 python3：{c!r}"


def test_deny_rules_use_only_edit_prefix():
    """Claude Code 只对 Edit(path) / Read(path) 做文件权限匹配。

    Write(path) / NotebookEdit(path) 会被接受但从不查询，且启动时刷警告。
    一条 Edit() 规则就覆盖所有编辑类工具。"""
    cfg = json.loads(SETTINGS.read_text(encoding="utf-8"))
    for rule in cfg["permissions"]["deny"]:
        assert rule.startswith(("Edit(", "Read(")), f"死规则（不参与文件权限匹配）：{rule}"
