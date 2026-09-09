"""ChecklistCard → 飞书卡片 JSON（§6 T1：「通过 SDK/Schema 构造不报错」）。

两头都验：

* **Schema** —— 卡片结构逐层查（config / header / elements / 按钮 value），
  外加附录 A 的「卡片 ≤30KB」。
* **SDK** —— 把卡片塞进 lark_oapi 生成的请求对象里构造一遍。顺带用 SDK 自己的
  `uri` / `http_method` 反证「更新消息」和「发送消息」是两个接口。
"""
from __future__ import annotations

import json

import pytest
from lark_oapi.api.im.v1 import (
    CreateMessageRequest,
    CreateMessageRequestBody,
    PatchMessageRequest,
    PatchMessageRequestBody,
)

from aite.adapters.feishu import CARD_MAX_BYTES, build_checklist_card, build_markdown_card, dumps_card
from aite.contracts import ChecklistCard, ChecklistItemView

VALID_TAGS = {"div", "hr", "note", "action", "markdown", "img"}


def check_card_schema(card: dict) -> None:
    """飞书 v1 卡片的最小 schema 检查。字段错了平台只会回一句 invalid params。"""
    assert isinstance(card, dict)
    assert set(card) <= {"config", "header", "elements", "i18n_elements", "card_link"}
    assert "elements" in card and isinstance(card["elements"], list) and card["elements"]

    config = card.get("config", {})
    assert isinstance(config, dict)
    # 不开 update_multi，update_card 只对第一个看到卡片的人生效。
    assert config.get("update_multi") is True

    if "header" in card:
        header = card["header"]
        assert header["title"]["tag"] == "plain_text"
        assert isinstance(header["title"]["content"], str)
        assert header["template"] in {"blue", "green", "red", "grey", "orange", "turquoise"}

    for element in card["elements"]:
        assert isinstance(element, dict), element
        tag = element.get("tag")
        assert tag in VALID_TAGS, f"未知元素 tag：{tag}"
        if tag == "div":
            assert "text" in element or "fields" in element
            for field in element.get("fields", []):
                assert isinstance(field["is_short"], bool)
                assert field["text"]["tag"] in {"plain_text", "lark_md"}
                assert isinstance(field["text"]["content"], str)
            if "text" in element:
                assert element["text"]["tag"] in {"plain_text", "lark_md"}
        elif tag == "note":
            assert element["elements"] and all(e["tag"] == "plain_text" for e in element["elements"])
        elif tag == "action":
            assert element["actions"]
            for action in element["actions"]:
                assert action["tag"] == "button"
                assert action["text"]["tag"] == "plain_text"
                assert action["type"] in {"default", "primary", "danger"}
                assert isinstance(action["value"], dict)
        elif tag == "markdown":
            assert isinstance(element["content"], str)


# ---------------------------------------------------------------------------
# Schema
# ---------------------------------------------------------------------------

def test_checklist_card_passes_schema(checklist_card: ChecklistCard) -> None:
    check_card_schema(build_checklist_card(checklist_card))


@pytest.mark.parametrize("status", ["working", "delivered", "failed", "cancelled"])
def test_every_status_builds(checklist_card: ChecklistCard, status: str) -> None:
    """四种任务状态都要能构造出合法卡片，且头部配色各不相同。"""
    checklist_card.status = status
    card = build_checklist_card(checklist_card)
    check_card_schema(card)
    assert card["header"]["template"]


def test_status_templates_are_distinct(checklist_card: ChecklistCard) -> None:
    templates = set()
    for status in ("working", "delivered", "failed", "cancelled"):
        checklist_card.status = status
        templates.add(build_checklist_card(checklist_card)["header"]["template"])
    assert len(templates) == 4, "四种状态的配色要能一眼分开"


@pytest.mark.parametrize("state", ["todo", "doing", "done", "failed"])
def test_every_item_state_renders(checklist_card: ChecklistCard, state: str) -> None:
    checklist_card.items = [ChecklistItemView(id="c1", text="读取 CSV", state=state)]
    card = build_checklist_card(checklist_card)
    check_card_schema(card)
    body = json.dumps(card, ensure_ascii=False)
    assert "读取 CSV" in body


def test_empty_checklist_still_builds(checklist_card: ChecklistCard) -> None:
    """卡片先发、待办后加（W3）：items 为空时也不能构造失败。"""
    checklist_card.items = []
    check_card_schema(build_checklist_card(checklist_card))


def test_item_note_is_rendered(checklist_card: ChecklistCard) -> None:
    body = json.dumps(build_checklist_card(checklist_card), ensure_ascii=False)
    assert "共 3 个 sheet" in body


def test_task_no_and_title_are_in_the_header(checklist_card: ChecklistCard) -> None:
    title = build_checklist_card(checklist_card)["header"]["title"]["content"]
    assert checklist_card.task_no in title
    assert checklist_card.title in title


