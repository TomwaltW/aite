# aite/contracts/capabilities.py
from typing import Literal

from pydantic import BaseModel

Platform = Literal["feishu", "dingtalk", "wecom", "fake"]

class PlatformCapabilities(BaseModel):
    platform: Platform
    supports_thread: bool                 # 有原生线程/话题
    supports_history: bool                # 能读群历史
    supports_passive_listen: bool         # 能收到非 @ 消息
    supports_card_edit: bool              # 卡片能原地更新
    card_edit_window_sec: int             # 卡片可更新时长（飞书 14 天 = 1209600）
    inbound_file_in_group: bool           # 群里能收文件
    proactive_requires_prior_message: bool
    outbound_rate_per_min: int            # 出站限速（消息数/分钟），adapter 自己令牌桶

FEISHU_P0 = PlatformCapabilities(
    platform="feishu", supports_thread=True, supports_history=True,
    supports_passive_listen=False,        # 3.7 权限核实后若拿到"获取群组中所有消息"，由 adapter 运行时改为 True
    supports_card_edit=True, card_edit_window_sec=1209600,
    inbound_file_in_group=True, proactive_requires_prior_message=False,
    outbound_rate_per_min=60,
)
