//! 起飞时把上一条命的残局收干净（移植 `test_t22_startup_recovery.py`，9 条）。
//!
//! `SqliteSessionStore::recover_orphan_tasks` 是药，`run_app` 是调用点。这一组测的就是
//! 接线本身：
//!
//! * 收拾发生在 `platform.start()` **之前**（真机上 `start()` 是"起投递面"，排在它后面的
//!   代码在 Python 单进程版里一辈子轮不到；Rust 版虽然 `start()` 会返回，顺序仍然冻结）；
//! * §3.3 对失败的三件事一件不少：task `failed`（store 做）+ 回帖 + evidence `failed`，
//!   外加把停在「进行中」的那张卡置成 failed（W4）；
//! * **一个孤儿收不掉，不许连累别人，更不许让进程起不来**。
//!
//! 崩溃怎么模拟：照 T18 的办法，**一步收尾都不走**（不 `platform.stop()`、不 `plane.join()`、
//! 不 finalize evidence），只把连接关掉，再在同一个 .db 文件上重开一套组装。
//!
//! 与 Python 版的一处手法差异：Python 用 `GatedPlatform.adopt_cards()` 把上一条命发出的
//! 卡片搬到新平台上；Rust 这边直接**共用同一个 `FakePlatform`**（平台不是崩的那一方），
//! 重开时 `calls.clear()` 归零调用记账 —— 卡片状态天然带过去，断言面还是干净的。
mod common;

use common::*;

use aite_contracts::{
    CardStatus, ChecklistItem, ChecklistState, EvidenceKind, Session, SessionStore, Task,
    TaskStatus,
};
use aite_store::{ORPHAN_RESULT_SUMMARY, SqliteSessionStore};
use aite_testing::FakeSandbox;
use aite_worker::card::render_card;
use serde_json::json;
use std::sync::Arc;

const ROOT: &str = "om_1";

