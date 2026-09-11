//! 模型侧协议（对应旧 aite/contracts/protocol.py）：模型只能通过这些"工具"驱动进度面与产出。
use std::collections::HashSet;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::events::str_enum;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema (object)
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// 沙箱内绝对路径，必须以 /work/ 开头
    pub path: String,
    pub title: String,
    #[serde(default)]
    pub mime: Option<String>,
}

fn spec(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    }
}

// —— worker 本地处理的工具（不进 Gateway）——
static CHECKLIST_TOOLS: LazyLock<Vec<ToolSpec>> = LazyLock::new(|| {
    vec![
        spec(
            "checklist_add",
            "添加待办项，仅在任务开始或发现新步骤时调用；每项 ≤20 字",
            json!({"type": "object", "properties": {"items": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 8}}, "required": ["items"]}),
        ),
        spec(
            "checklist_check",
            "把某项标记为完成",
            json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        ),
        spec(
            "checklist_fail",
            "把某项标记为失败并说明原因",
            json!({"type": "object", "properties": {"id": {"type": "string"}, "reason": {"type": "string"}}, "required": ["id", "reason"]}),
        ),
        spec(
            "checklist_note",
            "在卡片上写一条 ≤40 字的备注（不新增消息）",
            json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
        ),
    ]
});

static FINAL_TOOL: LazyLock<ToolSpec> = LazyLock::new(|| {
    spec(
        "final",
        "交付最终结果。reply 为 markdown；artifacts 为沙箱 /work/ 下要发回线程的文件。调用后任务结束。",
        json!({"type": "object", "properties": {
            "reply": {"type": "string"},
            "artifacts": {"type": "array", "items": {"type": "object", "properties": {
                "path": {"type": "string"}, "title": {"type": "string"}}, "required": ["path", "title"]}}},
            "required": ["reply"]}),
    )
});

// —— Gateway 工具（P0 目录，名字与 schema 冻结）——
static GATEWAY_TOOLS: LazyLock<Vec<ToolSpec>> = LazyLock::new(|| {
    vec![
        spec(
            "read_group_history",
            "读取本群最近的消息（只含真人消息）",
            json!({"type": "object", "properties": {
                "limit": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50},
                "thread_only": {"type": "boolean", "default": false}}}),
        ),
        spec(
            "read_document",
            "读取一篇飞书云文档，返回 markdown 文本",
            json!({"type": "object", "properties": {"url_or_token": {"type": "string"}}, "required": ["url_or_token"]}),
        ),
        spec(
            "download_attachment",
            "把本次消息里的附件下载到沙箱 /work/in/ 下，返回路径",
            json!({"type": "object", "properties": {"file_key": {"type": "string"}}, "required": ["file_key"]}),
        ),
        spec(
            "run_python",
            "在隔离沙箱里执行 Python（无网络）。工作目录 /work，输出文件写到 /work/ 下",
            json!({"type": "object", "properties": {
                "code": {"type": "string"},
                "timeout_sec": {"type": "integer", "minimum": 1, "maximum": 300, "default": 120}}, "required": ["code"]}),
        ),
        spec(
            "list_files",
            "列出沙箱 /work 下的文件",
            json!({"type": "object", "properties": {}}),
        ),
    ]
});

static ALL_MODEL_TOOLS: LazyLock<Vec<ToolSpec>> = LazyLock::new(|| {
    let mut all = CHECKLIST_TOOLS.clone();
    all.push(FINAL_TOOL.clone());
    all.extend(GATEWAY_TOOLS.iter().cloned());
    all
});

static LOCAL_TOOL_NAMES: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let mut names: HashSet<String> = CHECKLIST_TOOLS.iter().map(|t| t.name.clone()).collect();
    names.insert(FINAL_TOOL.name.clone());
    names
});

pub fn checklist_tools() -> &'static [ToolSpec] {
    &CHECKLIST_TOOLS
}
pub fn final_tool() -> &'static ToolSpec {
    &FINAL_TOOL
}
pub fn gateway_tools() -> &'static [ToolSpec] {
    &GATEWAY_TOOLS
}
/// CHECKLIST_TOOLS + [FINAL_TOOL] + GATEWAY_TOOLS，顺序固定（进模型的工具目录就是它）。
pub fn all_model_tools() -> &'static [ToolSpec] {
    &ALL_MODEL_TOOLS
}
/// {checklist_add, checklist_check, checklist_fail, checklist_note, final}
pub fn local_tool_names() -> &'static HashSet<String> {
    &LOCAL_TOOL_NAMES
}
pub fn is_local_tool(name: &str) -> bool {
    LOCAL_TOOL_NAMES.contains(name)
}

str_enum! {
    /// 模型调用面的消息角色（OpenAI-compatible 子集）
    Role { System => "system", User => "user", Assistant => "assistant", Tool => "tool" }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    /// assistant
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCallRequest>>,
    /// tool
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// tool
    #[serde(default)]
    pub name: Option<String>,
}

impl Message {
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
}

fn default_finish_reason() -> String {
    "stop".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelTurn {
    pub message: Message,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default = "default_finish_reason")]
    pub finish_reason: String,
    #[serde(default)]
    pub raw: Map<String, Value>,
}
