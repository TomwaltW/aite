//! 断线重连后那一批重推，进程级贯通（移植 `test_t24_reconnect_replay.py`，8 条
//! ／两条带参数化，Rust 里拆成两条同义用例，共 10 个 `#[tokio::test]`）。
//!
//! §2.4 M2 的原话是两句：拔网线 30 秒再插回 → 服务自动重连；**断网期间群里发的 @
//! 在重连后被处理且只处理一次**。前半句 adapter 那一层（R1）验过；后半句在进程级
//! 一条都没有 —— 去重只被 `evals/p0/10_duplicate_event.yaml` 验过，而那条投的是
//! **顺序**的两条一模一样的事件。
//!
//! 真机上重连那一刻的形状不长这样。它是**一批**：几条不同的 @、夹着几条重复的、
//! 可能还有断网前就已经处理过的，而且是**同时**进来的。这一组把那个形状造出来，
//! 从 `build_app` + `run_app` 走。
//!
//! **替身写在本文件里，不进 `common`**（§3.2 末：重复远比冲突便宜）。
mod common;

use common::*;

use aite_contracts::{EvidenceKind, NormalizedEvent, Task, TaskStatus};
use aite_testing::ScriptStep;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 断网**之前**就办完的那个活。重连时平台会把它一起重推上来。
const BEFORE_EVENT: &str = "ev-before";
const BEFORE_MSG: &str = "om_before";

/// 断网期间群里发的三个 @，各占一个话题。
const DURING: [(&str, &str); 3] = [("ev-a", "om_a"), ("ev-b", "om_b"), ("ev-c", "om_c")];

/// 给 n 个任务各配一份两步的脚本：先加一项清单（逼出 W3 那张卡），再 final。
///
/// `FakeModel` 的游标是**全脚本共用**的，多任务会串 —— 这里不串是因为 `run_forever`
/// 一次只跑一个任务（`run_one` 跑完才取下一个），模型调用天然是串行的。
/// 多建了任务的话脚本会被吃穿（`ScriptExhausted`），红得很响，这正是我们要的。
fn script_for(n_tasks: usize) -> Vec<ScriptStep> {
    let mut steps = Vec::with_capacity(n_tasks * 2);
    for i in 0..n_tasks {
        steps.push(tool_step(
            "checklist_add",
            json!({"items": [format!("第 {} 个活", i + 1)]}),
        ));
        steps.push(final_step(&format!("第 {} 个活干完了。", i + 1)));
    }
    steps
}

// --------------------------------------------------------------------------
// 替身
// --------------------------------------------------------------------------

/// 会断线、会重推的平台替身（套在 `GatedPlatform` 外面）。
///
/// 与 `GatedPlatform` 的 `stopped` 是两码事：`stopped` 是**本进程在收尾**（不再收新事件），
/// 这里的 `online` 是**长连接断了**（平台还在收消息，只是送不到本进程）。
struct Replay {
    gated: Arc<GatedPlatform>,
    online: Mutex<bool>,
    /// 断线期间平台替用户攒着的事件（真机上是飞书那边的重推队列）
    backlog: Mutex<Vec<NormalizedEvent>>,
    inflight: AtomicUsize,
    /// 同时压在 `on_event` 里的事件数的峰值。
    ///
    /// 「同时到达」这件事光靠 `join_all` 是断言不了的 —— 万一 `handle_event` 一路不让出，
    /// 出来的也是一条跑完再一条。有了这个峰值，并发用例才算真的在验并发。
    max_concurrent: AtomicUsize,
}

impl Replay {
    fn new(gated: Arc<GatedPlatform>) -> Arc<Self> {
        Arc::new(Self {
            gated,
            online: Mutex::new(true),
            backlog: Mutex::new(Vec::new()),
            inflight: AtomicUsize::new(0),
            max_concurrent: AtomicUsize::new(0),
        })
    }

    async fn emit(&self, ev: &NormalizedEvent) {
        if !*self.online.lock().expect("online 锁") {
            self.backlog.lock().expect("backlog 锁").push(ev.clone());
            return;
        }
        let n = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_concurrent.fetch_max(n, Ordering::SeqCst);
        self.gated.emit(ev).await;
        self.inflight.fetch_sub(1, Ordering::SeqCst);
    }

    /// 拔网线。此后 `emit` 的事件都进 backlog，一条也到不了 `on_event`。
    fn unplug(&self) {
        *self.online.lock().expect("online 锁") = false;
    }

    fn backlog_len(&self) -> usize {
        self.backlog.lock().expect("backlog 锁").len()
    }

