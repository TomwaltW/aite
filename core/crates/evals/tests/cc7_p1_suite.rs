//! `evals/p1` 下的场景文件本身（CC7 ⑦）。
//!
//! 三条都只看 `evals/p1` **顶层**的 `*.yaml` / `*.yml`（与 `load_suite` 同口径：`read_dir`、
//! 不递归），子目录（如 FF9 的 `evals/p1/live/`）不算。
//!
//! W2 起 DD3–DD8、EE*、FF* 都往 `evals/p1/` 加自己的 yaml，而它们的可写面里没有本目录 ——
//! 所以第一条**只判包含**三个 CC7 名字，**不许改成判相等**（名字就叫 contains）。
use std::path::{Path, PathBuf};

use aite_evals::checks::CheckError;
use aite_evals::regex_mini::Regex;
use aite_evals::scenario::{Scenario, load_suite};
use aite_evals::{DepsOptions, build_deps, run_check};

const SUITE_DIR: &str = "../../../evals/p1";

const CC7_SEEDS: [&str; 3] = [
    "CC7_capability_override",
    "CC7_injection_in_history",
    "CC7_two_chats_independent",
];

fn suite() -> Vec<Scenario> {
    load_suite(Path::new(SUITE_DIR)).expect("evals/p1 加载")
}

/// 顶层的场景文件（与 `load_suite` 同一个筛法）。
fn scenario_files() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(SUITE_DIR)
        .expect("读 evals/p1")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("yaml") | Some("yml")
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn p1_suite_contains_the_three_cc7_seeds() {
    let names: Vec<String> = suite().into_iter().map(|s| s.name).collect();
    for seed in CC7_SEEDS {
        assert!(
            names.iter().any(|n| n == seed),
            "evals/p1 里缺 {seed}；实际 {names:?}"
        );
    }
}

#[test]
fn p1_every_expect_uses_a_known_check() {
    // 空替身上跑一遍：未知 check 返回带「未知的 check」的 CheckError；别的 CheckError
    // （比如缺参数）也说明断言写错了，一并报出来。「没过」（Ok(Some)）在空替身上是正常的。
    let deps = build_deps(&Scenario::named("probe"), &DepsOptions::default()).expect("造 deps");
    for sc in suite() {
        assert!(!sc.expect.is_empty(), "{} 一条 expect 都没有", sc.name);
        for (i, spec) in sc.expect.iter().enumerate() {
            let map = spec
                .as_object()
                .unwrap_or_else(|| panic!("{} expect[{i}] 不是 mapping：{spec}", sc.name));
            match run_check(&deps, map) {
                Err(CheckError(msg)) if msg.contains("未知的 check") => {
                    panic!("{} expect[{i}] 用了未知的 check：{msg}", sc.name)
                }
                Err(CheckError(msg)) => panic!("{} expect[{i}] 写错了：{msg}", sc.name),
                Ok(_) => {}
            }
        }
    }
}

#[test]
fn p1_scenario_files_carry_a_track_prefix() {
    // 要放得过 CC7_、T0c_、FF10_、T01c_ 这类轨号
    let re = Regex::new("^[A-Z]+[0-9]+[a-z]?_").expect("正则");
    let files = scenario_files();
    assert!(!files.is_empty(), "evals/p1 下没有场景文件");
    for path in files {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        assert!(
            re.is_match(stem),
            "{} 的文件名不以轨号开头（形如 <轨号>_*.yaml）",
            path.display()
        );
    }
}
