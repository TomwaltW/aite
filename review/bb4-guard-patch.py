#!/usr/bin/env python3
"""BB4：治守卫的一个**真漏拦**（先 `cd` 就绕过冻结面）和一条**真误拦**（`cargo fmt --version`）。

两条都是 AA3（台账「十六、AA3 回执」）量准、点名「单开一轨」但明确没出补丁的账。
AA3 不出补丁的理由是「锚点定位不到 —— 要替换的是**判据代码**而不是字符串字面量，
黑盒给不出它的样子」。**本轨换了个办法绕开这个死结**：见下面「锚点怎么来的」。

--------------------------------------------------------------------------------
治什么
--------------------------------------------------------------------------------

**① 真漏拦（主交付）**：`PROT_PREFIXES` 匹配的是命令文本里写出来的那串字符，
少了 cwd 这一维：

    cd core && echo x > crates/contracts/src/lib.rs        → 放行（真能改到契约）
    会话 cwd 落在 core/ 时  Write(crates/contracts/src/lib.rs)  → 放行
    会话 cwd 落在 core/crates/contracts/src/ 时  echo x > lib.rs → 放行

BB4 把 `cd` 的形状矩阵铺开量了一遍（`&&` / `;` / `./` / 两级 / 子 shell / 换行 /
带引号 / `..` 回绕 / `tee` / `sed -i` / proto 面），**十二种全部放行** ——
不是某一种写法的疏漏，是整条 cwd 维度不存在。

**② 真误拦**：`cargo fmt --version` / `--help` / `-V` / `-h` 一个字节都不写，
却被判「写模式」。（`-V` / `-h` 是 BB4 补量的，AA3 只量了长选项那两个。）

--------------------------------------------------------------------------------
锚点怎么来的 —— 这份补丁和 AA3「定位不到」的分界线
--------------------------------------------------------------------------------

`.claude/hooks/guard_bash.py` 在守卫自己的 `PROT_PATHS` 里（`readable=False`，
**读和写都拦**），出补丁的会话一个字节都读不到它。V6 / Z2 两份补丁靠的是
「逐字量出来的字符串字面量」当锚点；AA3 要改的是**判据代码**，那条路走不通。

**本轨的办法：不猜形状，改用 AST 结构定位。**

补丁脚本是**由人跑**的（`AITE_RELOCK=1`），它自己读得到源码。所以锚点不写成
「我猜代码长这样的一串字符」，而写成「在语法树上满足这些条件的那个节点」：

    锚点 A：模块里**唯一**一处从 stdin 读 payload 的调用
            （候选：`json.load(…sys.stdin…)` / `json.loads(…sys.stdin…)` /
             `sys.stdin.read()`；**三个候选合起来必须恰好命中 1 处**）
    锚点 B：模块 import 段结束的位置（第一个既不是 docstring 也不是 import 的
            顶层语句），注入处放函数定义

两条锚点都**不依赖任何字符串字面量**，也不依赖判据写成什么样 —— 它们只依赖
「这是个从 stdin 读 JSON 的 Python 脚本」这件事，而那是黑盒实测反复确认过的。
命中数不是 1 就**整份拒绝执行、一个字节都不写**，并把现状报成人话。

`--check` 在不改任何东西的前提下报出命中/未命中（外加被包裹的那一小段表达式原文）——
那是**单个锚点**级别的信息，不是 dump 源码。派单的硬约束划的就是这条线。

--------------------------------------------------------------------------------
① 的药：不改判据，**再跑一遍守卫自己**
--------------------------------------------------------------------------------

注入的那一层（`_bb4_precheck`）做的事：把 payload 里的相对路径按
「进程 cwd + 命令文本里的 `cd`」展开成**仓库根相对路径**，若展开后与原来不同，
就拿这份等价 payload **再跑一遍守卫自己**（同一个文件，带防递归的环境标记）。
任一遍判拦就拦。

**为什么这不会造出成片误拦** —— AA3 点名的那个风险（「`cd` 的解析一旦做歪，
误拦会成片出现」）在结构上被这七条挡着，每一条都由 `tests/guard.rs` 钉着：

  1. **原判定完整保留**。原 payload 照旧走全部原判据，这一层只**新增**拦截 ——
     「原来拦的现在放」在结构上不可能发生。
  2. **cwd == 仓库根时归一化是恒等变换**。`run_guard()` 把 cwd 钉在仓库根，
     所以既有回归**一条都不会变**（除了带 `cd` 的那批 characterization，
     那正是要治的）。
  3. **不改每一段的首词**。首词是命令名 —— 把 `cat` 改成 `core/cat` 会让它掉出
     `READ_SAFE`，那才是成片误拦真正的来源。这一条是实测教训。
  4. **只改「像路径」的 token**：含 `/` 或 `.`、不以 `-` 开头、不是绝对路径。
  5. **归一化后落在仓库外的不改**。`cd /tmp && echo x > crates/contracts/…`
     写的是 `/tmp/crates/…`，跟冻结面无关，照旧放行。
  6. **`cd` 目标要到运行时才知道就放弃**（`$PWD`、`$(…)`、反引号）——
     从那一段往后不再归一化，退化成今天的行为。
  7. **cwd 推断错只会少拦，不会误拦**。`os.getcwd()` 若不是会话 cwd 而是仓库根，
     归一化退化成恒等 —— 就是今天的行为。

**fail-closed 复核**：这一层跑在守卫**内部**。守卫压根没跑起来时它也不存在，
碰不到「守卫失效时怎么办」那一层（那是 hook 命令的事，Z2 的 `[ -f ]` 回退管着，
本补丁一个字没动）。二次调用出任何意外（起不来 / 超时 / 解析不了）就**退回原判定**,
原有保护一条不少 —— 不比打补丁之前差。而 ① 方向上只会让守卫**拦得更多**。

--------------------------------------------------------------------------------
② 的药：入口白名单，**不动判据代码**
--------------------------------------------------------------------------------

AA3 建议的是「把『有没有 `--check`』那个判据扩成『有没有 `--check` / `--version` /
`--help`』」。本轨**没有那样做**，理由是：BB4 实测 `cargo fmt --checkfoo` **被拦** ——
守卫那条判据本来就是**按词**的，去动它等于为了两个选项重写一处正在正确工作的判据。

改成在入口加一条**极窄**的白名单早退：整条命令的词必须全部落在
`{cargo, fmt, --all, --quiet, -q}` ∪ `{--version, -V, --help, -h}` 里，
前两个词必须是 `cargo fmt`，且至少有一个查询选项。按**词**匹配，不是子串 ——
`--help-xyz` 不在白名单里，照旧拦（AA3 ③.1 点名要的正是这一条）。

**差别要说清楚**：AA3 那个改法会让 `cargo fmt -p aite --version` 也放行，
白名单不会（`aite` 不在白名单里）。选窄的那一版，因为它的失败方向是「多拦一点」。

--------------------------------------------------------------------------------
用法
--------------------------------------------------------------------------------

    # 第一条命令：干跑，报锚点命中情况，不写盘
    AITE_RELOCK=1 python3 review/bb4-guard-patch.py --check

    # 真写（两个文件一起落：守卫 + 它的回归测试，免得两边不同步）
    AITE_RELOCK=1 python3 review/bb4-guard-patch.py

    # 自验（不碰 .claude/**，所以不要授权）：对合成件跑
    python3 review/bb4-guard-patch.py --make-fixture <临时目录> --shape 1
    python3 review/bb4-guard-patch.py --root <临时目录> --check

改完必须跑（**这是补丁生效的判据**）：

    cd core && cargo test -p aite --test guard      # 期望 21 passed

`cargo_fmt_is_judged_by_the_check_flag_alone` 那两行 characterization 由本补丁
一并从 `blocked` 挪到 `allowed`，`protected_prefixes_…_cd_first_slips_through`
与 `the_guard_never_looks_at_the_process_cwd` 里的 `0` 一并改成 `BLOCKED` ——
**守卫和钉它的测试是同一次落盘**，不会出现「改了守卫、测试还钉着旧行为」的半拉子状态。
"""

