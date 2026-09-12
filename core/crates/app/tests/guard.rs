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
//! **它验不了的那件事**：守卫在**当前这个会话里有没有真的挂上**。hook 命令是
//! `python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"`，而 `CLAUDE_PROJECT_DIR`
//! 在 claude 进程启动那一刻定死 —— 会话在仓库子目录里起（或先起 claude 再 `cd`），
//! 它就指着子目录，文件不存在 → hook 执行失败 → 非阻塞放行、不报警 → 整场静默失效。
//! 这里验的是「守卫脚本本身好使」和「hook 配置那条命令写得对」，**不是**「它挂上了」。
//! 后者只有开场自检那一条能验：Read 一下守卫脚本自己，必须被拦。
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
