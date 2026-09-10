"""长连接（§3.3「长连接断开」那一行 + 附录 A）。

这里只干两件事：

1. 定义 adapter 眼里的连接长什么样（`WSConnection`）—— 三个方法，够 `FeishuPlatform`
   跑重连循环即可。测试拿假连接对象顶上去，不用起真 websocket。
2. 把 lark_oapi 的 `ws.Client` 包成这个形状。

重连退避**不在**这里做，在 `FeishuPlatform` 的循环里（`backoff_delay` 是它用的）：
退避策略是 adapter 的行为契约（§6 T1 要断言 `[1,2,4,8,16,30,30]`），
不该被真连接实现绑架。SDK 自己的 `auto_reconnect` 一律关掉 —— 它用的是服务端下发的
固定间隔，跟 §3.3 要求的指数退避不是一回事。
"""
from __future__ import annotations

import asyncio
import logging
from collections.abc import Awaitable, Callable
from typing import Any, Protocol

from .normalize import EVENT_CARD_ACTION, EVENT_MESSAGE_RECEIVE

logger = logging.getLogger(__name__)

#: §3.3：1s → 2s → … → 30s 封顶，无限重试。
RECONNECT_BASE_SEC = 1
RECONNECT_MAX_SEC = 30

#: 原始事件（信封 dict）的消费者。
RawEventHandler = Callable[[dict[str, Any]], Awaitable[None]]


def backoff_delay(attempt: int) -> int:
    """第 `attempt` 次重连前该等多少秒（attempt 从 1 起）。

    1, 2, 4, 8, 16, 30, 30, …（32 会被 30s 的上限压回去）。
    """
    if attempt < 1:
        return 0
    return min(RECONNECT_BASE_SEC * 2 ** (attempt - 1), RECONNECT_MAX_SEC)


class WSConnection(Protocol):
    """adapter 需要的连接能力。"""

    async def connect(self) -> None:
        """建连。失败抛异常 —— 由 FeishuPlatform 决定退避多久再来。"""
        ...

    async def wait_closed(self) -> None:
        """一直等到连接断掉才返回。"""
        ...

    async def close(self) -> None:
        """主动断开，幂等。"""
        ...


