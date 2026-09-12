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
