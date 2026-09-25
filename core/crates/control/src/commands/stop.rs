//! `!stop`。CC2 从 `plane.rs` 原样搬来（命令本体、找目标、那几句文案）。
use aite_contracts::{IngressError, NormalizedEvent, Session};

use super::{StopTarget, normalize_task_no};
use crate::plane::InProcessControlPlane;

pub(crate) const ENABLED: bool = true;

/// `!help` 里这一行（启用时才列出）。
pub(crate) const HELP: &str = "!stop <任务号>　停止一个任务（本群只有一个活跃任务时可省略任务号）";

pub const NO_SUCH_TASK_TEXT: &str = "没有这个任务";
pub const NO_ACTIVE_TASK_TEXT: &str = "本群没有活跃任务";

/// `!stop` / 卡片 stop 按钮撞上一个**已经在交付里**的任务时的那句话。
///
/// 不能回 `NO_SUCH_TASK_TEXT` —— V5 之后它明明还在 `!status` 的列表里，
/// 用户看得见的东西不许被这一句说成不存在（见 `StopTarget` 那段）。
pub fn stop_while_delivering_text(task_no: &str) -> String {
    format!("任务 {task_no} 正在把答复发给你，停不了了。")
}
/// `!stop` 省略了任务号、而本群有不止一个活跃任务时的那句话。
///
/// **不能回 `NO_SUCH_TASK_TEXT`**：用户一个任务号都没给，「没有这个任务」说的是哪一个？
/// 他刚在 `!status` 里看见三个，这一句只会让他以为那份列表是假的。缺的是「指哪一个」，
/// 那就把可选的列出来 —— 列的和 `!status` 是同一份 `status_tasks`、同一个序。
///
/// 为什么把任务号全列出来而不是只说「请带上任务号」：那样用户还得再敲一次 `!status`。
/// 多说这一串的代价只有长度，而**不设条数上限是刻意的** —— 这一句列的和 `!status`
/// 列的是同一批任务，那边一行一个、同样没有上限；单给这里加一个截断，等于让两条命令
/// 又各说各的（W2 收的正是这个口）。
pub fn stop_needs_task_no_text(task_nos: &[String]) -> String {
    format!(
        "本群有 {} 个活跃任务，要停哪个请带上任务号：{}。",
        task_nos.len(),
        task_nos.join("、")
    )
}

pub(crate) async fn run(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
    session: Option<Session>,
    rest: &str,
) -> Result<(), IngressError> {
    let _ = session;
    plane.cmd_stop(ev, rest).await
}

impl InProcessControlPlane {
    pub(crate) async fn cmd_stop(
        &self,
        ev: &NormalizedEvent,
        rest: &str,
    ) -> Result<(), IngressError> {
        match self.resolve_stop_target(&ev.chat_id, rest).await? {
            // 与 `cmd_status` 同一句：两条命令看的是同一份列表，列表空的时候不许各说各的。
            StopTarget::NoneActive => self.reply(ev, NO_ACTIVE_TASK_TEXT).await,
            StopTarget::Ambiguous(tasks) => {
                let task_nos: Vec<String> = tasks.into_iter().map(|t| t.task_no).collect();
                self.reply(ev, &stop_needs_task_no_text(&task_nos)).await
            }
            StopTarget::NotFound => self.reply(ev, NO_SUCH_TASK_TEXT).await,
            StopTarget::Delivering(task) => {
                self.reply(ev, &stop_while_delivering_text(&task.task_no))
                    .await
            }
            StopTarget::Stoppable(task) => {
                // CC2 ⑨：先写命令证据、再取消（「不在跑」那一支会 finalize，写在后面 manifest 就对不上）
                self.record_command(ev, &task, "!stop").await;
                self.cancel_task_by(
                    task,
                    Some(ev.anchor.message_id.clone()),
                    Some(ev.chat_id.clone()),
                    true,
                    Some(ev.sender_id.clone()),
                )
                .await;
                Ok(())
            }
        }
    }

    /// `!stop` / 卡片 stop 按钮找目标，找**同一份**列表：`status_tasks`。
    ///
    /// V5 只把控制面的 `running` 并进了 `cmd_status`，`resolve_stop_target` /
    /// `resolve_task` 原地没动，于是交付中的短任务在 `!status` 里列得出来、
    /// `!stop` 却回「没有这个任务」—— 用户看见它在列表里、伸手去停却被告知不存在。
    /// 改之前两条命令口径一致（都查不到），改之后互相矛盾，比原来更费解。
    ///
    /// 这里把「查哪些任务」统一到 `status_tasks`，再按能不能停分成两种结局。
    /// **`ACTIVE_TASK_STATUSES` 一个字都没动** —— 它是双重冻结的
    /// （`contracts` 的 `frozen_values.rs` + `roundtrip.rs`），这条路本来也走不通。
    pub(crate) async fn resolve_stop_target(
        &self,
        chat_id: &str,
        raw: &str,
    ) -> Result<StopTarget, IngressError> {
        let tasks = self.status_tasks(chat_id).await?;
        if raw.is_empty() {
            // 只有一个任务时允许省略任务号。「只有一个」按**用户看得见的那份列表**算，
            // 也就是 `!status` 列出来的那些 —— 否则又成了两条命令各数各的。
            //
            // 另外两种数目以前一起掉进 `NotFound`，各自分出去（见 `StopTarget` 那段）：
            // 一个都没有 → 说 `!status` 那句；有好几个 → 点名要任务号并列出候选。
            return Ok(match tasks.len() {
                0 => StopTarget::NoneActive,
                1 => StopTarget::of_existing(tasks.into_iter().next().expect("刚判过长度")),
                _ => StopTarget::Ambiguous(tasks),
            });
        }
        // 给了任务号就按任务号回答：对不上就是「没有这个任务」，**哪怕列表本来就是空的**。
        // 空列表那一格只归省略任务号那条路 —— 用户指着一个号问，答的该是这个号的下落。
        let want = normalize_task_no(raw);
        Ok(StopTarget::of(
            tasks.into_iter().find(|t| t.task_no == want),
        ))
    }

    pub(crate) async fn resolve_task(
        &self,
        chat_id: &str,
        task_id: Option<&str>,
        card_id: &str,
    ) -> Result<StopTarget, IngressError> {
        let tasks = self.status_tasks(chat_id).await?;
        let found = if let Some(task_id) = task_id.filter(|t| !t.is_empty()) {
            tasks.into_iter().find(|t| t.id == task_id)
        } else {
            tasks.into_iter().find(|t| {
                t.card_id
                    .as_deref()
                    .is_some_and(|c| !c.is_empty() && c == card_id)
            })
        };
        Ok(StopTarget::of(found))
    }
}
