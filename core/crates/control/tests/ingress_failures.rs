//! 事件入口的失败面：路由半路炸了，那条事件去哪了（owner: T24）。
//! 对应 `tests/control/test_ingress_failures.py`（9 条）。
//!
//! `tests/ingress.rs` 验的是 `Ingress` 这层薄片本身（错误不漏回 adapter、慢回调被叫出来）。
//! 这一份往下走一层，问的是**代价**：错误被兜住之后，那条事件还有没有人管。
//!
//! §3.3 最后一行写的是：
//!
//! ```text
//! 任何未捕获异常 → task failed + 回帖 + evidence failed；进程不退出
//! ```
//!
//! 「进程不退出」这半一直成立。另一半在两种情况下**一件都做不到**：
//!
//! 1. 错误发生在 `seen_event`（去重那一步）—— 此刻连 task 都还没建，没有 task 可标 failed、
//!    没有帖可回、没有证据链可写。事件被静默丢掉。
//! 2. 错误发生在 `seen_event` **之后** —— 更糟：去重记录已经落库了，平台再重推同一条，
//!    R2 会把它当重复丢掉。这条事件从此**救不回来**。
//!
//! 这两条都先钉成用例。判断和代价写在回执里 —— 钉住现状本身就是交付的一部分。
mod support;

use std::sync::Arc;

use aite_contracts::{ControlPlane, IngressError, SessionStore};
use aite_control::Ingress;
use support::{CHAT, Harness, ROOT, active_tasks, ev};

// --------------------------------------------------------------------------
// 1 抛在 seen_event：事件被静默丢掉
// --------------------------------------------------------------------------

/// **当前行为，钉住而非认可**：`seen_event` 报错 → 什么都没发生。
///
/// 没有 task（连 `create_task` 都没走到）、没有会话、群里没有任何一句话、证据目录是空的。
/// 用户看到的就是「@ 了 Aite，它一点反应都没有」。
#[tokio::test]
async fn a_failure_in_seen_event_drops_the_event_silently() {
    let h = Harness::new();
    h.store.fail_next("seen_event", 1);
    let plane = h.plane();
    let ingress = Ingress::new(plane.clone());

    ingress
        .on_event(ev().text("磁盘抖的时候发的那句话").build())
        .await; // 不抛

    assert_eq!(ingress.counter("ingress.errors"), 1);
    assert_eq!(ingress.counter("events.handled"), 0);
    // 三件承诺，一件都没有
    assert!(
        active_tasks(&h.store, CHAT).await.is_empty(),
        "连 task 都没建出来，无从标 failed"
    );
    assert!(h.platform.texts().is_empty(), "没有 task 就没有帖可回");
    assert!(h.platform.cards().is_empty());
    assert!(h.platform.reactions().is_empty());
    assert!(
        h.evidence.task_ids().is_empty(),
        "没有 task_id 就没有证据链"
    );
}

/// 抖完就好：下一条事件照常接得住。「活着但废了」不算过关。
#[tokio::test]
async fn the_process_keeps_working_after_the_disk_settles() {
    let h = Harness::new();
    h.store.fail_next("seen_event", 1);
    let plane = h.plane();
    let ingress = Ingress::new(plane);

    ingress
        .on_event(ev().id("ev-boom").text("撞上磁盘的那条").build())
        .await;
    ingress
        .on_event(
            ev().id("ev-ok")
                .text("抖完之后的那条")
                .message_id("om_2")
                .build(),
        )
        .await;

    assert_eq!(ingress.counter("ingress.errors"), 1);
    assert_eq!(ingress.counter("events.handled"), 1);
    let titles: Vec<String> = active_tasks(&h.store, CHAT)
        .await
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(titles, vec!["抖完之后的那条"]);
}

// --------------------------------------------------------------------------
// 2 抛在 seen_event 之后：这条事件从此救不回来
// --------------------------------------------------------------------------

