"""tests/control 的 fixture（§3.4：各 track 的 fixture 放自己目录，不动 tests/conftest.py）。"""
import pytest
from control_fakes import (
    FakeClock,
    FakeGateway,
    FakePlatform,
    FakeSandbox,
    ScriptedModel,
    make_config,
)

from aite.control import InProcessControlPlane, SqliteSessionStore
from aite.evidence import FileEvidenceWriter


@pytest.fixture
def clock() -> FakeClock:
    return FakeClock()


@pytest.fixture
def config(tmp_path):
    return make_config(tmp_path)


@pytest.fixture
async def store(config):
    s = SqliteSessionStore(config.storage.sqlite_path)
    await s.init()
    yield s
    await s.close()


@pytest.fixture
def platform() -> FakePlatform:
    return FakePlatform()


@pytest.fixture
def sandbox() -> FakeSandbox:
    return FakeSandbox()


@pytest.fixture
def gateway() -> FakeGateway:
    return FakeGateway()


@pytest.fixture
def evidence(config) -> FileEvidenceWriter:
    return FileEvidenceWriter(config.storage.evidence_dir)


@pytest.fixture
def make_plane(store, platform, evidence, config, sandbox, gateway, clock):
    """造一个 ControlPlane。默认不给 model —— 路由测试不需要 worker 真跑。"""

    def _make(model: ScriptedModel | None = None, **kwargs) -> InProcessControlPlane:
        params = dict(
            store=store,
            platform=platform,
            evidence=evidence,
            config=config,
            model=model,
            gateway=gateway,
            sandbox=sandbox,
            clock=clock,
            sleep=clock.sleep,
        )
        params.update(kwargs)
        return InProcessControlPlane(**params)

    return _make


@pytest.fixture
def plane(make_plane) -> InProcessControlPlane:
    return make_plane()
