//! T2 独有验收：`!status` / `!stop` / `!restart` / `!new` 四条命令（§3.5 R5 + §3.3）。
//! 对应 `tests/control/test_commands.py`（10 条）。
mod support;

use std::sync::Arc;

use aite_contracts::{
    CardStatus, ControlPlane, EvidenceKind, SessionStatus, SessionStore, TaskStatus,
};
use support::{
    CHAT, Harness, ROOT, RunningPlane, ScriptedWorker, WorkerAction, active_tasks, ev, turn_texts,
    within,
};

use aite_control::{
    InProcessControlPlane, UNKNOWN_COMMAND_TEXT, restart_while_delivering_text,
    stop_needs_task_no_text, stop_while_delivering_text,
};

async fn cmd(plane: &Arc<InProcessControlPlane>, text: &str, message_id: &str) {
    plane
        .handle_event(
            ev().id(&format!("ev-{message_id}"))
                .text(text)
                .mentioned(true)
                .message_id(message_id)
                .build(),
        )
        .await
        .expect("命令不该报错");
}

async fn cmd_in_thread(plane: &Arc<InProcessControlPlane>, text: &str, message_id: &str) {
    plane
        .handle_event(
            ev().id(&format!("ev-{message_id}"))
                .text(text)
                .mentioned(false)
                .message_id(message_id)
                .thread(ROOT)
                .build(),
        )
        .await
        .expect("命令不该报错");
}

// ---- !status --------------------------------------------------------------

#[tokio::test]
async fn status_empty() {
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "!status", "om_9").await;
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务"
    );
    assert_eq!(plane.counter("commands!status"), 1);
}

#[tokio::test]
async fn status_lists_active_tasks() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("画个趋势图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    cmd(&plane, "!status", "om_9").await;

    let body = h.platform.last_text().expect("该回帖").text;
    assert!(body.contains(&task.task_no), "{body}");
    assert!(body.contains("画个趋势图"), "{body}");
    assert!(body.contains("created"), "{body}");
}

#[tokio::test]
async fn status_is_scoped_to_this_chat() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(
            ev().id("e1")
                .text("别的群的活")
                .message_id("om_x")
                .chat("oc_other")
                .build(),
        )
        .await
        .expect("别的群建任务");

    cmd(&plane, "!status", "om_9").await;

    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务"
    );
}

/// 正在交付中的任务不许从 `!status` 里消失（审核记账 4.2 第 3 行）。
///
/// worker 的 `deliver()` 在「第一步就 final、一张卡片都没发过」那一路把状态落成
/// `Answering`（`agent.rs` 里 `let answering = !ctx.card.sent()`），而 `Answering`
/// **不在** `ACTIVE_TASK_STATUSES` 里 —— contracts 的 `roundtrip.rs` 有一条断言专门
/// 钉着 `!Answering.is_active()`，`frozen_values.rs` 又逐值钉着那三个值，所以
/// 「把 Answering 加进活跃口径」这条路是双重关死的，不能走。
///
/// 补的是另一半：控制面自己的 `running` 知道谁在 worker 手上。从落 `Answering` 到
/// `finish()` 落 `Delivered` 之间那几笔（逐个发产物、send_text、写 delivered 证据、
/// 收卡片）本来会让任务整个消失，用户这时问一句，得到的是「本群没有活跃任务」。
#[tokio::test]
async fn status_still_lists_a_task_that_is_answering() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::WaitForCancel],
    );
    let plane = h.plane_builder().worker(worker.clone()).build();
    plane
        .handle_event(
            ev().id("e1")
                .text("算一下上周退款")
                .message_id(ROOT)
                .build(),
        )
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    let _running = RunningPlane::start(plane.clone());
    within("等 worker 接手", async {
        while worker.calls().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(plane.state().running, vec![task.id.clone()]);

    // worker 走到了 deliver()：状态落 Answering，正在往群里发东西
    let mut answering = h.store.get_task(&task.id).await.expect("读").expect("有");
    answering.status = TaskStatus::Answering;
    h.store.update_task(&answering).await.expect("回写");
    assert!(
        active_tasks(&h.store, CHAT).await.is_empty(),
        "前提：库的活跃口径这时确实看不见它"
    );

    cmd(&plane, "!status", "om_9").await;

    let body = h.platform.last_text().expect("该回帖").text;
    assert!(
        body.contains(&task.task_no),
        "正在把答复发给你的任务不该从 !status 里消失：{body}"
    );
    assert!(body.contains("answering"), "{body}");
    assert!(body.contains("算一下上周退款"), "{body}");
}

/// `running` 是**进程级**的，别把别的群的任务串进这一句。
#[tokio::test]
async fn status_does_not_leak_a_running_task_from_another_chat() {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::WaitForCancel],
    );
    let plane = h.plane_builder().worker(worker.clone()).build();
    plane
        .handle_event(
            ev().id("e1")
                .text("别的群的活")
                .message_id("om_x")
                .chat("oc_other")
                .build(),
        )
        .await
        .expect("别的群建任务");
    let other = active_tasks(&h.store, "oc_other").await.remove(0);

    let _running = RunningPlane::start(plane.clone());
    within("等 worker 接手", async {
        while worker.calls().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await;

    let mut answering = h.store.get_task(&other.id).await.expect("读").expect("有");
    answering.status = TaskStatus::Answering;
    h.store.update_task(&answering).await.expect("回写");

    cmd(&plane, "!status", "om_9").await;

    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务"
    );
}

