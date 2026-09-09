"""出站：文本 / 卡片 / 文件 / 表情（§6 T1 的「出站测试」那一条）。

最关键的一条是 `update_card` 必须走「更新消息」而不是「发送消息」——
用 respx 直接断言 HTTP 方法与路径，顺带断言「发送消息」的两条路由一次都没被碰过。
W4 的「过程中卡片至少更新 3 次且不新增消息」就压在这上面。
"""
from __future__ import annotations

import json

import httpx
import pytest
import respx

from aite.adapters.feishu import FeishuApiClient, FeishuPlatform
from aite.contracts import OutboundFile, OutboundText

DOMAIN = "https://open.feishu.cn"
CHAT_ID = "oc_chat_p0_demo_0001"
ROOT_MESSAGE_ID = "om_toplevel_0001"
CARD_MESSAGE_ID = "om_checklist_card_0001"

URL_TOKEN = f"{DOMAIN}/open-apis/auth/v3/tenant_access_token/internal"
URL_MESSAGES = f"{DOMAIN}/open-apis/im/v1/messages"
URL_REPLY = f"{DOMAIN}/open-apis/im/v1/messages/{ROOT_MESSAGE_ID}/reply"
URL_PATCH_CARD = f"{DOMAIN}/open-apis/im/v1/messages/{CARD_MESSAGE_ID}"
URL_REACTIONS = f"{DOMAIN}/open-apis/im/v1/messages/{ROOT_MESSAGE_ID}/reactions"
URL_FILES = f"{DOMAIN}/open-apis/im/v1/files"
URL_IMAGES = f"{DOMAIN}/open-apis/im/v1/images"


async def _never_sleep(seconds: float) -> None:
    """出站测试不该踩到退避；真踩到了就让它响。"""
    raise AssertionError(f"不该等待 {seconds}s —— 出站路径上没有重试才对")


def make_platform(**kwargs) -> FeishuPlatform:
    api = FeishuApiClient(
        app_id="cli_a1b2c3d4e5f60123",
        app_secret="secret",
        domain=DOMAIN,
        sleep=_never_sleep,
        **kwargs,
    )
    return FeishuPlatform(
        app_id="cli_a1b2c3d4e5f60123",
        bot_open_id="ou_aite_bot_0000000000000000000001",
        api=api,
    )


def mock_token(router: respx.Router) -> None:
    router.post(URL_TOKEN).mock(
        return_value=httpx.Response(
            200, json={"code": 0, "msg": "ok", "tenant_access_token": "t-fake", "expire": 7200}
        )
    )


def ok(message_id: str = "om_sent_0001") -> httpx.Response:
    return httpx.Response(200, json={"code": 0, "msg": "ok", "data": {"message_id": message_id}})


# ---------------------------------------------------------------------------
# update_card：更新消息 ≠ 发送消息
# ---------------------------------------------------------------------------

async def test_update_card_patches_the_same_message_and_never_sends_a_new_one(checklist_card) -> None:
    """§3.2：`update_card` 必须原地更新同一条消息，绝不新发消息。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        patch_route = router.patch(URL_PATCH_CARD).mock(
            return_value=httpx.Response(200, json={"code": 0, "msg": "ok", "data": {}})
        )
        create_route = router.post(URL_MESSAGES).mock(return_value=ok())
        reply_route = router.post(URL_REPLY).mock(return_value=ok())

        platform = make_platform()
        await platform.update_card(CARD_MESSAGE_ID, checklist_card)

        # —— 判据：方法是 PATCH，路径是那条卡片自己 ——
        assert patch_route.called
        request = patch_route.calls.last.request
        assert request.method == "PATCH"
        assert request.url.path == f"/open-apis/im/v1/messages/{CARD_MESSAGE_ID}"
        # —— 判据：两条「发送消息」的路由一次都没碰 ——
        assert not create_route.called, "update_card 退化成发新消息了"
        assert not reply_route.called, "update_card 退化成回复新消息了"

        body = json.loads(request.content)
        assert set(body) == {"content"}, "更新卡片只带 content，别把 receive_id 之类捎上"
        card_json = json.loads(body["content"])
        assert card_json["config"]["update_multi"] is True


async def test_repeated_updates_never_grow_the_message_count(checklist_card) -> None:
    """连更 3 次（B3 / M3 的形态）：PATCH 3 次，发送消息仍然 0 次。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        patch_route = router.patch(URL_PATCH_CARD).mock(
            return_value=httpx.Response(200, json={"code": 0, "msg": "ok", "data": {}})
        )
        create_route = router.post(URL_MESSAGES).mock(return_value=ok())

        platform = make_platform()
        for state in ("todo", "doing", "done"):
            checklist_card.items[0].state = state
            await platform.update_card(CARD_MESSAGE_ID, checklist_card)

        assert patch_route.call_count == 3
        assert create_route.call_count == 0
        assert {call.request.method for call in patch_route.calls} == {"PATCH"}


# ---------------------------------------------------------------------------
# send_card / send_text
# ---------------------------------------------------------------------------

