//! 评测 runner 的命令行入口（对应旧 `aite/evals/__main__.py`）。
//!
//! ```text
//! aite evals run evals/p0 --platform fake --model scripted
//! aite evals run evals/p0 --list
//! aite evals demo-fixture all -d /tmp/aite-demo
//! ```
//!
//! `run` 跑完打印 JSON 摘要，**最后一行是 `passed k/n`**。退出码：全过 0，有失败 1，
//! 参数/环境问题（场景加载失败、未知场景、live 起不来、docker 体检不过）2。
//!
//! RΩ 接线之前 k 会是 0 —— 那时每个场景报的是「ControlPlane 未接线（RΩ）」这类人话
//! 原因，不是 panic。RΩ 阶段才要求 `passed 10/10` 且退出码 0（B8）。
//!
//! `--protocol-report` 是模型实测用的眼睛：摘要写 stderr、完整 JSON 写文件，
//! **stdout 那份 JSON 摘要不开这个开关时一个字段都不多**。
//!
//! `--timeout-scale` 把场景的两个等待上限一起放大：yaml 里那些秒数是照替身的尺度定的，
//! 真模型撑不下。`--model live` 默认就按 `LIVE_TIMEOUT_SCALE` 放大，不用每次手写。
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::demo_fixture;
use crate::deps::{DepsOptions, ModelFactory, SANDBOXES, SandboxFactory};
use crate::protocol_probe::render_digest;
use crate::real_stack::{DockerProbe, docker_preflight, scenarios_with_exec_script};
use crate::runner::{PlaneFactory, RunOptions, SuiteResult, run_suite};
use crate::scenario::{Scenario, load_suite};

pub const PLATFORMS: [&str; 1] = ["fake"];
pub const MODELS: [&str; 2] = ["scripted", "live"];

/// `--model live` 时 `--timeout-scale` 的默认值。
///
/// 场景 yaml 里的 `timeout_sec: 10` 和 `after_timeout_sec: 5` 是照替身的尺度定的 ——
/// 脚本化模型瞬时返回，十秒够跑几十步。真模型一次调用就 2–20s，10s 连一步都不一定
/// 收得回来。这个倍数把两个上限一起放大，**不改任何一份场景文件**。
pub const LIVE_TIMEOUT_SCALE: f64 = 12.0;

/// RΩ 组装时要注入的四个工厂。全 None = 并行期的样子：场景跑得完、报人话原因。
#[derive(Clone, Default)]
pub struct Wiring {
    /// 真 ControlPlane（R4 + R5，RΩ 组装）
    pub plane: Option<PlaneFactory>,
    /// `--model live` 的真模型客户端（R5 的 aite-models，RΩ 接）
    pub model: Option<ModelFactory>,
    /// `--sandbox docker` 的真沙箱 + 真 Gateway（R2/R6，RΩ 接）
    pub sandbox: Option<SandboxFactory>,
    /// `--sandbox docker` 的起飞前体检（连 daemon、查镜像）。
    /// 与 `sandbox` 成对注入：只给工厂不给体检的话，daemon 没起时十个场景会各自
    /// 烂在第一个工具调用上，报出来的是十条不着边际的 sandbox 错误。
    pub preflight: Option<DockerProbe>,
}

#[derive(Debug, Default)]
struct Args {
    suite: String,
    platform: String,
    model: String,
    sandbox: String,
    list: bool,
    only: Vec<String>,
    json_out: Option<PathBuf>,
    traceback: bool,
    config: String,
    protocol_report: Option<String>,
    timeout_scale: Option<f64>,
}

const USAGE: &str = "\
用法：
  aite evals run <suite> [--platform fake] [--model scripted|live] [--sandbox fake|docker]
                         [--list] [--only NAME]… [--json PATH] [--traceback]
                         [--config PATH] [--protocol-report [PATH]] [--timeout-scale K]
  aite evals demo-fixture {csv,history,all} [-o PATH | -d DIR] [--start YYYY-MM]
                         [--months N] [--seed N] [--count N]";

/// 参数没解析成的两种收场。分开是因为**要人看用法**和**参数写错了**在命令行上是两件事：
/// 前者是 stdout + 退出码 0（argparse 的 `-h` 就是这样，脚本里 `set -e` 不会被它带死），
/// 后者是 stderr + 退出码 2。原来两者都走后者（审核记账 R7）。
enum ParseOutcome {
    /// `-h` / `--help`
    Help,
    Bad(String),
}

impl From<String> for ParseOutcome {
    fn from(e: String) -> Self {
        ParseOutcome::Bad(e)
    }
}

