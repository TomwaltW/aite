"""B1（§2.2）：飞书原始事件 → NormalizedEvent，`x.json` 与 `x.expected.json` 逐字段一致。

两层判据，缺一不可：

1. **黄金文件**：`x.json` 跑一遍归一化，结果与 `x.expected.json` 逐字段比。
   它锁的是「以后别悄悄漂」，锁不住「今天就写错了」—— expected 是实现生成的。
2. **手写断言**：§6 T1 点名的 6 个 fixture，每个把关键字段用字面量写死在下面。
   这一层才是真判据；黄金文件只是它的全字段兜底。
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

from aite.adapters.feishu import normalize
from aite.adapters.feishu.normalize import to_datetime
from aite.contracts import EventKind, NormalizedEvent, SenderKind

FIXTURES_DIR = Path(__file__).resolve().parents[2] / "fixtures" / "feishu"

BOT_OPEN_ID = "ou_aite_bot_0000000000000000000001"
APP_ID = "cli_a1b2c3d4e5f60123"
CHAT_ID = "oc_chat_p0_demo_0001"
ROOT_MESSAGE_ID = "om_toplevel_0001"

#: §6 T1 点名必须有的 6 个。多的可以有，少一个就不算数。
REQUIRED_FIXTURES = [
    "message_at_bot_toplevel",
    "message_in_thread_no_at",
    "message_in_thread_with_at",
    "message_from_bot",
    "message_with_file",
    "card_action_stop",
]


def fixture_names() -> list[str]:
    return sorted(
        p.name[: -len(".json")]
        for p in FIXTURES_DIR.glob("*.json")
        if not p.name.endswith(".expected.json")
    )


def load(name: str, suffix: str = ".json") -> dict[str, Any]:
    return json.loads((FIXTURES_DIR / f"{name}{suffix}").read_text(encoding="utf-8"))


def normalized(name: str) -> NormalizedEvent:
    event = normalize(
        load(name), bot_open_id=BOT_OPEN_ID, workspace_id=APP_ID, tenant_id="default"
    )
    assert event is not None, f"{name} 应该能被归一化"
    return event


# ---------------------------------------------------------------------------
# fixture 清单本身
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("name", REQUIRED_FIXTURES)
def test_required_fixture_exists(name: str) -> None:
    """§6 T1 点名的 6 个 fixture 一个都不能少。"""
    assert (FIXTURES_DIR / f"{name}.json").is_file(), f"缺 fixture：{name}.json"


@pytest.mark.parametrize("name", fixture_names())
def test_every_fixture_has_expected(name: str) -> None:
    """每个 `x.json` 都要有配套的 `x.expected.json`。"""
    assert (FIXTURES_DIR / f"{name}.expected.json").is_file(), f"{name} 缺 .expected.json"


# ---------------------------------------------------------------------------
# 黄金文件：逐字段比
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("name", fixture_names())
def test_fixture_matches_expected_field_by_field(name: str) -> None:
    """`x.json` → NormalizedEvent 与 `x.expected.json` 逐字段一致。"""
    actual = normalized(name).model_dump(mode="json")
    expected = load(name, ".expected.json")

    assert set(actual) == set(expected), f"{name}: 字段集合对不上"
    for field in expected:
        assert actual[field] == expected[field], f"{name}: 字段 {field} 不一致"


@pytest.mark.parametrize("name", fixture_names())
def test_expected_round_trips_back_into_the_contract(name: str) -> None:
    """expected 里躺的必须是合法的 NormalizedEvent，不是一坨随手写的 JSON。"""
    assert NormalizedEvent.model_validate(load(name, ".expected.json"))


@pytest.mark.parametrize("name", fixture_names())
def test_raw_is_kept_verbatim(name: str) -> None:
    """§3.1：`raw` 原样存原始事件供审计。"""
    assert normalized(name).raw == load(name)


# ---------------------------------------------------------------------------
# 6 个 fixture 各自验什么（手写字面量）
# ---------------------------------------------------------------------------

def test_message_at_bot_toplevel() -> None:
    """群里顶层 @Aite —— R7 的入口。thread_id 必须是 None：本条自己才是话题 root。"""
    event = normalized("message_at_bot_toplevel")
    assert event.kind is EventKind.message
    assert event.sender_kind is SenderKind.human
    assert event.mentioned is True
    assert event.text == "把这个季度的销售数据画成趋势图"      # @Aite 剥掉并 strip 过
    assert event.raw_text == "@_user_1 把这个季度的销售数据画成趋势图"
    assert event.anchor.thread_id is None
    assert event.anchor.message_id == ROOT_MESSAGE_ID
    assert event.chat_type == "group"
    assert event.workspace_id == APP_ID                        # 飞书 app_id
    assert event.event_id == "evt_at_bot_toplevel_0001"


def test_message_in_thread_no_at() -> None:
    """话题里不带 @ 的追问 —— R6 靠 thread_id 续接，不要求 mentioned。"""
    event = normalized("message_in_thread_no_at")
    assert event.mentioned is False
    assert event.anchor.thread_id == ROOT_MESSAGE_ID
    assert event.text == "再按季度画一张"


def test_message_in_thread_with_at() -> None:
    """话题里带 @：root_id 优先于 parent_id / thread_id；别人的 @ 换成人名。"""
    raw = load("message_in_thread_with_at")["event"]["message"]
    assert raw["root_id"] != raw["parent_id"] != raw["thread_id"], "这条 fixture 就是要三者都不同"

    event = normalized("message_in_thread_with_at")
    assert event.mentioned is True
    assert event.anchor.thread_id == ROOT_MESSAGE_ID           # 不是 parent_id，也不是 omt_
    assert event.text == "顺便把 @李四 上周给的口径也对一下"
    assert "@_user_" not in event.text                          # 占位符不能漏给模型


def test_message_from_bot() -> None:
    """机器人发的消息 —— 即使 @ 了 Aite，sender_kind 也必须不是 human（R1 靠它丢弃）。"""
    event = normalized("message_from_bot")
    assert event.sender_kind in (SenderKind.bot, SenderKind.app)
    assert event.sender_kind is not SenderKind.human
    assert event.mentioned is True, "它确实 @ 了；R1 该看 sender_kind 而不是 mentioned"


def test_message_with_file() -> None:
    """带文件的消息：text 为 ""，附件进 attachments（M3 的 CSV 走这条）。"""
    event = normalized("message_with_file")
    assert event.text == ""                                     # §3.1：非文本消息为 ""
    assert event.raw_text is None
    assert len(event.attachments) == 1
    attachment = event.attachments[0]
    assert attachment.kind == "file"
    assert attachment.file_key.startswith("file_v2_")
    assert attachment.name == "2026Q3-sales.csv"
    assert attachment.size == 20480                             # 平台给的是字符串，这里已转 int
    assert attachment.message_id == event.anchor.message_id     # 配 message_id 才能下载


def test_card_action_stop() -> None:
    """卡片 stop 按钮回传 —— R3 的入口。card_id 就是卡片所在消息的 message_id。"""
    event = normalized("card_action_stop")
    assert event.kind is EventKind.card_action
    assert event.card_action is not None
    assert event.card_action.action == "stop"
    assert event.card_action.card_id == "om_checklist_card_0001"
    assert event.card_action.card_id == event.anchor.message_id
    assert event.card_action.task_id == "b7c1e6f0-1111-4222-8333-444455556666"
    assert event.sender_kind is SenderKind.human                # 点按钮的一定是真人
    assert event.text == ""
    assert event.chat_id == CHAT_ID


# ---------------------------------------------------------------------------
# 额外覆盖
# ---------------------------------------------------------------------------

def test_post_message_is_flattened_and_image_becomes_attachment() -> None:
    """富文本：正文拍平进 text，post 里的 `at` 段也算 @，内嵌图片进 attachments。"""
    event = normalized("message_post_with_image")
    assert event.mentioned is True                              # at 段 @ 的是机器人
    assert "@Aite" not in event.text
    assert event.text.startswith("本周复盘")
    assert "[指标字典](https://example.feishu.cn/docx/DocTokenAbc123)" in event.text
    assert [a.kind for a in event.attachments] == ["image"]


def test_unsubscribed_event_type_is_ignored() -> None:
    """P0 只订阅两类事件；别的一律返回 None，而不是硬凑一个 EventKind 出来。"""
    raw = load("message_at_bot_toplevel")
    raw["header"]["event_type"] = "im.message.message_read_v1"
    assert normalize(raw, bot_open_id=BOT_OPEN_ID) is None


def test_card_action_with_unknown_action_is_ignored() -> None:
    """`CardAction.action` 是 Literal["stop","evidence"]，别的按钮不该硬塞进契约。"""
    raw = load("card_action_stop")
    raw["event"]["action"]["value"]["action"] = "rerun"
    assert normalize(raw, bot_open_id=BOT_OPEN_ID) is None


def test_adapter_does_not_deduplicate() -> None:
    """§3.3：去重是 ControlPlane 的活，adapter 同一条投两次要给出两个等价事件。"""
    first = normalized("message_at_bot_toplevel")
    second = normalized("message_at_bot_toplevel")
    assert first.event_id == second.event_id
    assert first.model_dump() == second.model_dump()


def test_at_someone_else_only_is_not_mentioned() -> None:
    """只 @ 了别人 → mentioned 为 False，且文本里换成人名。"""
    raw = load("message_at_bot_toplevel")
    message = raw["event"]["message"]
    message["content"] = json.dumps({"text": "@_user_2 你看下"}, ensure_ascii=False)
    message["mentions"] = [
        {
            "key": "@_user_2",
            "id": {"open_id": "ou_li_si_000000000000000000000002"},
            "name": "李四",
        }
    ]
    event = normalize(raw, bot_open_id=BOT_OPEN_ID)
    assert event is not None
    assert event.mentioned is False
    assert event.text == "@李四 你看下"


# ---------------------------------------------------------------------------
# T16：拿官方文档核对出来的三条
# ---------------------------------------------------------------------------

@pytest.mark.parametrize(
    ("ticks", "why"),
    [
        ("1788916020000", "im.message.receive_v1 的例子给的是 13 位毫秒"),
        ("1788916020000000", "事件订阅概述与 card.action.trigger 的例子给的是 16 位微秒"),
        (1788916020000, "整数形式的毫秒"),
        (1788916020000000, "整数形式的微秒"),
    ],
)
def test_timestamp_unit_is_detected_by_magnitude(ticks: object, why: str) -> None:
    """毫秒和微秒都要落到同一个时刻 —— 官方文档两种单位都出现过（见 to_datetime）。"""
    parsed = to_datetime(ticks)
    assert parsed is not None, why
    assert parsed.isoformat() == "2026-09-09T01:07:00+00:00", why


def test_card_action_survives_a_microsecond_timestamp() -> None:
    """card.action.trigger 的 header.create_time 是微秒。

    当成毫秒除会得到五万年后的秒数，`fromtimestamp` 当场抛 —— 而 `_dispatch_raw`
    没有兜底，`!stop` / `证据` 按钮会整条丢掉。这条就是钉住它不许回去。
    """
    raw = load("card_action_stop")
    assert raw["header"]["create_time"] == "1788916020000000", "fixture 该是 16 位微秒"

    event = normalize(raw, bot_open_id=BOT_OPEN_ID)
    assert event is not None
    assert event.occurred_at.year == 2026
    assert event.occurred_at.isoformat() == "2026-09-09T01:07:00+00:00"


def test_post_at_segment_carries_a_placeholder_not_an_open_id() -> None:
    """post 的 at 段里 user_id 是 `@_user_N` 序号，身份要回 mentions 里查。

    https://open.feishu.cn/document/server-docs/im-v1/message-content-description/message_content
    照 open_id 直接比永远不相等，@ 会被漏判成 False。
    """
    raw = load("message_post_with_image")
    at = json.loads(raw["event"]["message"]["content"])["content"][0][0]
    assert at["tag"] == "at"
    assert at["user_id"] == "@_user_1", "fixture 要按文档写成占位序号"

    event = normalize(raw, bot_open_id=BOT_OPEN_ID)
    assert event is not None
    assert event.mentioned is True                              # 靠 mentions 认出来的
    assert "@_user_" not in event.text                          # 占位符不能漏给模型
    assert "@_user_1" in (event.raw_text or "")                 # raw_text 与 text 类消息同口径


def test_post_at_placeholder_without_mentions_cannot_identify_anyone() -> None:
    """占位序号 + mentions 缺失 = 查无此人。不能炸，也不能把占位符漏给模型。"""
    raw = load("message_post_with_image")
    raw["event"]["message"].pop("mentions")

    event = normalize(raw, bot_open_id=BOT_OPEN_ID)
    assert event is not None
    assert event.mentioned is False
    assert "@_user_" not in event.text


def test_post_at_with_a_raw_open_id_is_still_recognised() -> None:
    """兜底分支：某个客户端真在 at 段塞了 open_id（文档说不该有），也得认出来。

    这条走的是 `_post_mentions_bot` —— mentions 里没有机器人时它才有机会开口。
    """
    raw = load("message_post_with_image")
    message = raw["event"]["message"]
    message.pop("mentions")
    content = json.loads(message["content"])
    content["content"][0][0]["user_id"] = BOT_OPEN_ID          # 老形状：直接是 open_id
    message["content"] = json.dumps(content, ensure_ascii=False)

    event = normalize(raw, bot_open_id=BOT_OPEN_ID)
    assert event is not None
    assert event.mentioned is True
    assert BOT_OPEN_ID not in event.text                        # 别把 open_id 吐给模型
    assert event.text.startswith("本周复盘")