async fn make_app(
    config: &aite_contracts::AiteConfig,
    platform: Arc<GatedPlatform>,
    reply: &str,
) -> (Arc<aite_app::AiteApp>, Arc<RecordingModel>) {
    let model = RecordingModel::new(vec![final_step(reply)]);
    let app = build_with(
        config.clone(),
        platform,
        model.clone(),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    (app, model)
}

/// 造一个「崩溃那一刻正在干活」的任务，返回它。
///
/// 比纯 store 层那份多一样，因为这一轨要看的正是它：卡片已经发出去停在「进行中」。
/// evidence 由 `handle_event` 自然写下 task_created / event_received 两条，finalize 没走。
async fn crash_with_a_task_in_flight(
    config: &aite_contracts::AiteConfig,
    platform: &Arc<GatedPlatform>,
    text: &str,
    root: &str,
) -> Task {
    let (app, _model) = make_app(config, platform.clone(), "不会用到").await;
    app.store.init().await.expect("init"); // run_app 起飞时做的那一步
    // 上一次崩溃留下的任务也还是 active，靠「这一轮新冒出来的那个」认人
    let before: Vec<String> = app
        .store
        .list_active_tasks(CHAT)
        .await
        .expect("list")
        .into_iter()
        .map(|t| t.id)
        .collect();
    app.plane
        .handle_event(Ev::new(&format!("e-{root}"), text).message_id(root).build())
        .await
        .expect("handle_event");

    let mut task: Task = app
        .store
        .list_active_tasks(CHAT)
        .await
        .expect("list")
        .into_iter()
        .find(|t| !before.contains(&t.id))
        .expect("这一轮新建的任务");
    let session: Session = app
        .store
        .get_session(&task.session_id)
        .await
        .expect("get_session")
        .expect("session");

    // worker 领走任务后做的第一件事：发卡片（W3）
    task.status = TaskStatus::Working;
    task.title = text.to_string();
    task.checklist = vec![ChecklistItem {
        id: "c1".to_string(),
        text: "画".to_string(),
        state: ChecklistState::Doing,
        note: None,
    }];
    let card = render_card(
        &task,
        &session,
        &session.created_by,
        CardStatus::Working,
        None,
    );
    let result = app
        .platform
        .send_card(&session.chat_id, Some(root), &card)
        .await
        .expect("send_card");
    task.card_id = result.card_id.clone();
    app.store.update_task(&task).await.expect("update_task");

    app.store.close().await.expect("close"); // 只关连接；收尾序列全跳过
    task
}

/// 在同一个 .db 上重开一套组装。调用记账归零，卡片状态保留。
async fn reopen(
    config: &aite_contracts::AiteConfig,
    platform: &Arc<GatedPlatform>,
    reply: &str,
) -> (Arc<aite_app::AiteApp>, Arc<RecordingModel>) {
    platform.inner.calls.clear();
    make_app(config, platform.clone(), reply).await
}

// --------------------------------------------------------------------------
// 主线：孤儿被收干净
// --------------------------------------------------------------------------

/// 起飞就把孤儿收了：库里 failed、群里有话、卡片不再绿、证据链收口。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn takeoff_closes_the_orphan_in_the_group() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let orphan = crash_with_a_task_in_flight(&config, &platform, "画个图", ROOT).await;

    let (app, _model) = reopen(&config, &platform, "这次答上了。").await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    // 1) 状态（store 做的那一件）
    let closed = read_task_from_disk(&config, &orphan.id)
        .await
        .expect("任务还在库里");
    assert_eq!(closed.status, TaskStatus::Failed);
    assert_eq!(closed.result_summary, ORPHAN_RESULT_SUMMARY);

    // 2) 回帖：回到任务原来那条话题里，带得出任务号
    assert_eq!(platform.inner.count("send_text"), 1);
    let notice = platform.inner.calls.of("send_text")[0].clone();
    let text = notice.arg_str("text").unwrap_or_default().to_string();
    assert!(text.contains(&orphan.task_no), "{text}");
    assert!(text.contains(ORPHAN_RESULT_SUMMARY), "{text}");
    assert_eq!(notice.arg_str("chat_id"), Some(CHAT));
    assert_eq!(notice.arg_str("reply_to"), Some(ROOT));
    assert_eq!(notice.arg("in_thread"), Some(&json!(true)));

    // 3) 卡片：原地 PATCH 同一条，置 failed —— 群里那张绿不了的卡是人最先看见的
    assert_eq!(
        platform.inner.count("send_card"),
        0,
        "收残局不许新发卡片，只能原地更新"
    );
    let snapshots = platform.inner.card_snapshots(orphan.card_id.as_deref());
    assert_eq!(snapshots.last().expect("快照").status, CardStatus::Failed);
    assert_eq!(snapshots.last().expect("快照").task_no, orphan.task_no);

    // 4) evidence：failed 事件 + manifest，root_hash 回写进库
    let kinds: Vec<EvidenceKind> = read_events(&config, &orphan.id)
        .iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            EvidenceKind::TaskCreated, // 崩溃前 handle_event 写下的两条
            EvidenceKind::EventReceived,
            EvidenceKind::Failed, // 收残局补上的这一条
        ]
    );
    let payload = read_events(&config, &orphan.id)
        .last()
        .expect("最后一条")
        .payload
        .clone()
        .expect("内联 payload");
    assert_eq!(payload["reason"], json!(ORPHAN_RESULT_SUMMARY));
    assert_eq!(payload["by"], json!("startup_recovery"));
    assert_eq!(
        read_manifest(&config, &orphan.id)["root_hash"],
        json!(closed.evidence_root_hash)
    );

    // 5) 用户看得见的那一面：`!status` 不再挂着它
    platform
        .emit(&Ev::new("e-st", "!status").message_id("om_s").build())
        .await;
    wait_until(|| platform.inner.count("send_text") == 2, "!status 回帖").await;
    let last = platform.inner.texts().last().cloned().unwrap_or_default();
    assert!(!last.contains(&orphan.task_no), "{last}");

    run.shutdown().await.expect("run_app 正常收场");
}

