//! PreToolUse 守卫（`.claude/hooks/guard_bash.py`）的回归测试。
//!
//! **这是接管，不是新增。** 原来是 `tests/contracts/test_guard.py`（140 行、pytest 参数化），
//! 随整棵 Python 树在 `598f476` 一起删了，之后没有任何人接手。而守卫是这个仓库唯一的
//! **冻结面机器强制** —— 一个有洞的守卫比没有守卫更危险，因为它给人「契约有人看着」的
//! 错觉，而 hook 执行失败是**非阻塞放行且不报警**的。
//!
//! 旧用例指着 `aite/contracts/**` 的那些，逐条换成 Rust/proto 面上的等价物
//! （`core/crates/contracts/**` / `proto/**`）。两条随 Python 树作废、不搬：
//! `pytest tests/contracts -q` 与 `ruff check .`；`python3 -m aite.contracts.lock --check`
//! 换成 `aite contracts lock --check`。
//!
//! **为什么落在 Rust 集成测试里**：这样它自动进 `cargo test --workspace` → `scripts/check.sh`
//! 的 B 行 → CI，一行 `check.sh` 都不用改（而 `check.sh` 也不在 V6 的可写面里）。
//! 代价是这个 crate 的测试依赖外部 `python3`。
//!
//! **所以这里不 skip。** 守卫本身就是个 python3 脚本，跑不了 python3 的机器根本没有守卫
//! 可言 —— 那件事该红，不该被一个 skip 分支咽掉（这一轮审核刚抓到三条恒真断言，不再造
//! 第四条）。本机 python3 3.11.7 一定在，CI 是 ubuntu-latest 也一定在。
//!
//! **每次 spawn 都摘掉 `AITE_RELOCK`**：守卫在 `AITE_RELOCK=1` 时整体放行（进 `main()`
//! 之前第一件事就 `sys.exit(0)`）。不摘的话，只要有人用 `AITE_RELOCK=1 claude …` 起会话，
//! 这一整份测试就变成一堆恒真断言 —— 正是它要防的那种东西。
//!
//! **hook 命令现在长这样**（`review/z2-guard-patch.py` 落的那一条）：
//!
//! ```text
//! d=$(git rev-parse --show-toplevel 2>/dev/null); [ -f "$d/.claude/hooks/guard_bash.py" ] \
//!   || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"
//! ```
//!
//! 它是两条病史叠出来的，两条都真踩过：
//!
//! 1. **cwd 漂移（2026-09-12）**：原来写的是 `python3 "$CLAUDE_PROJECT_DIR/…"`，而
//!    `CLAUDE_PROJECT_DIR` 在 claude 进程启动那一刻定死 —— 会话在仓库子目录里起
//!    （或先起 claude 再 `cd`），它就指着子目录 → 文件不存在 → hook 执行失败 →
//!    **非阻塞放行、不报警** → 整场守卫静默失效。V6 (3c) 改成 `$(git rev-parse --show-toplevel)`
//!    治好了这一条。
//! 2. **仓库外把会话锁死（2026-09-13）**：(3c) 那一版没有恢复路径。会话 cwd 停在
//!    `~/.claude/projects/…/memory`（不在任何 git 仓库里）时，命令展开成
//!    `python3 "/.claude/hooks/guard_bash.py"` → 文件不存在 → 退出码 2 → PreToolUse
//!    读作**拦截** → `Bash` / `Read` / `Write` 全是同一条错。而**会话 cwd 只能靠 Bash 的
//!    `cd` 改，Bash 已经被拦** —— 会话变砖，只能由人重启。Z2 补的就是这一条。
//!
//! 回退判据是 **`[ -f ]`（那个根里到底有没有守卫）而不是 `||`（git 成功没有）**：cwd 落在
//! **另一个** git 仓库里时 `git` 会成功并返回那边的根，`||` 分支于是永远不触发，照样变砖。
//! 两个来源都不可用时仍然退出 2（**故意的** —— fail-closed 优先于不锁死）：命令的最后一句
//! 永远是 `python3 "$d/…"`，`$d` 算成什么都好，文件不在就是 2。`[ -f ]` 这一探只能改变
//! 「跑哪一份守卫」，改不了「到底跑不跑守卫」。
//!
//! 这三种 cwd 由 `the_hook_command_recovers_outside_the_repo_instead_of_bricking_the_session`
//! 真跑命令钉着，两个 payload 都断（该拦的拦、该放的放）——
//! 只断前者是恒真断言，一条把**一切**都拦掉的坏命令照样满足它。
//!
//! **它仍然验不了的那件事**：守卫在**当前这个会话里有没有真的挂上**。上面那条新测试把
//! 「命令在各种 cwd 下找不找得到守卫」从形状层推进到了行为层，但它读的始终是
//! `settings.json` 的**内容** —— 一份内容完美却压根没被 Claude Code 加载的配置，
//! 这里每一条都会绿。所以「它挂上了」**照旧只有开场自检那一条能验**：
//! Read 一下守卫脚本自己，**必须被拦**。
//!
//! ---
//!
//! **守卫拦过什么（八次误拦归类，精简版；全表在台账「十二、Z2 回执」）**。
//! 撞上了先照「标准绕法」换写法，**别碰守卫**：
//!
//! | 撞到的写法 | 判定 | 标准绕法 |
//! |---|---|---|
//! | `AITE_RELOCK=1 …` | 设计如此 | 没有。改 `.claude/**` 只能出补丁脚本给人跑 |
//! | `find … -delete` / `-exec` | 设计如此 | `ls` 列出来 + 点名 `rm -f` |
//! | Read / `cat` 守卫自身或 `settings.json` | 设计如此 | 没有。这条正是开场自检要撞的那一条 |
//! | 命令里出现受保护路径的**字面量**（`git add <那个路径>`） | 固有代价 | `git add -u` |
//! | `cargo fmt` **且命令的词里没有 `--check`**（跟 `--all` 无关，跟它会碰什么也无关） | 固有代价 | 逐个文件 `rustfmt --edition 2024 <file>`（AA3 实测更正，2026-09-13） |
//! | **某一行内 ASCII 引号未闭合**（跟 heredoc 无关；中文引号不触发） | 固有代价 | **别让引号跨行**；正文里有 `it's` 这种撇号时才改用 Write 工具落文件（AA3 实测更正，2026-09-13） |
//! | **命令替换**（`$(…)` / 反引号 / `$((…))`）**且**同条命令里有受保护路径 → 「不透明载荷」 | 固有代价 | 别让两者同时出现；`$VAR` / `${VAR}` 与进程替换 `<(…)` 都不触发（AA3 实测更正，2026-09-13） |
//! | 正文里的 `**`（markdown 加粗）反向匹配到死探针 | 已收窄 | 不再复现：V6 (3a) 把 `aite/contracts/__init__.py` 从 `PROBES` 里删了（`4969a8d`） |
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// PreToolUse hook 的「拦截」退出码。
const BLOCKED: i32 = 2;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn guard_path() -> PathBuf {
    repo_root().join(".claude/hooks/guard_bash.py")
}

fn settings_path() -> PathBuf {
    repo_root().join(".claude/settings.json")
}

/// 把一份 PreToolUse payload 喂给守卫，返回退出码。
///
/// 用 `python3`（不是随便哪个解释器）：`settings.json` 里的 hook 命令就是 `python3`，
/// 本机也**没有** `python` 这个命令，必须验的是真正会执行守卫的那个解释器。
fn run_guard(payload: &Value) -> i32 {
    let mut child = Command::new("python3")
        .arg(guard_path())
        .current_dir(repo_root())
        .env_remove("AITE_RELOCK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起不来 python3 —— 守卫是个 python3 脚本，跑不了它就等于没有守卫");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("往守卫写 payload");
    child
        .wait_with_output()
        .expect("等守卫退出")
        .status
        .code()
        .expect("守卫是被信号杀掉的，没有退出码")
}

