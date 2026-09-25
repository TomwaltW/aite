//! 本会话 transcript（W1）。CC3 从 `context.rs` 原样搬来。
use aite_contracts::{Message, Role, Turn, TurnRole};

use crate::texts;

/// 超过这个数才截断
pub const MAX_TRANSCRIPT_TURNS: usize = 40;
/// 保留最前面几轮（原始诉求通常在这里）
pub const HEAD_TURNS: usize = 2;
/// 保留最近几轮
pub const TAIL_TURNS: usize = 30;

fn role_of(turn: TurnRole) -> Role {
    match turn {
        TurnRole::User => Role::User,
        TurnRole::Assistant => Role::Assistant,
        TurnRole::SystemNote => Role::System,
    }
}

/// 本会话 transcript。超过 40 轮时保留前 2 轮 + 最近 30 轮 + 一条省略说明。
pub fn transcript_messages(turns: &[Turn]) -> Vec<Message> {
    let (kept, omitted): (Vec<&Turn>, usize) = if turns.len() <= MAX_TRANSCRIPT_TURNS {
        (turns.iter().collect(), 0)
    } else {
        let mut kept: Vec<&Turn> = turns[..HEAD_TURNS].iter().collect();
        kept.extend(turns[turns.len() - TAIL_TURNS..].iter());
        (kept, turns.len() - HEAD_TURNS - TAIL_TURNS)
    };

    let mut out = Vec::with_capacity(kept.len() + 1);
    for (i, t) in kept.iter().enumerate() {
        if omitted > 0 && i == HEAD_TURNS {
            out.push(Message::text(Role::System, texts::omitted_turns(omitted)));
        }
        out.push(Message::text(role_of(t.role), t.content.clone()));
    }
    out
}
