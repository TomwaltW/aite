"""出站限速（§3.1 `PlatformCapabilities.outbound_rate_per_min`，「adapter 自己令牌桶」）。

时钟和 sleep 都可注入：测试要断言「第 N 次调用被挡下来了」，不能真睡一分钟。
"""
from __future__ import annotations

import asyncio
import time
from collections.abc import Awaitable, Callable


class TokenBucket:
    """每分钟 `rate_per_min` 个令牌的匀速令牌桶。

    桶容量默认等于每分钟额度：空闲一分钟后允许一次突发，之后按 60/rate 的间隔匀速放行。
    """

    def __init__(
        self,
        rate_per_min: int,
        *,
        capacity: int | None = None,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
    ) -> None:
        if rate_per_min <= 0:
            raise ValueError(f"rate_per_min 必须为正：{rate_per_min}")
        self.rate_per_min = rate_per_min
        self.capacity = capacity if capacity is not None else rate_per_min
        self._per_sec = rate_per_min / 60.0
        self._clock = clock
        self._sleep = sleep
        self._tokens = float(self.capacity)
        self._updated_at = clock()
        self._lock = asyncio.Lock()

    def _refill(self) -> None:
        now = self._clock()
        elapsed = now - self._updated_at
        if elapsed > 0:
            self._tokens = min(self.capacity, self._tokens + elapsed * self._per_sec)
            self._updated_at = now

    @property
    def tokens(self) -> float:
        """当前可用令牌数（会先补一次）。只给测试和排障看。"""
        self._refill()
        return self._tokens

    async def acquire(self, tokens: int = 1) -> float:
        """取 `tokens` 个令牌，不够就等到够。返回实际等待的秒数。"""
        if tokens > self.capacity:
            raise ValueError(f"一次要 {tokens} 个令牌，超过桶容量 {self.capacity}")
        waited = 0.0
        async with self._lock:
            while True:
                self._refill()
                if self._tokens >= tokens:
                    self._tokens -= tokens
                    return waited
                deficit = tokens - self._tokens
                delay = deficit / self._per_sec
                waited += delay
                await self._sleep(delay)