from __future__ import annotations

import argparse
import ast
import json
import os
import pathlib
import sys

GUARD_REL = ".claude/hooks/guard_bash.py"
TESTS_REL = "core/crates/app/tests/guard.rs"

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent


# =============================================================================
# 注入到 guard_bash.py 的那一层
# =============================================================================
#
# r-string：里面的 `\s` / `\n` / `\"` 要**原样**出现在守卫源码里，不能在这里就被
# Python 吃掉一层。

SHIM = r'''

# === BB4（2026-09-15）：路径按 cwd 归一化 + cargo fmt 纯查询放行 ================
#
# 治 AA3（台账「十六、AA3 回执」）量到的两条：
#   ① 真漏拦：`cd core && echo x > crates/contracts/src/lib.rs` 与会话 cwd 落在
#      `core/` 之后的 `Write(crates/contracts/src/lib.rs)` 都能改到契约、却放行。
#   ② 真误拦：`cargo fmt --version` / `--help` / `-V` / `-h` 一个字节都不写，
#      却被判「写模式」。
#
# ① **只新增拦截，不移除任何一条**：原 payload 照旧往下走全部原判据；这一层只是
# 额外拿一份「相对路径已按 cwd 展开」的等价 payload 再跑一遍守卫自己（同一个文件，
# 带防递归标记），任一遍判拦就拦。cwd == 仓库根时归一化是恒等变换，既有回归一条都不变。
#
# **fail-closed**：这一层跑在守卫内部 —— 守卫没跑起来时它也不存在，碰不到
# 「守卫失效时怎么办」那一层（那是 hook 命令的事）。二次调用出任何意外就退回原判定。
#
# 详见 review/bb4-guard-patch.py 的模块 docstring，与 tests/guard.rs 的
# `the_guard_never_looks_at_the_process_cwd`。

import json as _bb4_json
import os as _bb4_os
import re as _bb4_re
import subprocess as _bb4_subprocess
import sys as _bb4_sys

_BB4_INNER = "_AITE_GUARD_BB4_INNER"

_BB4_TOKEN = _bb4_re.compile(r"[A-Za-z0-9_.][A-Za-z0-9_./-]*")
_BB4_SEP = _bb4_re.compile(r"(\s*(?:&&|\|\||;;|;|\||\n)\s*)")
_BB4_CD = _bb4_re.compile(r"^\s*\(?\s*cd\s+([^\s;&|)]+)\s*$")
_BB4_HEAD = _bb4_re.compile(r"^\s*\(?\s*(?:[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*\S+")

# ② 极窄的白名单：整条命令的词必须全落在这两个集合里，前两词是 `cargo fmt`，
# 且至少有一个查询选项。按**词**匹配，不是子串 —— `--help-xyz` 不在里面，照旧拦。
_BB4_FMT_OK = {"cargo", "fmt", "--all", "--quiet", "-q"}
_BB4_FMT_QUERY = {"--version", "-V", "--help", "-h"}


def _bb4_repo_root():
    """.claude/hooks/guard_bash.py -> 仓库根"""
    hooks = _bb4_os.path.dirname(_bb4_os.path.abspath(__file__))
    return _bb4_os.path.dirname(_bb4_os.path.dirname(hooks))


def _bb4_repo_rel(p, root):
    """p 落在 root 之下时返回它相对 root 的路径，否则 None（仓库外的不碰）。"""
    try:
        rel = _bb4_os.path.relpath(_bb4_os.path.normpath(p), root)
    except (ValueError, OSError):
        return None
    if rel == ".." or rel.startswith(".." + _bb4_os.sep) or _bb4_os.path.isabs(rel):
        return None
    return rel


def _bb4_map_token(tok, cur, root):
    if tok.startswith("-") or _bb4_os.path.isabs(tok):
        return tok
    if "/" not in tok and "." not in tok:
        return tok                       # 裸词不是路径，别动
    rel = _bb4_repo_rel(_bb4_os.path.join(cur, tok), root)
    return rel if rel and rel != tok else tok


def _bb4_rewrite_cmd(cmd, cwd, root):
    """把命令里的相对路径按 cwd（叠加命令自己的 `cd`）展开成仓库根相对路径。

    **不改每一段的首词** —— 首词是命令名，改了会让 `cat` 变成 `core/cat`、
    掉出 READ_SAFE，那才是成片误拦的来源。
    """
    cur = cwd
    out = []
    for i, seg in enumerate(_BB4_SEP.split(cmd)):
        if i % 2:                        # 分隔符原样留着
            out.append(seg)
            continue
        m = _BB4_CD.match(seg)
        if m:
            tgt = m.group(1).strip('"').strip("'")
            if "$" in tgt or "`" in tgt:
                cur = None               # 要到运行时才知道 —— 从这里往后放弃
            elif _bb4_os.path.isabs(tgt):
                cur = _bb4_os.path.normpath(tgt)
            elif cur is not None:
                cur = _bb4_os.path.normpath(_bb4_os.path.join(cur, tgt))
            out.append(seg)
            continue
        if cur is None:
            out.append(seg)
            continue
        h = _BB4_HEAD.match(seg)
        k = h.end() if h else 0
        out.append(seg[:k] + _BB4_TOKEN.sub(
            lambda mo: _bb4_map_token(mo.group(0), cur, root), seg[k:]))
    return "".join(out)


def _bb4_normalize(data, cwd, root):
    """返回「路径按 cwd 展开」的等价 payload；没什么可归一化的就返回 None。"""
    ti = data.get("tool_input")
    if not isinstance(ti, dict):
        return None
    if data.get("tool_name") == "Bash":
        cmd = ti.get("command")
        if not isinstance(cmd, str):
            return None
        new = _bb4_rewrite_cmd(cmd, cwd, root)
        if new == cmd:
            return None
        d = dict(data)
        d["tool_input"] = dict(ti)
        d["tool_input"]["command"] = new
        return d
    fp = ti.get("file_path")
    if not isinstance(fp, str) or _bb4_os.path.isabs(fp):
        return None
    rel = _bb4_repo_rel(_bb4_os.path.join(cwd, fp), root)
    if not rel or rel == fp:
        return None
    d = dict(data)
    d["tool_input"] = dict(ti)
    d["tool_input"]["file_path"] = rel
    return d


def _bb4_precheck(data):
    """守卫的正式判定之前先走一遍。返回原 data（途中可能直接 sys.exit）。"""
    if _bb4_os.environ.get(_BB4_INNER) == "1":
        return data                      # 第二遍：只跑原判据，别套娃
    try:
        if data.get("tool_name") == "Bash":
            cmd = (data.get("tool_input") or {}).get("command")
            if isinstance(cmd, str) and "\n" not in cmd:
                toks = cmd.split()
                if (toks[:2] == ["cargo", "fmt"]
                        and set(toks) <= (_BB4_FMT_OK | _BB4_FMT_QUERY)
                        and set(toks) & _BB4_FMT_QUERY):
                    _bb4_sys.exit(0)     # ② 纯查询，一个字节都不写
        root = _bb4_repo_root()
        norm = _bb4_normalize(data, _bb4_os.getcwd(), root)
        if norm is None:
            return data
        env = dict(_bb4_os.environ)
        env[_BB4_INNER] = "1"
        r = _bb4_subprocess.run(
            [_bb4_sys.executable, _bb4_os.path.abspath(__file__)],
            input=_bb4_json.dumps(norm), capture_output=True, text=True,
            env=env, timeout=15)
    except SystemExit:
        raise
    except BaseException:
        return data                      # 出意外就退回原判定，原有保护一条不少
    if r.returncode != 0:
        _bb4_sys.stderr.write(r.stderr)
        _bb4_sys.exit(r.returncode)
    return data


def _bb4_precheck_raw(text):
    """payload 还是字符串那一形态时的入口。解析不了就原样放过（交给原逻辑报错）。"""
    try:
        data = _bb4_json.loads(text)
    except SystemExit:
        raise
    except BaseException:
        return text
    _bb4_precheck(data)
    return text

# === BB4 注入结束 =============================================================
'''


