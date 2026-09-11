//! `aite evals demo-fixture` 的测试（移植自 `tests/tools/test_demo_fixture.py`，20 条）。
//!
//! 这个子命令的存在理由只有一条：**演示不该每次都是一场赌博**。所以测试也就盯着那条 ——
//! 不是「函数返回了个 list」，而是「照着剧本演的人，会不会看到和彩排时不同的东西」。
use aite_evals::cli::{Captured, Wiring, run_capture};
use aite_evals::demo_fixture as fx;

fn cli(args: &[&str]) -> Captured {
    let mut argv = vec!["demo-fixture".to_string()];
    argv.extend(args.iter().map(|s| s.to_string()));
    run_capture(argv, &Wiring::default())
}

fn default_start() -> (i32, u32) {
    fx::parse_start(fx::DEFAULT_START).unwrap()
}

// --------------------------------------------------------------------------
// 可复现
// --------------------------------------------------------------------------

/// 彩排看到的那张图和正式演示那张，是同一张 —— 全靠这条。
#[test]
fn same_args_twice_are_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.csv"), dir.path().join("b.csv"));
    assert_eq!(cli(&["csv", "-o", a.to_str().unwrap()]).code, 0);
    assert_eq!(cli(&["csv", "-o", b.to_str().unwrap()]).code, 0);
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

/// 没有这条，上一条可能只是因为 seed 压根没被用上。
#[test]
fn a_different_seed_really_changes_the_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.csv"), dir.path().join("b.csv"));
    cli(&["csv", "-o", a.to_str().unwrap(), "--seed", "1"]);
    cli(&["csv", "-o", b.to_str().unwrap(), "--seed", "2"]);
    assert_ne!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

/// 默认起始月跟了今天，就没有「同样参数」这回事了。
#[test]
fn start_month_is_a_frozen_constant_not_today() {
    assert_eq!(fx::DEFAULT_START, "2024-09");
}

#[test]
fn line_endings_are_lf_and_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sales.csv");
    cli(&["csv", "-o", out.to_str().unwrap()]);
    let raw = std::fs::read(&out).unwrap();
    assert!(!raw.contains(&b'\r'));
    assert_eq!(*raw.last().unwrap(), b'\n');
    String::from_utf8(raw).expect("解不开就是编码漂了");
}

// --------------------------------------------------------------------------
// CSV 的形状
// --------------------------------------------------------------------------

/// 04_csv_to_chart.yaml 的脚本化模型写死了 x="month", y="amount"。
/// 列名一变，脚本化彩排和真模型演示就不是同一副牌了。
#[test]
fn header_matches_the_eval_scenario() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sales.csv");
    cli(&["csv", "-o", out.to_str().unwrap()]);
    let text = std::fs::read_to_string(&out).unwrap();
    assert_eq!(text.lines().next().unwrap(), "month,amount");
    assert_eq!(fx::COLUMNS, ["month", "amount"]);
}

#[test]
fn row_count_follows_months() {
    let dir = tempfile::tempdir().unwrap();
    for months in [1u32, 12, 24, 36] {
        let out = dir.path().join(format!("{months}.csv"));
        cli(&[
            "csv",
            "-o",
            out.to_str().unwrap(),
            "--months",
            &months.to_string(),
        ]);
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(text.lines().count(), months as usize + 1, "{months}"); // 表头 + 数据行
    }
}

/// 派单要求「几十行」：飞书里预览得下、模型也读得完。
#[test]
fn default_size_is_previewable_in_feishu() {
    assert!((12..=60).contains(&fx::DEFAULT_MONTHS));
}

/// 月份连续且可排序、金额是正整数。
#[test]
fn every_row_is_what_pandas_will_need() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sales.csv");
    cli(&["csv", "-o", out.to_str().unwrap(), "--months", "24"]);
    let text = std::fs::read_to_string(&out).unwrap();
    let rows: Vec<Vec<&str>> = text
        .lines()
        .skip(1)
        .map(|l| l.split(',').collect())
        .collect();

    let months: Vec<&str> = rows.iter().map(|r| r[0]).collect();
    let mut sorted = months.clone();
    sorted.sort_unstable();
    assert_eq!(months, sorted, "字典序 == 时间序，YYYY-MM 才有这性质");
    let unique: std::collections::BTreeSet<&&str> = months.iter().collect();
    assert_eq!(unique.len(), months.len());
    assert_eq!(months[0], "2024-09");
    assert_eq!(*months.last().unwrap(), "2026-08");

    for r in &rows {
        let amount: i64 = r[1].parse().expect("不是整数这里就炸");
        assert!(amount > 0);
    }
}

/// 全是直线的图演示效果差。用极差比均值粗略卡一道。
#[test]
fn the_line_is_not_flat() {
    let rows = fx::build_rows(default_start(), 24, fx::DEFAULT_SEED);
    let values: Vec<i64> = rows.iter().map(|r| r.1).collect();
    let mean = values.iter().sum::<i64>() as f64 / values.len() as f64;
    let span = (values.iter().max().unwrap() - values.iter().min().unwrap()) as f64;
    assert!(span / mean > 0.5, "{}", span / mean);
}

