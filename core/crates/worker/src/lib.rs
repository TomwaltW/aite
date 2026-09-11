//! aite-worker —— Agent loop / 卡片合并 / 上下文构造 / 提示词（对应旧 `aite/worker/**`
//! 与 `aite/app.py::AppWorker`）。owner: R5
//!
//! 对外只有三样东西：`AgentWorker`（`impl TaskWorker`）、`card` / `context` 两个模块，
//! 外加 `texts` / `fingerprint` 两个供别的轨复用的面：
//!
//! - `texts`：§3.3 三条硬约束里的「固定文案逐字不变」那条，集中在一个文件里好核对；
//! - `fingerprint`：T20 的「同一张牌」口径，R7 的 protocol probe 直接复用同一个函数。
//!
//! 提示词 `prompts/platform.md` 与 Python 版逐字节相同（`cmp` 在 CI 与派单验收里都钉着），
//! 内容不许在本轨改。
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

pub mod agent;
pub mod card;
pub mod context;
pub mod fingerprint;
pub mod mime;
pub mod texts;

pub use agent::{
    AgentWorker, MAX_CONSECUTIVE_INVALID_ARGS, MAX_CONSECUTIVE_REPEATS,
    MAX_CONSECUTIVE_SANDBOX_ERRORS, MODEL_RETRY_DELAYS, REPEAT_NUDGE_AT, WorkerDeps,
};

/// 单调钟（秒）。测试注入假钟，免得 500ms 卡片合并窗口与 max_wall_sec 要真等。
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;
/// 退避用的 sleep。测试注入「只把钟拨过去」的版本。
pub type Sleeper = Arc<dyn Fn(f64) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

pub(crate) fn default_clock() -> Clock {
    Arc::new(|| PROCESS_START.elapsed().as_secs_f64())
}

pub(crate) fn default_sleeper() -> Sleeper {
    Arc::new(|secs: f64| {
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_secs_f64(secs)).await;
        })
    })
}

/// worker 内部的失败面。这些错误一律被 `run()` 收敛成 task failed，绝不外抛
/// （`TaskWorker::run` 返回 `Task`，不是 `Result`）。
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("system prompt 不存在：{0}")]
    SystemPromptMissing(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{0}")]
    Worker(#[from] WorkerError),
    #[error("{0}")]
    Store(#[from] aite_contracts::StoreError),
    #[error("{0}")]
    Evidence(#[from] aite_contracts::EvidenceError),
    #[error("{0}")]
    Platform(#[from] aite_contracts::PlatformError),
}
