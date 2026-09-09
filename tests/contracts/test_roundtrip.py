"""§3.1 全部契约模型的 round-trip 测试（dev-spec-2026-09-09 §6 的 T0 行）。

验收原文要求："所有 3.1 模型的 JSON round-trip（OutboundFile 含 bytes，用 python 模式
model_dump()/model_validate() round-trip，不走 JSON）"。

本文件做三件事：
1. 为 §3.1 九个模块里的每一个 BaseModel 子类构造一个"字段填满"的样例（不是全默认）；
2. 除 OutboundFile 外走 model_dump_json() -> model_validate_json()，断言与原实例相等；
   OutboundFile 走 python 模式 model_dump() -> model_validate()，并断言 data 仍是 bytes；
3. 用 inspect 遍历这九个模块做"完整性守卫"——spec 将来加了模型而本文件没跟上就会红。
"""

from __future__ import annotations

import enum
import inspect
from datetime import UTC, datetime
from typing import Literal, get_args, get_origin

import pytest
from pydantic import BaseModel
from pydantic_core import PydanticSerializationError, PydanticUndefined

from aite.contracts import (
    capabilities,
    config,
    events,
    evidence,
    gateway,
    outbound,
    ports,
    protocol,
    sandbox,
    session,
)

# §3.1 的九个模块。ports.py 不在此列——它属于 §3.2 调用面，主体是 Protocol；
# 但它**也**声明了 HistoryMessage / DocumentContent 两个 BaseModel，
# 那两个由本文件末尾的 §3.2 小节单独覆盖，不混进 §3.1 的完整性守卫。
SPEC_31_MODULES = [
    capabilities,
    events,
    outbound,
    session,
    protocol,
    gateway,
    sandbox,
    evidence,
    config,
]

# 逐字节写死的模型名单：spec §3.1 每个模块自己定义（非 import 进来）的 BaseModel 子类。
# 将来 spec 增删模型时这张表会先红，逼着测试跟上契约。
EXPECTED_MODEL_NAMES: dict[str, tuple[str, ...]] = {
    "aite.contracts.capabilities": ("PlatformCapabilities",),
    "aite.contracts.events": ("Anchor", "Attachment", "CardAction", "NormalizedEvent"),
    "aite.contracts.outbound": (
        "ChecklistCard",
        "ChecklistItemView",
        "OutboundFile",
        "OutboundText",
        "SendResult",
    ),
    "aite.contracts.session": ("ChecklistItem", "Session", "Task", "Turn"),
    "aite.contracts.protocol": (
        "ArtifactRef",
        "Message",
        "ModelTurn",
        "ToolCallRequest",
        "ToolSpec",
        "Usage",
    ),
    "aite.contracts.gateway": ("ToolContext", "ToolError", "ToolResult"),
    "aite.contracts.sandbox": ("ExecRequest", "ExecResult", "FileEntry", "SandboxSpec"),
    "aite.contracts.evidence": ("EvidenceEvent",),
    "aite.contracts.config": (
        "AiteConfig",
        "FeishuConfig",
        "ModelConfig",
        "SandboxConfig",
        "StorageConfig",
        "WorkerConfig",
    ),
}

EXPECTED_MODEL_COUNT = 34

# —— 时间锚点：带时区，保证 JSON round-trip 后仍与原值相等（含微秒）——
T0 = datetime(2026, 9, 9, 9, 2, 15, 123456, tzinfo=UTC)
T1 = datetime(2026, 9, 9, 9, 4, 31, 654321, tzinfo=UTC)
T2 = datetime(2026, 9, 9, 9, 7, 0, 1, tzinfo=UTC)

# OutboundFile.data 故意用非 UTF-8 字节（PNG magic + 0xFF 0xFE），
# 这正是它不能走 JSON、必须走 python 模式 round-trip 的原因。
FILE_BYTES = b"\x89PNG\r\n\x1a\n\x00\xff\xfe report"


