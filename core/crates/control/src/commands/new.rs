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
    let _ = session;
    plane.cmd_new(ev, rest).await
}

impl InProcessControlPlane {
    pub(crate) async fn cmd_new(
        &self,
        ev: &NormalizedEvent,
        rest: &str,
    ) -> Result<(), IngressError> {
        // 强制新建，即使已经在别的话题里：本条消息成为新话题的 root
        let thread_id = ev.anchor.message_id.clone();
        self.new_session(ev, &thread_id, rest, !rest.is_empty(), None)
            .await?;
        if rest.is_empty() {
            return self.reply(ev, RESTART_EMPTY_TEXT).await;
        }
        // rest 非空 → 不回帖（§9 第 3 条）
        Ok(())
    }
}
