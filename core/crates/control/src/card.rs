//! `clip` 与 `render_card` —— 对应 `aite/worker/card.py` 的同名函数。
//!
//! 为什么控制面自己有一份：`cancel_task` 的「不在跑」分支要把卡片置 cancelled
//! （`plane.py` 里 `from ..worker.card import ... render_card`），而 `aite-worker` 是
//! 并行的另一轨（R5），并行期间跨轨依赖是禁止的（§3.2 末段）。两边输出必须一致，
//! 口径就写在这里的注释里，RΩ 合流时对一遍即可。
//!
//! 卡片合并（`CardCoalescer`）不在这里 —— 控制面只在取消时推一次，没有窗口可合并。
use aite_contracts::{CardActionKind, CardStatus, ChecklistCard, ChecklistItemView, Session, Task};
use chrono::Timelike;

/// `ChecklistCard.title`：「≤40 字，worker 截断」。
pub const MAX_TITLE_CHARS: usize = 40;
/// W9：checklist 每项 ≤20 字。
pub const MAX_ITEM_CHARS: usize = 20;

/// 折叠所有空白后按**字符数**截断，超长时留 `limit-1` 个字符再接一个省略号。
///
/// 与 Python `" ".join(text.split())` 逐条对齐：`split()` 不带参数会按任意空白切并丢掉
/// 空串，所以换行、制表、连续空格都会被压成单个空格，首尾空白直接消失。
/// `len()` 数的是码点，所以这里用 `chars().count()` 而不是 `len()`（字节）。
pub fn clip(text: &str, limit: usize) -> String {
    let folded: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() <= limit {
        return folded;
    }
    // limit 为 0 时 Python 会取 text[:-1]（去掉最后一个字符）再接省略号，那是没人会用的
    // 退化输入；这里按 saturating_sub 收敛成「只剩省略号」，不 panic。
    let keep = limit.saturating_sub(1);
    let mut out: String = folded.chars().take(keep).collect();
    out.push('…');
    out
}

/// 把 Task 的当前状态渲染成卡片。
///
/// 与 Python `render_card` 的逐条对齐（`inventory-core.md` §4「卡片合并」段）：
/// - footer `f"已用 {steps} 步 · ¥{cost:.2f}"`，有 note 时前置 `f"{clip(note,40)} · "`；
/// - `title` 空则 `"处理中"`，再 `clip(·, 40)`；
/// - `initiator` 空则退回 `session.created_by`；
/// - `started_at = f"{created_at.hour}:{created_at.minute:02d}"` —— **UTC、小时不补零**
///   （`inventory-core.md` §9 第 23 条钉住的现状，别顺手改成 `%H:%M`）；
/// - `items` 每项 `clip(text, 20)`；
/// - `actions` 取契约默认 `[stop]`。
pub fn render_card(
    task: &Task,
    session: &Session,
    initiator: &str,
    status: CardStatus,
    note: Option<&str>,
) -> ChecklistCard {
    let mut footer = format!("已用 {} 步 · ¥{:.2}", task.steps, task.cost);
    if let Some(note) = note.filter(|n| !n.is_empty()) {
        footer = format!("{} · {}", clip(note, MAX_TITLE_CHARS), footer);
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
        actions: vec![CardActionKind::Stop],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_folds_whitespace_and_counts_characters() {
        assert_eq!(clip("  a  b\n\tc ", 40), "a b c");
        assert_eq!(clip("一二三四五", 5), "一二三四五"); // 恰好等于上限不截
        assert_eq!(clip("一二三四五六", 5), "一二三四…"); // limit-1 个字符 + 省略号
        assert_eq!(clip("", 40), "");
    }

    #[test]
    fn clip_result_never_exceeds_the_limit() {
        for limit in 1..=8usize {
            let out = clip("超过限额的一段很长的文字", limit);
            assert!(out.chars().count() <= limit, "limit={limit} out={out}");
        }
    }
}
