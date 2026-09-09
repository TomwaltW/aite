"""场景文件（evals/p0/*.yaml）的形状与加载（dev-spec §3.8）。

一个场景 = 「喂什么」+「模型怎么出牌」+「该看到什么」：

```yaml
name: 03_checklist_progress
title: 卡片进度原地更新
verifies: send_card×1、update_card>=3、无第二条卡片
platform:            # FakePlatform 的初始数据：群历史 / 云文档 / 附件
sandbox:             # FakeSandbox 的 exec 脚本
events:              # 按顺序投给 ControlPlane 的归一化事件
model_script:        # FakeModel 每一步出什么牌
expect:              # 断言清单，语义见 aite/evals/checks.py
```

字段默认值给得很足，场景 yaml 里只写关心的那几个 —— 场景文件是给人读的，
不该被十几行样板淹掉。
"""
from __future__ import annotations

from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any, Literal

import yaml
from pydantic import BaseModel, Field

from ..contracts import (
    Anchor,
    Attachment,
    CardAction,
    EventKind,
    NormalizedEvent,
    SenderKind,
)
from ..contracts.ports import DocumentContent, HistoryMessage
from ..testing.fake_model import ScriptStep
from ..testing.fake_sandbox import ExecScriptStep, as_bytes

#: 所有时间戳从这里起算，场景之间可复现
BASE_TIME = datetime(2026, 9, 9, 9, 0, 0, tzinfo=UTC)

DEFAULT_CHAT_ID = "oc_demo"
DEFAULT_WORKSPACE_ID = "cli_fake_app"


class ScenarioError(ValueError):
    """场景文件本身写错了（缺字段、名字对不上目录等）。"""


class AttachmentSpec(BaseModel):
    kind: Literal["image", "file"] = "file"
    file_key: str
    name: str | None = None
    size: int | None = None
    mime: str | None = None


class EventSpec(BaseModel):
    """一条投给 ControlPlane 的事件。默认是「群里 @Aite 的一条真人消息」。"""

    event_id: str
    kind: EventKind = EventKind.message
    text: str = ""
    raw_text: str | None = None
    mentioned: bool = True
    sender_id: str = "ou_alice"
    sender_kind: SenderKind = SenderKind.human
    sender_name: str | None = "Alice"
    chat_id: str = DEFAULT_CHAT_ID
    chat_type: Literal["group", "p2p"] = "group"
    workspace_id: str = DEFAULT_WORKSPACE_ID
    tenant_id: str = "default"
    #: 触发消息 id；不写就按 event_id 派生
    message_id: str | None = None
    thread_id: str | None = None
    task_no: str | None = None
    attachments: list[AttachmentSpec] = Field(default_factory=list)
    card_action: CardAction | None = None
    #: 相对 BASE_TIME 的秒偏移；不写就用事件在列表里的下标
    at_sec: int | None = None

    def build(self, index: int) -> NormalizedEvent:
        message_id = self.message_id or f"om_{self.event_id}"
        return NormalizedEvent(
            event_id=self.event_id,
            kind=self.kind,
            platform="fake",
            tenant_id=self.tenant_id,
            workspace_id=self.workspace_id,
            chat_id=self.chat_id,
            chat_type=self.chat_type,
            sender_id=self.sender_id,
            sender_kind=self.sender_kind,
            sender_name=self.sender_name,
            text=self.text,
            raw_text=self.raw_text,
            mentioned=self.mentioned,
            anchor=Anchor(
                platform="fake",
                chat_id=self.chat_id,
                message_id=message_id,
                thread_id=self.thread_id,
                task_no=self.task_no,
            ),
            attachments=[
                Attachment(
                    kind=a.kind,
                    file_key=a.file_key,
                    message_id=message_id,
                    name=a.name,
                    size=a.size,
                    mime=a.mime,
                )
                for a in self.attachments
            ],
            card_action=self.card_action,
            occurred_at=BASE_TIME + timedelta(seconds=self.at_sec if self.at_sec is not None else index),
        )


