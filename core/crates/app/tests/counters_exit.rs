//! BB1 ③：core 侧的计数器要有一个查得到的出口 —— 收尾时那一行 `aite.counters`。
//!
//! `acceptance-M.md` §8 第 3 条记的观测缺口里，edge 那一半早就不成立了（退出时打一行
//! `edge.counters`，§7 末）；**缺的只是 core 侧**。改之前 `counters()` 全仓没有任何
//! 非测试调用方，只有 `events.dropped` 一个数经 `!status` 的尾巴漏出来一点点 ——
//! M4 判「没投递 vs 投递了被丢」于是只能去开放平台翻推送记录。
//!
//! **单独一个测试文件 = 单独一个测试二进制**：`set_global_default` 一个进程只装得了一次，
//! 而这条要收的是 INFO。订阅者是手写的最小实现，只用 `tracing` 本身
//! （范本 `evidence/tests/torn_tail_log.rs` / `control/tests/drop_logs.rs`）。
mod common;

use std::sync::{Arc, Mutex};

use common::*;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

// --------------------------------------------------------------------------
// 手写订阅者
// --------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Capture {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 第一条命中 `needle` 的行在序列里的位置 —— 用来钉退出四连的**先后**。
    fn position(&self, needle: &str) -> Option<usize> {
        self.lines().iter().position(|l| l.contains(needle))
    }
}

#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: Vec<String>,
}

impl LineVisitor {
    fn push(&mut self, field: &Field, rendered: String) {
        if field.name() == "message" {
            self.message = rendered;
        } else {
            self.fields.push(format!("{}={rendered}", field.name()));
        }
    }
}

impl Visit for LineVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.push(field, format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, value.to_string());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, value.to_string());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, value.to_string());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, value.to_string());
    }
}

impl Subscriber for Capture {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        *meta.level() <= Level::INFO
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut v = LineVisitor::default();
        event.record(&mut v);
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{} {}", v.message, v.fields.join(" ")));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

// --------------------------------------------------------------------------

/// 收尾那一行 `aite.counters`：**有这一行、数是真的、排在 `aite.down` 前面**。
///
/// 三个断言各钉一件事，缺一条都立不住：
///
/// 1. **有这一行** —— 改之前一行都没有（`counters()` 零调用方），这是底线交付本身；
/// 2. **数是真的** —— 只断「有这一行」的话，打一行写死的空壳也能全绿。所以这里先做两件
///    数得出来的事（一条群里的闲聊掉进 R8、一条 `!status`），再回头验这一行里
///    `events.ignored=1` 和 `commands!status=1` 都在。计数器名是**拼出来的**
///    （`commands!status`，§9 第 2 条），拼出来的 key 一样得出现在这一行里；
/// 3. **排在 `aite.down` 前面** —— 对齐 edge 的退出四连（`edge.signal` /
///    `edge.shutting_down` / `edge.counters` / `edge.down`，§7 末）。印在 `aite.down`
///    之后的话，人按「看到 down 就往上翻」的习惯去找会扑空。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_prints_the_core_counters_before_it_says_down() {
    let capture = Capture::default();
    tracing::subscriber::set_global_default(capture.clone()).expect("装订阅者");

    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(vec![final_step("不会被用到")]);
    let sandbox = ClosableFakeSandbox::new();
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;

    // 做两件数得出来的事。`emit` 是 await 到 `handle_event` 返回才回来的，
    // 所以这里不需要等待也不需要睡 —— 两条都已经数进去了。
    platform
        .emit(&Ev::new("ev-chat", "大家早").mentioned(false).build())
        .await;
    platform
        .emit(
            &Ev::new("ev-status", "!status")
                .message_id("om_status")
                .build(),
        )
        .await;
    assert_eq!(
        app.plane
            .counters()
            .get("events.ignored")
            .and_then(|v| v.as_i64()),
        Some(1),
        "前提：那条闲聊确实掉在 R8 上了"
    );

    run.shutdown().await.expect("run_app 正常收场");

    let lines = capture.lines();
    let counters: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("aite.counters"))
        .collect();
    assert_eq!(
        counters.len(),
        1,
        "收尾该打**一行** aite.counters。全部日志：{lines:?}"
    );
    let line = counters[0];

    for needle in ["events.ignored=1", "commands!status=1", "events.handled=2"] {
        assert!(
            line.contains(needle),
            "这一行得是真数出来的，`{needle}` 不在里面：{line}"
        );
    }
    assert!(
        line.contains("plane=[") && line.contains("ingress=["),
        "plane 与 ingress 是两套各自独立的计数器，分两个字段印、各自用方括号界定 —— \
         合并会在撞名时静默吃掉一个，不界定则 `plane=commands!status=1 events.ignored=1` \
         读起来像三个平级字段：{line}"
    );
    assert!(
        line.contains("ingress=[events.handled=2]"),
        "方括号得把归属框住：ingress 那一格只该有它自己那两三个数：{line}"
    );

    let at_counters = capture.position("aite.counters").expect("刚断言过它在");
    let at_down = capture.position("aite.down").expect("收尾走完那一行");
    assert!(
        at_counters < at_down,
        "aite.counters 要排在 aite.down 前面（对齐 edge 的退出四连）：\
         counters 在第 {at_counters} 行、down 在第 {at_down} 行"
    );
}