/// 收拾必须排在 `platform.start()` 之前。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orphans_are_closed_before_the_platform_starts() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    crash_with_a_task_in_flight(&config, &platform, "画个图", ROOT).await;

    let (app, _model) = reopen(&config, &platform, "这次答上了。").await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    let methods = platform.inner.calls.methods();
    let start_at = methods
        .iter()
        .position(|m| m == "start")
        .unwrap_or_else(|| panic!("没调 start：{methods:?}"));
    let card_at = methods
        .iter()
        .position(|m| m == "update_card")
        .unwrap_or_else(|| panic!("没改卡片：{methods:?}"));
    let text_at = methods
        .iter()
        .position(|m| m == "send_text")
        .unwrap_or_else(|| panic!("没回帖：{methods:?}"));
    assert!(card_at < start_at, "卡片收在 start() 之后了：{methods:?}");
    assert!(text_at < start_at, "回帖发在 start() 之后了：{methods:?}");

    run.shutdown().await.expect("run_app 正常收场");
}

/// M6 正题：收干净之后，在旧线程追问照样续接。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_question_in_the_old_thread_still_lands() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let orphan = crash_with_a_task_in_flight(&config, &platform, "画个图", ROOT).await;

    let (app, model) = reopen(&config, &platform, "这次答上了。").await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    platform
        .emit(
            &Ev::new("e9", "接着上面那个问题")
                .mentioned(false)
                .message_id("om_9")
                .thread_id(ROOT)
                .build(),
        )
        .await;
    // handle_event 在 emit 里同步走完，任务当场就在库里
    let new_task = app.store.list_active_tasks(CHAT).await.expect("list")[0].clone();
    assert_eq!(new_task.task_no, "#A2");
    assert_ne!(new_task.task_no, orphan.task_no);
    assert_eq!(
        new_task.session_id, orphan.session_id,
        "同一会话，话题锚点没断"
    );

    wait_until(|| platform.inner.count("send_text") == 2, "新任务交付").await;
    let prompts = model.prompt_texts();
    assert!(
        prompts
            .iter()
            .any(|ms| ms.iter().any(|t| t.contains("画个图"))),
        "旧话题的上下文没喂进 prompt：{prompts:?}"
    );

    run.shutdown().await.expect("run_app 正常收场");
}

/// 库里没有残局时，起飞一句话都不许说。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_to_recover_stays_quiet() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let (app, _model) = make_app(&config, platform.clone(), "干完了。").await;

    let mut run = RunningApp::start(app.clone(), &platform, None).await;
    settle().await;
    assert_eq!(platform.inner.outbound_count(), 0);
    run.shutdown().await.expect("run_app 正常收场");
    assert_eq!(evidence_task_dirs(&config), Vec::<String>::new());
}

// --------------------------------------------------------------------------
// 一个孤儿收不掉，不许连累别人
// --------------------------------------------------------------------------

/// 让指定 task 的 `evidence.append` / `finalize` 一律失败。
///
/// 真机上最常见的触发原因是崩溃留下的半行 JSON（写入侧自愈归 T21 那条账）；这里**不**用
/// 残行来演 —— 要钉的是「起飞这一层碰上写不进去的证据怎么降级」，那是本轨自己的账，
/// 绑着 evidence 面的当期实现写，等那边一修好这条测试就变成空转。
///
/// 手法：把 `events.jsonl` 这个**文件名占成一个目录**。`FileEvidenceWriter` 既读不出
/// 链尾也写不进去，两头都只能返回 Err —— 比包一层假 writer 更接近真实故障
/// （真机上就是盘/权限级的失败），也不用把组装逻辑抄第二遍。
fn break_evidence_dir(config: &aite_contracts::AiteConfig, task_id: &str) {
    let dir = std::path::Path::new(&config.storage.evidence_dir).join(task_id);
    std::fs::create_dir_all(&dir).expect("建目录");
    // events.jsonl 变成一个目录：`append` 打不开它，只能失败。
    let events = dir.join("events.jsonl");
    let _ = std::fs::remove_file(&events);
    std::fs::create_dir_all(&events).expect("把 events.jsonl 占成目录");
}

