//! 平台能力描述（对应旧 aite/contracts/capabilities.py）。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformCapabilities {
    /// "feishu" | "dingtalk" | "wecom" | "fake"
    pub platform: String,
    /// 有原生线程/话题
    pub supports_thread: bool,
    /// 能读群历史
    pub supports_history: bool,
    /// 能收到非 @ 消息
    pub supports_passive_listen: bool,
    /// 卡片能原地更新
    pub supports_card_edit: bool,
    /// 卡片可更新时长（飞书 14 天 = 1209600）
    pub card_edit_window_sec: u32,
    /// 群里能收文件
    pub inbound_file_in_group: bool,
    pub proactive_requires_prior_message: bool,
    /// 出站限速（消息数/分钟），adapter 自己令牌桶
    pub outbound_rate_per_min: u32,
}

/// 旧契约里的 FEISHU_P0 常量。每次调用返回新值：调用方可安全改 `supports_passive_listen`
/// （§3.7 权限核实后 adapter 运行时可能改成 true），不会污染别处。
pub fn feishu_p0() -> PlatformCapabilities {
    PlatformCapabilities {
        platform: "feishu".to_string(),
        supports_thread: true,
        supports_history: true,
        supports_passive_listen: false,
        supports_card_edit: true,
        card_edit_window_sec: 1_209_600,
        inbound_file_in_group: true,
        proactive_requires_prior_message: false,
        outbound_rate_per_min: 60,
    }
}
