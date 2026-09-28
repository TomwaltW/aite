//! 厂商识别：从模型名 / 端点域名推断是哪一家（CC6）。
//!
//! 一个 OpenAI 兼容客户端通吃所有厂商，但各家在请求体、思考字段、缓存命中字段上各有
//! 各的脾气（总计划附录 A「国产模型接入要点」）。这里只做**识别**，调请求体在
//! `lib.rs` 的 `request_body`，回挂思考字段在 `echo.rs`。
//!
//! 判定顺序：**先看模型名前缀，再看 base_url 的 host**。百炼上托管的 Kimi / GLM /
//! DeepSeek 都走 `*.maas.aliyuncs.com`，只看域名会全判成 Qwen。都不中 → `Generic`
//! （请求体与 CC6 之前逐字节一致）。本轨不因端点在海外而报错或告警（那是 DD7 的事）；
//! `selfhost` 只能显式配置（T0 的 `ModelConfig.vendor`），推断永不返回它，本枚举也不带。
//!
//! 2026-09-25 DeepSeek 实测（总管本机真 key 测的；云端没有 key，未复测）：
//!
//! 1. `GET /models` 只剩 `deepseek-flash` 与 `deepseek-v4-pro` 两个 id。
//! 2. 配置里的 `deepseek-chat` 仍被接受，静默路由到 `deepseek-flash`（非思考模式），带工具能用。
//! 3. `deepseek-flash` 思考模式返回了 `reasoning_content`；第二轮工具调用**不回传**它也返回
//!    200 —— 调研里「会 400」没复现。

/// 厂商档。字符串与 T0 契约 `ModelVendor` 的前 7 个逐一相同（T0c 好一一对上）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Vendor {
    Generic,
    Deepseek,
    Qwen,
    Glm,
    Kimi,
    Doubao,
    Minimax,
}

impl Vendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Vendor::Generic => "generic",
            Vendor::Deepseek => "deepseek",
            Vendor::Qwen => "qwen",
            Vendor::Glm => "glm",
            Vendor::Kimi => "kimi",
            Vendor::Doubao => "doubao",
            Vendor::Minimax => "minimax",
        }
    }
}

/// 模型名前缀（不分大小写）→ 厂商。顺序无关：前缀两两不互为前缀。
const MODEL_PREFIXES: &[(&str, Vendor)] = &[
    ("deepseek", Vendor::Deepseek),
    ("qwen", Vendor::Qwen),
    ("glm", Vendor::Glm),
    ("kimi", Vendor::Kimi),
    ("moonshot", Vendor::Kimi),
    ("doubao", Vendor::Doubao),
    ("minimax", Vendor::Minimax),
];

/// 先模型名前缀、再 base_url 的 host；都不中 → `Generic`。
///
/// `base_url` 解析不了或没有 host 时只按前缀判。运行时路径，不许 panic。
pub fn infer_vendor(model: &str, base_url: &str) -> Vendor {
    let name = model.trim().to_ascii_lowercase();
    if let Some((_, v)) = MODEL_PREFIXES.iter().find(|(p, _)| name.starts_with(p)) {
        return *v;
    }
    let host = reqwest::Url::parse(base_url.trim())
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    host.as_deref().map_or(Vendor::Generic, vendor_of_host)
}

fn vendor_of_host(host: &str) -> Vendor {
    match host {
        "api.deepseek.com" => Vendor::Deepseek,
        "open.bigmodel.cn" => Vendor::Glm,
        "api.moonshot.cn" => Vendor::Kimi,
        "ark.cn-beijing.volces.com" => Vendor::Doubao,
        "api.minimax.cn" => Vendor::Minimax,
        h if h.ends_with(".maas.aliyuncs.com")
            || (h.starts_with("dashscope") && h.ends_with(".aliyuncs.com")) =>
        {
            Vendor::Qwen
        }
        _ => Vendor::Generic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_inference_prefix_wins_over_host() {
        // 百炼上托管的 Kimi / GLM / DeepSeek：前缀先判，不被域名带成 Qwen
        let bailian = "https://ws-abc.cn-beijing.maas.aliyuncs.com/compatible-mode/v1";
        assert_eq!(infer_vendor("Kimi-K2-Instruct", bailian), Vendor::Kimi);
        assert_eq!(infer_vendor("glm-4.6", bailian), Vendor::Glm);
        assert_eq!(infer_vendor("deepseek-v3.2", bailian), Vendor::Deepseek);
        assert_eq!(infer_vendor("some-model", bailian), Vendor::Qwen);
        // 无前缀 → 看 host
        let cases = [
            ("https://api.deepseek.com", Vendor::Deepseek),
            (
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
                Vendor::Qwen,
            ),
            ("https://open.bigmodel.cn/api/paas/v4", Vendor::Glm),
            ("https://api.moonshot.cn/v1", Vendor::Kimi),
            ("https://ark.cn-beijing.volces.com/api/v3", Vendor::Doubao),
            ("https://api.minimax.cn/v1", Vendor::Minimax),
            ("https://example.com/v1", Vendor::Generic),
            ("不是 URL", Vendor::Generic),
            ("", Vendor::Generic),
        ];
        for (url, want) in cases {
            assert_eq!(infer_vendor("m1", url), want, "{url}");
        }
        assert_eq!(infer_vendor("moonshot-v1-8k", ""), Vendor::Kimi);
        assert_eq!(infer_vendor("MiniMax-M2", "不是 URL"), Vendor::Minimax);
        let strs: Vec<&str> = [
            Vendor::Generic,
            Vendor::Deepseek,
            Vendor::Qwen,
            Vendor::Glm,
            Vendor::Kimi,
            Vendor::Doubao,
            Vendor::Minimax,
        ]
        .iter()
        .map(|v| v.as_str())
        .collect();
        assert_eq!(
            strs,
            [
                "generic", "deepseek", "qwen", "glm", "kimi", "doubao", "minimax"
            ]
        );
    }
}