# =============================================================================
# AST 锚点
# =============================================================================


def _seg(src: str, node: ast.AST) -> str:
    """节点在源码里的原文。"""
    return ast.get_source_segment(src, node) or ""


def _has_stdin(node: ast.AST) -> bool:
    return any(
        isinstance(n, ast.Attribute) and n.attr == "stdin" for n in ast.walk(node)
    )


def find_stdin_anchor(src: str):
    """找**唯一**一处从 stdin 读 payload 的调用。

    返回 (node, kind)；kind 是 "data"（拿到的是 dict）或 "raw"（拿到的是 str）。
    候选之间是「或」，但**三个候选合起来必须恰好命中 1 处**，否则整份拒写。
    """
    tree = ast.parse(src)
    data_hits, raw_hits = [], []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        f = node.func
        name = f.attr if isinstance(f, ast.Attribute) else (
            f.id if isinstance(f, ast.Name) else None
        )
        if name in ("load", "loads") and _has_stdin(node):
            data_hits.append(node)
        elif (
            name == "read"
            and isinstance(f, ast.Attribute)
            and _has_stdin(f)
        ):
            raw_hits.append(node)
    # `json.loads(sys.stdin.read())` 会同时命中两边 —— 外层那个才是要包的。
    if data_hits:
        raw_hits = [
            n for n in raw_hits
            if not any(n in set(ast.walk(d)) for d in data_hits)
        ]
    hits = [(n, "data") for n in data_hits] + [(n, "raw") for n in raw_hits]
    return hits