    fn max_concurrent(&self) -> usize {
        self.max_concurrent.load(Ordering::SeqCst)
    }

    /// 插回网线，把这一批重推上来，返回实际推了哪些。
    ///
    /// `extra` 是「平台多推的那些」—— 重复的、断网前就处理过的。至少一次投递的长连接
    /// 在重连边界上本来就会这样，adapter 明确不去重（§3.3），一律交给 R2。
    ///
    /// `together=true` 是重连那一刻的真形状：整批同时进 `on_event`。
    async fn replug(
        self: &Arc<Self>,
        extra: Vec<NormalizedEvent>,
        together: bool,
    ) -> Vec<NormalizedEvent> {
        *self.online.lock().expect("online 锁") = true;
        let mut batch = std::mem::take(&mut *self.backlog.lock().expect("backlog 锁"));
        batch.extend(extra);
        if together {
            let mut handles = Vec::with_capacity(batch.len());
            for ev in &batch {
                let me = Arc::clone(self);
                let ev = ev.clone();
                handles.push(tokio::spawn(async move { me.emit(&ev).await }));
            }
            for h in handles {
                h.await.expect("emit task");
            }
        } else {
            for ev in &batch {
                self.emit(ev).await;
            }
        }
        batch
    }
}

/// 第一次 `chat` 挂在闸门上不返回，其余照常出牌。
///
/// 用来造「网断的时候，有个任务正跑在半路上」：任务已经被 worker 领走、卡片也发了，
/// 就卡在下一次模型往返上。`hold_ticks` 做不到这件事 —— 它让的是固定几个 tick，
/// 这里要的是「挂到我说放行为止」。
struct GatedModel {
    inner: Arc<RecordingModel>,
    armed: std::sync::atomic::AtomicBool,
    reached: Arc<tokio::sync::Notify>,
    /// 闸门**到过没有**。`reached` 那个 `Notify` 答不了这个问题（通知是一次性的，
    /// 发的时候没人在等就没了），所以另立一个标志位 —— 理由见 `wait_gate_reached`。
    gate_reached: std::sync::atomic::AtomicBool,
    gate: Arc<tokio::sync::Notify>,
    gate_open: std::sync::atomic::AtomicBool,
}

impl GatedModel {
    fn new(script: Vec<ScriptStep>) -> Arc<Self> {
        Arc::new(Self {
            inner: RecordingModel::new(script),
            armed: std::sync::atomic::AtomicBool::new(true),
            reached: Arc::new(tokio::sync::Notify::new()),
            gate_reached: std::sync::atomic::AtomicBool::new(false),
            gate: Arc::new(tokio::sync::Notify::new()),
            gate_open: std::sync::atomic::AtomicBool::new(false),
        })
    }
    fn release(&self) {
        self.gate_open.store(true, Ordering::SeqCst);
        self.gate.notify_waiters();
    }

    /// 闸门到过没有。给用例当判据用 —— 它不经过 `wait_gate_reached` 那条被测的路。
    fn gate_reached(&self) -> bool {
        self.gate_reached.load(Ordering::SeqCst)
    }

    /// 等 worker 走到第一次 `chat`（也就是闸门跟前）。
    ///
    /// **这里有个真会发生的竞态，而且它咬过人。** `chat()` 里那句 `notify_waiters()`
    /// 只叫得醒**当下已经挂在等待队列里**的人；worker 要是比本函数先一步到闸门，
    /// 那一发通知就发给了空气，而 `armed` 是一次性的，不会再有第二发。
    ///
    /// 两个窗口要分开看，兜法不一样：
    /// - **通知发生在 `notified()` 之后**：tokio 自己兜住了 —— `Notify::notified()` 建
    ///   future 时会记下当时的 `notify_waiters` 调用次数，第一次 poll 发现次数变了就直接
    ///   Ready。这一半从来不是问题。
    /// - **通知发生在 `notified()` 之前**：tokio 兜不住，只能靠一个「到过没有」的标志位。
    ///   原来这行写的是 `if self.inner.call_count() > 0` —— **它恒为 false**：闸门期间
    ///   `inner.chat` 压根还没被调到，计数当然是 0。于是这道守卫从没生效过，
    ///   它本该挡住的那个竞态一次都没被挡住。2026-09-12 建 W2 worktree 跑基线时撞到
    ///   `cargo passed=792 failed=1`，失败名逐字是 `-p aite --test reconnect_replay`，
    ///   而单独连跑五遍 10/10 全绿 —— 就是这里。本轨把这个窗口人为撑到 200ms 之后，
    ///   它是**必现**的（复现过，panic 逐字落在下面那句 `expect` 上）。
    ///
    /// 所以标志位换成 `gate_reached`：`chat()` 在 `notify_waiters()` **之前**把它置真。
    /// 「先置位、再通知」配上「先建 notified、再查位」，两个方向都不漏。
    async fn wait_gate_reached(&self) {
        let notified = self.reached.notified();
        if self.gate_reached() {
            return;
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), notified)
            .await
            .expect("5s 内 worker 没走到第一次 chat");
    }
}

