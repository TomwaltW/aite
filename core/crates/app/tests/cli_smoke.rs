//! `aite` 命令面的状态门禁。
//!
//! R0 时期这里断言"所有子命令都是 not implemented"；R3（evidence）/ R7（evals）/ RΩ
//! 落地后按**各子命令当下该有的样子**分别钉住：
//!   run                  → RΩ 已落地：读不到配置是退出码 2 + 一行人话（不是 panic、不是 0）
//!   preflight            → RΩ 已落地：`--offline` 七行都有结论，退出码 0，第 2 组只报变量名
//!                          （「绝不打印密钥取值」那条红线钉在 `src/preflight.rs` 的单元测试里 ——
//!                          `--offline` 下 3/4/5 组全 skip，Redactor 在这一档根本没上场）
//!   evals                → R7 + RΩ：--list 出 10 个场景名；跑 suite 是退出码 0 + `passed 10/10`
//!                          + stderr 为空（scripted 路径的硬约束）
//!   evidence             → R3 已落地：找不到任务是退出码 2 + 人话，不是 not implemented
//!   contracts lock       → R0：--check 干净树上 OK；--write 没有 AITE_RELOCK=1 就拒绝
use std::process::Command;

fn aite() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aite"))
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// `aite run`：配置读不到时是「一行人话 + 退出码 2」，不是 panic、不是假装成功。
///
/// 在仓库根跑且没有 `config/aite.yaml`（它不入库）—— 这就是真机第一次的状态。
#[test]
fn run_without_a_config_says_which_file_is_missing() {
    let out = aite()
        .args(["run", "--config", "no/such/aite.yaml"])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("aite 起不来"), "{text}");
    assert!(text.contains("no/such/aite.yaml"), "{text}");
    assert!(text.contains("config/aite.example.yaml"), "{text}");
    assert!(!text.contains("not implemented"), "{text}");
    assert!(!text.contains("panicked"), "{text}");
}

/// `aite preflight --offline`：七项各占一行，1/2/7 有结论、其余 skip，退出码 0，
/// 且第 2 组点名环境变量时只报**名字**、不报取值。
///
/// 这一档不碰网络也不碰 docker，所以 CI 上跑得动。
///
/// **它不是「绝不打印密钥」的守门人**，别把它当成那个：`redactor.add()` 的全部调用点
/// 都在第 3/4/5 组里，`--offline` 下这几组全 skip —— 取值根本没机会进报告，
/// 拿 Redactor 当判据在这一档是恒真的（把 `Redactor` 整个删掉这条也不会红）。
/// 那道红线钉在 `src/preflight.rs` 的 `mod tests` 里：纯函数、`add()` 的调用点、
/// 渲染前真的过了一遍，三层各一条。这里只钉一件事 —— 第 2 组报的是名字，不是取值。
#[test]
fn preflight_offline_reports_seven_lines_and_names_env_vars_without_values() {
    let out = aite()
        .args(["preflight", "--offline", "--json"])
        .current_dir(repo_root())
        .env("AITE_MODEL_API_KEY", "sk-preflight-smoke-secret")
        .env("FEISHU_APP_SECRET", "feishu-smoke-secret")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "--offline 不该有 FAIL");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("--json 必须是 JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["total"], 7);
    let checks = report["checks"].as_array().expect("checks 是数组");
    assert_eq!(checks.len(), 7);
    let names: Vec<&str> = checks.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "config",
            "env",
            "feishu_token",
            "feishu_identity",
            "model",
            "sandbox",
            "storage"
        ]
    );
    // 1/2/7 有结论，3/4/5/6 是 skip
    for (i, want_skip) in [false, false, true, true, true, true, false]
        .iter()
        .enumerate()
    {
        let status = checks[i]["status"].as_str().unwrap();
        assert_eq!(status == "skip", *want_skip, "{}：{status}", names[i]);
    }
    // 第 2 组：点名到的变量报名字，取值一个字都不许跟出来。
    // （这两个变量在上面用 .env(...) 设过，所以第 2 组一定会把它们算作「已设置」。）
    let whole = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        whole.contains("AITE_MODEL_API_KEY"),
        "变量名该报出来：{whole}"
    );
    assert!(
        whole.contains("FEISHU_APP_SECRET"),
        "变量名该报出来：{whole}"
    );
    assert!(
        !whole.contains("sk-preflight-smoke-secret"),
        "第 2 组把模型密钥的取值也报出来了：{whole}"
    );
    assert!(
        !whole.contains("feishu-smoke-secret"),
        "第 2 组把飞书密钥的取值也报出来了：{whole}"
    );
}

/// R7 的评测面：场景清单读得出来，且恰好是 §3.8 的十个。
#[test]
fn evals_lists_the_ten_p0_scenarios() {
    let out = aite()
        .args(["evals", "run", "evals/p0", "--list"])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let names: Vec<String> = serde_json::from_slice(&out.stdout).expect("--list 必须是 JSON 数组");
    assert_eq!(names.len(), 10, "{names:?}");
    assert_eq!(names[0], "01_simple_qa");
    assert_eq!(names[9], "10_duplicate_event");
}

