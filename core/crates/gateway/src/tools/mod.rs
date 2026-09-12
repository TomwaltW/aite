//! 5 个 Gateway 工具的公共底座（对应旧 `aite/tools/base.py` + `aite/tools/__init__.py`）。
//!
//! 工具实现只做「干活」，不构造 `ToolResult`：失败一律 `Err(ToolFailure{code, message})`，
//! 由 `P0ToolGateway::call` 统一翻成 `ToolResult{ok: false, error}`。§3.2 写明
//! 「永远不抛异常给调用方」—— 那是对 `call` 的要求，工具层往外返错是这个约定的实现方式。
//!
//! 错误码分工（§3.3 / 移植清单 §1 的对应表）：
//!   平台 / 外部系统失败 → upstream；沙箱创建、执行、读写 → sandbox；
//!   工具自己判定超时 → timeout（见 `python_exec`）。
//!   `not_found` / `invalid_args` / `denied` 由 Gateway 在调工具**之前**判掉，工具层不产出。
use std::collections::HashMap;
use std::sync::Arc;

use aite_contracts::ports::BoxFuture;
use aite_contracts::{ArtifactRef, PlatformPort, SandboxPort, ToolContext, ToolErrorCode};
use serde_json::{Map, Value};

use crate::gateway::Inner;

mod attachments;
mod documents;
mod files;
mod history;
mod python_exec;

/// 沙箱工作目录。与 `SandboxSpec::default_workdir()` 同值，旧实现也是拿模块常量排版。
pub(crate) const WORKDIR: &str = "/work";

/// 工具执行失败。`code` 直接就是要写进 `ToolError` 的那个码。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ToolFailure {
    pub code: ToolErrorCode,
    pub message: String,
}

impl ToolFailure {
    pub fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn upstream(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::Upstream, message)
    }
    pub fn sandbox(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::Sandbox, message)
    }
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::Timeout, message)
    }
}

/// 工具成功时的产出。`content` 由 Gateway 截断到 `MAX_TOOL_CONTENT_CHARS`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolOutcome {
    pub content: String,
    pub data: Option<Map<String, Value>>,
    pub artifacts: Vec<ArtifactRef>,
}

impl ToolOutcome {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            data: None,
            artifacts: Vec::new(),
        }
    }
    pub fn with_data(mut self, data: Map<String, Value>) -> Self {
        self.data = Some(data);
        self
    }
    pub fn with_artifacts(mut self, artifacts: Vec<ArtifactRef>) -> Self {
        self.artifacts = artifacts;
        self
    }
}

/// 工具实现的形状。arguments 传所有权：future 要 `'static` 才能进 `tokio::time::timeout`。
pub type ToolImpl = Arc<
    dyn Fn(
            ToolEnv,
            ToolContext,
            Map<String, Value>,
        ) -> BoxFuture<'static, Result<ToolOutcome, ToolFailure>>
        + Send
        + Sync,
>;

/// 一次 `call` 里工具能碰到的东西。
///
/// 沙箱不直接给 sandbox_id，而是两个动作：Gateway 按 task_id 记着容器，
/// `acquire_sandbox()` 是「有就复用、没有才建」，`current_sandbox_id()` 是
/// 「有就给、没有就 None」—— `list_files` 用后者，免得为了列一个空目录白建个容器。
#[derive(Clone)]
pub struct ToolEnv {
    inner: Arc<Inner>,
    task_id: String,
}

impl ToolEnv {
    pub(crate) fn new(inner: Arc<Inner>, task_id: impl Into<String>) -> Self {
        Self {
            inner,
            task_id: task_id.into(),
        }
    }

    /// 旧 `require_platform`。
    pub fn platform(&self) -> Result<Arc<dyn PlatformPort>, ToolFailure> {
        self.inner
            .platform()
            .ok_or_else(|| ToolFailure::upstream("本次运行没有接入平台，这个工具用不了"))
    }

    /// 旧 `require_sandbox`。
    pub fn sandbox(&self) -> Result<Arc<dyn SandboxPort>, ToolFailure> {
        self.inner
            .sandbox()
            .ok_or_else(|| ToolFailure::sandbox("本次运行没有可用沙箱，这个工具用不了"))
    }

    /// 有就复用、没有才建（per-task 锁，并发两个 tool_call 不会各建一个）。
    pub async fn acquire_sandbox(&self) -> Result<String, ToolFailure> {
        self.inner.acquire_sandbox(&self.task_id).await
    }

