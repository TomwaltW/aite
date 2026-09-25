//! `!` 命令：解析（原 `commands.rs`）+ 注册表 + 分发。对应 `aite/control/commands.py`。
//!
//! CC2 起每条命令一个文件（`commands/<名字>.rs`），各自带 `pub(crate) const ENABLED: bool`
//! 与统一签名的 `run`。下面的分发一次写全 20 条：**以后启用一条命令只改它自己的文件**。
//! 停用的命令走「未知命令」那条路，计数器仍是 `commands!<名字>`。
use std::collections::HashSet;

use serde_json::Value;

use aite_contracts::{EvidenceKind, IngressError, NormalizedEvent, Session, Task};

use crate::evidence_log::{ROUTE_COMMAND, event_payload};

use crate::lock;
use crate::plane::InProcessControlPlane;

mod about;
mod access;
mod approve;
mod configure;
mod connect;
mod evidence;
mod feedback;
mod fork;
mod help;
mod memory;
mod model;
mod mute;
mod new;
mod reject;
mod restart;
mod routines;
mod status;
mod stop;
mod unmute;
mod usage;

pub use restart::{RESTART_EMPTY_TEXT, restart_while_delivering_text};
pub use stop::{
    NO_ACTIVE_TASK_TEXT, NO_SUCH_TASK_TEXT, stop_needs_task_no_text, stop_while_delivering_text,
};

/// 未知命令的回帖原文（CC2 ⑤ 解冻：原来逐条列四个命令，现在指路 `!help`，
/// 因为可用的命令由注册表决定、会随各轨启用而变，写死在这一句里迟早对不上）。
pub const UNKNOWN_COMMAND_TEXT: &str = "未知命令，发 !help 看全部命令";

/// 中文别名 → 规范名（CC2 ⑤）。**只在带 `!` / `！` 时生效**：群里说一句「状态」不是命令。
/// 计数器 key 用规范名（`commands!status`）。
pub const ALIASES: [(&str, &str); 5] = [
    ("状态", "!status"),
    ("停止", "!stop"),
    ("重开", "!restart"),
    ("新话题", "!new"),
    ("帮助", "!help"),
];

/// 注册表里的一条（CC2 ⑤）：`!help` 与分发都以各命令文件自己的 `ENABLED` 为准。
pub(crate) struct CommandEntry {
    pub(crate) name: &'static str,
    pub(crate) enabled: bool,
    pub(crate) help: &'static str,
}

/// 全部 20 条命令，`!help` 按这个顺序列出启用的那些。
pub(crate) fn registry() -> [CommandEntry; 20] {
    macro_rules! entry {
        ($m:ident, $name:literal) => {
            CommandEntry {
                name: $name,
                enabled: $m::ENABLED,
                help: $m::HELP,
            }
        };
    }
    [
        entry!(status, "!status"),
        entry!(stop, "!stop"),
        entry!(restart, "!restart"),
        entry!(new, "!new"),
        entry!(help, "!help"),
        entry!(about, "!about"),
        entry!(access, "!access"),
        entry!(configure, "!configure"),
        entry!(mute, "!mute"),
        entry!(unmute, "!unmute"),
        entry!(feedback, "!feedback"),
        entry!(routines, "!routines"),
        entry!(fork, "!fork"),
        entry!(memory, "!memory"),
        entry!(approve, "!approve"),
        entry!(reject, "!reject"),
        entry!(evidence, "!evidence"),
        entry!(connect, "!connect"),
        entry!(usage, "!usage"),
        entry!(model, "!model"),
    ]
}

/// 这条消息长得像不像命令（R5 的前半个条件）：去掉首尾空白后以 `!` 或全角 `！` 开头。
pub fn is_command(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with('!') || text.starts_with('！')
}