/// B8（§4.2）：退出码 0、最后一行 `passed 10/10`、stderr 一个字节都没有。
///
/// stderr 为空是硬约束：check.sh 的 B8 与 CI 读的就是 stdout 那两条。
#[test]
fn evals_run_passes_all_ten_scenarios_without_noise() {
    let out = aite()
        .args([
            "evals",
            "run",
            "evals/p0",
            "--platform",
            "fake",
            "--model",
            "scripted",
        ])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "十个场景全过时退出码必须是 0");
    assert!(
        out.stderr.is_empty(),
        "scripted 路径 stderr 必须为空，实际：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().last().unwrap(),
        "passed 10/10",
        "最后一行必须是 passed k/n"
    );
    let summary: serde_json::Value =
        serde_json::from_str(stdout.trim_end().rsplit_once('\n').unwrap().0)
            .expect("摘要必须是 JSON");
    assert_eq!(summary["total"], 10);
    assert_eq!(summary["passed"], 10);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["contract_version"], "p0.2");
    for sc in summary["scenarios"].as_array().unwrap() {
        assert_eq!(sc["phase"], "ok", "{sc:?}");
        assert_eq!(sc["passed"], true, "{sc:?}");
    }
}

/// R3 的证据面：找不到任务是"人话 + 退出码 2"，不是 not implemented，也不是 panic。
#[test]
fn evidence_show_reports_missing_task_in_plain_words() {
    let out = aite()
        .args(["evidence", "show", "definitely-not-a-task"])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("definitely-not-a-task"), "{text}");
    assert!(!text.contains("not implemented"), "{text}");
}

// --- V6 ①：`--model live` / `--sandbox docker` 的校验不许跑在参数解析前面 ------------
//
// **为什么这几条必须钉在这个文件里**：`core/crates/evals/tests/cli_help.rs` 走的是
// `cli::run_capture(args, &Wiring::default())` —— 直调库函数，`main.rs` 那一层
// （以及 `wiring::evals_wiring`）根本不在调用链上。病就长在那一层：`main.rs` 先无条件
// 调一次 `evals_wiring`，后者一见 argv 里有 `--model live` 就 `load_config` + 试造，
// 失败即退出 2。于是 `-h` 打不出用法、`--list` 列不出场景、`--platform` 拼错了都被模型
// 抢了先，而那边三条用例全绿。这里起的是真进程，才看得见。
//
// **一律显式给 `--config no/such/aite.yaml`**：`repo_root()` 在主仓根下**有**
// `config/aite.yaml`（它不入库，各 worktree 里没有），靠「默认配置不存在」当前提的话，
// 同一条用例在 worktree 里绿、在主仓里验的是另一回事 —— 又一条环境相关的假绿。

/// 一台没配好模型的机器上，`--list` 仍然列得出场景名。
#[test]
fn evals_list_is_not_blocked_by_an_unusable_live_model() {
    let out = aite()
        .args([
            "evals",
            "run",
            "evals/p0",
            "--list",
            "--model",
            "live",
            "--config",
            "no/such/aite.yaml",
        ])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let names: Vec<String> = serde_json::from_slice(&out.stdout).expect("--list 必须是 JSON 数组");
    assert_eq!(names.len(), 10, "{names:?}");
}

/// `--sandbox docker` 同理：`--list` 用不着 edge，就不该被「连不上 edge」挡住。
#[test]
fn evals_list_is_not_blocked_by_an_unusable_docker_sandbox() {
    let out = aite()
        .args([
            "evals",
            "run",
            "evals/p0",
            "--list",
            "--sandbox",
            "docker",
            "--config",
            "no/such/aite.yaml",
        ])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let names: Vec<String> = serde_json::from_slice(&out.stdout).expect("--list 必须是 JSON 数组");
    assert_eq!(names.len(), 10, "{names:?}");
}

/// `-h` 要用法进 stdout、stderr 一个字节都没有、退出码 0 —— 命令行上多一个
/// `--model live` 也一样。RΩ 刚修好的这条，就是被 `main.rs` 那一层打回原形的。
#[test]
fn evals_help_stays_on_stdout_even_with_live_model_on_the_command_line() {
    let out = aite()
        .args([
            "evals",
            "run",
            "-h",
            "--model",
            "live",
            "--config",
            "no/such/aite.yaml",
        ])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "`-h` 不许往 stderr 写一个字节，实际：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("用法："),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// 纯参数错误要报**那个参数**，不许被模型的「起不来」抢先。
#[test]
fn evals_bad_platform_is_reported_before_the_live_model() {
    let out = aite()
        .args([
            "evals",
            "run",
            "evals/p0",
            "--platform",
            "feishu",
            "--model",
            "live",
            "--config",
            "no/such/aite.yaml",
        ])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("--platform"), "{text}");
    assert!(
        !text.contains("--model live 起不来"),
        "模型抢在参数校验前面报了：{text}"
    );
}

