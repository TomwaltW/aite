//! aite-app —— 把 R1–R7 的零件装成一个能起飞的 core 进程。owner: RΩ
//!
//! 对应旧 `aite/app.py`（627 行）。分三层，一层比一层多做一点事：
//!
//! * [`build_app`] **只组装，不产生副作用**：不连网、不起容器、不发消息。唯一落盘的是按
//!   `StorageConfig` 建那几个目录。三个口子（platform / model / sandbox）给了就用给的，
//!   不给才按 config 造真的 —— 集成测试靠它们把替身塞进来测真实接线。
//!   与 Python 版唯一的结构差异是**进程边界**：真平台与真沙箱都在 edge（Go），
//!   所以"造真的"这一路是 `EdgeClient::connect` + 一次 `contract_version` 比对。
//! * [`run_app`] 起投递面、把派发循环挂后台、等停机信号，然后按冻结的顺序收尾。
//! * [`cli::run`] 命令行 + 起飞前体检：读不到配置、prompt 缺了、两边契约版本不一致这类
//!   一眼能看出来的问题，一行中文说清缺什么怎么补，退出码 2。
//!
//! 做成 lib（而不是只有 `main.rs`）是为了让 `tests/` 里移植过来的集成测试能直接
//! `build_app(config, Injections{platform: Some(fake), ..})` —— Python 那 53 条
//! 整条都建立在"注入替身、测真实接线"上，隔着进程看退出码测不出来。
pub mod app;
pub mod cli;
pub mod features;
pub mod preflight;
pub mod run;
pub mod wiring;

pub use app::{
    AiteApp, EXIT_HARD_STOP, EXIT_STARTUP, Injections, StartupError, build_app,
    build_app_with_features, load_config, sandbox_spec_of,
};
pub use run::{DEFAULT_SHUTDOWN_GRACE_SEC, ServeOptions, StopSignal, run_app};
