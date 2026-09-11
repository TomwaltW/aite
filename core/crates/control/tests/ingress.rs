//! Ingress：adapter 的事件回调必须 1s 内返回，且错误不能漏回去（§3.3 第一行、最后一行）。
//! 对应 `tests/control/test_ingress.py`（5 条）。
mod support;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Map, Value};

use aite_contracts::{ControlPlane, IngressError, NormalizedEvent, Task};
use aite_control::Ingress;
use support::{FakeClock, FakePlatform, ev};

/// 只记事件、不做别的控制面（对应 Python 的 `RecordingPlane`）。
struct RecordingPlane {
    seen: Mutex<Vec<NormalizedEvent>>,
    boom: bool,
    clock: Option<Arc<FakeClock>>,
    cost: f64,
}

impl RecordingPlane {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            boom: false,
            clock: None,
            cost: 0.0,
        })
    }
    fn boom() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            boom: true,
            clock: None,
            cost: 0.0,
        })
    }
    fn costing(clock: Arc<FakeClock>, cost: f64) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            boom: false,
            clock: Some(clock),
            cost,
        })
    }
    fn seen(&self) -> Vec<NormalizedEvent> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

#[async_trait]
impl ControlPlane for RecordingPlane {
    async fn handle_event(&self, event: NormalizedEvent) -> Result<(), IngressError> {
        self.seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(event);
        if let Some(clock) = self.clock.as_ref()
            && self.cost != 0.0
        {
            clock.advance(self.cost);
        }
        if self.boom {
            return Err(IngressError::Other("路由炸了".into()));
        }
        Ok(())
    }
    async fn run_forever(&self) {
        unreachable!("协议要求，这里用不到")
    }
    async fn run_pending(&self) {}
    fn pending(&self) -> usize {
        0
    }
    async fn join(&self) {}
    async fn cancel_task(
        &self,
        task: Task,
        _reply_to: Option<String>,
        _chat_id: Option<String>,
        _notify: bool,
    ) -> Task {
        task
    }
    fn counters(&self) -> Map<String, Value> {
        Map::new()
    }
}

#[tokio::test]
async fn events_go_to_the_plane() {
    let plane = RecordingPlane::new();
    let ingress = Ingress::new(plane.clone());
    let event = ev().build();

    ingress.on_event(event.clone()).await;

    assert_eq!(plane.seen(), vec![event]);
    assert_eq!(ingress.counter("events.handled"), 1);
    assert_eq!(ingress.counter("ingress.errors"), 0);
}

/// 回调里抛出去会把长连接的读循环带走，所以这里必须吞掉并计数。
#[tokio::test]
async fn errors_never_reach_the_adapter() {
    let plane = RecordingPlane::boom();
    let ingress = Ingress::new(plane);

    ingress.on_event(ev().build()).await; // 不抛

    assert_eq!(ingress.counter("ingress.errors"), 1);
    assert_eq!(ingress.counter("events.handled"), 0);
}

#[tokio::test]
async fn slow_callback_is_counted() {
    let clock = FakeClock::new();
    let plane = RecordingPlane::costing(clock.clone(), 2.0);
    let ingress = Ingress::new(plane).with_clock(clock.as_mono());

    ingress.on_event(ev().build()).await;

    assert_eq!(ingress.counter("ingress.slow"), 1);
}

#[tokio::test]
async fn fast_callback_is_not_flagged() {
    let clock = FakeClock::new();
    let plane = RecordingPlane::costing(clock.clone(), 0.2);
    let ingress = Ingress::new(plane).with_clock(clock.as_mono());

    ingress.on_event(ev().build()).await;

    assert_eq!(ingress.counter("ingress.slow"), 0);
    assert_eq!(ingress.counter("events.handled"), 1);
}

/// `start` 把回调交给平台。
///
/// Python 那条最后比的是 `ingress.as_handler() == ingress.on_event`（绑定方法每次取都是
/// 新对象，所以比相等不比同一）。Rust 里 `EventHandler` 是 `Arc<dyn Fn>`，没有等价的
/// 「相等」可比 —— 改成验它**真的会投递**：拿平台收下的那个回调喂一条事件，控制面就该收到。
#[tokio::test]
async fn start_hands_a_working_handler_to_the_platform() {
    let plane = RecordingPlane::new();
    let ingress = Ingress::new(plane.clone());
    let platform = FakePlatform::new();

    ingress.start(platform.as_ref()).await.expect("start");

    assert!(platform.started());
    let handler = platform.handler().expect("平台该收到回调");
    let event = ev().id("ev-via-handler").build();
    handler(event.clone()).await.expect("回调永远 Ok");
    assert_eq!(plane.seen(), vec![event]);
    assert_eq!(ingress.counter("events.handled"), 1);
}