/// **反向的那条：「起飞前试造一次」不许丢。**
///
/// 没有它，把 live 校验整个删掉也能让上面几条一起变绿 —— 而代价是配置缺一样时，
/// 十个场景各自跑到第一次 chat 才抛，被 worker 当模型 5xx 白重试 2 次（2s + 5s），
/// 十次 7 秒空等，真正的原因一个字都看不到。
#[test]
fn evals_live_model_is_still_built_once_before_takeoff() {
    let out = aite()
        .args([
            "evals",
            "run",
            "evals/p0",
            "--only",
            "01_simple_qa",
            "--model",
            "live",
            "--config",
            "no/such/aite.yaml",
        ])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("--model live 起不来"), "{text}");
    assert!(text.contains("no/such/aite.yaml"), "{text}");
    assert!(!text.contains("passed"), "根本不该跑到场景里去：{text}");
}

// --- V6 ④：几条低危但会在录演示 / 排障时正面撞上的命令行毛病 ----------------------

/// ④a：`--grace` 传超大有限数不许一路穿到收尾那一刻才 panic。
///
/// 引爆点在 `run.rs` 的 `Duration::from_secs_f64(grace.max(0.0))`：`max(0.0)` 挡住了负数，
/// 挡不住上溢，而 panic 会把它后面的 `sandbox.close_all()` / `store.close()` 整段跳过。
/// **这条只钉住「挡在门口」** —— `ServeOptions::shutdown_grace_sec` 仍然是个裸 `f64`，
/// 拆弹归 `run.rs`（V5）。
#[test]
fn run_rejects_a_grace_that_would_overflow_duration() {
    for raw in ["1e300", "-1", "nan", "inf"] {
        let out = aite()
            .args(["run", "--grace", raw, "--config", "no/such/aite.yaml"])
            .current_dir(repo_root())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "--grace {raw} 没被挡下");
        let text = String::from_utf8_lossy(&out.stderr);
        assert!(text.contains("--grace"), "--grace {raw}：{text}");
        assert!(
            !text.contains("aite 起不来"),
            "--grace {raw} 被当成合法值收下了，走到读配置那一步了：{text}"
        );
    }
    // 边界内的值要照常放行（走到读配置那一步才失败），别把门关死了。
    for raw in ["0", "20", "86400"] {
        let out = aite()
            .args(["run", "--grace", raw, "--config", "no/such/aite.yaml"])
            .current_dir(repo_root())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stderr);
        assert!(text.contains("aite 起不来"), "--grace {raw} 被误拦：{text}");
    }
}

/// ④b(i)：`aite run -- -h` 要用法进 stdout + 退出码 0，不是 stderr + 2。
///
/// **`aite run --help` 仍然被 clap 截胡**（打的是 clap 那份不含任何真实选项的帮助）——
/// 根治要在 `main.rs` 的 `Run` variant 上加 `#[command(disable_help_flag = true)]`，
/// 而 `main.rs` 归 R0。所以这里钉的是够得着的那条。
#[test]
fn run_help_after_double_dash_goes_to_stdout_with_exit_zero() {
    let out = aite()
        .args(["run", "--", "-h"])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "用法不许走 stderr：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("aite run"), "{stdout}");
    assert!(stdout.contains("--grace"), "{stdout}");
}

/// ④d：`aite evals demo-fixture --help` / `-h` 两个写法都要 stdout + 退出码 0。
///
/// 原来两个都掉进 `demo_fixture::run` 的「不认识的子命令」→ stderr + 2
/// （RΩ 那次只修了 `evals run` 一半）。
#[test]
fn evals_demo_fixture_help_goes_to_stdout_with_exit_zero() {
    for flag in ["--help", "-h"] {
        let out = aite()
            .args(["evals", "demo-fixture", flag])
            .current_dir(repo_root())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "demo-fixture {flag}");
        assert!(
            out.stderr.is_empty(),
            "demo-fixture {flag} 往 stderr 写了：{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("demo-fixture"), "{stdout}");
        assert!(
            !stdout.contains("不认识的子命令"),
            "求助被当成了参数错误：{stdout}"
        );
    }
}

/// ④d 的邻居：子命令位置上的 `--help` 也要 stdout + 退出码 0。
///
/// 够得着的写法是 `aite evals -- --help`（原来是「不认识的子命令」→ stderr + 2）。
/// **`aite evals --help`（不加 `--`）仍然被 clap 截胡**，跟 `aite run --help` 同病 ——
/// 打的是 clap 那份不含任何真实选项的帮助，根治要动 `main.rs`（归 R0）。
#[test]
fn evals_help_in_the_subcommand_slot_goes_to_stdout_with_exit_zero() {
    let out = aite()
        .args(["evals", "--", "--help"])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "求助不许走 stderr：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("用法："), "{stdout}");
    assert!(!stdout.contains("不认识的子命令"), "{stdout}");
}

#[test]
fn contracts_lock_check_is_ok_on_a_clean_tree() {
    let out = aite()
        .args(["contracts", "lock", "--check", "--repo"])
        .arg(repo_root())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("OK "),
        "stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn contracts_lock_write_requires_relock_env() {
    let out = aite()
        .args(["contracts", "lock", "--write", "--repo"])
        .arg(repo_root())
        .env_remove("AITE_RELOCK")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("AITE_RELOCK"));
}
