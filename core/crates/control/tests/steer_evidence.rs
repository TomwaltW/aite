//! R6 追问在证据链上留下的那一条（owner: T24）。
//! 对应 `tests/control/test_steer_evidence.py`（8 条）。
//!
//! `EvidenceKind` 是冻结契约（不新增 kind），于是 `event_received` 在时间线上承载两种语义：
//!
//! ```text
//! route=new_task   这条事件是任务的起点（R6 新建 / R7 @）
//! route=steer      任务已经在跑，用户中途改了要求
//! ```
//!
//! 两种必须在 payload 上分得开 —— 分不开的话 `evidence show` 讲出来的故事是糊的：
//! 读的人看到两条一模一样的 `event_received`，不知道哪条是起点、哪条是转折。
//!
//! 这一份验的是「写了什么、什么时候写、什么时候不写」。写完之后**模型真的看见了**那句话，
//! 那是 R5 的地盘（`tests/worker/test_steer.py` 验注入，这里验记账）。
mod support;

use aite_contracts::{ControlPlane, EvidenceKind, SessionStore, TaskStatus};
use aite_control::{ROUTE_NEW_TASK, ROUTE_STEER};
use serde_json::json;
use support::{CHAT, Harness, ROOT, ScriptedWorker, active_tasks, ev};

const FOLLOWUP: &str = "别按月了，改成按季度";

async fn followup(
    plane: &std::sync::Arc<aite_control::InProcessControlPlane>,
    text: &str,
    event_id: &str,
    message_id: &str,
) {
    plane
        .handle_event(
            ev().id(event_id)
                .text(text)
                .mentioned(false)
                .message_id(message_id)
                .thread(ROOT)
                .build(),
        )
        .await
        .expect("追问不该报错");
}

fn routes(h: &Harness, task_id: &str) -> Vec<String> {
    h.evidence
        .received(task_id)
        .into_iter()
        .map(|p| {
            p.get("route")
                .and_then(|v| v.as_str())
                .unwrap_or("<缺 route>")
                .to_string()
        })
        .collect()
}

// --------------------------------------------------------------------------
// 1 写在哪一步，写了什么
// --------------------------------------------------------------------------

