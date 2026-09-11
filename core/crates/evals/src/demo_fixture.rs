//! 演示素材生成器（对应旧 `scripts/demo_fixture.py`）：`aite evals demo-fixture`。
//!
//! 演示要用一个 CSV。手捏一个每次都不一样的文件，等于每次演示都在赌：数据长什么样、
//! 最高最低落在哪个月、画出来好不好看，全靠临场。这个子命令把这件事钉死：
//!
//! * **同样参数跑两次，字节一致**。彩排时看到的那张图，和正式演示时那张，是同一张。
//! * 数据带年度季节性 + 增长趋势 + 一处刻意的塌陷，**最高最低都不含糊**。收尾会把这
//!   两个月份打出来 —— 演示时当场拿它核对模型说得对不对。
//! * 只有两列 `month,amount`，和 `evals/p0/04_csv_to_chart.yaml` 里脚本化模型那句
//!   `df.plot(x="month", y="amount")` 是同一副牌。
//!
//! 默认落到 `/tmp/aite-demo/`，**不往 `data/` 里写**：`data/` 是运行时目录，演示前经常
//! 要清空，素材放那儿会被一起清掉。
//!
//! 抖动用的是 **CPython `random.Random` 那一套**（MT19937 + `genrand_res53`），所以
//! 同样的 `--seed` 在 Rust 与 Python 两边**逐字节一样**：旧脚本产出的 sales.csv 与这里
//! 产出的能直接 `cmp`。没引第三方 rng —— workspace 钉的 `rand` 不是梅森旋转，对不上。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 默认输出目录。刻意不是 data/：演示前的检查清单会把 data/ 清空。
pub const DEFAULT_DIR: &str = "/tmp/aite-demo";
pub const CSV_NAME: &str = "sales.csv";
pub const HISTORY_NAME: &str = "history.txt";

/// 列名对着 04_csv_to_chart.yaml 里那句 `df.plot(x="month", y="amount")`。
/// 改这里等于让脚本化彩排和真模型演示看的不是同一副牌，别改。
pub const COLUMNS: [&str; 2] = ["month", "amount"];

pub const DEFAULT_START: &str = "2024-09";
pub const DEFAULT_MONTHS: u32 = 24; // 两年，飞书里预览得下，模型也读得完
pub const DEFAULT_SEED: u64 = 20260910;

const BASE_AMOUNT: f64 = 86_000.0; // 元，第 0 个月的基准盘子
const MONTHLY_GROWTH: f64 = 0.018; // 每月线性增长，24 个月累计 x1.41
const JITTER: f64 = 0.04; // ±4% 的抖动，让折线不至于太画出来的

/// 刻意留一处塌陷（第 10 个月，默认参数下是 2025-07）。
/// 有它，「最低是哪个月」才有唯一答案；没它，最低点会落在春节，和相邻月份拉不开差距。
const ANOMALY_INDEX: usize = 10;
const ANOMALY_FACTOR: f64 = 0.62;

/// 自然月 → 季节系数。用查表不用正弦：正弦画出来对称得不像真数据，
/// 而双十一那根尖峰和春节那个坑正是这张图好看的原因。
fn season(month: u32) -> f64 {
    match month {
        1 => 0.88, // 元旦后淡季
        2 => 0.72, // 春节，全年最低的自然低点
        3 => 0.98,
        4 => 1.00,
        5 => 1.06, // 五一
        6 => 1.12, // 618
        7 => 0.95,
        8 => 0.97,
        9 => 1.03,  // 开学季
        10 => 1.08, // 国庆
        11 => 1.28, // 双十一，全年最高的自然高点
        _ => 1.15,  // 12 月年末冲量
    }
}

// --------------------------------------------------------------------------
// CPython 的 random.Random（MT19937）
// --------------------------------------------------------------------------

/// 与 CPython `random.Random(seed)` 逐位一致的梅森旋转。
///
/// 只实现用得到的两件事：整数种子的 `init_by_array` 播种，和 `random()` 的
/// `genrand_res53`（两次 32 位取高 27 + 26 位拼成 53 位尾数）。有了它，
/// `uniform(a, b) = a + (b - a) * random()` 与 Python 得到同一串浮点数。
struct MersenneTwister {
    state: [u32; 624],
    index: usize,
}