/// 和 `run_guard` 一样，只是**在指定 cwd 下**起守卫（路径相对仓库根）。
///
/// 守卫认不认进程 cwd，是 BB4 要量的那一维 —— `run_guard` 把 cwd 钉死在仓库根，
/// 那一维在它下面是看不见的。
fn run_guard_in(cwd_rel: &str, payload: &Value) -> i32 {
    let cwd = repo_root().join(cwd_rel);
    let mut child = Command::new("python3")
        .arg(guard_path())
        .current_dir(&cwd)
        .env_remove("AITE_RELOCK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起不来 python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("往守卫写 payload");
    child
        .wait_with_output()
        .expect("等守卫退出")
        .status
        .code()
        .expect("守卫是被信号杀掉的，没有退出码")
}

fn bash_in(cwd_rel: &str, cmd: &str) -> i32 {
    run_guard_in(
        cwd_rel,
        &json!({"tool_name": "Bash", "tool_input": {"command": cmd}}),
    )
}

fn tool_in(cwd_rel: &str, name: &str, file_path: &str) -> i32 {
    run_guard_in(
        cwd_rel,
        &json!({"tool_name": name, "tool_input": {"file_path": file_path}}),
    )
}

fn bash(cmd: &str) -> i32 {
    run_guard(&json!({"tool_name": "Bash", "tool_input": {"command": cmd}}))
}

fn tool(name: &str, file_path: &str) -> i32 {
    run_guard(&json!({"tool_name": name, "tool_input": {"file_path": file_path}}))
}

/// 前提本身就是一条断言：守卫在、python3 在、而且它认得懂 payload。
///
/// 放在最前面是为了让「环境不对」一眼可辨 —— 不然下面那堆用例会以各种莫名其妙的方式红。
#[test]
fn the_guard_script_is_present_and_python3_can_run_it() {
    assert!(
        guard_path().exists(),
        "守卫脚本不在：{}。它是入库文件（git ls-files .claude/ 有两个），不该缺",
        guard_path().display()
    );
    assert!(settings_path().exists(), "{}", settings_path().display());
    // 一条谁都不碰的命令：守卫要能正常跑完并放行。
    assert_eq!(
        bash("echo hello"),
        0,
        "守卫连 `echo hello` 都跑不通 —— 脚本自己坏了，下面的用例都没意义"
    );
}

// --- 必须拦下的写入姿态（旧 MUST_BLOCK 的 14 条，逐条换到 Rust/proto 面）-------------

#[test]
fn bash_writes_to_the_frozen_surface_are_blocked() {
    // (标签, 命令)。前七条是原始面，后七条是审计实测出来、后来才堵上的绕过。
    let cases: &[(&str, &str)] = &[
        (
            "重定向覆盖契约",
            "echo x > core/crates/contracts/src/lib.rs",
        ),
        (
            "sed -i 改契约",
            r#"sed -i "" s/a/b/ core/crates/contracts/src/lib.rs"#,
        ),
        ("通配符扫到契约", "rm core/crates/contracts/src/*.rs"),
        (
            "命令替换藏路径",
            "cat $(echo proto/aite/v1/events.proto) > /tmp/x",
        ),
        ("变量回填", "F=core/crates/contracts/src/lib.rs; rm $F"),
        (
            "git checkout 覆盖",
            "git checkout HEAD -- proto/aite/v1/events.proto",
        ),
        (
            "heredoc 写契约",
            "cat > core/crates/contracts/src/x.rs <<EOF\nx\nEOF",
        ),
        // APFS 大小写不敏感：`Contracts` 和 `contracts` 在本机是同一个目录
        (
            "大小写不敏感（APFS）",
            "echo x > core/crates/Contracts/src/lib.rs",
        ),
        (
            "xargs 管道",
            "echo core/crates/contracts/src/lib.rs | xargs rm",
        ),
        (
            "key=value 藏路径",
            "dd if=/dev/zero of=proto/aite/v1/events.proto",
        ),
        ("find -delete", r#"find . -name "*.rs" -delete"#),
        // 工具已经不在这个仓库里了，但拦截仍然生效 —— 拆掉它就等于给未来的回归让路
        ("全树重写：ruff format", "ruff format ."),
        ("全树重写：ruff --fix", "ruff check --fix ."),
        // 上面那两条活着的等价物：Rust 侧的全树重写工具
        ("全树重写：cargo fmt", "cargo fmt"),
    ];
    for (label, cmd) in cases {
        assert_eq!(bash(cmd), BLOCKED, "{label} 没被拦：{cmd:?}");
    }
}

/// 重锁与授权这两件事，谁都不许自己给自己开门。
#[test]
fn relock_and_self_authorization_are_blocked() {
    assert_eq!(
        bash("aite contracts lock --write"),
        BLOCKED,
        "重写契约锁必须被拦（旧用例是 `python3 -m aite.contracts.lock --write`）"
    );
    assert_eq!(
        bash("AITE_RELOCK=1 echo hi"),
        BLOCKED,
        "守卫只在 AITE_RELOCK=1 时整体放行 —— 要是能在 Bash 命令里自己赋值，\
         那这个开关等于不存在"
    );
}

// --- 必须拦下的工具调用（旧 MUST_BLOCK_TOOLS 的 5 条 + 两条新增）---------------------

#[test]
fn tool_calls_touching_the_frozen_surface_are_blocked() {
    let cases: &[(&str, &str, &str)] = &[
        ("Write 契约", "Write", "core/crates/contracts/src/lib.rs"),
        ("Write proto", "Write", "proto/aite/v1/events.proto"),
        ("Edit 契约锁", "Edit", ".contracts.lock"),
        ("Write 冻结 spec", "Write", "docs/dev-spec-2026-09-09.md"),
        // 守卫自己与 hook 配置：读和写都不许（`PROT_PATHS` 命中时 readable=False）
        ("Read 守卫自身", "Read", ".claude/hooks/guard_bash.py"),
        ("Read 契约锁", "Read", ".contracts.lock"),
        ("Edit hook 配置", "Edit", ".claude/settings.json"),
    ];
    for (label, name, path) in cases {
        assert_eq!(
            run_guard(&json!({"tool_name": name, "tool_input": {"file_path": path}})),
            BLOCKED,
            "{label} 没被拦：{name}({path})"
        );
    }
}

// --- 必须放行的日常操作（守卫太紧会把所有人卡死，这半边和上半边一样要紧）--------------

#[test]
fn everyday_work_is_not_blocked() {
    let cases: &[(&str, &str)] = &[
        ("lock --check 是每轨的验收项", "aite contracts lock --check"),
        ("find 普通检索", r#"find . -name "*.rs""#),
        (
            "cat 契约（要照着写代码）",
            "cat core/crates/contracts/src/lib.rs",
        ),
        ("写自己轨的文件", "echo x > core/crates/app/src/cli.rs"),
        ("跑测试", "cargo test --workspace"),
        // `cargo fmt` 被拦，`--check` 不是写模式，必须放行 —— 它是纪律 5 的验收项
        ("cargo fmt --check 不是写模式", "cargo fmt --check"),
        // gofmt 只改 *.go，而受保护面里一个 .go 都没有（契约是 Rust/proto）；
        // `edge/go.mod` 真被点名了会被 PROT_PATHS 拦住，所以不用给它补 REWRITERS 条目
        ("gofmt -w 全树", "gofmt -w ."),
    ];
    for (label, cmd) in cases {
        assert_eq!(bash(cmd), 0, "{label} 被误拦：{cmd:?}");
    }
}

/// 执行者必须读得到契约和 spec，否则没法照着写代码。
#[test]
fn reading_contracts_and_the_frozen_spec_is_allowed() {
    for path in [
        "core/crates/contracts/src/lib.rs",
        "proto/aite/v1/events.proto",
        "docs/dev-spec-2026-09-09.md",
    ] {
        assert_eq!(tool("Read", path), 0, "Read {path} 被误拦");
    }
}

/// `Glob(**/*.rs)` 是正常操作，不能因为可能展开到契约就整个拦掉。
#[test]
fn globbing_the_whole_repo_is_allowed() {
    assert_eq!(
        run_guard(&json!({"tool_name": "Glob", "tool_input": {"pattern": "**/*.rs"}})),
        0
    );
}

// --- hook 接线本身（旧的两条，一条都不能少）-----------------------------------------

fn settings() -> Value {
    let text = std::fs::read_to_string(settings_path()).expect("读 .claude/settings.json");
    serde_json::from_str(&text).expect(".claude/settings.json 不是合法 JSON")
}

fn pretooluse_commands() -> Vec<String> {
    settings()["hooks"]["PreToolUse"]
        .as_array()
        .expect("settings.json 里没有 hooks.PreToolUse 数组")
        .iter()
        .flat_map(|entry| entry["hooks"].as_array().cloned().unwrap_or_default())
        .filter(|h| h["type"] == "command")
        .filter_map(|h| h["command"].as_str().map(str::to_string))
        .collect()
}

/// 本机**没有** `python` 命令；hook 写成 `python` 会 command not found，
/// 而 hook 失败是非阻塞放行且不报警 —— 守卫会静默失效，且没有任何人会知道。
///
/// 这条纪律在 Python 树删掉之后，全仓一度没有任何东西钉着。
///
/// 判据写成「含 python3 且不含裸 python」而不是旧那条「第一个词是 python3」：
/// 命令有可能要先解析仓库根（见 (3c)），那样第一个词就不是解释器了，但纪律没变。
#[test]
fn the_hook_command_uses_python3_not_python() {
    let cmds = pretooluse_commands();
    assert!(
        !cmds.is_empty(),
        "settings.json 里没有 PreToolUse 命令 hook"
    );
    for c in &cmds {
        assert!(
            c.contains("guard_bash.py"),
            "PreToolUse hook 没指向守卫：{c:?}"
        );
        assert!(c.contains("python3"), "hook 命令没用 python3：{c:?}");
        let bare_python = c.split_whitespace().any(|t| {
            let t = t.trim_matches('"').trim_matches('\'');
            t == "python" || t.ends_with("/python")
        });
        assert!(
            !bare_python,
            "hook 命令里出现了裸 python（本机没有这个命令，会 command not found → \
             hook 失败 → 非阻塞放行 → 守卫静默失效）：{c:?}"
        );
    }
}

/// 在 `cwd` 下用 `sh -c` **真跑一遍** `settings.json` 里那条 hook 命令，返回退出码。
///
/// 和 `run_guard` 的分工：那个直接 `python3 <守卫脚本>`，验的是**守卫本身**好不好使；
/// 这个跑的是**命令字符串**，验的是「在这个 cwd 下它到底还找不找得到守卫」。
/// PreToolUse 的命令是交给 shell 执行的，所以这里也过一层 shell。
///
/// `CLAUDE_PROJECT_DIR` 按参数给 —— 它模拟的是「claude 进程是在哪个目录起的」，
/// 而那个值在进程启动那一刻就定死了，正是 (3c) 那条病的病根。
fn run_hook_command(cmd: &str, cwd: &Path, project_dir: &Path, payload: &Value) -> i32 {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .env("CLAUDE_PROJECT_DIR", project_dir)
        .env_remove("AITE_RELOCK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起不来 sh");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("往 hook 命令写 payload");
    child
        .wait_with_output()
        .expect("等 hook 命令退出")
        .status
        .code()
        .expect("hook 命令是被信号杀掉的，没有退出码")
}

/// `git rev-parse --show-toplevel` 在 `dir` 下的结果；不在任何仓库里就是 `None`。
fn git_toplevel(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// hook 命令找不到守卫时**必须还能被人救回来**，不许把会话变成砖头。
///
/// 上面那条 `the_hook_command_uses_python3_not_python` 只看命令的**形状**。形状对、
/// 而在某个 cwd 下它把**一切**都拦掉 —— 这种病它一个字也看不见。2026-09-13 真踩了：
/// 会话 cwd 停在 `~/.claude/projects/…/memory`（不在任何 git 仓库里），
/// `$(git rev-parse --show-toplevel)` 失败 → 命令展开成 `python3 "/.claude/hooks/guard_bash.py"`
/// → 文件不存在 → 退出码 2 → `Bash` / `Read` / `Write` 全部同一条错。
/// 而**会话 cwd 只能靠 Bash 的 `cd` 改，Bash 已经被拦** —— 只能由人重启。
///
/// 三种 cwd 一起钉，因为它们各自锁着一种退化：
///
/// * **仓库外** —— 上面那条病史本身；
/// * **另一个 git 仓库里** —— `git` 会**成功**并返回那边的根，所以「`git` 失败才回退」
///   这种写法（`d=$(…) || d="$CLAUDE_PROJECT_DIR"`）在这里仍然变砖。判据得是
///   「找到的那个根里有没有守卫」，不是「git 成功没有」；
/// * **子目录里起的会话**（`CLAUDE_PROJECT_DIR` 指着子目录）—— 这是 V6 (3c) 治的那条
///   原病（2026-09-12），锁着「别退回纯 `$CLAUDE_PROJECT_DIR`」。
///
/// **两个 payload 都要断，缺一条这测试就没意义**：只断「该拦的拦住了」是**恒真断言** ——
/// 一条把所有东西都拦掉的坏命令照样满足它。要分辨「守卫在工作」和「守卫在乱拦」，
/// 只有「该放行的真放行了」这一条能做到。
///
/// **它在 `review/z2-guard-patch.py` 跑之前是红的，跑之后必须绿。** 交付时红是对的，
/// 不是失败：这条测试读的是 `settings.json` 的真实内容，而那个文件在守卫的保护面里
/// （`readable=False`），只有人能改。红在第二个 payload（该放行的被拦了）。
#[test]
fn the_hook_command_recovers_outside_the_repo_instead_of_bricking_the_session() {
    let root = repo_root().canonicalize().expect("仓库根算不出来");
    let outside = tempfile::tempdir().expect("建不了临时目录");
    let other = tempfile::tempdir().expect("建不了临时目录");

    // 前提要量，不能假设：`TMPDIR` 通常不在任何 git 仓库里，但「通常」不是判据。
    assert!(
        git_toplevel(outside.path()).is_none(),
        "临时目录 {} 落在一个 git 仓库里（{:?}）—— 这条用例要的是「仓库外」那种 cwd。\
         换个 TMPDIR 再跑，别把它当回归失败",
        outside.path().display(),
        git_toplevel(outside.path())
    );
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(other.path())
            .status()
            .expect("起不来 git")
            .success(),
        "git init 没成 —— 这条用例要一个「有 git 仓库但没有守卫」的 cwd"
    );
    let other_root = git_toplevel(other.path()).expect("git init 完了却不是仓库");
    assert!(
        !Path::new(&other_root)
            .join(".claude/hooks/guard_bash.py")
            .exists(),
        "临时仓库 {other_root} 里居然有守卫 —— 这条用例要的是「那边没有守卫」"
    );

    // 子目录：模拟「会话在 core/crates/app 里起」，CLAUDE_PROJECT_DIR 因此指着子目录。
    let subdir = root.join("core/crates/app");
    assert!(subdir.is_dir(), "{} 不在", subdir.display());

    let scenarios: &[(&str, &Path, &Path)] = &[
        (
            "仓库外（2026-09-13 锁死会话的那种 cwd）",
            outside.path(),
            &root,
        ),
        (
            "另一个 git 仓库里（git 成功了，但那边没有守卫）",
            other.path(),
            &root,
        ),
        (
            "子目录里起的会话（CLAUDE_PROJECT_DIR 指着子目录，V6 (3c) 的原病）",
            &subdir,
            &subdir,
        ),
    ];

    let cmds = pretooluse_commands();
    assert!(
        !cmds.is_empty(),
        "settings.json 里没有 PreToolUse 命令 hook"
    );

    // 写冻结面：无论 cwd 在哪都得拦住。**这一条单独看是恒真的**（坏命令也「通过」）。
    let must_block = json!({
        "tool_name": "Write",
        "tool_input": {"file_path": "proto/aite/v1/events.proto"},
    });
    // 谁都不碰的一条命令：无论 cwd 在哪都得放行。**承重墙在这儿。**
    let must_pass = json!({"tool_name": "Bash", "tool_input": {"command": "echo hello"}});

    for cmd in &cmds {
        for (label, cwd, project_dir) in scenarios {
            assert_eq!(
                run_hook_command(cmd, cwd, project_dir, &must_block),
                BLOCKED,
                "【{label}】写冻结面没被拦 —— 守卫在这种 cwd 下压根没跑起来，\
                 而 hook 失败是非阻塞放行且不报警：{cmd:?}"
            );
            assert_eq!(
                run_hook_command(cmd, cwd, project_dir, &must_pass),
                0,
                "【{label}】`echo hello` 被拦了 —— 这条命令在这种 cwd 下找不到守卫，\
                 于是把**一切**都拦掉。而会话 cwd 只能靠 Bash 的 `cd` 改，Bash 也拦着，\
                 会话从内部出不来、只能由人重启。命令要留一条回退路径：{cmd:?}"
            );
        }
    }
}

/// Claude Code 只对 `Edit(path)` / `Read(path)` 做文件权限匹配。
///
/// `Write(path)` / `NotebookEdit(path)` 会被接受但**从不查询**，且启动时刷警告 ——
/// 也就是说写成那样的 deny 规则是死的。一条 `Edit()` 规则就覆盖所有编辑类工具。
#[test]
fn deny_rules_only_use_the_edit_and_read_prefixes() {
    let deny = settings()["permissions"]["deny"]
        .as_array()
        .expect("settings.json 里没有 permissions.deny 数组")
        .clone();
    for rule in &deny {
        let rule = rule.as_str().expect("deny 规则要是字符串");
        assert!(
            rule.starts_with("Edit(") || rule.starts_with("Read("),
            "死规则（不参与文件权限匹配）：{rule}"
        );
    }
}

// =====================================================================================
// AA3（2026-09-13）：把误拦归类表从**归纳**推到**实测**。
//
// 上面模块头那张表原本是从会话经历里归出来的，一行都没有黑盒验过。AA3 拿 `run_guard()`
// 喂了 183 条 payload 把每一格的**触发条件**量到精确，下面八条各钉一格。
//
// 量出来和那张表不一致的三格（引号解析 / 不透明载荷 / `cargo fmt`）**已经在模块头就地
// 改正**（BB4，2026-09-15 —— 不再留两段并存，读的人只会看到实测那一版），判据分别由
// `a_quote_must_close_on_its_own_line`、
// `command_substitution_is_what_the_opaque_payload_verdict_means`、
// `cargo_fmt_is_judged_by_the_check_flag_alone` 钉着。详见台账「十六、AA3 回执」。
//
// **这八条只断退出码，不断错误消息的措辞。** 措辞改了不是行为变了；而标签
// （`读取位置` / `写入/执行位置` / `不透明载荷` / `解析失败` / `解释器内联代码` /
// `授权变量赋值`）作为判据证据写在各条的注释里 —— 那是黑盒能拿到的全部信息。
// =====================================================================================

/// 守卫把命令**按行**切开逐行解析，所以引号必须在**它自己那一行**里闭合。
///
/// 这一条治的是历史上最常撞的那一格（Y1 1 次、Y2 1 次、Z1 1 次，本轨开工十分钟内又撞
/// 一次）。老归因写的是「heredoc 正文里有配不平的引号 / 中文引号」，**两半都不准**：
///
/// * **跟 heredoc 无关**。`echo '\nhi\n'` 一个 heredoc 都没有，整条看引号还是**配平的**
///   （两个 `'`），照样被判 `<命令无法解析: No closing quotation>（解析失败）` ——
///   因为第一行 `echo '` 里那个单引号没在本行闭合。heredoc 只是最容易写出跨行引号的场合。
/// * **中文引号不触发**。`“”`、`‘’`、`「」` 三种全部放行：解析器只认 ASCII 的 `'` 和 `"`。
///   历史上那几次归到中文引号头上的，正文里多半还有个英文撇号（`it's` 那种）。
///
/// **所以标准绕法要改口径**：不是「别用 heredoc」，是「别让引号跨行」。
/// 正文里有 `it's` 这种撇号时才需要换 Write 工具落文件。
#[test]
fn a_quote_must_close_on_its_own_line() {
    // 该拦的：某一行里的 ASCII 引号没在本行闭合
    let blocked: &[(&str, &str)] = &[
        ("单行内不配对的单引号", "echo it's fine"),
        ("单行内不配对的双引号", r#"echo "abc"#),
        (
            "引号跨行 —— 整条看是配平的，逐行看第一行就炸了",
            "echo '\nhi\n'",
        ),
        (
            "python3 -c 的双引号跨行（本轨开工十分钟内自撞的那一条）",
            "python3 -c \"\nprint('hi')\n\"",
        ),
        (
            "heredoc 正文里有英文撇号",
            "cat > /tmp/x <<'EOF'\nit's fine\nEOF",
        ),
    ];
    for (label, cmd) in blocked {
        assert_eq!(bash(cmd), BLOCKED, "{label} 没被拦：{cmd:?}");
    }

    // 该放的：引号各自在本行闭合，或者压根不是 ASCII 引号
    let allowed: &[(&str, &str)] = &[
        ("同一条压成单行就好了", "python3 -c \"print('hi')\""),
        ("多行，但每行的引号各自闭合", "echo \"a\"\necho \"b\""),
        ("中文弯引号", "echo 他说“你好”"),
        ("中文单弯引号", "echo 他说‘你好’"),
        ("中文直角引号", "echo 他说「你好」"),
        (
            "heredoc 正文里的中文弯引号",
            "cat > /tmp/x <<'EOF'\n他说“你好”\nEOF",
        ),
        // 反引号不参与这条判据（它归命令替换那条管）
        ("不配对的反引号", "echo `date"),
    ];
    for (label, cmd) in allowed {
        assert_eq!(bash(cmd), 0, "{label} 被误拦：{cmd:?}");
    }
}

/// 「不透明载荷」= **命令替换**里出现受保护路径，不是「正文太长」、也不是「判不出读/写」。
///
/// 两版旧归因都证伪了：
///
/// * **Y2 的「正文太长」**：8KB 的纯填充命令放行；8KB 填充**加上**一条契约路径也放行
///   （`echo` 是读取位置）。长度一个字都不参与。
/// * **Z2 的「受保护路径出现在判不出读/写的位置」**：那种情况实测拿到的标签是
///   **`写入/执行位置`**（`foobarbaz <契约>`、`touch <契约>`、heredoc 正文里的路径……
///   全是它）。`不透明载荷` 这个标签只有命令替换能触发。
///
/// 判据窄得很干净：`$(…)`、反引号、`$((…))` 三种展开 **+** 同一条命令里有受保护路径。
/// `$VAR` / `${VAR}` 这种纯变量展开**不触发**，进程替换 `<(…)` 也**不触发**。
/// 外层命令在不在 `READ_SAFE` 里也不管用 —— `echo $(echo <契约>)` 照样拦。
///
/// 本轨自撞那一条（一份带 `$(echo …)` 的 heredoc）逐字喂回去复现了这个标签；
/// 把那两行 `$(echo …)` 删掉，同一条命令**放行**。判据就落在这儿。
#[test]
fn command_substitution_is_what_the_opaque_payload_verdict_means() {
    const CONTRACT: &str = "core/crates/contracts/src/lib.rs";

    let blocked: &[(&str, String)] = &[
        (
            "$(…) 里藏契约路径",
            format!("cat $(echo {CONTRACT}) > /tmp/x"),
        ),
        ("反引号里藏契约路径", format!("cat `echo {CONTRACT}`")),
        (
            "外层是 echo（读取位置）也没用 —— 命令替换的判定在前",
            format!("echo $(echo {CONTRACT})"),
        ),
        ("算术展开也算命令替换", format!("echo $((1+1)) {CONTRACT}")),
        (
            "命令替换在赋值右边",
            format!("X=$(echo {CONTRACT}); echo done"),
        ),
    ];
    for (label, cmd) in blocked {
        assert_eq!(bash(cmd), BLOCKED, "{label} 没被拦：{cmd:?}");
    }

    let allowed: &[(&str, String)] = &[
        (
            "命令替换但不碰受保护面",
            "cat $(echo /tmp/x) > /tmp/y".into(),
        ),
        // 长度不是判据 —— Y2 的归因在这两条上证伪
        ("8KB 纯填充", format!("echo {}", "x".repeat(8000))),
        (
            "8KB 填充 + 一条契约路径（echo 是读取位置，PREFIX 族放行）",
            format!("echo {} {CONTRACT}", "x".repeat(8000)),
        ),
        // 纯变量展开不是命令替换
        ("$VAR 展开", format!("cat $F/{CONTRACT}")),
        ("${{VAR}} 展开", format!("cat ${{F}}/{CONTRACT}")),
    ];
    for (label, cmd) in allowed {
        assert_eq!(bash(cmd), 0, "{label} 被误拦：{cmd:?}");
    }
}

/// `cargo fmt` 只看命令里有没有 `--check`，**不看 `--all`、也不看它到底会碰什么**。
///
/// 老表写的是「`cargo fmt --all`（写模式，碰冻结面）」，两个修饰语都不是判据：
///
/// * `--all` 不是判据：`cargo fmt`（不带 `--all`）照样拦，`cargo fmt --check --all` 照样放行；
/// * 「碰冻结面」也不是判据：`cargo fmt -p aite` 碰不到 `core/crates/contracts/**`，
///   一样被拦（守卫算不出 `-p` 的覆盖面，宁可错杀 —— 这条是**固有代价**，别改）。
///
/// **`--check` 在哪个位置都认**：`--check --all`、`--all --check`、`--all -- --check`
/// 三种写法全放行。派单里「只读模式还拦的话是一条可以收窄的真误拦」那个猜想，**证伪**。
///
/// **但这里有一条真误拦**：`cargo fmt --version` / `--help` 一个字节都不写，照样被判
/// 「写模式」。下面钉的是**现状**（characterization），不是我们想要的行为 ——
/// 守卫哪天收窄了这一格，这两行会红，那时来改它、别当回归失败。
/// 收窄提案与它的 fail-closed 复核见台账「十六、AA3 回执」③④。
#[test]
fn cargo_fmt_is_judged_by_the_check_flag_alone() {
    let blocked: &[(&str, &str)] = &[
        ("光杆 cargo fmt", "cargo fmt"),
        ("--all 写模式", "cargo fmt --all"),
        (
            "-p 限定单个 package 也拦（覆盖面算不出来，固有代价）",
            "cargo fmt -p aite",
        ),
        // ↓ 真误拦，钉的是现状
        ("--version 纯查询，一个字节不写", "cargo fmt --version"),
        ("--help 纯查询，一个字节不写", "cargo fmt --help"),
        // 短选项同病（BB4 补量 —— AA3 只量了长选项那两个）
        ("-V 纯查询", "cargo fmt -V"),
        ("-h 纯查询", "cargo fmt -h"),
        ("--all 配 --version 也拦", "cargo fmt --all --version"),
        // ↓ **判据本来就是按词的**，不是按子串：`--checkfoo` 里含 `--check` 这个子串，
        //   照样被拦。AA3 ③.1 担心的「子串匹配会让 --help-xyz 绕过去」那个风险
        //   在守卫这一侧并不存在（BB4 实测）；那句话的真正用处是约束**收窄补丁**本身。
        (
            "--checkfoo 不是 --check（按词，不是子串）",
            "cargo fmt --checkfoo",
        ),
        ("--help-xyz 不是 --check", "cargo fmt --all -- --help-xyz"),
    ];
    for (label, cmd) in blocked {
        assert_eq!(bash(cmd), BLOCKED, "{label} 没被拦：{cmd:?}");
    }

    let allowed: &[(&str, &str)] = &[
        ("--check 在前", "cargo fmt --check --all"),
        ("--check 在后", "cargo fmt --all --check"),
        ("--check 经 rustfmt 直传", "cargo fmt --all -- --check"),
        ("--check 配 -p", "cargo fmt -p aite --check"),
        // 纪律 5 的标准绕法：绕开 cargo fmt，直接喊 rustfmt
        (
            "rustfmt 单文件",
            "rustfmt --edition 2024 crates/app/tests/guard.rs",
        ),
        // 同族的别的重写工具，判据各不相同
        ("ruff check 不带 --fix", "ruff check ."),
        ("gofmt -w（受保护面里一个 .go 都没有）", "gofmt -w ."),
        (
            "clippy --fix 不在 REWRITERS 里",
            "cargo clippy --fix --workspace",
        ),
    ];
    for (label, cmd) in allowed {
        assert_eq!(bash(cmd), 0, "{label} 被误拦：{cmd:?}");
    }
}

/// **冻结面是按命令文本里的路径字面量匹配的，所以先 `cd` 进去就绕过了。**
///
/// 这一条钉的是**漏拦**，不是误拦 —— 一个拦过十次不该拦的东西，边界的**另**半边同样没人量过。
///
/// `PROT_PREFIXES`（`core/crates/contracts/**`、`proto/**`）匹配的是命令文本里写出来的
/// 那一串字符。写法变形基本都堵住了（绝对路径、`./` 前缀、双斜杠、`..` 回绕、`$HOME`
/// 展开、大小写变体 —— 本轨逐条量过，全部拦住）。**唯独少了 cwd 这一维**：
///
/// ```text
/// cd core && echo x > crates/contracts/src/lib.rs      → 放行
/// Write(crates/contracts/src/lib.rs)                   → 放行
/// ```
///
/// 两条都真能改到契约文件，而 `cd core` 就写在命令里、不需要任何前置状态。
/// `Write` 那条更不需要命令 —— 会话在 `core/` 下起，`file_path` 自然就是这个形状。
///
/// **没在本轨修**：修它要动 `guard_bash.py`（把路径按 cwd 归一化再匹配），而本轨拿不到
/// 那个文件的内容，锚点定位不到就不许写补丁（派单 ③）。**先钉成已知行为**，
/// 下一个人一眼看得到这个洞在哪、有多大。守卫收窄之后这两行会红 —— 那时来改它。
///
/// 这条**不影响 fail-closed**：它是「本该拦的没拦」，不是「守卫失效了却放行」。
/// 守卫仍然在跑、仍然会对它认得出的写法退 2。
#[test]
fn protected_prefixes_match_the_literal_path_so_a_cd_first_slips_through() {
    // 绝对路径那两条按**当前仓库根**拼 —— 写死 `/Users/…/task-aa3/…` 的话，
    // 换个 worktree 跑就在验一条不存在的路径，而守卫是按文本匹配的、照样会绿：
    // 那就成了恒真断言。
    let root = repo_root()
        .canonicalize()
        .expect("仓库根算不出来")
        .display()
        .to_string();
    let abs = format!("{root}/core/crates/contracts/src/lib.rs");

    // 认得出的那些写法：全拦
    let blocked: &[(&str, String)] = &[
        ("绝对路径", format!("echo x > {abs}")),
        (
            "./ 前缀",
            "echo x > ./core/crates/contracts/src/lib.rs".into(),
        ),
        (
            "双斜杠",
            "echo x > core//crates//contracts//src//lib.rs".into(),
        ),
        (
            ".. 回绕",
            "echo x > core/crates/contracts/src/../src/lib.rs".into(),
        ),
        (
            "$HOME 展开后才是绝对路径",
            format!(
                "echo x > $HOME{}",
                abs.trim_start_matches(&std::env::var("HOME").expect("HOME 没设"))
            ),
        ),
        (
            "大小写变体（APFS 不敏感）",
            "echo x > core/crates/CONTRACTS/src/lib.rs".into(),
        ),
        (
            "tee 写",
            "echo x | tee core/crates/contracts/src/lib.rs".into(),
        ),
    ];
    for (label, cmd) in blocked {
        assert_eq!(bash(cmd), BLOCKED, "{label} 没被拦：{cmd:?}");
    }

    // ↓ 下面断的都是「**没**拦住」。钉的是现状，不是我们想要的行为。
    //
    // **BB4 补量**：AA3 只试了 `cd core &&` 一种写法。本轨把 `cd` 的形状矩阵铺开，
    // **十二种全部放行** —— 也就是说这不是某一种写法的疏漏，是整条 cwd 维度不存在。
    let cd_slips: &[(&str, &str)] = &[
        ("&& 串联", "cd core && echo x > crates/contracts/src/lib.rs"),
        ("; 串联", "cd core; echo x > crates/contracts/src/lib.rs"),
        (
            "./ 前缀",
            "cd ./core && echo x > crates/contracts/src/lib.rs",
        ),
        ("两级 cd", "cd core/crates && echo x > contracts/src/lib.rs"),
        (
            "直接 cd 进 src",
            "cd core/crates/contracts/src && echo x > lib.rs",
        ),
        (
            "子 shell",
            "(cd core && echo x > crates/contracts/src/lib.rs)",
        ),
        ("换行分隔", "cd core\necho x > crates/contracts/src/lib.rs"),
        (
            "cd 目标带引号",
            "cd \"core\" && echo x > crates/contracts/src/lib.rs",
        ),
        (
            "cd 路径里带 .. 回绕",
            "cd core/crates/contracts/../contracts/src && echo x > lib.rs",
        ),
        (
            "tee 写",
            "cd core && echo x | tee crates/contracts/src/lib.rs",
        ),
        (
            "sed -i 改",
            "cd core && sed -i \"\" s/a/b/ crates/contracts/src/lib.rs",
        ),
        // proto/** 这半边一样漏
        ("proto 面同病", "cd proto && echo x > aite/v1/events.proto"),
    ];
    for (label, cmd) in cd_slips {
        assert_eq!(
            bash(cmd),
            0,
            "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改，别当回归失败：{label}"
        );
    }

    // **`cd` 的等价物 —— 单独钉着，别跟上面那批一起挪。**
    //
    // `review/bb4-guard-patch.py` 认的是命令文本里的 `cd <目录>`；下面这四种改的是
    // 同一件事（工作目录），但要认出它们，守卫得逐个去理解各自的参数语义
    // （`-C` 之于 git/make、`pushd` 的目录栈、`$PWD` 要到运行时才有值）——
    // 那是另一条曲线，收益远小于把 `cd` 解析做歪的风险。**补丁跑完这四条仍然放行**，
    // 记账转出去了（台账「二十一、BB4 回执」的「记账转出去的」）。
    for (label, cmd) in [
        (
            "pushd 不是 cd",
            "pushd core && echo x > crates/contracts/src/lib.rs",
        ),
        (
            "git -C 等价于 cd",
            "git -C core checkout HEAD -- crates/contracts/src/lib.rs",
        ),
        ("make -C 等价于 cd", "make -C core fmt"),
        (
            "cd 目标要到运行时才知道（$PWD / $(…)）",
            "cd \"$PWD/core\" && echo x > crates/contracts/src/lib.rs",
        ),
    ] {
        assert_eq!(
            bash(cmd),
            0,
            "{label} 开始被拦了（好事）—— 这条该跟着改：{cmd:?}"
        );
    }

    // **漏拦的机制就在这儿**（BB4 实测）：`cd` 在 `READ_SAFE` 里 ——
    // `cd core/crates/contracts/src` 这一段本身被判「读取位置」，前缀族于是放行；
    // 而后一段 `echo x > lib.rs` 里那个相对路径压根不带前缀，两头都落空。
    // 对照：`pushd` **不在** `READ_SAFE` 里，同一个目标当场被判写入/执行位置。
    assert_eq!(
        bash("cd core/crates/contracts/src"),
        0,
        "cd 到保护面被误拦了 —— 那 cd 就不在 READ_SAFE 里了，上面那批的机制要重新量"
    );
    assert_eq!(
        bash("pushd core/crates/contracts/src"),
        BLOCKED,
        "pushd 到保护面没被拦"
    );

    // **对照：不是所有保护面都漏。** 点名族（`edge/go.mod` / 守卫自身）在 `cd` 之后
    // 照样拦得住 —— 它们的匹配认得出「尾段」，前缀族只认从命令文本开头对齐的那一串。
    // 这一条同时保证上面那堆 `assert_eq!(…, 0, …)` 不是恒真：守卫确实在跑、确实在判事。
    for (label, cmd) in [
        ("cd 进 edge 再读 go.mod", "cd edge && cat go.mod"),
        (
            "cd 进 hooks 再读守卫",
            "cd .claude/hooks && cat guard_bash.py",
        ),
    ] {
        assert_eq!(
            bash(cmd),
            BLOCKED,
            "{label} 没被拦 —— 点名族这半边也塌了：{cmd:?}"
        );
    }

    // **`cd` 到仓库外不是洞，别顺手「治」它**：写的是 `/tmp/crates/…`，
    // 跟冻结面没关系。守卫哪天认 cwd 了，这一条仍然必须放行。
    assert_eq!(
        bash("cd /tmp && echo x > crates/contracts/src/lib.rs"),
        0,
        "cd 到仓库外之后的同名相对路径被误拦了 —— 归一化做歪了才会这样"
    );

    // `Write(crates/contracts/src/lib.rs)` 在 **cwd = 仓库根**下放行是**对的**
    // （它写的是 `<root>/crates/contracts/…`，那儿没有冻结面）。真正的洞在
    // cwd 落进 `core/` 之后 —— 那一格归 `the_guard_never_looks_at_the_process_cwd`。
    assert_eq!(
        tool("Write", "crates/contracts/src/lib.rs"),
        0,
        "这条在 cwd=仓库根下本来就该放行，别把它当洞"
    );
}

/// **守卫一个字都不看进程 cwd** —— 这是上一条那个洞的另一半，也是更难堵的那一半。
///
/// 上一条的 `cd` 至少还写在命令文本里，守卫**看得见**。这一条连文本都没有：
/// 会话在哪个目录起，`file_path` 和命令里的相对路径就是相对那儿的，而守卫始终
/// 拿它去跟「相对仓库根」的保护面比。
///
/// ```text
/// cwd = core/                      echo x > crates/contracts/src/lib.rs   → 放行
/// cwd = core/crates/contracts/src/ echo x > lib.rs                        → 放行
/// ```
///
/// **第二条是这个洞最尖锐的形状**：会话 cwd 就落在冻结面**内部**时，写它只需要一个
/// 裸文件名，命令文本里连一个受保护路径的字符都不出现。本轨这条会话真漂到过那儿
/// （`cd core && cargo run …` 之后 cwd 就停在 `core/`），会话层复验过一条无副作用的
/// 等价物：`cd crates/contracts/src && test -w lib.rs` 在 cwd=`core/` 下**放行**，
/// 而 `test -w` 返回真 —— 契约文件在那条路径上确实是可写的。
///
/// **钉成现状，没在本轨修**（本轨零产品代码改动）。补丁在 `review/bb4-guard-patch.py`，
/// 由人跑；跑完这几条会红，**那时把它们从 `0` 改成 `BLOCKED`**，别当回归失败。
#[test]
fn the_guard_never_looks_at_the_process_cwd() {
    const CONTRACT: &str = "core/crates/contracts/src/lib.rs";

    // ↓ 断的是「没拦住」。钉现状。
    assert_eq!(
        bash_in("core", "echo x > crates/contracts/src/lib.rs"),
        0,
        "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改"
    );
    assert_eq!(
        bash_in("core/crates/contracts/src", "echo x > lib.rs"),
        0,
        "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改"
    );
    assert_eq!(
        tool_in("core", "Write", "crates/contracts/src/lib.rs"),
        0,
        "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改"
    );
    assert_eq!(
        tool_in("core/crates/contracts/src", "Write", "lib.rs"),
        0,
        "守卫开始认 cwd 了（好事）—— 这条 characterization 该跟着改"
    );

    // **承重墙**：换 cwd 不许把守卫变傻。写全名在任何 cwd 下都得拦住 ——
    // 少了这一条，上面四条就是恒真断言（一个压根没跑起来的守卫也满足它们）。
    for cwd in ["core", "core/crates/contracts/src", "edge", "."] {
        assert_eq!(
            bash_in(cwd, &format!("echo x > {CONTRACT}")),
            BLOCKED,
            "cwd={cwd} 下写全名没被拦 —— 守卫在这种 cwd 下根本没跑起来"
        );
    }

    // 读取位置在任何 cwd 下都放行（前缀族可读）—— 另一侧的承重墙。
    for cwd in ["core", "edge", "."] {
        assert_eq!(
            bash_in(cwd, &format!("cat {CONTRACT}")),
            0,
            "cwd={cwd} 下读契约被误拦"
        );
    }

    // **补丁之后必须仍然放行的对照组**（cwd 落在 `core/` 的日常操作）。
    // 归一化一旦做歪，成片误拦就从这里开始 —— 它们现在全绿，改完也必须全绿。
    for (label, cmd) in [
        ("跑测试", "cargo test --workspace"),
        ("读自己轨的文件", "cat crates/app/src/cli.rs"),
        ("写自己轨的文件", "echo x > crates/app/src/cli.rs"),
        ("列目录", "ls crates/"),
        (
            "rustfmt 单文件（纪律 5 的绕法）",
            "rustfmt --edition 2024 crates/app/tests/guard.rs",
        ),
        ("grep 全树", "grep -rn fn crates/app/src/"),
        ("cd 回上级再读", "cd .. && cat README.md"),
        ("裸词不是路径", "echo hello"),
    ] {
        assert_eq!(
            bash_in("core", cmd),
            0,
            "cwd=core 下 {label} 被误拦：{cmd:?}"
        );
    }
}

/// **受保护面的成员表** —— AA3「没做的」第 4 条说「量不出来，只能一个个猜着试」，
/// 本轨按「依赖表 / 锁文件 / 配置」三类系统地试了一遍，这里钉的是试出来的结果。
///
/// 三档（每档的读写行为差一整档，别混着记）：
///
/// | 档 | 成员 | `cat` | 写 |
/// |---|---|---|---|
/// | 点名族 | `.claude/hooks/guard_bash.py`、`.claude/settings.json`、`.contracts.lock`、**`edge/go.mod`**、**`edge/go.sum`** | **拦** | 拦 |
/// | 前缀族 | `core/crates/contracts/`、`proto/` | 放行 | 拦 |
/// | 冻结 spec | `docs/dev-spec-*.md` | 放行 | 拦 |
///
/// **`edge/go.mod` 与 `edge/go.sum` 台账里一个字都没记过**（2026-09-15 靠
/// `grep -n lark edge/go.mod` 被拦才撞出来）。Go 侧的依赖表是双重冻结面的一部分，
/// 而这件事此前只存在于守卫源码里。
///
/// **下半截同样要紧：试过但没命中的。** 那是下一个人不用再试的部分 —— 派单点名的
/// 「依赖表 / 锁文件 / 配置」三类里，Rust 侧和容器侧**一个都不在**保护面里。
#[test]
fn the_protected_surface_membership_measured_not_guessed() {
    // 点名族：读写都拦
    for p in [
        ".claude/hooks/guard_bash.py",
        ".claude/settings.json",
        ".contracts.lock",
        "edge/go.mod",
        "edge/go.sum",
    ] {
        assert_eq!(bash(&format!("cat {p}")), BLOCKED, "点名族读没拦：{p}");
        assert_eq!(bash(&format!("touch {p}")), BLOCKED, "点名族写没拦：{p}");
    }

    // 前缀族 / spec 族：可读不可写
    for p in [
        "core/crates/contracts/src/lib.rs",
        "core/crates/contracts/Cargo.toml",
        "proto/aite/v1/events.proto",
        "docs/dev-spec-2026-09-09.md",
        "docs/dev-spec-2026-09-11-rustgo.md",
    ] {
        assert_eq!(bash(&format!("cat {p}")), 0, "可读档读不了：{p}");
        assert_eq!(bash(&format!("touch {p}")), BLOCKED, "可读档写进去了：{p}");
    }

    // **试过、没命中** —— 依赖表 / 锁文件 / 配置这三类里，除了 Go 那两个，一个都不在。
    // 列在这儿是为了让下一个人不用重试；哪天谁把它们加进保护面，这里会红。
    for p in [
        "core/Cargo.toml",
        "core/Cargo.lock",
        "core/rust-toolchain.toml",
        "core/crates/app/Cargo.toml",
        "config/aite.example.yaml",
        "docker-compose.yml",
        ".dockerignore",
        "docker/core/Dockerfile",
        "docker/edge/Dockerfile",
        "docker/sandbox/Dockerfile",
        "Makefile",
        "scripts/check.sh",
        ".github/workflows/ci.yml",
        ".gitignore",
        "README.md",
        // `.claude/**` 不是整个目录受保护 —— 只有点名的那两个文件
        ".claude/agents/foo.md",
        "edge/cmd/aite-edge/main.go",
        "core/crates/models/src/lib.rs",
    ] {
        assert_eq!(
            bash(&format!("touch {p}")),
            0,
            "它进保护面了（好事？）：{p}"
        );
    }

    // **两条边界，各是一个转出去的账**（都不在本轨的可写面里，只钉住）：
    //
    // 1. `core/crates/proto/**` 被 `proto/` 这个前缀**子串**命中 —— 那是 protobuf
    //    生成的 Rust 绑定，不是 `proto/` 那个冻结目录。报的面写的也是 `proto/**`。
    assert_eq!(
        bash("touch core/crates/proto/src/lib.rs"),
        BLOCKED,
        "core/crates/proto 不再被 proto/ 误命中了 —— 这条 characterization 该跟着改"
    );
    // 2. 顶层还有一份 `dev-spec-2026-09-09.md`（与 `docs/` 那份同尺寸、不同 inode，
    //    两份都在 git 里），而保护面只写了 `docs/dev-spec-*.md` —— 顶层那份**不设防**。
    assert_eq!(
        bash("touch dev-spec-2026-09-09.md"),
        0,
        "顶层那份 dev-spec 进保护面了（好事）—— 这条 characterization 该跟着改"
    );
}

/// 受保护面是按**路径段边界**匹配的，不是裸子串，也不是从头对齐。
///
/// 这一条把「守卫到底拿什么跟什么比」量清楚，因为 `cd` 归一化那条补丁要依赖它 ——
/// 归一化产出的必须是**守卫认得出的那种形状**，否则补了也白补。
///
/// * 段边界：`a/proto/b.rs` 拦、`myproto/x.rs` 放行、`protos/x.rs` 放行；
/// * 任意位置：`a/b/core/crates/contracts/c.rs` 拦（不必从命令文本开头对齐）；
/// * 大小写不敏感（APFS）：`PROTO/foo.rs`、`CORE/CRATES/CONTRACTS/x.rs` 都拦。
#[test]
fn protected_prefixes_match_on_path_segment_boundaries() {
    for (label, p) in [
        ("前面是 /", "a/proto/b.rs"),
        ("嵌在深处", "a/b/core/crates/contracts/c.rs"),
        ("大小写变体", "PROTO/foo.rs"),
        ("大小写变体 2", "CORE/CRATES/CONTRACTS/x.rs"),
        ("前缀族自己", "core/crates/contracts/src/x.rs"),
    ] {
        assert_eq!(bash(&format!("touch {p}")), BLOCKED, "{label} 没拦：{p}");
    }

    for (label, p) in [
        ("前面粘着字母", "myproto/x.rs"),
        ("前面粘着字母 2", "xproto/foo.rs"),
        ("后面粘着字母", "protos/x.rs"),
        ("只是名字像", "protobuf/x.rs"),
        ("core 前面粘着字母", "mycore/crates/contracts/z.rs"),
        ("contracts 后面粘着字母", "core/crates/contractsX/y.rs"),
    ] {
        assert_eq!(bash(&format!("touch {p}")), 0, "{label} 被误拦：{p}");
    }

    // `docs/dev-spec-*.md` 的 `*` **不跨 `/`**，而且那个 `-` 是判据的一部分
    for (label, p, want) in [
        ("空名字也算", "docs/dev-spec-.md", BLOCKED),
        ("嵌在深处", "x/docs/dev-spec-a.md", BLOCKED),
        ("没有那个横杠", "docs/dev-spec.md", 0),
        ("* 不跨斜杠", "docs/sub/dev-spec-a.md", 0),
        ("扩展名要对", "docs/dev-spec-a.markdown", 0),
    ] {
        assert_eq!(bash(&format!("touch {p}")), want, "{label}：{p}");
    }
}

/// `READ_SAFE` 只救得了 `PROT_PREFIXES` 那一族，救不了被点名的那几个文件。
///
/// 两族保护面的行为差一整档，而老表把它们混在一起写了：
///
/// | | `cat <它>` | `echo <它>` |
/// |---|---|---|
/// | `core/crates/contracts/**`、`proto/**`（前缀族） | 放行 | 放行 |
/// | `.claude/hooks/guard_bash.py`、`.claude/settings.json`、`.contracts.lock`（点名族） | **拦** | **拦** |
///
/// 点名族命中时 `readable=False`，**连读取位置一起拦** —— 所以 `cat` / `ls` / `echo`
/// 在它们身上一个都不管用。这正是开场自检要撞的那一条（Read 守卫自己必须被拦）。
///
/// `docs/dev-spec-*.md` 是第三档：**可读不可写**（`cat` 放行、`echo x >` 拦）。
#[test]
fn read_safe_rescues_the_prefix_family_but_not_the_named_files() {
    const CONTRACT: &str = "core/crates/contracts/src/lib.rs";
    const GUARD: &str = ".claude/hooks/guard_bash.py";
    const SPEC: &str = "docs/dev-spec-2026-09-09.md";

    // 前缀族：读取位置全放行
    for (label, cmd) in [
        ("cat", format!("cat {CONTRACT}")),
        ("grep", format!("grep -n fn {CONTRACT}")),
        ("ls", format!("ls -l {CONTRACT}")),
        ("head", format!("head -5 {CONTRACT}")),
        ("wc", format!("wc -l {CONTRACT}")),
        ("echo", format!("echo {CONTRACT}")),
        ("管道里的 cat", format!("cat {CONTRACT} | head -3")),
        (
            "git log 只读历史",
            format!("git log --oneline -1 -- {CONTRACT}"),
        ),
    ] {
        assert_eq!(bash(&cmd), 0, "前缀族 + {label} 被误拦：{cmd:?}");
    }

    // 前缀族：写入/执行位置全拦（`cp` 只读源也拦 —— 守卫判不出哪个参数是目标）
    for (label, cmd) in [
        ("git add 点名", format!("git add {CONTRACT}")),
        ("touch", format!("touch {CONTRACT}")),
        ("chmod", format!("chmod 644 {CONTRACT}")),
        (
            "cp（只读源，但判不出方向）",
            format!("cp {CONTRACT} /tmp/x"),
        ),
        ("守卫不认识的命令", format!("foobarbaz {CONTRACT}")),
    ] {
        assert_eq!(bash(&cmd), BLOCKED, "前缀族 + {label} 没被拦：{cmd:?}");
    }

    // 点名族：连 READ_SAFE 都救不了
    for (label, cmd) in [
        ("cat", format!("cat {GUARD}")),
        ("ls", format!("ls -l {GUARD}")),
        ("echo（纯输出，既不读也不写）", format!("echo {GUARD}")),
        ("echo 契约锁", "echo .contracts.lock".to_string()),
        (
            "echo settings.json",
            "echo .claude/settings.json".to_string(),
        ),
    ] {
        assert_eq!(bash(&cmd), BLOCKED, "点名族 + {label} 没被拦：{cmd:?}");
    }

    // 点名族的**目录**不在保护面里 —— 看得见有哪些文件，读不到内容
    for (label, cmd) in [
        ("ls 守卫所在目录", "ls -l .claude/hooks/"),
        ("git log -- .claude", "git log --oneline -1 -- .claude"),
    ] {
        assert_eq!(bash(cmd), 0, "{label} 被误拦：{cmd:?}");
    }

    // 第三档：冻结 spec 可读不可写
    assert_eq!(bash(&format!("cat {SPEC}")), 0, "冻结 spec 读不了");
    assert_eq!(bash(&format!("sed -n 1,5p {SPEC}")), 0, "冻结 spec 读不了");
    assert_eq!(
        bash(&format!("echo x > {SPEC}")),
        BLOCKED,
        "冻结 spec 写进去了"
    );
}

/// `find` 只要带 `-delete` / `-exec` / `-execdir` 就拦，**跟它指着哪儿完全无关**。
///
/// 老表写「`find … -delete` / `-exec`｜设计如此」是对的，但漏了范围有多大：
/// `find /tmp -name "aa3_*" -delete` —— 起点在 `/tmp`，跟冻结面一点关系都没有，照样拦。
/// 报的还是 `冻结面（proto/** 与 core/crates/contracts/**）（find 的 -delete/-exec 覆盖面判不出来）`。
///
/// **这是有意的宁可错杀**，别当误拦去改：`find` 的覆盖面要真算出来得把整棵树遍历一遍，
/// 而守卫只有命令文本。代价是每轨收尾清临时文件都得换写法 ——
/// **标准绕法**：`ls` 列出来 + 点名 `rm -f`（`rm -rf <非保护目录>` 本身是放行的）。
#[test]
fn find_with_delete_or_exec_is_blocked_no_matter_where_it_points() {
    let blocked: &[(&str, &str)] = &[
        ("-delete 在仓库里", r#"find . -name "*.rs" -delete"#),
        ("-exec 在仓库里", r#"find . -name "*.rs" -exec ls {} \;"#),
        ("-execdir", r#"find . -name "*.rs" -execdir ls {} \;"#),
        // ↓ 起点压根不在仓库里，照样拦
        ("-delete 指着 /tmp", r#"find /tmp -name "aa3_*" -delete"#),
        (
            "-exec 指着 /tmp",
            r#"find /tmp -name "aa3_*" -exec rm -f {} \;"#,
        ),
        (
            "-exec 的动作只是 echo",
            r#"find . -type f -exec echo {} \;"#,
        ),
    ];
    for (label, cmd) in blocked {
        assert_eq!(bash(cmd), BLOCKED, "{label} 没被拦：{cmd:?}");
    }

    let allowed: &[(&str, &str)] = &[
        ("光检索", r#"find . -name "*.rs""#),
        ("-print 是显式只读动作", r#"find . -name "*.rs" -print"#),
        // 标准绕法的后半截
        ("点名 rm -f", "rm -f /tmp/aa3_probe.txt"),
        (
            "rm -rf 非保护目录（走的是权限询问，不是守卫）",
            "rm -rf /tmp/aa3_dir",
        ),
    ];
    for (label, cmd) in allowed {
        assert_eq!(bash(cmd), 0, "{label} 被误拦：{cmd:?}");
    }
}

/// `AITE_RELOCK` 是按**文本**匹配的：赋成什么值、在注释里、在 heredoc 正文里，全拦。
///
/// `relock_and_self_authorization_are_blocked` 钉的是「不许自我授权」这条纪律；
/// 这一条量的是那道门有多宽 —— 它比纪律本身宽，宽出来的部分是**误拦**，
/// 而这些误拦恰好都站在 fail-closed 那一侧，所以**不提收窄**：
///
/// * `AITE_RELOCK=0`（明明是在**关**授权）也拦；
/// * 写在 `#` 注释里也拦；
/// * 写在 heredoc 正文里（比如往派单文档里抄一行运行命令）也拦 —— 这一格最容易撞到，
///   **标准绕法**：用 Write 工具落文件，别用 heredoc 抄带 `AITE_RELOCK=` 的命令。
///
/// 唯一放行的是**不带等号**的裸提及（`echo AITE_RELOCK`）——「赋值」这个形状是判据。
#[test]
fn the_relock_variable_is_matched_as_text_anywhere_in_the_command() {
    let blocked: &[(&str, &str)] = &[
        ("正经的自我授权", "AITE_RELOCK=1 echo hi"),
        ("赋 0 也拦（在关授权，照样算赋值）", "AITE_RELOCK=0 echo hi"),
        ("export 形式", "export AITE_RELOCK=1"),
        ("写在注释里", "echo hi  # AITE_RELOCK=1"),
        (
            "写在 heredoc 正文里（抄运行命令进文档时最容易撞）",
            "cat > /tmp/x <<'EOF'\nAITE_RELOCK=1 python3 review/x.py\nEOF",
        ),
    ];
    for (label, cmd) in blocked {
        assert_eq!(bash(cmd), BLOCKED, "{label} 没被拦：{cmd:?}");
    }

    let allowed: &[(&str, &str)] = &[
        ("裸提及，没有等号", "echo AITE_RELOCK"),
        ("别的变量赋值", "FOO=1 echo hi"),
        ("lock --check 是每轨的验收项", "aite contracts lock --check"),
    ];
    for (label, cmd) in allowed {
        assert_eq!(bash(cmd), 0, "{label} 被误拦：{cmd:?}");
    }
}

/// 整行 `#` 注释会被当成一条命令去判 —— 里面提到受保护路径就拦。
///
/// ```text
/// # 随手记一句 core/crates/contracts/src/lib.rs   → 拦（写入/执行位置）
/// echo hi  # 说的是 core/crates/contracts/src/lib.rs → 放行
/// ```
///
/// 差别在第一个词：整行注释的首词是 `#`，不在 `READ_SAFE` 里，于是那一行被当成
/// 「拿受保护路径去执行点什么」；尾部注释那一行的首词是 `echo`，读取位置，前缀族放行。
///
/// **这是一条真误拦** —— 一行注释不可能执行任何东西。没在本轨收窄：收益极小
/// （现实里极少单发一行注释），而任何「注释整行跳过」的改动都要动 `guard_bash.py`
/// 的分词那一段，本轨拿不到源码、锚点定位不到（派单 ③）。钉着，别再花时间重新发现它。
#[test]
fn a_whole_line_comment_is_parsed_as_a_command() {
    const CONTRACT: &str = "core/crates/contracts/src/lib.rs";
    const GUARD: &str = ".claude/hooks/guard_bash.py";

    assert_eq!(
        bash(&format!("# 随手记一句 {CONTRACT}")),
        BLOCKED,
        "整行注释不再被当命令了（好事）—— 这条 characterization 测试该跟着改"
    );
    assert_eq!(
        bash(&format!("# 随手记一句 {GUARD}")),
        BLOCKED,
        "整行注释不再被当命令了（好事）—— 这条 characterization 测试该跟着改"
    );
    // 对照：同一句话挂在真命令后面就放行
    assert_eq!(
        bash(&format!("echo hi  # 说的是 {CONTRACT}")),
        0,
        "尾部注释被误拦了"
    );
}
