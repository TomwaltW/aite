//! `!help`：列出已启用的命令 + 一句说明。CC2 ⑤ 实现并启用；W3 起归 EE8（只给新命令补条目）。
//!
//! 列不列出一条命令只看注册表里它自己的 `ENABLED`（`commands/<名字>.rs`），停用的一个都不出现。
use aite_contracts::{IngressError, NormalizedEvent, Session};

use super::{ALIASES, registry};
use crate::plane::InProcessControlPlane;

pub(crate) const ENABLED: bool = true;

/// `!help` 里这一行（启用时才列出）。
pub(crate) const HELP: &str = "!help　看这份命令列表";

pub(crate) async fn run(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
    session: Option<Session>,
    rest: &str,
) -> Result<(), IngressError> {
    let _ = (session, rest);
    plane.reply(ev, &help_text()).await
}

/// `!help` 的回帖正文。
pub(crate) fn help_text() -> String {
    let enabled: Vec<_> = registry().into_iter().filter(|c| c.enabled).collect();
    let lines: Vec<&str> = enabled.iter().map(|c| c.help).collect();
    let aliases: Vec<String> = ALIASES
        .iter()
        .filter(|(_, name)| enabled.iter().any(|c| c.name == *name))
        .map(|(alias, _)| format!("！{alias}"))
        .collect();
    format!(
        "可用命令：\n{}\n命令开头的 ! 也可以打全角的 ！；中文也行：{}",
        lines.join("\n"),
        aliases.join(" ")
    )
}
