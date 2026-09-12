//! 起飞与收尾（对应旧 `aite/app.py` 的 `run_app` / `_shutdown` / `_recover_orphans`）。
//!
//! 退出序列由 C-TΩ-1 冻结，一步都不跳（`review/inventory-core.md` §7）：
//!
//! ```text
//! platform.stop()                     先闭嘴，不再收新事件
//! plane.join() 限时 grace             在跑 / 排队的任务收尾
//!   超时 -> 先抄 worker.in_flight     取消之后就再也问不出它们是谁了
//! runner.abort()                      取消 run_forever 那条 task
//! cancel_task(notify=false) x stranded 给硬取消的任务善终
//! sandbox.close_all()
//! store.close()
//! ```
//!
//! `store.init()` 也在「try」里面：它一成功就有一条 SQLite 连接挂着，此后任何一处失败
//! 都必须走到收尾里的 `store.close()`。起飞阶段的收残局正落在这个窗口里。
use std::sync::Arc;
use std::time::Duration;

use aite_contracts::{EvidenceKind, OutboundText, Session, Task};
use aite_store::ORPHAN_RESULT_SUMMARY;
use aite_worker::card::render_card;
use serde_json::{Map, Value};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::app::{AiteApp, StartupError, initiator_of};

/// 优雅退出的宽限期，对齐 `docker-compose.yml` 的 `stop_grace_period: 20s`。
pub const DEFAULT_SHUTDOWN_GRACE_SEC: f64 = 20.0;

/// 停机信号。`watch<bool>` 而不是 `Notify`：收尾路径要能「事后问一次有没有被置起来」，
/// 而 `Notify` 的通知是一次性的，错过就没了。
#[derive(Clone)]
pub struct StopSignal {
    tx: Arc<watch::Sender<bool>>,
}

impl Default for StopSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl StopSignal {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(false);
        Self { tx: Arc::new(tx) }
    }

    pub fn set(&self) {
        let _ = self.tx.send(true);
    }

    pub fn is_set(&self) -> bool {
        *self.tx.borrow()
    }

    /// 等到被置起来（已经置起来了就立刻返回）。
    pub async fn wait(&self) {
        let mut rx = self.tx.subscribe();
        if *rx.borrow_and_update() {
            return;
        }
        // 发送端挂在 Arc 上，和本对象同生共死：changed() 只会因为 set() 返回。
        let _ = rx.changed().await;
    }
}

#[derive(Clone)]
pub struct ServeOptions {
    /// 调用方的停机开关；不给就自己建一个（集成测试靠它不用发真信号）。
    pub stop: Option<StopSignal>,
    pub shutdown_grace_sec: f64,
    /// 装 SIGINT / SIGTERM。测试里（非主线程 / 不想碰进程信号）置 false。
    pub install_signals: bool,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            stop: None,
            shutdown_grace_sec: DEFAULT_SHUTDOWN_GRACE_SEC,
            install_signals: true,
        }
    }
}

/// 起飞，然后一直跑到 SIGINT / SIGTERM 或调用方把 `stop` 置起来。
///
/// 返回 `Err` 只代表**起飞阶段**失败（建表 / 投递面起不来）；收尾一定会走完。
pub async fn run_app(app: &AiteApp, opts: &ServeOptions) -> Result<(), StartupError> {
    let stop = opts.stop.clone().unwrap_or_default();
    let signals = if opts.install_signals {
        install_signal_handlers(stop.clone())
    } else {
        None
    };

    let mut runner: Option<JoinHandle<()>> = None;
    let outcome = takeoff(app, &stop, &mut runner).await;
    shutdown(app, runner.take(), opts.shutdown_grace_sec).await;
    // handler 一直挂到收尾做完才摘：人再按一次 Ctrl-C 基本都是**因为**收尾在磨蹭，
    // 提前摘掉的话第二次信号就落回默认处理，硬退这条路等于没有。
    if let Some(h) = signals {
        h.abort();
    }
    outcome
}

async fn takeoff(
    app: &AiteApp,
    stop: &StopSignal,
    runner_slot: &mut Option<JoinHandle<()>>,
) -> Result<(), StartupError> {
    // 建表。事件在 `platform.start` 返回的下一刻就可能到，表得先在（§3.2 init 幂等）。
    app.store.init().await.map_err(|e| {
        StartupError(format!(
            "建表失败（{}）：{e}",
            app.config.storage.sqlite_path
        ))
    })?;
    log_takeoff(app);
    // `platform.start` 之前：出站链（回帖 / 改卡片）不依赖投递面，而 start 在真机上
    // 是"起监听 / 建长连接"，它之后的代码要等到 stop 才轮得到。
    recover_orphans(app).await;
    app.platform
        .start(app.ingress.handler())
        .await
        .map_err(|e| StartupError(format!("投递面起不来：{e}")))?;

    let plane = app.plane.clone();
    let handle = tokio::spawn(async move { plane.run_forever().await });
    *runner_slot = Some(handle);
    serve(stop, runner_slot.as_mut().expect("刚放进去")).await;
    Ok(())
}

