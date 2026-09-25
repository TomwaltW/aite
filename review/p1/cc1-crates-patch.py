#!/usr/bin/env python3
"""CC1 工作项 6：五个空骨架 crate（admin / search / githost / routines / memory）+ 三个 Cargo 文件。

**为什么是补丁脚本**：`core/Cargo.toml` 在 CC1 可写面里，但云端会话的权限规则拒写它
（Edit 工具报 `File is in a directory that is denied by your permission settings`，重试同样被拒）。
不换写法绕，改成本脚本交总管本机跑。形状照 `review/aa4-proto-patch.py`：

- 锚点恰好命中一次（`core/Cargo.toml` 两处、`core/crates/app/Cargo.toml` 一处），五个 crate 只新建、不覆盖；
- 全有或全无：任何一条对不上 / 验不过，一个字节都不写；
- `--check` 干跑；`--root <副本>` 在仓库副本上自测（CC1 在云端就是这么验的，见回执 CC1.md）；
- 有效性闸门：五个 lib.rs 过 rustfmt、没有 doctest 形状；在临时副本里 `cargo metadata` 让 cargo 做最小改锁，
  要求 Cargo.lock 恰好多 5 个 `aite-*` 包、零删除行（禁止 `cargo update` / `cargo generate-lockfile`）。

不碰守卫保护面，所以不要求 `AITE_RELOCK=1`。

**用法**（在仓库根跑，逐步）：

    python3 review/p1/cc1-crates-patch.py --check    # 期望：[命中 1 条] x3、两行 [  合法]、列出 13 个文件
    python3 review/p1/cc1-crates-patch.py            # 期望末尾：改了 core/Cargo.lock …、[读回 OK]
    git diff --numstat -- core/Cargo.lock           # 期望：「<加行数> 0 core/Cargo.lock」
    git diff -- core/Cargo.lock | grep '^+name = '  # 期望恰好 5 行，全是 aite-*
    scripts/check.sh                                # 期望：全部通过，cargo passed 与跑前相同（Δ=0）
    git add core/Cargo.toml core/Cargo.lock core/crates/app/Cargo.toml \
        core/crates/admin core/crates/search core/crates/githost core/crates/routines core/crates/memory
"""

from __future__ import annotations

import argparse
import difflib
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

