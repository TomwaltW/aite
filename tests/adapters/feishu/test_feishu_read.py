"""读取面：群历史、云文档、消息附件下载（§3.2 的后三个方法）。

`read_history` 有两条硬要求，都在这里钉住：
按时间**正序**返回；**不做 sender_kind 过滤**（过滤是 T3 `read_group_history` 的活）。
"""
from __future__ import annotations

import json

import httpx
import respx

from aite.adapters.feishu import FeishuApiClient, FeishuPlatform

DOMAIN = "https://open.feishu.cn"
CHAT_ID = "oc_chat_p0_demo_0001"
ROOT_MESSAGE_ID = "om_toplevel_0001"

URL_TOKEN = f"{DOMAIN}/open-apis/auth/v3/tenant_access_token/internal"
URL_MESSAGES = f"{DOMAIN}/open-apis/im/v1/messages"
URL_WIKI_NODE = f"{DOMAIN}/open-apis/wiki/v2/spaces/get_node"

# 2026-09-09T01:02:00Z 起，每条 +1 分钟。
T0 = 1788915720000


def make_platform() -> FeishuPlatform:
    api = FeishuApiClient(app_id="cli", app_secret="s", domain=DOMAIN)
    return FeishuPlatform(bot_open_id="ou_aite_bot_0000000000000000000001", api=api)


def mock_token(router: respx.Router) -> None:
    router.post(URL_TOKEN).mock(
        return_value=httpx.Response(
            200, json={"code": 0, "tenant_access_token": "t-fake", "expire": 7200}
        )
    )


def history_item(
    message_id: str,
    text: str,
    *,
    created_at: int,
    sender_type: str = "user",
    sender_name: str | None = "张三",
    root_id: str | None = None,
) -> dict:
    item: dict = {
        "message_id": message_id,
        "msg_type": "text",
        "create_time": str(created_at),
        "chat_id": CHAT_ID,
        "sender": {
            "id": "ou_" + message_id,
            "id_type": "open_id",
            "sender_type": sender_type,
            "sender_name": sender_name,
        },
        "body": {"content": json.dumps({"text": text}, ensure_ascii=False)},
    }
    if root_id:
        item["root_id"] = root_id
    return item


def page(items: list[dict], *, has_more: bool = False, page_token: str = "") -> httpx.Response:
    return httpx.Response(
        200,
        json={"code": 0, "data": {"items": items, "has_more": has_more, "page_token": page_token}},
    )


# ---------------------------------------------------------------------------
# read_history
# ---------------------------------------------------------------------------

async def test_history_is_returned_oldest_first() -> None:
    """§3.2：按时间正序返回。平台给的是倒序，adapter 负责翻回来。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        # 平台按 ByCreateTimeDesc 返回：最新的在最前面。
        router.get(URL_MESSAGES).mock(
            return_value=page([
                history_item("om_c", "第三条", created_at=T0 + 120_000),
                history_item("om_b", "第二条", created_at=T0 + 60_000),
                history_item("om_a", "第一条", created_at=T0),
            ])
        )

        messages = await make_platform().read_history(CHAT_ID, limit=3)

        assert [m.text for m in messages] == ["第一条", "第二条", "第三条"]
        assert [m.created_at for m in messages] == sorted(m.created_at for m in messages)


async def test_history_asks_for_the_most_recent_window() -> None:
    """W1 要的是「最近 N 条」，所以拉取必须用倒序 + 带上发送者名字。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.get(URL_MESSAGES).mock(return_value=page([]))

        await make_platform().read_history(CHAT_ID, limit=50)

        params = route.calls.last.request.url.params
        assert params["container_id_type"] == "chat"
        assert params["container_id"] == CHAT_ID
        assert params["sort_type"] == "ByCreateTimeDesc"
        assert params["with_sender_name"] == "true"


async def test_history_does_not_filter_by_sender_kind() -> None:
    """§3.2：不做 sender_kind 过滤 —— 过滤归 T3 的 read_group_history 工具。

    05_history_summary 那个场景就是「工具拿到含 bot 的历史，自己滤干净」，
    adapter 这层先滤掉的话，T3 就没东西可滤、那条场景也验不出东西。
    """
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        router.get(URL_MESSAGES).mock(
            return_value=page([
                history_item("om_bot", "日报机器人播报", created_at=T0 + 60_000,
                             sender_type="app", sender_name="日报机器人"),
                history_item("om_human", "收到", created_at=T0),
            ])
        )

        messages = await make_platform().read_history(CHAT_ID, limit=10)

        assert [m.sender_kind for m in messages] == ["human", "app"]
        assert len(messages) == 2, "机器人消息也要原样返回"