// ---- !stop ----------------------------------------------------------------

#[tokio::test]
async fn stop_cancels_and_releases_sandbox() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("跑个长活").message_id(ROOT).build())
        .await
        .expect("建任务");
    let mut task = active_tasks(&h.store, CHAT).await.remove(0);
    task.card_id = Some("om_card_1".into());
    task.sandbox_id = Some("sb_x".into());
    h.store.update_task(&task).await.expect("回写");

    cmd(&plane, &format!("!stop {}", task.task_no), "om_9").await;

    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
    assert_eq!(h.sandbox.released(), vec!["sb_x".to_string()]);
    let (card_id, card) = h.platform.card_updates().pop().expect("该更新卡片");
    assert_eq!(card_id, "om_card_1");
    assert_eq!(card.status, CardStatus::Cancelled);
    assert!(
        h.platform
            .last_text()
            .expect("该回帖")
            .text
            .contains(&task.task_no)
    );
    // 「不在跑」分支该把 Gateway 手上的沙箱也还了、链上留一条 cancelled、manifest 收口
    assert_eq!(h.gateway.released(), vec![task.id.clone()]);
    assert!(
        h.evidence
            .kinds(&task.id)
            .contains(&aite_contracts::EvidenceKind::Cancelled)
    );
    assert!(h.evidence.manifest(&task.id).is_some());
    assert!(saved.evidence_root_hash.is_some(), "root_hash 该进库");
}

#[tokio::test]
async fn stop_accepts_task_no_without_hash() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("活儿").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    let bare = task.task_no.trim_start_matches('#').to_lowercase();
    cmd(&plane, &format!("!stop {bare}"), "om_9").await;

    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
}

/// 第三格（BB1 三格里唯一不动的那个）：**给了任务号、但对不上** —— 这句在这一格是对的。
///
/// 它同时钉住那条分界：空列表那一格只归省略任务号那条路。这里本群一个任务都没有，
/// 可用户指着 `#A99` 问，答的就该是这个号的下落，不是「本群没有活跃任务」。
#[tokio::test]
async fn stop_unknown_task() {
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "!stop #A99", "om_9").await;
    assert_eq!(h.platform.last_text().expect("该回帖").text, "没有这个任务");
}

/// 第一格：**本群一个活跃任务都没有** —— `!stop` 和 `!status` 该说同一句话。
///
/// 改之前 `!status` 回「本群没有活跃任务」、`!stop` 回「没有这个任务」：后者说的是
/// 「你要的那个不存在」，可用户压根没指定哪一个。两条命令查的是同一份列表
/// （`status_tasks`，W2 收的口），列表空的时候不许各说各的。
///
/// 双向：正向钉住 `!stop` 的新回话，反向**直接拿 `!status` 的回话来比** ——
/// 把 `NO_ACTIVE_TASK_TEXT` 换成任何别的措辞都会红在第二个断言上，
/// 「两条命令一致」这件事因此不靠人眼盯着两个字符串常量。
#[tokio::test]
async fn stop_with_nothing_to_stop_says_exactly_what_status_says() {
    let h = Harness::new();
    let plane = h.plane();

    cmd(&plane, "!status", "om_8").await;
    let status_said = h.platform.last_text().expect("该回帖").text;

    cmd(&plane, "!stop", "om_9").await;
    let stop_said = h.platform.last_text().expect("该回帖").text;

    assert_eq!(
        stop_said, "本群没有活跃任务",
        "省略任务号、而本群一个活跃任务都没有 —— 不许再说「没有这个任务」（哪一个？）"
    );
    assert_eq!(
        stop_said, status_said,
        "两条命令查的是同一份列表，空的时候必须给同一句话"
    );
}

