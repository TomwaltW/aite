//! §3.3 硬约束之一：**给人看的固定文案逐字不变**（`inventory-core.md` §5 列全了）。
//!
//! 评测的 `text contains` 断言和 M1–M6 的验收剧本都依赖这些字符串，所以这一份把控制面
//! 会吐出来的每一句原样钉死。Python 树在 RΩ 之后会被删掉，那之后这里就是唯一的对照物 ——
//! 每条后面都标了它在 `aite/control/plane.py` / `commands.py` 里的出处。
//!
//! Rust 侧的补充用例（Python 没有对应文件：那边靠 `test_commands.py` 的零散断言覆盖）。
mod support;

use aite_contracts::{ControlPlane, EventKind, SessionStore, TaskStatus};
use aite_control::{
    NO_ACTIVE_TASK_TEXT, NO_SUCH_TASK_TEXT, RESTART_EMPTY_TEXT, ROUTE_NEW_TASK, ROUTE_STEER,
    UNKNOWN_COMMAND_TEXT,
};
use support::{CHAT, Harness, ROOT, active_tasks, ev};

/// 四条模块常量（`plane.py` 的 `NO_SUCH_TASK_TEXT` 等 + `commands.py`）。
#[test]
fn module_constants_are_byte_exact() {
    assert_eq!(NO_SUCH_TASK_TEXT, "没有这个任务");
    assert_eq!(NO_ACTIVE_TASK_TEXT, "本群没有活跃任务");
    assert_eq!(RESTART_EMPTY_TEXT, "已重开会话，请直接说要做什么。");
    assert_eq!(
        UNKNOWN_COMMAND_TEXT,
        "未知命令，可用：!status !stop <任务号> !restart !new"
    );
    assert_eq!(ROUTE_NEW_TASK, "new_task");
    assert_eq!(ROUTE_STEER, "steer");
}

/// `!status` 的两种正文 + `_dropped_note()` 那句警告。
#[tokio::test]
async fn status_body_and_dropped_note_are_byte_exact() {
    let h = Harness::new();
    h.store.fail_next("seen_event", 1);
    let plane = h.plane();

    // 先丢一条，让 `_dropped_note` 有话说
    plane
        .handle_event(ev().id("ev-boom").text("被吃掉的那句").build())
        .await
        .expect_err("该把错误传上来");

    // 无活跃任务
    plane
        .handle_event(ev().id("ev-s1").text("!status").message_id("om_s1").build())
        .await
        .expect("命令");
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务\n⚠ 本进程启动以来有 1 条事件没接住（多半是存储异常），\
可能有消息没被处理。翻日志看 ingress.handle_failed。"
    );

    // 有活跃任务：`{task_no} {status} {title}`，标题空则 `(无标题)`
    plane
        .handle_event(ev().id("ev-t").text("画个趋势图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    plane
        .handle_event(ev().id("ev-s2").text("!status").message_id("om_s2").build())
        .await
        .expect("命令");
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        format!(
            "本群活跃任务：\n{} created 画个趋势图\n⚠ 本进程启动以来有 1 条事件没接住（多半是存储异常），\
可能有消息没被处理。翻日志看 ingress.handle_failed。",
            task.task_no
        )
    );

    // 标题为空 → `(无标题)`
    task.title = String::new();
    h.store.update_task(&task).await.expect("回写空标题");
    plane
        .handle_event(ev().id("ev-s3").text("!status").message_id("om_s3").build())
        .await
        .expect("命令");
    let body = h.platform.last_text().expect("该回帖").text;
    assert!(
        body.contains(&format!("{} created (无标题)", task.task_no)),
        "{body}"
    );
}

/// `!stop` 的回帖、`!restart` 的三态、`!new` 的两态。
#[tokio::test]
async fn command_replies_are_byte_exact() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一版方案").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    // !stop → `任务 {task_no} 已停止。`
    plane
        .handle_event(
            ev().id("e-stop")
                .text(&format!("!stop {}", task.task_no))
                .message_id("om_stop")
                .build(),
        )
        .await
        .expect("停");
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        format!("任务 {} 已停止。", task.task_no)
    );

    // !restart（无文本、有 1 个在跑的任务）→ note + RESTART_EMPTY_TEXT
    plane
        .handle_event(ev().id("e2").text("再来一版").message_id("om_2").build())
        .await
        .expect("建任务");
    plane
        .handle_event(
            ev().id("e-restart")
                .text("!restart")
                .mentioned(false)
                .message_id("om_3")
                .thread("om_2")
                .build(),
        )
        .await
        .expect("重开");
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "终止了 1 个进行中的任务。已重开会话，请直接说要做什么。"
    );

    // !restart <文本>（有在跑的任务）→ `已重开会话，` + note
    plane
        .handle_event(ev().id("e4").text("第三版").message_id("om_4").build())
        .await
        .expect("建任务");
    plane
        .handle_event(
            ev().id("e-restart2")
                .text("!restart 换个思路")
                .mentioned(false)
                .message_id("om_5")
                .thread("om_4")
                .build(),
        )
        .await
        .expect("重开");
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "已重开会话，终止了 1 个进行中的任务。"
    );

    // !new（无文本）→ RESTART_EMPTY_TEXT
    let before = h.platform.texts().len();
    plane
        .handle_event(ev().id("e-new").text("!new").message_id("om_6").build())
        .await
        .expect("新开");
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "已重开会话，请直接说要做什么。"
    );
    assert_eq!(h.platform.texts().len(), before + 1);

    // !new <文本> → 不回帖（§9 第 3 条）
    plane
        .handle_event(
            ev().id("e-new2")
                .text("!new 另起一件事")
                .message_id("om_7")
                .build(),
        )
        .await
        .expect("新开");
    assert_eq!(
        h.platform.texts().len(),
        before + 1,
        "`!new <文本>` 不该回帖"
    );
}