class LarkWSConnection:
    """把 `lark_oapi.ws.Client` 包成 `WSConnection`。

    三处不得不碰 SDK 私有面，都写在这里，方便日后 SDK 补了公开 API 就替换掉：

    * `ws.Client.start()` 是**阻塞**的，而且它 `loop.run_until_complete(...)` 用的是
      模块级全局 loop —— 那个 loop 在 import 时就建好了，跟 `asyncio.run()` 起的
      运行时 loop 不是同一个。所以这里把全局 loop 对齐到当前运行 loop，再直接 await
      它的 `_connect()`。不对齐的话，`_connect()` 里 `loop.create_task(_receive_message_loop())`
      会挂到一个根本没在跑的 loop 上，连上了也收不到任何事件。
    * 断线没有回调可用（`auto_reconnect=False` 时 `_receive_message_loop` 只是把异常
      吞进 task 里），所以 `wait_closed()` 用轮询 `_conn is None` 来判。
    * 卡片回传帧要自己接回来，见 `route_card_frames_as_events`。
    """

    def __init__(
        self,
        *,
        app_id: str,
        app_secret: str,
        on_raw: RawEventHandler,
        domain: str = "https://open.feishu.cn",
        poll_interval: float = 1.0,
    ) -> None:
        self._app_id = app_id
        self._app_secret = app_secret
        self._on_raw = on_raw
        self._domain = domain
        self._poll_interval = poll_interval
        self._client: Any | None = None
        self._ping_task: asyncio.Task[None] | None = None
        self._loop: asyncio.AbstractEventLoop | None = None

    # -- 事件入口 ---------------------------------------------------------

    def _envelope(self, ctx: Any) -> dict[str, Any]:
        """SDK 的 CustomizedEvent → 原样的事件信封 dict。

        故意不带 `header.token`：它是应用的校验令牌，而信封会原样进
        `NormalizedEvent.raw` 落到审计里（§3.1），不该跟着躺进证据文件。
        """
        header = getattr(ctx, "header", None)
        header_dict = {}
        if header is not None:
            for key in ("event_id", "create_time", "event_type", "tenant_key", "app_id"):
                header_dict[key] = getattr(header, key, None)
        return {
            "schema": getattr(ctx, "schema", None) or "2.0",
            "header": header_dict,
            "event": getattr(ctx, "event", None) or {},
        }

    def _handle(self, ctx: Any) -> None:
        """SDK 回调（同步）→ 把原始事件丢回 adapter 的 loop。

        这里只做投递，不做任何重活：§3.2 要求 `on_event` 1s 内返回，
        而 SDK 是在收帧的协程里同步调这个函数的，堵在这里会把整条长连接拖住。
        """
        loop = self._loop
        if loop is None:  # pragma: no cover - connect 之前不会有事件
            return
        envelope = self._envelope(ctx)
        future = asyncio.run_coroutine_threadsafe(self._on_raw(envelope), loop)
        future.add_done_callback(_log_dispatch_failure)

    def _build_client(self) -> Any:
        import lark_oapi
        from lark_oapi.event.dispatcher_handler import EventDispatcherHandler

        handler = (
            EventDispatcherHandler.builder("", "")
            .register_p2_customized_event(EVENT_MESSAGE_RECEIVE, self._handle)
            .register_p2_customized_event(EVENT_CARD_ACTION, self._handle)
            .build()
        )
        return lark_oapi.ws.Client(
            self._app_id,
            self._app_secret,
            event_handler=handler,
            domain=self._domain,
            # 重连归 FeishuPlatform 管，SDK 自己那套固定间隔不要掺和进来。
            auto_reconnect=False,
        )

    # -- WSConnection -----------------------------------------------------

    async def connect(self) -> None:
        import lark_oapi.ws.client as ws_client

        self._loop = asyncio.get_running_loop()
        ws_client.loop = self._loop
        self._client = self._build_client()
        route_card_frames_as_events(self._client)
        await self._client._connect()
        self._ping_task = asyncio.create_task(self._client._ping_loop())

    async def wait_closed(self) -> None:
        while self._client is not None and self._client._conn is not None:
            await asyncio.sleep(self._poll_interval)

    async def close(self) -> None:
        if self._ping_task is not None:
            self._ping_task.cancel()
            try:
                await self._ping_task
            except asyncio.CancelledError:
                pass
            except Exception as exc:  # pragma: no cover - 关连接路上的失败不值得再抛
                logger.warning("feishu.ping_stop_failed err=%s", exc)
            self._ping_task = None
        if self._client is not None:
            try:
                await self._client._disconnect()
            except Exception as exc:  # pragma: no cover - 关连接的失败没什么可做的
                logger.warning("feishu.disconnect_failed err=%s", exc)
            self._client = None


def route_card_frames_as_events(client: Any) -> None:
    """把长连接上的卡片回传帧接回事件分发 —— 不接的话 SDK 会直接丢掉。

    lark-oapi 1.7.3 的 `ws/client.py::_handle_data_frame` 里，`MessageType.CARD`
    那一支是一句光秃秃的 `return`：卡片回传帧连分发都进不去，`!stop` / `证据`
    按钮点下去服务端什么都不会发生。`MessageType.CARD` 在整个 SDK 里**只出现这一处**，
    没有第二条路能接到它。

    偏偏事件帧那一支认得 `card.action.trigger`：
    `EventDispatcherHandler._do_without_validation()` 拿 `header.event_type` 拼出
    `p2.card.action.trigger` 去查 `_processorMap`，而那正是 `_build_client()` 里
    `register_p2_customized_event(EVENT_CARD_ACTION, ...)` 写进去的 key。

    所以这里只做一件事：把 CARD 帧的 type 头改写成 event，再交回 SDK 原来的处理。
    只碰 CARD 帧 —— 平台若本来就用 EVENT 帧发卡片回传，或 SDK 哪天自己补上这一支，
    这段就是一次空过路，不会多出第二种行为。

    值得多一句：官方明说回调是**同步**的、**不提供补推**，超时未响应即算这次失败。
    也就是说丢掉的卡片回传永远不会重来，不像事件还有重推兜底。
    https://open.feishu.cn/document/event-subscription-guide/callback-subscription/callback-overview
    """
    from lark_oapi.ws.const import HEADER_TYPE
    from lark_oapi.ws.enum import MessageType

    original = client._handle_data_frame

    async def _handle_data_frame(frame: Any) -> None:
        for header in frame.headers:
            if header.key == HEADER_TYPE and header.value == MessageType.CARD.value:
                header.value = MessageType.EVENT.value
                break
        await original(frame)

    client._handle_data_frame = _handle_data_frame


def _log_dispatch_failure(future: Any) -> None:
    try:
        future.result()
    except Exception as exc:  # pragma: no cover - 只为了不吞异常
        logger.exception("feishu.dispatch_failed err=%s", exc)
