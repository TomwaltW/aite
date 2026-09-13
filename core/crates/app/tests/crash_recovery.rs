//! 崩溃恢复里**进程级**的那几条（移植 `test_t18_crash_recovery.py` 里不归 R3 的部分）。
//!
//! T18 那 11 条里，8 条测的是存储 / 证据面本身，R3 已经移植进
//! `core/crates/store/tests/crash_recovery.rs`（僵尸任务、`recover_orphan_tasks`、
//! `journal_mode == delete`、崩溃前的数据存活、`seen_event` 抛而不是静默 false、
//! 并发写中崩溃）与 `core/crates/evidence/tests/torn_tail*.rs`（残行三条）。
//! 剩下这三条只有装好的进程才答得上，所以落在这里。
//!
//! **怎么模拟崩溃**：不真 `kill -9`（测试里不好控），而是「不走收尾路径」——
//! `build_app` + `store.init()` 把状态造出来，然后**一步收尾都不走**，只把连接关掉，
//! 再在同一个 .db 文件上重开一套组装。
mod common;

use common::*;

use aite_contracts::{SessionStore, Task, TaskStatus};
use aite_store::SqliteSessionStore;
use aite_testing::FakeSandbox;
use std::sync::Arc;

const ROOT: &str = "om_1";

/// 一套组装。`replies` 一句一步 —— **任务有几个就得给几步**，
/// 少给一步的那个任务会撞 `ScriptExhausted`（`store_failure_…` 就是这么假绿了一整轨）。
async fn make_app(
    config: &aite_contracts::AiteConfig,
    platform: Arc<GatedPlatform>,
    replies: &[&str],
) -> (Arc<aite_app::AiteApp>, Arc<RecordingModel>) {
    let model = RecordingModel::new(replies.iter().map(|r| final_step(r)).collect());
    let app = build_with(
        config.clone(),
        platform,
        model.clone(),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    (app, model)
}

/// 造一个「正在跑」的任务，然后崩溃。返回 (task_id, task_no)。
async fn crash_with_a_task_in_flight(
    config: &aite_contracts::AiteConfig,
    platform: &Arc<GatedPlatform>,
) -> (String, String) {
    let (app, _model) = make_app(config, platform.clone(), &["不会用到"]).await;
    app.store.init().await.expect("init"); // run_app 起飞时做的那一步
    app.plane
        .handle_event(Ev::new("e1", "画个图").message_id(ROOT).build())
        .await
        .expect("handle_event");

    let mut task = first_active_task(&app.store, CHAT, "handle_event 把任务建出来").await;
    task.status = TaskStatus::Working; // 崩溃那一刻它正在干活
    app.store.update_task(&task).await.expect("update_task");

    app.store.close().await.expect("close"); // 只关连接；收尾序列全跳过
    (task.id, task.task_no)
}

/// **钉住现状（这是个真问题）**：崩溃时在跑的任务，重开后还是 `working`，
/// 而且群里的 `!status` 真的会把它列出来 —— 这才是用户看得见的那一面。
///
/// R3 的 store 版只验到 `list_active_tasks` 还挂着它；`!status` 那一半要控制面才答得上。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_leaves_a_zombie_task_hanging_on_status() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let (task_id, task_no) = crash_with_a_task_in_flight(&config, &platform).await;

    // 不走 run_app（那会把孤儿收掉），直接 init 一套新的看现状
    platform.inner.calls.clear();
    let (app2, _model) = make_app(&config, platform.clone(), &["干完了。"]).await;
    app2.store.init().await.expect("init");

    let still_there = app2.store.list_active_tasks(CHAT).await.expect("list");
    assert_eq!(
        still_there.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        vec![task_id]
    );
    assert_eq!(still_there[0].status, TaskStatus::Working, "没人动过它");

    app2.plane
        .handle_event(Ev::new("e2", "!status").message_id("om_s").build())
        .await
        .expect("handle_event");
    let last = platform.inner.texts().last().cloned().unwrap_or_default();
    assert!(last.contains(&task_no), "!status 该列出这个僵尸：{last}");

    app2.store.close().await.expect("close");
}

