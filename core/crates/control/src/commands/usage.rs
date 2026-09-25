//! `!usage`：用量。主人：EE3。
//!
//! CC2 预埋的停用桩：`ENABLED = false` 时注册表把 `!usage` 当未知命令（计数器仍是
//! `commands!usage`），`run` 不会被调到。启用它**只改这个文件**。
use aite_contracts::{IngressError, NormalizedEvent, Session};

use super::UNKNOWN_COMMAND_TEXT;
use crate::plane::InProcessControlPlane;

pub(crate) const ENABLED: bool = false;

/// `!help` 里这一行（启用时才列出）。
pub(crate) const HELP: &str = "!usage　看用量";

pub(crate) async fn run(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
    session: Option<Session>,
    rest: &str,
) -> Result<(), IngressError> {
    let _ = (session, rest);
    plane.reply(ev, UNKNOWN_COMMAND_TEXT).await
}