/// `"!stop #A17"` → `("!stop", "#A17")`。命令名统一小写，参数原样 trim。
///
/// CC2 ⑤ 起：认全角 `！`（U+FF01，与 ASCII `!` 等价）；按**第一个 Unicode 空白**切
/// （`char::is_whitespace`：U+3000 全角空格、制表符都算），中文输入法下打出来的命令也认得；
/// 名字命中 [`ALIASES`] 就换成规范名。
pub fn parse_command(text: &str) -> (String, String) {
    let text = text.trim();
    let text = match text.strip_prefix('！') {
        Some(tail) => format!("!{tail}"),
        None => text.to_string(),
    };
    let (head, rest) = match text.split_once(char::is_whitespace) {
        Some((head, rest)) => (head.to_lowercase(), rest.trim().to_string()),
        None => (text.to_lowercase(), String::new()),
    };
    let head = head
        .strip_prefix('!')
        .and_then(|bare| ALIASES.iter().find(|(alias, _)| *alias == bare))
        .map(|(_, name)| (*name).to_string())
        .unwrap_or(head);
    (head, rest)
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

/// `!stop` / 卡片 stop 按钮找目标的结局。
///
/// **「找不着」不是一件事，是三件。** BB1 之前它们压在同一格 `NotFound` 上、
/// 共用一句「没有这个任务」，而用户那头看到的是三种完全不同的处境：本群一个活跃任务
/// 都没有（那该说的和 `!status` 是同一句）、省略了任务号而本群有好几个（那缺的是
/// 「指哪一个」，不是「有没有」）、给了任务号但对不上（这一格「没有这个任务」才是对的）。
/// 中间那一格最伤：用户刚在 `!status` 里看见三个任务，敲一句 `!stop` 被告知
/// 「没有这个任务」—— 与 V5 留下那半截是同一种自相矛盾，只是换了个入口。
///
/// 所以 `NoneActive` / `Ambiguous` 从 `NotFound` 里分出来。**「查哪些任务」一个字没动**
/// （仍是 `status_tasks`，W2 收的那个口），分出来的只是「找不着之后说什么」。
///
/// **「交付中的任务停不了」是刻意的约定，不是查不到。** 理由在 worker 那边：
/// 取消标志位只在每一步的**开头**被看一眼（`worker/src/agent.rs` 里
/// `if (hooks.is_cancelled)()` 是全仓唯一一处），而 `deliver()` 是最后一步之后的一段直路 ——
/// 逐个 `send_file` → `send_text` → 写 delivered 证据 → 收卡片 → `finish()`，
/// 中间**一个取消点都没有**。
///
/// 所以「让 `!stop` 跟上、真去停它」那条路是走不通的，它只会造出三样坏东西：
/// 1. 文件和答复该发的照发 —— 用户已经收到了，却被告知「已停止」；
/// 2. `cancel_task` 写进去的 `Cancelled` 随后被 `finish()` 的 `Delivered` 盖掉，
///    库里最终是 `delivered`，回帖却说停了，两边对不上；
/// 3. V5 那道「落刀前按 id 重读、终态不改写」的防线在这里**兜不住** ——
///    `Answering` 不是终态，重读出来照样往下走。
///
/// 于是本轨选的是把它写成明面上的约定：查得到（口径与 `!status` 完全一致），
/// 但回一句说得通的话（`stop_while_delivering_text`），而不是「没有这个任务」。
pub(crate) enum StopTarget {
    /// 可停：还在活跃口径里（created / planning / working），worker 每步开头会看标志位。
    Stoppable(Task),
    /// 找得到，但停不了：已经走进 `deliver()`。
    Delivering(Task),
    /// 本群一个活跃任务都没有 —— 这时该说的和 `!status` 是同一句。
    ///
    /// 只有 `!stop` 省略任务号那条路给得出这一格：**给了任务号就按任务号回答**
    /// （即使列表是空的也回「没有这个任务」），没给才按群回答。卡片那条路
    /// （`resolve_task`）永远给不出它，它按 `task_id` / `card_id` 找，没有「省略」这个形状。
    NoneActive,
    /// 省略了任务号，而本群不止一个活跃任务：指不到唯一的那一个。
    ///
    /// 带着候选走，不是只带个数 —— 回话要把可选的列出来（`stop_needs_task_no_text`），
    /// 而那份列表必须和 `!status` 同源同序，不能让调用方再查一遍。
    Ambiguous(Vec<Task>),
    /// 给了任务号，本群这份列表里对不上。**这一格「没有这个任务」是对的**，不动。
    NotFound,
}

impl StopTarget {
    fn of(task: Option<Task>) -> Self {
        match task {
            None => Self::NotFound,
            Some(task) => Self::of_existing(task),
        }
    }

    /// 一个**确定存在**的任务能不能停。
    ///
    /// 单独拆出来是给 `cmd_restart` 用的：它手里的任务是从 `status_tasks` 逐条取出来的，
    /// 没有「找不到」那一格。两条命令因此共用**同一条**分流规则 ——
    /// 「什么算活跃」曾经有两种意见，那正是 AA2 这笔账的由来，不该再长出第三种。
    fn of_existing(task: Task) -> Self {
        if task.status.is_active() {
            return Self::Stoppable(task);
        }
        // 非活跃、又没到终态（`status_tasks` 会把终态滤掉）—— P0 里只可能是 `Answering`。
        // `AwaitingApproval` 那一格全仓没有任何一处写得进去，只活在契约枚举和
        // `frozen_values.rs` 里；真有一天用上了，它同样不该被「没有这个任务」打发掉。
        Self::Delivering(task)
    }
}

impl InProcessControlPlane {
    /// 命令作用到一个具体任务时，在它的链上记一条 `event_received`（`route = "command"` +
    /// `command`，CC2 ⑨），让「谁、用哪条命令停的」在证据里查得到。
    ///
    /// **只给真会走 `cancel_task` 的目标写**（`!stop` 的 `Stoppable`、`!restart` 计进 `stopped`
    /// 的那些）；`Delivering` 一律不写 —— 它马上会 `finish()` 落 manifest，写在后面会让
    /// manifest 的 `root_hash` / `event_count` 过期。调用方负责先写这条、再 `cancel_task`。
    ///
    /// 写之前按 id 重读：已是终态或已经 finalize（有 `evidence_root_hash`）就跳过。重读与追加
    /// 之间在跑的任务仍可能恰好收尾 —— 这个残余窗口要动 worker 才修得掉（CC3 的面），这里不修。
    /// 写失败只记日志：命令本身已经认了，不能因为一条证据写不进去就不停。
    pub(crate) async fn record_command(&self, ev: &NormalizedEvent, task: &Task, command: &str) {
        match self.store.get_task(&task.id).await {
            Ok(Some(fresh))
                if !fresh.status.is_terminal()
                    && fresh
                        .evidence_root_hash
                        .as_deref()
                        .is_none_or(|h| h.is_empty()) => {}
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(target: "aite.control", task = %task.id, error = %e, "control.command_evidence_reread_failed");
                return;
            }
        }
        let mut payload = event_payload(ev, ROUTE_COMMAND, None);
        payload.insert("command".into(), Value::String(command.into()));
        if let Err(e) = self
            .evidence
            .append(&task.id, EvidenceKind::EventReceived, payload)
            .await
        {
            tracing::warn!(target: "aite.control", task = %task.id, error = %e, "control.command_evidence_failed");
        }
    }

    // ---- R5 --------------------------------------------------------------

    /// 分发（R5）。注册表：每条命令的 `ENABLED` 在它自己的文件里；停用的走「未知命令」。
    pub(crate) async fn on_command(
        &self,
        ev: &NormalizedEvent,
        session: Option<Session>,
        text: &str,
    ) -> Result<(), IngressError> {
        let (name, rest) = parse_command(text);
        // 计数器 key 是拼出来的：`commands!status`（§9 第 2 条）
        self.shared.bump(&format!("commands{name}"));

        match name.as_str() {
            "!status" if status::ENABLED => status::run(self, ev, session, &rest).await,
            "!stop" if stop::ENABLED => stop::run(self, ev, session, &rest).await,
            "!restart" if restart::ENABLED => restart::run(self, ev, session, &rest).await,
            "!new" if new::ENABLED => new::run(self, ev, session, &rest).await,
            "!help" if help::ENABLED => help::run(self, ev, session, &rest).await,
            "!about" if about::ENABLED => about::run(self, ev, session, &rest).await,
            "!access" if access::ENABLED => access::run(self, ev, session, &rest).await,
            "!configure" if configure::ENABLED => configure::run(self, ev, session, &rest).await,
            "!mute" if mute::ENABLED => mute::run(self, ev, session, &rest).await,
            "!unmute" if unmute::ENABLED => unmute::run(self, ev, session, &rest).await,
            "!feedback" if feedback::ENABLED => feedback::run(self, ev, session, &rest).await,
            "!routines" if routines::ENABLED => routines::run(self, ev, session, &rest).await,
            "!fork" if fork::ENABLED => fork::run(self, ev, session, &rest).await,
            "!memory" if memory::ENABLED => memory::run(self, ev, session, &rest).await,
            "!approve" if approve::ENABLED => approve::run(self, ev, session, &rest).await,
            "!reject" if reject::ENABLED => reject::run(self, ev, session, &rest).await,
            "!evidence" if evidence::ENABLED => evidence::run(self, ev, session, &rest).await,
            "!connect" if connect::ENABLED => connect::run(self, ev, session, &rest).await,
            "!usage" if usage::ENABLED => usage::run(self, ev, session, &rest).await,
            "!model" if model::ENABLED => model::run(self, ev, session, &rest).await,
            _ => self.reply(ev, UNKNOWN_COMMAND_TEXT).await,
        }
    }

    /// `!status` 要列的任务。
    ///
    /// 库里那一半是 `list_active_tasks`（口径 `status in ACTIVE_TASK_STATUSES` =
    /// created / planning / working）。那三个值在契约里冻结，还被两条冻结测试逐值钉着
    /// （`contracts` 的 `frozen_values.rs` 与 `roundtrip.rs`），**不动它**。
    ///
    /// **另一半是控制面自己的 `running`。** 漏的是这一条路：worker 的 `deliver()` 在
    /// 「第一步就 final、一张卡片都没发过」那一路把状态落成 `Answering`
    /// （`worker/src/agent.rs` 里 `let answering = !ctx.card.sent()`），而 `Answering`
    /// **不在**活跃口径里 —— `roundtrip.rs` 专门有一条断言钉着 `!Answering.is_active()`。
    /// 于是从落 `Answering` 到 `finish()` 落 `Delivered` 之间那几笔（逐个发产物、
    /// send_text、写 delivered 证据、收卡片）任务从 `!status` 里整个消失，
    /// 用户这时问一句，得到的是「本群没有活跃任务」。
    ///
    /// Python 原版一模一样（`store.py` 的 ACTIVE_TASK_STATUSES 同三值，`plane.py` 的
    /// `!status` 同样只走 `list_active_tasks`），所以这不是 Rust 引入的偏差。补它也
    /// **不需要动契约**：`!status` 本来就该回答「现在到底什么情况」，一个正在把文件
    /// 发给你的任务不该从这句话里消失。
    ///
    /// **取锁**：只取 `running` 一把，抄完即放，之后才 await（std 的 Mutex 不许跨 await）。
    /// 全程没有第二把，`Shared` 那条全局锁序（cancelled → owned → running → steer →
    /// counters）自然成立。
    pub(crate) async fn status_tasks(&self, chat_id: &str) -> Result<Vec<Task>, IngressError> {
        let mut tasks = self.store.list_active_tasks(chat_id).await?;
        let extra: Vec<String> = {
            let listed: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
            lock(&self.shared.running)
                .iter()
                .filter(|id| !listed.contains(id.as_str()))
                .cloned()
                .collect()
        };
        for task_id in extra {
            let Some(task) = self.store.get_task(&task_id).await? else {
                continue;
            };
            // 抄的是快照：`RunningGuard` 摘条目和 worker 落终态之间有先后，
            // 已经收场的不该被这句话重新拉出来。
            if task.status.is_terminal() {
                continue;
            }
            // `running` 是进程级的，别把别的群的任务串进这一句。
            let same_chat = self
                .store
                .get_session(&task.session_id)
                .await?
                .is_some_and(|s| s.chat_id == chat_id);
            if !same_chat {
                continue;
            }
            tasks.push(task);
        }
        // 与 §3.2 给 `list_active_tasks` 的承诺同序：(created_at, id) 正序。
        tasks.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(tasks)
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

    /// CC2 ⑤ 翻转：原来断言四个命令都写在未知命令那句里（`KNOWN_COMMANDS` 已由注册表取代）；
    /// 现在那句只指路 `!help`，所以要钉的是「`!help` 在、而且是启用的」。
    #[test]
    fn unknown_text_points_to_an_enabled_help() {
        assert!(UNKNOWN_COMMAND_TEXT.contains("!help"));
        assert!(registry().iter().any(|c| c.name == "!help" && c.enabled));
    }

    #[test]
    fn registry_names_match_their_help_lines() {
        for c in registry() {
            assert!(
                c.help.starts_with(c.name),
                "{} 的 HELP 该以命令名开头",
                c.name
            );
        }
    }
}
