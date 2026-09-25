//! AIGC 标识 / 页脚模型名。主人：DD5（`LabelConfig` 由 T0c 加进本文件）。
//!
//! CC3 预埋：挂点在 `deliver.rs` 的 `send_text` 之前，今天没有调用点。

/// 桩：原样返回。
pub fn apply(text: &str) -> String {
    text.to_string()
}