class HistorySpec(BaseModel):
    message_id: str
    text: str
    sender_id: str = "ou_someone"
    sender_kind: str = "human"
    sender_name: str | None = None
    thread_id: str | None = None
    at_sec: int | None = None

    def build(self, index: int) -> HistoryMessage:
        return HistoryMessage(
            message_id=self.message_id,
            sender_id=self.sender_id,
            sender_kind=self.sender_kind,
            sender_name=self.sender_name if self.sender_name is not None else self.sender_id,
            text=self.text,
            thread_id=self.thread_id,
            created_at=BASE_TIME
            + timedelta(seconds=-600 + (self.at_sec if self.at_sec is not None else index)),
        )


class DocumentSpec(BaseModel):
    #: read_document 的 url_or_token 用哪个键命中
    key: str
    title: str
    text: str = ""
    url: str | None = None

    def build(self) -> DocumentContent:
        return DocumentContent(title=self.title, text=self.text, url=self.url or self.key)


class FileSpec(BaseModel):
    """群消息里的一个附件：download_file(message_id, file_key) 能取到它。"""

    message_id: str
    file_key: str
    content: Any = ""

    def data(self) -> bytes:
        return as_bytes(self.content)


class PlatformFixture(BaseModel):
    history: list[HistorySpec] = Field(default_factory=list)
    documents: list[DocumentSpec] = Field(default_factory=list)
    files: list[FileSpec] = Field(default_factory=list)


class SandboxFixture(BaseModel):
    exec_script: list[ExecScriptStep] = Field(default_factory=list)


class Scenario(BaseModel):
    name: str
    title: str = ""
    verifies: str = ""
    spec_ref: str = ""
    #: 覆盖 AiteConfig 的片段，例如 {"worker": {"max_steps": 3}}
    config: dict[str, Any] = Field(default_factory=dict)
    platform: PlatformFixture = Field(default_factory=PlatformFixture)
    sandbox: SandboxFixture = Field(default_factory=SandboxFixture)
    model_script: list[ScriptStep] = Field(default_factory=list)
    events: list[EventSpec] = Field(default_factory=list)
    expect: list[dict[str, Any]] = Field(default_factory=list)
    #: 单个场景的驱动上限（秒）
    timeout_sec: float = 10.0
    #: 来源文件，加载时填
    source: str = ""

    def build_events(self) -> list[NormalizedEvent]:
        return [e.build(i) for i, e in enumerate(self.events)]

    def build_history(self) -> list[HistoryMessage]:
        return [h.build(i) for i, h in enumerate(self.platform.history)]

    def build_documents(self) -> dict[str, DocumentContent]:
        return {d.key: d.build() for d in self.platform.documents}

    def build_files(self) -> dict[tuple[str, str], bytes]:
        return {(f.message_id, f.file_key): f.data() for f in self.platform.files}


def load_scenario(path: Path) -> Scenario:
    raw = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        raise ScenarioError(f"{path}: 顶层必须是 mapping，实际是 {type(raw).__name__}")
    raw.setdefault("name", path.stem)
    if raw["name"] != path.stem:
        raise ScenarioError(f"{path}: name={raw['name']!r} 与文件名 {path.stem!r} 不一致")
    sc = Scenario.model_validate(raw)
    sc.source = str(path)
    return sc


def load_suite(suite_dir: str | Path) -> list[Scenario]:
    """按文件名排序加载一个目录下的全部场景（*.yaml / *.yml）。"""
    root = Path(suite_dir)
    if not root.is_dir():
        raise ScenarioError(f"场景目录不存在：{root}")
    paths = sorted(p for p in root.iterdir() if p.suffix in (".yaml", ".yml"))
    if not paths:
        raise ScenarioError(f"{root} 下没有 *.yaml 场景文件")
    return [load_scenario(p) for p in paths]
