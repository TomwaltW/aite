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

/// 给 User 轮署名（CC3 ④）：返回一份正文带 `[名字] ` 前缀的副本，**存库的正文不改**。
///
/// Assistant / SystemNote 与没有 `platform_user_id` 的 User 轮不加。名字怎么取由调用方给
/// （`name_of(platform_user_id)`：发起人用显示名、其余用群历史里的 `sender_name`、再不行用 id）。
/// 与 [`transcript_messages`] 分开两步：那个函数仍是纯截断。
pub fn attributed_turns(turns: &[Turn], name_of: impl Fn(&str) -> String) -> Vec<Turn> {
    turns
        .iter()
        .map(|t| match (&t.role, &t.platform_user_id) {
            (TurnRole::User, Some(uid)) => Turn {
                content: texts::attributed_line(&name_of(uid), &t.content),
                ..t.clone()
            },
            _ => t.clone(),
        })
        .collect()
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