# 五个骨架 crate 的全文（CC1 起草；依赖逐 crate 的理由写在各自 Cargo.toml 的注释里，回执第 6 节另有汇总）。
CRATES = {
    'admin': {
        "Cargo.toml": '[package]\nname = "aite-admin"\ndescription = "管理台（axum 服务端渲染，`aite admin serve`）。骨架 crate：CC1 预建，W2 由 DD12 填。owner: DD12"\nversion.workspace = true\nedition.workspace = true\nrust-version.workspace = true\npublish.workspace = true\n\n# 依赖是 CC1 按 DD12 原卡预先声明的，全部取自 [workspace.dependencies]、都已在 Cargo.lock 里，不是新依赖。\n# 这一份刻意「宁少勿多」：DD12 本来就要加 axum 并改锁（计划 D4 / D12），缺什么届时一并加。\n# 不声明 aite-gateway：管理台只读写配置与证据，不注册工具。\n# aite-contracts 取 Scope / Bundle / 证据的类型；serde / serde_json 出表单与 JSON 接口；\n# tokio / async-trait 是服务端与存储端口的异步面；thiserror 出错误类型；tracing 记登录与审计日志；\n# chrono 给会话超时、登录锁定窗口算时间。\n[dependencies]\naite-contracts.workspace = true\nserde.workspace = true\nserde_json.workspace = true\ntokio.workspace = true\nasync-trait.workspace = true\nthiserror.workspace = true\ntracing.workspace = true\nchrono.workspace = true\n\n[dev-dependencies]\ntokio.workspace = true\ntempfile.workspace = true\n',
        "lib.rs": '//! aite-admin —— 管理台 v0（CT24 的配对与上线页、CT16 的 Scope / Bundle 编辑、\n//! CT27 的审计页）。owner: DD12（W2），之后 EE9 / FF7 往上加页面。\n//!\n//! 现在是空的：CC1（W1）预建这个 crate，只为把它在工作区清单与 `Cargo.lock` 里的\n//! 位置先占好，免得 W2 / W3 好几轨同时改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。\n//!\n//! 计划里它会装：\n//!\n//! - `aite admin serve` 的 axum 服务端渲染（D4：只用 Rust，无前端构建链）\n//! - 平台 OAuth 与一次性引导令牌登录，等保二级的登录锁定、会话超时、CSRF、转义\n//! - Scope / Bundle / Skills 编辑页、任务证据只读页、「模型与备案」面板\n//!\n//! 依赖为什么这么少：DD12 本来就要加 axum 并改锁（计划 D4 / D12 已批），\n//! 其余缺的届时一并加；预声明的理由见本 crate 的 `Cargo.toml`。\n',
    },
    'search': {
        "Cargo.toml": '[package]\nname = "aite-search"\ndescription = "服务端联网搜索（SearchPort）与工作区搜索工具。骨架 crate：CC1 预建，W3 由 EE4 填。owner: EE4"\nversion.workspace = true\nedition.workspace = true\nrust-version.workspace = true\npublish.workspace = true\n\n# 依赖是 CC1 预先声明的：W3 谁都不改 Cargo.toml / Cargo.lock（计划 §6.4），漏了只能停下报告。\n# 全部取自 [workspace.dependencies]、都已在 Cargo.lock 里，不是新依赖。\n# aite-gateway 是定死的（原卡 [REVISION 2026-09-25]）：web_search 等工具实现 CC4 放在 gateway 里的 GatewayTool trait。\n# reqwest：联网搜索在服务端跑、直接发 HTTP（博查 / 智谱 / 千帆，计划 CT19），不经沙箱代理。\n# rusqlite：工作区搜索的本地消息索引（派单点名的候选）；用不上也不花钱（bundled 已在锁里）。\n# serde / serde_json 解析搜索结果；tokio / async-trait / thiserror / tracing 同其他 crate。\n[dependencies]\naite-contracts.workspace = true\naite-gateway.workspace = true\nreqwest.workspace = true\nserde.workspace = true\nserde_json.workspace = true\ntokio.workspace = true\nasync-trait.workspace = true\nthiserror.workspace = true\ntracing.workspace = true\nrusqlite.workspace = true\n\n[dev-dependencies]\ntokio.workspace = true\ntempfile.workspace = true\naite-testing.workspace = true\n',
        "lib.rs": '//! aite-search —— 联网搜索与工作区搜索（CT08、CT11、CT19）。owner: EE4（W3）。\n//!\n//! 现在是空的：CC1（W1）预建这个 crate，把它在工作区清单与 `Cargo.lock` 里的位置\n//! 与依赖先占好 —— W3 谁都不改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。\n//!\n//! 计划里它会装：\n//!\n//! - `SearchPort`：默认博查或智谱 web search，百度千帆兜底；在服务端跑、不经沙箱代理，\n//!   密钥走环境变量，结果包成 `<external>` 再进上下文，引用出处\n//! - `web_search` / `search_messages` / `search_docs` 工具（实现 gateway 里的\n//!   `GatewayTool`，走 catalog）；跨群结果按请求者成员资格过滤，外部群默认不开\n//!\n//! 依赖为什么预声明：`aite-gateway` 是为了实现 `GatewayTool`（原卡定死）；\n//! `reqwest` 发搜索请求；其余理由见本 crate 的 `Cargo.toml`。\n',
    },
    'githost': {
        "Cargo.toml": '[package]\nname = "aite-githost"\ndescription = "GitHost 端口与 GitLab / 极狐适配（提交 API 建分支、Draft MR）。骨架 crate：CC1 预建，W3 由 EE5 填。owner: EE5"\nversion.workspace = true\nedition.workspace = true\nrust-version.workspace = true\npublish.workspace = true\n\n# 依赖是 CC1 预先声明的：W3 谁都不改 Cargo.toml / Cargo.lock（计划 §6.4），漏了只能停下报告。\n# 全部取自 [workspace.dependencies]、都已在 Cargo.lock 里，不是新依赖。\n# aite-gateway 是定死的（原卡 [REVISION 2026-09-25]）：git 工具实现 CC4 放在 gateway 里的 GatewayTool trait。\n# reqwest 调 GitLab REST（项目访问令牌只在服务端，不进沙箱）；\n# sha2 / hex 给提交内容与归档算摘要（提交 API 的 content 校验、证据里记摘要）；\n# uuid 给分支名去重；serde / serde_json 出 API 请求体；tokio / async-trait / thiserror / tracing 同其他 crate。\n[dependencies]\naite-contracts.workspace = true\naite-gateway.workspace = true\nreqwest.workspace = true\nserde.workspace = true\nserde_json.workspace = true\ntokio.workspace = true\nasync-trait.workspace = true\nthiserror.workspace = true\ntracing.workspace = true\nsha2.workspace = true\nhex.workspace = true\nuuid.workspace = true\n\n[dev-dependencies]\ntokio.workspace = true\ntempfile.workspace = true\naite-testing.workspace = true\n',
        "lib.rs": '//! aite-githost —— Git 托管（CT12、CT15、NEW24）。owner: EE5（W3）。\n//!\n//! 现在是空的：CC1（W1）预建这个 crate，把它在工作区清单与 `Cargo.lock` 里的位置\n//! 与依赖先占好 —— W3 谁都不改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。\n//!\n//! 计划里它会装：\n//!\n//! - `GitHost` 端口与 GitLab / 极狐适配：服务端拉归档进 `/work/repo`、提交 API 建分支、\n//!   开 `Draft: ` MR 并反链话题；MR 作者是 bot 用户，项目访问令牌不进沙箱\n//! - Gitee 只做桩；GitHub / Codeup / Gitea 在 P1 报「未实现」\n//! - git 相关工具（实现 gateway 里的 `GatewayTool`，走 catalog）\n//!\n//! 依赖为什么预声明：`aite-gateway` 是为了实现 `GatewayTool`（原卡定死）；\n//! `reqwest` 调 GitLab API；其余理由见本 crate 的 `Cargo.toml`。\n//! 注意：归档若是 tar.gz，解包要的 crate 不在 `[workspace.dependencies]` 里，\n//! 那是新第三方依赖，EE5 届时要停下报告（CC1 回执已记账）。\n',
    },
    'routines': {
        "Cargo.toml": '[package]\nname = "aite-routines"\ndescription = "例程：定时 / 跟进 / 频道观察的调度与 schedule_followup 工具。骨架 crate：CC1 预建，W3 由 EE2 填。owner: EE2"\nversion.workspace = true\nedition.workspace = true\nrust-version.workspace = true\npublish.workspace = true\n\n# 依赖是 CC1 预先声明的：W3 谁都不改 Cargo.toml / Cargo.lock（计划 §6.4），漏了只能停下报告。\n# 全部取自 [workspace.dependencies]、都已在 Cargo.lock 里，不是新依赖。\n# aite-gateway 是定死的（原卡 [REVISION 2026-09-25]）：例程工具实现 CC4 放在 gateway 里的 GatewayTool trait。\n# chrono 算 next_run（Asia/Shanghai 固定 +08:00，不引入时区库，计划 NEW20）；\n# rusqlite：routines 表（计划 CT22）若落在本 crate 自己的库里就要它；uuid 给例程编号；\n# serde / serde_json 出工具参数与结果；tokio 跑调度循环；async-trait / thiserror / tracing 同其他 crate。\n[dependencies]\naite-contracts.workspace = true\naite-gateway.workspace = true\nchrono.workspace = true\nserde.workspace = true\nserde_json.workspace = true\ntokio.workspace = true\nasync-trait.workspace = true\nthiserror.workspace = true\ntracing.workspace = true\nrusqlite.workspace = true\nuuid.workspace = true\n\n[dev-dependencies]\ntokio.workspace = true\ntempfile.workspace = true\naite-testing.workspace = true\n',
        "lib.rs": '//! aite-routines —— 例程（CT22、NEW20、NEW21）。owner: EE2（W3）。\n//!\n//! 现在是空的：CC1（W1）预建这个 crate，把它在工作区清单与 `Cargo.lock` 里的位置\n//! 与依赖先占好 —— W3 谁都不改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。\n//!\n//! 计划里它会装：\n//!\n//! - `next_run` 计算：按 Asia/Shanghai 固定 +08:00，不引入时区库\n//! - 调度循环（control 内部入口，绕开 `ControlPlane` trait）\n//! - `schedule_followup` 等例程工具（实现 gateway 里的 `GatewayTool`，走 catalog）\n//! - 输出目标校验：所在群、已授权的公开群、创建者私聊\n//!\n//! 依赖为什么预声明：`aite-gateway` 是为了实现 `GatewayTool`（原卡定死）；\n//! `chrono` 算下次运行时间；其余理由见本 crate 的 `Cargo.toml`。\n',
    },
    'memory': {
        "Cargo.toml": '[package]\nname = "aite-memory"\ndescription = "按群 / 工作区的记忆文件与 memory_read / memory_write 工具。骨架 crate：CC1 预建，W3 由 EE1 填。owner: EE1"\nversion.workspace = true\nedition.workspace = true\nrust-version.workspace = true\npublish.workspace = true\n\n# 依赖是 CC1 预先声明的：W3 谁都不改 Cargo.toml / Cargo.lock（计划 §6.4），漏了只能停下报告。\n# 全部取自 [workspace.dependencies]、都已在 Cargo.lock 里，不是新依赖。\n# aite-gateway 是定死的（原卡 [REVISION 2026-09-25]）：记忆工具实现 CC4 放在 gateway 里的 GatewayTool trait。\n# rusqlite：每群一份记忆（计划 CT21「每群一份记忆文件（SQLite）」）；bundled，不引入系统库。\n# uuid 给记忆条目编号；chrono 盖写入时间；serde / serde_json 出工具参数与结果；\n# tokio / async-trait 是工具与存储的异步面；thiserror 出错误类型；tracing 记审计日志。\n[dependencies]\naite-contracts.workspace = true\naite-gateway.workspace = true\nserde.workspace = true\nserde_json.workspace = true\ntokio.workspace = true\nasync-trait.workspace = true\nthiserror.workspace = true\ntracing.workspace = true\nchrono.workspace = true\nrusqlite.workspace = true\nuuid.workspace = true\n\n[dev-dependencies]\ntokio.workspace = true\ntempfile.workspace = true\naite-testing.workspace = true\n',
        "lib.rs": '//! aite-memory —— 记忆（CT21、NEW22）。owner: EE1（W3）。\n//!\n//! 现在是空的：CC1（W1）预建这个 crate，把它在工作区清单与 `Cargo.lock` 里的位置\n//! 与依赖先占好 —— W3 谁都不改 `Cargo.toml` / `Cargo.lock`（计划 §6.4）。\n//!\n//! 计划里它会装：\n//!\n//! - 按群 / 工作区的记忆存储（每群一份，SQLite）；公开群进工作区、私有群自存，\n//!   钉钉 / 企微一律按私有群\n//! - `memory_read` / `memory_write` 工具（实现 gateway 里的 `GatewayTool`，走 catalog）\n//! - 写入过滤：只存工作事实、带来源 task_id，拒收身份证号 / 手机号一类\n//!\n//! 依赖为什么预声明：`aite-gateway` 是为了实现 `GatewayTool`（原卡定死）；\n//! `rusqlite` 是记忆文件的存储；其余理由见本 crate 的 `Cargo.toml`。\n',
    },
}


REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
CRATE_NAMES = ["admin", "search", "githost", "routines", "memory"]

# --- core/Cargo.toml：两处锚点 ----------------------------------------------------

WS_COMMENT_OLD = "# 本仓库 crate（各轨只许依赖 contracts / proto / testing；跨轨依赖 → 停下报告）\n"
WS_COMMENT_NEW = (
    "# 本仓库 crate（各轨只许依赖 contracts / proto / testing；"
    "memory / routines / search / githost 四个骨架 crate 另许依赖 aite-gateway；其余跨轨依赖 → 停下报告）\n"
)
WS_DEPS_OLD = 'aite-evals = { path = "crates/evals" }\n'
WS_DEPS_NEW = WS_DEPS_OLD + (
    "# P1 骨架 crate（2026-09-25 CC1 预建，W2 / W3 各轨只写自己那一个目录，不改这里与 Cargo.lock）\n"
    'aite-admin = { path = "crates/admin" }\n'
    'aite-search = { path = "crates/search" }\n'
    'aite-githost = { path = "crates/githost" }\n'
    'aite-routines = { path = "crates/routines" }\n'
    'aite-memory = { path = "crates/memory" }\n'
)

# --- core/crates/app/Cargo.toml：一处锚点 -----------------------------------------

APP_OLD = "aite-evals.workspace = true\nclap.workspace = true\n"
APP_NEW = (
    "aite-evals.workspace = true\n"
    "# P1 骨架 crate：CC1 预先挂上，W2 / W3 接线时不必再改这份清单与 Cargo.lock（计划 §6.4）。\n"
    "# 现在都是空 crate，app 代码里没有 `use` 它们（接线归 CC4 / 各功能轨的 features/*.rs）。\n"
    "aite-admin.workspace = true\n"
    "aite-search.workspace = true\n"
    "aite-githost.workspace = true\n"
    "aite-routines.workspace = true\n"
    "aite-memory.workspace = true\n"
    "clap.workspace = true\n"
)


