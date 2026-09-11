//! `TaskWorker::in_flight()` —— 旧 `aite/app.py::AppWorker.in_flight` 并进 worker 的那半。
//!
//! Python 侧只有 `tests/integration/test_t8_shutdown_grace_timeout.py` 间接压着它
//! （收尾超时时 `stranded = list(worker.in_flight.values())`，归 RΩ）。本轨是它的 owner，
//! 所以在这里直接钉两条：跑的时候看得见、跑完清得干净，且看到的是**当前**进度不是开跑快照
//! —— 收尾给硬取消的任务善终时，写进证据的 `steps` 靠的就是这个。
mod common;

use std::sync::{Arc, Mutex, Weak};

use aite_contracts::{TaskStatus, TaskWorker};
use aite_worker::AgentWorker;
use common::*;
use serde_json::json;

type Peek = Arc<Mutex<Vec<(usize, Vec<(aite_contracts::Task, aite_contracts::Session)>)>>>;

/// 跑到第 `step` 步时抄一份 `in_flight()`。
fn peek_hook(worker: Arc<Mutex<Weak<AgentWorker>>>, seen: Peek) -> OnCall {
    Arc::new(move |step| {
        let worker = worker.clone();
        let seen = seen.clone();
        Box::pin(async move {
            let live = worker
                .lock()
                .expect("worker")
                .upgrade()
                .map(|w| w.in_flight())
                .unwrap_or_default();
            seen.lock().expect("seen").push((step, live));
        })
    })
}

#[tokio::test]
async fn in_flight_shows_the_running_task_and_clears_afterwards() {
    let mut base = Harness::new();
    base.seed("帮我出个图", Vec::new()).await;
    let h = Arc::new(base);

    let slot: Arc<Mutex<Weak<AgentWorker>>> = Arc::new(Mutex::new(Weak::new()));
    let seen: Peek = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(
        ScriptedModel::new(vec![
            tool_turn(&[("checklist_add", json!({"items": ["取数"]}))]),
            final_turn("好了"),
        ])
        .with_clock(h.clock.clone(), 0.6)
        .with_on_call(peek_hook(slot.clone(), seen.clone())),
    );
    let worker = Arc::new(h.worker(model));
    *slot.lock().expect("worker") = Arc::downgrade(&worker);

    let task = worker
        .run(
            h.task.clone(),
            h.session.clone(),
            Some("张三".into()),
            h.hooks(),
        )
        .await;

    assert_eq!(task.status, TaskStatus::Delivered);
    let seen = seen.lock().expect("seen").clone();
    assert_eq!(seen.len(), 2, "两步都抄到了");
    for (step, live) in &seen {
        assert_eq!(live.len(), 1, "第 {step} 步时在飞的任务只有一个");
        assert_eq!(live[0].0.id, h.task.id);
        assert_eq!(live[0].1.id, h.session.id);
    }
    assert!(worker.in_flight().is_empty(), "跑完要从表里摘掉");
}

#[tokio::test]
async fn in_flight_snapshot_tracks_progress_not_the_launch_state() {
    // 收尾硬取消时要拿这份快照去写 cancelled 证据，steps 停在 0 的话时间线就对不上了
    let mut base = Harness::new();
    base.seed("帮我出个图", Vec::new()).await;
    let h = Arc::new(base);

    let slot: Arc<Mutex<Weak<AgentWorker>>> = Arc::new(Mutex::new(Weak::new()));
    let seen: Peek = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(
        ScriptedModel::repeating(tool_turn(&[("checklist_note", json!({"text": "在算"}))]))
            .with_clock(h.clock.clone(), 0.6)
            .with_on_call(peek_hook(slot.clone(), seen.clone())),
    );
    let worker = Arc::new(h.worker(model));
    *slot.lock().expect("worker") = Arc::downgrade(&worker);

    worker
        .run(h.task.clone(), h.session.clone(), None, h.hooks())
        .await;

    let seen = seen.lock().expect("seen").clone();
    // 第 1 次 peek 在第 0 步开跑之前 → steps 还是 0；第 4 次 peek 时前 3 步已经落库
    assert_eq!(seen[0].1[0].0.steps, 0);
    assert_eq!(seen[3].1[0].0.steps, 3);
    assert_eq!(seen[3].1[0].0.status, TaskStatus::Working);
}
