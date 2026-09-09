"""飞书原始事件 → `NormalizedEvent`（dev-spec §3.1 数据形状 + 附录 A）。

只认 P0 订阅的两类事件：`im.message.receive_v1` 和 `card.action.trigger`（附录 A）。
其余事件返回 `None` —— P0 压根没订阅它们，凭空映射成别的 `EventKind` 就是在发明契约。

归一化全程只读原始 dict，不经过 lark_oapi 的类型对象：`raw` 要原样进
`NormalizedEvent.raw` 供审计（§3.1「任何逻辑不得依赖 raw」），过一遍 SDK 模型
再吐回来只会丢字段。这也让 §6 的 `x.json → x.expected.json` 判据能直接用
`tests/fixtures/feishu/` 下的原始事件喂进来。
"""
from __future__ import annotations

import json
import re
from datetime import UTC, datetime
from typing import Any

from ...contracts import (
    Anchor,
    Attachment,
    CardAction,
    EventKind,
    NormalizedEvent,
    SenderKind,
)

PLATFORM = "feishu"

EVENT_MESSAGE_RECEIVE = "im.message.receive_v1"
EVENT_CARD_ACTION = "card.action.trigger"

#: P0 订阅的事件；normalize() 只处理这两个。
SUBSCRIBED_EVENT_TYPES = frozenset({EVENT_MESSAGE_RECEIVE, EVENT_CARD_ACTION})

#: 飞书 `sender.sender_type` → 契约的 SenderKind。
#: §3.1 / 派单：`user` → human，其余 → app/bot。飞书自己的机器人消息 sender_type 是
#: `app`，所以「其余」的兜底落在 app 上；显式写出 bot 只是给别的平台/未来值留位置。
#: 一律不映射到 `system`：§3.5 R1 只看「是不是 human」，把未知值猜成 system 反而
#: 会让审计看不出它其实是个应用。
_SENDER_KIND = {
    "user": SenderKind.human,
    "bot": SenderKind.bot,
    "app": SenderKind.app,
    "system": SenderKind.system,
}
_SENDER_KIND_FALLBACK = SenderKind.app

#: 消息类型 → (附件 kind, content 里装 key 的字段名)。
_ATTACHMENT_BY_MSG_TYPE: dict[str, tuple[str, str]] = {
    "image": ("image", "image_key"),
    "sticker": ("image", "file_key"),
    "file": ("file", "file_key"),
    "audio": ("file", "file_key"),
    "media": ("file", "file_key"),
}

#: 带正文文本的消息类型；其余（image/file/audio/media/sticker/interactive…）
#: 按 §3.1「非文本消息为 ""」处理。
_TEXT_MSG_TYPES = frozenset({"text", "post"})

#: 占位符后跟的空白一并吃掉；U+00A0 是飞书客户端在 @ 后面塞的不换行空格。
_PLACEHOLDER_RE = re.compile(r"@_(?:user|all)_\d+[ \t\u00a0]*")


# ---------------------------------------------------------------------------
# 小工具
# ---------------------------------------------------------------------------

def _first(*values: Any) -> Any | None:
    """返回第一个非空值；全空返回 None。"""
    for value in values:
        if value:
            return value
    return None


