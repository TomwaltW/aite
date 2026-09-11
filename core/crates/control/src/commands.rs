//! `!` 命令的解析（§3.5 R5 / §3.3）。对应 `aite/control/commands.py`。
//!
//! 只做解析，不碰状态 —— 执行在 [`crate::plane`] 里。

/// 未知命令的回帖原文（`inventory-core.md` §5，逐字不变）。
pub const UNKNOWN_COMMAND_TEXT: &str = "未知命令，可用：!status !stop <任务号> !restart !new";

/// Python 里定义了但 plane 没用；这里照搬，语义同样是「文档用」。
pub const KNOWN_COMMANDS: [&str; 4] = ["!status", "!stop", "!restart", "!new"];

/// `"!stop #A17"` → `("!stop", "#A17")`。命令名统一小写，参数原样 trim。
///
/// 按**第一个空格**切（Python 的 `str.partition(" ")`）：`"!stop"` 之后要是跟了
/// 制表符，Python 也不会把它当分隔符，所以这里同样只认 U+0020。
pub fn parse_command(text: &str) -> (String, String) {
    let text = text.trim();
    match text.split_once(' ') {
        Some((head, rest)) => (head.to_lowercase(), rest.trim().to_string()),
        None => (text.to_lowercase(), String::new()),
    }
}

/// `"a17"` / `"A17"` / `"#a17"` 一律归成 `"#A17"`，对齐 `encode_task_no` 的输出形状。
pub fn normalize_task_no(raw: &str) -> String {
    let s = raw.trim().to_uppercase();
    if s.is_empty() {
        return String::new();
    }
    if s.starts_with('#') {
        s
    } else {
        format!("#{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_splits_on_the_first_space() {
        assert_eq!(parse_command("!stop #A17"), ("!stop".into(), "#A17".into()));
        assert_eq!(parse_command("!STATUS"), ("!status".into(), String::new()));
        assert_eq!(
            parse_command("  !restart   换个思路  "),
            ("!restart".into(), "换个思路".into())
        );
        assert_eq!(parse_command("!new a b c"), ("!new".into(), "a b c".into()));
    }

    #[test]
    fn normalize_task_no_adds_the_hash_and_upcases() {
        assert_eq!(normalize_task_no("a17"), "#A17");
        assert_eq!(normalize_task_no("A17"), "#A17");
        assert_eq!(normalize_task_no(" #a17 "), "#A17");
        assert_eq!(normalize_task_no(""), "");
        assert_eq!(normalize_task_no("   "), "");
    }

    #[test]
    fn known_commands_are_all_reachable_from_the_unknown_text() {
        for name in KNOWN_COMMANDS {
            assert!(
                UNKNOWN_COMMAND_TEXT.contains(name),
                "{name} 该出现在未知命令的提示里"
            );
        }
    }
}