#[async_trait::async_trait]
impl aite_contracts::ModelPort for GatedModel {
    fn name(&self) -> String {
        self.inner.name()
    }
    async fn chat(
        &self,
        messages: &[aite_contracts::Message],
        tools: &[aite_contracts::ToolSpec],
        max_tokens: u32,
        temperature: f32,
    ) -> Result<aite_contracts::ModelTurn, aite_contracts::ModelError> {
        if self.armed.swap(false, Ordering::SeqCst) {
            // 先置位再通知：等的人要是还没挂上队列，通知会丢，标志位不会。
            self.gate_reached.store(true, Ordering::SeqCst);
            self.reached.notify_waiters();
            while !self.gate_open.load(Ordering::SeqCst) {
                let waiter = self.gate.notified();
                if self.gate_open.load(Ordering::SeqCst) {
                    break;
                }
                // 有超时兜底：闸门永远不开也只会让这条用例红，不会挂死整套测试
                let _ = tokio::time::timeout(std::time::Duration::from_millis(50), waiter).await;
            }
        }
        self.inner
            .chat(messages, tools, max_tokens, temperature)
            .await
    }
}

// --------------------------------------------------------------------------
// 驱动
// --------------------------------------------------------------------------

struct Rig {
    config: aite_contracts::AiteConfig,
    app: Arc<aite_app::AiteApp>,
    platform: Arc<GatedPlatform>,
    replay: Arc<Replay>,
    run: RunningApp,
}

async fn make_rig(
    config: aite_contracts::AiteConfig,
    n_tasks: usize,
) -> (Rig, Arc<RecordingModel>) {
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(script_for(n_tasks));
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        ClosableFakeSandbox::new(),
    )
    .await;
    let run = RunningApp::start(app.clone(), &platform, None).await;
    let replay = Replay::new(platform.clone());
    (
        Rig {
            config,
            app,
            platform,
            replay,
            run,
        },
        model,
    )
}

async fn make_gated_rig(
    config: aite_contracts::AiteConfig,
    n_tasks: usize,
) -> (Rig, Arc<GatedModel>) {
    let platform = GatedPlatform::new();
    let model = GatedModel::new(script_for(n_tasks));
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        ClosableFakeSandbox::new(),
    )
    .await;
    let run = RunningApp::start(app.clone(), &platform, None).await;
    let replay = Replay::new(platform.clone());
    (
        Rig {
            config,
            app,
            platform,
            replay,
            run,
        },
        model,
    )
}

impl Rig {
    /// 等到这一批全部收完 —— 交付数到位，且 worker 手上什么都不剩。
    ///
    /// 只等 `send_text` 是不够的（那时收尾还没走完），只等 `in_flight` 空也不够
    /// （队列里可能还压着没派发的）。三件一起等。
    async fn settled(&self, deliveries: usize) {
        let p = self.platform.clone();
        wait_until(
            || p.inner.count("send_text") == deliveries,
            &format!("{deliveries} 次交付回帖"),
        )
        .await;
        self.app.plane.join().await;
        let a = self.app.clone();
        wait_until(|| a.worker.in_flight().is_empty(), "worker.run() 全部返回").await;
    }

    /// 这一趟建出来的全部任务，按任务号排。
    ///
    /// 契约的 `SessionStore` 没有「列出全部任务」的口子；task_id 从 evidence 目录拿：
    /// 建完任务第一件事就是写 `task_created`，所以「盘上有几个任务目录」就是
    /// 「一共建了几个任务」。这条本身也是断言的一部分。
    async fn all_tasks(&self) -> Vec<Task> {
        let mut tasks = Vec::new();
        for task_id in evidence_task_dirs(&self.config) {
            if let Ok(Some(t)) = self.app.store.get_task(&task_id).await {
                tasks.push(t);
            }
        }
        tasks.sort_by(|a, b| a.task_no.cmp(&b.task_no));
        tasks
    }

