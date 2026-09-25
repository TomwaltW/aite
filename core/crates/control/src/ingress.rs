//! 事件入口 —— 对应 `aite/ingress/handler.py`。
//!
//! adapter 的 `PlatformPort::start(on_event)` 回调必须在 1s 内返回，重活不能在回调里做
//! （§3.3 第一行）。`ControlPlane::handle_event` 本身只做「去重 + 入库 + 入队」，真正的
//! 执行在 `run_forever` 那条线上，所以这层薄薄一片，只负责两件事：
//!
//! 1. **不让错误漏回 adapter**：回调里往外抛会让长连接的读循环挂掉，而 §3.3 要求
//!    「任何未捕获异常 → 进程不退出」。
//! 2. **把慢回调叫出来**：超过 1s 就打 WARNING，免得哪天有人往路由里塞了重活还没人发现。
//!
//! **CC2 ③ 起有一类错误要传出去**：[`Ingress::handler`]（交给 `PlatformPort::start` 的那个回调）
//! 对存储 / 证据错误（`IngressError::Store` / `::Evidence`）返回 `Err`，edge-client 的 gRPC 入口
//! 把它翻成 INTERNAL（`proto/src/status.rs`：非 `Invalid` 一律 INTERNAL），edge 让平台重推。
//! 其余错误（平台、非法事件、其它）照旧吞掉返回 `Ok`。计数器与 `ingress.handle_failed` 日志
//! 两种情况都照打。[`Ingress::on_event`] 仍返回 `()`：直接调它的测试与旧用法不变。
//!
//! 边界：`seen_event` **之后**的失败，重推回来仍会被 R2 当重复吃掉（`ingress_failures.rs` 钉着）——
//! 这里只负责「传出去」；让它真能救回来要撤销那条去重记录，`SessionStore` 没这个方法（契约缺口）。
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use aite_contracts::{
    ControlPlane, EventHandler, IngressError, NormalizedEvent, PlatformError, PlatformPort,
};

use crate::lock;

pub const SLOW_CALLBACK_SEC: f64 = 1.0;

/// 单调钟（秒）。默认 `Instant`，测试注入假钟。
type MonoClock = Arc<dyn Fn() -> f64 + Send + Sync>;

fn default_clock() -> MonoClock {
    let origin = std::time::Instant::now();
    Arc::new(move || origin.elapsed().as_secs_f64())
}

struct IngressInner {
    plane: Arc<dyn ControlPlane>,
    slow_callback_sec: f64,
    clock: MonoClock,
    counters: Mutex<BTreeMap<String, i64>>,
}

impl IngressInner {
    fn bump(&self, key: &str) {
        *lock(&self.counters).entry(key.to_string()).or_insert(0) += 1;
    }

    /// 处理一条事件；错误照旧计数、打日志，并**原样交回**给调用方去决定吞不吞。
    async fn on_event(&self, ev: NormalizedEvent) -> Result<(), IngressError> {
        let t0 = (self.clock)();
        let event_id = ev.event_id.clone();
        let kind = ev.kind;
        let out = self.plane.handle_event(ev).await;
        match &out {
            Ok(()) => self.bump("events.handled"),
            Err(e) => {
                self.bump("ingress.errors");
                tracing::error!(
                    target: "aite.ingress",
                    event = %event_id,
                    kind = %kind,
                    error = %e,
                    "ingress.handle_failed"
                );
            }
        }
        let elapsed = (self.clock)() - t0;
        if elapsed > self.slow_callback_sec {
            self.bump("ingress.slow");
            tracing::warn!(
                target: "aite.ingress",
                event = %event_id,
                elapsed,
                "ingress.slow_callback"
            );
        }
        out
    }
}

/// 把 `PlatformPort` 的事件流接到 `ControlPlane`。
#[derive(Clone)]
pub struct Ingress {
    inner: Arc<IngressInner>,
}

impl Ingress {
    pub fn new(plane: Arc<dyn ControlPlane>) -> Self {
        Self::build(plane, SLOW_CALLBACK_SEC, default_clock())
    }

    /// 改慢回调阈值（默认 [`SLOW_CALLBACK_SEC`]）。只在构造期用。
    pub fn with_slow_callback_sec(self, secs: f64) -> Self {
        Self::build(
            Arc::clone(&self.inner.plane),
            secs,
            Arc::clone(&self.inner.clock),
        )
    }

    /// 注入单调钟（测试）。只在构造期用。
    pub fn with_clock(self, clock: MonoClock) -> Self {
        Self::build(
            Arc::clone(&self.inner.plane),
            self.inner.slow_callback_sec,
            clock,
        )
    }

    fn build(plane: Arc<dyn ControlPlane>, slow_callback_sec: f64, clock: MonoClock) -> Self {
        Self {
            inner: Arc::new(IngressInner {
                plane,
                slow_callback_sec,
                clock,
                counters: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// 事件回调本体。**任何错误都在这里吞掉**，不往 adapter 抛。
    pub async fn on_event(&self, ev: NormalizedEvent) {
        let _ = self.inner.on_event(ev).await;
    }

    /// 交给 `PlatformPort::start` 的那个回调。
    ///
    /// 存储 / 证据错误返回 `Err`（gRPC 入口翻成 INTERNAL，平台重推）；其余错误吞掉返回 `Ok`，
    /// 与 CC2 之前一样（平台抽风、非法事件重推也没用，只会让读循环反复撞同一条）。
    pub fn handler(&self) -> EventHandler {
        let inner = Arc::clone(&self.inner);
        Arc::new(move |ev: NormalizedEvent| {
            let inner = Arc::clone(&inner);
            Box::pin(async move {
                match inner.on_event(ev).await {
                    Err(e @ (IngressError::Store(_) | IngressError::Evidence(_))) => Err(e),
                    Ok(()) | Err(_) => Ok(()),
                }
            })
        })
    }

    /// 建长连接并开始投递。adapter 负责重连，这里只是把回调交出去。
    pub async fn start(&self, platform: &dyn PlatformPort) -> Result<(), PlatformError> {
        platform.start(self.handler()).await
    }

    /// 计数器快照：`events.handled` / `ingress.errors` / `ingress.slow`。
    pub fn counters(&self) -> Map<String, Value> {
        lock(&self.inner.counters)
            .iter()
            .map(|(k, v)| (k.clone(), Value::from(*v)))
            .collect()
    }

    /// 单个计数器，没被碰过就是 0（对齐 Python 的 `defaultdict(int)`）。
    pub fn counter(&self, key: &str) -> i64 {
        lock(&self.inner.counters).get(key).copied().unwrap_or(0)
    }
}
