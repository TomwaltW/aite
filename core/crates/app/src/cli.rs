//! `aite run` 的命令行（对应旧 `aite/app.py` 的 `_main`）。
//!
//! 起飞前体检没过一律「一行人话 + 退出码 2」，裸错误链只在 `--traceback` 时打。
use crate::app::{EXIT_STARTUP, Injections, StartupError, build_app, load_config};
use crate::run::{DEFAULT_SHUTDOWN_GRACE_SEC, ServeOptions, run_app};

const USAGE: &str = "\
用法：
  aite run [--config PATH] [--grace SEC] [--traceback]

  --config PATH   配置文件路径（默认 config/aite.yaml；样例见 config/aite.example.yaml）
  --grace SEC     优雅退出的宽限期，默认 20（对齐 compose 的 stop_grace_period）
  --traceback     起不来时把完整错误链打到 stderr（默认只打一行人话）";

#[derive(Debug)]
struct Args {
    config: String,
    grace: f64,
    traceback: bool,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
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
                if !args.grace.is_finite() || args.grace < 0.0 {
                    return Err(format!("--grace 要是 >= 0 的有限数，收到 {raw:?}"));
                }
            }
            "--traceback" => args.traceback = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("不认识的参数 {other:?}\n{USAGE}")),
        }
        i += 1;
    }
    Ok(args)
}

/// `aite run` 的入口：收到的是 `run` 之后的全部参数，返回进程退出码。
pub fn run(argv: Vec<String>) -> i32 {
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
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
