"""事件入口（owner: T2）。

adapter 的 `PlatformPort.start(on_event)` 回调必须在 1s 内返回，重活不能在回调里做
（§3.3 第一行）。`ControlPlane.handle_event` 本身只做「去重 + 入库 + 入队」，
真正的执行在 `run_forever` 那条线上，所以这层薄薄一片，只负责两件事：

1. **不让异常漏回 adapter**：回调里抛出去会让长连接的读循环挂掉，而 §3.3 要求
   「任何未捕获异常 → 进程不退出」。
2. **把慢回调叫出来**：超过 1s 就打 WARNING，免得哪天有人往路由里塞了重活还没人发现。
"""
import asyncio
import logging
import time
from collections import defaultdict
from collections.abc import Callable

from ..contracts import ControlPlane, EventHandler, NormalizedEvent, PlatformPort

log = logging.getLogger("aite.ingress")

SLOW_CALLBACK_SEC = 1.0


class Ingress:
    """把 PlatformPort 的事件流接到 ControlPlane。"""

    def __init__(
        self,
        plane: ControlPlane,
        *,
        slow_callback_sec: float = SLOW_CALLBACK_SEC,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._plane = plane
        self._slow = slow_callback_sec
        self._clock = clock
        self.counters: dict[str, int] = defaultdict(int)

    async def on_event(self, ev: NormalizedEvent) -> None:
        t0 = self._clock()
        try:
            await self._plane.handle_event(ev)
            self.counters["events.handled"] += 1
        except asyncio.CancelledError:
            raise
        except Exception:
            self.counters["ingress.errors"] += 1
            log.exception("ingress.handle_failed event=%s kind=%s", ev.event_id, ev.kind)
        finally:
            elapsed = self._clock() - t0
            if elapsed > self._slow:
                self.counters["ingress.slow"] += 1
                log.warning("ingress.slow_callback event=%s elapsed=%.3fs", ev.event_id, elapsed)

    def as_handler(self) -> EventHandler:
        return self.on_event

    async def start(self, platform: PlatformPort) -> None:
        """建长连接并开始投递。adapter 负责重连，这里只是把回调交出去。"""
        await platform.start(self.on_event)