    pub fn current_sandbox_id(&self) -> Option<String> {
        self.inner.current_sandbox_id(&self.task_id)
    }

    /// 把记账里那个已经不存在的容器摘掉（见 `Inner::forget_sandbox`）。
    pub fn forget_sandbox(&self) -> Option<String> {
        self.inner.forget_sandbox(&self.task_id)
    }
}

/// 沙箱被 reaper 收走之后重建了一个，这句话要让**模型**看见。
///
/// 放进 `tracing` 是不够的：模型手上还留着「我刚才把中间结果写进了 /work/x.csv」这个
/// 记忆，新容器的 /work 是空的。不明说的话它下一步会去读一个不存在的文件，然后按
/// 「文件读不到」去猜原因 —— 那比直接报错更糟。所以这句话排在工具正文的最前面。
pub(crate) const SANDBOX_REBUILT_NOTE: &str = "【沙箱已重建】原来的容器因为长时间没有动作被回收了，这一次是在一个全新的容器里跑的：\
     之前写进 /work 的文件都不在了，需要哪个就重新生成一遍。";

/// `list_files` 那一路：容器没了就等于 /work 空了，这里**不**重建（模块头那条
/// 「列目录不该为它起一个容器」照样成立）。
pub(crate) const SANDBOX_REAPED_NOTE: &str = "【沙箱已回收】原来的容器因为长时间没有动作被回收了，之前写进 /work 的文件都不在了。\
     下一次 run_python 会起一个全新的空容器。";

/// 「容器没了」之后摘掉记账、重建一个，返回新的 sandbox_id。
///
/// **只给重试那一次用。** 重试之后再撞 NotFound 就照常收成 `code=sandbox`，不然
/// §3.3 的「连续 2 次沙箱失败 → task failed」那道闸会被这里的无限重建架空。
pub(crate) async fn rebuild_sandbox(env: &ToolEnv) -> Result<String, ToolFailure> {
    env.forget_sandbox();
    env.acquire_sandbox()
        .await
        .map_err(|e| ToolFailure::sandbox(format!("沙箱没了，重建也没成：{e}")))
}

macro_rules! tool_entry {
    ($name:literal, $f:path) => {
        (
            $name.to_string(),
            Arc::new(
                |env: ToolEnv,
                 ctx: ToolContext,
                 args: Map<String, Value>|
                 -> BoxFuture<'static, Result<ToolOutcome, ToolFailure>> {
                    Box::pin($f(env, ctx, args))
                },
            ) as ToolImpl,
        )
    };
}

/// 工具名 → 实现。键必须与 §3.1 `gateway_tools()` 里的 name 一一对应
/// （tests/catalog.rs 有一条断言把两边的名字集合钉在一起）。
pub(crate) fn default_tools() -> HashMap<String, ToolImpl> {
    HashMap::from([
        tool_entry!("read_group_history", history::read_group_history),
        tool_entry!("read_document", documents::read_document),
        tool_entry!("download_attachment", attachments::download_attachment),
        tool_entry!("run_python", python_exec::run_python),
        tool_entry!("list_files", files::list_files),
    ])
}

/// schema 校验过之后取参数：类型已经对了，拿不到就用 schema 的默认值兜底。
pub(crate) fn str_arg<'a>(args: &'a Map<String, Value>, name: &str) -> &'a str {
    args.get(name).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn u32_arg(args: &Map<String, Value>, name: &str, fallback: u32) -> u32 {
    args.get(name)
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(fallback)
}

pub(crate) fn bool_arg(args: &Map<String, Value>, name: &str, fallback: bool) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(fallback)
}

/// `json!({...})` 出来的一定是 Object；这层只是把类型落到 `ToolResult.data` 要的 Map 上。
pub(crate) fn as_map(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// `mimetypes.guess_type` 的本地版：只认 P0 用得到的后缀，答案与 Python 3.11 逐条对齐
/// （`.md` / `.yaml` 在 Python 那边也是 None，别"顺手"补上）。
pub(crate) fn guess_mime(path: &str) -> Option<String> {
    let ext = path.rsplit('/').next()?.rsplit_once('.')?.1.to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "txt" | "log" => "text/plain",
        "html" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "py" => "text/x-python",
        "bin" => "application/octet-stream",
        "wav" => "audio/x-wav",
        "mp4" => "video/mp4",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => return None,
    };
    Some(mime.to_string())
}
