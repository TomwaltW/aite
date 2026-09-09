# aite/contracts/protocol.py  —— 模型侧协议：模型只能通过这些"工具"驱动进度面与产出
from typing import Any, Literal

from pydantic import BaseModel, Field


class ToolSpec(BaseModel):
    name: str
    description: str
    parameters: dict[str, Any]      # JSON Schema (object)

class ToolCallRequest(BaseModel):
    call_id: str
    name: str
    arguments: dict[str, Any] = Field(default_factory=dict)

class ArtifactRef(BaseModel):
    path: str                       # 沙箱内绝对路径，必须以 /work/ 开头
    title: str
    mime: str | None = None

# —— worker 本地处理的工具（不进 Gateway）——
CHECKLIST_TOOLS: list[ToolSpec] = [
    ToolSpec(name="checklist_add", description="添加待办项，仅在任务开始或发现新步骤时调用；每项 ≤20 字",
             parameters={"type": "object", "properties": {"items": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 8}}, "required": ["items"]}),
    ToolSpec(name="checklist_check", description="把某项标记为完成",
             parameters={"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
    ToolSpec(name="checklist_fail", description="把某项标记为失败并说明原因",
             parameters={"type": "object", "properties": {"id": {"type": "string"}, "reason": {"type": "string"}}, "required": ["id", "reason"]}),
    ToolSpec(name="checklist_note", description="在卡片上写一条 ≤40 字的备注（不新增消息）",
             parameters={"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
]
FINAL_TOOL = ToolSpec(
    name="final",
    description="交付最终结果。reply 为 markdown；artifacts 为沙箱 /work/ 下要发回线程的文件。调用后任务结束。",
    parameters={"type": "object", "properties": {
        "reply": {"type": "string"},
        "artifacts": {"type": "array", "items": {"type": "object", "properties": {
            "path": {"type": "string"}, "title": {"type": "string"}}, "required": ["path", "title"]}}},
        "required": ["reply"]},
)

# —— Gateway 工具（P0 目录，名字与 schema 冻结；实现归 T3）——
GATEWAY_TOOLS: list[ToolSpec] = [
    ToolSpec(name="read_group_history", description="读取本群最近的消息（只含真人消息）",
             parameters={"type": "object", "properties": {
                 "limit": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50},
                 "thread_only": {"type": "boolean", "default": False}}}),
    ToolSpec(name="read_document", description="读取一篇飞书云文档，返回 markdown 文本",
             parameters={"type": "object", "properties": {"url_or_token": {"type": "string"}}, "required": ["url_or_token"]}),
    ToolSpec(name="download_attachment", description="把本次消息里的附件下载到沙箱 /work/in/ 下，返回路径",
             parameters={"type": "object", "properties": {"file_key": {"type": "string"}}, "required": ["file_key"]}),
    ToolSpec(name="run_python", description="在隔离沙箱里执行 Python（无网络）。工作目录 /work，输出文件写到 /work/ 下",
             parameters={"type": "object", "properties": {
                 "code": {"type": "string"},
                 "timeout_sec": {"type": "integer", "minimum": 1, "maximum": 300, "default": 120}}, "required": ["code"]}),
    ToolSpec(name="list_files", description="列出沙箱 /work 下的文件", parameters={"type": "object", "properties": {}}),
]

ALL_MODEL_TOOLS: list[ToolSpec] = CHECKLIST_TOOLS + [FINAL_TOOL] + GATEWAY_TOOLS
LOCAL_TOOL_NAMES = {t.name for t in CHECKLIST_TOOLS} | {FINAL_TOOL.name}

# —— 模型调用面的消息形状（OpenAI-compatible 子集）——
class Message(BaseModel):
    role: Literal["system", "user", "assistant", "tool"]
    content: str = ""
    tool_calls: list[ToolCallRequest] | None = None   # assistant
    tool_call_id: str | None = None                   # tool
    name: str | None = None                           # tool

class Usage(BaseModel):
    input_tokens: int = 0
    output_tokens: int = 0
    cached_tokens: int = 0

class ModelTurn(BaseModel):
    message: Message
    usage: Usage = Field(default_factory=Usage)
    finish_reason: str = "stop"
    raw: dict[str, Any] = Field(default_factory=dict)
