"""tests/scripts 的私有 fixture：加载被测脚本 + 三个外部依赖的替身。

`scripts/preflight.py` 不是包里的模块（它是给人在命令行敲的），所以按文件路径加载。

辅助件一律以 fixture 的形式交给用例，不让用例 `from conftest import ...` ——
理由同 tests/sandbox/conftest.py 开头那段：tests/ 下没有 __init__.py，那样写会把
本文件当成顶层模块 `conftest` 再导一遍，而每条轨都有自己的 conftest.py，撞名就是
pytest 的 import file mismatch。

这里的替身撑起派单那条硬要求：**测试里不许真连网、不许真起容器**。
* 飞书走 respx（拦 httpx，`FeishuApiClient` 用的就是 httpx）
* 模型注入假 client（`OpenAICompatModel(cfg, client=...)` 支持）——
  不能用 respx：本机 openai>=1.40 自带 vendored 的 httpx2，respx 拦不到，
  理由同 tests/e2e/test_t4_model_openai_compat.py 的开头那段
* docker 注入假 client（`DockerSandbox(client=...)` 支持）

所以本目录一条用例都不需要 `-m docker`。
"""
from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest
import yaml
from docker.errors import DockerException, ImageNotFound, NotFound

REPO_ROOT = Path(__file__).resolve().parents[2]
PREFLIGHT_PATH = REPO_ROOT / "scripts" / "preflight.py"

#: 假取值。取得足够长、认得出来，「密钥不外泄」那条断言靠它们。
FAKE_APP_ID = "cli_FAKEappid0000000"
FAKE_APP_SECRET = "FAKEappsecret9999999999999999999"
FAKE_BOT_OPEN_ID = "ou_FAKEbotopenid00000000000000000"
FAKE_MODEL_KEY = "sk-FAKEmodelkey1111111111111111111"
FAKE_TOKEN = "t-FAKEtenantaccesstoken2222222222"

FULL_ENV = {
    "FEISHU_APP_ID": FAKE_APP_ID,
    "FEISHU_APP_SECRET": FAKE_APP_SECRET,
    "FEISHU_BOT_OPEN_ID": FAKE_BOT_OPEN_ID,
    "AITE_MODEL_API_KEY": FAKE_MODEL_KEY,
}

DEFAULT_PROBE_STDOUT = (
    b'AITE_PREFLIGHT_OK {"pandas": "2.2.3", "matplotlib": "3.9.2", '
    b'"openpyxl": "3.1.5", "docx": "1.1.2"}\n'
)


def _load_preflight():
    spec = importlib.util.spec_from_file_location("aite_preflight_under_test", PREFLIGHT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


_PREFLIGHT = _load_preflight()


# ---------------------------------------------------------------------------
# 配置
# ---------------------------------------------------------------------------

def _write_config(tmp_path: Path, **override: Any) -> Path:
    """写一份「填好了的」配置：model 有 base_url/model，storage 指到 tmp_path。

    不直接用 config/aite.example.yaml：它的 base_url/model 是空的，第 5 组永远走不到
    真分支；而它的 storage 指向仓库里的 data/，测试不该往被测仓库里写东西。
    """
    data: dict[str, Any] = {
        "tenant_id": "default",
        "platform": "feishu",
        "feishu": {
            "app_id_env": "FEISHU_APP_ID",
            "app_secret_env": "FEISHU_APP_SECRET",
            "bot_name": "Aite",
            "bot_open_id_env": "FEISHU_BOT_OPEN_ID",
        },
        "model": {
            "provider": "openai_compat",
            "base_url": "https://model.example.com/compatible-mode/v1",
            "api_key_env": "AITE_MODEL_API_KEY",
            "model": "qwen-plus",
            "price_in_per_mtok": 0.8,
            "price_out_per_mtok": 2.0,
        },
        "sandbox": {"image": "aite-sandbox:p0"},
        "storage": {
            "sqlite_path": str(tmp_path / "data" / "aite.db"),
            "evidence_dir": str(tmp_path / "data" / "evidence"),
            "artifacts_dir": str(tmp_path / "data" / "artifacts"),
        },
    }
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(data.get(key), dict):
            data[key].update(value)
        else:
            data[key] = value
    path = tmp_path / "aite.yaml"
    path.write_text(yaml.safe_dump(data, allow_unicode=True), encoding="utf-8")
    return path


# ---------------------------------------------------------------------------
# 模型替身
# ---------------------------------------------------------------------------

def _chat_payload(*, content: str = "pong", prompt_tokens: int = 7, completion_tokens: int = 2):
    return {
        "id": "chatcmpl-preflight",
        "model": "qwen-plus",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop",
            }
        ],
        "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens},
    }


class _FakeCompletions:
    def __init__(self, payload=None, error: BaseException | None = None) -> None:
        self.payload = payload if payload is not None else _chat_payload()
        self.error = error
        self.calls: list[dict[str, Any]] = []

    async def create(self, **kwargs):
        self.calls.append(kwargs)
        if self.error is not None:
            raise self.error
        return self.payload


class FakeModelClient:
    """长得像 AsyncOpenAI 的最小面：`chat.completions.create` + `close`。"""

    def __init__(self, payload=None, error: BaseException | None = None) -> None:
        self.completions = _FakeCompletions(payload, error)
        self.chat = SimpleNamespace(completions=self.completions)
        self.closed = False

    async def close(self) -> None:
        self.closed = True


# ---------------------------------------------------------------------------
# docker 替身
# ---------------------------------------------------------------------------

class FakeExecResult:
    def __init__(self, exit_code: int = 0, stdout: bytes = b"", stderr: bytes = b"") -> None:
        self.exit_code = exit_code
        self.output = (stdout, stderr)