async def test_history_carries_sender_name_and_thread_id() -> None:
    """W1 的上下文格式是 `[message_id] 姓名: 文本`，姓名要真拿得到。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        router.get(URL_MESSAGES).mock(
            return_value=page([
                history_item("om_x", "在话题里说的", created_at=T0, root_id=ROOT_MESSAGE_ID)
            ])
        )

        message = (await make_platform().read_history(CHAT_ID, limit=10))[0]

        assert message.sender_name == "张三"
        assert message.thread_id == ROOT_MESSAGE_ID
        assert message.message_id == "om_x"


async def test_history_can_be_narrowed_to_one_thread() -> None:
    """thread_id 给了就只留这条话题里的消息。

    飞书的 container_id_type=thread 收的是 `omt_` 话题 id，而锚点里存的是话题 root
    **消息** id（§3.1 Anchor 注释），两者不是一个 id 空间，所以在客户端筛。
    """
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        router.get(URL_MESSAGES).mock(
            return_value=page([
                history_item("om_other", "别的话题", created_at=T0 + 120_000),
                history_item("om_in", "本话题里的", created_at=T0 + 60_000, root_id=ROOT_MESSAGE_ID),
                history_item(ROOT_MESSAGE_ID, "话题根消息", created_at=T0),
            ])
        )

        messages = await make_platform().read_history(
            CHAT_ID, limit=10, thread_id=ROOT_MESSAGE_ID
        )

        assert [m.message_id for m in messages] == [ROOT_MESSAGE_ID, "om_in"]


async def test_history_paginates_until_the_limit_is_filled() -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        pages = [
            page([history_item(f"om_{i}", f"第 {i} 条", created_at=T0 + i * 1000) for i in range(60, 10, -1)],
                 has_more=True, page_token="pt2"),
            page([history_item(f"om_{i}", f"第 {i} 条", created_at=T0 + i * 1000) for i in range(10, 0, -1)]),
        ]
        route = router.get(URL_MESSAGES).mock(side_effect=pages)

        messages = await make_platform().read_history(CHAT_ID, limit=55)

        assert route.call_count == 2
        assert route.calls[1].request.url.params["page_token"] == "pt2"
        assert len(messages) == 55
        assert messages[0].created_at < messages[-1].created_at


async def test_history_limit_zero_makes_no_request() -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.get(URL_MESSAGES).mock(return_value=page([]))

        assert await make_platform().read_history(CHAT_ID, limit=0) == []
        assert route.call_count == 0


async def test_history_non_text_messages_have_empty_text() -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        item = history_item("om_file", "", created_at=T0)
        item["msg_type"] = "file"
        item["body"] = {"content": '{"file_key":"file_v2_x","file_name":"a.csv"}'}
        router.get(URL_MESSAGES).mock(return_value=page([item]))

        assert (await make_platform().read_history(CHAT_ID, limit=5))[0].text == ""


# ---------------------------------------------------------------------------
# read_document
# ---------------------------------------------------------------------------

async def test_read_docx_document() -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        meta = router.get(f"{DOMAIN}/open-apis/docx/v1/documents/DocTokenAbc123").mock(
            return_value=httpx.Response(
                200, json={"code": 0, "data": {"document": {"title": "指标字典"}}}
            )
        )
        raw = router.get(f"{DOMAIN}/open-apis/docx/v1/documents/DocTokenAbc123/raw_content").mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"content": "GMV 口径：…"}})
        )

        doc = await make_platform().read_document("https://example.feishu.cn/docx/DocTokenAbc123")

        assert meta.called and raw.called
        assert doc.title == "指标字典"
        assert doc.text == "GMV 口径：…"
        assert doc.url == "https://example.feishu.cn/docx/DocTokenAbc123"


async def test_read_wiki_document_resolves_to_its_docx() -> None:
    """wiki 链接先换成它挂的 docx token 再读。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        node = router.get(URL_WIKI_NODE).mock(
            return_value=httpx.Response(
                200,
                json={"code": 0, "data": {"node": {"obj_token": "DocReal999", "obj_type": "docx"}}},
            )
        )
        router.get(f"{DOMAIN}/open-apis/docx/v1/documents/DocReal999").mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"document": {"title": "周会纪要"}}})
        )
        router.get(f"{DOMAIN}/open-apis/docx/v1/documents/DocReal999/raw_content").mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"content": "正文"}})
        )

        doc = await make_platform().read_document("https://example.feishu.cn/wiki/WikiToken777")

        assert node.calls.last.request.url.params["token"] == "WikiToken777"
        assert doc.title == "周会纪要"


async def test_read_document_accepts_a_bare_token() -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        router.get(f"{DOMAIN}/open-apis/docx/v1/documents/DocTokenAbc123").mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"document": {"title": "T"}}})
        )
        router.get(f"{DOMAIN}/open-apis/docx/v1/documents/DocTokenAbc123/raw_content").mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"content": "c"}})
        )

        assert (await make_platform().read_document("DocTokenAbc123")).title == "T"


# ---------------------------------------------------------------------------
# download_file
# ---------------------------------------------------------------------------

async def test_download_file_uses_type_file() -> None:
    key = "file_v2_0a1b2c3d"
    url = f"{DOMAIN}/open-apis/im/v1/messages/om_with_file_0005/resources/{key}"
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.get(url).mock(return_value=httpx.Response(200, content=b"a,b\n1,2\n"))

        data = await make_platform().download_file("om_with_file_0005", key)

        assert data == b"a,b\n1,2\n"
        assert route.calls.last.request.url.params["type"] == "file"


async def test_download_image_uses_type_image() -> None:
    key = "img_v3_0a1b2c3d"
    url = f"{DOMAIN}/open-apis/im/v1/messages/om_post_0007/resources/{key}"
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.get(url).mock(return_value=httpx.Response(200, content=b"\x89PNG\r\n\x1a\n"))

        data = await make_platform().download_file("om_post_0007", key)

        assert data[:4] == b"\x89PNG"
        assert route.calls.last.request.url.params["type"] == "image"