/// evidence 写不进去，但群里那两件事照做，进程照常起飞。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_evidence_chain_still_gets_the_group_told() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let orphan = crash_with_a_task_in_flight(&config, &platform, "画个图", ROOT).await;
    break_evidence_dir(&config, &orphan.id);

    let (app, _model) = reopen(&config, &platform, "这次答上了。").await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    let closed = read_task_from_disk(&config, &orphan.id)
        .await
        .expect("任务还在库里");
    assert_eq!(closed.status, TaskStatus::Failed);

    // append 都没成，manifest 自然也不该有：没写进去的东西不许假装收了口
    assert!(!manifest_path(&config, &orphan.id).is_file());
    assert!(
        closed
            .evidence_root_hash
            .as_deref()
            .unwrap_or("")
            .is_empty()
    );

    // 但人看得见的两件事一件没少
    assert_eq!(platform.inner.count("send_text"), 1);
    assert!(platform.inner.texts()[0].contains(&orphan.task_no));
    assert_eq!(
        platform
            .inner
            .card_snapshots(orphan.card_id.as_deref())
            .last()
            .expect("快照")
            .status,
        CardStatus::Failed
    );

    run.shutdown().await.expect("run_app 正常收场");
}

/// 两个孤儿，头一个怎么收都收不掉，第二个照样收干净。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_hopeless_orphan_does_not_block_the_others() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let orphan_a = crash_with_a_task_in_flight(&config, &platform, "画个图", ROOT).await;
    let orphan_b = crash_with_a_task_in_flight(&config, &platform, "写个稿", "om_2").await;

    // 头一个：证据写不进去，卡片也 PATCH 不动（fail_next 抛完即清，只砸第一次调用）
    break_evidence_dir(&config, &orphan_a.id);
    let (app, _model) = reopen(&config, &platform, "这次答上了。").await;
    platform.inner.fail_next("update_card", "飞书那头 500 了");
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    for orphan in [&orphan_a, &orphan_b] {
        let closed = read_task_from_disk(&config, &orphan.id)
            .await
            .expect("任务还在库里");
        assert_eq!(closed.status, TaskStatus::Failed, "{}", orphan.task_no);
    }

    // 两个都回了帖 —— 头一个的 evidence 和卡片全军覆没也没吞掉它自己那条
    let texts = platform.inner.texts();
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains(&orphan_a.task_no)),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains(&orphan_b.task_no)),
        "{texts:?}"
    );

    // 第二个的证据链是完整收口的：头一个的塌方没有蔓延
    assert_eq!(
        read_manifest(&config, &orphan_b.id)["event_count"],
        json!(3)
    );
    assert_eq!(
        platform
            .inner
            .card_snapshots(orphan_b.card_id.as_deref())
            .last()
            .expect("快照")
            .status,
        CardStatus::Failed
    );
    assert!(
        app.store
            .list_active_tasks(CHAT)
            .await
            .expect("list")
            .is_empty()
    );

    run.shutdown().await.expect("run_app 正常收场");
}

/// session 行没了：回帖和卡片没有收件人，只写 evidence，起飞照常。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_orphan_without_a_session_does_not_block_takeoff() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let orphan = crash_with_a_task_in_flight(&config, &platform, "画个图", ROOT).await;

    // 直接把 session 行删掉 —— 崩溃现场里库残了就是这个样子
    delete_session_row(&config.storage.sqlite_path, &orphan.session_id);

    let (app, _model) = reopen(&config, &platform, "这次答上了。").await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    let closed = read_task_from_disk(&config, &orphan.id)
        .await
        .expect("任务还在库里");
    assert_eq!(closed.status, TaskStatus::Failed);

    // 硬发只会发到别处去，所以一句话都不说
    assert_eq!(platform.inner.count("send_text"), 0);
    assert_eq!(platform.inner.count("update_card"), 0);

    // evidence 只要 task_id 就够，照写照收口
    assert_eq!(
        read_events(&config, &orphan.id)
            .last()
            .expect("最后一条")
            .kind,
        EvidenceKind::Failed
    );
    assert_eq!(
        read_manifest(&config, &orphan.id)["root_hash"],
        json!(closed.evidence_root_hash)
    );

    run.shutdown().await.expect("run_app 正常收场");
}