/// 第二格，本轨的正主：**省略任务号 + 本群有多个** —— 点名要任务号，并把可选的列出来。
///
/// 用户刚在 `!status` 里看见两个任务，敲一句 `!stop` 被告知「没有这个任务」——
/// 这是 W2 记下、AA2 确认还没销的那条账。
///
/// 三向断言，缺一条都立不住：
/// 1. 回话里**两个任务号都在**（列表和 `!status` 同源，用户不用再敲一次 `!status`）；
/// 2. 不是「没有这个任务」（旧行为的红点）；
/// 3. **一个任务都没被停掉** —— 「多于一个就随便挑一个停了」同样能让 1、2 全绿，
///    而那是比原病更坏的改法（用户没指名，系统替他做了不可逆的决定）。
#[tokio::test]
async fn stop_without_a_task_no_lists_the_candidates_instead_of_denying_them() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一个活").message_id(ROOT).build())
        .await
        .expect("建任务 1");
    plane
        .handle_event(ev().id("e2").text("第二个活").message_id("om_2").build())
        .await
        .expect("建任务 2");
    let tasks = active_tasks(&h.store, CHAT).await;
    assert_eq!(tasks.len(), 2, "前提：本群这时有两个活跃任务");

    cmd(&plane, "!stop", "om_9").await;

    let body = h.platform.last_text().expect("该回帖").text;
    assert_eq!(
        body,
        stop_needs_task_no_text(&tasks.iter().map(|t| t.task_no.clone()).collect::<Vec<_>>()),
        "该点名要任务号，并把 !status 里那两个原样列出来"
    );
    assert_ne!(
        body, "没有这个任务",
        "它们明明都在 !status 的列表里 —— 这正是本轨要收掉的那句话"
    );
    for task in &tasks {
        let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
        assert_eq!(
            saved.status,
            TaskStatus::Created,
            "指不到唯一一个的时候，一个都不许停 —— 不许替用户挑（任务 {}）",
            task.task_no
        );
    }
}

/// 同一组任务，`!status` 列出来的和 `!stop` 点名要的**必须是同一批**。
///
/// 上一条钉的是「说了什么」，这条钉的是「说的和 `!status` 对不对得上」：
/// 候选列表要是自己另查一份（比如退回 `list_active_tasks`），这条就红。
#[tokio::test]
async fn the_candidates_offered_by_stop_are_the_ones_status_listed() {
    let h = Harness::new();
    let plane = h.plane();
    for (i, text) in ["第一个活", "第二个活", "第三个活"].into_iter().enumerate() {
        plane
            .handle_event(
                ev().id(&format!("e{i}"))
                    .text(text)
                    .message_id(&format!("om_root_{i}"))
                    .build(),
            )
            .await
            .expect("建任务");
    }

    cmd(&plane, "!status", "om_8").await;
    let listed = h.platform.last_text().expect("该回帖").text;
    cmd(&plane, "!stop", "om_9").await;
    let offered = h.platform.last_text().expect("该回帖").text;

    let task_nos: Vec<String> = active_tasks(&h.store, CHAT)
        .await
        .into_iter()
        .map(|t| t.task_no)
        .collect();
    assert_eq!(task_nos.len(), 3, "前提：三个活跃任务");
    for task_no in &task_nos {
        assert!(
            listed.contains(task_no),
            "!status 该列出 {task_no}：{listed}"
        );
        assert!(
            offered.contains(task_no),
            "!stop 该把 {task_no} 也当成候选：{offered}"
        );
    }
    assert!(
        offered.contains('3'),
        "条数得说出来，不然用户不知道自己看全了没有：{offered}"
    );
}

