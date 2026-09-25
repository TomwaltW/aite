//! `!status`。主人：DD3（W2 起）。CC2 从 `plane.rs` 原样搬来。
use aite_contracts::{IngressError, NormalizedEvent, Session};

use super::stop::NO_ACTIVE_TASK_TEXT;
use crate::plane::InProcessControlPlane;

pub(crate) const ENABLED: bool = true;

/// `!help` 里这一行（启用时才列出）。
pub(crate) const HELP: &str = "!status　列出本群的活跃任务";

pub(crate) async fn run(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
    session: Option<Session>,
    rest: &str,
) -> Result<(), IngressError> {
    let _ = (session, rest);
    plane.cmd_status(ev).await
}

impl InProcessControlPlane {
    pub(crate) async fn cmd_status(&self, ev: &NormalizedEvent) -> Result<(), IngressError> {
        let tasks = self.status_tasks(&ev.chat_id).await?;
        if tasks.is_empty() {
            let text = format!("{NO_ACTIVE_TASK_TEXT}{}", self.dropped_note());
            return self.reply(ev, &text).await;
        }
        let lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                let title = if t.title.is_empty() {
                    "(无标题)"
                } else {
                    t.title.as_str()
                };
                format!("{} {} {}", t.task_no, t.status.as_str(), title)
            })
            .collect();
        let text = format!(
            "本群活跃任务：\n{}{}",
            lines.join("\n"),
            self.dropped_note()
        );
        self.reply(ev, &text).await
    }

    /// 丢过事件才多说这一句，平时一个字都不加。
    ///
    /// 这条警告是**进程级**的（不分群）：`events.dropped` 数的是路由抛出去的事件，
    /// 而抛在 `seen_event` 上时连 `chat_id` 归谁都还没走到，分不了群。
    ///
    /// **数的是两个计数器的和（BB1）**，因为这句话回答的是「本进程有没有事情没办成」：
    /// - `events.dropped` —— 事件在路由里炸了（`handle_event` 的错误分支）；
    /// - `control.cancel_save_failed` —— `!stop` 认下来了，但 `Cancelled` 没落进库，
    ///   而那一支是直接 `return`，用户连「没停成」都听不到。
    ///
    /// 这两笔以前共用 `events.dropped` 一个名字。拆名字是为了 M4 分得开（见
    /// `cancel_task` 里那段），把和加回来是为了这句话别因为拆名字而漏报 ——
    /// 用户要的是「有没有」，排障的人要的才是「是哪一类」。
    /// 文案一个字没动（`{n}` 的口径从来就是「没接住的条数」，不是某个计数器的名字）。
    pub(crate) fn dropped_note(&self) -> String {
        let n = self.shared.counter("events.dropped")
            + self.shared.counter("control.cancel_save_failed");
        if n == 0 {
            return String::new();
        }
        format!(
            "\n⚠ 本进程启动以来有 {n} 条事件没接住（多半是存储异常），\
可能有消息没被处理。翻日志看 ingress.handle_failed。"
        )
    }
}