def find_import_anchor(src: str) -> int:
    """模块 import 段结束的行号（1-based，注入点在这一行**之前**）。"""
    tree = ast.parse(src)
    line = None
    for node in tree.body:
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            continue
        if isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant) \
                and isinstance(node.value.value, str):
            continue                      # 模块 docstring
        line = node.lineno
        break
    if line is None:                      # 整个文件只有 import 和 docstring
        line = len(src.splitlines()) + 1
    return line


def patch_guard(src: str, report) -> str | None:
    """给 guard_bash.py 注入那一层。对不上就返回 None。"""
    if "_bb4_precheck" in src:
        report("[  已打过] guard_bash.py: 里面已经有 _bb4_precheck，不用再跑")
        return None

    hits = find_stdin_anchor(src)
    if len(hits) != 1:
        report(f"[命中 {len(hits)} 处] guard_bash.py: 锚点 A（从 stdin 读 payload 的调用）"
               f"—— 期望恰好 1 处")
        if hits:
            for n, kind in hits:
                report(f"           └ 第 {n.lineno} 行（{kind}）：{_seg(src, n)}")
        else:
            report("           └ 一处都没找到：这个守卫读 payload 的写法本补丁没预料到"
                   "（不是 json.load*(…sys.stdin…)，也不是 sys.stdin.read()）。"
                   "人工核一遍再说，别硬来。")
        return None

    node, kind = hits[0]
    orig = _seg(src, node)
    fn = "_bb4_precheck" if kind == "data" else "_bb4_precheck_raw"
    report(f"[  命中 1] guard_bash.py: 锚点 A 在第 {node.lineno} 行（{kind}）")
    report(f"           └ 包裹：{orig}  →  {fn}({orig})")

    lines = src.splitlines(keepends=True)

    # 先包裹（从后往前改，免得行号漂）
    s_line, s_col = node.lineno - 1, node.col_offset
    e_line, e_col = node.end_lineno - 1, node.end_col_offset
    if s_line == e_line:
        ln = lines[s_line]
        lines[s_line] = ln[:s_col] + fn + "(" + ln[s_col:e_col] + ")" + ln[e_col:]
    else:
        lines[e_line] = lines[e_line][:e_col] + ")" + lines[e_line][e_col:]
        lines[s_line] = lines[s_line][:s_col] + fn + "(" + lines[s_line][s_col:]

    imp = find_import_anchor(src)
    report(f"[  命中 1] guard_bash.py: 锚点 B（import 段结束）在第 {imp} 行，注入点在它之前")
    lines.insert(imp - 1, SHIM.lstrip("\n") + "\n")

    out = "".join(lines)
    try:
        ast.parse(out)
    except SyntaxError as exc:
        report(f"[  语法炸] 改完的 guard_bash.py 不是合法 Python（{exc}）")
        return None
    return out