/// `!stop` 认下来了、但 `Cancelled` 没落进库那一笔：**自己的计数器，别的照旧**（BB1 ②）。
///
/// 改之前它数的是 `events.dropped` —— 和「事件在路由里炸了」共用一个名字。两件事差得远：
/// 这条命令投递到了、也被认出来了，只有落库这一下没成；而 M4 要分的正是
/// 「没投递 vs 投递了被丢」。混在一个名字上就分不出来了，`dropped_note` 给的指路
/// （去 grep `ingress.handle_failed`）对这一笔也是错的。
///
/// 双向，两头都得钉：
/// * 正向 —— 新计数器 +1，且 `events.dropped` **一个字都没动**；
/// * 反向 —— `!status` 尾巴那句进程级警告**照旧说得出来**。只拆名字不把和加回去的话，
///   用户就彻底失去了唯一的信号：这一支是直接 `return` 的，`!stop` 连「没停成」
///   都不回一个字（下面第三个断言钉的就是这个沉默本身）。
#[tokio::test]
async fn a_failed_cancel_save_counts_on_its_own_but_still_warns_in_status() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("活儿").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);
    let texts_before = h.platform.texts().len();

    h.store.fail_next("update_task", 1);
    cmd(&plane, &format!("!stop {}", task.task_no), "om_9").await;

    assert_eq!(
        plane.counter("control.cancel_save_failed"),
        1,
        "落库失败该记在自己名下"
    );
    assert_eq!(
        plane.counter("events.dropped"),
        0,
        "路由一点没炸 —— 这笔不许再混进「事件没接住」那个数里"
    );
    assert_eq!(
        h.platform.texts().len(),
        texts_before,
        "现状（不是本轨要改的）：这一支直接 return，用户连「没停成」都收不到 —— \
         那句进程级警告因此是唯一的信号"
    );
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(
        saved.status,
        TaskStatus::Created,
        "库里确实没被改成 cancelled"
    );

    cmd(&plane, "!status", "om_8").await;
    let body = h.platform.last_text().expect("该回帖").text;
    assert!(
        body.contains("有 1 条事件没接住"),
        "拆了名字不许让这句警告哑掉 —— 用户要的是「有没有事情没办成」：{body}"
    );
}

// ---- !restart -------------------------------------------------------------

#[tokio::test]
async fn restart_archives_session_and_starts_new_one() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一版方案").message_id(ROOT).build())
        .await
        .expect("建任务");
    let old = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");

    cmd_in_thread(&plane, "!restart 换个思路重来", "om_2").await;

    let archived = h.store.get_session(&old.id).await.expect("读").expect("有");
    assert_eq!(archived.status, SessionStatus::Archived);
    let new = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("同一话题该有新会话");
    assert_ne!(new.id, old.id, "同一话题，换了会话");
    assert_eq!(new.anchor.thread_id.as_deref(), Some(ROOT));

    let tasks = active_tasks(&h.store, CHAT).await;
    assert_eq!(tasks.len(), 1, "旧会话那个已被终止");
    assert_eq!(tasks[0].session_id, new.id);
    assert_eq!(tasks[0].title, "换个思路重来");
    assert_eq!(turn_texts(&h.store, &new.id).await, vec!["换个思路重来"]);
    assert!(
        h.platform
            .last_text()
            .expect("该回帖")
            .text
            .contains("终止了 1 个进行中的任务")
    );
}

#[tokio::test]
async fn restart_without_text_only_archives() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("第一版").message_id(ROOT).build())
        .await
        .expect("建任务");
    let old = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");

    cmd_in_thread(&plane, "!restart", "om_2").await;

    let archived = h.store.get_session(&old.id).await.expect("读").expect("有");
    assert_eq!(archived.status, SessionStatus::Archived);
    assert!(active_tasks(&h.store, CHAT).await.is_empty());
    assert!(
        h.platform
            .last_text()
            .expect("该回帖")
            .text
            .contains("请直接说要做什么")
    );
}

// ---- !new -----------------------------------------------------------------

