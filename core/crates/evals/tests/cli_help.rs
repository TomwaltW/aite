//! `-h` / `--help` 的收场（RΩ 补，审核记账 R7）。
//!
//! 「人要看用法」和「参数写错了」在命令行上是两件事：
//! 前者 stdout + 退出码 0（argparse 就是这样，脚本里 `set -e` 不会被它带死），
//! 后者 stderr + 退出码 2。原来两者都走后者 —— `aite evals run --help` 会让 CI 挂掉。
use aite_evals::cli::{Captured, Wiring, run_capture};

fn call(args: &[&str]) -> Captured {
    run_capture(
        args.iter().map(|s| (*s).to_string()).collect(),
        &Wiring::default(),
    )
}

#[test]
fn help_goes_to_stdout_with_exit_zero() {
    for flag in ["-h", "--help"] {
        let out = call(&["run", flag]);
        assert_eq!(out.code, 0, "{flag} 该是退出码 0");
        assert!(
            out.stderr.is_empty(),
            "{flag} 不该往 stderr 写：{:?}",
            out.stderr
        );
        assert!(
            out.stdout_text().contains("用法："),
            "{flag} 该把用法打到 stdout：{:?}",
            out.stdout
        );
    }
}

#[test]
fn a_bad_flag_is_still_stderr_with_exit_two() {
    let out = call(&["run", "evals/p0", "--nope"]);
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty(), "报错不该占 stdout：{:?}", out.stdout);
    assert!(out.stderr_text().contains("--nope"), "{:?}", out.stderr);
}

/// 缺场景目录仍然是「参数写错了」那一档。
#[test]
fn a_missing_suite_is_an_error_not_help() {
    let out = call(&["run"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr_text().contains("缺场景目录"), "{:?}", out.stderr);
}
