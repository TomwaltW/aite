"""长连接与重连（§3.3「长连接断开」+ §6 T1 的退避序列判据）。

判据原文：重连策略对假连接对象的退避序列断言为 `[1, 2, 4, 8, 16, 30, 30]`。

假连接对象在这个文件里自建（§3.4：不 import `aite/testing/`）。这里也顺带把
「adapter 不去重」「on_event 炸了不能带走长连接」两条钉住。
"""
from __future__ import annotations

import json
import logging
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any

import pytest

from aite.adapters.feishu import FeishuPlatform, backoff_delay
from aite.adapters.feishu import platform as platform_module
from aite.contracts import NormalizedEvent

FIXTURES_DIR = Path(__file__).resolve().parents[2] / "fixtures" / "feishu"
BOT_OPEN_ID = "ou_aite_bot_0000000000000000000001"

#: §6 T1 点名的序列。
EXPECTED_BACKOFF = [1, 2, 4, 8, 16, 30, 30]


class StopLoop(Exception):
    """踩到预期的 sleep 次数后掀桌，把 start() 的死循环停下来。"""


class RecordingSleep:
    """记录每次退避等了多久；到第 `stop_after` 次就抛 StopLoop。"""

    def __init__(self, stop_after: int) -> None:
        self.delays: list[float] = []
        self._stop_after = stop_after

    async def __call__(self, seconds: float) -> None:
        self.delays.append(seconds)
        if len(self.delays) >= self._stop_after:
            raise StopLoop


class FakeConnection:
    """`WSConnection` 的假实现。

    `script` 逐次决定第 n 次 `connect()` 的结果：`"fail"` 抛异常，`"ok"` 连上。
    连上之后 `wait_closed()` 立刻返回 —— 等价于「刚连上就又断了」，
    这样重连循环会一直转，正好用来量退避序列。
    """

    instances: list[FakeConnection] = []

    def __init__(self, on_raw: Callable[[dict[str, Any]], Awaitable[None]], script: list[str]) -> None:
        self.on_raw = on_raw
        self.script = script
        self.connect_calls = 0
        self.close_calls = 0
        FakeConnection.instances.append(self)

    async def connect(self) -> None:
        outcome = self.script[self.connect_calls] if self.connect_calls < len(self.script) else "ok"
        self.connect_calls += 1
        if outcome == "fail":
            raise ConnectionError(f"假连接第 {self.connect_calls} 次故意失败")

    async def wait_closed(self) -> None:
        return None

    async def close(self) -> None:
        self.close_calls += 1


def make_platform(script: list[str], sleep: RecordingSleep) -> FeishuPlatform:
    """每次重连都新建一个假连接（真实现也是这样：连接对象不复用）。"""
    shared: dict[str, Any] = {"connects": 0}

    def factory(on_raw: Callable[[dict[str, Any]], Awaitable[None]]) -> FakeConnection:
        index = shared["connects"]
        shared["connects"] += 1
        remaining = script[index:] if index < len(script) else ["ok"]
        return FakeConnection(on_raw, remaining[:1])

    return FeishuPlatform(bot_open_id=BOT_OPEN_ID, connection_factory=factory, sleep=sleep)


async def noop_handler(event: NormalizedEvent) -> None:
    return None


# ---------------------------------------------------------------------------
# 退避序列
# ---------------------------------------------------------------------------

def test_backoff_delay_is_exponential_capped_at_30() -> None:
    """§3.3：1s → 2s → … → 30s 封顶。"""
    assert [backoff_delay(n) for n in range(1, 8)] == EXPECTED_BACKOFF
    assert backoff_delay(20) == 30, "封顶之后一直是 30，不许再翻倍"
    assert backoff_delay(0) == 0


async def test_reconnect_backoff_sequence_is_1_2_4_8_16_30_30() -> None:
    """§6 T1：对假连接对象的退避序列必须是 [1, 2, 4, 8, 16, 30, 30]。"""
    sleep = RecordingSleep(stop_after=len(EXPECTED_BACKOFF))
    platform = make_platform(["fail"] * 20, sleep)

    with pytest.raises(StopLoop):
        await platform.start(noop_handler)

    assert sleep.delays == EXPECTED_BACKOFF


async def test_backoff_resets_after_a_successful_connect() -> None:
    """连上一次之后退避要归零，否则一晚上的抖动会把重连推到 30s 起步。"""
    sleep = RecordingSleep(stop_after=3)
    platform = make_platform(["fail", "fail", "ok"], sleep)

    with pytest.raises(StopLoop):
        await platform.start(noop_handler)

    # 前两次失败 → 1s、2s；第三次连上；连上就断 → 又从 1s 起。
    assert sleep.delays == [1, 2, 1]


async def test_infinite_retry_never_gives_up() -> None:
    """§3.3：无限重试。连挂 50 次也不许抛出去。"""
    sleep = RecordingSleep(stop_after=50)
    platform = make_platform(["fail"] * 60, sleep)

    with pytest.raises(StopLoop):
        await platform.start(noop_handler)

    assert len(sleep.delays) == 50
    assert sleep.delays[-5:] == [30, 30, 30, 30, 30]