#[tokio::test]
async fn new_forces_fresh_session_inside_existing_thread() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("老话题").message_id(ROOT).build())
        .await
        .expect("建任务");
    let old = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");

    cmd_in_thread(&plane, "!new 另起一件事", "om_5").await;

    // CC2 ⑧ 翻转：原来断言「按 ROOT 查到的还是老会话」「`om_5` 成了新 root」。
    // 现在话题内的 `!new` 把新会话挂在同一个话题上，按 ROOT 查到的是新会话；
    // 老会话不归档、任务不动（下面两条断言照旧成立）。
    let fresh = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");
    assert_ne!(fresh.id, old.id, "同一话题，新会话胜出");
    assert!(
        h.store
            .find_session_by_thread(CHAT, "om_5")
            .await
            .expect("查")
            .is_none(),
        "`!new` 这条消息不再自己当 root"
    );
    let still = h.store.get_session(&old.id).await.expect("读").expect("有");
    assert_eq!(still.status, SessionStatus::Active, "老会话没被归档");

    let tasks = active_tasks(&h.store, CHAT).await;
    let session_ids: std::collections::HashSet<String> =
        tasks.iter().map(|t| t.session_id.clone()).collect();
    assert_eq!(
        session_ids,
        [old.id.clone(), fresh.id.clone()].into_iter().collect()
    );
    let fresh_task = tasks
        .iter()
        .find(|t| t.session_id == fresh.id)
        .expect("新会话下该有任务");
    assert_eq!(fresh_task.title, "另起一件事");
    // `!new <文本>` 不回帖（§9 第 3 条）
    assert!(h.platform.texts().is_empty(), "{:?}", h.platform.texts());
}

// ---- 未知命令 -------------------------------------------------------------

#[tokio::test]
async fn unknown_command() {
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "!oops 干点啥", "om_9").await;
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        UNKNOWN_COMMAND_TEXT
    );
    // CC2 ⑤ 翻转：原来断言这句里含 `!status` / `!restart`；现在它只指路 `!help`
    assert!(UNKNOWN_COMMAND_TEXT.contains("!help"));
    assert_eq!(plane.counter("commands!oops"), 1);
}

// ---- `!stop` 与 `!status` 的口径一致 ---------------------------------------