def _anchor() -> events.Anchor:
    return events.Anchor(
        platform="feishu",
        chat_id="oc_chat_001",
        message_id="om_msg_001",
        thread_id="om_thread_root_001",
        task_no="#AH",
    )


def _attachment() -> events.Attachment:
    return events.Attachment(
        kind="file",
        file_key="file_v3_abc123",
        message_id="om_msg_001",
        name="上月对账.xlsx",
        size=20480,
        mime="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    )


def _artifact_ref() -> protocol.ArtifactRef:
    return protocol.ArtifactRef(path="/work/out/report.md", title="对账报告", mime="text/markdown")


def _tool_call_request() -> protocol.ToolCallRequest:
    return protocol.ToolCallRequest(
        call_id="call_0001",
        name="run_python",
        arguments={"code": "print('嗨')", "timeout_sec": 30},
    )


def _message() -> protocol.Message:
    return protocol.Message(
        role="assistant",
        content="我先跑一段 Python 核对数字。",
        tool_calls=[_tool_call_request()],
        tool_call_id="call_0001",
        name="run_python",
    )


def _usage() -> protocol.Usage:
    return protocol.Usage(input_tokens=1234, output_tokens=567, cached_tokens=89)


def _feishu_config() -> config.FeishuConfig:
    return config.FeishuConfig(
        app_id_env="AITE_FEISHU_APP_ID",
        app_secret_env="AITE_FEISHU_APP_SECRET",
        bot_name="Aite-测试",
        bot_open_id_env="AITE_FEISHU_BOT_OPEN_ID",
        history_window=80,
    )


def _model_config() -> config.ModelConfig:
    return config.ModelConfig(
        provider="scripted",
        base_url="https://example.invalid/v1",
        api_key_env="AITE_TEST_MODEL_KEY",
        model="qwen-max-2026",
        max_tokens=8192,
        temperature=0.25,
        price_in_per_mtok=2.5,
        price_out_per_mtok=7.5,
    )


def _sandbox_config() -> config.SandboxConfig:
    return config.SandboxConfig(
        image="aite-sandbox:test",
        cpu=1.5,
        mem_mb=2048,
        idle_sec=600,
        exec_timeout_sec=90,
    )


def _worker_config() -> config.WorkerConfig:
    return config.WorkerConfig(
        max_steps=25,
        max_wall_sec=900,
        card_update_min_interval_ms=750,
        # 故意不用默认值 "aite/worker/prompts/platform.md"：字段若在序列化中掉了，
        # 默认值会把它悄悄填回同一个值，round-trip 照样绿（见 test_sample_value_differs_from_default）
        system_prompt_path="aite/worker/prompts/platform-test.md",
    )


def _storage_config() -> config.StorageConfig:
    return config.StorageConfig(
        sqlite_path="data/test-aite.db",
        evidence_dir="data/test-evidence",
        artifacts_dir="data/test-artifacts",
    )


def _build_samples() -> dict[type[BaseModel], BaseModel]:
    """每个模型都把**全部**字段显式填上（见 test_sample_fills_every_field 的机器校验）。"""
    return {
        # —— capabilities.py ——
        capabilities.PlatformCapabilities: capabilities.PlatformCapabilities(
            platform="feishu",
            supports_thread=True,
            supports_history=True,
            supports_passive_listen=False,
            supports_card_edit=True,
            card_edit_window_sec=1209600,
            inbound_file_in_group=True,
            proactive_requires_prior_message=False,
            outbound_rate_per_min=60,
        ),
        # —— events.py ——
        events.Anchor: _anchor(),
        events.Attachment: _attachment(),
        events.CardAction: events.CardAction(
            card_id="om_card_001",
            action="evidence",
            task_id="task-uuid-0001",
            value={"tag": "button", "extra": {"from": "card", "n": 3}},
        ),
        events.NormalizedEvent: events.NormalizedEvent(
            event_id="om_msg_001",
            kind=events.EventKind.card_action,
            platform="feishu",
            tenant_id="tenant-a",
            workspace_id="cli_app_0001",
            chat_id="oc_chat_001",
            chat_type="group",
            sender_id="ou_sender_001",
            sender_kind=events.SenderKind.human,
            sender_name="沈某",
            text="帮我把上月对账核一遍",
            raw_text="@Aite 帮我把上月对账核一遍",
            mentioned=True,
            anchor=_anchor(),
            attachments=[_attachment()],
            card_action=events.CardAction(
                card_id="om_card_001",
                action="stop",
                task_id="task-uuid-0001",
                value={"tag": "button"},
            ),
            occurred_at=T0,
            raw={"schema": "2.0", "header": {"event_type": "im.message.receive_v1"}, "n": [1, 2]},
        ),
        # —— outbound.py ——
        outbound.OutboundText: outbound.OutboundText(
            chat_id="oc_chat_001",
            text="**已完成**：3 处差异已列出。",
            reply_to="om_msg_001",
            in_thread=False,
        ),
        outbound.ChecklistItemView: outbound.ChecklistItemView(
            id="c1",
            text="下载对账附件",
            state="done",
            note="20 KB",
        ),
        outbound.ChecklistCard: outbound.ChecklistCard(
            task_id="task-uuid-0001",
            task_no="#AH",
            title="核对上月对账差异",
            initiator="沈某",
            started_at="9:02",
            status="working",
            items=[
                outbound.ChecklistItemView(id="c1", text="下载对账附件", state="done", note="20 KB"),
                outbound.ChecklistItemView(id="c2", text="比对金额", state="doing", note=None),
                outbound.ChecklistItemView(id="c3", text="输出差异表", state="todo", note=None),
            ],
            footer="预计 2 分钟 · 已用 ¥0.12",
            actions=["stop", "evidence"],
        ),
        outbound.OutboundFile: outbound.OutboundFile(
            chat_id="oc_chat_001",
            reply_to="om_msg_001",
            name="差异表.png",
            mime="image/png",
            data=FILE_BYTES,
        ),
        outbound.SendResult: outbound.SendResult(message_id="om_msg_002", card_id="om_card_001"),
        # —— session.py ——
        session.Turn: session.Turn(
            session_id="sess-uuid-0001",
            seq=7,
            role="assistant",
            platform_user_id="ou_sender_001",
            content="我先看一眼附件。",
            attachments=[_attachment()],
            created_at=T1,
        ),
        session.ChecklistItem: session.ChecklistItem(
            id="c1",
            text="下载对账附件",
            state="done",
            note="20 KB",
        ),
        session.Session: session.Session(
            id="sess-uuid-0001",
            tenant_id="tenant-a",
            workspace_id="cli_app_0001",
            chat_id="oc_chat_001",
            kind=session.SessionKind.task,
            anchor=_anchor(),
            status=session.SessionStatus.idle,
            created_by="ou_sender_001",
            config_snapshot={"model": "qwen-max-2026", "prompt_sha256": "deadbeef"},
            created_at=T0,
            last_active_at=T1,
            archived_at=T2,
        ),
        session.Task: session.Task(
            id="task-uuid-0001",
            session_id="sess-uuid-0001",
            task_no="#AH",
            status=session.TaskStatus.working,
            title="核对上月对账差异",
            checklist=[
                session.ChecklistItem(id="c1", text="下载对账附件", state="done", note="20 KB"),
                session.ChecklistItem(id="c2", text="比对金额", state="failed", note="缺 3 行"),
            ],
            card_id="om_card_001",
            sandbox_id="sbx-0001",
            session_token="0123456789abcdef0123456789abcdef",
            model="qwen-max-2026",
            steps=5,
            tokens_in=1234,
            tokens_out=567,
            cost=0.1234,
            max_steps=25,
            max_wall_sec=900,
            result_summary="发现 3 处差异",
            evidence_root_hash="cdae94bd29b7de0c2a7af392f4d984ed5a41b15bdd872f7b9726132508047adc",
            created_by="ou_sender_001",
            created_at=T0,
            updated_at=T1,
        ),
        # —— protocol.py ——
        protocol.ToolSpec: protocol.ToolSpec(
            name="run_python",
            description="在隔离沙箱里执行 Python（无网络）",
            parameters={
                "type": "object",
                "properties": {"code": {"type": "string"}},
                "required": ["code"],
            },
        ),
        protocol.ToolCallRequest: _tool_call_request(),
        protocol.ArtifactRef: _artifact_ref(),
        protocol.Message: _message(),
        protocol.Usage: _usage(),
        protocol.ModelTurn: protocol.ModelTurn(
            message=_message(),
            usage=_usage(),
            finish_reason="tool_calls",
            raw={"id": "chatcmpl-1", "choices": [{"index": 0}]},
        ),
        # —— gateway.py ——
        gateway.ToolError: gateway.ToolError(
            code=gateway.ToolErrorCode.timeout,
            message="沙箱执行超时",
        ),
        gateway.ToolContext: gateway.ToolContext(
            tenant_id="tenant-a",
            workspace_id="cli_app_0001",
            chat_id="oc_chat_001",
            session_id="sess-uuid-0001",
            task_id="task-uuid-0001",
            session_token="0123456789abcdef0123456789abcdef",
            thread_id="om_thread_root_001",
            attachments_message_id="om_msg_001",
        ),
        gateway.ToolResult: gateway.ToolResult(
            call_id="call_0001",
            name="run_python",
            ok=False,
            content="差异 3 行，见 /work/out/report.md",
            data={"rows": 3, "paths": ["/work/out/report.md"]},
            error=gateway.ToolError(code=gateway.ToolErrorCode.sandbox, message="退出码 1"),
            duration_ms=1450,
            artifacts=[_artifact_ref()],
        ),
        # —— sandbox.py ——
        sandbox.SandboxSpec: sandbox.SandboxSpec(
            image="aite-sandbox:test",
            cpu=1.5,
            mem_mb=2048,
            network="none",   # Literal 单值，无法取非默认值
            workdir="/work/sbx-0001",
        ),
        sandbox.ExecRequest: sandbox.ExecRequest(
            language="python",
            code="print('嗨')",
            timeout_sec=30,
        ),
        sandbox.FileEntry: sandbox.FileEntry(path="/work/out/report.md", size=4096),
        sandbox.ExecResult: sandbox.ExecResult(
            exit_code=1,
            stdout="嗨\n",
            stderr="Traceback…\n",
            duration_ms=1450,
            truncated=True,
            files_out=[
                sandbox.FileEntry(path="/work/out/report.md", size=4096),
                sandbox.FileEntry(path="/work/out/差异表.png", size=20480),
            ],
        ),
        # —— evidence.py ——
        # payload / payload_hash / hash 用 §3.1 注释里的测试向量，逐字节抄。
        evidence.EvidenceEvent: evidence.EvidenceEvent(
            task_id="task-uuid-0001",
            seq=0,
            kind=evidence.EvidenceKind.tool_result,
            payload_hash="015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862",
            payload_ref="payloads/0.json",
            payload={"a": 1},
            prev_hash=evidence.GENESIS,
            hash="cdae94bd29b7de0c2a7af392f4d984ed5a41b15bdd872f7b9726132508047adc",
            created_at=T0,
        ),
        # —— config.py ——
        config.FeishuConfig: _feishu_config(),
        config.ModelConfig: _model_config(),
        config.SandboxConfig: _sandbox_config(),
        config.WorkerConfig: _worker_config(),
        config.StorageConfig: _storage_config(),
        config.AiteConfig: config.AiteConfig(
            tenant_id="tenant-a",
            platform="fake",
            feishu=_feishu_config(),
            model=_model_config(),
            sandbox=_sandbox_config(),
            worker=_worker_config(),
            storage=_storage_config(),
        ),
    }


SAMPLES: dict[type[BaseModel], BaseModel] = _build_samples()

# OutboundFile 含 bytes，按 §6 的 T0 行单独走 python 模式，不进 JSON 参数表。
JSON_CASES = [(cls, obj) for cls, obj in SAMPLES.items() if cls is not outbound.OutboundFile]
JSON_IDS = [f"{cls.__module__.rsplit('.', 1)[-1]}.{cls.__name__}" for cls, _ in JSON_CASES]
ALL_CASES = list(SAMPLES.items())
ALL_IDS = [f"{cls.__module__.rsplit('.', 1)[-1]}.{cls.__name__}" for cls, _ in ALL_CASES]


def _declared_models(module: object) -> dict[str, type[BaseModel]]:
    """模块里**自己定义**的 BaseModel 子类；从别处 import 进来的（__module__ 不同）不算。"""
    found: dict[str, type[BaseModel]] = {}
    for name, obj in inspect.getmembers(module, inspect.isclass):
        if issubclass(obj, BaseModel) and obj is not BaseModel and obj.__module__ == module.__name__:
            found[name] = obj
    return found


def _enum_fields(sample: BaseModel) -> list[tuple[str, enum.Enum]]:
    """样例里取值为枚举成员的字段（含 StrEnum）。"""
    out: list[tuple[str, enum.Enum]] = []
    for fname in type(sample).model_fields:
        value = getattr(sample, fname)
        if isinstance(value, enum.Enum):
            out.append((fname, value))
    return out


def _default_of(field: object) -> tuple[bool, object]:
    """(有没有默认值, 默认值)。default_factory 的每次调用结果算它的默认值。"""
    if field.default is not PydanticUndefined:
        return True, field.default
    if field.default_factory is not None:
        return True, field.default_factory()
    return False, None


def _is_single_value_literal(annotation: object) -> bool:
    """Literal["none"] 这种只有一个取值的字段，样例不可能取到非默认值。"""
    return get_origin(annotation) is Literal and len(get_args(annotation)) == 1


@pytest.mark.parametrize(("model_cls", "sample"), JSON_CASES, ids=JSON_IDS)
def test_json_roundtrip(model_cls: type[BaseModel], sample: BaseModel) -> None:
    """§6 T0：除 OutboundFile 外，所有 §3.1 模型都能 model_dump_json -> model_validate_json 复原。"""
    dumped = sample.model_dump_json()
    assert isinstance(dumped, str)
    restored = model_cls.model_validate_json(dumped)
    assert type(restored) is model_cls
    # 相等：pydantic v2 的 __eq__ 逐字段比对（含嵌套模型、datetime、StrEnum）
    assert restored == sample
    # 再 dump 一次必须逐字节相同，排除"两边都错成同一个默认值"的假绿
    assert restored.model_dump_json() == dumped
    # python 模式的字段字典也必须完全一致
    assert restored.model_dump() == sample.model_dump()
    # StrEnum 降级成裸 str 的话，上面三个断言全都抓不到——
    # StrEnum("card_action") == "card_action" 为 True，model_dump() 两边也都是同一个字符串。
    # 所以枚举字段必须单独比"类型本身"。
    for fname, original in _enum_fields(sample):
        assert type(getattr(restored, fname)) is type(original), fname


def test_outbound_file_python_mode_roundtrip() -> None:
    """OutboundFile 含 bytes 字段 data：按 §6 T0 走 python 模式 round-trip，不走 JSON。"""
    sample = SAMPLES[outbound.OutboundFile]
    dumped = sample.model_dump()

    # 关键断言：python 模式下 data 仍是 bytes，没有被顺手转成 str
    assert isinstance(dumped["data"], bytes)
    assert not isinstance(dumped["data"], str)
    assert dumped["data"] == FILE_BYTES
    assert dumped["data"] == b"\x89PNG\r\n\x1a\n\x00\xff\xfe report"
    assert len(dumped["data"]) == 18

    # 其余字段逐字段写死
    assert dumped == {
        "chat_id": "oc_chat_001",
        "reply_to": "om_msg_001",
        "name": "差异表.png",
        "mime": "image/png",
        "data": FILE_BYTES,
    }

    restored = outbound.OutboundFile.model_validate(dumped)
    assert type(restored) is outbound.OutboundFile
    assert restored == sample
    assert restored.data == FILE_BYTES
    assert isinstance(restored.data, bytes)
    assert restored.model_dump() == dumped


def test_outbound_file_data_is_not_utf8_decodable() -> None:
    """佐证 OutboundFile 为什么必须走 python 模式：这串 bytes 根本不是合法 UTF-8。"""
    with pytest.raises(UnicodeDecodeError):
        FILE_BYTES.decode("utf-8")


def test_outbound_file_json_serialization_actually_fails() -> None:
    """直接坐实 §6 T0 "不走 JSON" 这句：OutboundFile 走 JSON 是真的会炸，不是风格偏好。

    上一条只证明了这串 bytes 不是合法 UTF-8（间接佐证）。这一条证明 pydantic 确实
    因此拒绝序列化——否则"OutboundFile 单独走 python 模式"就成了没有依据的例外。
    """
    sample = SAMPLES[outbound.OutboundFile]
    with pytest.raises(PydanticSerializationError):
        sample.model_dump_json()
    with pytest.raises(UnicodeDecodeError):
        sample.model_dump(mode="json")


@pytest.mark.parametrize(("model_cls", "sample"), ALL_CASES, ids=ALL_IDS)
def test_sample_fills_every_field(model_cls: type[BaseModel], sample: BaseModel) -> None:
    """样例必须"字段填满"：每个字段都显式传过值，不能靠默认值蒙混过 round-trip。"""
    assert sample.model_fields_set == set(model_cls.model_fields)


@pytest.mark.parametrize(("model_cls", "sample"), ALL_CASES, ids=ALL_IDS)
def test_sample_value_differs_from_default(model_cls: type[BaseModel], sample: BaseModel) -> None:
    """样例取值必须**不等于**字段默认值，否则 round-trip 会出现假绿。

    只有"显式传过值"是不够的：若某字段在序列化里整个掉了，反序列化时默认值会把它
    悄悄填回同一个值，`restored == sample` 依旧成立，测试照绿。取非默认值才能让
    "字段掉了"这件事真的变红。单值 Literal（如 network: Literal["none"]）没有别的
    取值可选，是唯一允许等于默认值的例外。
    """
    same_as_default = []
    for fname, field in model_cls.model_fields.items():
        has_default, default = _default_of(field)
        if not has_default or _is_single_value_literal(field.annotation):
            continue
        if getattr(sample, fname) == default:
            same_as_default.append(fname)
    assert same_as_default == [], (
        f"{model_cls.__name__} 的这些字段取值等于默认值，掉字段也验不出来: {same_as_default}"
    )


def test_every_spec_31_model_has_a_sample() -> None:
    """完整性守卫：§3.1 九个模块里每个自定义 BaseModel 子类都必须在 SAMPLES 里出现。"""
    discovered: dict[str, type[BaseModel]] = {}
    for module in SPEC_31_MODULES:
        declared = _declared_models(module)
        # 逐模块对照写死的名单，spec 增删模型时这里先红
        assert tuple(sorted(declared)) == EXPECTED_MODEL_NAMES[module.__name__], module.__name__
        for name, cls in declared.items():
            discovered[f"{module.__name__}.{name}"] = cls

    assert len(discovered) == EXPECTED_MODEL_COUNT
    assert len(SAMPLES) == EXPECTED_MODEL_COUNT

    missing = sorted(key for key, cls in discovered.items() if cls not in SAMPLES)
    assert missing == [], f"§3.1 新增模型未覆盖 round-trip: {missing}"

    # 反向：SAMPLES 里不许有 §3.1 之外的类混进来
    extra = sorted(
        f"{cls.__module__}.{cls.__name__}" for cls in SAMPLES if cls not in set(discovered.values())
    )
    assert extra == []

    # 每个样例都必须是它自己那个类的实例（防止表里写错 key）
    for cls, sample in SAMPLES.items():
        assert type(sample) is cls


def test_imported_models_are_not_counted_as_declared() -> None:
    """守卫用的 __module__ 过滤真的在起作用：session/gateway 里 import 来的类不算它们自己的。"""
    # session.py 从 events.py import 了 Anchor / Attachment
    assert session.Anchor is events.Anchor
    assert session.Attachment is events.Attachment
    assert "Anchor" not in _declared_models(session)
    assert "Attachment" not in _declared_models(session)
    # gateway.py 从 protocol.py import 了 ArtifactRef
    assert gateway.ArtifactRef is protocol.ArtifactRef
    assert "ArtifactRef" not in _declared_models(gateway)
    assert "ArtifactRef" in _declared_models(protocol)


# ——————————————————————————————————————————————————————————————————————
# §3.2 ports.py：主体是 Protocol，但它也声明了两个 BaseModel。
# 它们跨 track 传（PlatformPort.read_history / read_document 的返回形状），
# 却不在 §6 T0 的 "3.1 模型" 字面范围内，全仓库此前零覆盖。补上，顺带钉住
# "ports.py 里只有这两个 BaseModel"——将来谁把 §3.1 的模型挪进来，这里先红。
# ——————————————————————————————————————————————————————————————————————

PORTS_SAMPLES: dict[type[BaseModel], BaseModel] = {
    ports.HistoryMessage: ports.HistoryMessage(
        message_id="om_msg_001",
        sender_id="ou_sender_001",
        sender_kind="human",
        sender_name="沈某",
        text="帮我把上月对账核一遍",
        thread_id="om_thread_root_001",
        created_at=T0,
    ),
    ports.DocumentContent: ports.DocumentContent(
        title="上月对账说明",
        text="# 对账说明\n\n- 口径：应收\n",
        url="https://example.invalid/docx/doccnAbc123",
    ),
}

PORTS_CASES = list(PORTS_SAMPLES.items())
PORTS_IDS = [f"ports.{cls.__name__}" for cls, _ in PORTS_CASES]


@pytest.mark.parametrize(("model_cls", "sample"), PORTS_CASES, ids=PORTS_IDS)
def test_ports_json_roundtrip(model_cls: type[BaseModel], sample: BaseModel) -> None:
    """§3.2 的两个 BaseModel 同样要能 JSON round-trip（断言口径与 §3.1 完全一致）。"""
    dumped = sample.model_dump_json()
    restored = model_cls.model_validate_json(dumped)
    assert type(restored) is model_cls
    assert restored == sample
    assert restored.model_dump_json() == dumped
    assert restored.model_dump() == sample.model_dump()
    assert sample.model_fields_set == set(model_cls.model_fields)


def test_ports_declares_exactly_two_models() -> None:
    """钉住 ports.py 的 BaseModel 名单，且它们都有样例。"""
    declared = _declared_models(ports)
    assert tuple(sorted(declared)) == ("DocumentContent", "HistoryMessage")
    assert set(declared.values()) == set(PORTS_SAMPLES)
    # §3.1 的完整性守卫不该把 ports.py 的模型算进去
    assert ports not in SPEC_31_MODULES
    for cls in declared.values():
        assert cls not in SAMPLES
