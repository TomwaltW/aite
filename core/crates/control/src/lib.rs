//! aite-control —— ControlPlane：路由 R1–R8、命令、派发队列、steer、reaper、取消、Ingress。
//!
//! 对应的 Python 源码（规格，只读）：
//! - `aite/control/plane.py` → [`plane`]（`InProcessControlPlane`；CC2 起按职责拆进
//!   `routing/`、`sessions`、`dispatch`、`reaper`、`evidence_log`、`commands/`）
//! - `aite/control/commands.py` → [`commands`]
//! - `aite/ingress/handler.py` → [`ingress`]（`Ingress`）
//! - `aite/worker/card.py` 的 `clip` / `render_card` → [`card`]（本 crate 私写一份：取消路径
//!   要把卡片置 cancelled，而 worker crate 是并行的另一轨，§7.3「替身私有，重复远比冲突便宜」）
//!
//! 并发结构与 Python 版的对应（`review/inventory-core.md` §1 那张表）：
//!
//! | Python | 这里 |
//! |---|---|
//! | `asyncio.Queue[str]` | `queue::DispatchQueue`（`VecDeque` + `Notify`，带 `task_done/join`） |
//! | `_steer: dict[task_id, list[str]]` | `Mutex<HashMap<String, Vec<String>>>` |
//! | `_cancelled` / `_running` / `_owned` | `Mutex<HashSet<String>>` |
//! | `_turn_seq_lock: asyncio.Lock` | `tokio::sync::Mutex<()>`（唯一跨 await 持有的锁） |
//! | `counters: defaultdict[str,int]` | `Mutex<BTreeMap<String, i64>>` |
//!
//! 除 `_turn_seq_lock` 外全是 `std::sync::Mutex`，临界区里不 await（§7.6）。
use std::sync::{Mutex, MutexGuard};

pub(crate) mod ambient;
pub(crate) mod approvals;
pub mod card;
pub mod commands;
pub(crate) mod dispatch;
pub(crate) mod dm;
pub(crate) mod evidence_log;
pub(crate) mod gate;
pub mod ingress;
pub(crate) mod internal;
pub(crate) mod intro;
pub(crate) mod mute;
pub mod plane;
pub(crate) mod queue;
pub(crate) mod reaper;
pub(crate) mod routing;
pub(crate) mod sessions;
pub(crate) mod sessions_channel;

pub use commands::{
    ALIASES, NO_ACTIVE_TASK_TEXT, NO_SUCH_TASK_TEXT, RESTART_EMPTY_TEXT, UNKNOWN_COMMAND_TEXT,
    is_command, normalize_task_no, parse_command, restart_while_delivering_text,
    stop_needs_task_no_text, stop_while_delivering_text,
};
pub use evidence_log::{ROUTE_NEW_TASK, ROUTE_STEER};
pub use ingress::{Ingress, SLOW_CALLBACK_SEC};
pub use plane::{
    ControlDeps, InProcessControlPlane, PlaneState, STUCK_AFTER_SEC, SleepFn, WallClock,
};
pub use reaper::REAPER_INTERVAL_SEC;
pub use routing::stuck_task_replaced_text;

/// 取锁，中毒也不 panic（§7.7：`unwrap/expect` 不落在运行期可失败的路径上）。
///
/// 锁中毒只可能来自「持锁时 panic」，而这里的临界区都是纯内存操作；真撞上了也没有
/// 「放弃整条控制面」的道理 —— 拿回内层数据继续，比让进程带着一把毒锁瘫掉好。
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