    /// 每条交付回帖回到了哪条消息上。任务与话题 root 一一对应，所以这个集合就是
    /// 「哪些事件真的变成了任务并交付」—— 而且不吃派发顺序。
    fn reply_targets(&self) -> BTreeSet<String> {
        self.platform
            .inner
            .calls
            .of("send_text")
            .iter()
            .filter_map(|c| c.arg_str("reply_to").map(str::to_string))
            .collect()
    }

    fn counter(&self, key: &str) -> u64 {
        self.app
            .plane
            .counters()
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    }
}

fn mention(event_id: &str, message_id: &str) -> NormalizedEvent {
    Ev::new(event_id, &format!("{event_id} 的活"))
        .message_id(message_id)
        .build()
}

fn mention_text(event_id: &str, message_id: &str, text: &str) -> NormalizedEvent {
    Ev::new(event_id, text).message_id(message_id).build()
}

fn set_of<const N: usize>(items: [&str; N]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// --------------------------------------------------------------------------
// 1 重推的一批：重复只算一次，不同的一个不丢，断网前处理过的也认得
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replayed_batch_dedups_without_dropping_anything_new() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let (mut rig, _model) = make_rig(make_config(tmp.path()), 4).await;

    // ── 断网之前：正常办完一个活 ──────────────────────────────
    rig.replay
        .emit(&mention_text(BEFORE_EVENT, BEFORE_MSG, "断网前的活"))
        .await;
    rig.settled(1).await;

    // ── 拔网线：这三条一条也到不了本进程 ──────────────────────
    rig.replay.unplug();
    for (event_id, message_id) in DURING {
        rig.replay.emit(&mention(event_id, message_id)).await;
    }
    assert_eq!(rig.replay.backlog_len(), 3);
    settle().await;
    assert_eq!(
        rig.platform.inner.count("send_text"),
        1,
        "断网期间不该有任何新交付"
    );

    // ── 插回网线：整批一次性重推，夹着重复和断网前那条 ────────
    let batch = rig
        .replay
        .replug(
            vec![
                mention("ev-a", "om_a"),                              // 重复 #1
                mention("ev-a", "om_a"),                              // 重复 #2
                mention("ev-b", "om_b"),                              // 重复 #1
                mention_text(BEFORE_EVENT, BEFORE_MSG, "断网前的活"), // 断网前就办完的
            ],
            true,
        )
        .await;
    assert_eq!(
        batch.len(),
        7,
        "这一批该是 3 条新的 + 3 条重复 + 1 条断网前的"
    );

    rig.settled(4).await;
    settle().await; // 再给系统一把机会去做那件不该做的事（多建一个 task）

    // ── 逐条判据 ──────────────────────────────────────────────
    assert_eq!(
        rig.reply_targets(),
        set_of([BEFORE_MSG, "om_a", "om_b", "om_c"]),
        "每个话题各该有且只有一次交付"
    );
    assert_eq!(rig.platform.inner.count("send_text"), 4);
    assert_eq!(
        rig.platform.inner.count("send_card"),
        4,
        "一个任务一张卡，4 个任务就是 4 张"
    );
    assert_eq!(rig.platform.inner.card_count(), 4);

    let tasks = rig.all_tasks().await;
    let sessions: BTreeSet<String> = tasks.iter().map(|t| t.session_id.clone()).collect();
    assert_eq!(tasks.len(), 4, "4 个 event_id 就该有 4 个任务");
    assert_eq!(sessions.len(), 4, "四条 @ 各占一个话题，会话也该是 4 个");
    assert!(
        tasks.iter().all(|t| t.status == TaskStatus::Delivered),
        "这一批该全部交付，实际 {:?}",
        tasks
            .iter()
            .map(|t| (t.task_no.clone(), t.status.as_str()))
            .collect::<Vec<_>>()
    );

    // 计数器对得上：4 条重复（ev-a×2、ev-b×1、断网前那条×1）
    assert_eq!(
        rig.counter("events.duplicate"),
        4,
        "重复事件该被 R2 数走 4 条"
    );
    assert_eq!(rig.app.ingress.counter("events.handled"), 8); // 1 + 7
    assert_eq!(rig.app.ingress.counter("ingress.errors"), 0);

    // ack 也只加一次 —— 重复事件在 R2 就被拦下，走不到 R7 的 add_reaction
    let acked: BTreeSet<String> = rig
        .platform
        .inner
        .reactions()
        .into_iter()
        .map(|(m, _)| m)
        .collect();
    assert_eq!(
        acked,
        set_of([BEFORE_MSG, "om_a", "om_b", "om_c"]),
        "@ 一条只该 ack 一次"
    );
    assert_eq!(rig.platform.inner.reactions().len(), 4);

    // 证据目录也是一个任务一个，重复事件不该凭空多出一份链
    let mut ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
    ids.sort();
    assert_eq!(evidence_task_dirs(&rig.config), ids);
    rig.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 2 一批**同时**到达：去重仍然只放行一次
// --------------------------------------------------------------------------

/// 六份一模一样的重推**同时**进 `on_event`。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_event_arriving_all_at_once_still_creates_one_task() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let (mut rig, model) = make_rig(make_config(tmp.path()), 1).await;

    rig.replay.unplug();
    rig.replay.emit(&mention("ev-storm", "om_storm")).await;
    rig.replay
        .replug(
            (0..5).map(|_| mention("ev-storm", "om_storm")).collect(),
            true,
        )
        .await;

    rig.settled(1).await;
    settle().await;

    let tasks = rig.all_tasks().await;
    assert_eq!(tasks.len(), 1, "六份重推只该建一个任务");
    assert_eq!(rig.platform.inner.count("send_text"), 1);
    assert_eq!(rig.platform.inner.count("send_card"), 1);
    assert_eq!(
        rig.platform.inner.reactions(),
        vec![("om_storm".to_string(), aite_contracts::ReactionKind::Ack)]
    );
    assert_eq!(rig.counter("events.duplicate"), 5);
    // 模型只该被调两次（checklist_add + final）。多一次就是多跑了一个任务。
    assert_eq!(
        model.call_count(),
        2,
        "模型被调了 {} 次",
        model.call_count()
    );
    assert!(
        rig.replay.max_concurrent() >= 2,
        "这一批压根没同时压在 on_event 里（峰值 {}），那这条用例验的就不是并发了",
        rig.replay.max_concurrent()
    );
    rig.run.shutdown().await.expect("run_app 正常收场");
}