def test_footer_becomes_a_note(checklist_card: ChecklistCard) -> None:
    notes = [e for e in build_checklist_card(checklist_card)["elements"] if e["tag"] == "note"]
    assert notes and notes[0]["elements"][0]["content"] == checklist_card.footer


# ---------------------------------------------------------------------------
# 按钮：发出去的 value 与点回来的 card_action 要对得上
# ---------------------------------------------------------------------------

def test_button_value_round_trips_into_card_action(checklist_card: ChecklistCard) -> None:
    """按钮 value 就是 normalize_card_action 读的字段，两头必须是一套口径。"""
    from aite.adapters.feishu import normalize_card_action

    actions = [e for e in build_checklist_card(checklist_card)["elements"] if e["tag"] == "action"]
    assert actions
    value = actions[0]["actions"][0]["value"]

    event = normalize_card_action(
        {
            "header": {"event_id": "evt_x", "create_time": "1788915720000",
                       "event_type": "card.action.trigger", "app_id": "cli_x"},
            "event": {
                "operator": {"open_id": "ou_zhang_san"},
                "action": {"value": value, "tag": "button"},
                "context": {"open_message_id": "om_card", "open_chat_id": "oc_chat"},
            },
        }
    )
    assert event is not None
    assert event.card_action is not None
    assert event.card_action.action == "stop"
    assert event.card_action.task_id == checklist_card.task_id


def test_actions_are_rendered_as_given(checklist_card: ChecklistCard) -> None:
    """按 card.actions 原样渲染 —— 留不留「停止」是 worker 的决定（W4），adapter 不替它判。"""
    checklist_card.actions = ["stop", "evidence"]
    actions = [e for e in build_checklist_card(checklist_card)["elements"] if e["tag"] == "action"]
    assert [a["value"]["action"] for a in actions[0]["actions"]] == ["stop", "evidence"]

    checklist_card.actions = []
    assert not [e for e in build_checklist_card(checklist_card)["elements"] if e["tag"] == "action"]


# ---------------------------------------------------------------------------
# 附录 A：卡片 ≤30KB
# ---------------------------------------------------------------------------

def test_oversized_card_is_trimmed_under_30kb(checklist_card: ChecklistCard) -> None:
    """待办项被模型灌爆时，宁可少显示几行，也不要整条 update_card 被平台打回来。"""
    checklist_card.items = [
        ChecklistItemView(id=f"c{i}", text="很长的一项" * 200, state="todo") for i in range(80)
    ]
    card = build_checklist_card(checklist_card)
    check_card_schema(card)

    size = len(dumps_card(card).encode("utf-8"))
    assert size <= CARD_MAX_BYTES, f"卡片 {size} 字节，超过 {CARD_MAX_BYTES}"
    assert "未显示" in dumps_card(card), "被裁掉的项要在卡片上说一声"


def test_normal_card_is_nowhere_near_the_limit(checklist_card: ChecklistCard) -> None:
    assert len(dumps_card(build_checklist_card(checklist_card)).encode("utf-8")) < 2000


# ---------------------------------------------------------------------------
# markdown 文本卡片
# ---------------------------------------------------------------------------

def test_markdown_card_passes_schema() -> None:
    check_card_schema(build_markdown_card("**已完成**\n- 见附件"))


def test_markdown_card_keeps_the_source_verbatim() -> None:
    text = "| a | b |\n| - | - |\n| 1 | 2 |"
    card = build_markdown_card(text)
    assert card["elements"][0]["content"] == text


# ---------------------------------------------------------------------------
# SDK 构造
# ---------------------------------------------------------------------------

def test_card_is_constructible_through_the_sdk(checklist_card: ChecklistCard) -> None:
    """把卡片喂进 lark_oapi 生成的请求对象，构造不报错。"""
    content = dumps_card(build_checklist_card(checklist_card))

    body = (
        CreateMessageRequestBody.builder()
        .receive_id("oc_chat_p0_demo_0001")
        .msg_type("interactive")
        .content(content)
        .build()
    )
    request = CreateMessageRequest.builder().receive_id_type("chat_id").request_body(body).build()

    assert request.uri == "/open-apis/im/v1/messages"
    assert json.loads(request.request_body.content)["header"]["title"]["tag"] == "plain_text"


def test_sdk_agrees_that_update_is_not_send(checklist_card: ChecklistCard) -> None:
    """SDK 自己也认：更新卡片是 PATCH `messages/:message_id`，发送是 POST `messages`。"""
    patch_body = (
        PatchMessageRequestBody.builder()
        .content(dumps_card(build_checklist_card(checklist_card)))
        .build()
    )
    patch = PatchMessageRequest.builder().message_id("om_card").request_body(patch_body).build()
    create = (
        CreateMessageRequest.builder()
        .receive_id_type("chat_id")
        .request_body(CreateMessageRequestBody.builder().build())
        .build()
    )

    assert patch.http_method.name == "PATCH"
    assert patch.uri == "/open-apis/im/v1/messages/:message_id"
    assert create.http_method.name == "POST"
    assert create.uri == "/open-apis/im/v1/messages"
    assert patch.uri != create.uri
