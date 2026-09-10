"""T16：卡片回传帧在长连接上到底能不能到我们手里。

附录 A 第 1 条说卡片回传走长连接、事件名 `card.action.trigger`。事件名对得上，
但 lark-oapi 1.7.3 的 `ws/client.py::_handle_data_frame` 把 `MessageType.CARD`
那一支写成了一句 `return` —— 帧收到了，然后被丢掉，`!stop` / `证据` 按钮点了没反应。

这两条测试一正一反：
* `test_sdk_alone_drops_card_frames` 钉住 SDK 当下的行为。哪天升级 lark-oapi
  它自己修好了，这条会红 —— 那就是可以把 `route_card_frames_as_events` 删掉的信号。
* `test_card_frames_reach_the_handler` 钉住加了转接之后卡片回传能一路到 `on_raw`。

不需要真凭证：直接构造 protobuf 帧喂给 SDK 的数据帧处理函数。
"""
from __future__ import annotations

import asyncio
import json
from typing import Any

import pytest

from aite.adapters.feishu.connection import LarkWSConnection, route_card_frames_as_events

pytestmark = pytest.mark.asyncio

CARD_ACTION_PAYLOAD: dict[str, Any] = {
    "schema": "2.0",
    "header": {
        "event_id": "evt_card_frame_0001",
        "create_time": "1788916020000000",
        "event_type": "card.action.trigger",
        "tenant_key": "tk_p0_demo",
        "app_id": "cli_a1b2c3d4e5f60123",
    },
    "event": {
        "operator": {"open_id": "ou_zhang_san_00000000000000000001"},
        "action": {"tag": "button", "value": {"action": "stop", "task_id": "t-1"}},
        "context": {
            "open_message_id": "om_checklist_card_0001",
            "open_chat_id": "oc_chat_p0_demo_0001",
        },
    },
}


class _StubConn:
    """`_handle_data_frame` 末尾要往连接上回写一帧响应，给它一个收得下的桶。"""

    def __init__(self) -> None:
        self.sent: list[bytes] = []

    async def send(self, data: bytes) -> None:
        self.sent.append(data)


def _data_frame(message_type: str, payload: dict[str, Any]) -> Any:
    """造一个 SDK 认得的数据帧。四个头缺一个都会抛 HeaderNotFoundException。"""
    from lark_oapi.ws.const import (
        HEADER_MESSAGE_ID,
        HEADER_SEQ,
        HEADER_SUM,
        HEADER_TRACE_ID,
        HEADER_TYPE,
    )
    from lark_oapi.ws.enum import FrameType
    from lark_oapi.ws.pb.pbbp2_pb2 import Frame

    frame = Frame()
    # 这四个是 protobuf 的 required 字段，不填 SerializeToString() 会抛 EncodeError
    # （照 SDK 自己 `_new_ping_frame` 的填法）。
    frame.service = 1
    frame.method = FrameType.DATA.value
    frame.SeqID = 0
    frame.LogID = 0
    for key, value in (
        (HEADER_TYPE, message_type),
        (HEADER_MESSAGE_ID, "msg-1"),
        (HEADER_TRACE_ID, "trace-1"),
        (HEADER_SUM, "1"),
        (HEADER_SEQ, "0"),
    ):
        header = frame.headers.add()
        header.key = key
        header.value = value
    frame.payload = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    return frame


async def _make_client(received: list[dict[str, Any]]) -> Any:
    async def on_raw(raw: dict[str, Any]) -> None:
        received.append(raw)

    connection = LarkWSConnection(app_id="cli_test", app_secret="secret", on_raw=on_raw)
    connection._loop = asyncio.get_running_loop()
    client = connection._build_client()
    client._conn = _StubConn()
    return client


async def _drain() -> None:
    """`_handle` 是用 run_coroutine_threadsafe 投递的，让出几次让它跑完。"""
    for _ in range(10):
        await asyncio.sleep(0)


async def test_sdk_alone_drops_card_frames() -> None:
    """lark-oapi 1.7.3 自己会把 MessageType.CARD 丢掉 —— 这条红了就说明 SDK 修好了。"""
    from lark_oapi.ws.enum import MessageType

    received: list[dict[str, Any]] = []
    client = await _make_client(received)

    await client._handle_data_frame(_data_frame(MessageType.CARD.value, CARD_ACTION_PAYLOAD))
    await _drain()

    assert received == [], "SDK 突然会转发卡片帧了？那 route_card_frames_as_events 就该退休"


async def test_card_frames_reach_the_handler() -> None:
    """接上转接之后，卡片回传帧一路到 `on_raw`，内容原样。"""
    from lark_oapi.ws.enum import MessageType

    received: list[dict[str, Any]] = []
    client = await _make_client(received)
    route_card_frames_as_events(client)

    await client._handle_data_frame(_data_frame(MessageType.CARD.value, CARD_ACTION_PAYLOAD))
    await _drain()

    assert len(received) == 1, "卡片回传该到 on_raw"
    envelope = received[0]
    assert envelope["header"]["event_type"] == "card.action.trigger"
    assert envelope["header"]["event_id"] == "evt_card_frame_0001"
    assert envelope["event"]["action"]["value"]["action"] == "stop"
    assert envelope["event"]["context"]["open_message_id"] == "om_checklist_card_0001"
    assert "token" not in envelope["header"], "校验令牌不该跟着进审计（见 _envelope）"


async def test_event_frames_are_untouched_by_the_shim() -> None:
    """转接只碰 CARD 帧：普通消息事件走原路，行为一个字不变。"""
    from lark_oapi.ws.enum import MessageType

    payload = {
        "schema": "2.0",
        "header": {
            "event_id": "evt_msg_0001",
            "create_time": "1788915720000",
            "event_type": "im.message.receive_v1",
            "app_id": "cli_a1b2c3d4e5f60123",
        },
        "event": {"message": {"message_id": "om_x", "chat_id": "oc_x"}},
    }

    received: list[dict[str, Any]] = []
    client = await _make_client(received)
    route_card_frames_as_events(client)

    await client._handle_data_frame(_data_frame(MessageType.EVENT.value, payload))
    await _drain()

    assert len(received) == 1
    assert received[0]["header"]["event_type"] == "im.message.receive_v1"