/// 连「查出残局」这一步都炸了，进程照样起得来。
///
/// 怎么让它炸：把 `tasks` 表换成**缺 `data` 列**的样子。`init()` 的
/// `CREATE TABLE IF NOT EXISTS` 与 `CREATE INDEX … (session_id)` 照样过得去，
/// 而 `recover_orphan_tasks` 的 `SELECT data FROM tasks` 必然报「no such column」——
/// 正好只砸这一支，不连累起飞。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_store_query_does_not_block_takeoff() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    // 先正常 init 一次把表建出来，再把 tasks 表改坏
    {
        let store = SqliteSessionStore::open(&config.storage.sqlite_path).expect("open");
        store.init().await.expect("init");
        store.close().await.expect("close");
    }
    break_tasks_table(&config.storage.sqlite_path);

    let (app, _model) = make_app(&config, platform.clone(), "干完了。").await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;
    // 起飞成功：start 调过了，进程活着，一句话都没往群里发
    assert!(platform.inner.started());
    assert_eq!(platform.inner.outbound_count(), 0);
    run.shutdown().await.expect("run_app 正常收场");
}

/// `store.init()` 成功之后抛出的异常必须走到 `store.close()`，否则进程想退退不出去。
///
/// Python 版断言的是 aiosqlite 那条**非 daemon** 工作线程被收掉了；Rust 的
/// `SqliteSessionStore` 没有后台线程（`spawn_blocking` 用的是 tokio 的池子），
/// 所以这里钉的是等价的直接证据：`close()` 真的被调了 —— 表现为 store 之后
/// 一律回 `NotInitialized`，而连接已经释放（同一个文件能被新 store 打开并读到数据）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_is_closed_when_takeoff_explodes_after_init() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    // 让 `platform.start()` 失败：那是 `store.init()` 成功之后的第一个可失败点。
    platform.fail_start("投递面起不来");
    let (app, _model) = make_app(&config, platform.clone(), "干完了。").await;

    let opts = aite_app::ServeOptions {
        stop: Some(aite_app::StopSignal::new()),
        shutdown_grace_sec: 1.0,
        install_signals: false,
    };
    let err = aite_app::run_app(&app, &opts)
        .await
        .expect_err("start 失败必须让起飞失败");
    assert!(err.0.contains("投递面起不来"), "{err}");

    // store.close() 走到了
    assert!(
        app.store.get_task("whatever").await.is_err(),
        "init 成功之后炸掉，收尾里的 store.close() 没走到"
    );
    // 连接真的释放了：同一个文件能被新 store 打开
    let again = SqliteSessionStore::open(&config.storage.sqlite_path).expect("open");
    again.init().await.expect("init");
    assert!(
        again
            .get_task("whatever")
            .await
            .expect("get_task")
            .is_none()
    );
    again.close().await.expect("close");
}

// --------------------------------------------------------------------------
// 直接动库（崩溃现场的那些残状）
// --------------------------------------------------------------------------

fn delete_session_row(db_path: &str, session_id: &str) {
    let conn = rusqlite::Connection::open(db_path).expect("open db");
    conn.execute(
        "DELETE FROM sessions WHERE id = ?1",
        rusqlite::params![session_id],
    )
    .expect("删 session 行");
}

/// 把 `tasks` 表换成**缺 `data` 列**的样子。
///
/// 留着 `session_id`，这样 `init()` 里那条 `CREATE INDEX … (session_id)` 照样建得出来 ——
/// 只有 `recover_orphan_tasks` 的 `SELECT data FROM tasks` 会红。
fn break_tasks_table(db_path: &str) {
    let conn = rusqlite::Connection::open(db_path).expect("open db");
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_tasks_session;
         DROP TABLE IF EXISTS tasks;
         CREATE TABLE tasks (
            id          TEXT PRIMARY KEY,
            session_id  TEXT NOT NULL,
            task_no     TEXT NOT NULL,
            status      TEXT NOT NULL,
            created_at  TEXT NOT NULL
         );",
    )
    .expect("改 tasks 表");
}