/// 把一个任务推到「worker 手上、状态已落 `Answering`」这个确定的形状。
///
/// 与 `status_still_lists_a_task_that_is_answering` 同一套造法：worker 停在
/// `WaitForCancel` 上，所以它一直挂在控制面的 `running` 里；状态则手工写成 `Answering`，
/// 也就是 `deliver()` 在「第一步就 final、没发过卡片」那一路落的那个值。
async fn a_task_stuck_in_answering() -> (
    Harness,
    Arc<InProcessControlPlane>,
    RunningPlane,
    aite_contracts::Task,
) {
    let h = Harness::new();
    let worker = ScriptedWorker::new(
        h.store.clone(),
        h.platform.clone(),
        vec![WorkerAction::WaitForCancel],
    );
    let plane = h.plane_builder().worker(worker.clone()).build();
    plane
        .handle_event(
            ev().id("e1")
                .text("算一下上周退款")
                .message_id(ROOT)
                .build(),
        )
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    let running = RunningPlane::start(plane.clone());
    within("等 worker 接手", async {
        while worker.calls().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(plane.state().running, vec![task.id.clone()]);

    let mut answering = h.store.get_task(&task.id).await.expect("读").expect("有");
    answering.status = TaskStatus::Answering;
    h.store.update_task(&answering).await.expect("回写");
    assert!(
        active_tasks(&h.store, CHAT).await.is_empty(),
        "前提：库的活跃口径这时确实看不见它"
    );

    (h, plane, running, answering)
}

/// V5 留下的半截：`!status` 列得出来、`!stop` 却回「没有这个任务」。
///
/// 现在两条命令查的是同一份列表（`status_tasks`），`!stop` 找得到它，
/// 并且回一句说得通的话 —— 交付中停不了，而不是「不存在」。
#[tokio::test]
async fn stop_on_an_answering_task_says_it_is_delivering() {
    let (h, plane, _running, task) = a_task_stuck_in_answering().await;

    cmd(&plane, &format!("!stop {}", task.task_no), "om_9").await;

    let body = h.platform.last_text().expect("该回帖").text;
    assert_eq!(body, stop_while_delivering_text(&task.task_no));
    assert_ne!(body, "没有这个任务", "它明明在 !status 的列表里");

    // 而且**一个字都没改**：不许把一条正在交付的任务写成 cancelled
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(
        saved.status,
        TaskStatus::Answering,
        "交付中的任务不该被改写 —— worker 马上会用 finish() 落 Delivered"
    );
    assert!(h.sandbox.released().is_empty(), "也不该顺手把沙箱还了");
}

/// 同一个任务，`!status` 和 `!stop` 的答复必须对得上。
///
/// 这是 V5 之后那个「更费解」的形状的正面判据：用户先问一句看见它，
/// 再伸手去停 —— 不许被告知它不存在。
#[tokio::test]
async fn status_and_stop_agree_on_the_same_task() {
    let (h, plane, _running, task) = a_task_stuck_in_answering().await;

    cmd(&plane, "!status", "om_8").await;
    let listed = h.platform.last_text().expect("该回帖").text;
    assert!(listed.contains(&task.task_no), "!status 该列出它：{listed}");

    cmd(&plane, &format!("!stop {}", task.task_no), "om_9").await;
    let stopped = h.platform.last_text().expect("该回帖").text;
    assert!(
        stopped.contains(&task.task_no),
        "!stop 得认得出 !status 刚列出来的那个任务号：{stopped}"
    );
    assert_ne!(
        stopped, "没有这个任务",
        "两条命令对同一个任务给出互相矛盾的答复 —— 这正是本轨要收掉的那个形状"
    );
}

/// 省略任务号那条路也按同一份列表算：本群只有这一个任务，`!stop` 就该认得出它。
#[tokio::test]
async fn stop_without_a_task_no_uses_the_same_list_as_status() {
    let (h, plane, _running, task) = a_task_stuck_in_answering().await;

    cmd(&plane, "!stop", "om_9").await;

    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        stop_while_delivering_text(&task.task_no)
    );
}

/// 卡片 stop 按钮走的是另一条解析路（`resolve_task`），同样要跟上。
///
/// P0 现在一个按钮都不渲染（lark SDK 收不到回传帧），但这条路的代码还在、契约 R3 也还在，
/// 所以它跟 `!stop` 不许再有两套口径。
#[tokio::test]
async fn card_stop_on_an_answering_task_says_it_is_delivering() {
    let (h, plane, _running, task) = a_task_stuck_in_answering().await;

    plane
        .handle_event(
            ev().id("ev-card")
                .kind(aite_contracts::EventKind::CardAction)
                .text("")
                .message_id("om_9")
                .card_action(support::card_action(
                    "om_card_1",
                    aite_contracts::CardActionKind::Stop,
                    Some(&task.id),
                ))
                .build(),
        )
        .await
        .expect("卡片停止");

    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        stop_while_delivering_text(&task.task_no)
    );
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Answering);
}

/// 反向的那一半没被弄丢：还在活跃口径里的任务照样停得掉。
#[tokio::test]
async fn stop_still_cancels_a_task_that_is_really_stoppable() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().id("e1").text("跑个长活").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    cmd(&plane, &format!("!stop {}", task.task_no), "om_9").await;

    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(saved.status, TaskStatus::Cancelled);
    assert!(
        h.platform
            .last_text()
            .expect("该回帖")
            .text
            .contains("已停止"),
        "可停的任务还是走原来那条路"
    );
}

// ---- `!restart` 与 `!stop` 的口径一致（AA2）--------------------------------

