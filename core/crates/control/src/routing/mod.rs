//! §3.5 路由：R1–R8 按编号顺序求值、命中即停。CC2 从 `plane.rs` 原样搬来。
//!
//! ## 链的顺序与预埋桩（CC2 ①）
//!
//! 桩一律返回 [`Flow`]：`Pass` = 今天的行为照走，`Handled` = 桩已处理、链到此为止。
//! 桩现在全是 `Pass`、不做 I/O、不计数、不打日志 —— 所以 ① 是零行为变化的。
//! 以后启用某个桩**只改那一个文件**，这张顺序表归 `routing/mod.rs` 的主人（W2 起是 DD3）。
//!
//! | 位置 | 桩 | 主人 |
//! |---|---|---|
//! | R1 非真人 → R2 去重 之后、R3 之前 | [`crate::gate::hook`]（启用检查 / 外部群 / 成员白名单；未启用的群里 `!connect` `!help` `!about` 要 `Pass` 放行到命令路由） | EE8 |
//! | R3 里、Stop / Evidence 分支之前 | [`card_actions::hook`] | EE7（T0c 先加 log+drop 分支） |
//! | R3 之后、`R4_KINDS` 判定之前，对**每个非卡片事件**都调（含 R4 的那几种） | [`external::hook`]、[`crate::mute::pre_r4`] —— 桩内自己按 kind 过滤 | EE11、EE9 |
//! | R4 的 `MessageEdited` 分支 | [`edits::hook`] | EE13 |
//! | R4 的 `BotAdded` 分支 | [`crate::intro::hook`] | EE9 |
//! | `thread_session` 没命中时 | [`resolver::resolve`] | DD3 |
//! | R5 之前 | [`crate::mute::pre_r5`] | EE9 |
//! | R6 之前，只收 p2p | [`crate::dm::hook`] | FF4 |
//! | R7 与 R8 之间 | [`crate::sessions_channel::hook`]、[`crate::ambient::hook`] | FF1 |
//!
//! 不进链的：[`crate::internal`]（`submit_internal` 的落点，DD3）、[`crate::approvals`]（审批信箱，EE7）。
use aite_contracts::{
    CardActionKind, ChatType, ControlPlane, EventKind, EvidenceKind, IngressError, NormalizedEvent,
    SenderKind, Session, Task, TurnRole,
};

use crate::commands::{NO_SUCH_TASK_TEXT, StopTarget, is_command, stop_while_delivering_text};
use crate::dispatch::steer_target;
use crate::evidence_log::steer_payload;
use crate::lock;
use crate::plane::InProcessControlPlane;
use crate::{ambient, dm, gate, intro, mute, sessions_channel};

pub(crate) mod card_actions;
pub(crate) mod edits;
pub(crate) mod external;
pub(crate) mod resolver;

/// 桩的结局。`Pass` = 照今天的路走；`Handled` = 桩已处理完，链到此为止。
pub(crate) enum Flow {
    Pass,
    /// 今天没有桩返回它；主人轨启用各自的桩时才构造（EE7 / EE8 / EE9 / EE11 / EE13 / FF1 / FF4）。
    #[allow(dead_code)]
    Handled,
}

/// R4 的四种「不触发任务」事件。
const R4_KINDS: [EventKind; 4] = [
    EventKind::MessageEdited,
    EventKind::MessageDeleted,
    EventKind::BotAdded,
    EventKind::MemberChanged,
];

/// 路由自己丢掉一条事件时的那行 INFO（`acceptance-M.md` §8 第 3 条的观测缺口）。
///
/// **为什么非要有日志，计数器不够**：R1 / R2 / R8 各自的计数器（`events.nonhuman` /
/// `events.duplicate` / `events.ignored`）早就有了，但它们只回答「丢了几条」，
/// 答不了「丢的是哪一条」。M4 要判的恰恰是后者 —— 手上拿着开放平台的一个 event_id，
/// 问「这条到底有没有投递到 core」。计数器答不了，于是只能去翻推送记录。
///
/// **一条规则一个名字，不合并。** 合成一个 `control.event_dropped` 再拿字段区分，
/// 等于把 M4 要分的那件事（没投递 vs 投递了被丢、以及被谁丢的）重新糊回一起：
/// `grep control.drop_duplicate` 是「平台重推的」，`grep control.drop_ignored` 是
/// 「投递条件没满足」，两者的处置完全不同 —— 前者正常，后者多半是用户以为自己在跟
/// Aite 说话而 Aite 没听见。
///
/// **级别是 INFO**：这三条都不是故障（R1/R2 是规则正常生效，R8 是群里的日常闲聊），
/// 真机上 R8 尤其会很吵。放 WARN 会把真正的 WARN 淹掉；放 DEBUG 又等于没有 ——
/// §0.3 的四个观察窗默认就是 INFO。字段名与 §7 那张表既有的一致（`event` / `kind`）。
mod drop_log {
    use aite_contracts::NormalizedEvent;

