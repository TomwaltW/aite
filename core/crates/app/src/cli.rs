//! `aite run` 的命令行（对应旧 `aite/app.py` 的 `_main`）。
//!
//! 起飞前体检没过一律「一行人话 + 退出码 2」；`--traceback` 在那之后多打一行
//! 错误值的 `Debug` 形（**不是**错误链 —— `StartupError` 是个没有 `source()` 的 newtype，
//! 这条路径上压根没有链可打，见 [`startup_failed`]）。
use crate::app::{EXIT_STARTUP, Injections, StartupError, build_app, load_config};
use crate::run::{DEFAULT_SHUTDOWN_GRACE_SEC, ServeOptions, run_app};

const USAGE: &str = "\
用法：
  aite run [--config PATH] [--grace SEC] [--traceback]

  --config PATH   配置文件路径（默认 config/aite.yaml；样例见 config/aite.example.yaml）
  --grace SEC     优雅退出的宽限期，0–86400 秒，默认 20（对齐 compose 的 stop_grace_period）
  --traceback     起不来时在人话后面多打一行错误值的 Debug 形（默认只打一行人话）";

/// `--grace` 的上界（秒）。
///
/// 挡的是 `run.rs` 那句 `Duration::from_secs_f64(grace.max(0.0))`：`max(0.0)` 挡住了负数，
/// 挡不住上溢 —— `--grace 1e300` 一路穿过校验，到收尾那一刻 panic，把它后面的
/// `sandbox.close_all()` / `store.close()` 整段跳过。
///
/// 取一天：宽限期是「等在途任务收完」的上限，超过一天没有任何真实用途；而 86400 离
/// `Duration` 的上限（`u64::MAX` 秒 ≈ 1.8e19）还差 14 个数量级，f64 怎么舍入都推不过去。
///
/// **门口校验 + 里面兜底，两道都在**（2026-09-13 起）：这里挡住命令行那一路，
/// 而 `run.rs` 的 `grace_duration()` 走 `Duration::try_from_secs_f64`，造不出来就退到默认
/// 20s 并 warn `aite.grace_invalid` —— 所以 `ServeOptions::shutdown_grace_sec` 这个裸 `f64`
/// 被别的调用方直接塞成 1e300 / NaN 时也不再 panic，收尾序列一步都不会少。
/// （原注释写的是「这是把炸弹挡在门口，不是拆弹，真正的拆弹归 V5」—— W2 已经拆了。）
const MAX_GRACE_SEC: f64 = 86_400.0;

#[derive(Debug)]
struct Args {
    config: String,
    grace: f64,
    traceback: bool,
}

/// 参数没解析成的两种收场。分开是因为**要人看用法**和**参数写错了**在命令行上是两件事：
/// 前者是 stdout + 退出码 0（argparse 的 `-h` 就是这样，脚本里 `set -e` 不会被它带死），
/// 后者是 stderr + 退出码 2。口径与 `aite evals run` 的 `ParseOutcome` 逐字对齐。
///
/// **`aite run --help` 现在够得着这里**（2026-09-13 起）：`main.rs` 的四个
/// `trailing_var_arg` variant 各加了 `#[command(disable_help_flag = true)]`，clap 不再把
/// `-h` / `--help` 截胡去打它自己那份不含任何真实选项的帮助。带不带 `--` 打出来逐字节相同，
/// 都是 stdout + 退出码 0。钉住它的是 `tests/cli_smoke.rs`（进程级，走 `main.rs`——
/// 单元测试直调 `run_capture` 看不见那一层，原来那条回归就是从那儿躲过去的）。
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
        config: crate::app::DEFAULT_CONFIG_PATH.to_string(),
        grace: DEFAULT_SHUTDOWN_GRACE_SEC,
        traceback: false,
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--config" => {
                i += 1;
                args.config = argv
                    .get(i)
                    .cloned()
                    .ok_or_else(|| "--config 后面缺一个路径".to_string())?;
            }
            "--grace" => {
                i += 1;
                let raw = argv
                    .get(i)
                    .cloned()
                    .ok_or_else(|| "--grace 后面缺一个秒数".to_string())?;
                args.grace = raw
                    .parse()
                    .map_err(|_| format!("--grace 要是数字，收到 {raw:?}"))?;
                // 范围判据自带 NaN 与 ±inf 的兜底（两者都不在闭区间里）。
                // 上界的理由见 `MAX_GRACE_SEC`：不挡的话 `1e300` 会穿到收尾那一刻才 panic。
                if !(0.0..=MAX_GRACE_SEC).contains(&args.grace) {
                    return Err(format!(
                        "--grace 要是 0 到 {MAX_GRACE_SEC:.0} 之间的有限秒数，收到 {raw:?}"
                    )
                    .into());
                }
            }
            "--traceback" => args.traceback = true,
            "-h" | "--help" => return Err(ParseOutcome::Help),
            other => return Err(format!("不认识的参数 {other:?}\n{USAGE}").into()),
        }
        i += 1;
    }
    Ok(args)
}

/// `aite run` 的入口：收到的是 `run` 之后的全部参数，返回进程退出码。
pub fn run(argv: Vec<String>) -> i32 {
    let args = match parse_args(&argv) {
        Ok(a) => a,
        // 「人要看用法」不是「参数写错了」：stdout + 退出码 0。
        Err(ParseOutcome::Help) => {
            println!("{USAGE}");
            return 0;
        }
        Err(ParseOutcome::Bad(e)) => {
            eprintln!("{e}");
            return EXIT_STARTUP;
        }
    };
    init_tracing();

    // 真机是长连接 + 串行派发 + SQLite 的 spawn_blocking，多线程 runtime。
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("aite 起不来：tokio runtime 建不起来：{e}");
            return EXIT_STARTUP;
        }
    };

    runtime.block_on(async move {
        let app = match load_config(&args.config) {
            Ok(config) => match build_app(config, Injections::default()).await {
                Ok(app) => app,
                Err(e) => return startup_failed(&e, &args),
            },
            Err(e) => return startup_failed(&e, &args),
        };
        let opts = ServeOptions {
            shutdown_grace_sec: args.grace,
            ..ServeOptions::default()
        };
        match run_app(&app, &opts).await {
            Ok(()) => 0,
            Err(e) => startup_failed(&e, &args),
        }
    })
}

/// 起不来时的收场：一行人话 + 用的哪份配置，`--traceback` 再多打一行 `Debug` 形。
///
/// **那一行不是错误链。** `StartupError`（`app.rs`）是 `#[error("{0}")]` 的 newtype，
/// 没有 `source()`，所以 `{e:?}` 就是把 `{e}` 那句话外面套一个 `StartupError("…")`。
/// 文案原来承诺的是「完整错误链」，名不副实 —— 已改成说实话（USAGE 与模块头）。
/// 真要有链得让 `StartupError` 带 `source`，那是 `app.rs`（归 V5）；顺带一个事实是
/// `load_config` 早就把底层错误 `{e}` 格式化进字符串了，这条路径上也没东西可链。
fn startup_failed(e: &StartupError, args: &Args) -> i32 {
    eprintln!("aite 起不来：{e}");
    eprintln!(
        "用的配置是 {}（样例见 config/aite.example.yaml）",
        args.config
    );
    if args.traceback {
        eprintln!("{e:?}");
    }
    EXIT_STARTUP
}

/// 日志到 stderr、默认 INFO（Python 版 `logging.basicConfig(level=INFO, stream=stderr)`）。
/// `RUST_LOG` 能覆盖。重复装不报错（集成测试里同进程可能装两次）。
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
