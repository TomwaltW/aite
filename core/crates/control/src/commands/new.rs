//! `!new`。CC2 从 `plane.rs` 原样搬来。
use aite_contracts::{IngressError, NormalizedEvent, Session};

use super::restart::RESTART_EMPTY_TEXT;
use crate::plane::InProcessControlPlane;

pub(crate) const ENABLED: bool = true;

/// `!help` 里这一行（启用时才列出）。
pub(crate) const HELP: &str = "!new [要做的事]　另起一个新话题";

pub(crate) async fn run(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
    session: Option<Session>,
    rest: &str,
) -> Result<(), IngressError> {
    plane.cmd_new(ev, session, rest).await
}

impl InProcessControlPlane {
    /// `!new`：强制新建一个会话。
    ///
    /// CC2 ⑧：**在话题里**发的 `!new`，新会话挂在**同一个话题**上（用该话题的 `thread_id`）。
    /// 原来是让 `!new` 这条消息自己当新 root，可飞书里用户接着在原话题里回复，`thread` 仍是
    /// 老 root —— `find_session_by_thread` 找到的还是老会话，`!new` 等于白发。老会话不归档、
    /// 其任务不动（这是 `!new` 与 `!restart` 的区别）。
    ///
    /// **正确性依赖 `created_at` 严格递增**：同一 `thread_id` 上此时挂着两个非归档会话，
    /// `find_session_by_thread` 取 `created_at` 最新的那个（同刻时按 id，而 id 是随机 uuid）。
    /// 两个会话若在同一时间戳上建出来（SQLite 时间戳精度、或注入了固定钟），胜负是随机的。
    ///
    /// 不在话题里（顶层 @ 着发的）照旧：本条消息成为新话题的 root。
    pub(crate) async fn cmd_new(
        &self,
        ev: &NormalizedEvent,
        session: Option<Session>,
        rest: &str,
    ) -> Result<(), IngressError> {
        let thread_id = session
            .as_ref()
            .and_then(|s| s.anchor.thread_id.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| ev.anchor.message_id.clone());
        self.new_session(ev, &thread_id, rest, !rest.is_empty(), None)
            .await?;
        if rest.is_empty() {
            return self.reply(ev, RESTART_EMPTY_TEXT).await;
        }
        // rest 非空 → 不回帖（§9 第 3 条）
        Ok(())
    }
}