# =============================================================================
# guard.rs 的 characterization 挪位（锚点是逐字的 —— 这个文件本轨读得到）
# =============================================================================

FMT_MOVED = '''        // ↓ 真误拦，钉的是现状
        ("--version 纯查询，一个字节不写", "cargo fmt --version"),
        ("--help 纯查询，一个字节不写", "cargo fmt --help"),
        // 短选项同病（BB4 补量 —— AA3 只量了长选项那两个）
        ("-V 纯查询", "cargo fmt -V"),
        ("-h 纯查询", "cargo fmt -h"),
        ("--all 配 --version 也拦", "cargo fmt --all --version"),
'''

RS_EDITS = [
    (
        "guard.rs: cargo fmt 的五条查询选项从 blocked 里摘掉",
        FMT_MOVED,
        "",
    ),
    (
        "guard.rs: 同五条挪进 allowed",
        '''    let allowed: &[(&str, &str)] = &[
        ("--check 在前", "cargo fmt --check --all"),''',
        '''    let allowed: &[(&str, &str)] = &[
        // ↓ BB4 的补丁把这五条从 blocked 挪过来了：纯查询，一个字节都不写。
        //   白名单极窄（词必须全落在 {cargo, fmt, --all, --quiet, -q} ∪
        //   {--version, -V, --help, -h} 里），按词不按子串。
        ("--version 纯查询", "cargo fmt --version"),
        ("--help 纯查询", "cargo fmt --help"),
        ("-V 纯查询", "cargo fmt -V"),
        ("-h 纯查询", "cargo fmt -h"),
        ("--all 配 --version", "cargo fmt --all --version"),
        ("--check 在前", "cargo fmt --check --all"),''',
    ),
    (
        "guard.rs: cd 矩阵从「放行」改成「拦」",
        '''    for (label, cmd) in cd_slips {
        assert_eq!(
            bash(cmd),
            0,
            "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改，别当回归失败：{label}"
        );
    }''',
        '''    for (label, cmd) in cd_slips {
        assert_eq!(
            bash(cmd),
            BLOCKED,
            "cd 之后写冻结面又漏过去了 —— BB4 的归一化那一层没生效：{label}"
        );
    }''',
    ),
    (
        "guard.rs: cwd 那条（core 下写契约）",
        '''        bash_in("core", "echo x > crates/contracts/src/lib.rs"),
        0,
        "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改"''',
        '''        bash_in("core", "echo x > crates/contracts/src/lib.rs"),
        BLOCKED,
        "cwd 落在 core/ 时写契约又漏过去了 —— BB4 的归一化那一层没生效"''',
    ),
    (
        "guard.rs: cwd 那条（src 下写 lib.rs）",
        '''        bash_in("core/crates/contracts/src", "echo x > lib.rs"),
        0,
        "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改"''',
        '''        bash_in("core/crates/contracts/src", "echo x > lib.rs"),
        BLOCKED,
        "cwd 就在冻结面里时写裸文件名又漏过去了 —— BB4 的归一化那一层没生效"''',
    ),
    (
        "guard.rs: cwd 那条（core 下 Write 相对路径）",
        '''        tool_in("core", "Write", "crates/contracts/src/lib.rs"),
        0,
        "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改"''',
        '''        tool_in("core", "Write", "crates/contracts/src/lib.rs"),
        BLOCKED,
        "cwd 落在 core/ 时 Write 相对路径又漏过去了 —— BB4 的归一化那一层没生效"''',
    ),
    (
        "guard.rs: cwd 那条（src 下 Write lib.rs）",
        '''        tool_in("core/crates/contracts/src", "Write", "lib.rs"),
        0,
        "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改"''',
        '''        tool_in("core/crates/contracts/src", "Write", "lib.rs"),
        BLOCKED,
        "cwd 就在冻结面里时 Write 裸文件名又漏过去了 —— BB4 的归一化那一层没生效"''',
    ),
]


