"""§3.1 取值面的整片冻结（dev-spec-2026-09-09 §3 「以下定义在本轮内不可更改」）。

为什么需要这一份：round-trip 测试只证明「写进去能读出来」，证明不了取值没被改。
把 DEFAULT_TOOL_TIMEOUT_SEC 从 60 改成 6、把 Task.max_steps 从 40 改成 400、
把 SandboxSpec.network 的 Literal 从 "none" 改成别的，round-trip 全部照绿；
.contracts.lock 也拦不住 —— 跑一次 `--write` 它就重新绿了。
（pydantic v2 默认 validate_default=False，连它自己都不会兜底 Literal 默认值。）

所以这里把**每个字段的默认值、每个 Literal 的取值集合、每个 StrEnum 的成员、
每个模块级常量**逐条钉成字面量。谁动 §3 的取值，这里先红。

表是机器生成的（T0），改契约后要同步刷新 —— 但那正是「停下来找总管」的时刻。
"""
import importlib
import inspect
import typing
from enum import StrEnum

import pytest
from pydantic import BaseModel
from pydantic_core import PydanticUndefined

MODULES = ['capabilities', 'events', 'outbound', 'session', 'protocol', 'gateway', 'sandbox', 'evidence', 'config']
REQUIRED = "<required>"

