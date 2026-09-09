"""PreToolUse 守卫：拦截触碰受保护面的写入 / 执行操作。

改编自 MAOS 的 scripts/guard_bash.py，保护面换成 Aite 的：
  aite/contracts/**、.contracts.lock、docs/dev-spec-*.md，以及守卫自身与 hook 配置。

判定分两步 —— 先规范化（变量回填、shlex 分词、路径归一），再看受保护路径
处在什么位置：只有写入 / 执行位置才拦，读取位置按可读白名单放行。
契约与 spec 允许 Read（执行者要照着它们写代码），守卫脚本与本配置不允许 Read。

失败一律按拒绝处理：命令解析不了、未知命令带受保护路径参数、守卫自身
抛异常，全部 exit(2)，不给静默放行留口子。

授权改契约时（只有 T0 / 总管该这么做）：AITE_RELOCK=1 整体放行。
"""
import fnmatch
import json
import os
import posixpath
import re
import shlex
import sys

# ------------------------------------------------------------------ 保护面

# 整片保护的目录：新增契约文件自动落在保护面内，不用回来改这张表。
PROT_PREFIXES = ("aite/contracts/",)
# 冻结的 spec，按通配匹配（将来换日期也不用改守卫）。
PROT_GLOBS = ("docs/dev-spec-*.md",)
# 单文件保护。
PROT_PATHS = [
    ".contracts.lock",
    ".claude/hooks/guard_bash.py",
    ".claude/settings.json",
    ".claude/settings.local.json",
]
# 通配符可能展开到受保护面 —— 用这些代表路径做反向匹配。
PROBES = ["aite/contracts/__init__.py", "docs/dev-spec-2026-09-09.md"]
# 不会重名的 basename，覆盖 `cd .claude/hooks && python3 guard_bash.py` 这类相对调用。
BARE_MATCH = {"guard_bash.py", ".contracts.lock"}

# 只读命令白名单。白名单外的一律按写入位置处理（fail-closed）。
READ_SAFE = {
    "echo", "printf", "cat", "head", "tail", "less", "more", "nl", "od",
    "grep", "egrep", "fgrep", "rg", "ag", "wc", "ls", "find", "file", "stat",
    "diff", "cmp", "uniq", "cut", "tr", "tree", "pwd", "which", "type",
    "basename", "dirname", "column", "jq", "yq", "shasum", "sha256sum", "md5",
    "pytest", "true", "false", "test", "date", "sleep", "cd", "export", "set",
}
# 只是前缀包装，真正的程序名在后面
WRAPPERS = {"env", "sudo", "nohup", "time", "command", "builtin", "exec",
            "xargs", "stdbuf", "nice", "then", "do", "else", "!"}
INTERPRETERS = {"python", "python3", "python3.12", "perl", "ruby", "node", "sh", "bash", "zsh", "php"}
GIT_READ = {"diff", "status", "log", "show", "grep", "ls-files", "blame",
            "describe", "rev-parse", "cat-file", "shortlog"}
# 就地重写整棵树的工具：写模式下没有任何一个 token 会写出契约路径，
# 逐 token 判位置根本拦不住 —— `ruff format .` 会把契约文件一起改了。
REWRITERS = {"ruff", "black", "isort", "autopep8", "yapf", "docformatter"}
REWRITE_FLAGS = {"format", "--fix", "--fix-only", "-i", "--in-place", "-w", "--write"}
# find 的这些谓词是执行/删除，不再是只读
FIND_WRITE_ACTIONS = {"-delete", "-exec", "-execdir", "-ok", "-okdir"}
# `python -m aite.contracts.lock --write` 走的是点分模块名，argv 里不出现任何路径，
# 位置判定必然落空 —— 而它恰好是唯一能把「篡改过的契约」洗成绿 C2 的命令。
RELOCK_MODULE_RE = re.compile(r"aite[./]contracts[./]lock\b")
RELOCK_ARG_RE = re.compile(r"(?:^|\s)--write(?:\s|$)")

# 无法做位置判定的构造 —— 命中就对整条命令做规范化子串扫描
OPAQUE_RE = re.compile(r"\$\(|`|\beval\b|\bbase64\b|\bawk\b|\bsource\b|<<<|\bxargs\b")
# 授权变量只许读（echo $AITE_RELOCK），不许在命令里赋值 / 导出 / 清除
RELOCK_WRITE_RE = re.compile(
    r"AITE_RELOCK\s*=|(?:^|[\s;&|])(?:export|unset|declare|typeset)\s+AITE_RELOCK\b")
