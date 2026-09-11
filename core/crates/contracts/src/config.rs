//! config/aite.yaml 的形状（对应旧 aite/contracts/config.py + 新增 edge 段）。
//! 密钥只放环境变量名，绝不放值。edge（Go）读的是它的子集镜像（edge/internal/config）。
use serde::{Deserialize, Serialize};

use crate::events::str_enum;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FeishuConfig {
    pub app_id_env: String,
    pub app_secret_env: String,
    pub bot_name: String,
    /// 用于识别 @ 的是不是自己
    pub bot_open_id_env: String,
    pub history_window: u32,
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            app_id_env: "FEISHU_APP_ID".into(),
            app_secret_env: "FEISHU_APP_SECRET".into(),
            bot_name: "Aite".into(),
            bot_open_id_env: "FEISHU_BOT_OPEN_ID".into(),
            history_window: 50,
        }
    }
}

str_enum! {
    ModelProvider { OpenaiCompat => "openai_compat", Scripted => "scripted" }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: ModelProvider,
    pub base_url: String,
    pub api_key_env: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    /// 元/百万 token，只用于卡片上的"已用 ¥"
    pub price_in_per_mtok: f64,
    pub price_out_per_mtok: f64,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: ModelProvider::OpenaiCompat,
            base_url: String::new(),
            api_key_env: "AITE_MODEL_API_KEY".into(),
            model: String::new(),
            max_tokens: 4096,
            temperature: 0.0,
            price_in_per_mtok: 0.0,
            price_out_per_mtok: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    pub image: String,
    pub cpu: f64,
    pub mem_mb: u32,
    pub idle_sec: u32,
    pub exec_timeout_sec: u32,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            image: "aite-sandbox:p0".into(),
            cpu: 1.0,
            mem_mb: 1024,
            idle_sec: 300,
            exec_timeout_sec: 120,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerConfig {
    pub max_steps: u32,
    pub max_wall_sec: u32,
    pub card_update_min_interval_ms: u64,
    /// 相对仓库根
    pub system_prompt_path: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            max_steps: 40,
            max_wall_sec: 1200,
            card_update_min_interval_ms: 500,
            system_prompt_path: "core/crates/worker/prompts/platform.md".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub sqlite_path: String,
    pub evidence_dir: String,
    pub artifacts_dir: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            sqlite_path: "data/aite.db".into(),
            evidence_dir: "data/evidence".into(),
            artifacts_dir: "data/artifacts".into(),
        }
    }
}

/// core ↔ edge 的进程边界（proto/aite/v1/edge.proto）。socket 路径相对仓库根。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EdgeConfig {
    /// edge 监听：PlatformService / SandboxService / EdgeStatusService
    pub edge_socket: String,
    /// core 监听：IngressService
    pub core_socket: String,
    /// edge 调 HandleEvent 的 deadline（§3.3 "on_event 1s 内返回"）
    pub handle_event_deadline_ms: u64,
    /// 两边 gRPC 的最大消息体（文件走 bytes）
    pub max_message_mb: u32,
}

impl Default for EdgeConfig {
    fn default() -> Self {
        Self {
            edge_socket: "data/run/aite-edge.sock".into(),
            core_socket: "data/run/aite-core.sock".into(),
            handle_event_deadline_ms: 1000,
            max_message_mb: 64,
        }
    }
}

str_enum! {
    PlatformChoice { Feishu => "feishu", Fake => "fake" }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiteConfig {
    pub tenant_id: String,
    pub platform: PlatformChoice,
    pub feishu: FeishuConfig,
    pub model: ModelConfig,
    pub sandbox: SandboxConfig,
    pub worker: WorkerConfig,
    pub storage: StorageConfig,
    pub edge: EdgeConfig,
}

impl Default for AiteConfig {
    fn default() -> Self {
        Self {
            tenant_id: "default".into(),
            platform: PlatformChoice::Feishu,
            feishu: FeishuConfig::default(),
            model: ModelConfig::default(),
            sandbox: SandboxConfig::default(),
            worker: WorkerConfig::default(),
            storage: StorageConfig::default(),
            edge: EdgeConfig::default(),
        }
    }
}

impl AiteConfig {
    /// 空文本 = 全默认；顶层必须是 mapping。未知键报错（防拼错静默失效）。
    pub fn from_yaml_str(text: &str) -> Result<Self, crate::errors::ConfigError> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let value: serde_yaml::Value = serde_yaml::from_str(text)?;
        if !value.is_mapping() {
            return Err(crate::errors::ConfigError::NotAMapping);
        }
        Ok(serde_yaml::from_value(value)?)
    }
}
