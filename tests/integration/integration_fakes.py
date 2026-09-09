"""tests/integration 的公共器材（owner: T8）。

平台与模型用 T4 的官方替身（`aite.testing`，§3.4 明确它是给整合测试用的），
只在两处做了最小加厚 —— 加厚的都是**真实现真有、替身没有**的行为，不是为了让断言好写：

* `GatedPlatform`：`stop()` 之后不再往上投事件。真 adapter 停掉长连接就是这个样子，
  而 `FakePlatform.emit` 是测试侧的注入口，不看 `stopped`。收尾期间「新事件不再被处理」
  要验的正是这条。
* `ClosableFakeSandbox`：补一个 `aclose()`，行为照 `DockerSandbox.aclose`
  （把本进程记着的沙箱全还掉）。C-TΩ-1 的退出序列写死了 `sandbox.aclose()`，
  但 `SandboxPort`（§3.2 冻结）里没有这个方法、`FakeSandbox` 也没有 —— 见回执 D-1。
* `RecordingModel`：把每次 `chat` 收到的 messages 原样留下。跨进程续接要验的不只是
  「库里有那一行」，而是「第二个进程的模型上下文里真的读到了上一轮」。

落盘断言一律走 `tmp_path`，谁都不许往仓库的 data/ 写。
"""
from __future__ import annotations

import asyncio
import json
from collections.abc import Callable, Iterable
from contextlib import asynccontextmanager
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from app_under_test import AiteApp, run_app

from aite.contracts import (
    AiteConfig,
    Anchor,
    Attachment,
    CardAction,
    EventKind,
    EvidenceEvent,
    Message,
    NormalizedEvent,
    SenderKind,
    Task,
)
from aite.control import SqliteSessionStore
from aite.evidence import EVENTS_FILE, MANIFEST_FILE
from aite.testing import FakeModel, FakePlatform, FakeSandbox

REPO_ROOT = Path(__file__).resolve().parents[2]
PLATFORM_MD = REPO_ROOT / "aite" / "worker" / "prompts" / "platform.md"

CHAT = "oc_chat"
TENANT = "default"
SENDER = "ou_user"

#: 等一个条件成立的默认上限。这一轨全程跑在同一个事件循环上，正常几毫秒就到；
#: 给到秒级只是为了在慢机器上不误报，等不到就是真的没发生。
WAIT_TIMEOUT_SEC = 5.0


# --------------------------------------------------------------------------
# 配置与事件
# --------------------------------------------------------------------------

def make_config(tmp_path: Path, **overrides: Any) -> AiteConfig:
    """一份全落在 tmp_path 里的配置。system_prompt 走绝对路径，免得吃 cwd。"""
    cfg = AiteConfig.model_validate(
        {
            "tenant_id": TENANT,
            "platform": "fake",
            "storage": {
                "sqlite_path": str(tmp_path / "aite.db"),
                "evidence_dir": str(tmp_path / "evidence"),
                "artifacts_dir": str(tmp_path / "artifacts"),
            },
            "worker": {
                "system_prompt_path": str(PLATFORM_MD),
                # 卡片合并窗口置 0：这一轨要看的是「卡片有没有被更新」，
                # 不是 W4 的 500ms 合并（那条 T2 已经单测过了）。
                "card_update_min_interval_ms": 0,
            },
            "model": {"provider": "scripted", "model": "scripted-p0"},
        }
    )
    return cfg.model_copy(update=overrides) if overrides else cfg