/// 比第 1 条更糟的那一半，也是「重试」这条路走不通的根本原因。
///
/// `seen_event` 是**先落库再往下走**的：它成功那一刻去重记录就已经写进去了。如果紧接着的
/// `create_session` / `create_task` 炸了，事件同样被丢掉 —— 但这一次平台把它重推上来也没用，
/// R2 认得它，会当成重复直接丢。
///
/// 所以「磁盘抖动多半是瞬时的，重试一次就好」这个直觉在这里不成立：能重试的只有
/// **整条 `handle_event`**，而重跑第一步就会撞上 R2。
#[tokio::test]
async fn a_failure_after_seen_event_makes_the_event_unrecoverable() {
    let h = Harness::new();
    h.store.fail_next("create_session", 1);
    let plane = h.plane();
    let ingress = Ingress::new(plane.clone());
    let event = ev().id("ev-lost").text("卡在建会话上的那句话").build();

    ingress.on_event(event.clone()).await;
    assert_eq!(ingress.counter("ingress.errors"), 1);
    assert!(active_tasks(&h.store, CHAT).await.is_empty());

    // 去重记录已经落库了 —— 平台重推也救不回来
    assert!(
        h.store.seen_event("ev-lost").await.expect("查去重"),
        "seen_event 是先落库的，所以这条事件在库里已经算「见过」了"
    );
    ingress.on_event(event).await; // 平台重推同一条
    assert!(
        active_tasks(&h.store, CHAT).await.is_empty(),
        "重推被 R2 当成重复丢掉了"
    );
    assert_eq!(plane.counter("events.duplicate"), 1);
    assert!(h.platform.texts().is_empty());
}

// --------------------------------------------------------------------------
// 3 计数器要看得见
// --------------------------------------------------------------------------

/// 错误逃出 `handle_event` 时，控制面自己也记一笔。
///
/// `Ingress` 的 `ingress.errors` 早就在数了，但那个计数器挂在 ingress 上，而 `!status`
/// 是控制面回的 —— 控制面拿不到 ingress。所以在这里也记一笔，两个计数器数的是同一批事件，
/// 只是待在不同的层上。
#[tokio::test]
async fn dropped_events_are_counted_on_the_plane() {
    let h = Harness::new();
    h.store.fail_next("seen_event", 1);
    let plane = h.plane();

    // 直接调控制面：错误必须还往上传
    let err = plane
        .handle_event(ev().build())
        .await
        .expect_err("该把错误传上来");
    assert!(
        matches!(&err, IngressError::Store(_)) && err.to_string().contains("只读磁盘"),
        "实际是 {err}"
    );

    assert_eq!(plane.counter("events.dropped"), 1);
    assert_eq!(plane.counter("events.duplicate"), 0);
}

/// `!status` 是人在问「现在到底什么情况」的地方，丢过事件就得在这里说出来。
///
/// 不说的话，唯一的痕迹是一行 `ingress.handle_failed` 日志 —— 一人公司没有告警，
/// 没人会去翻。而用户能想到的动作恰恰就是 `!status`。
#[tokio::test]
async fn status_says_out_loud_that_events_were_dropped() {
    let h = Harness::new();
    h.store.fail_next("seen_event", 1);
    let plane = h.plane();
    let ingress = Ingress::new(plane);

    ingress
        .on_event(ev().id("ev-boom").text("被磁盘吃掉的那句").build())
        .await;
    ingress
        .on_event(
            ev().id("ev-status")
                .text("!status")
                .message_id("om_2")
                .build(),
        )
        .await;

    let body = h.platform.last_text().expect("该回帖").text;
    assert!(
        body.contains('1') && body.contains("没接住"),
        "!status 该把丢掉的事件数说出来，实际回的是：{body:?}"
    );
    assert!(body.contains("ingress.handle_failed"), "{body:?}");
}

/// 没丢过就一个字都不多说 —— 平时的 `!status` 不该被一句常驻警告污染。
#[tokio::test]
async fn status_stays_quiet_when_nothing_was_dropped() {
    let h = Harness::new();
    let plane = h.plane();
    plane
        .handle_event(ev().text("!status").build())
        .await
        .expect("命令");
    assert_eq!(
        h.platform.last_text().expect("该回帖").text,
        "本群没有活跃任务"
    );
}

// --------------------------------------------------------------------------
// 4 并发下的 seq 分配（T24 修的那条）
// --------------------------------------------------------------------------

