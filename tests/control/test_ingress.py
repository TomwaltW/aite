"""Ingress：adapter 的事件回调必须 1s 内返回，且异常不能漏回去（§3.3 第一行、最后一行）。"""
from control_fakes import FakeClock, FakePlatform, make_event

from aite.contracts import NormalizedEvent
from aite.ingress import Ingress


class RecordingPlane:
    def __init__(self, *, boom: bool = False, clock: FakeClock | None = None, cost: float = 0.0) -> None:
        self.seen: list[NormalizedEvent] = []
        self._boom = boom
        self._clock = clock
        self._cost = cost

    async def handle_event(self, ev: NormalizedEvent) -> None:
        self.seen.append(ev)
        if self._clock is not None and self._cost:
            self._clock.advance(self._cost)
        if self._boom:
            raise RuntimeError("路由炸了")

    async def run_forever(self) -> None:  # pragma: no cover - 协议要求，这里用不到
        raise NotImplementedError


async def test_events_go_to_the_plane():
    plane = RecordingPlane()
    ingress = Ingress(plane)
    ev = make_event()

    await ingress.on_event(ev)

    assert plane.seen == [ev]
    assert ingress.counters["events.handled"] == 1
    assert ingress.counters["ingress.errors"] == 0


async def test_exceptions_never_reach_the_adapter():
    """回调里抛出去会把长连接的读循环带走，所以这里必须吞掉并计数。"""
    plane = RecordingPlane(boom=True)
    ingress = Ingress(plane)

    await ingress.on_event(make_event())    # 不抛

    assert ingress.counters["ingress.errors"] == 1
    assert ingress.counters["events.handled"] == 0


async def test_slow_callback_is_counted():
    clock = FakeClock()
    plane = RecordingPlane(clock=clock, cost=2.0)
    ingress = Ingress(plane, clock=clock)

    await ingress.on_event(make_event())

    assert ingress.counters["ingress.slow"] == 1


async def test_fast_callback_is_not_flagged():
    clock = FakeClock()
    plane = RecordingPlane(clock=clock, cost=0.2)
    ingress = Ingress(plane, clock=clock)

    await ingress.on_event(make_event())

    assert ingress.counters["ingress.slow"] == 0


async def test_start_hands_the_handler_to_the_platform():
    plane = RecordingPlane()
    ingress = Ingress(plane)
    platform = FakePlatform()

    await ingress.start(platform)

    assert platform.started is True
    assert ingress.as_handler() == ingress.on_event   # 绑定方法每次取都是新对象，比相等不比同一
