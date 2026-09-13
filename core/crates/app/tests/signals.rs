//! 停机信号本身（`StopSignal`）—— 单元层 + 进程层两道。
//!
//! 这一组钉的是一条**真机会挂死进程**的缺陷：`StopSignal::set()` 原来写的是
//! `let _ = self.tx.send(true)`，而 `tokio::sync::watch::Sender::send` 在一个活跃接收者
//! 都没有时返回 `Err` 且**连内部那个值都不改**。`StopSignal::new()` 当场就把建出来的
//! `_rx` 丢了，唯一订阅它的地方是 `serve()`（`wait()` 里的 `subscribe()`）—— 于是在
//! `run_app` 走到 `serve()` 之前，`set()` 是一次**彻底的 no-op**：信号被吃掉，不留痕迹。
//!
//! `aite run` 把 `SIGINT` / `SIGTERM` 都接到同一个 `StopSignal`（`install_signal_handlers`）。
//! 信号赶在 `serve()` 之前到达 —— compose 的 `stop_grace_period`、k8s 滚动更新、人手快按
//! Ctrl-C 都会 —— 进程就**永远不退，只能 `SIGKILL`**；而 `docker-compose.yml` 配的是
//! `restart: unless-stopped`，杀完还会被拉起来。
//!
//! 为什么要两层：
//!
//! * **单元层**钉住 `set()` / `is_set()` / `wait()` 三者的口径，两种时序都过。
//! * **进程层**钉住那个**后果** —— 真二进制收到 `SIGTERM` 之后自己退得掉。这条病能活这么久，
//!   正因为没有任何测试站在进程这一层：单元层的断言再全，也证明不了 `aite` 这个进程
//!   收到信号会退。
mod common;

use std::time::Duration;

use aite_app::StopSignal;

// --------------------------------------------------------------------------
// 单元层：set / is_set / wait 的口径
// --------------------------------------------------------------------------

/// **病根本身**：一个订阅者都还没出现时 `set()` 也必须算数。
///
/// 这是 X1 那个五行探针的断言形式。把 `set()` 改回 `self.tx.send(true)` 这一条立刻红。
#[test]
fn set_counts_even_with_no_subscriber_yet() {
    let stop = StopSignal::new();
    assert!(!stop.is_set(), "刚建出来不该是置起来的");
    stop.set();
    assert!(
        stop.is_set(),
        "没有任何接收者时 set() 也必须留下痕迹 —— 否则 serve() 此后永远等不到"
    );
}

/// 置起来之后**才**开始等：`wait()` 必须立刻返回，不能挂在 `changed()` 上。
///
/// 这是「信号早于 `serve()`」那条真机路径的最小形状。死线给 2s 是因为正确实现走的是
/// `borrow_and_update()` 那个早退分支，实测微秒级；撞穿它只可能是挂死。
#[tokio::test]
async fn wait_returns_at_once_when_set_happened_first() {
    let stop = StopSignal::new();
    stop.set();
    tokio::time::timeout(Duration::from_secs(2), stop.wait())
        .await
        .expect("set() 先发生时 wait() 必须立刻返回");
    assert!(
        stop.is_set(),
        "wait() 返回之后值还得在（watch 不是一次性的）"
    );
}

/// 反过来的时序：先挂在 `wait()` 上，`set()` 后到 —— 也必须被叫醒。
///
/// 上药不能把这条弄坏（`send_replace` 与 `send` 一样会通知所有接收者）。
/// 「等的那条已经进去了」由 `ready` 这个 oneshot 报告，不靠睡时间；报告与 `subscribe()`
/// 之间那一点缝隙不影响断言 —— 两种落点下 `wait()` 都必须返回。
#[tokio::test]
async fn wait_wakes_up_when_set_happens_later() {
    let stop = StopSignal::new();
    let mine = stop.clone();
    let (ready, is_ready) = tokio::sync::oneshot::channel::<()>();
    let waiter = tokio::spawn(async move {
        let _ = ready.send(());
        mine.wait().await;
    });

    is_ready.await.expect("等的那条起来了");
    stop.set();

    tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("set() 后到时 wait() 必须被叫醒")
        .expect("等的那条别 panic");
}

/// `StopSignal` 是 `Clone` 的，各份共用同一个开关 —— `install_signal_handlers` 拿走的
/// 就是一份 clone（`run_app` 里的 `stop.clone()`），它置起来，`serve()` 手上那份要看得见。
#[test]
fn clones_share_one_switch() {
    let stop = StopSignal::new();
    let handed_to_signals = stop.clone();
    handed_to_signals.set();
    assert!(stop.is_set(), "信号那份置起来，serve() 那份必须看得见");
}

// --------------------------------------------------------------------------
// 进程层：真二进制收到 SIGTERM 之后自己退得掉
// --------------------------------------------------------------------------