impl MersenneTwister {
    fn seeded(seed: u64) -> Self {
        let mut mt = Self {
            state: [0; 624],
            index: 624,
        };
        mt.init_genrand(19_650_218);
        // CPython 把整数种子按 32 位小端拆成 key 数组（0 也算一格）
        let mut key: Vec<u32> = Vec::new();
        let mut n = seed;
        while n > 0 {
            key.push((n & 0xFFFF_FFFF) as u32);
            n >>= 32;
        }
        if key.is_empty() {
            key.push(0);
        }
        mt.init_by_array(&key);
        mt
    }

    fn init_genrand(&mut self, s: u32) {
        self.state[0] = s;
        for i in 1..624 {
            let prev = self.state[i - 1];
            self.state[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.index = 624;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        let (mut i, mut j) = (1usize, 0usize);
        let mut k = 624.max(key.len());
        while k > 0 {
            let prev = self.state[i - 1];
            self.state[i] = (self.state[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                self.state[0] = self.state[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        let mut k = 623;
        while k > 0 {
            let prev = self.state[i - 1];
            self.state[i] = (self.state[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                self.state[0] = self.state[623];
                i = 1;
            }
            k -= 1;
        }
        self.state[0] = 0x8000_0000; // MSB 置 1，保证初始数组非零
    }

    fn generate(&mut self) {
        const MATRIX_A: u32 = 0x9908_b0df;
        const UPPER: u32 = 0x8000_0000;
        const LOWER: u32 = 0x7fff_ffff;
        for i in 0..624 {
            let y = (self.state[i] & UPPER) | (self.state[(i + 1) % 624] & LOWER);
            let mut next = self.state[(i + 397) % 624] ^ (y >> 1);
            if y & 1 != 0 {
                next ^= MATRIX_A;
            }
            self.state[i] = next;
        }
        self.index = 0;
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            self.generate();
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// CPython 的 `genrand_res53`。
    fn next_f64(&mut self) -> f64 {
        let a = f64::from(self.next_u32() >> 5);
        let b = f64::from(self.next_u32() >> 6);
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// CPython 的 `random.uniform(a, b)`。
    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next_f64()
    }
}

// --------------------------------------------------------------------------
// 数据
// --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FixtureError(pub String);

/// `2024-09` → (2024, 9)。
pub fn parse_start(text: &str) -> Result<(i32, u32), FixtureError> {
    let bad = || FixtureError(format!("起始月要写成 YYYY-MM，收到 {text:?}"));
    let (y, m) = text.split_once('-').ok_or_else(bad)?;
    let year: i32 = y.parse().map_err(|_| bad())?;
    let month: u32 = m.parse().map_err(|_| bad())?;
    if !(1..=12).contains(&month) {
        return Err(FixtureError(format!("月份要在 1–12 之间，收到 {text:?}")));
    }
    if !(1970..=2999).contains(&year) {
        return Err(FixtureError(format!("年份不像话，收到 {text:?}")));
    }
    Ok((year, month))
}

/// 从 start 起连续 months 个月的 `YYYY-MM` 标签。
pub fn month_labels(start: (i32, u32), months: u32) -> Vec<String> {
    let (mut year, mut month) = start;
    let mut out = Vec::with_capacity(months as usize);
    for _ in 0..months {
        out.push(format!("{year:04}-{month:02}"));
        month += 1;
        if month == 13 {
            year += 1;
            month = 1;
        }
    }
    out
}

/// 生成 (month, amount) 行。同样参数必然得到同样结果。
///
/// 抖动用 `Random(seed).uniform()`（梅森旋转，CPython 跨版本稳定），最后一律
/// 四舍五入成整数 —— 不留浮点，就不会有 repr 差异带来的字节漂移。
pub fn build_rows(start: (i32, u32), months: u32, seed: u64) -> Vec<(String, i64)> {
    let mut rng = MersenneTwister::seeded(seed);
    month_labels(start, months)
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let calendar_month: u32 = label[5..].parse().unwrap_or(1);
            let mut value = BASE_AMOUNT;
            value *= 1.0 + MONTHLY_GROWTH * index as f64;
            value *= season(calendar_month);
            value *= 1.0 + rng.uniform(-JITTER, JITTER);
            if index == ANOMALY_INDEX {
                value *= ANOMALY_FACTOR;
            }
            (label, python_round(value))
        })
        .collect()
}

/// Python 的 `round()`：银行家舍入（.5 取偶）。Rust 的 `f64::round` 是四舍五入到远离零，
/// 两者在恰好 .5 上不同 —— 为了跟旧脚本逐字节一致，这里照 Python 的来。
fn python_round(v: f64) -> i64 {
    let floor = v.floor();
    let frac = v - floor;
    if (frac - 0.5).abs() < f64::EPSILON {
        let f = floor as i64;
        if f % 2 == 0 { f } else { f + 1 }
    } else if frac > 0.5 {
        floor as i64 + 1
    } else {
        floor as i64
    }
}

/// 行 → CSV 文本。行尾钉成 LF，不跟宿主平台走。
pub fn render_csv(rows: &[(String, i64)]) -> String {
    let mut out = String::new();
    out.push_str(&COLUMNS.join(","));
    out.push('\n');
    for (month, amount) in rows {
        out.push_str(&format!("{month},{amount}\n"));
    }
    out
}

/// 演示时用来核对模型的那几行 —— 它说的最高最低，得和这里对得上。
pub fn summarize(rows: &[(String, i64)]) -> Vec<String> {
    let top = rows.iter().max_by_key(|r| r.1).expect("至少一行");
    let low = rows.iter().min_by_key(|r| r.1).expect("至少一行");
    let total: i64 = rows.iter().map(|r| r.1).sum();
    vec![
        format!(
            "  区间   {} … {}（{} 个月）",
            rows[0].0,
            rows[rows.len() - 1].0,
            rows.len()
        ),
        format!("  最高   {}  {} 元", top.0, thousands(top.1)),
        format!("  最低   {}  {} 元", low.0, thousands(low.1)),
        format!("  合计   {} 元", thousands(total)),
    ]
}

fn thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

// --------------------------------------------------------------------------
// 群历史垫场文本
// --------------------------------------------------------------------------

/// (发送人, 消息正文)。前两条之外故意混了「已经办完的」和「纯闲聊」，
/// 汇总时它俩不该出现在开放事项里 —— 这是加演那段真正想让人看见的东西。
pub const HISTORY_POOL: [(&str, &str); 8] = [
    (
        "Alice（产品）",
        "对账脚本这周能上线吗？月底前要给财务一个说法",
    ),
    ("Bob（设计）", "我这边等设计稿，周四之前给你们"),
    ("Carol（法务）", "客户合同还差法务盖章，我今天催一下"),
    ("Dave（运维）", "上周那台机器的告警已经处理完了，可以关掉了"),
    ("Alice（产品）", "618 复盘的数据谁来出？下周一评审要用"),
    ("Erin（财务）", "报销单我提交了，还等审批"),
    ("Bob（设计）", "中午一起吃饭吗"),
    ("Frank（销售）", "华东那个客户要一版报价，最晚周五给"),
];

/// 与 Python 版 `scripts/demo_fixture.py` 的 `HISTORY_HEADER` 逐字节一致
/// （`cmp` 对得上是这个子命令的验收面之一）。
const HISTORY_HEADER: &str = "\
# Aite 演示 · 群历史垫场文本
#
# 用途：演示的加演那一段（「汇总本群本周开放事项」）要有真实群消息才有东西可汇总。
#      空群里问这句，模型只能编 —— 那正是演示最不该出现的画面。
#
# 用法：演示开始前 5 分钟，把每条**缩进的那一行**原样发进测试群，一行一条。
#      有替身账号 → 按「发送人」分开发，汇总里能出现不同名字，效果更好；
#      只有一个账号 → 全用自己发也成立（拉历史只过滤非真人消息，不看是谁）。
#
# 三条注意：
#   1. 这些消息**一条都不要 @Aite** —— @ 一下就是一个任务，群里会多出一堆卡片。
#   2. 发在群里，不要发在话题（线程）里 —— 拉的是群历史。
#   3. 第 4 条和第 7 条是干扰项（一条已办完、一条纯闲聊）。汇总里**不该**出现它俩，
#      这两条不出现，才说明它是在筛选而不是在复述。
";

pub fn render_history(count: usize) -> String {
    let mut parts = vec![HISTORY_HEADER.to_string()];
    for (i, (who, text)) in HISTORY_POOL.iter().take(count).enumerate() {
        parts.push(format!("{:2}. {who}\n    {text}\n", i + 1));
    }
    format!("{}\n", parts.join("\n").trim_end_matches('\n'))
}

// --------------------------------------------------------------------------
// 落盘与命令行
// --------------------------------------------------------------------------

pub fn write_text(path: &Path, text: &str) -> Result<(), FixtureError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| FixtureError(format!("建不出目录 {}：{e}", parent.display())))?;
    }
    std::fs::write(path, text).map_err(|e| FixtureError(format!("写不进 {}：{e}", path.display())))
}

#[derive(Debug, Clone)]
pub struct CsvOptions {
    pub start: (i32, u32),
    pub months: u32,
    pub seed: u64,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            start: parse_start(DEFAULT_START).expect("常量"),
            months: DEFAULT_MONTHS,
            seed: DEFAULT_SEED,
        }
    }
}

fn check_months(months: u32) -> Result<(), FixtureError> {
    if !(1..=120).contains(&months) {
        return Err(FixtureError(format!(
            "--months 要在 1–120 之间，收到 {months}"
        )));
    }
    Ok(())
}

fn check_count(count: usize) -> Result<(), FixtureError> {
    if count < 1 || count > HISTORY_POOL.len() {
        return Err(FixtureError(format!(
            "--count 要在 1–{} 之间，收到 {count}",
            HISTORY_POOL.len()
        )));
    }
    Ok(())
}

fn emit_csv(path: &Path, opts: &CsvOptions, out: &mut Vec<String>) -> Result<(), FixtureError> {
    check_months(opts.months)?;
    let rows = build_rows(opts.start, opts.months, opts.seed);
    write_text(path, &render_csv(&rows))?;
    out.push(format!("CSV      {}", path.display()));
    out.extend(summarize(&rows));
    Ok(())
}

fn emit_history(path: &Path, count: usize, out: &mut Vec<String>) -> Result<(), FixtureError> {
    check_count(count)?;
    write_text(path, &render_history(count))?;
    out.push(format!(
        "群历史   {}（{count} 条，其中 2 条是干扰项）",
        path.display()
    ));
    Ok(())
}

/// `aite evals demo-fixture csv|history|all …`。返回打给 stdout 的行。
pub fn run(args: &[String]) -> Result<Vec<String>, FixtureError> {
    let mut flags: BTreeMap<String, String> = BTreeMap::new();
    let sub = args.first().cloned().ok_or_else(|| {
        FixtureError("用法：aite evals demo-fixture {csv,history,all} […]".into())
    })?;
    if !["csv", "history", "all"].contains(&sub.as_str()) {
        return Err(FixtureError(format!(
            "不认识的子命令 {sub:?}，只认 csv / history / all"
        )));
    }

    let mut i = 1;
    while i < args.len() {
        let key = args[i].clone();
        let name = match key.as_str() {
            "-o" | "--out" => "out",
            "-d" | "--dir" => "dir",
            "--start" => "start",
            "--months" => "months",
            "--seed" => "seed",
            "--count" => "count",
            other => {
                return Err(FixtureError(format!("不认识的参数 {other:?}")));
            }
        };
        let value = args
            .get(i + 1)
            .cloned()
            .ok_or_else(|| FixtureError(format!("{key} 后面缺一个取值")))?;
        flags.insert(name.to_string(), value);
        i += 2;
    }

    let mut opts = CsvOptions::default();
    if let Some(v) = flags.get("start") {
        opts.start = parse_start(v)?;
    }
    if let Some(v) = flags.get("months") {
        opts.months = v
            .parse()
            .map_err(|_| FixtureError(format!("--months 要是整数，收到 {v:?}")))?;
    }
    if let Some(v) = flags.get("seed") {
        opts.seed = v
            .parse()
            .map_err(|_| FixtureError(format!("--seed 要是非负整数，收到 {v:?}")))?;
    }
    let count = match flags.get("count") {
        None => HISTORY_POOL.len(),
        Some(v) => v
            .parse()
            .map_err(|_| FixtureError(format!("--count 要是整数，收到 {v:?}")))?,
    };

    let mut out = Vec::new();
    match sub.as_str() {
        "csv" => {
            let path = flags
                .get("out")
                .map(PathBuf::from)
                .unwrap_or_else(|| Path::new(DEFAULT_DIR).join(CSV_NAME));
            emit_csv(&path, &opts, &mut out)?;
        }
        "history" => {
            let path = flags
                .get("out")
                .map(PathBuf::from)
                .unwrap_or_else(|| Path::new(DEFAULT_DIR).join(HISTORY_NAME));
            emit_history(&path, count, &mut out)?;
        }
        _ => {
            let dir = flags
                .get("dir")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_DIR));
            emit_csv(&dir.join(CSV_NAME), &opts, &mut out)?;
            emit_history(&dir.join(HISTORY_NAME), count, &mut out)?;
        }
    }
    out.push(String::new());
    out.push(
        "下一步：把 CSV 拖进飞书测试群那条 @ 消息里；垫场文本按里面的说明发。剧本见 docs/demo-3min.md。"
            .to_string(),
    );
    Ok(out)
}