/// 去重不能误伤：五个不同的 `event_id` 同时到达，五个任务一个不少。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_batch_of_distinct_events_loses_none() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let (mut rig, _model) = make_rig(make_config(tmp.path()), 5).await;

    rig.replay.unplug();
    for i in 0..5 {
        rig.replay
            .emit(&mention(&format!("ev-{i}"), &format!("om_{i}")))
            .await;
    }
    rig.replay.replug(Vec::new(), true).await;

    rig.settled(5).await;
    settle().await;

    let tasks = rig.all_tasks().await;
    assert_eq!(tasks.len(), 5, "五个不同的事件该有五个任务");
    assert_eq!(
        rig.reply_targets(),
        set_of(["om_0", "om_1", "om_2", "om_3", "om_4"])
    );
    assert_eq!(rig.counter("events.duplicate"), 0, "一条都不该被当成重复");
    let nos: BTreeSet<String> = tasks.iter().map(|t| t.task_no.clone()).collect();
    assert_eq!(
        nos,
        set_of(["#A1", "#A2", "#A3", "#A4", "#A5"]),
        "任务号该是连着的五个 —— 并发下 next_task_no 发重号的话这里先红"
    );
    assert!(
        rig.replay.max_concurrent() >= 2,
        "这一批压根没同时压在 on_event 里（峰值 {}）",
        rig.replay.max_concurrent()
    );
    rig.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 3 断网之前处理过的，跨断线仍然认得
// --------------------------------------------------------------------------

/// `seen_event` 是落库的，所以断线本身不该让去重失忆。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_handled_before_the_drop_are_still_known_after_it() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let (mut rig, _model) = make_rig(make_config(tmp.path()), 2).await;

    rig.replay
        .emit(&mention_text(BEFORE_EVENT, BEFORE_MSG, "断网前的活"))
        .await;
    rig.settled(1).await;
    let first = rig.all_tasks().await[0].clone();

    rig.replay.unplug();
    settle().await;
    // 重连：平台只重推了断网前那条（边界上的至少一次投递）
    rig.replay
        .replug(
            vec![mention_text(BEFORE_EVENT, BEFORE_MSG, "断网前的活")],
            true,
        )
        .await;
    settle().await;

    let after = rig.all_tasks().await;
    assert_eq!(after.len(), 1, "断网前办过的活不该被重推出第二遍");
    assert_eq!(after[0].id, first.id);
    assert_eq!(rig.platform.inner.count("send_text"), 1);
    assert_eq!(rig.counter("events.duplicate"), 1);

    // 系统没被这条重推带偏：紧接着来的新事件照样接得住
    rig.replay.emit(&mention("ev-after", "om_after")).await;
    rig.settled(2).await;
    assert_eq!(rig.all_tasks().await.len(), 2);
    rig.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 4 断网期间在跑的任务，重连之后照常收尾