class _FakeContainer:
    def __init__(self, cid: str, labels: dict[str, str], owner: FakeDocker) -> None:
        self.id = cid
        self.labels = labels
        self.attrs: dict[str, Any] = {"State": {}, "Created": None}
        self._owner = owner

    def start(self) -> None:
        self._owner.events.append(("start", self.id))

    def exec_run(self, cmd, workdir=None, demux=False):
        self._owner.events.append(("exec", tuple(cmd)))
        return self._owner.exec_reply(list(cmd))

    def put_archive(self, parent, data) -> bool:
        self._owner.events.append(("put_archive", parent))
        return True

    def get_archive(self, path):
        raise NotFound(path)

    def remove(self, force: bool = False) -> None:
        self._owner.events.append(("remove", self.id))
        self._owner.live.pop(self.id, None)


class _FakeImages:
    def __init__(self, owner: FakeDocker) -> None:
        self._owner = owner

    def get(self, name: str):
        if name not in self._owner.image_names:
            raise ImageNotFound(f"no such image: {name}")
        return SimpleNamespace(id="sha256:" + "1" * 64)


class _FakeContainers:
    def __init__(self, owner: FakeDocker) -> None:
        self._owner = owner

    def create(self, **kwargs):
        cid = f"fake{len(self._owner.events):04d}" + "0" * 52
        container = _FakeContainer(cid, dict(kwargs.get("labels") or {}), self._owner)
        self._owner.live[cid] = container
        self._owner.created.append(kwargs)
        self._owner.events.append(("create", cid))
        return container

    def get(self, cid: str):
        container = self._owner.live.get(cid)
        if container is None:
            raise NotFound(cid)
        return container

    def list(self, all: bool = False, filters=None):  # noqa: A002 - docker SDK 的参数名就叫 all
        want = (filters or {}).get("label")
        rows = list(self._owner.live.values())
        if want and "=" in want:
            key, value = want.split("=", 1)
            return [c for c in rows if c.labels.get(key) == value]
        if want:
            return [c for c in rows if want in c.labels]
        return rows


class FakeDocker:
    """够 `DockerSandbox` 跑完 acquire → exec → release 的最小 docker client。

    `live` 是「本机现在还有哪些容器」—— 收尾没做干净它就不空，
    第 6 组那条「容器一定被收掉」的断言看的就是它。
    """

    def __init__(self, *, images=("aite-sandbox:p0",), probe=None) -> None:
        self.image_names = set(images)
        self.images = _FakeImages(self)
        self.containers = _FakeContainers(self)
        self.live: dict[str, _FakeContainer] = {}
        self.events: list[tuple] = []
        self.created: list[dict] = []
        self.closed = False
        self._probe = probe

    def version(self):
        return {"Version": "27.0.0-fake", "ApiVersion": "1.46"}

    def close(self) -> None:
        self.closed = True

    def exec_reply(self, cmd: list[str]) -> FakeExecResult:
        if cmd and cmd[0] in ("mkdir", "rm"):
            return FakeExecResult(0)
        if cmd[:2] == ["python", "-c"]:
            # 探路脚本和文件快照脚本都是 `python -c <code>`，按内容认。
            return (
                FakeExecResult(0, b"AITE_SANDBOX_OK\n")
                if "AITE_SANDBOX_OK" in cmd[2]
                else FakeExecResult(0, json.dumps({}).encode())
            )
        if cmd and cmd[0] == "timeout":
            if callable(self._probe):
                return self._probe()
            return self._probe or FakeExecResult(0, DEFAULT_PROBE_STDOUT)
        return FakeExecResult(0)


def _boom_docker(*_args, **_kwargs):
    """谁叫它就说明「不该碰 docker」这条约束破了。"""
    raise DockerException("测试里不该连 docker daemon")


# ---------------------------------------------------------------------------
# fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def pf():
    """被测脚本本身。"""
    return _PREFLIGHT


@pytest.fixture(scope="session")
def secrets():
    """假凭证 + 一份齐全的环境变量。"""
    return SimpleNamespace(
        app_id=FAKE_APP_ID,
        app_secret=FAKE_APP_SECRET,
        bot_open_id=FAKE_BOT_OPEN_ID,
        model_key=FAKE_MODEL_KEY,
        token=FAKE_TOKEN,
        env=dict(FULL_ENV),
        all=(FAKE_APP_ID, FAKE_APP_SECRET, FAKE_BOT_OPEN_ID, FAKE_MODEL_KEY, FAKE_TOKEN),
    )


@pytest.fixture(scope="session")
def fakes():
    """替身工具箱。"""
    return SimpleNamespace(
        Docker=FakeDocker,
        ModelClient=FakeModelClient,
        ExecResult=FakeExecResult,
        boom_docker=_boom_docker,
        chat_payload=_chat_payload,
    )


@pytest.fixture(scope="session")
def write_config():
    return _write_config


@pytest.fixture
def run_preflight():
    """跑一次 `preflight.main`，外部依赖全打桩。

    `docker` 可以给 FakeDocker 实例，也可以给一个工厂函数（验「不该碰 docker」那种）。
    """

    def _run(argv, *, env=None, docker=None, model=None, out=None):
        fake_docker = docker if isinstance(docker, FakeDocker) else FakeDocker()
        fake_model = model if model is not None else FakeModelClient()
        factory = docker if callable(docker) else (lambda: fake_docker)
        deps = _PREFLIGHT.Deps(
            model_client_factory=lambda _cfg, _key: fake_model,
            docker_client_factory=factory,
        )
        code = _PREFLIGHT.main(
            argv, env=dict(FULL_ENV) if env is None else env, deps=deps, out=out
        )
        return SimpleNamespace(code=code, docker=fake_docker, model=fake_model)

    return _run