/// 同一个话题里的两条事件**同时**进 `handle_event`，每条都要落进 transcript。
///
/// 这是重连那一刻的真形状：edge 每收一帧就起一个独立的调用，整批一起上来。
///
/// 没有 `turn_seq_lock` 的话：`append_turn` 先 `list_turns` 读 seq、再 `append_turn` 写，
/// 两条都读到「还没有 turn」→ 都写 seq=0 → 第二条撞唯一约束报 `DuplicateTurn`。错误被
/// `Ingress` 兜住，用户那句追问静默消失，而且因为 `seen_event` 已经落库，平台重推也救不回来。
///
/// （`FakeStore` 的每个方法都 `yield_now()`，就是为了让这个竞态真的能发生 —— 见 support 的头注释。）
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn six_events_in_one_thread_arriving_together_all_land() {
    let h = Harness::new();
    let plane = h.plane();
    let ingress = Ingress::new(plane);

    ingress
        .on_event(ev().id("ev1").text("按月画个图").message_id(ROOT).build())
        .await;
    let session_id = active_tasks(&h.store, CHAT).await.remove(0).session_id;

    let mut handles = Vec::new();
    for i in 0..6 {
        let ingress = ingress.clone();
        let event = ev()
            .id(&format!("ev-f{i}"))
            .text(&format!("追问 {i}"))
            .mentioned(false)
            .message_id(&format!("om_f{i}"))
            .thread(ROOT)
            .build();
        handles.push(tokio::spawn(async move { ingress.on_event(event).await }));
    }
    for handle in handles {
        handle.await.expect("投递任务不该 panic");
    }

    assert_eq!(
        ingress.counter("ingress.errors"),
        0,
        "有追问在路由里炸了 —— 它既没进 transcript 也不会被重推第二次"
    );
    let turns = h
        .store
        .list_turns(&session_id, 200)
        .await
        .expect("读 turns");
    let mut contents: Vec<String> = turns.iter().map(|t| t.content.clone()).collect();
    let seqs: Vec<u64> = turns.iter().map(|t| t.seq).collect();
    assert_eq!(
        seqs,
        (0..7u64).collect::<Vec<_>>(),
        "seq 该是连着的，实际 {seqs:?} —— 撞号就说明分配不是原子的"
    );
    contents.sort();
    let mut want: Vec<String> = (0..6).map(|i| format!("追问 {i}")).collect();
    want.push("按月画个图".into());
    want.sort();
    assert_eq!(contents, want, "六条追问该一条不少地进 transcript");
}

/// 把批量放大到 20 条：seq 一个不重、一个不漏。
///
/// 小批量下事件循环可能恰好不交错，看起来是绿的。这条把批量放大，让「读 seq 到写 turn
/// 之间被插进来」真的发生。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_seq_allocation_survives_a_bigger_burst() {
    let h = Harness::new();
    let plane = h.plane();
    let ingress = Ingress::new(plane);

    ingress
        .on_event(ev().id("ev1").text("root").message_id(ROOT).build())
        .await;
    let session_id = active_tasks(&h.store, CHAT).await.remove(0).session_id;

    let mut handles = Vec::new();
    for i in 0..20 {
        let ingress = ingress.clone();
        let event = ev()
            .id(&format!("ev-{i}"))
            .text(&format!("第 {i} 句"))
            .mentioned(false)
            .message_id(&format!("om_{i}"))
            .thread(ROOT)
            .build();
        handles.push(tokio::spawn(async move { ingress.on_event(event).await }));
    }
    for handle in handles {
        handle.await.expect("投递任务不该 panic");
    }

    assert_eq!(ingress.counter("ingress.errors"), 0);
    let turns = h
        .store
        .list_turns(&session_id, 100)
        .await
        .expect("读 turns");
    assert_eq!(turns.len(), 21);
    assert_eq!(
        turns.iter().map(|t| t.seq).collect::<Vec<_>>(),
        (0..21u64).collect::<Vec<_>>()
    );
    let got: std::collections::HashSet<String> = turns.iter().map(|t| t.content.clone()).collect();
    let mut want: std::collections::HashSet<String> =
        (0..20).map(|i| format!("第 {i} 句")).collect();
    want.insert("root".into());
    assert_eq!(got, want);
}

/// store 只抖一次，但 `on_event` 在任何一条上都不许把错误漏回 adapter。
///
/// §3.3 第一行的另一半：回调抛出去会把长连接的读循环带走。
#[tokio::test]
async fn a_failing_store_still_does_not_take_the_platform_down() {
    for method in ["seen_event", "create_session", "create_task", "append_turn"] {
        let h = Harness::new();
        h.store.fail_next(method, 1);
        let plane: Arc<dyn ControlPlane> = h.plane();
        let ingress = Ingress::new(plane);

        ingress
            .on_event(
                ev().id(&format!("ev-{method}"))
                    .message_id(&format!("om_{method}"))
                    .build(),
            )
            .await;

        assert_eq!(
            ingress.counter("ingress.errors"),
            1,
            "抛在 {method} 上时没被兜住"
        );
    }
}