/// M6 正题：杀进程重启后**在旧线程追问，仍能续接**。
///
/// 收拾掉僵尸只是把摊子收干净，用户要的是接着问下去 —— 命中同一会话、
/// 模型上下文里带着崩溃前那一轮、任务号接着往下发。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_does_not_block_new_work_in_the_same_thread() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let (old_id, old_no) = crash_with_a_task_in_flight(&config, &platform).await;

    platform.inner.calls.clear();
    let (app, model) = make_app(&config, platform.clone(), &["这次答上了。"]).await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    // 起飞把孤儿收了，旧任务落 failed
    let closed = read_task_from_disk(&config, &old_id)
        .await
        .expect("任务还在库里");
    assert_eq!(closed.status, TaskStatus::Failed);

    // 旧话题里追问，不带 @（R6）
    platform
        .emit(
            &Ev::new("e9", "接着上面那个问题")
                .mentioned(false)
                .message_id("om_9")
                .thread_id(ROOT)
                .build(),
        )
        .await;
    let new_task = first_active_task(&app.store, CHAT, "旧话题里那句追问建出新任务").await;
    assert_eq!(
        new_task.session_id, closed.session_id,
        "同一会话，话题锚点没断"
    );
    assert_ne!(new_task.task_no, old_no);
    assert_eq!(new_task.task_no, "#A2", "任务号接着往下发，不回退也不复用");

    let p = platform.clone();
    // 收孤儿那条回帖 + 新任务的交付 = 2
    wait_until(|| p.inner.count("send_text") == 2, "新任务交付").await;
    assert!(
        model
            .prompt_texts()
            .iter()
            .any(|ms| ms.iter().any(|t| t.contains("画个图"))),
        "崩溃前那一轮没进新进程的模型上下文"
    );
    run.shutdown().await.expect("run_app 正常收场");
}

/// 盘恢复之后那个任务的答复原文。判「交付了」看的是**这句话有没有发出去**，
/// 不是 `count("send_text")` —— 计数正是这条用例栽过的地方（见下面那段注释）。
const RECOVERED_REPLY: &str = "恢复之后这条也干完了。";

/// 这条用例自己的等待死线，不吃 `WAIT_TIMEOUT_SEC`（5.0）。
///
/// 最坏路径：等的每一步背后都压着存储写，而 `aite_store::BUSY_TIMEOUT_MS` 就是 5000 ——
/// 一次撞锁的写**合法地**要等满 5s 才回 SQLITE_BUSY。死线取 5.0 的话，「还在合法重试」
/// 和「彻底不动了」在时间上完全重叠，这条用例就永远说不清自己红在哪。取 2 倍留满一倍
/// 余量；和 `RunningApp::shutdown` 的 10s 同量级、远短于 20s 的默认停机宽限，真挂死时
/// 还是快速红，不会把整套测试拖住。
const RECOVERY_WAIT_SEC: f64 = 10.0;

