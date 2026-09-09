"""官方测试替身：FakePlatform / FakeModel / FakeSandbox / FakeToolGateway /
FakeSessionStore / FakeEvidenceWriter（owner: T4，dev-spec §3.2 全部 Port）。

并行期间 T1/T2/T3 **不要** import 本包（dev-spec §3.4 测试替身规则），各自在
tests/<自己的目录>/ 下写私有替身。这一份是给 tests/e2e/、aite/evals/ 的场景
runner 和 TΩ 的整合测试用的。

替身只记账、不断言：谁被以什么参数调用了全进 CallLog，断言留给使用方。
"""
from .fake_gateway import FakeToolGateway
from .fake_model import FakeModel, FakeModelError, ScriptedToolCall, ScriptExhausted, ScriptStep
from .fake_platform import FAKE_P0, FakePlatform, FakePlatformError, history_message
from .fake_sandbox import ExecScriptStep, FakeSandbox, FakeSandboxError, as_bytes
from .fake_store import FakeEvidenceWriter, FakeSessionStore
from .recorder import Call, CallLog
from .samples import BUILTINS, CSV_SAMPLE, PNG_1X1, PNG_MAGIC, png_bytes

__all__ = [
    "BUILTINS",
    "CSV_SAMPLE",
    "FAKE_P0",
    "PNG_1X1",
    "PNG_MAGIC",
    "Call",
    "CallLog",
    "ExecScriptStep",
    "FakeEvidenceWriter",
    "FakeModel",
    "FakeModelError",
    "FakePlatform",
    "FakePlatformError",
    "FakeSandbox",
    "FakeSandboxError",
    "FakeSessionStore",
    "FakeToolGateway",
    "ScriptExhausted",
    "ScriptStep",
    "ScriptedToolCall",
    "as_bytes",
    "history_message",
    "png_bytes",
]
