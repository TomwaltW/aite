"""FakePlatform 自测：记账准不准、卡片是不是真的原地更新（dev-spec §3.2 / §3.8 03）。"""
from __future__ import annotations

import inspect

import pytest

from aite.contracts import ChecklistCard, ChecklistItemView, OutboundFile, OutboundText
from aite.contracts.ports import DocumentContent, PlatformPort
from aite.testing import FakePlatform, FakePlatformError, history_message
from aite.testing.samples import PNG_1X1, PNG_MAGIC

PORT_METHODS = [
    "start", "stop", "send_text", "send_card", "update_card",
    "send_file", "add_reaction", "read_history", "read_document", "download_file",
]


def make_card(status="working", items=("拉数据",)):
    return ChecklistCard(
        task_id="t1",
        task_no="#A1",
        title="跑对账",
        initiator="Alice",
        started_at="9:02",
        status=status,
        items=[ChecklistItemView(id=f"c{i}", text=t, state="todo") for i, t in enumerate(items, 1)],
    )


def test_implements_every_platform_port_method():
    """§3.2 的 10 个方法一个都不能少，且异步形态要对上。"""
    p = FakePlatform()
    for name in PORT_METHODS:
        fn = getattr(p, name, None)
        assert callable(fn), f"FakePlatform 缺 {name}"
        assert inspect.iscoroutinefunction(fn), f"{name} 应该是 async"
    assert set(PORT_METHODS) == {
        n for n, o in vars(PlatformPort).items() if not n.startswith("_") and inspect.isfunction(o)
    }
    assert p.capabilities.platform == "fake"


async def test_send_text_is_recorded_with_params():
    p = FakePlatform()
    res = await p.send_text(OutboundText(chat_id="oc_1", text="你好", reply_to="om_1"))
    assert res.message_id
    assert p.count("send_text") == 1
    call = p.calls.last("send_text")
    assert call.kwargs == {"chat_id": "oc_1", "reply_to": "om_1", "in_thread": True, "text": "你好"}
    assert p.texts() == ["你好"]


async def test_card_is_updated_in_place_not_resent():
    """一张卡片、三次原地更新 —— §3.8 的 03 要的就是这个形状。"""
    p = FakePlatform()
    sent = await p.send_card("oc_1", "om_1", make_card())
    for state in ("working", "working", "delivered"):
        await p.update_card(sent.card_id, make_card(status=state))

    assert p.count("send_card") == 1
    assert p.update_count == 3
    assert p.card_count == 1                      # 没有第二条卡片
    snaps = p.card_snapshots(sent.card_id)
    assert len(snaps) == 4                        # 初始 1 + 更新 3
    assert snaps[-1].status == "delivered"


async def test_update_card_with_unknown_id_raises():
    """拿不存在的 card_id 更新 = 没有原地更新，必须当场炸。"""
    p = FakePlatform()
    with pytest.raises(FakePlatformError, match="不存在"):
        await p.update_card("card-nope", make_card())


async def test_card_snapshots_are_deep_copied():
    """快照要冻住当时的样子，之后再改同一个对象不能污染历史。"""
    p = FakePlatform()
    card = make_card()
    sent = await p.send_card("oc_1", None, card)
    card.items[0].state = "done"
    assert p.card_snapshots(sent.card_id)[0].items[0].state == "todo"


async def test_send_file_records_size_and_keeps_bytes():
    p = FakePlatform()
    await p.send_file(
        OutboundFile(chat_id="oc_1", reply_to="om_1", name="out.png", mime="image/png", data=PNG_1X1)
    )
    assert p.count("send_file") == 1
    assert p.calls.last("send_file").kwargs["size"] == len(PNG_1X1)
    assert p.sent_files[0].data[:8] == PNG_MAGIC


async def test_read_history_does_not_filter_sender_kind():
    """契约注释写死：过滤归 Gateway，adapter/平台不过滤。"""
    p = FakePlatform(
        history=[
            history_message("om_1", "人说的"),
            history_message("om_2", "机器人播报", sender_kind="bot"),
        ]
    )
    rows = await p.read_history("oc_1")
    assert [r.sender_kind for r in rows] == ["human", "bot"]


async def test_read_history_is_time_ordered_and_limited():
    p = FakePlatform(history=[history_message(f"om_{i}", f"第{i}条") for i in range(5)])
    rows = await p.read_history("oc_1", limit=2)
    assert [r.message_id for r in rows] == ["om_3", "om_4"]


async def test_read_history_filters_by_thread():
    p = FakePlatform(
        history=[
            history_message("om_1", "顶层"),
            history_message("om_2", "话题里", thread_id="om_1"),
        ]
    )
    rows = await p.read_history("oc_1", thread_id="om_1")
    assert [r.message_id for r in rows] == ["om_2"]


async def test_missing_document_and_file_raise_with_hint():
    p = FakePlatform(documents={"k": DocumentContent(title="T", text="", url="u")})
    assert (await p.read_document("k")).title == "T"
    with pytest.raises(FakePlatformError, match="没有这篇文档"):
        await p.read_document("nope")
    with pytest.raises(FakePlatformError, match="没有这个附件"):
        await p.download_file("om_1", "fk")


async def test_download_file_returns_bytes():
    p = FakePlatform(files={("om_1", "fk"): b"csv,data"})
    assert await p.download_file("om_1", "fk") == b"csv,data"


async def test_fail_next_injects_one_failure():
    """§3.3 的失败面要能演出来，且只演一次。"""
    p = FakePlatform()
    p.fail_next["send_text"] = RuntimeError("平台 500")
    with pytest.raises(RuntimeError, match="平台 500"):
        await p.send_text(OutboundText(chat_id="oc_1", text="x"))
    await p.send_text(OutboundText(chat_id="oc_1", text="x"))   # 第二次恢复正常


async def test_outbound_count_covers_all_five_outbound_methods():
    p = FakePlatform()
    await p.send_text(OutboundText(chat_id="oc_1", text="a"))
    sent = await p.send_card("oc_1", None, make_card())
    await p.update_card(sent.card_id, make_card())
    await p.send_file(
        OutboundFile(chat_id="oc_1", reply_to=None, name="f", mime="text/plain", data=b"x")
    )
    await p.add_reaction("om_1", "ack")
    assert p.outbound_count == 5


async def test_emit_requires_start():
    """没接上 on_event 就投事件 = 测试写错了，要当场说清楚。"""
    p = FakePlatform()
    with pytest.raises(FakePlatformError, match="还没有 start"):
        await p.emit(None)

    seen = []

    async def on_event(ev):
        seen.append(ev)

    await p.start(on_event)
    assert p.started is True and p.stopped is False
    await p.stop()
    assert p.stopped is True