/// 第二幕要当场核对模型说的最高/最低月。并列或差之毫厘就核不成。
#[test]
fn peak_and_trough_are_unambiguous() {
    let mut rows = fx::build_rows(default_start(), fx::DEFAULT_MONTHS, fx::DEFAULT_SEED);
    rows.sort_by_key(|r| r.1);
    let (low, second_low) = (&rows[0], &rows[1]);
    let (top, second_top) = (&rows[rows.len() - 1], &rows[rows.len() - 2]);

    // 这两个月份就是剧本里写的那两个；改了 SEASON / 种子就等于改了演示的台词
    assert_eq!(top.0, "2025-11");
    assert_eq!(low.0, "2025-07");
    // 亚军拉开 3% 以上，肉眼和读数都不会犹豫
    assert!(top.1 as f64 > second_top.1 as f64 * 1.03);
    assert!((low.1 as f64) < second_low.1 as f64 / 1.03);
}

/// 与 Python 版逐字节一致：MT19937 + genrand_res53 + 银行家舍入都对上了才有这几个数。
#[test]
fn the_first_rows_match_the_python_script_byte_for_byte() {
    let rows = fx::build_rows(default_start(), fx::DEFAULT_MONTHS, fx::DEFAULT_SEED);
    let text = fx::render_csv(&rows);
    // 取自 `python3 scripts/demo_fixture.py csv` 的实际产出（两边 cmp 逐字节一致）
    let head: Vec<&str> = text.lines().take(4).collect();
    assert_eq!(
        head,
        [
            "month,amount",
            "2024-09,91287",
            "2024-10,92687",
            "2024-11,110975"
        ]
    );
    assert_eq!(rows.iter().map(|r| r.1).sum::<i64>(), 2_491_172);
}

// --------------------------------------------------------------------------
// 群历史垫场文本
// --------------------------------------------------------------------------

#[test]
fn history_is_byte_reproducible() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
    cli(&["history", "-o", a.to_str().unwrap()]);
    cli(&["history", "-o", b.to_str().unwrap()]);
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

/// 垫场消息里出现 @ = 每条都触发一个任务，群里瞬间多出一堆卡片。
///
/// 只管正文：文件头部那句「一条都不要 @Aite」的告警本身当然带 @，
/// 而真正会被发进群的，只有渲染出来的缩进行。
#[test]
fn history_bodies_have_no_at_mention() {
    assert!(!fx::HISTORY_POOL.iter().any(|(_, body)| body.contains('@')));
    let rendered = fx::render_history(fx::HISTORY_POOL.len());
    let bodies: Vec<&str> = rendered.lines().filter(|l| l.starts_with("    ")).collect();
    assert_eq!(bodies.len(), fx::HISTORY_POOL.len());
    assert!(!bodies.iter().any(|l| l.contains('@')));
}

/// 一条已办完、一条纯闲聊。汇总里不该出现它俩 —— 这是加演那段的看点。
#[test]
fn history_keeps_the_two_distractors() {
    assert!(fx::HISTORY_POOL[3].1.contains("已经处理完了"));
    assert!(fx::HISTORY_POOL[6].1.contains("中午一起吃饭"));
}

#[test]
fn history_count_slices_from_the_front() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("h.txt");
    cli(&["history", "-o", out.to_str().unwrap(), "--count", "3"]);
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains(fx::HISTORY_POOL[2].1));
    assert!(!text.contains(fx::HISTORY_POOL[3].1));
}

// --------------------------------------------------------------------------
// 命令行
// --------------------------------------------------------------------------

#[test]
fn all_writes_both_files() {
    let dir = tempfile::tempdir().unwrap();
    let out = cli(&["all", "-d", dir.path().to_str().unwrap()]);
    assert_eq!(out.code, 0);
    let csv_path = dir.path().join(fx::CSV_NAME);
    let his_path = dir.path().join(fx::HISTORY_NAME);
    assert!(csv_path.exists() && his_path.exists());

    let text = out.stdout_text();
    assert!(text.contains(csv_path.to_str().unwrap()));
    assert!(text.contains(his_path.to_str().unwrap()));
    // 演示时要拿这两行去核对模型，不打出来这子命令就白写了
    assert!(text.contains("最高") && text.contains("最低"));
}

#[test]
fn it_creates_missing_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("nested").join("deeper").join("sales.csv");
    assert_eq!(cli(&["csv", "-o", out.to_str().unwrap()]).code, 0);
    assert!(out.exists());
}

#[test]
fn rejects_out_of_range_months() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.csv");
    for bad in ["0", "121"] {
        let r = cli(&["csv", "-o", out.to_str().unwrap(), "--months", bad]);
        assert_eq!(r.code, 2, "{bad}");
        assert!(r.stderr_text().contains("--months"), "{}", r.stderr_text());
    }
}

#[test]
fn rejects_out_of_range_count() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.txt");
    let r = cli(&["history", "-o", out.to_str().unwrap(), "--count", "99"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr_text().contains("--count"), "{}", r.stderr_text());
}

#[test]
fn rejects_malformed_start() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.csv");
    for bad in ["2024/09", "2024-13", "九月"] {
        let r = cli(&["csv", "-o", out.to_str().unwrap(), "--start", bad]);
        assert_eq!(r.code, 2, "{bad}");
    }
}

/// 默认落 `/tmp/aite-demo/`，不往 `data/` 写。
#[test]
fn default_output_dir_is_not_the_runtime_data_dir() {
    assert_eq!(fx::DEFAULT_DIR, "/tmp/aite-demo");
    assert!(!fx::DEFAULT_DIR.contains("data"));
}

#[test]
fn rejects_unknown_subcommands_and_flags() {
    assert_eq!(cli(&["nope"]).code, 2);
    assert_eq!(cli(&[]).code, 2);
    assert_eq!(cli(&["csv", "--nope", "x"]).code, 2);
    assert_eq!(cli(&["csv", "-o"]).code, 2); // 缺取值
}