# --- 冻结表：字段默认值 ---------------------------------------------------
FROZEN_FIELDS = {
    'capabilities.PlatformCapabilities': {
        'platform': '<required>',
        'supports_thread': '<required>',
        'supports_history': '<required>',
        'supports_passive_listen': '<required>',
        'supports_card_edit': '<required>',
        'card_edit_window_sec': '<required>',
        'inbound_file_in_group': '<required>',
        'proactive_requires_prior_message': '<required>',
        'outbound_rate_per_min': '<required>',
    },
    'events.Anchor': {
        'platform': '<required>',
        'chat_id': '<required>',
        'message_id': '<required>',
        'thread_id': None,
        'task_no': None,
    },
    'events.Attachment': {
        'kind': '<required>',
        'file_key': '<required>',
        'message_id': '<required>',
        'name': None,
        'size': None,
        'mime': None,
    },
    'events.CardAction': {
        'card_id': '<required>',
        'action': '<required>',
        'task_id': None,
        'value': {},
    },
    'events.NormalizedEvent': {
        'event_id': '<required>',
        'kind': '<required>',
        'platform': '<required>',
        'tenant_id': 'default',
        'workspace_id': '<required>',
        'chat_id': '<required>',
        'chat_type': '<required>',
        'sender_id': '<required>',
        'sender_kind': '<required>',
        'sender_name': None,
        'text': '<required>',
        'raw_text': None,
        'mentioned': '<required>',
        'anchor': '<required>',
        'attachments': [],
        'card_action': None,
        'occurred_at': '<required>',
        'raw': {},
    },
    'outbound.OutboundText': {
        'chat_id': '<required>',
        'text': '<required>',
        'reply_to': None,
        'in_thread': True,
    },
    'outbound.ChecklistItemView': {
        'id': '<required>',
        'text': '<required>',
        'state': '<required>',
        'note': None,
    },
    'outbound.ChecklistCard': {
        'task_id': '<required>',
        'task_no': '<required>',
        'title': '<required>',
        'initiator': '<required>',
        'started_at': '<required>',
        'status': '<required>',
        'items': [],
        'footer': '',
        'actions': ['stop'],
    },
    'outbound.OutboundFile': {
        'chat_id': '<required>',
        'reply_to': '<required>',
        'name': '<required>',
        'mime': '<required>',
        'data': '<required>',
    },
    'outbound.SendResult': {
        'message_id': '<required>',
        'card_id': None,
    },
    'session.Turn': {
        'session_id': '<required>',
        'seq': '<required>',
        'role': '<required>',
        'platform_user_id': '<required>',
        'content': '<required>',
        'attachments': [],
        'created_at': '<required>',
    },
    'session.ChecklistItem': {
        'id': '<required>',
        'text': '<required>',
        'state': 'todo',
        'note': None,
    },
    'session.Session': {
        'id': '<required>',
        'tenant_id': '<required>',
        'workspace_id': '<required>',
        'chat_id': '<required>',
        'kind': '<required>',
        'anchor': '<required>',
        'status': 'SessionStatus.active',
        'created_by': '<required>',
        'config_snapshot': {},
        'created_at': '<required>',
        'last_active_at': '<required>',
        'archived_at': None,
    },
    'session.Task': {
        'id': '<required>',
        'session_id': '<required>',
        'task_no': '<required>',
        'status': 'TaskStatus.created',
        'title': '',
        'checklist': [],
        'card_id': None,
        'sandbox_id': None,
        'session_token': '<required>',
        'model': '',
        'steps': 0,
        'tokens_in': 0,
        'tokens_out': 0,
        'cost': 0.0,
        'max_steps': 40,
        'max_wall_sec': 1200,
        'result_summary': '',
        'evidence_root_hash': None,
        'created_by': '<required>',
        'created_at': '<required>',
        'updated_at': '<required>',
    },
    'protocol.ToolSpec': {
        'name': '<required>',
        'description': '<required>',
        'parameters': '<required>',
    },
    'protocol.ToolCallRequest': {
        'call_id': '<required>',
        'name': '<required>',
        'arguments': {},
    },
    'protocol.ArtifactRef': {
        'path': '<required>',
        'title': '<required>',
        'mime': None,
    },
    'protocol.Message': {
        'role': '<required>',
        'content': '',
        'tool_calls': None,
        'tool_call_id': None,
        'name': None,
    },
    'protocol.Usage': {
        'input_tokens': 0,
        'output_tokens': 0,
        'cached_tokens': 0,
    },
    'protocol.ModelTurn': {
        'message': '<required>',
        'usage': ('Usage', {'input_tokens': 0, 'output_tokens': 0, 'cached_tokens': 0}),
        'finish_reason': 'stop',
        'raw': {},
    },
    'gateway.ToolError': {
        'code': '<required>',
        'message': '<required>',
    },
    'gateway.ToolContext': {
        'tenant_id': '<required>',
        'workspace_id': '<required>',
        'chat_id': '<required>',
        'session_id': '<required>',
        'task_id': '<required>',
        'session_token': '<required>',
        'thread_id': None,
        'attachments_message_id': None,
    },
    'gateway.ToolResult': {
        'call_id': '<required>',
        'name': '<required>',
        'ok': '<required>',
        'content': '<required>',
        'data': None,
        'error': None,
        'duration_ms': 0,
        'artifacts': [],
    },
    'sandbox.SandboxSpec': {
        'image': '<required>',
        'cpu': 1.0,
        'mem_mb': 1024,
        'network': 'none',
        'workdir': '/work',
    },
    'sandbox.ExecRequest': {
        'language': 'python',
        'code': '<required>',
        'timeout_sec': 120,
    },
    'sandbox.FileEntry': {
        'path': '<required>',
        'size': '<required>',
    },
    'sandbox.ExecResult': {
        'exit_code': '<required>',
        'stdout': '<required>',
        'stderr': '<required>',
        'duration_ms': '<required>',
        'truncated': False,
        'files_out': [],
    },
    'evidence.EvidenceEvent': {
        'task_id': '<required>',
        'seq': '<required>',
        'kind': '<required>',
        'payload_hash': '<required>',
        'payload_ref': '<required>',
        'payload': '<required>',
        'prev_hash': '<required>',
        'hash': '<required>',
        'created_at': '<required>',
    },
    'config.FeishuConfig': {
        'app_id_env': 'FEISHU_APP_ID',
        'app_secret_env': 'FEISHU_APP_SECRET',
        'bot_name': 'Aite',
        'bot_open_id_env': 'FEISHU_BOT_OPEN_ID',
        'history_window': 50,
    },
    'config.ModelConfig': {
        'provider': 'openai_compat',
        'base_url': '',
        'api_key_env': 'AITE_MODEL_API_KEY',
        'model': '',
        'max_tokens': 4096,
        'temperature': 0.0,
        'price_in_per_mtok': 0.0,
        'price_out_per_mtok': 0.0,
    },
    'config.SandboxConfig': {
        'image': 'aite-sandbox:p0',
        'cpu': 1.0,
        'mem_mb': 1024,
        'idle_sec': 300,
        'exec_timeout_sec': 120,
    },
    'config.WorkerConfig': {
        'max_steps': 40,
        'max_wall_sec': 1200,
        'card_update_min_interval_ms': 500,
        'system_prompt_path': 'aite/worker/prompts/platform.md',
    },
    'config.StorageConfig': {
        'sqlite_path': 'data/aite.db',
        'evidence_dir': 'data/evidence',
        'artifacts_dir': 'data/artifacts',
    },
    'config.AiteConfig': {
        'tenant_id': 'default',
        'platform': 'feishu',
        'feishu': ('FeishuConfig', {'app_id_env': 'FEISHU_APP_ID', 'app_secret_env': 'FEISHU_APP_SECRET', 'bot_name': 'Aite', 'bot_open_id_env': 'FEISHU_BOT_OPEN_ID', 'history_window': 50}),
        'model': ('ModelConfig', {'provider': 'openai_compat', 'base_url': '', 'api_key_env': 'AITE_MODEL_API_KEY', 'model': '', 'max_tokens': 4096, 'temperature': 0.0, 'price_in_per_mtok': 0.0, 'price_out_per_mtok': 0.0}),
        'sandbox': ('SandboxConfig', {'image': 'aite-sandbox:p0', 'cpu': 1.0, 'mem_mb': 1024, 'idle_sec': 300, 'exec_timeout_sec': 120}),
        'worker': ('WorkerConfig', {'max_steps': 40, 'max_wall_sec': 1200, 'card_update_min_interval_ms': 500, 'system_prompt_path': 'aite/worker/prompts/platform.md'}),
        'storage': ('StorageConfig', {'sqlite_path': 'data/aite.db', 'evidence_dir': 'data/evidence', 'artifacts_dir': 'data/artifacts'}),
    },
}

