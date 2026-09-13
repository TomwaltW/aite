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

/// **进程级**那条：配置指着一个不存在的 system prompt 时，`aite preflight` 的退出码
/// 必须是 1 —— 而不是像 2026-09-12 那次一样报「全部没红，可以起飞」然后退 0。
///
/// 那天的现场：`preflight --offline` 退出码 0、`aite run` 退出码 2
/// （`读不到 system prompt：aite/worker/prompts/platform.md`），因为那份配置还指着当天
/// 被删掉的 Python 树，而七组里没有一组碰 `worker.system_prompt_path`。
/// 判据面（FAIL 而不是 WARN、「怎么补」的正文、offline 下照跑）钉在
/// `tests/preflight_e2e.rs`；**这一条只钉真二进制的退出码和它打给人看的那两行** ——
/// 脚本判的是退出码，上面那些断言全绿而退出码是 0 的话，总管照样会起飞失败。
#[test]
fn preflight_exits_one_when_the_system_prompt_is_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("stale.yaml");
    // storage 指进 tempdir：第 7 组的可写探测是真建一个目录再删，别碰仓库的 data/。
    std::fs::write(
        &cfg,
        format!(
            "platform: feishu\nmodel:\n  provider: openai_compat\n  \
             base_url: http://127.0.0.1:1/v1\n  model: fake-model\n\
             worker:\n  system_prompt_path: aite/worker/prompts/platform.md\n\
             storage:\n  sqlite_path: {d}/aite.db\n  evidence_dir: {d}/evidence\n  \
             artifacts_dir: {d}/artifacts\n",
            d = tmp.path().display()
        ),
    )
    .expect("写 config");

    let out = aite()
        .args(["preflight", "--offline", "--config"])
        .arg(&cfg)
        .current_dir(repo_root())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "退出码必须是 1：\n{stdout}");
    let row = stdout
        .lines()
        .find(|l| l.starts_with("[1/7]"))
        .unwrap_or_else(|| panic!("没有第 1 组那一行：\n{stdout}"));
    assert!(row.contains("FAIL"), "第 1 组没红：{row}");
    assert!(row.contains("worker.system_prompt_path"), "{row}");
    assert!(
        stdout.contains("core/crates/worker/prompts/platform.md"),
        "「怎么补」里没有正确路径：\n{stdout}"
    );
    assert!(
        !stdout.contains("全部没红，可以起飞"),
        "起不来还说可以起飞 —— 这正是 2026-09-12 那条：\n{stdout}"
    );
}