/// 起飞路径上那几个标记，按 `takeoff()` 里真实的先后排：
///
/// ```text
/// aite.signal_ready   install_signal_handlers 两个 signal() 都回了 Ok —— 窗口从这里开
/// aite.up             store.init() 过了（log_takeoff）
/// aite.stopping       serve() 返回了，进收尾（shutdown 的第一行）
/// aite.down           收尾走完
/// ```
const READY: &str = "aite.signal_ready";
const SIGNAL_SEEN: &str = "aite.signal 收到";
const UP: &str = "aite.up";
const STOPPING: &str = "aite.stopping";
const DOWN: &str = "aite.down";

/// 轮询到 `mark` 出现为止，超过 `budget` 就带着现场 panic。
///
/// 轮的是日志文件（`wait_until_within` 那个口径），不是睡一个固定时长赌时序 ——
/// 这条测试全程没有任何一句「睡 N 秒然后假定对面已经走到哪儿了」。
fn wait_for_mark(log: &std::path::Path, mark: &str, budget: Duration, what: &str) -> String {
    let deadline = std::time::Instant::now() + budget;
    loop {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if text.contains(mark) {
            return text;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "{:.1}s 内没等到 `{mark}`（{what}）。日志现场：\n{text}",
                budget.as_secs_f64()
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// **本轨的交付重点**：`SIGTERM` 赶在 `serve()` 之前到达，进程必须自己退干净。
///
/// 单元层那四条钉的是 `StopSignal` 的口径；这一条钉的是**后果** —— 真机上
/// `docker compose stop` / k8s 滚动更新发来的 `SIGTERM` 落在起飞半路时，`aite` 这个进程
/// 走不走得掉。这条病能活这么久，正因为从来没有测试站在进程这一层。
///
/// # 怎么把「赶在 `serve()` 之前」做成确定的
///
/// 窗口是 `install_signal_handlers` → `serve()` 这一段，真机上只有 ~25ms（实测），
/// 直接发信号是在赌。所以拿一把**外部 SQLite 写锁**把它撑开：
///
/// ```text
/// 父：BEGIN EXCLUSIVE 持住 sqlite_path          ← 窗口撑开
/// 子：build_app -> install_signal_handlers -> 打 aite.signal_ready
/// 子：store.init() 建表撞 SQLITE_BUSY，卡在 busy_timeout 的重试里（aite.up 打不出来）
/// 父：等到 aite.signal_ready -> SIGTERM -> 等到 aite.signal（handler 已经跑完 set()）
/// 父：ROLLBACK 放锁                              ← 窗口关上
/// 子：init 成功 -> aite.up -> ... -> serve()
/// ```
///
/// `aite.signal` 出现在 `aite.up` **之前**是这条测试的自检：它证明 `set()` 确实发生在
/// `serve()` 订阅之前，而不是信号来晚了、测了个寂寞。断言里两处都核。
///
/// 病还在时这条是这样红的（实测，本轨原样贴进回执）：日志停在 `aite.up`，
/// `aite.stopping` 一个字都没有，20s 死线撞穿 —— 也就是真机上那个「只能 `SIGKILL`」。
///
/// # 几个说明
///
/// * 用真 `aite` 二进制（`CARGO_BIN_EXE_aite`），不是同进程调 `run_app`：
///   `install_signals: true` 这条路只有真进程才走得到。
/// * `platform: feishu` 而不是 `fake` —— `build_app` 明确拒绝「fake 又不注入」，
///   而真二进制是 `Injections::default()`。edge 不起也没关系：`platform.start()` 里
///   只有 `ingress.start()`（本地监听）是硬要求，能力表问不到只 warn 一行。
/// * 起步要 ~6s，绝大部分花在 edge 不可达的那 5 次 `GetStatus` 重试上（每次隔 1s）。
///   这是 `build_app` 的既有行为，不是这条测试自己在等。
/// * 发信号借 `/bin/kill`，省得为一个 `libc::kill` 往依赖表里加东西。
#[test]
fn sigterm_before_serve_still_exits_by_itself() {
    let dir = tempfile::tempdir().expect("tmpdir");
    // socket 路径要短于 SUN_LEN（104）：TMPDIR 下的 tempdir 实测 ~65 字符，够用；
    // 换到长路径下这里会先炸在 `EdgeClient::connect` 上，不会静悄悄地变成别的毛病。
    std::fs::create_dir_all(dir.path().join("run")).expect("建 run 目录");
    let db = dir.path().join("aite.db");
    let cfg_path = dir.path().join("aite.yaml");
    let log_path = dir.path().join("out.log");

    let example = std::fs::read_to_string(common::repo_root().join("config/aite.example.yaml"))
        .expect("读 config/aite.example.yaml");
    let cfg = example
        .replace(
            "sqlite_path: data/aite.db",
            &format!("sqlite_path: {}", db.display()),
        )
        .replace(
            "evidence_dir: data/evidence",
            &format!("evidence_dir: {}/evidence", dir.path().display()),
        )
        .replace(
            "artifacts_dir: data/artifacts",
            &format!("artifacts_dir: {}/artifacts", dir.path().display()),
        )
        .replace(
            "system_prompt_path: core/crates/worker/prompts/platform.md",
            &format!("system_prompt_path: {}", common::platform_md().display()),
        )
        // 模型只在真有任务时才会被调到，这里只要过 build_app 的「配置不完整」那关。
        .replace("base_url: \"\"", "base_url: \"http://127.0.0.1:9/v1\"")
        .replace("model: \"\"", "model: \"signals-e2e\"")
        .replace(
            "edge_socket: data/run/aite-edge.sock",
            &format!("edge_socket: {}/run/e.sock", dir.path().display()),
        )
        .replace(
            "core_socket: data/run/aite-core.sock",
            &format!("core_socket: {}/run/c.sock", dir.path().display()),
        );
    std::fs::write(&cfg_path, cfg).expect("写配置");

    // --- 撑开窗口：把库锁住，子进程的 store.init() 建表就只能排队等 ---
    let lock = rusqlite::Connection::open(&db).expect("开库");
    lock.execute_batch("PRAGMA journal_mode=delete; BEGIN EXCLUSIVE;")
        .expect("持 EXCLUSIVE 写锁");

    let out = std::fs::File::create(&log_path).expect("建日志文件");
    let err = out.try_clone().expect("复制句柄");
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_aite"))
        .args(["run", "--config", cfg_path.to_str().expect("配置路径")])
        .current_dir(common::repo_root())
        .env("AITE_MODEL_API_KEY", "signals-e2e")
        .stdout(out)
        .stderr(err)
        .spawn()
        .expect("起 aite run");

    // 收尾兜底：中间任何一条断言炸掉都别把进程留在机器上。
    struct Reaper(std::process::Child);
    impl Drop for Reaper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // 1) 信号面接管了 —— 窗口从这一刻起是开的。
    let text = wait_for_mark(
        &log_path,
        READY,
        Duration::from_secs(60),
        "install_signal_handlers 装好",
    );
    assert!(
        !text.contains(UP),
        "锁没撑住 store.init()：`{UP}` 已经打出来了，窗口是假的，这条测试什么也没测到。\n{text}"
    );

    // 2) 发 SIGTERM，并等到 handler 真的跑了（`aite.signal` 那一行紧挨着 `stop.set()`）。
    let pid = child.id();
    let mut child = Reaper(child);
    let killed = std::process::Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("发 SIGTERM");
    assert!(killed.success(), "/bin/kill -TERM {pid} 没成功");

    let text = wait_for_mark(
        &log_path,
        SIGNAL_SEEN,
        Duration::from_secs(10),
        "信号 handler 收到 SIGTERM",
    );
    assert!(
        !text.contains(UP),
        "信号落在窗口外了（`{UP}` 先出现）：这一遍测的不是「serve() 之前」。\n{text}"
    );

    // 3) 放锁，窗口关上，起飞继续 —— 从这里开始它必须自己走完收尾。
    lock.execute_batch("ROLLBACK;").expect("放锁");
    drop(lock);

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.0.try_wait().expect("查子进程") {
            Some(s) => break s,
            None if std::time::Instant::now() >= deadline => {
                let text = std::fs::read_to_string(&log_path).unwrap_or_default();
                panic!(
                    "20s 内进程没有自己退出 —— SIGTERM 被吃掉了，真机上这时只剩 SIGKILL 一条路\
                     （compose 还配着 restart: unless-stopped，杀完又会被拉起来）。\
                     看日志最后一条 `aite.*`：停在 `{STOPPING}` 之前就是卡在 `serve()`。\n{text}"
                );
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };

    let text = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert_eq!(
        status.code(),
        Some(0),
        "要的是自己走完 C-TΩ-1 退出序列（0）。`None` = 被信号打死的，\
         非 0 多半是起飞失败（比如放锁太慢，建表撞穿了 busy_timeout）。\n{text}"
    );
    assert!(
        text.contains(STOPPING),
        "没进收尾：`{STOPPING}` 没打。\n{text}"
    );
    assert!(text.contains(DOWN), "收尾没走完：`{DOWN}` 没打。\n{text}");

    // 最后再核一次顺序：整条日志上 `aite.signal` 必须排在 `aite.up` 前面。
    let at_signal = text.find(SIGNAL_SEEN).expect("signal 行在");
    let at_up = text.find(UP).expect("up 行在");
    assert!(
        at_signal < at_up,
        "信号没落在 `serve()` 之前（`{SIGNAL_SEEN}` 排在 `{UP}` 后面）。\n{text}"
    );
}