/// 排 steer 的那一刻就写，不等任务收尾。
///
/// 等收尾才写就没意义了：时间线上这条要落在「用户说话的那一刻」，夹在两次 `model_call`
/// 中间，读的人才看得出「哦，是这里改的主意」。
#[tokio::test]
async fn a_steer_appends_an_event_received_to_the_running_task() {
    let h = Harness::new();
    // 空脚本的 worker：任务只入队，不真跑
    let plane = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    plane
        .handle_event(ev().text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    let before = h.evidence.events(&task.id).len();
    followup(&plane, FOLLOWUP, "ev2", "om_2").await;

    assert_eq!(plane.counter("events.steer"), 1);
    assert_eq!(plane.pending_steer(&task.id), vec![FOLLOWUP.to_string()]);
    assert_eq!(
        h.evidence.events(&task.id).len(),
        before + 1,
        "排 steer 的当下就该落一条证据"
    );

    let payload = h.evidence.received(&task.id).pop().expect("该有 payload");
    let want = json!({
        "event_id": "ev2",
        "kind": "message",
        "chat_id": CHAT,
        "sender_id": "ou_user",
        "message_id": "om_2",
        "mentioned": false,
        "route": ROUTE_STEER,
        "text": FOLLOWUP,
    });
    assert_eq!(serde_json::Value::Object(payload), want);
}

/// 同一条链上两条 `event_received`，`route` 把它们分开。
#[tokio::test]
async fn the_two_kinds_of_event_received_are_told_apart_by_route() {
    let h = Harness::new();
    let plane = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    plane
        .handle_event(ev().text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);
    followup(&plane, FOLLOWUP, "ev2", "om_2").await;

    assert_eq!(
        routes(&h, &task.id),
        vec![ROUTE_NEW_TASK, ROUTE_STEER],
        "起点那条该是 new_task、追问那条该是 steer"
    );

    // 顺序也要对：起点在最前面（紧跟 task_created），追问在它之后
    let kinds = h.evidence.kinds(&task.id);
    assert_eq!(kinds[0], EvidenceKind::TaskCreated);
    assert_eq!(kinds[1], EvidenceKind::EventReceived);
    assert_eq!(kinds[kinds.len() - 1], EvidenceKind::EventReceived);
}

/// 用户说的话进证据，按卡片标题那个截断口径（≤40 字）。
///
/// 截断的代价写在这里：超过 40 字的追问，证据里只留个开头，全文在 transcript。
#[tokio::test]
async fn the_users_own_words_are_in_the_chain_but_clipped() {
    let h = Harness::new();
    let plane = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    plane
        .handle_event(ev().text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    let long_text = format!("改成按季度，另外{}", "把去年同期也画上，".repeat(20));
    followup(&plane, &long_text, "ev2", "om_2").await;

    let payload = h.evidence.received(&task.id).pop().expect("该有 payload");
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .expect("steer 那条该带 text");
    assert_eq!(text.chars().count(), 40, "该截到 40 字，实际 {text}");
    assert!(text.starts_with("改成按季度，另外"));
    assert!(text.ends_with('…'));

    // 全文没丢，在 transcript 里 —— 证据是索引，不是备份
    let turns = h
        .store
        .list_turns(&task.session_id, 200)
        .await
        .expect("读 turns");
    assert_eq!(turns.last().expect("有").content, long_text);
}

/// 连着追问三句，链上就该有三条 —— 合并成一条会把「改了几次主意」抹掉。
#[tokio::test]
async fn several_steers_each_get_their_own_line() {
    let h = Harness::new();
    let plane = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    plane
        .handle_event(ev().text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    for i in 0..3 {
        followup(
            &plane,
            &format!("第 {i} 次改主意"),
            &format!("ev-{i}"),
            &format!("om_{i}"),
        )
        .await;
    }

    let steers: Vec<String> = h
        .evidence
        .received(&task.id)
        .into_iter()
        .filter(|p| p.get("route").and_then(|v| v.as_str()) == Some(ROUTE_STEER))
        .filter_map(|p| p.get("text").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    assert_eq!(
        steers,
        vec!["第 0 次改主意", "第 1 次改主意", "第 2 次改主意"]
    );
    assert_eq!(
        plane.pending_steer(&task.id),
        vec!["第 0 次改主意", "第 1 次改主意", "第 2 次改主意"]
    );
}

// --------------------------------------------------------------------------
// 2 什么时候**不**写
// --------------------------------------------------------------------------

/// 上一个任务已经交付之后的追问走的是 R6 的「否则新建 task 继续」—— 那是一个新任务的
/// 起点，`route` 该是 `new_task`，不是 `steer`。记反了的话时间线会讲成
/// 「有个任务跑到一半被改了要求」，而实际上是两次独立的对话。
#[tokio::test]
async fn a_followup_that_becomes_a_new_task_is_not_logged_as_a_steer() {
    let h = Harness::new();
    let plane = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    plane
        .handle_event(ev().text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let mut first = active_tasks(&h.store, CHAT).await.remove(0);
    first.status = TaskStatus::Delivered;
    h.store.update_task(&first).await.expect("落终态");

    followup(&plane, FOLLOWUP, "ev2", "om_2").await;

    let fresh: Vec<aite_contracts::Task> = active_tasks(&h.store, CHAT)
        .await
        .into_iter()
        .filter(|t| t.id != first.id)
        .collect();
    assert_eq!(fresh.len(), 1);
    assert_eq!(plane.counter("events.steer"), 0);

    // 老任务的链没被动过；新任务自己有一条 new_task
    assert_eq!(routes(&h, &first.id), vec![ROUTE_NEW_TASK]);
    assert_eq!(routes(&h, &fresh[0].id), vec![ROUTE_NEW_TASK]);
}

/// 孤儿任务（上个进程留下的 working）不收追问，也就不该往它的链上写东西。
///
/// T14 修的正是这条：孤儿不算 steer 目标，追问走「新建 task」。写证据这一步挂在
/// `target is not None` 里面，所以它天然跟着那个判断走 —— 这条用例把「天然」钉住，
/// 免得哪天有人把 evidence 写到 `if` 外面去，给一个永远没人 drain 的队列记账。
///
/// 「换进程」= 同一个 store 换一个控制面实例（Python 那边是 `plane._owned.discard(...)`，
/// Rust 的集成测试进不了私有字段，换实例更贴近真形状）。
#[tokio::test]
async fn an_orphan_task_gets_no_steer_evidence() {
    let h = Harness::new();
    let plane1 = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    plane1
        .handle_event(ev().text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let mut orphan = active_tasks(&h.store, CHAT).await.remove(0);
    orphan.status = TaskStatus::Working;
    h.store.update_task(&orphan).await.expect("落 working");

    let plane2 = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    followup(&plane2, FOLLOWUP, "ev2", "om_2").await;

    assert_eq!(plane2.counter("events.orphan_task"), 1);
    assert_eq!(plane2.counter("events.steer"), 0);
    assert_eq!(
        routes(&h, &orphan.id),
        vec![ROUTE_NEW_TASK],
        "孤儿的链上不该多出一条它永远收不到的追问"
    );
}

/// `!status` 这类命令在 R5 就被截走了，走不到 R6，链上不该有它。
///
/// 证据链讲的是「这个任务经历了什么」，不是「这个群里发生过什么」。
#[tokio::test]
async fn a_command_in_the_thread_is_not_a_steer() {
    let h = Harness::new();
    let plane = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    plane
        .handle_event(ev().text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);

    followup(&plane, "!status", "ev-cmd", "om_cmd").await;

    assert_eq!(plane.counter("events.steer"), 0);
    assert_eq!(routes(&h, &task.id), vec![ROUTE_NEW_TASK]);
}

// --------------------------------------------------------------------------
// 3 链本身还得是条链
// --------------------------------------------------------------------------

/// 多写一条不能把 hash 链写断 —— 证据链断了，整份证据就不可信了（C1/§2.4）。
#[tokio::test]
async fn the_chain_still_verifies_after_a_steer() {
    let h = Harness::new();
    let plane = h
        .plane_builder()
        .worker(ScriptedWorker::idle(h.store.clone(), h.platform.clone()))
        .build();
    plane
        .handle_event(ev().text("按月画个图").message_id(ROOT).build())
        .await
        .expect("建任务");
    let task = active_tasks(&h.store, CHAT).await.remove(0);
    followup(&plane, FOLLOWUP, "ev2", "om_2").await;
    followup(&plane, "再加上同比", "ev3", "om_3").await;

    let events = h.evidence.events(&task.id);
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        (0..events.len() as u64).collect::<Vec<_>>(),
        "seq 该是连着的"
    );
    assert!(
        aite_contracts::EvidenceWriter::verify(h.evidence.as_ref(), &task.id),
        "加了 steer 之后链校验不过了"
    );
}
