//! 沙箱 reaper（W7）：先 sleep 后 reap，异常不打断循环。CC2 从 `plane.rs` 原样搬来。
use crate::plane::InProcessControlPlane;

/// W7：沙箱 reaper 的节奏。
pub const REAPER_INTERVAL_SEC: f64 = 60.0;

impl InProcessControlPlane {
    /// W7：**先 sleep 后 reap**，异常不打断循环。
    pub(crate) async fn reaper_loop(&self) {
        loop {
            (self.sleep)(self.reaper_interval_sec).await;
            let Some(sandbox) = self.sandbox.as_ref() else {
                continue;
            };
            match sandbox.reap_idle(self.config.sandbox.idle_sec).await {
                Err(e) => {
                    tracing::error!(target: "aite.control", error = %e, "control.reap_failed");
                    continue;
                }
                Ok(released) if !released.is_empty() => {
                    let n = released.len();
                    for _ in 0..n {
                        self.shared.bump("sandbox.reaped");
                    }
                    tracing::info!(target: "aite.control", n, "control.reaped");
                }
                Ok(_) => {}
            }
        }
    }
}