/// 等停机信号。派发循环先一步死掉的话也别干等 —— 那时该进收尾。
async fn serve(stop: &StopSignal, runner: &mut JoinHandle<()>) {
    tokio::select! {
        _ = stop.wait() => {}
        joined = &mut *runner => {
            match joined {
                Ok(()) => tracing::error!(target: "aite.app", "aite.plane_died 派发循环自己返回了，进入收尾"),
                Err(e) if e.is_cancelled() => {}
                Err(e) => tracing::error!(target: "aite.app", error = %e, "aite.plane_died 派发循环异常收场，进入收尾"),
            }
        }
    }
}

/// 一行「接了谁」。真机排障时这是第一现场，别省。
fn log_takeoff(app: &AiteApp) {
    tracing::info!(
        target: "aite.app",
        platform = %app.config.platform,
        model = %{ let n = app.model.name(); if n.is_empty() { "(未命名)".to_string() } else { n } },
        sandbox = %if app.sandbox.is_some() { app.config.sandbox.image.clone() } else { "(无沙箱)".to_string() },
        sqlite = %app.config.storage.sqlite_path,
        evidence = %app.config.storage.evidence_dir,
        edge = %app.edge.as_ref().map(|e| e.edge_socket().display().to_string()).unwrap_or_else(|| "(注入的替身)".to_string()),
        "aite.up"
    );
}

// --------------------------------------------------------------------------
// 收尾
// --------------------------------------------------------------------------

async fn shutdown(app: &AiteApp, runner: Option<JoinHandle<()>>, grace: f64) {
    tracing::info!(target: "aite.app", grace, pending = app.plane.pending(), "aite.stopping");

    if let Err(e) = app.platform.stop().await {
        tracing::warn!(target: "aite.app", error = %e, "aite.platform_stop_failed");
    }

    let mut stranded: Vec<(Task, Session)> = Vec::new();
    if runner.as_ref().is_some_and(|h| !h.is_finished())
        && tokio::time::timeout(Duration::from_secs_f64(grace.max(0.0)), app.plane.join())
            .await
            .is_err()
    {
        // 超时这一支：先把在飞的任务抄下来，取消之后就再也问不出它们是谁了。
        stranded = app.worker.in_flight();
        tracing::warn!(
            target: "aite.app",
            grace,
            running = stranded.len(),
            pending = app.plane.pending(),
            "aite.shutdown_timeout 宽限期内没收完，强行取消"
        );
    }

    if let Some(h) = runner {
        h.abort();
        let _ = h.await;
    }

    // 被硬取消的任务自己没机会收场（worker 的 cancel 分支只在每步开头查标志位）。
    // 取消已经跑完、控制面的 running 也空了，此刻走 `cancel_task` 正好命中「不在跑」
    // 那条：写 evidence cancelled、卡片置 cancelled、把 Gateway 手上的沙箱还掉。
    for (task, _session) in stranded {
        app.plane.cancel_task(task, None, None, false).await;
    }

    if let Some(sandbox) = &app.sandbox
        && let Err(e) = sandbox.close_all().await
    {
        tracing::warn!(target: "aite.app", error = %e, "aite.sandbox_close_failed");
    }
    if let Err(e) = app.store.close().await {
        tracing::warn!(target: "aite.app", error = %e, "aite.store_close_failed");
    }
    tracing::info!(target: "aite.app", "aite.down");
}

// --------------------------------------------------------------------------
// 孤儿收场（§2.4 M6 的重启场景）
// --------------------------------------------------------------------------

/// 把上一条命遗留的活跃任务收干净。
///
/// §3.3 对失败的要求是三件事：task `failed` + 回帖 + evidence `failed` 事件。第一件
/// `recover_orphan_tasks()` 已经在库里做完了，这里补后两件，外加把停在「进行中」的
/// 卡片置成 failed（W4）。**一个孤儿收不掉，不许连累别人，更不许让进程起不来。**
pub(crate) async fn recover_orphans(app: &AiteApp) {
    let orphans = match app.store.recover_orphan_tasks().await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(target: "aite.app", error = %e, "aite.recover_failed 上一条命的残局没查出来，照常起飞");
            return;
        }
    };
    if orphans.is_empty() {
        return;
    }
    tracing::warn!(target: "aite.app", n = orphans.len(), "aite.orphans 上一条命没跑完的任务，逐个收场");
    for task in orphans {
        close_orphan(app, task).await;
    }
}

