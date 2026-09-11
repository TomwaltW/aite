//! Checklist 卡片的渲染与更新合并（W3 / W4）。对应旧 `aite/worker/card.py`。
//!
//! W4 要求「同一任务 500ms 内多次变更只调一次 `update_card`，任务结束时必定再调一次」。
//! 这里用**拉取式**合并，不起后台定时器：
//!
//! - 一个 500ms 窗口内的第一次变更立即推送，窗口内后续变更只更新待发状态；
//! - 每步结束时 `maybe_flush()` 会把过期的待发状态推出去；
//! - 任务结束时 `force_flush()` 无条件再推一次，兜住窗口末尾没推出去的那次。
//!
//! 取舍：没有定时器就意味着「最后一次变更后如果再无任何活动」，要等到任务结束才落地。
//! P0 里任务结束一定会到（delivered / failed / cancelled 三条路都调 force_flush），
//! 所以变更不会丢；换来的是测试完全确定、不依赖真实时钟。
use std::sync::Arc;

use aite_contracts::{
    CardStatus, ChecklistCard, ChecklistItemView, PlatformError, PlatformPort, Session, Task,
};
use chrono::Timelike;

use crate::Clock;

/// `ChecklistCard.title`：「≤40 字，worker 截断」
pub const MAX_TITLE_CHARS: usize = 40;
/// W9：checklist 每项 ≤20 字
pub const MAX_ITEM_CHARS: usize = 20;

/// 折叠所有空白后按**字符数**（不是字节）截断，超长时末尾换成省略号。
pub fn clip(text: &str, limit: usize) -> String {
    let folded: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() <= limit {
        return folded;
    }
    // limit == 0 时 Python 的 text[:-1] 会砍掉最后一个字符；这里等价地只留省略号。
    let keep = limit.saturating_sub(1);
    let mut out: String = folded.chars().take(keep).collect();
    out.push('…');
    out
}

/// 把 Task 的当前状态渲染成卡片。
///
/// footer 只写步数与花费，P0 不做耗时预估；`checklist_note` 的备注也挂在 footer 上 ——
/// 契约里卡片没有单独的备注字段，而「不新增消息」要求它必须落在这张卡片里面。
pub fn render_card(
    task: &Task,
    session: &Session,
    initiator: &str,
    status: CardStatus,
    note: Option<&str>,
) -> ChecklistCard {
    let mut footer = format!("已用 {} 步 · ¥{:.2}", task.steps, task.cost);
    if let Some(note) = note.filter(|n| !n.is_empty()) {
        footer = format!("{} · {}", clip(note, 40), footer);
    }
    let title = if task.title.is_empty() {
        "处理中"
    } else {
        task.title.as_str()
    };
    ChecklistCard {
        task_id: task.id.clone(),
        task_no: task.task_no.clone(),
        title: clip(title, MAX_TITLE_CHARS),
        initiator: if initiator.is_empty() {
            session.created_by.clone()
        } else {
            initiator.to_string()
        },
        // UTC、小时不补零（旧版就是这么渲染的，评测剧本按它对）
        started_at: format!("{}:{:02}", task.created_at.hour(), task.created_at.minute()),
        status,
        items: task
            .checklist
            .iter()
            .map(|i| ChecklistItemView {
                id: i.id.clone(),
                text: clip(&i.text, MAX_ITEM_CHARS),
                state: i.state,
                note: i.note.clone(),
            })
            .collect(),
        footer,
        actions: vec![aite_contracts::CardActionKind::Stop],
    }
}

/// 一个任务一个实例。保证 `send_card` 至多一次，`update_card` 按 W4 合并。
pub struct CardCoalescer {
    platform: Arc<dyn PlatformPort>,
    interval: f64,
    clock: Clock,
    card_id: Option<String>,
    pending: Option<ChecklistCard>,
    last_flush: f64,
}

impl CardCoalescer {
    pub fn new(platform: Arc<dyn PlatformPort>, min_interval_ms: u64, clock: Clock) -> Self {
        Self {
            platform,
            interval: min_interval_ms as f64 / 1000.0,
            clock,
            card_id: None,
            pending: None,
            last_flush: 0.0,
        }
    }

    pub fn card_id(&self) -> Option<&str> {
        self.card_id.as_deref()
    }

    pub fn sent(&self) -> bool {
        self.card_id.is_some()
    }

    /// 首次调用发卡片；之后是 no-op —— 「绝不出现第二条 send_card」靠这里保证。
    pub async fn ensure_card(
        &mut self,
        chat_id: &str,
        reply_to: Option<&str>,
        card: &ChecklistCard,
    ) -> Result<String, PlatformError> {
        if self.card_id.is_none() {
            let res = self.platform.send_card(chat_id, reply_to, card).await?;
            self.card_id = Some(res.card_id.unwrap_or(res.message_id));
            self.last_flush = (self.clock)();
            self.pending = None;
        }
        Ok(self.card_id.clone().unwrap_or_default())
    }

    /// 状态变了就调这个。是否真的推送由 500ms 窗口决定。
    pub async fn update(&mut self, card: ChecklistCard) -> Result<(), PlatformError> {
        if self.card_id.is_none() {
            return Ok(());
        }
        self.pending = Some(card);
        self.maybe_flush().await
    }

    pub async fn maybe_flush(&mut self) -> Result<(), PlatformError> {
        if self.card_id.is_none() || self.pending.is_none() {
            return Ok(());
        }
        if (self.clock)() - self.last_flush < self.interval {
            return Ok(());
        }
        self.flush().await
    }

    /// 任务结束时调用：无条件推一次。
    pub async fn force_flush(&mut self, card: Option<ChecklistCard>) -> Result<(), PlatformError> {
        if self.card_id.is_none() {
            return Ok(());
        }
        if card.is_some() {
            self.pending = card;
        }
        if self.pending.is_none() {
            return Ok(());
        }
        self.flush().await
    }

    async fn flush(&mut self) -> Result<(), PlatformError> {
        let card = match self.pending.take() {
            Some(c) => c,
            None => return Ok(()),
        };
        self.last_flush = (self.clock)();
        let card_id = self.card_id.clone().unwrap_or_default();
        self.platform.update_card(&card_id, &card).await
    }
}
