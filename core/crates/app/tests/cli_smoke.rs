//! `aite` 命令面的状态门禁。
//!
//! R0 时期这里断言"所有子命令都是 not implemented"；R3（evidence）与 R7（evals）落地后
//! 那条断言就过期了 —— 现在按**各子命令当下该有的样子**分别钉住：
//!   run / preflight      → RΩ 还欠着：退出码 2 + not implemented
//!   evals                → R7 已落地：--list 出 10 个场景名；跑 suite 是退出码 1 + `passed 0/10`
//!                          + stderr 为空（scripted 路径的硬约束），失败原因是"等 RΩ 接 PlaneFactory"
//!   evidence             → R3 已落地：找不到任务是退出码 2 + 人话，不是 not implemented
//!   contracts lock       → R0：--check 干净树上 OK；--write 没有 AITE_RELOCK=1 就拒绝
//!
//! RΩ 把 run / preflight / PlaneFactory 接上之后，本文件的前两组断言要跟着改成真行为，
//! `passed 0/10` 那条要改成 `passed 10/10` + 退出码 0（§4.2 B8）。
use std::process::Command;

fn aite() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aite"))
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// RΩ 还欠的两个：必须"明说没做"，而不是 panic 或假装成功退出 0。
#[test]
fn omega_subcommands_still_say_not_implemented() {
    for args in [vec!["run"], vec!["preflight"]] {
        let out = aite().args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not implemented"),
            "{args:?}"
        );
    }
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

/// R7 落地、RΩ 未接线时的确切形状：退出码 1、最后一行 `passed 0/10`、stderr 一个字节都没有、
/// 每个场景以 phase=wiring 报人话原因（不是异常栈）。
/// stderr 为空是硬约束：check.sh 的 B8 与 CI 读的就是 stdout 那两条。
#[test]
fn evals_run_reports_pending_wiring_without_noise() {
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
    assert_eq!(out.status.code(), Some(1), "有失败场景时退出码必须是 1");
    assert!(
        out.stderr.is_empty(),
        "scripted 路径 stderr 必须为空，实际：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().last().unwrap(),
        "passed 0/10",
        "最后一行必须是 passed k/n"
    );
    let summary: serde_json::Value =
        serde_json::from_str(stdout.trim_end().rsplit_once('\n').unwrap().0)
            .expect("摘要必须是 JSON");
    assert_eq!(summary["total"], 10);
    assert_eq!(summary["failed"], 10);
    assert_eq!(summary["contract_version"], "p0.2");
    for sc in summary["scenarios"].as_array().unwrap() {
        assert_eq!(sc["phase"], "wiring", "{sc:?}");
        let reason = sc["reason"].as_str().unwrap_or_default();
        assert!(reason.contains("ControlPlane"), "原因要是人话：{reason}");
        assert!(!reason.contains("panicked"), "不许是异常栈：{reason}");
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