// --------------------------------------------------------------------------

/// M2 说的是「服务自动重连」，不是「重启」—— 进程一直活着，在跑的任务不该受影响。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_running_across_the_reconnect_finishes_normally() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let (mut rig, model) = make_gated_rig(make_config(tmp.path()), 3).await;

    rig.replay
        .emit(&mention_text("ev-long", "om_long", "一个跑得久的活"))
        .await;
    model.wait_gate_reached().await;

    let a = rig.app.clone();
    wait_until(|| !a.worker.in_flight().is_empty(), "任务被 worker 领走").await;
    let running = rig.all_tasks().await;
    assert_eq!(running.len(), 1);
    let task_a = running[0].clone();

    // ── 网断了，群里又发了两条 ────────────────────────────────
    rig.replay.unplug();
    rig.replay.emit(&mention("ev-x", "om_x")).await;
    rig.replay.emit(&mention("ev-y", "om_y")).await;
    settle().await;
    let in_flight: Vec<String> = rig
        .app
        .worker
        .in_flight()
        .into_iter()
        .map(|(t, _)| t.id)
        .collect();
    assert!(
        in_flight.contains(&task_a.id),
        "断线不该把在跑的任务弄没了：{in_flight:?}"
    );

    // ── 重连，一批推上来（A 还挂着，它们只能先排队）──────────
    rig.replay
        .replug(vec![mention("ev-x", "om_x")], true) // 夹一条重复
        .await;
    settle().await;
    let in_flight: Vec<String> = rig
        .app
        .worker
        .in_flight()
        .into_iter()
        .map(|(t, _)| t.id)
        .collect();
    assert!(
        in_flight.contains(&task_a.id),
        "重连推进来的一批不该把正在跑的任务顶掉"
    );
    assert_eq!(
        rig.app.plane.pending(),
        2,
        "两个新任务该排在队列里等 A 跑完，实际队列里 {} 个",
        rig.app.plane.pending()
    );

    // ── 放行 A ────────────────────────────────────────────────
    model.release();
    rig.settled(3).await;
    settle().await;

    let tasks = rig.all_tasks().await;
    assert_eq!(tasks.len(), 3, "A + 两条新的 = 3 个任务");
    assert!(tasks.iter().all(|t| t.status == TaskStatus::Delivered));
    assert_eq!(rig.reply_targets(), set_of(["om_long", "om_x", "om_y"]));
    assert_eq!(rig.counter("events.duplicate"), 1);

    // A 的证据链自己收了口：断线没有在它中间插进任何东西
    let kinds: Vec<EvidenceKind> = read_events(&rig.config, &task_a.id)
        .iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds[0], EvidenceKind::TaskCreated);
    assert_eq!(kinds[1], EvidenceKind::EventReceived);
    assert_eq!(
        *kinds.last().expect("最后一条"),
        EvidenceKind::Delivered,
        "跨断线的任务该正常交付收口"
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == EvidenceKind::EventReceived)
            .count(),
        1,
        "重连推进来的那一批不该往 A 的证据链上写东西 —— 它们是别的话题"
    );

    let from_disk = rig
        .app
        .store
        .get_task(&task_a.id)
        .await
        .expect("get_task")
        .expect("任务在库里");
    assert_eq!(from_disk.status, TaskStatus::Delivered);
    rig.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 5 同一话题的 root 与追问一起被重推（两种到达形状）
// --------------------------------------------------------------------------