/// 等到每个已经开跑的任务在库里都落成终态 —— 即 worker 的**最后一笔** `update_task`
/// 也写完了。
///
/// 为什么不能只等 `send_text`：交付回帖发出去之后 worker 还要写 evidence、关卡片、把
/// delivered 落库（`agent.rs` 的 `deliver → finish → save`）。这条用例得等到那之后才动
/// 目录权限，否则只读窗口有相当概率砸在那笔 `update_task` 上，第一个任务反倒失败回帖。
///
/// 为什么不看 `list_active_tasks`：`deliver` 第一件事就是把状态改成 `Answering` 再 save，
/// 而 `Answering` 不在 `ACTIVE_TASK_STATUSES` 里 —— 那一刻列表已经空了，可后面还有三四笔
/// 写没落。任务号从 evidence 目录名拿（目录名就是 task_id）。
async fn wait_tasks_settled(
    app: &Arc<aite_app::AiteApp>,
    config: &aite_contracts::AiteConfig,
    what: &str,
) {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs_f64(RECOVERY_WAIT_SEC);
    loop {
        let ids = evidence_task_dirs(config);
        let mut pending: Vec<String> = Vec::new();
        for id in &ids {
            match app.store.get_task(id).await.expect("get_task") {
                Some(t) if t.status.is_terminal() => {}
                Some(t) => pending.push(format!("{}={}", t.task_no, t.status.as_str())),
                None => pending.push(format!("{id}=库里没有")),
            }
        }
        if !ids.is_empty() && pending.is_empty() {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("{RECOVERY_WAIT_SEC}s 内没等到：{what}（还挂着 {pending:?}）");
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
}

/// 按 `result_summary` 从**磁盘**上把任务捞出来。
///
/// evidence 的目录名就是 task_id，所以不用在跑的时候去抢 `list_active_tasks` 那一瞬间
/// （交付得太快，抢不稳）。
async fn task_by_summary(config: &aite_contracts::AiteConfig, summary: &str) -> Option<Task> {
    for id in evidence_task_dirs(config) {
        if let Some(task) = read_task_from_disk(config, &id).await
            && task.result_summary == summary
        {
            return Some(task);
        }
    }
    None
}

/// 存储层写失败 → 进程不退出（§3.3 最后一行）。
///
/// db 目录只读（写不出 journal）时投事件：`Ingress::on_event` 把错误兜住并计
/// `ingress.errors`，进程必须还活着、投递面也不许自己闭嘴；盘恢复后还能接着干活。
///
/// 另一半（task failed + 回帖 + evidence failed）在这条路上不成立：异常发生在
/// `seen_event`，此刻连 task 都还没建，所以没有 task 可以标 failed、也没有帖可回 ——
/// 事件被静默丢掉。这是继承自 Python 版的已知边界，不是本轨引入的。
///
/// **这条用例从前是假的**（RΩ 合流前修）。旧写法只给一步模型脚本，恢复后的任务必然
/// `ScriptExhausted`，worker 按 `[0s, 2s, 5s]` 重试三发 = 7s 之后才发得出任何 `send_text`，
/// 而死线是 5s —— 物理上走不通。它 20 次里绿 11 次，靠的是只读窗口偶尔砸在第一个任务
/// 交付后的 `update_task` 上、worker 走 `fail()` 多回一条错误帖，把 `count == 2` 凑够：
/// **系统表现更差的那些次才绿**。现在改成：第二个任务有自己的脚本步，判据是它自己的
/// 答复原文 + 库里落成 delivered。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_failure_does_not_kill_the_process() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let db_parent = std::path::Path::new(&config.storage.sqlite_path)
        .parent()
        .expect("父目录")
        .to_path_buf();
    let platform = GatedPlatform::new();
    // 两步脚本：只读窗口之前一个任务，盘恢复之后一个任务，各吃一步。
    let (app, _model) = make_app(&config, platform.clone(), &["干完了。", RECOVERED_REPLY]).await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    platform.emit(&event("ok-1", "正常的活")).await;
    let p = platform.clone();
    wait_until_within(
        RECOVERY_WAIT_SEC,
        || p.inner.count("send_text") == 1,
        "第一个任务交付",
    )
    .await;
    wait_tasks_settled(&app, &config, "第一个任务收完尾").await;

    // 只读目录：SQLite 建不出 -journal
    let restore = std::fs::metadata(&db_parent)
        .expect("metadata")
        .permissions();
    std::fs::set_permissions(&db_parent, std::fs::Permissions::from_mode(0o500)).expect("改成只读");
    let before = app.ingress.counter("ingress.errors");
    platform
        .emit(
            &Ev::new("boom", "这条会撞上只读磁盘")
                .message_id("om_b")
                .build(),
        )
        .await;
    settle().await;
    let errors = app.ingress.counter("ingress.errors");
    std::fs::set_permissions(&db_parent, restore).expect("恢复权限");

    assert_eq!(errors, before + 1, "异常没被 Ingress 兜住");
    assert!(platform.inner.started(), "投递面自己闭嘴了");
    assert!(!platform.inner.stopped(), "投递面自己停了");
    // 撞上只读盘的那条事件是被静默丢掉的：不建 task、不回帖。多出来的任何一条
    // 都说明只读窗口砸到了别人身上（旧写法就是靠这条回帖假绿的）。
    assert_eq!(
        platform.inner.count("send_text"),
        1,
        "只读窗口只该吃掉 boom 那一条事件，回帖数不该动：{:?}",
        platform.inner.texts()
    );

    // 磁盘恢复后还能接着干活 —— 不是活着但废了
    platform
        .emit(&Ev::new("ok-2", "恢复后的活").message_id("om_c").build())
        .await;
    let p = platform.clone();
    wait_until_within(
        RECOVERY_WAIT_SEC,
        || p.inner.texts().iter().any(|t| t == RECOVERED_REPLY),
        "盘恢复之后的任务把自己的答复发出来",
    )
    .await;
    wait_tasks_settled(&app, &config, "恢复后的任务收完尾").await;

    // 「交付」不是发条消息就算数：任务自己要在库里落成 delivered，号也要接着往下发。
    let recovered = task_by_summary(&config, RECOVERED_REPLY)
        .await
        .expect("库里找不到恢复后的那个任务");
    assert_eq!(
        recovered.status,
        TaskStatus::Delivered,
        "恢复后的任务没走完"
    );
    assert_eq!(recovered.task_no, "#A2", "任务号接着往下发，不回退也不复用");

    run.shutdown().await.expect("run_app 正常收场");
}

/// 崩溃前 commit 过的东西一个字都不许丢（进程级：从**装好的 app** 那条路走一遍）。
///
/// R3 的 store 版直接调 store；这条走的是 `build_app` 之后的 `app.store`，
/// 顺带验「话题锚点还在」与「任务号接着发」。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_written_before_the_crash_survives() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let (task_id, task_no) = crash_with_a_task_in_flight(&config, &platform).await;
    assert_eq!(task_no, "#A1");

    let store = SqliteSessionStore::open(&config.storage.sqlite_path).expect("open");
    store.init().await.expect("init");
    let task = store
        .get_task(&task_id)
        .await
        .expect("get_task")
        .expect("任务还在");
    assert_eq!(task.status, TaskStatus::Working);

    let session = store
        .find_session_by_thread(CHAT, ROOT)
        .await
        .expect("find_session_by_thread");
    assert!(session.is_some(), "话题锚点没了");
    let turns = store
        .list_turns(&task.session_id, 100)
        .await
        .expect("list_turns");
    assert_eq!(
        turns.iter().map(|t| t.content.clone()).collect::<Vec<_>>(),
        vec!["画个图"]
    );
    assert_eq!(
        store.next_task_no(TENANT).await.expect("next_task_no"),
        "#A2",
        "号接着发，不回退"
    );
    store.close().await.expect("close");
}
