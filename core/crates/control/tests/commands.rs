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
    stop_while_delivering_text,
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

#[tokio::test]
async fn stop_unknown_task() {
    let h = Harness::new();
    let plane = h.plane();
    cmd(&plane, "!stop #A99", "om_9").await;
    assert_eq!(h.platform.last_text().expect("该回帖").text, "没有这个任务");
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

    let still = h
        .store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("查")
        .expect("有");
    assert_eq!(still.id, old.id, "老话题没被动");
    let fresh = h
        .store
        .find_session_by_thread(CHAT, "om_5")
        .await
        .expect("查")
        .expect("本条消息成了新 root");
    assert_ne!(fresh.id, old.id);

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
    assert!(UNKNOWN_COMMAND_TEXT.contains("!status"));
    assert!(UNKNOWN_COMMAND_TEXT.contains("!restart"));
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