/// AA2：`!restart` 归档会话时用的是 `list_active_tasks`，`Answering` 整个看不见 ——
/// 而它那段注释点名要防的两件事（结果落进已归档的会话、继续出现在 `!status` 里），
/// 一个正在交付的任务**恰好两条都中**。
///
/// **为什么不是「换个列表、照停不误」**：实测过两头。worker 那一侧，取消旗标在
/// `deliver()` 全程一次都不被看，文件与答复照发、照落 `Delivered`、证据链照样
/// `finalize` 且校验通过；控制面这一侧，`cancel_task` 只是把库写成 `Cancelled`，
/// 随后被 worker 的 `finish()` 盖回 `Delivered`。净结果就是回一句
/// 「终止了 1 个进行中的任务」，而用户手上答复一个字不少 —— 那是造假话。
///
/// 所以口径统一到 `status_tasks`（和 `!stop` 同一份），但停不掉的那一半
/// **一个字都不碰**，改成在回帖里点名。
#[tokio::test]
async fn restart_names_the_task_it_could_not_stop_instead_of_dropping_it() {
    let (h, plane, _running, task) = a_task_stuck_in_answering().await;

    cmd_in_thread(&plane, "!restart 换个思路", "om_2").await;

    let body = h.platform.last_text().expect("该回帖").text;
    assert_eq!(
        body,
        format!(
            "已重开会话，{}",
            restart_while_delivering_text(std::slice::from_ref(&task.task_no))
        ),
        "停不掉的那个必须被点名，不许悄悄漏掉"
    );
    assert!(
        !body.contains("终止了"),
        "它根本没被停掉，不许被算进那个计数：{body}"
    );

    // 和 `!stop` 撞上 Answering 时同一条规矩：一个字都不改
    let saved = h.store.get_task(&task.id).await.expect("读").expect("有");
    assert_eq!(
        saved.status,
        TaskStatus::Answering,
        "交付中的任务不该被改写 —— worker 马上会用 finish() 落 Delivered"
    );
    assert!(h.sandbox.released().is_empty(), "也不该顺手把沙箱还了");
    assert!(
        !h.evidence
            .kinds(&task.id)
            .contains(&EvidenceKind::Cancelled),
        "链上不许多出一条它从没发生过的 cancelled：{:?}",
        h.evidence.kinds(&task.id)
    );
}

/// 反方向的那一半没被弄丢：同一次 `!restart` 里，能停的照常停掉、停不掉的才点名。
///
/// 上一条单独看是「什么都别做」也能过的，这一条把它钉死：`stopped` 那个计数照样要准，
/// 而且**恰好**不含交付中的那个。
#[tokio::test]
async fn restart_stops_what_it_can_and_names_what_it_cannot() {
    let (h, plane, _running, answering) = a_task_stuck_in_answering().await;

    // 同一个会话里再来一句：第一个任务已经不在活跃集里，没有 steer 目标，
    // 于是按 R6 新建一个任务。派发是串行的（`dispatch_loop`），worker 正被第一个
    // 按着，所以它停在 `created` —— 一个货真价实的「可停」。
    plane
        .handle_event(
            ev().id("e2")
                .text("顺手也查一下发票")
                .mentioned(false)
                .message_id("om_2")
                .thread(ROOT)
                .build(),
        )
        .await
        .expect("追问该新建任务");
    let stoppable = active_tasks(&h.store, CHAT).await.remove(0);
    assert_ne!(stoppable.id, answering.id, "前提：这是第二个任务");

    cmd_in_thread(&plane, "!restart 换个思路", "om_3").await;

    assert_eq!(
        h.store
            .get_task(&stoppable.id)
            .await
            .expect("读")
            .expect("有")
            .status,
        TaskStatus::Cancelled,
        "可停的那一半不许被这次改动弄丢"
    );
    assert_eq!(
        h.store
            .get_task(&answering.id)
            .await
            .expect("读")
            .expect("有")
            .status,
        TaskStatus::Answering,
        "交付中的那一半不许被碰"
    );

    let body = h.platform.last_text().expect("该回帖").text;
    assert_eq!(
        body,
        format!(
            "已重开会话，终止了 1 个进行中的任务。{}",
            restart_while_delivering_text(std::slice::from_ref(&answering.task_no))
        ),
        "两半各说各的，一句话里说清"
    );
}

/// CC2 ⑨：对交付中的任务发 `!stop`，**不写命令证据**（它马上会 `finish()` 落 manifest，
/// 写在后面会让 manifest 过期）—— 链的长度不变，也没有 `route=command` 那条。
#[tokio::test]
async fn stop_on_an_answering_task_writes_no_command_evidence() {
    let (h, plane, _running, task) = a_task_stuck_in_answering().await;
    let before = h.evidence.events(&task.id).len();

    cmd(&plane, &format!("!stop {}", task.task_no), "om_9").await;

    assert_eq!(
        h.evidence.events(&task.id).len(),
        before,
        "交付中任务的链不许动"
    );
    assert!(
        h.evidence
            .received(&task.id)
            .iter()
            .all(|p| p.get("route").and_then(|v| v.as_str()) != Some("command")),
    );
}