# --- 冻结表：Literal 取值集合 ---------------------------------------------
FROZEN_LITERALS = {
    'capabilities.PlatformCapabilities.platform': ['feishu', 'dingtalk', 'wecom', 'fake'],
    'events.Attachment.kind': ['image', 'file'],
    'events.CardAction.action': ['stop', 'evidence'],
    'events.NormalizedEvent.chat_type': ['group', 'p2p'],
    'outbound.ChecklistItemView.state': ['todo', 'doing', 'done', 'failed'],
    'outbound.ChecklistCard.status': ['working', 'delivered', 'failed', 'cancelled'],
    'session.Turn.role': ['user', 'assistant', 'system_note'],
    'session.ChecklistItem.state': ['todo', 'doing', 'done', 'failed'],
    'protocol.Message.role': ['system', 'user', 'assistant', 'tool'],
    'sandbox.SandboxSpec.network': ['none'],
    'sandbox.ExecRequest.language': ['python'],
    'config.ModelConfig.provider': ['openai_compat', 'scripted'],
    'config.AiteConfig.platform': ['feishu', 'fake'],
    'capabilities.Platform': ['feishu', 'dingtalk', 'wecom', 'fake'],
    'outbound.ReactionKind': ['ack', 'done', 'fail'],
}

# --- 冻结表：StrEnum 成员 -------------------------------------------------
FROZEN_ENUMS = {
    'events.SenderKind': {'human': 'human', 'bot': 'bot', 'app': 'app', 'system': 'system'},
    'events.EventKind': {'message': 'message', 'message_edited': 'message_edited', 'message_deleted': 'message_deleted', 'card_action': 'card_action', 'bot_added': 'bot_added', 'member_changed': 'member_changed'},
    'session.SessionKind': {'channel': 'channel', 'task': 'task', 'dm': 'dm'},
    'session.SessionStatus': {'active': 'active', 'idle': 'idle', 'archived': 'archived'},
    'session.TaskStatus': {'created': 'created', 'planning': 'planning', 'answering': 'answering', 'working': 'working', 'awaiting_approval': 'awaiting_approval', 'delivered': 'delivered', 'failed': 'failed', 'cancelled': 'cancelled'},
    'gateway.ToolErrorCode': {'not_found': 'not_found', 'invalid_args': 'invalid_args', 'denied': 'denied', 'timeout': 'timeout', 'upstream': 'upstream', 'sandbox': 'sandbox'},
    'evidence.EvidenceKind': {'task_created': 'task_created', 'event_received': 'event_received', 'model_call': 'model_call', 'checklist_op': 'checklist_op', 'tool_call': 'tool_call', 'tool_result': 'tool_result', 'artifact': 'artifact', 'delivered': 'delivered', 'failed': 'failed', 'cancelled': 'cancelled'},
}

# --- 冻结表：模块级常量 ---------------------------------------------------
FROZEN_CONSTANTS = {
    'gateway.MAX_TOOL_CONTENT_CHARS': 12000,
    'gateway.DEFAULT_TOOL_TIMEOUT_SEC': 60,
    'sandbox.MAX_EXEC_OUTPUT_CHARS': 20000,
    'evidence.GENESIS': '0000000000000000000000000000000000000000000000000000000000000000',
}


# --- 运行时重新采集，与上面的冻结表逐条比 ----------------------------------