/// `!restart <文本>` 且没停掉任何任务 → 一个字都不回（§9 第 3 条的前半句）。
#[tokio::test]
async fn restart_with_text_and_nothing_stopped_says_nothing() {
    let h = Harness::new();
    let plane = h.plane();
    // 先建一个会话再让它的任务落终态，这样 restart 时 session 在、但没有活跃任务
    plane
        .handle_event(ev().id("e1").text("第一版").message_id(ROOT).build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.status = TaskStatus::Delivered;
    h.store.update_task(&task).await.expect("落终态");

    plane
        .handle_event(
            ev().id("e-restart")
                .text("!restart 换个思路")
                .mentioned(false)
                .message_id("om_2")
                .thread(ROOT)
                .build(),
        )
        .await
        .expect("重开");

    assert!(
        h.platform.texts().is_empty(),
        "实际回了：{:?}",
        h.platform.texts()
    );
}

/// R3 evidence 按钮：`任务 {task_id} 的证据目录：{task_dir}`。
#[tokio::test]
async fn evidence_reply_is_byte_exact() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(
            ev().id("ev-card")
                .kind(EventKind::CardAction)
                .text("")
                .message_id("om_9")
                .card_action(support::card_action(
                    "om_card_1",
                    aite_contracts::CardActionKind::Evidence,
                    Some("t-123"),
                ))
                .build(),
        )
        .await
        .expect("证据按钮");

    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        format!(
            "任务 t-123 的证据目录：{}/t-123",
            h.config.storage.evidence_dir
        )
    );
}

/// R4 编辑写进 transcript 的那条 `system_note`。
#[tokio::test]
async fn edit_note_is_byte_exact() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let session_id = active_tasks(&h.store, CHAT).await.remove(0).session_id;

    plane
        .handle_event(
            ev().id("ev-edit")
                .kind(EventKind::MessageEdited)
                .text("@Aite 画个柱状图")
                .message_id(ROOT)
                .build(),
        )
        .await
        .expect("编辑");

    let turns = h
        .store
        .list_turns(&session_id, 200)
        .await
        .expect("读 turns");
    assert_eq!(
        turns.last().expect("有").content,
        "[用户修改了消息] 新内容：@Aite 画个柱状图"
    );
}

/// R4 的其余两种事件只记审计账（计数器 key 是拼出来的，§9 第 2 条）。
#[tokio::test]
async fn audit_only_events_bump_their_own_counter() {
    let h = Harness::new();
    let plane = h.plane();

    plane
        .handle_event(
            ev().id("ev-bot")
                .kind(EventKind::BotAdded)
                .text("")
                .message_id("om_b")
                .build(),
        )
        .await
        .expect("bot_added");
    plane
        .handle_event(
            ev().id("ev-member")
                .kind(EventKind::MemberChanged)
                .text("")
                .message_id("om_m")
                .build(),
        )
        .await
        .expect("member_changed");

    assert_eq!(plane.counter("events.bot_added"), 1);
    assert_eq!(plane.counter("events.member_changed"), 1);
    assert!(active_tasks(&h.store, CHAT).await.is_empty());
    assert!(h.platform.texts().is_empty());
}

/// 缺 `card_action` 的 card_action 事件 → `events.bad_card_action`，不回帖。
#[tokio::test]
async fn a_card_action_without_a_payload_is_counted_and_dropped() {
    let h = Harness::new();
    let plane = h.plane();

    plane
        .handle_event(
            ev().id("ev-bad")
                .kind(EventKind::CardAction)
                .text("")
                .message_id("om_9")
                .build(), // 没有 card_action
        )
        .await
        .expect("坏卡片事件");

    assert_eq!(plane.counter("events.bad_card_action"), 1);
    assert!(h.platform.texts().is_empty());
}

/// `counters()` 是快照：key 拼出来的那些也要在里面（`commands!status` / `events.*`）。
#[tokio::test]
async fn counters_snapshot_carries_the_composed_keys() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("!status").build())
        .await
        .expect("命令");
    plane
        .handle_event(
            ev().id("e2")
                .text("大家早")
                .mentioned(false)
                .message_id("om_2")
                .build(),
        )
        .await
        .expect("闲聊");

    let snapshot = plane.counters();
    assert_eq!(
        snapshot.get("commands!status").and_then(|v| v.as_i64()),
        Some(1)
    );
    assert_eq!(
        snapshot.get("events.ignored").and_then(|v| v.as_i64()),
        Some(1)
    );
    assert!(
        !snapshot.contains_key("events.dropped"),
        "没碰过的 key 不该出现在快照里：{snapshot:?}"
    );
}

/// `!stop` 撞上交付中的任务时那句话，逐字钉住。
///
/// 它是 W2 新加的一句（V5 留下那半截的收口）：`!status` 列得出来的任务，
/// `!stop` 不许再回「没有这个任务」。M 剧本的排障表引的就是这句原话。
#[test]
fn stop_while_delivering_text_is_byte_exact() {
    assert_eq!(
        aite_control::stop_while_delivering_text("#A17"),
        "任务 #A17 正在把答复发给你，停不了了。"
    );
}