/// **进程级**那条（注入判据）：`platform: fake` 时退出码必须是 1。
///
/// 2026-09-13 的现场：`preflight --offline` 报「汇总：OK 2 · WARN 1 · FAIL 0 · SKIP 4 /
/// 全部没红，可以起飞。」退出 0，紧接着 `aite run` 退出码 2
/// （`config.platform=fake 时必须由调用方注入平台实现`）。与上面那条同形状、同一天记的账。
///
/// 判据面（2/3/4 组 SKIP、「怎么补」的正文、与 `build_app` 的对拍）钉在
/// `tests/preflight_e2e.rs`；**这一条只钉真二进制的退出码和它打给人看的那两行**。
#[test]
fn preflight_exits_one_when_the_platform_needs_an_injection() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("fake.yaml");
    // prompt 指到仓库里真有的那份：要红的是注入这一条，别让 prompt 那条抢答。
    std::fs::write(
        &cfg,
        format!(
            "platform: fake\nmodel:\n  provider: openai_compat\n  \
             base_url: http://127.0.0.1:1/v1\n  model: fake-model\n\
             worker:\n  system_prompt_path: core/crates/worker/prompts/platform.md\n\
             storage:\n  sqlite_path: {d}/aite.db\n  evidence_dir: {d}/evidence\n  \
             artifacts_dir: {d}/artifacts\n",
            d = tmp.path().display()
        ),
    )
    .expect("写 config");

    let out = aite()
        .args(["preflight", "--offline", "--config"])
        .arg(&cfg)
        .current_dir(repo_root())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "退出码必须是 1：\n{stdout}");
    let row = stdout
        .lines()
        .find(|l| l.starts_with("[1/7]"))
        .unwrap_or_else(|| panic!("没有第 1 组那一行：\n{stdout}"));
    assert!(row.contains("FAIL"), "第 1 组没红：{row}");
    assert!(row.contains("platform=fake"), "{row}");
    assert!(
        !stdout.contains("全部没红，可以起飞"),
        "起不来还说可以起飞 —— 这正是 2026-09-13 那条：\n{stdout}"
    );
    // 七行一行不少（fake 那一档把 2/3/4 组 SKIP 掉了，不是把它们删了）。
    assert_eq!(
        stdout.lines().filter(|l| l.starts_with('[')).count(),
        7,
        "{stdout}"
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
/// 不带 `--` 的 `aite run --help` **已经不被 clap 截胡了**（W3 ③ 在 `main.rs` 的 `Run`
/// variant 上加了 `#[command(disable_help_flag = true)]`）。这条留着的理由变了：它钉的
/// 不再是「够得着的那条」，而是**带 `--` 的写法一个字节都没跟着变** —— 两个写法现在
/// 打的是同一份用法，见下面 `help_with_and_without_the_dashes_prints_the_very_same_thing`。
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
/// 写法是 `aite evals -- --help`（原来是「不认识的子命令」→ stderr + 2）。
/// 不加 `--` 的 `aite evals --help` 从前跟 `aite run --help` 同病（被 clap 截胡），
/// **W3 ③ 一并修了** —— 两个写法现在打同一份用法。这条钉的是带 `--` 那半没变。
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

// --- W3 ③：四个转发型子命令的 `--help`，带不带 `--` 都得是真帮助 -------------------
//
// **病**：run / evals / evidence / preflight 的参数不由 clap 解析（`trailing_var_arg`
// + `allow_hyphen_values`，见 `main.rs` 的 `Cmd`），所以 clap 不知道任何一个真实选项，
// 它自动生成的那份 `--help` 里只有一句 `[ARGS]...` —— **而退出码是 0**。坏掉的帮助和
// 好的帮助在脚本里长得一模一样，`aite run --help | grep -q -- --grace` 这种判据静悄悄
// 地永远不中。根治是在 `main.rs` 给这四个 variant 加 `disable_help_flag = true`，
// 让 `--help` / `-h` 跟着 argv 落进手写解析器。
//
// **为什么钉在这个文件里**：病长在 `main.rs` 那一层，而 clap 的截胡只在真进程里发生。
// 单元测试直调 `cli::run_capture(...)` 看不见 `main.rs` —— V6 的提交里明写过
// 「原来那条回归就是从这一层躲过去的」。
//
// **`[ARGS]...` 是坏帮助的指纹**：那是 clap 对一个它一无所知的位置参数的唯一写法。
// 正向断言（有没有某个真实选项名）与反向断言（有没有这个指纹）两头都要，
// 少一头都能被「打一份别的空帮助」蒙过去。

/// 一个子命令 + 它真用法里必然出现的一个真实选项名（`--help` 好了才打得出来）。
const HELP_CASES: [(&str, &str); 4] = [
    ("run", "--grace"),
    ("preflight", "--offline"),
    ("evals", "demo-fixture"),
    ("evidence", "show"),
];

/// 八个写法（四个子命令 × 带不带 `--`）：stdout + stderr 空 + 退出码 0 + 有真实选项名。
///
/// 带 `--` 的那四个是 V6 刚修好的，这里一并钉住 —— **它们的行为一个字节都不许变**
/// （`snap.sh` 那轮逐字节对比在 W3 回执里；这条是留在门禁里的那一半）。
#[test]
fn help_on_the_four_forwarding_subcommands_is_real_with_and_without_the_dashes() {
    for (sub, needle) in HELP_CASES {
        for extra in [vec![], vec!["--"]] {
            let mut args = vec![sub];
            args.extend(extra.iter().copied());
            args.push("--help");
            let shown = args.join(" ");

            let out = aite()
                .args(&args)
                .current_dir(repo_root())
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            assert_eq!(out.status.code(), Some(0), "aite {shown}：{stderr}");
            assert!(stderr.is_empty(), "aite {shown} 往 stderr 写了：{stderr}");
            assert!(
                stdout.contains(needle),
                "aite {shown} 打的帮助里没有真实选项 {needle}：{stdout}"
            );
            assert!(
                !stdout.contains("[ARGS]..."),
                "aite {shown} 打的是 clap 那份空帮助（`[ARGS]...`）：{stdout}"
            );
        }
    }
}

/// `-h` 与 `--help` 一视同仁 —— 短写法是排障时手最顺的那个，别只修长的。
#[test]
fn short_help_on_the_four_forwarding_subcommands_is_real_too() {
    for (sub, needle) in HELP_CASES {
        let out = aite()
            .args([sub, "-h"])
            .current_dir(repo_root())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            out.status.code(),
            Some(0),
            "aite {sub} -h：{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains(needle), "aite {sub} -h：{stdout}");
        assert!(!stdout.contains("[ARGS]..."), "aite {sub} -h：{stdout}");
    }
}

/// 带 `--` 与不带 `--` 打的必须是**同一份**帮助。
///
/// 这条防的是「只修一半」：不带 `--` 的那半改好了、带 `--` 的那半被顺手改成别的样子，
/// 上面两条各自都还绿（两边都有真实选项名、都没有 `[ARGS]...`），只有这条会红。
#[test]
fn help_with_and_without_the_dashes_prints_the_very_same_thing() {
    for (sub, _) in HELP_CASES {
        let plain = aite()
            .args([sub, "--help"])
            .current_dir(repo_root())
            .output()
            .unwrap();
        let dashed = aite()
            .args([sub, "--", "--help"])
            .current_dir(repo_root())
            .output()
            .unwrap();
        assert_eq!(
            plain.stdout,
            dashed.stdout,
            "aite {sub} --help 与 aite {sub} -- --help 打的不是同一份：\n{}\n----\n{}",
            String::from_utf8_lossy(&plain.stdout),
            String::from_utf8_lossy(&dashed.stdout)
        );
        assert_eq!(plain.status.code(), dashed.status.code(), "aite {sub}");
    }
}

/// 顶层与 `contracts` 不在这次改动面里，**它们的帮助不许被顺带改掉**。
///
/// 真踩过一次：说明文字写成 `///` 挂在 `enum Cmd` 上，被 clap derive 当成顶层命令的
/// about，`aite --help` 的第一行从「Aite core（Rust）」变成了那段说明。降级成 `//`
/// 才修掉。`contracts` 的参数是真 clap 子命令，帮助本来就带着 `lock`。
#[test]
fn top_level_and_contracts_help_are_untouched() {
    let top = aite().args(["--help"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&top.stdout);
    assert_eq!(top.status.code(), Some(0));
    assert!(
        stdout.starts_with("Aite core（Rust）"),
        "顶层帮助的 about 被顶掉了：{stdout}"
    );
    for sub in ["run", "contracts", "evals", "evidence", "preflight"] {
        assert!(stdout.contains(sub), "顶层帮助该列出 {sub}：{stdout}");
    }

    let contracts = aite().args(["contracts", "--help"]).output().unwrap();
    let text = String::from_utf8_lossy(&contracts.stdout);
    assert_eq!(contracts.status.code(), Some(0));
    assert!(text.contains("lock"), "contracts 的帮助该带着 lock：{text}");
}
