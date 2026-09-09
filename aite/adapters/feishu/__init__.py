"""飞书 Adapter（T1）—— `PlatformPort`（§3.2）在飞书上的实现。"""
from .api import DEFAULT_DOMAIN, RETRY_DELAYS, FeishuApiClient
from .cards import CARD_MAX_BYTES, build_checklist_card, build_markdown_card, dumps_card
from .connection import (
    RECONNECT_MAX_SEC,
    LarkWSConnection,
    RawEventHandler,
    WSConnection,
    backoff_delay,
)
from .errors import PlatformError
from .normalize import normalize, normalize_card_action, normalize_message
from .platform import REACTION_EMOJI, FeishuPlatform
from .ratelimit import TokenBucket

__all__ = [
    "CARD_MAX_BYTES",
    "DEFAULT_DOMAIN",
    "REACTION_EMOJI",
    "RECONNECT_MAX_SEC",
    "RETRY_DELAYS",
    "FeishuApiClient",
    "FeishuPlatform",
    "LarkWSConnection",
    "PlatformError",
    "RawEventHandler",
    "TokenBucket",
    "WSConnection",
    "backoff_delay",
    "build_checklist_card",
    "build_markdown_card",
    "dumps_card",
    "normalize",
    "normalize_card_action",
    "normalize_message",
]