    /// R1：非真人。带上 `sender_kind` —— 「机器人自言自语」和「系统消息」的处置不一样。
    pub(super) fn nonhuman(ev: &NormalizedEvent) {
        tracing::info!(
            target: "aite.control",
            event = %ev.event_id,
            kind = %ev.kind,
            sender_kind = %ev.sender_kind,
            sender = %ev.sender_id,
            "control.drop_nonhuman R1 丢弃：不是真人发的，永远不触发任务"
        );
    }

    /// R2：`seen_event` 命中。这条是**正常**的（重连重推、平台的至少一次投递），
    /// 打出来是为了让 M4 那句「它到底有没有到过 core」有个肯定的答案。
    pub(super) fn duplicate(ev: &NormalizedEvent) {
        tracing::info!(
            target: "aite.control",
            event = %ev.event_id,
            kind = %ev.kind,
            "control.drop_duplicate R2 丢弃：这条已经处理过了（平台重推）"
        );
    }

    /// R8：其余丢弃。字段最多的一条，因为它是**唯一一条「本来可能该被处理」的丢弃**。
    ///
    /// `mentioned` 与 `thread` 合起来就是 R5/R6/R7 的三个入口条件为什么都没命中：
    /// 没 @（`mentioned=false`）、又不在任何已有话题里（`thread` 为空，或者填了
    /// 但库里查不到那个会话）。README §已知边界那条乱序重推的追问，在日志里的形状
    /// 正是 `mentioned=false thread=om_xxx` —— 在这一行出现之前它丢得一声不吭。
    pub(super) fn ignored(ev: &NormalizedEvent) {
        tracing::info!(
            target: "aite.control",
            event = %ev.event_id,
            kind = %ev.kind,
            chat_type = %ev.chat_type,
            mentioned = ev.mentioned,
            thread = %ev.anchor.thread_id.as_deref().unwrap_or(""),
            "control.drop_ignored R8 丢弃：既没 @ 机器人，也不在已有话题里"
        );
    }
}

impl InProcessControlPlane {
    // ---- §3.5 路由 -------------------------------------------------------