async def test_reconnected_is_logged_at_info(caplog: pytest.LogCaptureFixture) -> None:
    """§3.3：重连成功打 INFO 日志 `feishu.reconnected`。"""
    sleep = RecordingSleep(stop_after=2)
    platform = make_platform(["fail", "ok"], sleep)

    with caplog.at_level(logging.INFO, logger="aite.adapters.feishu.platform"):
        with pytest.raises(StopLoop):
            await platform.start(noop_handler)

    reconnected = [r for r in caplog.records if r.message.startswith("feishu.reconnected")]
    assert reconnected, "重连成功必须有一条 feishu.reconnected"
    assert reconnected[0].levelno == logging.INFO


async def test_first_connect_is_not_delayed() -> None:
    """开机第一次连不该先等 1 秒。"""
    sleep = RecordingSleep(stop_after=1)
    platform = make_platform(["ok"], sleep)

    with pytest.raises(StopLoop):
        await platform.start(noop_handler)

    # 第一次连上、断开之后才出现第一次退避。
    assert sleep.delays == [1]
    assert FakeConnection.instances[0].connect_calls == 1


async def test_connection_is_closed_before_reconnecting() -> None:
    """断开后要把旧连接关掉，不能攒着一堆半死的连接。"""
    FakeConnection.instances.clear()
    sleep = RecordingSleep(stop_after=2)
    platform = make_platform(["ok", "ok"], sleep)

    with pytest.raises(StopLoop):
        await platform.start(noop_handler)

    assert all(c.close_calls >= 1 for c in FakeConnection.instances if c.connect_calls)


async def test_stop_breaks_the_loop() -> None:
    """stop() 之后 start() 要能正常返回，而不是继续重连。"""
    delays: list[float] = []
    platform_ref: dict[str, FeishuPlatform] = {}

    async def sleep(seconds: float) -> None:
        delays.append(seconds)
        await platform_ref["p"].stop()

    platform = make_platform(["ok"], sleep)  # type: ignore[arg-type]
    platform_ref["p"] = platform

    await platform.start(noop_handler)       # 不抛异常、能返回，就算过
    assert delays == [1]


# ---------------------------------------------------------------------------
# 事件投递
# ---------------------------------------------------------------------------

def load_raw(name: str) -> dict[str, Any]:
    return json.loads((FIXTURES_DIR / f"{name}.json").read_text(encoding="utf-8"))


async def test_raw_events_are_normalized_and_delivered() -> None:
    """长连接收到的原始事件要归一化后交给 on_event。"""
    seen: list[NormalizedEvent] = []

    async def handler(event: NormalizedEvent) -> None:
        seen.append(event)

    platform = FeishuPlatform(bot_open_id=BOT_OPEN_ID, app_id="cli_a1b2c3d4e5f60123")
    platform._on_event = handler
    await platform._dispatch_raw(load_raw("message_at_bot_toplevel"))

    assert len(seen) == 1
    assert seen[0].text == "把这个季度的销售数据画成趋势图"
    assert seen[0].mentioned is True


async def test_adapter_does_not_deduplicate_replayed_events() -> None:
    """§3.3：重连后平台重推的重复事件由 ControlPlane 靠 event_id 去重，adapter 不管。"""
    seen: list[NormalizedEvent] = []

    async def handler(event: NormalizedEvent) -> None:
        seen.append(event)

    platform = FeishuPlatform(bot_open_id=BOT_OPEN_ID)
    platform._on_event = handler
    raw = load_raw("message_at_bot_toplevel")
    await platform._dispatch_raw(raw)
    await platform._dispatch_raw(raw)

    assert len(seen) == 2, "adapter 私自去重的话，ControlPlane 的 events.duplicate 就永远是 0"
    assert seen[0].event_id == seen[1].event_id


async def test_unsubscribed_event_is_dropped_without_calling_handler() -> None:
    called: list[NormalizedEvent] = []

    async def handler(event: NormalizedEvent) -> None:
        called.append(event)

    platform = FeishuPlatform(bot_open_id=BOT_OPEN_ID)
    platform._on_event = handler
    raw = load_raw("message_at_bot_toplevel")
    raw["header"]["event_type"] = "im.chat.member.user.added_v1"
    await platform._dispatch_raw(raw)

    assert called == []


async def test_handler_exception_does_not_kill_the_connection(
    caplog: pytest.LogCaptureFixture,
) -> None:
    """§3.3「任何未捕获异常…进程不退出」：回调炸了不能把长连接带走。"""
    async def boom(event: NormalizedEvent) -> None:
        raise RuntimeError("上游炸了")

    platform = FeishuPlatform(bot_open_id=BOT_OPEN_ID)
    platform._on_event = boom
    with caplog.at_level(logging.ERROR, logger="aite.adapters.feishu.platform"):
        await platform._dispatch_raw(load_raw("message_at_bot_toplevel"))  # 不抛出去

    assert any("feishu.on_event_failed" in r.message for r in caplog.records)


async def test_slow_handler_is_reported(
    caplog: pytest.LogCaptureFixture, monkeypatch: pytest.MonkeyPatch
) -> None:
    """§3.2：on_event 必须 1s 内返回。超了不拦（拦了会丢事件），但要吼一声。"""
    monkeypatch.setattr(platform_module, "ON_EVENT_BUDGET_SEC", 0.0)

    platform = FeishuPlatform(bot_open_id=BOT_OPEN_ID)
    platform._on_event = noop_handler
    with caplog.at_level(logging.WARNING, logger="aite.adapters.feishu.platform"):
        await platform._dispatch_raw(load_raw("message_at_bot_toplevel"))

    assert any("feishu.on_event_slow" in r.message for r in caplog.records)
