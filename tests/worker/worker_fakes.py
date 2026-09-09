"""tests/worker 私有的测试替身（§3.4：不 import aite.testing，重复远比冲突便宜）。"""
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from aite.contracts import (
    GATEWAY_TOOLS,
    AiteConfig,
    Anchor,
    Attachment,
    CardAction,
    ChecklistCard,
    DocumentContent,
    EventKind,
    HistoryMessage,
    Message,
    ModelTurn,
    NormalizedEvent,
    OutboundFile,
    OutboundText,
    PlatformCapabilities,
    SandboxSpec,
    SenderKind,
    SendResult,
    ToolCallRequest,
    ToolContext,
    ToolResult,
    ToolSpec,
    Usage,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
PLATFORM_MD = REPO_ROOT / "aite" / "worker" / "prompts" / "platform.md"

FAKE_CAPS = PlatformCapabilities(
    platform="fake",
    supports_thread=True,
    supports_history=True,
    supports_passive_listen=True,
    supports_card_edit=True,
    card_edit_window_sec=1209600,
    inbound_file_in_group=True,
    proactive_requires_prior_message=False,
    outbound_rate_per_min=60,
)


class FakeClock:
    """可手动推进的单调钟。worker 的 wall_sec 与卡片 500ms 窗口都读它。"""

    def __init__(self, start: float = 1000.0) -> None:
        self.t = start

    def __call__(self) -> float:
        return self.t

    def advance(self, dt: float) -> None:
        self.t += dt

    async def sleep(self, dt: float) -> None:
        """替掉 asyncio.sleep：不真等，只把钟拨过去。"""
        self.t += dt


class FakePlatform:
    """记录所有出站调用。断言全部对着这些 list 做。"""

    capabilities = FAKE_CAPS

    def __init__(self, history: list[HistoryMessage] | None = None) -> None:
        self.texts: list[OutboundText] = []
        self.cards: list[tuple[str, str | None, ChecklistCard]] = []
        self.card_updates: list[tuple[str, ChecklistCard]] = []
        self.files: list[OutboundFile] = []
        self.reactions: list[tuple[str, str]] = []
        self.history = history or []
        self.documents: dict[str, DocumentContent] = {}
        self.started = False
        self._n = 0

    def _next_id(self, prefix: str) -> str:
        self._n += 1
        return f"{prefix}_{self._n}"

    async def start(self, on_event) -> None:
        self.started = True

    async def stop(self) -> None:
        self.started = False

    async def send_text(self, msg: OutboundText) -> SendResult:
        self.texts.append(msg)
        return SendResult(message_id=self._next_id("om_text"))

    async def send_card(self, chat_id: str, reply_to: str | None, card: ChecklistCard) -> SendResult:
        self.cards.append((chat_id, reply_to, card))
        mid = self._next_id("om_card")
        return SendResult(message_id=mid, card_id=mid)

    async def update_card(self, card_id: str, card: ChecklistCard) -> None:
        self.card_updates.append((card_id, card))

    async def send_file(self, msg: OutboundFile) -> SendResult:
        self.files.append(msg)
        return SendResult(message_id=self._next_id("om_file"))

    async def add_reaction(self, message_id: str, kind: str) -> None:
        self.reactions.append((message_id, kind))

    async def read_history(
        self, chat_id: str, *, limit: int = 50, thread_id: str | None = None
    ) -> list[HistoryMessage]:
        return self.history[-limit:]

    async def read_document(self, url_or_token: str) -> DocumentContent:
        return self.documents.get(url_or_token, DocumentContent(title="", text="", url=url_or_token))

    async def download_file(self, message_id: str, file_key: str) -> bytes:
        return b""


class ScriptedModel:
    """按脚本出牌。脚本用完后：有 repeat 就一直出 repeat（死循环测试），否则报错。"""

    def __init__(
        self,
        script: list[ModelTurn] | None = None,
        *,
        repeat: ModelTurn | None = None,
        name: str = "scripted",
        clock: FakeClock | None = None,
        step_seconds: float = 0.0,
        fail_times: int = 0,
        on_call=None,
    ) -> None:
        self.name = name
        self._script = list(script or [])
        self._repeat = repeat
        self._clock = clock
        self._step_seconds = step_seconds
        self._fail_times = fail_times
        # on_call(step_index)：在这一步开始前做点别的事，比如往控制面塞一条 steer 消息
        self._on_call = on_call
        self.calls: list[list[Message]] = []
        self.tool_catalogs: list[list[ToolSpec]] = []

    async def chat(
        self, messages: list[Message], tools: list[ToolSpec], *, max_tokens: int, temperature: float
    ) -> ModelTurn:
        if self._on_call is not None:
            await self._on_call(len(self.calls))
        self.calls.append(list(messages))
        self.tool_catalogs.append(list(tools))
        if self._clock is not None and self._step_seconds:
            self._clock.advance(self._step_seconds)
        if self._fail_times > 0:
            self._fail_times -= 1
            raise RuntimeError("model 5xx")
        if self._script:
            return self._script.pop(0)
        if self._repeat is not None:
            return self._repeat.model_copy(deep=True)
        raise AssertionError(f"脚本已用完，但模型被第 {len(self.calls)} 次调用")


class FakeSandbox:
    def __init__(self, files: dict[str, bytes] | None = None) -> None:
        self.files = dict(files or {})
        self.acquired: list[str] = []
        self.released: list[str] = []
        self.reap_calls: list[int] = []
        self.reap_returns: list[str] = []

    async def acquire(self, task_id: str, spec: SandboxSpec) -> str:
        sid = f"sb_{task_id}"
        self.acquired.append(sid)
        return sid

    async def exec(self, sandbox_id: str, req):  # pragma: no cover - T2 不测执行面
        raise NotImplementedError

    async def put_file(self, sandbox_id: str, path: str, data: bytes) -> None:
        self.files[path] = data

    async def get_file(self, sandbox_id: str, path: str) -> bytes:
        return self.files[path]

    async def list_files(self, sandbox_id: str) -> list[str]:
        return sorted(self.files)

    async def touch(self, sandbox_id: str) -> None:
        return None

    async def release(self, sandbox_id: str) -> None:
        self.released.append(sandbox_id)

    async def reap_idle(self, idle_sec: int) -> list[str]:
        self.reap_calls.append(idle_sec)
        return list(self.reap_returns)


class FakeGateway:
    """按工具名返回预置 ToolResult；没预置的按 not_found 处理。"""

    def __init__(self, results: dict[str, ToolResult] | None = None) -> None:
        self.results = results or {}
        self.calls: list[tuple[ToolContext, ToolCallRequest]] = []

    def catalog(self, ctx: ToolContext) -> list[ToolSpec]:
        return list(GATEWAY_TOOLS)

    async def call(self, ctx: ToolContext, req: ToolCallRequest) -> ToolResult:
        self.calls.append((ctx, req))
        preset = self.results.get(req.name)
        if preset is None:
            return ToolResult(call_id=req.call_id, name=req.name, ok=True, content=f"{req.name} ok")
        return preset.model_copy(update={"call_id": req.call_id})


# ---- 构造器 --------------------------------------------------------------


def make_config(tmp_path: Path, **overrides: Any) -> AiteConfig:
    cfg = AiteConfig.model_validate(
        {
            "tenant_id": "default",
            "platform": "fake",
            "storage": {
                "sqlite_path": str(tmp_path / "aite.db"),
                "evidence_dir": str(tmp_path / "evidence"),
                "artifacts_dir": str(tmp_path / "artifacts"),
            },
            "worker": {"system_prompt_path": str(PLATFORM_MD)},
            "model": {"provider": "scripted", "model": "scripted-p0"},
        }
    )
    return cfg.model_copy(update=overrides) if overrides else cfg


def make_event(
    *,
    event_id: str = "ev1",
    kind: EventKind = EventKind.message,
    text: str = "你好",
    mentioned: bool = True,
    sender_kind: SenderKind = SenderKind.human,
    sender_id: str = "ou_user",
    sender_name: str | None = "张三",
    chat_id: str = "oc_chat",
    chat_type: str = "group",
    message_id: str = "om_1",
    thread_id: str | None = None,
    attachments: list[Attachment] | None = None,
    card_action: CardAction | None = None,
) -> NormalizedEvent:
    return NormalizedEvent(
        event_id=event_id,
        kind=kind,
        platform="fake",
        tenant_id="default",
        workspace_id="app_1",
        chat_id=chat_id,
        chat_type=chat_type,
        sender_id=sender_id,
        sender_kind=sender_kind,
        sender_name=sender_name,
        text=text,
        mentioned=mentioned,
        anchor=Anchor(platform="fake", chat_id=chat_id, message_id=message_id, thread_id=thread_id),
        attachments=attachments or [],
        card_action=card_action,
        occurred_at=datetime.now(UTC),
    )


def history(*rows: tuple[str, str, str, str]) -> list[HistoryMessage]:
    """(message_id, sender_kind, sender_name, text) → HistoryMessage 列表。"""
    return [
        HistoryMessage(
            message_id=mid,
            sender_id=f"ou_{name}",
            sender_kind=kind,
            sender_name=name,
            text=text,
            thread_id=None,
            created_at=datetime.now(UTC),
        )
        for mid, kind, name, text in rows
    ]


# ---- ModelTurn 构造器 ----------------------------------------------------


def text_turn(text: str) -> ModelTurn:
    return ModelTurn(message=Message(role="assistant", content=text), usage=Usage(input_tokens=10, output_tokens=5))


def tool_turn(*calls: tuple[str, dict]) -> ModelTurn:
    return ModelTurn(
        message=Message(
            role="assistant",
            tool_calls=[
                ToolCallRequest(call_id=f"call_{i}", name=name, arguments=args)
                for i, (name, args) in enumerate(calls)
            ],
        ),
        usage=Usage(input_tokens=10, output_tokens=5),
    )


def final_turn(reply: str, artifacts: list[dict] | None = None) -> ModelTurn:
    args: dict[str, Any] = {"reply": reply}
    if artifacts is not None:
        args["artifacts"] = artifacts
    return tool_turn(("final", args))