class Edit:
    """字面量精确替换，要求锚点恰好命中 1 次（0 次或 2 次都说明现状不是本补丁预料的形态）。"""

    def __init__(self, rel: str, label: str, old: str, new: str):
        self.rel, self.label, self.old, self.new = rel, label, old, new


EDITS = [
    Edit("core/Cargo.toml", "workspace 依赖表上方那句注释：放开四个骨架 crate 依赖 aite-gateway", WS_COMMENT_OLD, WS_COMMENT_NEW),
    Edit("core/Cargo.toml", "workspace 依赖表：aite-evals 之后加五条 aite-*", WS_DEPS_OLD, WS_DEPS_NEW),
    Edit("core/crates/app/Cargo.toml", "aite 的 [dependencies]：aite-evals 之后挂五个骨架 crate", APP_OLD, APP_NEW),
]


def new_files() -> dict[str, str]:
    out = {}
    for name in CRATE_NAMES:
        out[f"core/crates/{name}/Cargo.toml"] = CRATES[name]["Cargo.toml"]
        out[f"core/crates/{name}/src/lib.rs"] = CRATES[name]["lib.rs"]
    return out


def stage(root: pathlib.Path) -> tuple[dict[str, str], list[str]]:
    """算出所有要写的文件内容（相对路径 → 新内容）。有问题就返回问题清单、什么都不写。"""
    problems: list[str] = []
    staged: dict[str, str] = {}
    texts: dict[str, str] = {}
    for e in EDITS:
        p = root / e.rel
        if not p.exists():
            problems.append(f"找不到 {e.rel}")
            continue
        texts.setdefault(e.rel, p.read_text(encoding="utf-8"))
    if problems:
        return {}, problems
    for e in EDITS:
        n = texts[e.rel].count(e.old)
        if n == 1:
            print(f"[命中 1 条] {e.rel}: {e.label}")
            texts[e.rel] = texts[e.rel].replace(e.old, e.new, 1)
        else:
            hint = "看起来已经打过了（新内容已在文件里）" if e.new.splitlines()[-1] in texts[e.rel] and n == 0 else "原文被人动过，人工核一遍再改锚点"
            print(f"[命中 {n} 条] {e.rel}: {e.label} —— {hint}")
            problems.append(f"{e.rel}: 锚点命中 {n} 条（要求恰好 1 条）")
    staged.update(texts)
    for rel, body in new_files().items():
        if (root / rel).exists():
            problems.append(f"{rel} 已经存在 —— 本补丁只新建，不覆盖")
        staged[rel] = body
    return staged, problems