ASSIGN_RE = re.compile(r"""(?:^|[\s;&|(])([A-Za-z_]\w*)=("[^"]*"|'[^']*'|[^\s;&|)]*)""")
VAR_RE = re.compile(r"\$\{(\w+)\}|\$(\w+)")
WRITE_REDIR_RE = re.compile(r"^[0-9&]*>>?\|?$")
READ_REDIR_RE = re.compile(r"^[0-9]*<<?<?$")
SEP_CHARS = set(";|&()")


class Blocked(Exception):
    def __init__(self, path, where):
        super().__init__(path)
        self.path = path
        self.where = where


# -------------------------------------------------------------- 路径与位置

def norm_path(tok):
    """归一到可比较的形式：展开 ~、折叠 ./ // ..，保留通配符。"""
    t = (tok or "").strip()
    if not t:
        return ""
    if t.startswith("~"):
        t = t[1:].lstrip("/")
    if not t:
        return ""
    return posixpath.normpath(t)


def has_glob(t):
    return any(c in t for c in "*?[")


def under_prefix(t, pre):
    """t 是否落在受保护目录 pre 下（支持绝对路径与更深的前缀）。"""
    return t == pre.rstrip("/") or t.startswith(pre) or ("/" + pre) in (t + "/")


def glob_hit(t, pat):
    return fnmatch.fnmatch(t, pat) or fnmatch.fnmatch(t, "*/" + pat)


def hits(token, write_pos):
    """token 命中哪个受保护路径；返回 (显示用路径, 是否允许读) 或 None。

    全程 casefold 比较：本机是 APFS（大小写不敏感），`aite/Contracts/events.py`
    指向的就是真契约文件，区分大小写地比较等于给守卫开了个后门。
    """
    t = norm_path(token).casefold()
    if t in ("", ".", "..", "/"):
        return None
    for pre in PROT_PREFIXES:                       # 契约目录：可读不可写
        if under_prefix(t, pre):
            return (pre + "**", True)
    for pat in PROT_GLOBS:                          # 冻结 spec：可读不可写
        if glob_hit(t, pat):
            return (pat, True)
    for p in PROT_PATHS:                            # 锁 / 守卫 / hook 配置：读写都不许
        if t == p or t.endswith("/" + p):
            return (p, False)
        base = posixpath.basename(p)
        if base in BARE_MATCH and t == base:
            return (p, False)
    # 通配符可能展开到受保护路径 —— 只在写位置判，否则
    # Glob(pattern="**/*.py") 这类正常操作会被整个拦掉。
    if write_pos and has_glob(t):
        for probe in PROBES:
            if fnmatch.fnmatch(probe, t):
                return (probe, True)
        for p in PROT_PATHS:
            if fnmatch.fnmatch(p, t):
                return (p, False)
    return None


ARG_VALUE_RE = re.compile(r"^[-A-Za-z0-9_.]+=(.+)$")


def check_token(token, write_pos):
    # `dd of=aite/contracts/x.py`、`--output=...` 这类整体是一个 token，
    # 路径藏在等号右边，不拆开就永远命中不了。
    m = ARG_VALUE_RE.match(token or "")
    if m:
        _check_one(m.group(1), write_pos)
    _check_one(token, write_pos)


def _check_one(token, write_pos):
    hit = hits(token, write_pos)
    if hit is None:
        return
    path, readable = hit
    if not write_pos and readable:
        return
    raise Blocked(path, "写入/执行位置" if write_pos else "读取位置")


def scan_opaque(text, where="不透明载荷"):
    """无法解析的片段：去掉引号与反斜杠后做子串扫描，命中即拦。"""
    squashed = re.sub(r"""['"\\]""", "", text or "").casefold()
    for pre in PROT_PREFIXES:
        if pre in squashed:
            raise Blocked(pre + "**", where)
    if "dev-spec-" in squashed:
        raise Blocked("docs/dev-spec-*.md", where)
    for p in PROT_PATHS:
        if p in squashed:
            raise Blocked(p, where)
        base = posixpath.basename(p)
        if base in BARE_MATCH and base in squashed:
            raise Blocked(p, where)


# ------------------------------------------------------------------ Bash

def substitute_vars(cmd):
    """把同一条命令里定义的 NAME=value 回填到 $NAME / ${NAME}。"""
    env = {}
    for m in ASSIGN_RE.finditer(cmd):
        val = m.group(2)
        if len(val) >= 2 and val[0] == val[-1] and val[0] in "\"'":
            val = val[1:-1]
        env[m.group(1)] = val
    if not env:
        return cmd

    def repl(m):
        return env.get(m.group(1) or m.group(2), m.group(0))

    out = cmd
    for _ in range(3):
        new = VAR_RE.sub(repl, out)
        if new == out:
            break
        out = new
    return out


def tokenize(line):
    lex = shlex.shlex(line, posix=True, punctuation_chars=True)
    lex.whitespace_split = True
    lex.commenters = ""          # # 不当注释，免得把路径藏在井号后面
    try:
        return list(lex)
    except ValueError as exc:
        raise Blocked(f"<命令无法解析: {exc}>", "解析失败") from exc