def patch_tests(src: str, report) -> str | None:
    if "BB4 的归一化那一层没生效" in src:
        report("[  已打过] guard.rs: characterization 已经挪过位了")
        return None
    ok = True
    for label, old, new in RS_EDITS:
        n = src.count(old)
        if n == 1:
            report(f"[  命中 1] {label}")
            src = src.replace(old, new, 1)
        else:
            report(f"[命中 {n} 次] {label} —— 期望 1")
            ok = False
    return src if ok else None


# =============================================================================
# 自验用的合成件
# =============================================================================

FIXTURE_HEADS = {
    1: "    data = json.loads(sys.stdin.read())\n",
    2: "    data = json.load(sys.stdin)\n",
    3: "    raw = sys.stdin.read()\n    data = json.loads(raw)\n",
}

FIXTURE_TMPL = '''#!/usr/bin/env python3
"""合成的迷你守卫 —— 只给 review/bb4-guard-patch.py 自验用，不是真守卫。

行为按 BB4 黑盒实测的真守卫复刻：前缀族按路径段边界匹配、READ_SAFE 决定读/写位置、
cargo fmt 看命令的词里有没有 --check。
"""
import json
import re
import sys

PROT_PREFIXES = ("proto/", "core/crates/contracts/")
PROT_PATHS = {".claude/hooks/guard_bash.py": False, ".contracts.lock": False}
# cd 在 READ_SAFE 里、pushd 不在 —— BB4 实测（cd 到保护面放行、pushd 到保护面拦）。
# 这条正是「先 cd 就绕过」那个漏拦的机制之一：cd 那一段本身不触发写入判定，
# 而后一段里的相对路径又不带前缀，两头都落空。
READ_SAFE = ("cat", "grep", "ls", "head", "wc", "echo", "diff", "cd")


def blocked(face, why):
    sys.stderr.write("blocked: 该操作触碰受保护面 %s（%s）。停止当前工作并向人类报告。\\n"
                     % (face, why))
    sys.exit(2)


def hits_prefix(text):
    low = text.lower()
    for p in PROT_PREFIXES:
        for m in re.finditer(re.escape(p.lower()), low):
            if m.start() == 0 or low[m.start() - 1] in "/ \\t\\"'>|;&":
                return p + "**"
    return None


def judge(data):
    name = data.get("tool_name")
    ti = data.get("tool_input") or {}
    if name == "Bash":
        cmd = ti.get("command") or ""
        toks = cmd.split()
        if toks[:2] == ["cargo", "fmt"] and "--check" not in toks:
            blocked("core/crates/contracts/**",
                    "cargo fmt 写模式（用 cargo fmt --check）")
        face = hits_prefix(cmd)
        if face:
            # **按段判**，不是按整条命令判 —— 真守卫就是这样（BB4 实测：
            # cd 到保护面那一段本身不触发写入判定，即使整条命令后面有重定向）。
            for seg in re.split(r"&&|\\|\\||;|\\||\\n", cmd):
                if not hits_prefix(seg):
                    continue
                st = seg.split()
                head = st[0] if st else ""
                if head in READ_SAFE and ">" not in seg and "tee" not in st:
                    continue
                blocked(face, "写入/执行位置")
        return
    fp = ti.get("file_path") or ""
    face = hits_prefix(fp)
    if face and name in ("Write", "Edit", "NotebookEdit"):
        blocked(face, "写入/执行位置")


def main():
__HEAD__    judge(data)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
'''