def check_rustfmt(root: pathlib.Path, staged: dict[str, str]) -> str | None:
    """lib.rs 必须是 rustfmt 形态（A4b 的 cargo fmt --check 是硬判据）。在 core/ 下跑，工具链由 rust-toolchain.toml 钉。"""
    for rel, body in staged.items():
        if not rel.endswith(".rs"):
            continue
        try:
            r = subprocess.run(
                ["rustfmt", "--edition", "2024", "--emit", "stdout"],
                input=body, capture_output=True, text=True, cwd=root / "core", check=False,
            )
        except FileNotFoundError:
            return "找不到 rustfmt —— 装上再跑，别跳过。"
        if r.returncode != 0 or r.stdout != body:
            return f"{rel} 不是 rustfmt 形态：\n{r.stderr.strip() or '（格式有出入）'}"
        if "```" in body or "\n//!     " in body:
            return f"{rel} 的 //! 里有代码块或 4 空格缩进行 —— 会变成 doctest，cargo passed 就不是基线了"
    return None


def lock_packages(text: str) -> list[str]:
    return re.findall(r'^name = "([^"]+)"$', text, flags=re.M)


def check_lock(old: str, new: str) -> str | None:
    """锁的形状：恰好多 5 个 aite-* 包，零删除行。"""
    removed = [l for l in difflib.ndiff(old.splitlines(), new.splitlines()) if l.startswith("- ")]
    if removed:
        return "Cargo.lock 有删除行（不是最小插入）：\n" + "\n".join(removed[:20])
    added = sorted(set(lock_packages(new)) - set(lock_packages(old)))
    want = sorted(f"aite-{n}" for n in CRATE_NAMES)
    if added != want:
        return f"Cargo.lock 新增的包是 {added}，期望恰好 {want}"
    return None