def split_segments(tokens):
    """按 ; | & && || ( ) 切成若干 simple command。"""
    segs, cur = [], []
    for t in tokens:
        if t and set(t) <= SEP_CHARS:
            if cur:
                segs.append(cur)
            cur = []
        else:
            cur.append(t)
    if cur:
        segs.append(cur)
    return segs


def check_segment(argv):
    # 1. 重定向：> 之后是写位置，< 之后是读位置
    rest, i = [], 0
    while i < len(argv):
        t = argv[i]
        if WRITE_REDIR_RE.match(t) or READ_REDIR_RE.match(t):
            if i + 1 < len(argv):
                check_token(argv[i + 1], bool(WRITE_REDIR_RE.match(t)))
                i += 2
                continue
            i += 1
            continue
        rest.append(t)
        i += 1
    if not rest:
        return

    # 2. 剥掉前置赋值与包装器，拿到真正的程序名
    while rest and (re.fullmatch(r"[A-Za-z_]\w*=.*", rest[0], re.S)
                    or posixpath.basename(rest[0]) in WRAPPERS):
        rest = rest[1:]
    if not rest:
        return

    prog = posixpath.basename(rest[0])
    args = rest[1:]

    # 3. 定位置
    if prog == "git":
        sub = next((a for a in args if not a.startswith("-")), "")
        write_pos = sub not in GIT_READ
    elif prog in INTERPRETERS:
        if "-c" in args or "-e" in args:
            scan_opaque(" ".join(args), "解释器内联代码")
        # 其余参数按「被执行」处理
        write_pos = True
    elif prog in REWRITERS:
        if any(a in REWRITE_FLAGS for a in args):
            # 写模式下覆盖面由 CWD 决定而不是由参数决定，逐 token 判不出来，整条拦。
            raise Blocked("aite/contracts/**", f"全树重写工具（{prog} 写模式）")
        write_pos = False
    elif prog == "find":
        if any(a in FIND_WRITE_ACTIONS for a in args):
            raise Blocked("aite/contracts/**", "find 的 -delete/-exec 覆盖面判不出来")
        write_pos = False
    elif prog == "sort":
        write_pos = any(a == "-o" or a.startswith("--output") for a in args)
    elif prog == "sed":
        write_pos = any(a.startswith("-i") for a in args)
    elif prog in READ_SAFE:
        write_pos = False
    else:
        write_pos = True                     # 未知命令 → fail-closed

    for a in args:
        check_token(a, write_pos)
    check_token(rest[0], True)               # 程序名本身也可能是受保护路径


def check_bash(command):
    raw = command or ""
    if RELOCK_WRITE_RE.search(raw):
        raise Blocked("AITE_RELOCK", "授权变量赋值")
    if RELOCK_MODULE_RE.search(raw) and RELOCK_ARG_RE.search(raw):
        raise Blocked(".contracts.lock", "重生成契约锁（--check 放行，--write 需 AITE_RELOCK=1）")
    if OPAQUE_RE.search(raw):
        scan_opaque(raw)
    for line in substitute_vars(raw.replace("\\\n", " ")).split("\n"):
        if line.strip():
            for seg in split_segments(tokenize(line)):
                check_segment(seg)


# ------------------------------------------------------------------ 分派

def main():
    payload = json.load(sys.stdin)
    tool = payload.get("tool_name", "")
    ti = payload.get("tool_input") or {}

    if tool == "Bash":
        check_bash(ti.get("command", ""))
    elif tool == "Read":
        check_token(ti.get("file_path", ""), False)
    elif tool in ("Edit", "Write"):
        check_token(ti.get("file_path", ""), True)      # content 一律不看
    elif tool == "NotebookEdit":
        check_token(ti.get("notebook_path", ""), True)
    elif tool == "Grep":
        for f in ("path", "glob"):                      # pattern 一律不看
            check_token(ti.get(f) or "", False)
    elif tool == "Glob":
        for f in ("path", "pattern"):
            check_token(ti.get(f) or "", False)
    # 其余工具不在 matcher 内，放行


if __name__ == "__main__":
    if os.environ.get("AITE_RELOCK") == "1":
        sys.exit(0)
    try:
        main()
    except Blocked as b:
        print(f"blocked: 该操作触碰受保护面 {b.path}（{b.where}）。停止当前工作并向人类报告。",
              file=sys.stderr)
        sys.exit(2)
    except SystemExit:
        raise
    except BaseException as exc:
        print(f"guard internal error: {type(exc).__name__}: {exc}，按拒绝处理。", file=sys.stderr)
        sys.exit(2)
    sys.exit(0)