    /// R1–R8，按编号顺序求值、命中即停。
    pub(crate) async fn route(&self, ev: &NormalizedEvent) -> Result<(), IngressError> {
        // R1：非真人一律丢弃。机器人/应用/系统消息永远不触发任务（含 Aite 自己发的）
        if ev.sender_kind != SenderKind::Human {
            self.shared.bump("events.nonhuman");
            drop_log::nonhuman(ev);
            return Ok(());
        }

        // R2：重连后平台重推的重复事件
        if self.store.seen_event(&ev.event_id).await? {
            self.shared.bump("events.duplicate");
            drop_log::duplicate(ev);
            return Ok(());
        }

        // 准入闸（EE8）：R2 之后、R3 之前
        if let Flow::Handled = gate::hook(self, ev).await? {
            return Ok(());
        }

        // R3：卡片回传，不建会话
        if ev.kind == EventKind::CardAction {
            return self.on_card_action(ev).await;
        }

        // R3 之后、R4 判定之前：对每个非卡片事件都调（含 R4 那几种），桩内自己按 kind 过滤
        if let Flow::Handled = external::hook(self, ev).await? {
            return Ok(());
        }
        if let Flow::Handled = mute::pre_r4(self, ev).await? {
            return Ok(());
        }

        // R4：编辑/删除/成员变动，不触发任务
        if R4_KINDS.contains(&ev.kind) {
            return self.on_non_message(ev).await;
        }

        // 每条 message 事件都先做一次 find_session_by_thread —— R5 要用它判「在不在话题里」
        let session = match self.thread_session(ev, false).await? {
            Some(session) => Some(session),
            // 话题查不到会话时的第二条路（DD3）
            None => resolver::resolve(self, ev).await?,
        };
        let text = ev.text.trim().to_string();

        if let Flow::Handled = mute::pre_r5(self, ev).await? {
            return Ok(());
        }

        // R5：`!` 命令，先于 R6/R7 判定；只接受 @ 过的或已在话题内的消息
        if is_command(&text) && (ev.mentioned || session.is_some()) {
            return self.on_command(ev, session, &text).await;
        }

        // 私聊（FF4）：R6 之前，只收 p2p
        if ev.chat_type == ChatType::P2p
            && let Flow::Handled = dm::hook(self, ev).await?
        {
            return Ok(());
        }

        // R6：话题内续接，不要求 mentioned。CC2 ⑥：看平台有没有原生话题（`supports_thread`），
        // 不再写死「群聊」；p2p 不进 R6 —— 它先过上面的 DM 桩（FF4），桩关着时净行为与今天一致
        // （有 @ 走 R7，无 @ 走 R8）。
        if ev.chat_type != ChatType::P2p
            && self.platform.capabilities().supports_thread
            && let Some(session) = session
        {
            return self.continue_session(ev, session).await;
        }

        // R7：@ 了 Aite → 新建 task session，本条消息成为话题 root
        if ev.mentioned {
            let thread_id = ev.anchor.message_id.clone();
            let text = ev.text.clone();
            self.new_session(ev, &thread_id, &text, true, None).await?;
            return Ok(());
        }

        // R7 与 R8 之间：频道会话、环境观察（FF1）
        if let Flow::Handled = sessions_channel::hook(self, ev).await? {
            return Ok(());
        }
        if let Flow::Handled = ambient::hook(self, ev).await? {
            return Ok(());
        }

        // R8：其余丢弃（P0 无群会话、无 DM 主动监听）
        self.shared.bump("events.ignored");
        drop_log::ignored(ev);
        Ok(())
    }

    // ---- R3 --------------------------------------------------------------

    pub(crate) async fn on_card_action(&self, ev: &NormalizedEvent) -> Result<(), IngressError> {
        let Some(action) = ev.card_action.as_ref() else {
            self.shared.bump("events.bad_card_action");
            return Ok(());
        };
        // 新的卡片动作（EE7）：现有 Stop / Evidence 分支之前
        if let Flow::Handled = card_actions::hook(self, ev, action).await? {
            return Ok(());
        }
        match action.action {
            CardActionKind::Stop => {
                let task = self
                    .resolve_task(&ev.chat_id, action.task_id.as_deref(), &action.card_id)
                    .await?;
                match task {
                    // `resolve_task` 走 `of()`，只给得出 NotFound / Delivering / Stoppable
                    // 三格 —— 卡片按 `task_id` / `card_id` 找，没有「省略」这个形状，
                    // 另两格到不了这里。写进来只为让 match 穷尽，并且万一哪天到得了，
                    // 卡片这条路给的还是它原来那句话（BB1 一个字都没改卡片的行为）。
                    StopTarget::NotFound | StopTarget::NoneActive | StopTarget::Ambiguous(_) => {
                        self.reply(ev, NO_SUCH_TASK_TEXT).await?
                    }
                    StopTarget::Delivering(task) => {
                        self.reply(ev, &stop_while_delivering_text(&task.task_no))
                            .await?
                    }
                    StopTarget::Stoppable(task) => {
                        self.cancel_task(
                            task,
                            Some(ev.anchor.message_id.clone()),
                            Some(ev.chat_id.clone()),
                            true,
                        )
                        .await;
                    }
                }
            }
            CardActionKind::Evidence => {
                let task_id = action.task_id.clone().unwrap_or_default();
                // 契约里 EvidenceWriter 显式有 task_dir（D3），不再靠 getattr 探测
                let where_ = self.evidence.task_dir(&task_id).display().to_string();
                self.reply(ev, &format!("任务 {task_id} 的证据目录：{where_}"))
                    .await?;
            }
        }
        Ok(())
    }

    // ---- R4 --------------------------------------------------------------