def _as_dict(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def _as_list(value: Any) -> list[Any]:
    return value if isinstance(value, list) else []


def _to_datetime(value: Any) -> datetime | None:
    """飞书的毫秒时间戳（字符串或整数）→ tz-aware datetime。"""
    if value in (None, ""):
        return None
    try:
        ms = int(value)
    except (TypeError, ValueError):
        return None
    return datetime.fromtimestamp(ms / 1000, tz=UTC)


def _to_int(value: Any) -> int | None:
    if value in (None, ""):
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def mention_open_id(mention: dict[str, Any]) -> str | None:
    """取一条 mention 的 open_id。

    两种形状都要吃：事件里 `mentions[].id` 是对象（`{"open_id": ...}`），
    而「获取会话历史消息」返回的 `mentions[].id` 是裸字符串。
    """
    ident = mention.get("id")
    if isinstance(ident, dict):
        return ident.get("open_id")
    if isinstance(ident, str):
        return ident or None
    return None


def load_message_content(message: dict[str, Any]) -> dict[str, Any]:
    """`message.content` 是一段 JSON 字符串，解析失败时按空 content 处理。"""
    raw = message.get("content")
    if isinstance(raw, dict):
        return raw
    if not isinstance(raw, str) or not raw:
        return {}
    try:
        parsed = json.loads(raw)
    except (TypeError, ValueError):
        return {}
    return parsed if isinstance(parsed, dict) else {}


def apply_mentions(text: str, mentions: list[dict[str, Any]], bot_open_id: str | None) -> str:
    """把 `@_user_N` 占位符换成人话，并去掉 @Aite。

    * @ 到机器人自己的那条：连同紧跟的空白一起删掉，`text` 里不留 @Aite（§3.1）。
    * @ 到别人的：换成 `@姓名`，模型看到的是人名而不是 `@_user_2`。
    * 兜底再扫一遍剩余占位符 —— mentions 缺项时也不能把 `@_user_N` 漏给模型。
    """
    for mention in mentions:
        key = mention.get("key")
        if not key:
            continue
        if bot_open_id and mention_open_id(mention) == bot_open_id:
            text = re.sub(re.escape(key) + r"[ \t\u00a0]*", "", text)
        else:
            name = mention.get("name")
            text = text.replace(key, f"@{name}" if name else "")
    return _PLACEHOLDER_RE.sub("", text).strip()


def _flatten_post(
    content: dict[str, Any],
    *,
    bot_open_id: str | None,
    image_keys: list[str] | None = None,
) -> str:
    """富文本（post）拍平成纯文本；顺带把内嵌图片的 image_key 收走。

    post 是「文本消息」的一种，所以按 §3.1 给 `text` 填内容而不是留空 ——
    群里真人发的排版消息（M3/M5 的场景）不该在模型眼里变成一片空白。
    """
    lines: list[str] = []
    title = content.get("title")
    if isinstance(title, str) and title.strip():
        lines.append(title.strip())

    for paragraph in _as_list(content.get("content")):
        parts: list[str] = []
        for segment in _as_list(paragraph):
            if not isinstance(segment, dict):
                continue
            tag = segment.get("tag")
            if tag in ("text", "md", "code_block"):
                parts.append(str(segment.get("text") or segment.get("content") or ""))
            elif tag == "a":
                label = str(segment.get("text") or "")
                href = str(segment.get("href") or "")
                parts.append(f"[{label}]({href})" if href else label)
            elif tag == "at":
                user_id = segment.get("user_id") or ""
                if bot_open_id and user_id == bot_open_id:
                    continue
                name = segment.get("user_name")
                parts.append(f"@{name}" if name else "")
            elif tag == "img":
                key = segment.get("image_key")
                if key and image_keys is not None:
                    image_keys.append(str(key))
            elif tag == "emotion":
                parts.append(str(segment.get("emoji_type") or ""))
        line = "".join(parts).strip()
        if line:
            lines.append(line)

    return "\n".join(lines)


def sender_kind_of(sender_type: Any) -> SenderKind:
    """`sender.sender_type` → SenderKind（附录 A）。"""
    if not isinstance(sender_type, str):
        return _SENDER_KIND_FALLBACK
    return _SENDER_KIND.get(sender_type.lower(), _SENDER_KIND_FALLBACK)


def extract_text(
    message: dict[str, Any],
    mentions: list[dict[str, Any]],
    bot_open_id: str | None,
) -> tuple[str, str | None, list[str]]:
    """返回 `(text, raw_text, 内嵌图片的 image_key 列表)`。

    `text` 是去掉 @Aite、strip 过的纯文本；`raw_text` 保留 @ 之前的原样文本，
    好让审计能看出用户到底打了什么。非文本消息 `text=""`、`raw_text=None`（§3.1）。
    """
    msg_type = message.get("message_type") or message.get("msg_type")
    if msg_type not in _TEXT_MSG_TYPES:
        return "", None, []

    content = load_message_content(message)
    if msg_type == "text":
        raw_text = str(content.get("text") or "")
        return apply_mentions(raw_text, mentions, bot_open_id), raw_text, []

    # post：先拍平一份保留 @Aite 的做 raw_text，再拍平一份去掉 @Aite 的做 text。
    image_keys: list[str] = []
    raw_text = _flatten_post(content, bot_open_id=None)
    text = _flatten_post(content, bot_open_id=bot_open_id, image_keys=image_keys)
    return apply_mentions(text, mentions, bot_open_id), raw_text, image_keys


def _attachments_of(message: dict[str, Any], message_id: str, image_keys: list[str]) -> list[Attachment]:
    attachments: list[Attachment] = []
    msg_type = message.get("message_type") or message.get("msg_type")
    mapping = _ATTACHMENT_BY_MSG_TYPE.get(msg_type or "")
    if mapping:
        kind, key_field = mapping
        content = load_message_content(message)
        file_key = content.get(key_field)
        if file_key:
            attachments.append(
                Attachment(
                    kind=kind,
                    file_key=str(file_key),
                    message_id=message_id,
                    name=content.get("file_name") or None,
                    size=_to_int(content.get("file_size")),
                    mime=content.get("mime_type") or None,
                )
            )
    for image_key in image_keys:
        attachments.append(Attachment(kind="image", file_key=image_key, message_id=message_id))
    return attachments


# ---------------------------------------------------------------------------
# 两个事件的归一化
# ---------------------------------------------------------------------------

def normalize_message(
    raw: dict[str, Any],
    *,
    bot_open_id: str | None,
    workspace_id: str = "",
    tenant_id: str = "default",
) -> NormalizedEvent:
    """`im.message.receive_v1` → NormalizedEvent。"""
    header = _as_dict(raw.get("header"))
    event = _as_dict(raw.get("event"))
    message = _as_dict(event.get("message"))
    sender = _as_dict(event.get("sender"))

    message_id = str(message.get("message_id") or "")
    mentions = [m for m in _as_list(message.get("mentions")) if isinstance(m, dict)]
    text, raw_text, image_keys = extract_text(message, mentions, bot_open_id)

    mentioned = bool(bot_open_id) and (
        any(mention_open_id(m) == bot_open_id for m in mentions)
        or _post_mentions_bot(message, bot_open_id)
    )

    # 话题锚点：按 root_id → parent_id → thread_id 取第一个非空（派单原文列了这三个）。
    # 契约把 thread_id 说成「话题 root 消息 id」，root_id 正是它；顶层消息三个都没有 → None。
    thread_id = _first(message.get("root_id"), message.get("parent_id"), message.get("thread_id"))

    chat_type = "p2p" if message.get("chat_type") == "p2p" else "group"
    occurred_at = (
        _to_datetime(message.get("create_time"))
        or _to_datetime(header.get("create_time"))
        or datetime.now(tz=UTC)
    )

    return NormalizedEvent(
        event_id=str(_first(header.get("event_id"), message_id) or ""),
        kind=EventKind.message,
        platform=PLATFORM,
        tenant_id=tenant_id,
        workspace_id=str(_first(header.get("app_id"), workspace_id) or ""),
        chat_id=str(message.get("chat_id") or ""),
        chat_type=chat_type,
        sender_id=str(_as_dict(sender.get("sender_id")).get("open_id") or ""),
        sender_kind=sender_kind_of(sender.get("sender_type")),
        sender_name=None,
        text=text,
        raw_text=raw_text,
        mentioned=mentioned,
        anchor=Anchor(
            platform=PLATFORM,
            chat_id=str(message.get("chat_id") or ""),
            message_id=message_id,
            thread_id=str(thread_id) if thread_id else None,
        ),
        attachments=_attachments_of(message, message_id, image_keys),
        card_action=None,
        occurred_at=occurred_at,
        raw=raw,
    )


def _post_mentions_bot(message: dict[str, Any], bot_open_id: str) -> bool:
    """post 正文里的 `at` 段也算 @：它不走 mentions 列表。"""
    if (message.get("message_type") or message.get("msg_type")) != "post":
        return False
    content = load_message_content(message)
    for paragraph in _as_list(content.get("content")):
        for segment in _as_list(paragraph):
            if isinstance(segment, dict) and segment.get("tag") == "at":
                if segment.get("user_id") == bot_open_id:
                    return True
    return False


def normalize_card_action(
    raw: dict[str, Any],
    *,
    workspace_id: str = "",
    tenant_id: str = "default",
) -> NormalizedEvent | None:
    """`card.action.trigger` → NormalizedEvent。

    `action.value` 不是 `stop`/`evidence` 的（比如别的按钮）返回 None ——
    契约的 `CardAction.action` 是 Literal，塞别的值进去只会在这里炸成 500。
    """
    header = _as_dict(raw.get("header"))
    event = _as_dict(raw.get("event"))
    context = _as_dict(event.get("context"))
    operator = _as_dict(event.get("operator"))
    action = _as_dict(event.get("action"))
    value = _as_dict(action.get("value"))

    name = value.get("action")
    if name not in ("stop", "evidence"):
        return None

    card_id = str(context.get("open_message_id") or "")
    chat_id = str(context.get("open_chat_id") or "")
    occurred_at = _to_datetime(header.get("create_time")) or datetime.now(tz=UTC)

    return NormalizedEvent(
        event_id=str(_first(header.get("event_id"), card_id) or ""),
        kind=EventKind.card_action,
        platform=PLATFORM,
        tenant_id=tenant_id,
        workspace_id=str(_first(header.get("app_id"), workspace_id) or ""),
        chat_id=chat_id,
        # 卡片回传事件不带 chat_type。P0 的卡片只发在群里（§1 不做 DM），
        # 且 §3.5 R3 先于任何 chat_type 判定命中，这里固定 group。
        chat_type="group",
        sender_id=str(operator.get("open_id") or ""),
        # 点按钮的一定是真人；机器人点不动卡片。
        sender_kind=SenderKind.human,
        sender_name=None,
        text="",
        raw_text=None,
        # 点的是我们自己发的卡片，等价于「冲着 Aite 来的」。
        mentioned=True,
        anchor=Anchor(
            platform=PLATFORM,
            chat_id=chat_id,
            message_id=card_id,
            # 卡片回传只给得到卡片自己那条消息，话题归属由 ControlPlane 按 task_id 查。
            thread_id=None,
        ),
        attachments=[],
        card_action=CardAction(
            card_id=card_id,
            action=name,
            task_id=value.get("task_id") or None,
            value=value,
        ),
        occurred_at=occurred_at,
        raw=raw,
    )


def normalize(
    raw: dict[str, Any],
    *,
    bot_open_id: str | None,
    workspace_id: str = "",
    tenant_id: str = "default",
) -> NormalizedEvent | None:
    """事件总入口：认识就归一化，不认识返回 None。"""
    event_type = _as_dict(raw.get("header")).get("event_type")
    if event_type == EVENT_MESSAGE_RECEIVE:
        return normalize_message(
            raw, bot_open_id=bot_open_id, workspace_id=workspace_id, tenant_id=tenant_id
        )
    if event_type == EVENT_CARD_ACTION:
        return normalize_card_action(raw, workspace_id=workspace_id, tenant_id=tenant_id)
    return None
