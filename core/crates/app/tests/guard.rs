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
//! | `cargo fmt --all`（写模式，碰冻结面） | 固有代价 | 逐个文件 `rustfmt --edition 2024 <file>` |
//! | heredoc 正文里有配不平的引号 / 中文引号 | 固有代价 | 改用 Write 工具落文件，别用 heredoc |
//! | heredoc 正文被判「不透明载荷」 | 固有代价 | 同上（本轨 32KB 中文正文没复现，判据不是纯长度） |
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