def make_event(
    *,
    event_id: str = "e1",
    kind: EventKind = EventKind.message,
    text: str = "你好",
    mentioned: bool = True,
    sender_kind: SenderKind = SenderKind.human,
    sender_id: str = SENDER,
    sender_name: str | None = "张三",
    chat_id: str = CHAT,
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
        tenant_id=TENANT,
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


# --------------------------------------------------------------------------
# 替身
# --------------------------------------------------------------------------

class GatedPlatform(FakePlatform):
    """`stop()` 之后就不再往上投事件了 —— 长连接断了的真 adapter 就是这个行为。"""

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.dropped_after_stop = 0

    async def emit(self, ev: NormalizedEvent) -> None:
        if self.stopped:
            self.dropped_after_stop += 1
            return
        await super().emit(ev)


class ClosableFakeSandbox(FakeSandbox):
    """带 `aclose()` 的沙箱替身，语义照抄 `DockerSandbox.aclose`。"""

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.aclose_calls = 0

    async def aclose(self) -> None:
        self.aclose_calls += 1
        for sandbox_id in list(self.boxes):
            await self.release(sandbox_id)


class RecordingModel(FakeModel):
    """FakeModel + 每次 chat 收到的完整 messages。"""

    def __init__(self, script: Iterable[Any] | None = None, **kwargs: Any) -> None:
        super().__init__(list(script or []), **kwargs)
        self.prompts: list[list[Message]] = []

    async def chat(self, messages: list[Message], tools: list, **kwargs: Any):
        self.prompts.append(list(messages))
        return await super().chat(messages, tools, **kwargs)

    def prompt_texts(self) -> list[str]:
        """每次 chat 的上下文拼成一整段文本，方便断言「模型看见了什么」。"""
        return ["\n".join(m.content for m in prompt) for prompt in self.prompts]


# --------------------------------------------------------------------------
# 模型脚本片段
# --------------------------------------------------------------------------

def final_step(reply: str, artifacts: list[dict] | None = None, **extra: Any) -> dict:
    args: dict[str, Any] = {"reply": reply}
    if artifacts is not None:
        args["artifacts"] = artifacts
    return {"tool_calls": [{"name": "final", "arguments": args}], **extra}


def tool_step(name: str, arguments: dict | None = None, **extra: Any) -> dict:
    return {"tool_calls": [{"name": name, "arguments": arguments or {}}], **extra}


# --------------------------------------------------------------------------
# 驱动
# --------------------------------------------------------------------------

@dataclass
class RunningApp:
    """一个正在跑的 app。`stop` 是交给 `run_app` 的那个 Event，`runner` 是它的 task。"""

    app: AiteApp
    stop: asyncio.Event
    runner: asyncio.Task

    async def shutdown(self, *, timeout: float = 10.0) -> None:
        self.stop.set()
        await asyncio.wait_for(asyncio.shield(self.runner), timeout)


@asynccontextmanager
async def running_app(
    app: AiteApp,
    *,
    shutdown_grace_sec: float | None = None,
    exit_timeout: float = 10.0,
):
    """把 `run_app` 挂到后台跑，退出时保证 stop + 等它返回（超时就是挂死，直接红）。

    `shutdown_grace_sec` 不传就走 C-TΩ-1 的默认 20.0 —— 默认值本身也是被测面之一。
    """
    stop = asyncio.Event()
    kwargs: dict[str, Any] = {}
    if shutdown_grace_sec is not None:
        kwargs["shutdown_grace_sec"] = shutdown_grace_sec
    runner = asyncio.create_task(run_app(app, stop=stop, **kwargs))
    handle = RunningApp(app=app, stop=stop, runner=runner)
    try:
        await _wait_started(handle)
        yield handle
    finally:
        stop.set()
        await asyncio.wait_for(runner, exit_timeout)


async def _wait_started(handle: RunningApp, *, timeout: float = WAIT_TIMEOUT_SEC) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not getattr(handle.app.platform, "started", False):
        if handle.runner.done():
            await handle.runner                 # 里面真炸了的话，让原始异常冒出来
            raise AssertionError("run_app 还没调 platform.start 就返回了")
        if loop.time() >= deadline:
            raise AssertionError(f"{timeout}s 内 run_app 没有调 platform.start(ingress.on_event)")
        await asyncio.sleep(0)


async def wait_until(
    predicate: Callable[[], bool], *, what: str, timeout: float = WAIT_TIMEOUT_SEC
) -> None:
    """轮询等一个条件成立。等不到就是失败，不是「再等等」。"""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not predicate():
        if loop.time() >= deadline:
            raise AssertionError(f"{timeout}s 内没等到：{what}")
        await asyncio.sleep(0)


async def settle(ticks: int = 50) -> None:
    """让出若干个事件循环 tick。

    用在「断言某件事**没有**发生」之前：给系统足够的机会去做那件不该做的事。
    让的是 tick 不是墙钟，跑多快的机器上都是同一件事。
    """
    for _ in range(ticks):
        await asyncio.sleep(0)


# --------------------------------------------------------------------------
# 落盘结果的读取（全部从磁盘/新连接读，不看 app 手上的实例）
# --------------------------------------------------------------------------

async def read_task_from_disk(config: AiteConfig, task_id: str) -> Task | None:
    """用一条**新的** SQLite 连接把任务读回来。"""
    store = SqliteSessionStore(config.storage.sqlite_path)
    await store.init()
    try:
        return await store.get_task(task_id)
    finally:
        await store.close()


def evidence_task_dirs(config: AiteConfig) -> list[str]:
    """evidence 目录下有几个任务。"""
    root = Path(config.storage.evidence_dir)
    if not root.is_dir():
        return []
    return sorted(p.name for p in root.iterdir() if p.is_dir())


def events_path(config: AiteConfig, task_id: str) -> Path:
    return Path(config.storage.evidence_dir) / task_id / EVENTS_FILE


def manifest_path(config: AiteConfig, task_id: str) -> Path:
    return Path(config.storage.evidence_dir) / task_id / MANIFEST_FILE


def read_events(config: AiteConfig, task_id: str) -> list[EvidenceEvent]:
    lines = events_path(config, task_id).read_text(encoding="utf-8").splitlines()
    return [EvidenceEvent.model_validate_json(line) for line in lines]


def read_manifest(config: AiteConfig, task_id: str) -> dict:
    return json.loads(manifest_path(config, task_id).read_text(encoding="utf-8"))