/// 一个孤儿的收场：evidence → 卡片 → 回帖。三步彼此独立，谁炸都不挡后面的。
async fn close_orphan(app: &AiteApp, task: Task) {
    let mut task = task;
    let session: Option<Session> = match app.store.get_session(&task.session_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(target: "aite.app", task = %task.id, error = %e, "aite.orphan_session_failed");
            None
        }
    };
    if session.is_none() {
        tracing::warn!(
            target: "aite.app",
            task = %task.id,
            session = %task.session_id,
            "aite.orphan_no_session 只写 evidence，不回帖"
        );
    }

    let mut payload = Map::new();
    payload.insert(
        "reason".to_string(),
        Value::String(ORPHAN_RESULT_SUMMARY.to_string()),
    );
    payload.insert("steps".to_string(), Value::from(task.steps));
    payload.insert(
        "by".to_string(),
        Value::String("startup_recovery".to_string()),
    );
    match app
        .evidence
        .append(&task.id, EvidenceKind::Failed, payload)
        .await
    {
        Ok(_) => finalize_orphan_evidence(app, &mut task, session.as_ref()).await,
        Err(e) => {
            // 崩溃留下的半行 JSON 会让 append 直接抛。证据链残着是既成事实，
            // 但下面那两件「人看得见」的事照做 —— 不然用户那头就真的一点交代都没有。
            tracing::error!(target: "aite.app", task = %task.id, error = %e, "aite.orphan_evidence_failed");
        }
    }

    if let Some(session) = session.as_ref()
        && let Some(card_id) = task.card_id.clone()
        && !card_id.is_empty()
    {
        let card = render_card(
            &task,
            session,
            &initiator_of(session),
            aite_contracts::CardStatus::Failed,
            None,
        );
        if let Err(e) = app.platform.update_card(&card_id, &card).await {
            tracing::error!(target: "aite.app", task = %task.id, card = %card_id, error = %e, "aite.orphan_card_failed");
        }
    }

    if let Some(session) = session.as_ref() {
        // 带任务号：一次崩溃可能留下好几个孤儿，不带号没人知道说的是哪个。
        // 正文照抄库里的 `result_summary`，群里那句和 `!status` 是同一口径。
        let msg = OutboundText {
            chat_id: session.chat_id.clone(),
            text: format!("任务 {}：{}", task.task_no, ORPHAN_RESULT_SUMMARY),
            reply_to: Some(session.anchor.message_id.clone()),
            in_thread: true,
        };
        if let Err(e) = app.platform.send_text(&msg).await {
            tracing::error!(target: "aite.app", task = %task.id, error = %e, "aite.orphan_notice_failed");
        }
    }
}

/// 给孤儿的证据链收口，字段照抄 `plane::finalize_evidence`。
///
/// 不 finalize 的话这个任务的证据目录没有 manifest.json，`aite evidence show` 的时间线
/// 上就是残的 —— 而崩溃留下的目录恰恰是最需要被人翻的那种。
/// `finalize` 一个任务只走一次：已经有 root_hash 的说明崩溃前就收过尾了。
async fn finalize_orphan_evidence(app: &AiteApp, task: &mut Task, session: Option<&Session>) {
    if task
        .evidence_root_hash
        .as_deref()
        .is_some_and(|h| !h.is_empty())
    {
        return;
    }
    let mut extra = Map::new();
    extra.insert(
        "session_id".to_string(),
        Value::String(
            session
                .map(|s| s.id.clone())
                .unwrap_or_else(|| task.session_id.clone()),
        ),
    );
    extra.insert("task_no".to_string(), Value::String(task.task_no.clone()));
    extra.insert(
        "created_by".to_string(),
        Value::String(task.created_by.clone()),
    );
    extra.insert(
        "model".to_string(),
        Value::String(if task.model.is_empty() {
            app.model.name()
        } else {
            task.model.clone()
        }),
    );
    match app.evidence.finalize(&task.id, extra).await {
        Ok(root_hash) => {
            task.evidence_root_hash = Some(root_hash);
            task.updated_at = chrono::Utc::now();
            if let Err(e) = app.store.update_task(task).await {
                tracing::error!(target: "aite.app", task = %task.id, error = %e, "aite.orphan_save_failed");
            }
        }
        Err(e) => {
            tracing::error!(target: "aite.app", task = %task.id, error = %e, "aite.orphan_finalize_failed");
        }
    }
}

// --------------------------------------------------------------------------
// 信号
// --------------------------------------------------------------------------

/// SIGINT / SIGTERM 都进同一条优雅退出路径；第二次立刻硬退 130。
///
/// 返回那条后台 task 的 handle —— 收尾做完才 abort（见 `run_app`）。装不上（比如
/// 平台不支持）就返回 None，那时只认调用方传进来的 `StopSignal`。
fn install_signal_handlers(stop: StopSignal) -> Option<JoinHandle<()>> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(target: "aite.app", error = %e, "aite.signal_unavailable SIGINT");
            return None;
        }
    };
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(target: "aite.app", error = %e, "aite.signal_unavailable SIGTERM");
            return None;
        }
    };

    Some(tokio::spawn(async move {
        let mut hits = 0u32;
        loop {
            let name = tokio::select! {
                _ = sigint.recv() => "SIGINT",
                _ = sigterm.recv() => "SIGTERM",
            };
            hits += 1;
            if hits == 1 {
                tracing::warn!(target: "aite.app", signal = name, "aite.signal 收到，开始优雅退出（再来一次立即硬退）");
                stop.set();
                continue;
            }
            // 别让人 Ctrl-C 按不动：日志可能还压在订阅者的缓冲里，直接写 stderr。
            eprintln!("aite: 又收到 {name}，硬退出。");
            std::process::exit(crate::app::EXIT_HARD_STOP);
        }
    }))
}