fn parse_args(argv: &[String]) -> Result<Args, ParseOutcome> {
    let mut args = Args {
        platform: "fake".to_string(),
        model: "scripted".to_string(),
        sandbox: "fake".to_string(),
        config: "config/aite.yaml".to_string(),
        ..Args::default()
    };
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        let value = |i: &mut usize, flag: &str| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("{flag} 后面缺一个取值"))
        };
        match a {
            "--platform" => args.platform = value(&mut i, "--platform")?,
            "--model" => args.model = value(&mut i, "--model")?,
            "--sandbox" => args.sandbox = value(&mut i, "--sandbox")?,
            "--list" => args.list = true,
            "--only" => args.only.push(value(&mut i, "--only")?),
            "--json" => args.json_out = Some(PathBuf::from(value(&mut i, "--json")?)),
            "--traceback" => args.traceback = true,
            "--config" => args.config = value(&mut i, "--config")?,
            "--timeout-scale" => {
                let raw = value(&mut i, "--timeout-scale")?;
                args.timeout_scale = Some(
                    raw.parse()
                        .map_err(|_| format!("--timeout-scale 要是数字，收到 {raw:?}"))?,
                );
            }
            "--protocol-report" => {
                // nargs="?"：后面跟着的如果是另一个开关，就当没给 PATH
                let next = argv.get(i + 1);
                match next {
                    Some(p) if !p.starts_with("--") => {
                        i += 1;
                        args.protocol_report = Some(p.clone());
                    }
                    _ => args.protocol_report = Some(String::new()),
                }
            }
            "-h" | "--help" => return Err(ParseOutcome::Help),
            other if other.starts_with('-') => {
                return Err(format!("不认识的参数 {other:?}").into());
            }
            other => {
                if args.suite.is_empty() {
                    args.suite = other.to_string();
                } else {
                    return Err(format!("多余的位置参数 {other:?}").into());
                }
            }
        }
        i += 1;
    }
    if args.suite.is_empty() {
        return Err("缺场景目录，例如 aite evals run evals/p0"
            .to_string()
            .into());
    }
    if !PLATFORMS.contains(&args.platform.as_str()) {
        return Err(format!(
            "不认识的 --platform {:?}，只认 {PLATFORMS:?}",
            args.platform
        )
        .into());
    }
    if !MODELS.contains(&args.model.as_str()) {
        return Err(format!("不认识的 --model {:?}，只认 {MODELS:?}", args.model).into());
    }
    if !SANDBOXES.contains(&args.sandbox.as_str()) {
        return Err(format!("不认识的 --sandbox {:?}，只认 {SANDBOXES:?}", args.sandbox).into());
    }
    Ok(args)
}

/// 一次 CLI 调用的全部产出。真实路径与测试路径共用同一段代码：`run_with_wiring`
/// 只是把它打到 stdout / stderr 上，测试则直接读它 —— 输出形状（尤其
/// 「最后一行 passed k/n」「scripted 路径 stderr 为空」）因此是被真测到的。
#[derive(Debug, Default, Clone)]
pub struct Captured {
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
    pub code: i32,
}

impl Captured {
    pub fn stdout_text(&self) -> String {
        self.stdout.join("\n")
    }
    pub fn stderr_text(&self) -> String {
        self.stderr.join("\n")
    }
    /// stdout 去掉最后一行 `passed k/n` 之后的那份 JSON。
    pub fn payload(&self) -> Option<Value> {
        let body = self.stdout.split_last()?.1.join("\n");
        serde_json::from_str(&body).ok()
    }
    pub fn last_line(&self) -> &str {
        self.stdout.last().map(String::as_str).unwrap_or_default()
    }
}

/// 子命令入口：收到的是 `aite evals` 之后的全部参数。返回进程退出码。
pub fn run(args: Vec<String>) -> i32 {
    run_with_wiring(args, &Wiring::default())
}

/// RΩ 用的入口：把真 plane / 真模型 / 真沙箱的工厂注进来。
pub fn run_with_wiring(args: Vec<String>, wiring: &Wiring) -> i32 {
    let out = run_capture(args, wiring);
    for line in &out.stdout {
        println!("{line}");
    }
    for line in &out.stderr {
        eprintln!("{line}");
    }
    out.code
}