def make_fixture(root: pathlib.Path, shape: int) -> None:
    g = root / GUARD_REL
    g.parent.mkdir(parents=True, exist_ok=True)
    src = FIXTURE_TMPL.replace("__HEAD__", FIXTURE_HEADS[shape])
    g.write_text(src, encoding="utf-8")
    t = root / TESTS_REL
    t.parent.mkdir(parents=True, exist_ok=True)
    # **总是**重新铺一份 —— 合成件必须是确定性的，不能带上一轮跑剩的状态，
    # 不然「已经打过了」那一格验到的会是半拉子状态而不是它本身。
    t.write_text((REPO_ROOT / TESTS_REL).read_text(encoding="utf-8"), encoding="utf-8")
    print(f"合成件写好了（形状 {shape}）：{g}")


# =============================================================================
# main
# =============================================================================


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="只报告命中情况，不写盘")
    ap.add_argument("--root", default=None,
                    help="改哪个树（默认本仓库根）。**只给自验用**")
    ap.add_argument("--make-fixture", default=None, metavar="DIR",
                    help="生成一份合成守卫（自验用）")
    ap.add_argument("--shape", type=int, default=1, choices=sorted(FIXTURE_HEADS),
                    help="合成守卫从 stdin 读 payload 的写法（1/2/3）")
    args = ap.parse_args()

    if args.make_fixture:
        make_fixture(pathlib.Path(args.make_fixture).resolve(), args.shape)
        return 0

    root = pathlib.Path(args.root).resolve() if args.root else REPO_ROOT

    # 守卫**故意**拦自我授权（`AITE_RELOCK=1 …` 判「授权变量赋值」直接拦，
    # `tests/guard.rs::relock_and_self_authorization_are_blocked` 钉着）。
    # 真改 .claude/** 只能由人开那道门 —— 脚本自己再验一次。
    if args.root is None and os.environ.get("AITE_RELOCK") != "1":
        print("这份补丁改的是 .claude/** —— 守卫的保护面，故意只让人跑。\n"
              "  人跑：AITE_RELOCK=1 python3 review/bb4-guard-patch.py --check\n"
              "  自验：python3 review/bb4-guard-patch.py --root <临时目录> --check",
              file=sys.stderr)
        return 1

    guard_p, tests_p = root / GUARD_REL, root / TESTS_REL
    for p in (guard_p, tests_p):
        if not p.exists():
            print(f"找不到 {p}", file=sys.stderr)
            return 1

    lines: list[str] = []
    report = lines.append

    g_src = guard_p.read_text(encoding="utf-8")
    t_src = tests_p.read_text(encoding="utf-8")
    g_new = patch_guard(g_src, report)
    t_new = patch_tests(t_src, report)

    for ln in lines:
        print(ln)
    sys.stdout.flush()      # stderr 是无缓冲的，不 flush 的话报告会排到错误后面去

    if g_new is None or t_new is None:
        # 「已经打过了」和「锚点对不上」是两回事，别用同一句话打发人
        if all("已打过" in ln for ln in lines if ln.startswith("[")):
            print("\n这份补丁已经打过了（守卫里有 _bb4_precheck，测试也挪过位了）。"
                  "不用再跑。", file=sys.stderr)
            return 1
        print("\n对不上（一个字节都没写）：", file=sys.stderr)
        if g_new is None:
            print("  - guard_bash.py：见上面的锚点报告", file=sys.stderr)
        if t_new is None:
            print("  - guard.rs：锚点没唯一命中，多半是它在 BB4 之后又动过",
                  file=sys.stderr)
        print("\n**守卫和钉它的测试必须同一次落盘** —— 一个对不上就两个都不写。",
              file=sys.stderr)
        return 1

    if args.check:
        print("\n--check：没写盘。改完会多出这两件事：")
        print("  1. guard_bash.py：payload 进判定之前先过 _bb4_precheck（路径按 cwd 归一化）")
        print("  2. guard.rs：cargo fmt 的五条查询选项挪进 allowed，"
              "cd / cwd 那两批 characterization 从 0 改成 BLOCKED")
        return 0

    guard_p.write_text(g_new, encoding="utf-8")
    print(f"写了 {guard_p}")
    tests_p.write_text(t_new, encoding="utf-8")
    print(f"写了 {tests_p}")

    # 落盘后读回来再验一遍 —— 写坏了守卫整个不加载，那是最坏的那种静默失效。
    back = guard_p.read_text(encoding="utf-8")
    try:
        ast.parse(back)
    except SyntaxError as exc:
        print(f"\n写完了但 {guard_p.name} 不是合法 Python（{exc}）—— 别信这次运行，人工核",
              file=sys.stderr)
        return 1
    if "_bb4_precheck" not in back:
        print(f"\n写完了但 {guard_p.name} 里找不到注入的那一层 —— 别信这次运行，人工核",
              file=sys.stderr)
        return 1

    print("\n接着跑：cd core && cargo test -p aite --test guard")
    print("  期望 21 passed；跑之前它有 7 条钉着旧行为，跑之后必须全绿。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
