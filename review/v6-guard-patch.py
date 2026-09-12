#!/usr/bin/env python3
"""V6 ③(3a)+(3c)：清掉守卫里已删的保护面，并修掉 hook 命令的 CLAUDE_PROJECT_DIR 依赖。

**为什么是一个脚本而不是一份 diff**：`.claude/hooks/guard_bash.py` 与
`.claude/settings.json` 本身就在守卫的保护面里（`PROT_PATHS`，命中时 readable=False ——
读和写都不许）。派单里说「正文用 `cat -n` 读，cat 在 READ_SAFE 里」这条不成立：
READ_SAFE 只决定「这是读取位置」，PROT_PATHS 的 readable=False 连读取位置一起拦。
V6 会话实测三条都被拦：

    Read .claude/hooks/guard_bash.py   -> blocked: …（读取位置）
    cat -n .claude/hooks/guard_bash.py -> blocked: …（读取位置）
    cat -n .claude/settings.json       -> blocked: …（读取位置）

所以 V6 这一轨**没读过这两个文件一个字节**，出不了带上下文的 unified diff（git apply
会因为上下文对不上而失败）。改成精确替换：**每一条锚点都必须唯一命中，否则整份拒绝执行、
一个字节都不写**。要么完全照做，要么明确告诉你哪一条对不上。

锚点出处：`review/paste-V6.md` 的 (3a) 表（总管在 0bc8d55 上逐条开文件核过，
代码面与 8d6ffd3 逐字相同）。

用法（在仓库根跑；这两个文件被守卫护着，所以要 AITE_RELOCK=1）：

    AITE_RELOCK=1 python3 review/v6-guard-patch.py --check    # 只报告命中情况，不写盘
    AITE_RELOCK=1 python3 review/v6-guard-patch.py            # 真写
    AITE_RELOCK=1 python3 review/v6-guard-patch.py --partial  # 只写对得上的那些

改完必须跑：`cd core && cargo test -p aite --test guard`（9 条全绿才算数），
再把文件末尾那条 (3c) 断言补进 `core/crates/app/tests/guard.rs`。
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
GUARD = ROOT / ".claude" / "hooks" / "guard_bash.py"
SETTINGS = ROOT / ".claude" / "settings.json"


class Edit:
    """一条改动。`literal` 用字面量替换，`regex` 用正则 —— 后者只用在我不知道
    原文空白怎么排的地方（列表元素），而且一律写窄，靠「必须唯一命中」兜底。"""

    def __init__(self, path, label, old, new, *, regex=False, optional=False):
        self.path, self.label, self.old, self.new = path, label, old, new
        self.regex, self.optional = regex, optional

    def hits(self, text: str) -> int:
        return len(re.findall(self.old, text)) if self.regex else text.count(self.old)

    def apply(self, text: str) -> str:
        if self.regex:
            return re.sub(self.old, self.new, text, count=1)
        return text.replace(self.old, self.new, 1)


def json_escaped(s: str) -> str:
    """一个字符串在 JSON 文件里的转义形（去掉外层引号）。

    settings.json 里 hook 命令带内嵌双引号，手写转义太容易错 —— 让 json 自己算。"""
    return json.dumps(s, ensure_ascii=False)[1:-1]


FROZEN = "冻结面（proto/** 与 core/crates/contracts/**）"

EDITS: list[Edit] = [
    # --- (3a) 清死账 -----------------------------------------------------------
    # 1. 文档串里的死引用（guard_bash.py:4）。空白排法未知，用窄正则。
    Edit(GUARD, "文档串：去掉 aite/contracts/**（Python 旧契约）",
         r"[，,、]?\s*aite/contracts/\*\*（Python 旧契约）", "", regex=True),

    # 2. PROT_PREFIXES 第一项（guard_bash.py:27）。派单逐字引了这一行。
    Edit(GUARD, "PROT_PREFIXES：删掉 aite/contracts/",
         'PROT_PREFIXES = ("aite/contracts/", "proto/", "core/crates/contracts/")',
         'PROT_PREFIXES = ("proto/", "core/crates/contracts/")'),

    # 3. PROBES 第一项（guard_bash.py:41-42）指向已删文件。跨两行，排法未知。
    #    剩下三个探针都还在，所以通配符反向匹配没瘫，只是少一个探针。
    Edit(GUARD, "PROBES：删掉 aite/contracts/__init__.py",
         r'"aite/contracts/__init__\.py"\s*,\s*', "", regex=True),

    # 4. READ_SAFE 里的 pytest（guard_bash.py:52）——仓库里没有 pytest 了
    Edit(GUARD, "READ_SAFE：删掉 pytest", r'"pytest"\s*,\s*', "", regex=True),

    # 5. INTERPRETERS 里的 python / python3.12（guard_bash.py:61）。
    #    **python3 必须留着** —— 守卫自己就是 python3 脚本，`python3 <脚本>` 要按
    #    写入/执行位置判，删了它 `python3 .claude/hooks/guard_bash.py` 就不再被拦。
    Edit(GUARD, "INTERPRETERS：删掉 python（python3 保留）",
         r'"python"\s*,\s*', "", regex=True),
    Edit(GUARD, "INTERPRETERS：删掉 python3.12（python3 保留）",
         r'"python3\.12"\s*,\s*', "", regex=True),

    # 6/7. 两条拦截**消息**里指着已删路径（拦截本身还生效，只是话说错了）
    Edit(GUARD, "Blocked 消息（全树重写工具）改成冻结面",
         'raise Blocked("aite/contracts/**", f"全树重写工具（{prog} 写模式）")',
         f'raise Blocked("{FROZEN}", f"全树重写工具（{{prog}} 写模式）")'),
    Edit(GUARD, "Blocked 消息（find -delete）改成冻结面",
         'raise Blocked("aite/contracts/**", "find 的 -delete/-exec 覆盖面判不出来")',
         f'raise Blocked("{FROZEN}", "find 的 -delete/-exec 覆盖面判不出来")'),

    # 8. RELOCK_MODULE_RE 第一个分支（guard_bash.py:72）。
    #    第二个分支 contracts\s+lock\b 覆盖 `aite contracts lock --write`，活着，别动。
    Edit(GUARD, "RELOCK_MODULE_RE：删掉 aite[./]contracts[./]lock 分支",
         r"aite\[\./\]contracts\[\./\]lock\\b\|", "", regex=True),

    # 9. settings.json 里的死 deny 规则（settings.json:4）
    Edit(SETTINGS, "deny：删掉 Edit(/aite/contracts/**)",
         r'\s*"Edit\(/aite/contracts/\*\*\)"\s*,', "", regex=True),

    # --- (3c) hook 命令不再依赖会随 cwd 漂移的变量 --------------------------------
    #
    # 病根：`CLAUDE_PROJECT_DIR` 在 claude 进程启动那一刻定死。会话在仓库子目录里起
    # （或先起 claude 再 cd），它就指着子目录 → 文件不存在 → hook 执行失败 →
    # **非阻塞放行、不报警** → 整场守卫静默失效。2026-09-12 真踩过一次。
    #
    # V6 实测（同一份 payload，三个 cwd）：
    #   `git rev-parse --show-toplevel` 在 worktree 根、在它的子目录 core/crates/app 下
    #   都返回 /Users/shensikai/Documents/Aite/.worktrees/task-v6 —— 正是该指的地方。
    #   不在 git 仓库里时展开成 python3 "/.claude/hooks/guard_bash.py"，
    #   python3 找不到脚本的退出码是 **2**，PreToolUse 读作「拦截」——
    #   也就是说这个最简形式是 **fail-closed**，不会退化成静默放行：
    #       cwd = worktree 根          -> exit=0（放行 echo hello）
    #       cwd = worktree/core/crates/app -> exit=2（拦住 Read .contracts.lock）
    #       cwd = /tmp（非 git 仓库）   -> exit=2（拦截，不是静默放行）
    #
    # 代价：每次工具调用多起一个 git 进程（约 5ms）。
    Edit(SETTINGS, "(3c) hook 命令：$CLAUDE_PROJECT_DIR → $(git rev-parse --show-toplevel)",
         json_escaped('python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"'),
         json_escaped('python3 "$(git rev-parse --show-toplevel)/.claude/hooks/guard_bash.py"')),

    # --- 可选：REWRITERS 里五个 Python 格式化工具（--trim-rewriters 才做）----------
    #
    # **V6 的建议是不做。** 它们是死条目没错（仓库里早没有这些工具了），但删掉不买任何
    # 东西：条目是「工具名 → 全树重写」这一类的名单，留着就是白留着，哪天谁把 black
    # 装回来还接着拦。而 ruff 那两条正被 `tests/guard.rs` 的
    # `bash_writes_to_the_frozen_surface_are_blocked` 钉着（`ruff format .` /
    # `ruff check --fix .`），删了要连测试一起改。
    # 活着的等价物 `cargo fmt` 有自己的分支（guard_bash.py:285-287），实测
    # `cargo fmt` -> 2、`cargo fmt --check` -> 0，不归 REWRITERS 管。
    Edit(GUARD, "REWRITERS：删掉 black", r'"black"\s*,\s*', "", regex=True, optional=True),
    Edit(GUARD, "REWRITERS：删掉 isort", r'"isort"\s*,\s*', "", regex=True, optional=True),
    Edit(GUARD, "REWRITERS：删掉 autopep8", r'"autopep8"\s*,\s*', "", regex=True, optional=True),
    Edit(GUARD, "REWRITERS：删掉 yapf", r'"yapf"\s*,\s*', "", regex=True, optional=True),
    Edit(GUARD, "REWRITERS：删掉 docformatter",
         r'\s*,\s*"docformatter"', "", regex=True, optional=True),
]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="只报告命中情况，不写盘")
    ap.add_argument("--partial", action="store_true",
                    help="只写对得上的那些（默认是一条对不上就整份不写）")
    ap.add_argument("--trim-rewriters", action="store_true",
                    help="连 REWRITERS 里五个 Python 格式化工具一起删（V6 建议不做）")
    args = ap.parse_args()

    for p in (GUARD, SETTINGS):
        if not p.exists():
            print(f"找不到 {p}", file=sys.stderr)
            return 1

    original = {p: p.read_text(encoding="utf-8") for p in (GUARD, SETTINGS)}
    staged = dict(original)
    problems: list[str] = []

    for e in EDITS:
        if e.optional and not args.trim_rewriters:
            print(f"[    跳过] {e.path.name}: {e.label}（可选，--trim-rewriters 才做）")
            continue
        n = e.hits(staged[e.path])
        if n == 1:
            print(f"[      OK] {e.path.name}: {e.label}")
            staged[e.path] = e.apply(staged[e.path])
        else:
            print(f"[命中 {n} 次] {e.path.name}: {e.label}")
            problems.append(f"{e.path.name}: {e.label}（命中 {n} 次，期望 1）")

    if problems and not args.partial:
        print("\n对不上的锚点（一个字节都没写）：", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        print("\n多半是这两个文件在 V6 之后又动过。人工核一遍，或者 --partial 先做能做的。",
              file=sys.stderr)
        return 1

    if args.check:
        print("\n--check：没写盘。")
        return 1 if problems else 0

    # settings.json 改完必须仍然是合法 JSON —— 写坏了会让 hook 整个不加载，
    # 那就是又一次静默失效。
    try:
        json.loads(staged[SETTINGS])
    except json.JSONDecodeError as exc:
        print(f"\n改完的 settings.json 不是合法 JSON（{exc}），一个字节都没写", file=sys.stderr)
        return 1

    for p, t in staged.items():
        if t != original[p]:
            p.write_text(t, encoding="utf-8")
            print(f"写了 {p}")
    print("\n接着跑：cd core && cargo test -p aite --test guard")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())

# --- 改完 (3c) 之后，把这条断言补进 core/crates/app/tests/guard.rs -------------------
#
# 现在还不能提前加 —— 对着今天的 settings.json 它就是红的，check.sh 当场不过。
#
#     /// hook 命令不许依赖会随 cwd 漂移的变量。
#     ///
#     /// `CLAUDE_PROJECT_DIR` 在 claude 进程启动那一刻定死：会话在仓库子目录里起
#     /// （或先起 claude 再 cd），它就指着子目录 → 文件不存在 → hook 执行失败 →
#     /// 非阻塞放行、不报警 → 整场守卫静默失效。2026-09-12 真踩过一次，
#     /// 写 V6 派单的那个会话也正踩着。
#     #[test]
#     fn the_hook_command_resolves_the_repo_root_from_cwd() {
#         for c in pretooluse_commands() {
#             assert!(
#                 c.contains("git rev-parse --show-toplevel"),
#                 "hook 命令要从 cwd 往上找仓库根，别再依赖 $CLAUDE_PROJECT_DIR：{c:?}"
#             );
#         }
#     }