/// **两种形状的结论并不一样，原来这一组假定它一样 —— 那是这个 target 的第二个抖动源。**
///
/// 顺序到达（`together=false`）有先后：root 那条 `handle_event` 整个走完、会话落了库，
/// 追问才进来，所以 R6 必然命中，一条都不许丢。
///
/// 一起到达（`together=true`）**没有先后可言**：两条各自 `tokio::spawn`，追问完全可能在
/// root 的会话落库之前就进 `handle_event` —— 它自己没 @、话题又还不存在，于是命中
/// R8「其余丢弃」。这正是紧挨着的 `a_followup_replayed_before_its_root_is_dropped`
/// 逐字写下的那条**当前边界**（要补得改路由规则本身）。原来这一支照抄顺序那一支的断言
/// （`events.ignored == 0`），于是谁先谁后全看调度 —— 本轨实测基线上 12 遍红 1 遍，
/// 与本轨改动无关。
///
/// 所以并发那一支改成钉**真正成立的那条保证**：结局只许是两种之一，
/// 「既没进 transcript、也没被记成丢弃」这种静悄悄没了的形状一个都不许有。
async fn root_and_followup_replayed(together: bool) {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let (mut rig, _model) = make_rig(make_config(tmp.path()), 2).await;

    rig.replay.unplug();
    rig.replay
        .emit(&mention_text("ev-root", "om_root", "按月画个图"))
        .await;
    rig.replay
        .emit(
            &Ev::new("ev-follow", "再按季度画一张")
                .message_id("om_follow")
                .thread_id("om_root")
                .mentioned(false)
                .build(),
        )
        .await;
    rig.replay.replug(Vec::new(), together).await;
    settle().await;
    rig.app.plane.join().await;
    let a = rig.app.clone();
    wait_until(|| a.worker.in_flight().is_empty(), "worker.run() 全部返回").await;
    settle().await;

    let tasks = rig.all_tasks().await;
    let sessions: BTreeSet<String> = tasks.iter().map(|t| t.session_id.clone()).collect();
    let turns = match sessions.iter().next() {
        Some(sid) => rig
            .app
            .store
            .list_turns(sid, 100)
            .await
            .expect("list_turns"),
        None => Vec::new(),
    };

    let contents: Vec<String> = turns.iter().map(|t| t.content.clone()).collect();
    let seqs: Vec<u64> = turns.iter().map(|t| t.seq).collect();
    let ignored = rig.counter("events.ignored");

    // ── 两种形状都成立的那几条 ────────────────────────────────
    assert!(!tasks.is_empty(), "root 那条至少要变成一个任务");
    assert_eq!(
        sessions.len(),
        1,
        "两条在同一个话题里，只该有一个会话 —— 追问那条要么并进来、要么被丢，\
         就是不许自己另起一个话题"
    );
    assert_eq!(
        rig.app.ingress.counter("ingress.errors"),
        0,
        "有事件在路由里炸了 —— 它既没变成任务也不会被重推第二次，等于丢了"
    );

    let joined = contents == ["按月画个图", "再按季度画一张"] && ignored == 0;
    if together {
        // 并发到达没有先后，两种结局都合法；不合法的是「第三种」。
        let dropped_as_r8 = contents == ["按月画个图"] && ignored == 1;
        assert!(
            joined || dropped_as_r8,
            "并发重推只许落在两种结局上：追问并进同一份 transcript（ignored=0），\
             或者它抢在 root 前面、按 R8 被丢掉并记一笔（ignored=1）。\
             实际 transcript={contents:?}、events.ignored={ignored} —— \
             这是第三种：有东西静悄悄没了。计数器：{:?}",
            rig.app.plane.counters()
        );
    } else {
        // 顺序到达有先后：root 的会话必然已经落库，追问一条都不许丢。
        assert!(
            joined,
            "顺序重推时 root 先整个走完，追问必然命中 R6：\
             transcript={contents:?}、events.ignored={ignored}。计数器：{:?}",
            rig.app.plane.counters()
        );
    }
    if joined {
        assert_eq!(
            seqs,
            vec![0, 1],
            "两句话在同一份 transcript 里要按到达顺序排"
        );
    }
    rig.run.shutdown().await.expect("run_app 正常收场");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_root_and_its_thread_followup_replayed_in_order() {
    root_and_followup_replayed(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_root_and_its_thread_followup_replayed_together() {
    root_and_followup_replayed(true).await;
}

/// **当前边界，不是缺陷判定**：追问被重推在它的 root 前面时，它会掉进 R8。
///
/// R6 的前提是「库里找得到这个话题的会话」，而会话是 root 那条建的。平台按乱序把
/// 追问先推上来时，追问到达的那一刻话题还不存在、它自己又没 @，于是命中 R8「其余丢弃」。
/// 要补得靠**事件级的重排或缓冲**，那是路由规则本身要改 —— 超出本轨可写面。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_followup_replayed_before_its_root_is_dropped() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let (mut rig, _model) = make_rig(make_config(tmp.path()), 2).await;

    rig.replay.unplug();
    // 平台把追问推在了前面
    rig.replay
        .emit(
            &Ev::new("ev-follow", "再按季度画一张")
                .message_id("om_follow")
                .thread_id("om_root")
                .mentioned(false)
                .build(),
        )
        .await;
    rig.replay
        .emit(&mention_text("ev-root", "om_root", "按月画个图"))
        .await;
    // 顺序推：这条钉的是「乱序」，不是「并发」
    rig.replay.replug(Vec::new(), false).await;

    rig.settled(1).await;
    settle().await;

    let tasks = rig.all_tasks().await;
    assert_eq!(tasks.len(), 1, "只有 root 那条变成了任务");
    let turns = rig
        .app
        .store
        .list_turns(&tasks[0].session_id, 100)
        .await
        .expect("list_turns");
    assert_eq!(
        turns.iter().map(|t| t.content.clone()).collect::<Vec<_>>(),
        vec!["按月画个图"],
        "追问没进 transcript —— 这正是这条要钉的边界"
    );
    assert_eq!(rig.counter("events.ignored"), 1, "追问该是掉在 R8 上");
    // 没有异常、没有回帖：丢得**安静**，这也是它难被发现的原因
    assert_eq!(rig.app.ingress.counter("ingress.errors"), 0);
    assert_eq!(rig.platform.inner.count("send_text"), 1);
    rig.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 6 对照组：顺序到达与同时到达，结论必须一致
// --------------------------------------------------------------------------

async fn dedup_holds_for(together: bool) {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let (mut rig, _model) = make_rig(make_config(tmp.path()), 2).await;

    rig.replay.unplug();
    rig.replay.emit(&mention("ev-p", "om_p")).await;
    rig.replay.emit(&mention("ev-q", "om_q")).await;
    rig.replay
        .replug(
            vec![mention("ev-p", "om_p"), mention("ev-q", "om_q")],
            together,
        )
        .await;

    rig.settled(2).await;
    settle().await;

    assert_eq!(rig.all_tasks().await.len(), 2);
    assert_eq!(rig.reply_targets(), set_of(["om_p", "om_q"]));
    assert_eq!(rig.platform.inner.count("send_card"), 2);
    assert_eq!(rig.counter("events.duplicate"), 2);
    rig.run.shutdown().await.expect("run_app 正常收场");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dedup_holds_for_sequential_arrival() {
    dedup_holds_for(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dedup_holds_for_concurrent_arrival() {
    dedup_holds_for(true).await;
}

// --------------------------------------------------------------------------
// 6 脚手架自己的那道守卫 —— 它曾经恒为 false
// --------------------------------------------------------------------------

/// 闸门**已经到过**了，再问一次「到了没」必须立刻返回，而不是干等到 5s 超时然后报
/// 「worker 没走到第一次 chat」。
///
/// 这一条钉的不是产品代码，是上面 `a_task_running_across_the_reconnect_finishes_normally`
/// 赖以成立的脚手架。原来 `wait_gate_reached` 用 `inner.call_count() > 0` 当「到过了」
/// 的判据，而闸门期间 `inner.chat` 还没被调到 —— 判据恒为 0，守卫从没生效过，
/// 那条用例就跟着偶发假红（台账记的 `792/1` 那次）。
///
/// 用例刻意造出「通知早于等待」这个次序：先把模型驱到闸门上，**之后**才去问。
/// 把 `wait_gate_reached` 里的 `self.gate_reached()` 换回 `self.inner.call_count() > 0`，
/// 这条立刻红（5s 超时）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_gate_reached_survives_a_notification_that_came_first() {
    use aite_contracts::ModelPort;

    let model = GatedModel::new(script_for(1));

    // 直接驱一次 chat，把它停在闸门上（不经 worker，省掉整套建场）
    let driving = model.clone();
    let driver = tokio::spawn(async move { driving.chat(&[], &[], 128, 0.0).await });

    // 等到闸门确实被走到。判据用新标志位，不用被测的那条路，免得自己证自己。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !model.gate_reached() {
        assert!(
            std::time::Instant::now() < deadline,
            "5s 内模型没走到闸门 —— 这条用例的前提就没成立"
        );
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }

    // 前提：旧守卫那个判据在这一刻仍然是 0 —— 它恒为 false 的根就在这儿
    assert_eq!(
        model.inner.call_count(),
        0,
        "闸门期间 inner.chat 不该被调到；它要是已经涨了，旧守卫就不是死代码，本条前提作废"
    );

    // 正题：通知早就发过了（而且发的那一刻一个等待者都没有），这一问必须立刻返回
    tokio::time::timeout(std::time::Duration::from_secs(1), model.wait_gate_reached())
        .await
        .expect("闸门已经到过了，wait_gate_reached 必须立刻返回，不能干等到 5s 超时");

    model.release();
    driver.await.expect("driver").expect("chat");
}
