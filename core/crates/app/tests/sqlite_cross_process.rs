//! SQLite 真跨进程（移植 `test_t8_sqlite_cross_process.py`，2 条）。
//!
//! B6 只验到「同进程里换一个 `ControlPlane` 实例」。这里把标准抬到 §2.4 M6 的口径：
//! **整套组装拆掉重建**（`build_app` 再走一遍、平台/模型/沙箱全是新的替身、
//! `store.close()` 真的关过连接），再看同一个话题接不接得上。
//!
//! 判据不止「库里有那一行」：第二套组装的模型上下文里必须真的读到上一轮的正文 ——
//! 接不上上下文的话，续接对用户就是没发生。
mod common;

use common::*;

use aite_contracts::TaskStatus;
use aite_testing::FakeSandbox;
use std::sync::Arc;

const ROOT: &str = "om_1";

/// 一套组装跑完 → 关掉 → 用同一个 .db 新建第二套 → 同话题追问命中同一 session。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_app_on_the_same_db_resumes_the_same_thread() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());

    // ── 第一套组装 ────────────────────────────────────────────────
    let platform1 = GatedPlatform::new();
    let app1 = build_with(
        config.clone(),
        platform1.clone(),
        RecordingModel::new(vec![final_step("北京今天晴，最高 28℃。")]),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    let mut run1 = RunningApp::start(app1.clone(), &platform1, None).await;
    platform1
        .emit(&Ev::new("e1", "第一问").message_id(ROOT).build())
        .await;
    let task1 = app1.store.list_active_tasks(CHAT).await.expect("list")[0].clone();
    let session_id = task1.session_id.clone();
    let p1 = platform1.clone();
    wait_until(|| p1.inner.count("send_text") == 1, "第一个任务交付").await;
    run1.shutdown().await.expect("run_app 正常收场");

    // `run_app` 的收尾把连接关了：再用它查任何东西都该报「未初始化」。
    assert!(app1.store.get_task(&task1.id).await.is_err());

    // ── 第二套组装：同一个 .db 文件，其余全新 ──────────────────────
    let platform2 = GatedPlatform::new();
    let model2 = RecordingModel::new(vec![final_step("好的，按季度再画一张。")]);
    let app2 = build_with(
        config.clone(),
        platform2.clone(),
        model2.clone(),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    assert!(
        !Arc::ptr_eq(&app1.store, &app2.store),
        "真的是新建的一套，不是复用"
    );

    let mut run2 = RunningApp::start(app2.clone(), &platform2, None).await;
    // R6：话题内续接，不带 @
    platform2
        .emit(
            &Ev::new("e2", "第二问")
                .mentioned(false)
                .message_id("om_2")
                .thread_id(ROOT)
                .build(),
        )
        .await;
    let task2 = app2.store.list_active_tasks(CHAT).await.expect("list")[0].clone();
    let p2 = platform2.clone();
    wait_until(|| p2.inner.count("send_text") == 1, "第二个任务交付").await;

    assert_eq!(task2.session_id, session_id, "命中同一会话");
    assert_eq!(
        (task1.task_no.as_str(), task2.task_no.as_str()),
        ("#A1", "#A2"),
        "任务号计数器也跨进程连续"
    );

    let turns = app2
        .store
        .list_turns(&session_id, 100)
        .await
        .expect("list_turns");
    assert_eq!(
        turns.iter().map(|t| t.content.clone()).collect::<Vec<_>>(),
        vec!["第一问", "第二问"]
    );
    assert_eq!(turns.iter().map(|t| t.seq).collect::<Vec<_>>(), vec![0, 1]);

    // 关键的一条：第二个进程的模型上下文里真的带上了上一轮，不只是库里躺着一行 turn。
    assert!(
        model2
            .prompt_texts()
            .iter()
            .any(|ms| ms.iter().any(|t| t.contains("第一问"))),
        "第二套组装的模型上下文里没有上一轮的正文"
    );
    run2.shutdown().await.expect("run_app 正常收场");

    // 两个任务，两份证据，都在 tmp 下
    let mut want = vec![task1.id.clone(), task2.id.clone()];
    want.sort();
    assert_eq!(evidence_task_dirs(&config), want);
    for task_id in [&task1.id, &task2.id] {
        let reread = read_task_from_disk(&config, task_id)
            .await
            .expect("任务在库里");
        assert_eq!(reread.status, TaskStatus::Delivered);
    }
}

/// R2 的去重键落在库里：换一套组装后重推同一 event_id，不许再跑一遍。
///
/// 平台重连后重推是 §2.4 M2 的真实场景，只是这里把「重连」换成了更狠的「重启进程」。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seen_event_dedupe_survives_the_rebuild() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let duplicate = Ev::new("ev-dup", "干个活").message_id(ROOT).build();

    let platform1 = GatedPlatform::new();
    let app1 = build_with(
        config.clone(),
        platform1.clone(),
        RecordingModel::new(vec![final_step("干完了。")]),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    let mut run1 = RunningApp::start(app1.clone(), &platform1, None).await;
    platform1.emit(&duplicate).await;
    let p1 = platform1.clone();
    wait_until(|| p1.inner.count("send_text") == 1, "第一次真的跑了").await;
    run1.shutdown().await.expect("run_app 正常收场");

    let platform2 = GatedPlatform::new();
    let model2 = RecordingModel::new(vec![final_step("不该跑到这一步。")]);
    let app2 = build_with(
        config.clone(),
        platform2.clone(),
        model2.clone(),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    let mut run2 = RunningApp::start(app2.clone(), &platform2, None).await;
    platform2.emit(&duplicate).await;
    // handle_event 是在 emit 里同步走完的，计数当场就该到位
    assert_eq!(
        app2.plane.counters().get("events.duplicate"),
        Some(&serde_json::json!(1))
    );

    settle().await; // 给派发循环足够的机会去做不该做的事
    assert_eq!(model2.call_count(), 0, "模型一次都没被叫");
    assert_eq!(platform2.inner.outbound_count(), 0, "一条出站都没有");
    run2.shutdown().await.expect("run_app 正常收场");

    assert_eq!(
        evidence_task_dirs(&config).len(),
        1,
        "全程只有一个任务留下证据"
    );
}
