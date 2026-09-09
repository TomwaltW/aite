"""tests/gateway 的私有替身与 fixture（owner: T3）。

§3.4 的测试替身规则：需要 FakePlatform / FakeSandbox 就在自己目录下写私有版本，
**不 import `aite.testing`**（那是 T4 的官方替身，并行期间还是空包）。
「重复远比冲突便宜」。

两个替身都照 §3.2 的签名逐字实现，并且刻意保留了真实现的两条关键脾气：
- `FakeGatewayPlatform.read_history` **不做** sender_kind 过滤（§3.2 注释写明
  过滤归 Gateway 的 read_group_history 工具）—— 替身要是先过滤了，
  「过滤是 T3 的责任」这条就测不出来了。
- `FakeGatewaySandbox.put_file` 照样走 `require_file_path` 那道 /work 闸。
"""
import asyncio
from datetime import UTC, datetime
from typing import Any

import pytest

from aite.contracts import (
    FEISHU_P0,
    ChecklistCard,
    DocumentContent,
    ExecRequest,
    ExecResult,
    FileEntry,
    HistoryMessage,
    OutboundFile,
    OutboundText,
    SendResult,
    ToolContext,
)
from aite.gateway import P0ToolGateway
from aite.sandbox import require_file_path

TASK_ID = "task-b5"
SESSION_TOKEN = "0123456789abcdef0123456789abcdef"   # Task.session_token 是随机 32 hex
CHAT_ID = "oc_chat_1"
THREAD_ID = "om_root_1"
ATTACHMENTS_MESSAGE_ID = "om_msg_with_file"


def at(minute: int) -> datetime:
    return datetime(2026, 9, 9, 9, minute, tzinfo=UTC)


class FakeGatewayPlatform:
    """PlatformPort 的私有假实现（§3.2 签名逐字对齐）。"""

    def __init__(
        self,
        *,
        history: list[HistoryMessage] | None = None,
        documents: dict[str, DocumentContent] | None = None,
        files: dict[str, bytes] | None = None,
    ) -> None:
        self.capabilities = FEISHU_P0
        self.history = history if history is not None else []
        self.documents = documents or {}
        self.files = files or {}

        #: 往这几个上挂异常/延时，就能演出 §3.3 里的平台侧失败。
        self.read_history_error: Exception | None = None
        self.read_document_error: Exception | None = None
        self.download_error: Exception | None = None
        self.delay_sec: float = 0.0

        self.calls: list[tuple[str, tuple[Any, ...], dict[str, Any]]] = []

    async def _enter(self, name: str, *args: Any, **kwargs: Any) -> None:
        self.calls.append((name, args, kwargs))
        if self.delay_sec:
            await asyncio.sleep(self.delay_sec)

    # --- 出站：P0 的 Gateway 工具用不到，但签名要齐（Protocol 是结构化的）---

    async def start(self, on_event) -> None: ...

    async def stop(self) -> None: ...

    async def send_text(self, msg: OutboundText) -> SendResult:
        return SendResult(message_id="om_sent")

    async def send_card(
        self, chat_id: str, reply_to: str | None, card: ChecklistCard
    ) -> SendResult:
        return SendResult(message_id="om_card", card_id="om_card")

    async def update_card(self, card_id: str, card: ChecklistCard) -> None: ...

    async def send_file(self, msg: OutboundFile) -> SendResult:
        return SendResult(message_id="om_file")

    async def add_reaction(self, message_id: str, kind: str) -> None: ...

    # --- 入站：这三个才是 Gateway 工具真正用的 ---

    async def read_history(
        self, chat_id: str, *, limit: int = 50, thread_id: str | None = None
    ) -> list[HistoryMessage]:
        await self._enter("read_history", chat_id, limit=limit, thread_id=thread_id)
        if self.read_history_error is not None:
            raise self.read_history_error
        rows = self.history
        if thread_id is not None:
            rows = [m for m in rows if m.thread_id == thread_id]
        return rows[-limit:]

    async def read_document(self, url_or_token: str) -> DocumentContent:
        await self._enter("read_document", url_or_token)
        if self.read_document_error is not None:
            raise self.read_document_error
        try:
            return self.documents[url_or_token]
        except KeyError as exc:
            raise RuntimeError(f"文档不存在：{url_or_token}") from exc

    async def download_file(self, message_id: str, file_key: str) -> bytes:
        await self._enter("download_file", message_id, file_key)
        if self.download_error is not None:
            raise self.download_error
        try:
            return self.files[file_key]
        except KeyError as exc:
            raise RuntimeError(f"附件不存在：{file_key}") from exc


