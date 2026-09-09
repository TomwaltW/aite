"""`ChecklistCard`（§3.1）→ 飞书消息卡片 JSON。

用 v1 卡片（`config` / `header` / `elements`）。两个点是硬要求：

* `config.update_multi = true` —— 不开这个，「更新应用发送的消息卡片」只对第一个
  看到卡片的人生效，群里其他人看到的还是旧卡片。W4 的「原地更新」就废了。
* 按钮 `value` 里带 `{"action": ..., "task_id": ...}` —— 这正是
  `normalize.normalize_card_action()` 读的字段，卡片发出去和点回来是同一套口径。

附录 A：卡片 ≤30KB。这里在序列化后兜一道，超了就从尾部丢待办项，
宁可少显示几行，也不要整条 update_card 被平台打回来。
"""
from __future__ import annotations

import json
from typing import Any

from ...contracts import ChecklistCard

#: 附录 A：卡片 ≤30KB。留点余量给平台自己包的信封。
CARD_MAX_BYTES = 30_000

#: 任务状态 → 卡片头部配色。
STATUS_TEMPLATE = {
    "working": "blue",
    "delivered": "green",
    "failed": "red",
    "cancelled": "grey",
}

STATUS_LABEL = {
    "working": "进行中",
    "delivered": "已交付",
    "failed": "失败",
    "cancelled": "已取消",
}

#: checklist 单项状态 → 行首图标。
STATE_ICON = {"todo": "⬜", "doing": "🔄", "done": "✅", "failed": "❌"}

#: 卡片按钮：action 名 → (按钮文案, 按钮样式)。
ACTION_BUTTON = {
    "stop": ("停止", "danger"),
    "evidence": ("证据", "default"),
}


def _plain_text(content: str) -> dict[str, Any]:
    return {"tag": "plain_text", "content": content}


def _lark_md(content: str) -> dict[str, Any]:
    return {"tag": "lark_md", "content": content}


def _item_lines(card: ChecklistCard) -> list[str]:
    lines = []
    for item in card.items:
        icon = STATE_ICON.get(item.state, "⬜")
        line = f"{icon} {item.text}"
        if item.note:
            line += f"　—— {item.note}"
        lines.append(line)
    return lines


def _elements(card: ChecklistCard, item_lines: list[str], dropped: int) -> list[dict[str, Any]]:
    elements: list[dict[str, Any]] = [
        {
            "tag": "div",
            "fields": [
                {"is_short": True, "text": _lark_md(f"**发起人**\n{card.initiator}")},
                {"is_short": True, "text": _lark_md(f"**开始于**\n{card.started_at}")},
                {
                    "is_short": True,
                    "text": _lark_md(f"**状态**\n{STATUS_LABEL.get(card.status, card.status)}"),
                },
            ],
        },
        {"tag": "hr"},
    ]

    body = "\n".join(item_lines) if item_lines else "_（还没有待办项）_"
    if dropped:
        body += f"\n…… 另有 {dropped} 项未显示"
    elements.append({"tag": "div", "text": _lark_md(body)})

    if card.footer:
        elements.append({"tag": "note", "elements": [_plain_text(card.footer)]})

    # 按 card.actions 原样渲染：什么时候还该留「停止」按钮是 worker 的决定（W4），
    # adapter 不替它判断。
    actions = [
        {
            "tag": "button",
            "text": _plain_text(ACTION_BUTTON[name][0]),
            "type": ACTION_BUTTON[name][1],
            "value": {"action": name, "task_id": card.task_id},
        }
        for name in card.actions
        if name in ACTION_BUTTON
    ]
    if actions:
        elements.append({"tag": "action", "actions": actions})

    return elements


def build_checklist_card(card: ChecklistCard) -> dict[str, Any]:
    """`ChecklistCard` → 飞书卡片 JSON（dict）。"""
    title = f"{card.task_no} {card.title}".strip()
    lines = _item_lines(card)
    dropped = 0

    while True:
        payload = {
            # update_multi 必须为 true，否则 update_card 只对单个用户生效。
            "config": {"wide_screen_mode": True, "update_multi": True},
            "header": {
                "template": STATUS_TEMPLATE.get(card.status, "blue"),
                "title": _plain_text(title),
            },
            "elements": _elements(card, lines, dropped),
        }
        if len(dumps_card(payload).encode("utf-8")) <= CARD_MAX_BYTES or not lines:
            return payload
        lines = lines[:-1]
        dropped += 1


def dumps_card(payload: dict[str, Any]) -> str:
    """卡片 JSON → 发送用的字符串。飞书的 `content` 收的是字符串而不是对象。"""
    return json.dumps(payload, ensure_ascii=False, separators=(",", ":"))


def build_markdown_card(text: str) -> dict[str, Any]:
    """一段 markdown → 只有一个 markdown 元素的卡片。

    `OutboundText.text` 的契约注释写的是「markdown（飞书 post/markdown 由 adapter 转）」。
    飞书里唯一真能渲染 markdown 的载体就是卡片的 `markdown` 元素 —— msg_type=text
    会把 `**粗体**`、列表、链接原样当字面量吐出来，M5 那种「引用 ≥3 条群消息」的
    回复会糊成一坨。所以文本出站统一走这个卡片。
    """
    return {
        "config": {"wide_screen_mode": True, "update_multi": True},
        "elements": [{"tag": "markdown", "content": text}],
    }
