# aite/contracts/ports.py  —— 跨 track 的接口。实现者：PlatformPort=T1，SessionStore/ControlPlane/EvidenceWriter=T2，
#                            SandboxPort/ToolGateway=T3，ModelPort（live）=T4。测试替身各 track 自建。
from collections.abc import Awaitable, Callable
from datetime import datetime
from typing import Protocol

from pydantic import BaseModel

from .capabilities import PlatformCapabilities
from .events import NormalizedEvent
from .evidence import EvidenceEvent, EvidenceKind
from .gateway import ToolContext, ToolResult
from .outbound import ChecklistCard, OutboundFile, OutboundText, ReactionKind, SendResult
from .protocol import Message, ModelTurn, ToolCallRequest, ToolSpec
from .sandbox import ExecRequest, ExecResult, SandboxSpec
from .session import Session, Task, Turn


class HistoryMessage(BaseModel):
    message_id: str
    sender_id: str
    sender_kind: str
    sender_name: str | None
    text: str
    thread_id: str | None
    created_at: datetime

class DocumentContent(BaseModel):
    title: str
    text: str            # markdown
    url: str

EventHandler = Callable[[NormalizedEvent], Awaitable[None]]

class PlatformPort(Protocol):
    capabilities: PlatformCapabilities
    async def start(self, on_event: EventHandler) -> None: ...
        # 建立长连接并持续投递事件。on_event 必须在 1s 内返回（只做去重/入库/入队），重活不在回调里做。
        # 断线自动重连；重连后平台重推的重复事件由 ControlPlane 靠 event_id 去重，adapter 不负责去重。
    async def stop(self) -> None: ...
    async def send_text(self, msg: OutboundText) -> SendResult: ...
    async def send_card(self, chat_id: str, reply_to: str | None, card: ChecklistCard) -> SendResult: ...
    async def update_card(self, card_id: str, card: ChecklistCard) -> None: ...
        # 必须原地更新同一条消息，绝不新发消息
    async def send_file(self, msg: OutboundFile) -> SendResult: ...
    async def add_reaction(self, message_id: str, kind: ReactionKind) -> None: ...
    async def read_history(self, chat_id: str, *, limit: int = 50, thread_id: str | None = None) -> list[HistoryMessage]: ...
        # 按时间正序返回；不做 sender_kind 过滤（过滤归 Gateway 的 read_group_history 工具）
    async def read_document(self, url_or_token: str) -> DocumentContent: ...
    async def download_file(self, message_id: str, file_key: str) -> bytes: ...

class ModelPort(Protocol):
    name: str
    async def chat(self, messages: list[Message], tools: list[ToolSpec], *, max_tokens: int, temperature: float) -> ModelTurn: ...

class SandboxPort(Protocol):
    async def acquire(self, task_id: str, spec: SandboxSpec) -> str: ...          # 返回 sandbox_id；容器打标签 aite.task=<task_id>
    async def exec(self, sandbox_id: str, req: ExecRequest) -> ExecResult: ...
    async def put_file(self, sandbox_id: str, path: str, data: bytes) -> None: ...  # path 必须在 /work 下
    async def get_file(self, sandbox_id: str, path: str) -> bytes: ...
    async def list_files(self, sandbox_id: str) -> list[str]: ...
    async def touch(self, sandbox_id: str) -> None: ...                           # 刷新最近活动时间
    async def release(self, sandbox_id: str) -> None: ...                         # 幂等
    async def reap_idle(self, idle_sec: int) -> list[str]: ...                    # 释放空闲超时的，返回被释放的 id

class ToolGateway(Protocol):
    def catalog(self, ctx: ToolContext) -> list[ToolSpec]: ...                    # P0 = GATEWAY_TOOLS 原样
    async def call(self, ctx: ToolContext, req: ToolCallRequest) -> ToolResult: ...
        # 顺序：校验 session_token → 查工具存在 → 按 ToolSpec.parameters 校验 arguments → 执行（带超时）→ 截断 content → 返回
        # 永远不抛异常给调用方；所有失败以 ToolResult(ok=False, error=...) 表达

class SessionStore(Protocol):
    async def init(self) -> None: ...                                             # 建表，幂等
    async def get_session(self, session_id: str) -> Session | None: ...
    async def find_session_by_thread(self, chat_id: str, thread_id: str) -> Session | None: ...
    async def create_session(self, s: Session) -> None: ...
    async def update_session(self, s: Session) -> None: ...
    async def append_turn(self, t: Turn) -> None: ...                             # seq 由调用方分配，重复 (session_id, seq) 报错
    async def list_turns(self, session_id: str, *, limit: int = 200) -> list[Turn]: ...
    async def create_task(self, t: Task) -> None: ...
    async def update_task(self, t: Task) -> None: ...
    async def get_task(self, task_id: str) -> Task | None: ...
    async def list_active_tasks(self, chat_id: str) -> list[Task]: ...           # status in (created, planning, working)
    async def next_task_no(self, tenant_id: str) -> str: ...                     # 原子递增 + encode_task_no
    async def seen_event(self, event_id: str) -> bool: ...                       # 去重：首次调用记录并返回 False，之后 True

class EvidenceWriter(Protocol):
    async def append(self, task_id: str, kind: EvidenceKind, payload: dict) -> EvidenceEvent: ...
    async def finalize(self, task_id: str, manifest_extra: dict) -> str: ...     # 写 manifest，返回 root_hash
    def verify(self, task_id: str) -> bool: ...                                   # 重算整条链

class ControlPlane(Protocol):
    async def handle_event(self, ev: NormalizedEvent) -> None: ...               # 3.5 路由规则的唯一入口
    async def run_forever(self) -> None: ...                                      # 派发队列里的任务给 worker、跑沙箱 reaper