def relock(root: pathlib.Path) -> str | None:
    """让 cargo 做最小插入（等价于 cargo build 时顺手改锁）；绝不 update / generate-lockfile。"""
    for extra in (["--offline"], []):
        r = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", *extra],
            cwd=root / "core", capture_output=True, text=True, check=False,
        )
        if r.returncode == 0:
            return None
    return f"cargo metadata 失败：\n{r.stderr.strip()}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="干跑：报告命中与合法性，不写盘")
    ap.add_argument("--root", default=None, help="改哪棵树（默认本仓库根）；自验时指向 /tmp 下的仓库副本")
    args = ap.parse_args()
    root = pathlib.Path(args.root).resolve() if args.root else REPO_ROOT

    staged, problems = stage(root)
    if problems:
        print("\n对不上的地方（一个字节都没写）：", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    why = check_rustfmt(root, staged)
    if why:
        print(f"\n没过合法性检查，一个字节都没写：\n{why}", file=sys.stderr)
        return 1
    print("[  合法] 五个 lib.rs：rustfmt 认、没有 doctest 形状")

    # 有效性闸门：在临时副本里真解析一遍工作区并改锁，看锁的形状对不对。验不过同样一个字节都不写。
    lock_rel = "core/Cargo.lock"
    old_lock = (root / lock_rel).read_text(encoding="utf-8")
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        shutil.copytree(root / "core", tmp / "core", ignore=shutil.ignore_patterns("target"))
        for rel, body in staged.items():
            (tmp / rel).parent.mkdir(parents=True, exist_ok=True)
            (tmp / rel).write_text(body, encoding="utf-8")
        why = relock(tmp) or check_lock(old_lock, (tmp / lock_rel).read_text(encoding="utf-8"))
    if why:
        print(f"\n临时副本里的锁没过闸门，一个字节都没写：\n{why}", file=sys.stderr)
        return 1
    print("[  合法] 临时副本：cargo metadata 过、Cargo.lock 恰好多 5 个 aite-* 包、零删除行")

    if args.check:
        print("\n--check：没写盘。会写这些文件：")
        for rel in staged:
            print(f"  {rel}")
        return 0

    for rel, body in staged.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(body, encoding="utf-8")
        print(f"写了 {rel}")
    why = relock(root) or check_lock(old_lock, (root / lock_rel).read_text(encoding="utf-8"))
    if why:
        print(f"\n写完了但真树上的锁不对 —— 别提交，人工核：\n{why}", file=sys.stderr)
        return 1
    print(f"改了 {lock_rel}（恰好多 5 个 aite-* 包、零删除行）")
    for e in EDITS:
        back = (root / e.rel).read_text(encoding="utf-8")
        if e.new not in back:
            print(f"\n读回 {e.rel} 找不到新内容 —— 别提交，人工核", file=sys.stderr)
            return 1
    print("[读回 OK] 三处锚点都换成了新内容")
    print("\n接着跑：scripts/check.sh（期望 cargo passed 不变，A4a clippy 绿）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