class FakeGatewaySandbox:
    """SandboxPort 的私有假实现（§3.2 签名逐字对齐），/work 全在内存里。"""

    def __init__(self) -> None:
        self.trees: dict[str, dict[str, bytes]] = {}
        self.tasks: dict[str, str] = {}          # sandbox_id → task_id
        self.released: list[str] = []
        self.touched: list[str] = []
        self.exec_requests: list[tuple[str, ExecRequest]] = []
        self._seq = 0

        #: 行为旋钮
        self.acquire_error: Exception | None = None
        self.exec_error: Exception | None = None
        self.exec_result: ExecResult | None = None
        self.exec_delay_sec: float = 0.0

    async def acquire(self, task_id: str, spec) -> str:
        if self.acquire_error is not None:
            raise self.acquire_error
        self._seq += 1
        sandbox_id = f"fake-sbx-{self._seq}"
        self.trees[sandbox_id] = {}
        self.tasks[sandbox_id] = task_id
        return sandbox_id

    async def exec(self, sandbox_id: str, req: ExecRequest) -> ExecResult:
        self.exec_requests.append((sandbox_id, req))
        if self.exec_delay_sec:
            await asyncio.sleep(self.exec_delay_sec)
        if self.exec_error is not None:
            raise self.exec_error
        if self.exec_result is not None:
            return self.exec_result
        self.trees.setdefault(sandbox_id, {})["/work/out.txt"] = b"fake"
        return ExecResult(
            exit_code=0,
            stdout="fake stdout",
            stderr="",
            duration_ms=7,
            files_out=[FileEntry(path="/work/out.txt", size=4)],
        )

    async def put_file(self, sandbox_id: str, path: str, data: bytes) -> None:
        target = require_file_path(path)         # 真实现的 /work 闸，替身也走一遍
        self.trees.setdefault(sandbox_id, {})[str(target)] = data

    async def get_file(self, sandbox_id: str, path: str) -> bytes:
        target = require_file_path(path)
        return self.trees[sandbox_id][str(target)]

    async def list_files(self, sandbox_id: str) -> list[str]:
        return sorted(self.trees.get(sandbox_id, {}))

    async def touch(self, sandbox_id: str) -> None:
        self.touched.append(sandbox_id)

    async def release(self, sandbox_id: str) -> None:
        self.trees.pop(sandbox_id, None)
        self.tasks.pop(sandbox_id, None)
        self.released.append(sandbox_id)

    async def reap_idle(self, idle_sec: int) -> list[str]:
        victims = list(self.trees)
        for sandbox_id in victims:
            await self.release(sandbox_id)
        return victims


# --------------------------------------------------------------------- fixtures


@pytest.fixture
def history() -> list[HistoryMessage]:
    """故意混进 bot / app / system —— read_group_history 必须把它们过滤掉。"""
    rows = [
        ("om_1", "ou_zhang", "human", "张三", "这周的退款单据我整理好了", THREAD_ID),
        ("om_2", "cli_ci", "bot", "CI 机器人", "构建 #481 成功", THREAD_ID),
        ("om_3", "ou_li", "human", "李四", "麻烦按月度画个趋势图", THREAD_ID),
        ("om_4", "cli_aite", "app", "Aite", "已收到，正在处理", THREAD_ID),
        ("om_5", "sys", "system", None, "张三 邀请 李四 加入了群聊", None),
        ("om_6", "ou_wang", "human", "王五", "顺手把 Q3 也带上", None),
    ]
    return [
        HistoryMessage(
            message_id=mid,
            sender_id=sid,
            sender_kind=kind,
            sender_name=name,
            text=text,
            thread_id=thread,
            created_at=at(i),
        )
        for i, (mid, sid, kind, name, text, thread) in enumerate(rows)
    ]


@pytest.fixture
def platform(history) -> FakeGatewayPlatform:
    return FakeGatewayPlatform(
        history=history,
        documents={
            "https://feishu.cn/docx/abc": DocumentContent(
                title="退款流程 SOP",
                text="## 步骤\n\n1. 核对单据\n2. 走审批",
                url="https://feishu.cn/docx/abc",
            )
        },
        files={"file_v3_csv": "月份,销量\n2026-01,120\n".encode()},
    )


@pytest.fixture
def sandbox() -> FakeGatewaySandbox:
    return FakeGatewaySandbox()


@pytest.fixture
def ctx() -> ToolContext:
    return ToolContext(
        tenant_id="default",
        workspace_id="cli_app",
        chat_id=CHAT_ID,
        session_id="sess-1",
        task_id=TASK_ID,
        session_token=SESSION_TOKEN,
        thread_id=THREAD_ID,
        attachments_message_id=ATTACHMENTS_MESSAGE_ID,
    )


@pytest.fixture
def make_gateway(platform, sandbox):
    """造一个已经登记好 session_token 的 Gateway；kwargs 直通构造函数。"""

    def _make(*, register: bool = True, **kwargs) -> P0ToolGateway:
        kwargs.setdefault("platform", platform)
        kwargs.setdefault("sandbox", sandbox)
        gateway = P0ToolGateway(**kwargs)
        if register:
            gateway.register_task(TASK_ID, SESSION_TOKEN)
        return gateway

    return _make


@pytest.fixture
def gateway(make_gateway) -> P0ToolGateway:
    return make_gateway()
