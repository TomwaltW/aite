//! 沙箱规格与执行结果（对应旧 aite/contracts/sandbox.py；跨进程形态见 proto/aite/v1/sandbox.proto）。
use serde::{Deserialize, Serialize};

use crate::events::str_enum;

str_enum! {
    /// P0 沙箱无网络：凭证/平台调用都不在沙箱里发生
    SandboxNetwork { None => "none" }
}

str_enum! {
    ExecLanguage { Python => "python" }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub image: String,
    #[serde(default = "SandboxSpec::default_cpu")]
    pub cpu: f64,
    #[serde(default = "SandboxSpec::default_mem_mb")]
    pub mem_mb: u32,
    #[serde(default = "SandboxSpec::default_network")]
    pub network: SandboxNetwork,
    #[serde(default = "SandboxSpec::default_workdir")]
    pub workdir: String,
}

impl SandboxSpec {
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            cpu: Self::default_cpu(),
            mem_mb: Self::default_mem_mb(),
            network: Self::default_network(),
            workdir: Self::default_workdir(),
        }
    }
    fn default_cpu() -> f64 {
        1.0
    }
    fn default_mem_mb() -> u32 {
        1024
    }
    fn default_network() -> SandboxNetwork {
        SandboxNetwork::None
    }
    fn default_workdir() -> String {
        "/work".to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecRequest {
    #[serde(default = "ExecRequest::default_language")]
    pub language: ExecLanguage,
    pub code: String,
    #[serde(default = "ExecRequest::default_timeout_sec")]
    pub timeout_sec: u32,
}

impl ExecRequest {
    pub fn python(code: impl Into<String>, timeout_sec: u32) -> Self {
        Self {
            language: ExecLanguage::Python,
            code: code.into(),
            timeout_sec,
        }
    }
    fn default_language() -> ExecLanguage {
        ExecLanguage::Python
    }
    fn default_timeout_sec() -> u32 {
        120
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    /// stdout/stderr 超过 MAX_EXEC_OUTPUT_CHARS 被截断
    #[serde(default)]
    pub truncated: bool,
    /// /work 下本次新增/修改的文件
    #[serde(default)]
    pub files_out: Vec<FileEntry>,
}

pub const MAX_EXEC_OUTPUT_CHARS: usize = 20_000;
/// POSIX coreutils timeout 的惯例；沙箱内超时统一表达为这个退出码
pub const EXEC_TIMEOUT_EXIT_CODE: i32 = 124;