/// 同 `run_with_wiring`，但把输出收进结构里而不是直接打出去。
pub fn run_capture(args: Vec<String>, wiring: &Wiring) -> Captured {
    let Some((cmd, rest)) = args.split_first() else {
        return Captured {
            stderr: vec![USAGE.to_string()],
            code: 2,
            ..Captured::default()
        };
    };
    match cmd.as_str() {
        "run" => run_suite_cmd(rest, wiring),
        // `-h` / `--help`：stdout + 退出码 0，跟 `run` 一个口径。不接这一条的话它会掉进
        // `demo_fixture::run` 的「不认识的子命令 "--help"」→ stderr + 退出码 2 ——
        // 那是 RΩ 那次只修了 `evals run` 一半留下的账（审核台账 §4.4）。
        // 用法就用上面那份 `USAGE`（里面已经有 demo-fixture 那两行），不写第二份。
        // 跟 argparse 一样「出现在哪都算」；`-o -h` 这种把 `-h` 当取值的写法会被当成求助，
        // 属于没有真实用途的边角，不为它加一层位置判定。
        "demo-fixture" if rest.iter().any(|a| a == "-h" || a == "--help") => Captured {
            stdout: vec![USAGE.to_string()],
            code: 0,
            ..Captured::default()
        },
        "demo-fixture" => match demo_fixture::run(rest) {
            Ok(stdout) => Captured {
                stdout,
                code: 0,
                ..Captured::default()
            },
            Err(e) => Captured {
                stderr: vec![e.0],
                code: 2,
                ..Captured::default()
            },
        },
        // 子命令位置上的 `-h` / `--help`：跟 `run` / `demo-fixture` 一个口径，
        // stdout + 退出码 0，不是「不认识的子命令」。
        //
        // **`aite evals --help`（不加 `--`）现在也到得了这里**（2026-09-13 起）：
        // `main.rs` 给四个 `trailing_var_arg` variant 加了
        // `#[command(disable_help_flag = true)]`，clap 不再截胡。两种写法打出来逐字节相同。
        "-h" | "--help" => Captured {
            stdout: vec![USAGE.to_string()],
            code: 0,
            ..Captured::default()
        },
        other => Captured {
            stderr: vec![format!("不认识的子命令 {other:?}"), USAGE.to_string()],
            code: 2,
            ..Captured::default()
        },
    }
}

/// 派单说的 `run_with_factory(args, Option<PlaneFactory>)`：只注入 plane 的简化入口。
pub fn run_with_factory(args: Vec<String>, plane: Option<PlaneFactory>) -> i32 {
    run_with_wiring(
        args,
        &Wiring {
            plane,
            ..Wiring::default()
        },
    )
}

fn fail(message: String) -> Captured {
    Captured {
        stderr: vec![message],
        code: 2,
        ..Captured::default()
    }
}

fn run_suite_cmd(argv: &[String], wiring: &Wiring) -> Captured {
    let mut out = Captured::default();
    let args = match parse_args(argv) {
        Ok(args) => args,
        // `-h` 是「人要看用法」，不是「参数写错了」：stdout + 退出码 0（同 argparse）。
        Err(ParseOutcome::Help) => {
            return Captured {
                stdout: vec![USAGE.to_string()],
                code: 0,
                ..Captured::default()
            };
        }
        Err(ParseOutcome::Bad(e)) => return fail(e),
    };

    let mut scenarios = match load_suite(Path::new(&args.suite)) {
        Ok(s) => s,
        Err(e) => return fail(format!("场景加载失败：{}", e.0)),
    };

    if !args.only.is_empty() {
        let known: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
        let mut unknown: Vec<&String> = args
            .only
            .iter()
            .filter(|n| !known.contains(&n.as_str()))
            .collect();
        unknown.sort();
        if !unknown.is_empty() {
            return fail(format!("没有这些场景：{unknown:?}"));
        }
        scenarios.retain(|s| args.only.contains(&s.name));
    }

    if args.list {
        let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
        out.stdout
            .push(serde_json::to_string_pretty(&names).unwrap_or_default());
        return out;
    }

    if args.sandbox == "docker" {
        // 起飞前查一次：不查的话十个场景各自烂在第一个工具调用上，真正的原因一个字都看不到
        let images: Vec<String> = scenarios
            .iter()
            .map(|sc| {
                crate::deps::config_of(sc)
                    .map(|c| c.sandbox.image)
                    .unwrap_or_else(|_| "aite-sandbox:p0".to_string())
            })
            .collect();
        if wiring.sandbox.is_some() && wiring.preflight.is_none() {
            return fail(
                "--sandbox docker 接了 SandboxFactory 却没接 preflight：                 daemon 没起或镜像没 build 时，十个场景会各自烂在第一个工具调用上。                 起飞前体检要和工厂成对注入（Wiring::preflight）。"
                    .to_string(),
            );
        }
        if let Some(why) = docker_preflight(&images, wiring.preflight.as_ref()) {
            return fail(format!("--sandbox docker 起不来：{why}"));
        }
        let named = scenarios_with_exec_script(&scenarios);
        if !named.is_empty() {
            out.stderr.push(format!(
                "--sandbox docker：这些场景的 sandbox.exec_script 不生效（代码交给真容器跑）：{}",
                named.join("、")
            ));
        }
    }

    let scale = args.timeout_scale.unwrap_or({
        if args.model == "live" {
            LIVE_TIMEOUT_SCALE
        } else {
            1.0
        }
    });
    if scale != 1.0 {
        scenarios = scenarios.iter().map(|s| s.scaled(scale)).collect();
        out.stderr
            .push(format!("场景等待上限 x{scale}（--timeout-scale）"));
    }

    let mut model_factory: Option<ModelFactory> = None;
    if args.model == "live" {
        match &wiring.model {
            Some(f) => {
                // **起飞前试造一次，结果丢掉。** 位置对齐 Python `__main__.py` 的
                // `_live_model_factory`：排在 parse / load_suite / `--only` / `--list` /
                // docker 体检 / scale **之后** —— 前面那些一件都用不着模型，不该被它挡住
                // （接线方那侧是惰性的，见 `app::wiring` 的模块头）。
                //
                // 但也**不能不造**：不造的话配置缺一样就变成十个场景各自跑到第一次 chat
                // 才抛，被 worker 的 §3.3 当成模型 5xx 白重试 2 次（2s + 5s），十次 7 秒
                // 空等，而真正的原因（yaml 没填 / 环境变量没设）一个字都看不到。
                if let Err(e) = f() {
                    return fail(format!("--model live 起不来：{e}"));
                }
                model_factory = Some(f.clone());
            }
            None => {
                return fail(format!(
                    "--model live 起不来：真模型客户端（aite-models，R5）由 RΩ 通过 \
                     ModelFactory 注入后这一档才跑得动（配置：{}）",
                    args.config
                ));
            }
        }
    }

    let want_protocol = args.protocol_report.is_some();
    let options = RunOptions {
        deps: DepsOptions {
            sandbox_kind: args.sandbox.clone(),
            sandbox_factory: wiring.sandbox.clone(),
            model: None,
        },
        plane_factory: wiring.plane.clone(),
        collect_protocol: want_protocol,
    };

    // current_thread 运行时：与 Python 的单事件循环同构，`hold_ticks` 的让出语义
    // （`yield_now` 让别的任务插进来）才成立。
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return fail(format!("runner 起不来：{e}")),
    };
    let result: SuiteResult = runtime.block_on(run_suite(
        &scenarios,
        &args.suite,
        &args.platform,
        &args.model,
        &options,
        model_factory.as_ref(),
    ));

    emit(&mut out, &result, &args, want_protocol);
    out
}