async def test_send_card_into_thread_uses_reply_endpoint(checklist_card) -> None:
    """有 reply_to 就走「回复消息」并带 reply_in_thread，卡片才落在话题里（附录 A）。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        reply_route = router.post(URL_REPLY).mock(return_value=ok(CARD_MESSAGE_ID))

        platform = make_platform()
        result = await platform.send_card(CHAT_ID, ROOT_MESSAGE_ID, checklist_card)

        request = reply_route.calls.last.request
        assert request.method == "POST"
        assert request.url.path == f"/open-apis/im/v1/messages/{ROOT_MESSAGE_ID}/reply"
        body = json.loads(request.content)
        assert body["msg_type"] == "interactive"
        assert body["reply_in_thread"] is True
        # card_id 要能直接喂给 update_card（§3.1 SendResult 的注释）
        assert result.message_id == CARD_MESSAGE_ID
        assert result.card_id == CARD_MESSAGE_ID


async def test_send_card_without_reply_to_uses_create_endpoint(checklist_card) -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        create_route = router.post(URL_MESSAGES).mock(return_value=ok(CARD_MESSAGE_ID))

        platform = make_platform()
        await platform.send_card(CHAT_ID, None, checklist_card)

        request = create_route.calls.last.request
        assert request.method == "POST"
        assert request.url.path == "/open-apis/im/v1/messages"
        assert request.url.params["receive_id_type"] == "chat_id"
        body = json.loads(request.content)
        assert body["receive_id"] == CHAT_ID
        assert body["msg_type"] == "interactive"


async def test_send_text_carries_markdown(checklist_card) -> None:
    """`OutboundText.text` 是 markdown，出站要转成飞书能渲染的载体（§3.1 注释）。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        reply_route = router.post(URL_REPLY).mock(return_value=ok())

        platform = make_platform()
        await platform.send_text(
            OutboundText(chat_id=CHAT_ID, text="**已完成**\n- 图见附件", reply_to=ROOT_MESSAGE_ID)
        )

        body = json.loads(reply_route.calls.last.request.content)
        card = json.loads(body["content"])
        assert card["elements"] == [{"tag": "markdown", "content": "**已完成**\n- 图见附件"}]


async def test_send_text_respects_in_thread_false() -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        reply_route = router.post(URL_REPLY).mock(return_value=ok())

        platform = make_platform()
        await platform.send_text(
            OutboundText(chat_id=CHAT_ID, text="hi", reply_to=ROOT_MESSAGE_ID, in_thread=False)
        )

        assert json.loads(reply_route.calls.last.request.content)["reply_in_thread"] is False


# ---------------------------------------------------------------------------
# send_file
# ---------------------------------------------------------------------------

async def test_send_file_uploads_image_then_sends_it() -> None:
    """PNG 走「上传图片」→ msg_type=image（M3 的产物图要能内联显示）。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        upload = router.post(URL_IMAGES).mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"image_key": "img_v3_new"}})
        )
        reply_route = router.post(URL_REPLY).mock(return_value=ok())

        platform = make_platform()
        await platform.send_file(
            OutboundFile(
                chat_id=CHAT_ID, reply_to=ROOT_MESSAGE_ID,
                name="trend.png", mime="image/png", data=b"\x89PNG\r\n\x1a\n",
            )
        )

        assert upload.called
        assert upload.calls.last.request.url.path == "/open-apis/im/v1/images"
        body = json.loads(reply_route.calls.last.request.content)
        assert body["msg_type"] == "image"
        assert json.loads(body["content"]) == {"image_key": "img_v3_new"}


async def test_send_file_uploads_other_files_as_stream() -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        upload = router.post(URL_FILES).mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"file_key": "file_v2_new"}})
        )
        reply_route = router.post(URL_REPLY).mock(return_value=ok())

        platform = make_platform()
        await platform.send_file(
            OutboundFile(
                chat_id=CHAT_ID, reply_to=ROOT_MESSAGE_ID,
                name="report.csv", mime="text/csv", data=b"a,b\n1,2\n",
            )
        )

        assert upload.calls.last.request.url.path == "/open-apis/im/v1/files"
        body = json.loads(reply_route.calls.last.request.content)
        assert body["msg_type"] == "file"
        assert json.loads(body["content"]) == {"file_key": "file_v2_new"}


# ---------------------------------------------------------------------------
# add_reaction
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("kind", ["ack", "done", "fail"])
async def test_add_reaction_posts_an_emoji(kind: str) -> None:
    """M1：@ 之后 2 秒内出现表情回应。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.post(URL_REACTIONS).mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {}})
        )

        platform = make_platform()
        await platform.add_reaction(ROOT_MESSAGE_ID, kind)

        request = route.calls.last.request
        assert request.method == "POST"
        assert request.url.path == f"/open-apis/im/v1/messages/{ROOT_MESSAGE_ID}/reactions"
        assert json.loads(request.content)["reaction_type"]["emoji_type"]


# ---------------------------------------------------------------------------
# 出站限速
# ---------------------------------------------------------------------------

async def test_outbound_is_rate_limited_by_the_capability(checklist_card, fake_clock) -> None:
    """§3.1：出站按 `outbound_rate_per_min` 限速，adapter 自己令牌桶。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        router.patch(URL_PATCH_CARD).mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {}})
        )

        api = FeishuApiClient(
            app_id="cli", app_secret="s", domain=DOMAIN,
            rate_per_min=2, sleep=fake_clock.sleep, clock=fake_clock,
        )
        platform = FeishuPlatform(bot_open_id="ou_bot", api=api)

        # 桶容量 = 2：前两次不等，第三次要等 60/2 = 30s。
        for _ in range(3):
            await platform.update_card(CARD_MESSAGE_ID, checklist_card)

        assert fake_clock.slept == pytest.approx([30.0])


async def test_reads_do_not_consume_the_outbound_quota(fake_clock) -> None:
    """读接口（群历史/文档/下载）不占出站消息额度。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        router.get(URL_MESSAGES).mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"items": [], "has_more": False}})
        )

        api = FeishuApiClient(
            app_id="cli", app_secret="s", domain=DOMAIN,
            rate_per_min=1, sleep=fake_clock.sleep, clock=fake_clock,
        )
        platform = FeishuPlatform(bot_open_id="ou_bot", api=api)

        for _ in range(5):
            await platform.read_history(CHAT_ID, limit=10)

        assert fake_clock.slept == []