def _norm(v):
    """归一成「可写成字面量、且能咬住类型退化」的形式。

    - StrEnum -> "类名.取值"：直接比原值抓不到「枚举被降级成裸 str」，
      因为 StrEnum.active == "active" 恒为 True。
    - 嵌套 BaseModel（来自 default_factory）-> ("类名", model_dump())：
      repr 出来是构造器调用，不是合法字面量，且换个类同名字段也能咬住。
    """
    if isinstance(v, StrEnum):
        return f"{type(v).__name__}.{v.value}"
    if isinstance(v, BaseModel):
        return (type(v).__name__, v.model_dump())
    return v

def _collect():
    fields, literals, enums, consts = {}, {}, {}, {}
    for m in MODULES:
        mod = importlib.import_module(f"aite.contracts.{m}")
        for name, obj in vars(mod).items():
            if inspect.isclass(obj) and getattr(obj, "__module__", "") == mod.__name__:
                if issubclass(obj, BaseModel) and obj is not BaseModel:
                    row = {}
                    for fname, f in obj.model_fields.items():
                        if f.default is not PydanticUndefined:
                            row[fname] = _norm(f.default)
                        elif f.default_factory is not None:
                            row[fname] = _norm(f.default_factory())
                        else:
                            row[fname] = REQUIRED
                        if typing.get_origin(f.annotation) is typing.Literal:
                            literals[f"{m}.{obj.__name__}.{fname}"] = list(
                                typing.get_args(f.annotation))
                    fields[f"{m}.{obj.__name__}"] = row
                elif issubclass(obj, StrEnum) and obj is not StrEnum:
                    enums[f"{m}.{obj.__name__}"] = {e.name: e.value for e in obj}
            elif (not name.startswith("_") and not inspect.isclass(obj)
                  and not inspect.isfunction(obj) and not inspect.ismodule(obj)
                  and isinstance(obj, (int, str, float, bool))):
                consts[f"{m}.{name}"] = obj
    for m, n in [("capabilities", "Platform"), ("outbound", "ReactionKind")]:
        alias = getattr(importlib.import_module(f"aite.contracts.{m}"), n)
        literals[f"{m}.{n}"] = list(typing.get_args(alias))
    return fields, literals, enums, consts


LIVE_FIELDS, LIVE_LITERALS, LIVE_ENUMS, LIVE_CONSTANTS = _collect()


def test_model_set_is_frozen():
    """§3.1 的模型名单本身冻结 —— 加一个删一个都要先过总管。"""
    assert sorted(LIVE_FIELDS) == sorted(FROZEN_FIELDS)


@pytest.mark.parametrize("model", sorted(FROZEN_FIELDS), ids=lambda s: s)
def test_field_defaults_are_frozen(model):
    """逐字段比默认值：改一个默认值这里就红。"""
    assert LIVE_FIELDS[model] == FROZEN_FIELDS[model], (
        f"{model} 的字段默认值与 §3.1 不符\n"
        f"  实际={LIVE_FIELDS[model]}\n  冻结={FROZEN_FIELDS[model]}"
    )


def test_literal_choices_are_frozen():
    """Literal 的取值集合冻结。pydantic 默认不校验默认值，只能靠这里钉。"""
    assert LIVE_LITERALS == FROZEN_LITERALS


def test_str_enum_members_are_frozen():
    """StrEnum 的成员名与取值都冻结（改 value 会让平台侧的字符串对不上）。"""
    assert LIVE_ENUMS == FROZEN_ENUMS


def test_module_constants_are_frozen():
    """MAX_TOOL_CONTENT_CHARS / DEFAULT_TOOL_TIMEOUT_SEC / MAX_EXEC_OUTPUT_CHARS / GENESIS。

    这几个是跨 track 共享的语义常量：T3 的 Gateway 按超时值判超时、
    沙箱按输出上限截断、T2 的 worker 按它们写 evidence。漂了不会立刻红，只会零星 flake。"""
    assert LIVE_CONSTANTS == FROZEN_CONSTANTS


def test_frozen_table_is_not_vacuous():
    """反向自检：冻结表不能是空的 —— 空表比不设防更危险，因为它看起来是绿的。"""
    assert len(FROZEN_FIELDS) >= 30
    assert sum(len(v) for v in FROZEN_FIELDS.values()) >= 150
    assert len(FROZEN_LITERALS) >= 10
    assert len(FROZEN_ENUMS) >= 5
    assert len(FROZEN_CONSTANTS) >= 4
