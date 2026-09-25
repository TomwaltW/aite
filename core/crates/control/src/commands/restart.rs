//! `!restart`。主人：DD3（W2 起）。CC2 从 `plane.rs` 原样搬来。
use aite_contracts::{ControlPlane, IngressError, NormalizedEvent, Session, SessionStatus};

use super::StopTarget;
use crate::plane::InProcessControlPlane;

pub(crate) const ENABLED: bool = true;

/// `!help` 里这一行（启用时才列出）。
pub(crate) const HELP: &str = "!restart [要做的事]　重开当前会话";

pub const RESTART_EMPTY_TEXT: &str = "已重开会话，请直接说要做什么。";

/// `!restart` 归档会话时撞上**正在交付**的任务那句话。
///
/// 与 `stop_while_delivering_text` 是同一件事的两种说法：那句是「你伸手去停这一个」，
/// 这句是「你重开了会话，有这么几个我替你停不下来」。两句都**不许**说成「已停止」——
/// 理由在 `StopTarget` 那段：`deliver()` 是一段没有取消点的直路。
///
/// 为什么还要多说后半句「结果仍会回到原来那条话题里」：会话这时已经归档，而 worker
/// 手上那几笔（发产物、发答复）照样会落到旧话题里。用户不知道的话，只会看见一条
/// 「已重开会话」之后又从旧话题冒出一段答复，不知道它是哪来的。
pub fn restart_while_delivering_text(task_nos: &[String]) -> String {
    format!(
        "任务 {} 正在把答复发给你，停不了 —— 结果仍会回到原来那条话题里。",
        task_nos.join("、")
    )
}

pub(crate) async fn run(
    plane: &InProcessControlPlane,
    ev: &NormalizedEvent,
    session: Option<Session>,
    rest: &str,
) -> Result<(), IngressError> {
    plane.cmd_restart(ev, session, rest).await
}

impl InProcessControlPlane {
    /// `!restart`：归档当前会话、按其余文本新开一个。
    ///
    /// **要收的那些任务查的是 `status_tasks`，和 `!status` / `!stop` / 卡片按钮同一份列表；
    /// 分流也共用 `StopTarget::of_existing` 同一条规则。** 原来这里走的是
    /// `list_active_tasks`，于是 `Answering`（W3 那一路：第一步就 `final`、一张卡片都没发过）
    /// 整个从视野里消失 —— 而下面这段注释点名要防的正是它。
    ///
    /// 归档的会话不该留着还在跑的任务：它们的结果会落进一个已经不存在的会话里，
    /// 而且会继续出现在 `!status` 里。§3.3 的 cancel 路径正好是可停的那一半该有的收尾。
    ///
    /// **但只有可停的那一半收得掉。** `Answering` 已经走进 `deliver()`，那是一段
    /// **没有取消点**的直路（`agent.rs:117` 是全仓唯一一处取消检查），实测过两头：
    /// worker 那一侧，取消旗标在 `deliver()` 全程一次都不被看，文件与答复照发、
    /// 照落 `Delivered`、证据链照样 `finalize` 且校验通过；控制面这一侧，
    /// `cancel_task` 打上去只是把库写成 `Cancelled`，随后被 worker 的 `finish()`
    /// 盖回 `Delivered` —— 净结果是回帖说「终止了 1 个进行中的任务」，
    /// 而用户手上答复一个字不少。那是**造一句假话**，不是收尾。
    ///
    /// 所以这一半**一个字都不碰**，改成在回帖里点名说清（`restart_while_delivering_text`）。
    /// 上面那两件事在这一半里于是仍然会发生 —— 但不再是**悄悄**发生的，
    /// 这正是本轨与原状的差别。
    ///
    /// 与 `!stop` 的差别一句话：**`!stop` 是指着一个任务问「停它」，`!restart` 是
    /// 换一个会话、顺手把旧会话名下所有能停的都停掉；两者对「能不能停」的判定完全同源。**
    pub(crate) async fn cmd_restart(
        &self,
        ev: &NormalizedEvent,
        session: Option<Session>,
        rest: &str,
    ) -> Result<(), IngressError> {
        let mut stopped = 0usize;
        let mut delivering: Vec<String> = Vec::new();
        if let Some(mut session) = session.clone() {
            for t in self.status_tasks(&ev.chat_id).await? {
                if t.session_id != session.id {
                    continue;
                }
                match StopTarget::of_existing(t) {
                    StopTarget::Stoppable(t) => {
                        self.cancel_task(t, None, None, false).await;
                        stopped += 1;
                    }
                    // 停不掉的不许算进 `stopped`，也不许被默默漏掉：下面点名。
                    StopTarget::Delivering(t) => delivering.push(t.task_no),
                    // `of_existing` 给不出这三格：它们是「找目标」那一步的结局
                    // （`of(None)` 的那一半 + BB1 从它里面分出来的两格），
                    // 而这里手里的任务是从 `status_tasks` 逐条取出来的，一定存在。
                    StopTarget::NotFound | StopTarget::NoneActive | StopTarget::Ambiguous(_) => {}
                }
            }
            session.status = SessionStatus::Archived;
            session.archived_at = Some(self.now());
            self.store.update_session(&session).await?;
        }
        // 用其余文本新建：还在原话题里的话就继续用同一个 thread_id
        let thread_id = session
            .as_ref()
            .and_then(|s| s.anchor.thread_id.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| ev.anchor.message_id.clone());
        // 只有真的重开了一个话题（原来有会话）才有历史可回灌
        let seed = session.as_ref().map(|_| thread_id.as_str());
        self.new_session(ev, &thread_id, rest, !rest.is_empty(), seed)
            .await?;

        let mut note = if stopped > 0 {
            format!("终止了 {stopped} 个进行中的任务。")
        } else {
            String::new()
        };
        if !delivering.is_empty() {
            note.push_str(&restart_while_delivering_text(&delivering));
        }
        if rest.is_empty() {
            self.reply(ev, &format!("{note}{RESTART_EMPTY_TEXT}")).await
        } else if !note.is_empty() {
            self.reply(ev, &format!("已重开会话，{note}")).await
        } else {
            // rest 非空、既没停掉任何任务也没有停不掉的 → 不回帖（§9 第 3 条钉住的现状）
            Ok(())
        }
    }
}
