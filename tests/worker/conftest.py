"""tests/worker 的 fixture（§3.4：不动 tests/conftest.py）。

worker 测试一律**走完整链路**：ControlPlane 收事件建 task → run_pending 派给 worker。
这样 W3/W4 的断言看到的就是 FakePlatform 上真实的出站序列，而不是内部状态。
"""
import pytest
from worker_fakes import (
    FakeClock,
    FakeGateway,
    FakePlatform,
    FakeSandbox,
    ScriptedModel,
    make_config,
    make_event,
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
def run_task(store, platform, evidence, config, sandbox, gateway, clock):
    """脚本 → 跑完一个任务 → 返回 (plane, task)。"""

    async def _run(
        script=None,
        *,
        repeat=None,
        text: str = "帮我出个图",
        step_seconds: float = 0.0,
        fail_times: int = 0,
        attachments=None,
        **plane_kwargs,
    ):
        model = ScriptedModel(
            script, repeat=repeat, clock=clock, step_seconds=step_seconds, fail_times=fail_times
        )
        plane = InProcessControlPlane(
            store=store,
            platform=platform,
            evidence=evidence,
            config=config,
            model=model,
            gateway=gateway,
            sandbox=sandbox,
            clock=clock,
            sleep=clock.sleep,
            **plane_kwargs,
        )
        await plane.handle_event(make_event(text=text, attachments=attachments))
        task = (await store.list_active_tasks("oc_chat"))[0]
        await plane.run_pending()
        return plane, await store.get_task(task.id), model

    return _run
