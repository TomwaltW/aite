//! R0 骨架的命令面：各占位子命令必须"退出码 2 + not implemented"，而不是 panic 或退出 0。
use std::process::Command;

fn aite() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aite"))
}

#[test]
fn placeholder_subcommands_exit_2_with_not_implemented() {
    for args in [
        vec!["run"],
        vec![
            "evals",
            "run",
            "evals/p0",
            "--platform",
            "fake",
            "--model",
            "scripted",
        ],
        vec!["evidence", "show", "x"],
        vec!["preflight"],
    ] {
        let out = aite().args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not implemented"),
            "{args:?}"
        );
    }
}

#[test]
fn contracts_lock_check_is_ok_on_a_clean_tree() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let out = aite()
        .args(["contracts", "lock", "--check", "--repo"])
        .arg(&root)
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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let out = aite()
        .args(["contracts", "lock", "--write", "--repo"])
        .arg(&root)
        .env_remove("AITE_RELOCK")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("AITE_RELOCK"));
}