    pub(crate) async fn on_non_message(&self, ev: &NormalizedEvent) -> Result<(), IngressError> {
        match ev.kind {
            EventKind::MessageEdited => {
                if let Flow::Handled = edits::hook(self, ev).await? {
                    return Ok(());
                }
                // 编辑即使加上 @Aite 也不启动任务，只在命中的会话里留一条 system_note
                if let Some(session) = self.thread_session(ev, true).await? {
                    let content = format!("[用户修改了消息] 新内容：{}", ev.text);
                    self.append_turn(&session, ev, TurnRole::SystemNote, &content)
                        .await?;
                }
                self.shared.bump("events.edited");
            }
            EventKind::MessageDeleted => self.shared.bump("events.deleted"),
            EventKind::BotAdded => {
                if let Flow::Handled = intro::hook(self, ev).await? {
                    return Ok(());
                }
                self.shared.bump(&format!("events.{}", ev.kind.as_str()));
            }
            other => self.shared.bump(&format!("events.{}", other.as_str())),
        }
        Ok(())
    }

    // ---- R6 / R7 ---------------------------------------------------------

    pub(crate) async fn continue_session(
        &self,
        ev: &NormalizedEvent,
        mut session: Session,
    ) -> Result<(), IngressError> {
        let text = ev.text.clone();
        self.append_turn(&session, ev, TurnRole::User, &text)
            .await?;
        session.last_active_at = self.now();
        self.store.update_session(&session).await?;
        // CC2 ⑦：追问也给个「收到」，steer 与新建两条路都算（机器人在 R1 就被丢了，走不到这里）
        self.ack(ev).await;

        // 先把库读完再取锁：`std::sync::Mutex` 的 guard 不许跨 await 活着（§7.6）。
        let listed = self.store.list_active_tasks(&session.chat_id).await?;
        let active: Vec<Task> = {
            let cancelled = lock(&self.shared.cancelled);
            listed
                .into_iter()
                .filter(|t| t.session_id == session.id && !cancelled.contains(&t.id))
                .collect()
        };

        let target = {
            let owned = lock(&self.shared.owned);
            let running = lock(&self.shared.running);
            steer_target(&active, &owned, &running)
        };

        // 卡死替换（CC2 ④）：追问撞上一个在跑、却很久没进展的任务 → 停掉它、按这条消息新开
        if let Some(stuck) = target.as_ref().filter(|t| self.is_stuck(t)) {
            let stuck = stuck.clone();
            self.abandon_running(&stuck.id).await;
            let task_no = stuck.task_no.clone();
            self.cancel_task_inner(stuck, None, None, false).await;
            self.shared.bump("control.stuck_replaced");
            self.reply(
                ev,
                &stuck_task_replaced_text(&task_no, self.stuck_after.num_minutes()),
            )
            .await?;
            self.start_task(&session, ev, None).await?;
            return Ok(());
        }

        if let Some(target) = target {
            // 先写证据再排队，和 `start_task` 一个顺序：证据链宁可少一条也不撒谎，
            // 不能出现「任务收到了这句追问，但链上查不到它是什么时候来的」。
            self.evidence
                .append(&target.id, EvidenceKind::EventReceived, steer_payload(ev))
                .await?;
            // worker 每步开始前会把这些合并进上下文
            lock(&self.shared.steer)
                .entry(target.id.clone())
                .or_default()
                .push(ev.text.clone());
            self.shared.bump("events.steer");
            return Ok(());
        }
        if !active.is_empty() {
            // 库里还是 active，可本进程既没在跑它也没排过它 —— 上个进程被杀之后留在
            // working 上的孤儿，没人会来 drain 它的 steer。按 R6 的「否则新建 task
            // 继续」走：不然用户这句追问排进一个永不消费的队列，看起来就是 Aite 没反应。
            self.shared.bump("events.orphan_task");
        }
        self.start_task(&session, ev, None).await?;
        Ok(())
    }

    /// 卡死 = 本进程正在跑它，而它的 `updated_at`（worker 每步落库时刷新）离现在超过阈值。
    fn is_stuck(&self, task: &Task) -> bool {
        lock(&self.shared.running).contains(&task.id)
            && self.now() - task.updated_at > self.stuck_after
    }
}

/// 卡死替换的那一句（CC2 ④）。
pub fn stuck_task_replaced_text(task_no: &str, minutes: i64) -> String {
    format!("任务 {task_no} 超过 {minutes} 分钟没有进展，已停止，按这条消息重新开始。")
}
