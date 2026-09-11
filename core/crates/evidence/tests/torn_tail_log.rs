//! T21 留痕：静默截掉一条记录跟静默坏链一样难查 —— 字节数和残行原文都要进日志
//! （移植 `tests/evidence/test_torn_tail.py::test_healing_logs_a_warning_with_the_torn_bytes`）。
//!
//! 单独一个测试文件 = 单独一个测试二进制：`set_global_default` 一个进程只能装一次，
//! 而 `append` 的真正写入跑在 `spawn_blocking` 的另一个线程上，thread-local 的
//! `with_default` 到不了那里。
//!
//! 订阅者是手写的最小实现，只用 `tracing` 本身 —— 为一条测试往 `Cargo.lock` 里加
//! `tracing-subscriber` 不值当（`core/Cargo.lock` 归 R0，各轨不该动它）。

mod common;

use std::sync::{Arc, Mutex};

use aite_contracts::{EvidenceKind, EvidenceWriter};
use aite_evidence::FileEvidenceWriter;
use common::{p, tear};
use serde_json::json;
use tempfile::TempDir;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

const TASK: &str = "t-torn";
const HALF_LINE: &str = r#"{"task_id": "t-torn", "seq": 9, "kind": "model_ca"#;

/// 只收 WARNING 及以上的 message 字段，攒进一个缓冲区。
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Capture {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

impl Subscriber for Capture {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        *meta.level() <= Level::WARN
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut v = MessageVisitor::default();
        event.record(&mut v);
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{} {}", event.metadata().level(), v.message));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[tokio::test]
async fn healing_logs_a_warning_with_the_torn_bytes() {
    let capture = Capture::default();
    tracing::subscriber::set_global_default(capture.clone()).expect("装订阅者");

    let tmp = TempDir::new().unwrap();
    let w = FileEvidenceWriter::new(tmp.path().join("evidence"));
    w.append(TASK, EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();
    tear(&w, TASK, HALF_LINE);

    w.append(TASK, EvidenceKind::Failed, p(json!({"why": "crash"})))
        .await
        .unwrap();

    let path = w.events_path(TASK).display().to_string();
    let logged: Vec<String> = capture
        .lines()
        .into_iter()
        .filter(|l| l.contains(&path))
        .collect();
    assert_eq!(
        logged.len(),
        1,
        "收拾一次就该有且只有一条 WARNING：{:?}",
        capture.lines()
    );

    let message = &logged[0];
    assert!(message.starts_with("WARN "), "级别是 WARNING：{message}");
    assert!(
        message.contains(&HALF_LINE.len().to_string()),
        "截掉了多少字节：{message}"
    );
    assert!(
        message.contains(&HALF_LINE[..30]),
        "残行原文的前缀：{message}"
    );
    assert!(message.contains(&path), "路径：{message}");
}