fn emit(out: &mut Captured, result: &SuiteResult, args: &Args, want_protocol: bool) {
    let payload = result.to_json();
    let body = serde_json::to_string_pretty(&payload).unwrap_or_default();
    out.stdout.push(body.clone());
    if let Some(path) = &args.json_out
        && let Err(e) = std::fs::write(path, format!("{body}\n"))
    {
        out.stderr
            .push(format!("--json 写不出去（{}）：{e}", path.display()));
    }

    if want_protocol {
        let rows: Vec<(String, Map<String, Value>)> = result
            .results
            .iter()
            .map(|r| (r.name.clone(), r.protocol.clone()))
            .collect();
        // 摘要走 stderr：stdout 得保持「一份 JSON + 最后一行 passed k/n」
        out.stderr.push(render_digest(&rows));
        if let Some(path) = args.protocol_report.as_ref().filter(|p| !p.is_empty()) {
            let report = serde_json::json!({
                "suite": args.suite,
                "platform": args.platform,
                "model": args.model,
                // stdout 的 JSON 摘要不加这一格（默认路径逐字节不变），这份报告是 opt-in 的
                "sandbox": args.sandbox,
                "contract_version": payload["contract_version"],
                "scenarios": result.results.iter().map(|r| {
                    let mut row = Map::new();
                    row.insert("name".to_string(), Value::String(r.name.clone()));
                    for (k, v) in &r.protocol {
                        row.insert(k.clone(), v.clone());
                    }
                    Value::Object(row)
                }).collect::<Vec<_>>(),
            });
            let text = serde_json::to_string_pretty(&report).unwrap_or_default();
            if let Err(e) = std::fs::write(path, format!("{text}\n")) {
                out.stderr
                    .push(format!("--protocol-report 写不出去（{path}）：{e}"));
            }
        }
    }

    if args.traceback {
        for r in &result.results {
            if let Some(reason) = &r.reason {
                let mut block = format!("\n=== {} ===\nphase={} {reason}", r.name, r.phase);
                for f in &r.failures {
                    block.push_str(&format!("\n  - {f}"));
                }
                out.stderr.push(block);
            }
        }
    }

    out.stdout
        .push(format!("passed {}/{}", result.passed(), result.total()));
    out.code = if result.passed() == result.total() {
        0
    } else {
        1
    };
}

/// 给测试与 RΩ 用：按倍数放大场景的等待上限。
pub fn scale_timeouts(sc: &Scenario, k: f64) -> Scenario {
    sc.scaled(k)
}
