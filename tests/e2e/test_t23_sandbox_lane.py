"""`--sandbox {fake,docker}` 那一档（owner: T23，§2.4 M3）。

四件事要守住：

1. **默认档逐字节不变。** `--sandbox` 不给 = `fake`，造出来的还是 `FakeSandbox` /
   `FakeToolGateway`，`--list` 与 `passed k/10` 那两行 stdout 一个字都不多。
   `scripts/check.sh` 的 B8、CI、T4 的 200 条都吃这条路。
2. **docker 档只组装不连 daemon。** `build_deps(sandbox_kind="docker")` 本身不碰
   Docker（客户端是首次 `acquire` 才建的），daemon 在不在由起飞前体检查。
3. **daemon / 镜像缺席给人话。** 一行中文 + 退出码 2，不是异常栈 —— 照
   `--model live` 起不来那条先例。
4. **真容器那条**（`@pytest.mark.docker`）把 04 的整条 M3 主干走一遍：附件 →
   `download_attachment` → 容器里真跑 matplotlib → `final(artifacts)` → `send_file`
   是一张真 PNG。daemon / 镜像缺席就 skip，环境问题不该表现成代码红。

真容器用例只查**自己那个 task 的容器**收没收干净，不查
`docker ps -a --filter label=aite.task` 整体为空：那个 label 是全机器共享的命名空间，
并行的别的会话也在用，整体判据会互相误伤（tests/sandbox 那几条就是这么飘的）。
"""
import json
import subprocess

import pytest
from docker.errors import DockerException, ImageNotFound

from aite.evals.__main__ import main
from aite.evals.real_stack import (
    GatewayProbe,
    SandboxProbe,
    docker_preflight,
    scenarios_with_exec_script,
    token_resolver_of,
)
from aite.evals.runner import run_scenario
from aite.evals.scenario import Scenario, load_suite
from aite.evals.wiring import SANDBOXES, PhaseError, build_deps, sandbox_busy
from aite.gateway.tool_gateway import P0ToolGateway
from aite.sandbox.docker_sandbox import LABEL_TASK, DockerSandbox
from aite.testing import FakeSandbox, FakeSessionStore, FakeToolGateway

IMAGE = "aite-sandbox:p0"
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


# --------------------------------------------------------------------------
# CLI 参数面
# --------------------------------------------------------------------------

def test_sandbox_defaults_to_fake():
    from aite.evals.__main__ import build_parser

    args = build_parser().parse_args(["run", "evals/p0"])
    assert args.sandbox == "fake"


@pytest.mark.parametrize("kind", SANDBOXES)
def test_sandbox_accepts_both_lanes(kind):
    from aite.evals.__main__ import build_parser

    args = build_parser().parse_args(["run", "evals/p0", "--sandbox", kind])
    assert args.sandbox == kind


def test_sandbox_rejects_an_unknown_lane():
    from aite.evals.__main__ import build_parser

    with pytest.raises(SystemExit):
        build_parser().parse_args(["run", "evals/p0", "--sandbox", "podman"])


def test_list_output_is_unchanged_by_the_new_flag(capsys):
    """`--list` 那 10 行是 T4 的验收面之一，加了开关也得逐字节一样。"""
    assert main(["run", "evals/p0", "--list"]) == 0
    without = capsys.readouterr().out
    assert main(["run", "evals/p0", "--sandbox", "fake", "--list"]) == 0
    assert capsys.readouterr().out == without
    assert len(json.loads(without)) == 10


# --------------------------------------------------------------------------
# build_deps 的两档
# --------------------------------------------------------------------------

def test_default_lane_still_builds_the_fakes():
    deps = build_deps(Scenario(name="x"))
    assert isinstance(deps.sandbox, FakeSandbox)
    assert isinstance(deps.gateway, FakeToolGateway)


def test_docker_lane_builds_the_real_sandbox_and_gateway():
    deps = build_deps(Scenario(name="x"), sandbox_kind="docker")
    assert isinstance(deps.sandbox, SandboxProbe)
    assert isinstance(deps.sandbox.inner, DockerSandbox)
    assert isinstance(deps.gateway, GatewayProbe)
    assert isinstance(deps.gateway.inner, P0ToolGateway)


def test_docker_lane_does_not_touch_the_daemon_while_assembling(monkeypatch):
    """组装期一次 docker 调用都不该发生 —— 客户端是首次 acquire 才建的。"""
    from docker.client import DockerClient

    def boom(*a, **k):
        raise AssertionError("build_deps 不该在组装期连 Docker daemon")

    monkeypatch.setattr(DockerClient, "from_env", staticmethod(boom))
    deps = build_deps(Scenario(name="x"), sandbox_kind="docker")
    assert isinstance(deps.sandbox.inner, DockerSandbox)


def test_unknown_lane_is_a_phase_error_with_a_readable_reason():
    with pytest.raises(PhaseError) as excinfo:
        build_deps(Scenario(name="x"), sandbox_kind="podman")
    assert excinfo.value.phase == "wiring"
    assert "podman" in str(excinfo.value)


async def test_default_lane_aclose_is_a_no_op():
    """FakeSandbox 没有 aclose；默认档收摊什么都不该做，也不该炸。"""
    deps = build_deps(Scenario(name="x"))
    await deps.aclose()
    assert deps.sandbox.calls.methods() == []


# --------------------------------------------------------------------------
# session_token：这一档的第一堵墙
# --------------------------------------------------------------------------

def test_token_resolver_reads_the_store_without_recording_activity():
    """resolver 得直读 dict：走 get_task() 会往 store.calls 记账，把静默判据搅浑。"""
    store = FakeSessionStore()
    resolve = token_resolver_of(store)
    assert resolve("nope") is None

    task = _a_task("tid-1", "deadbeef" * 4)
    store.tasks[task.id] = task
    assert resolve("tid-1") == "deadbeef" * 4
    assert len(store.calls) == 0


def test_docker_lane_gateway_can_answer_the_token_challenge():
    """真 Gateway 的令牌校验失败关闭，接不上就是每个工具调用都 denied。"""
    deps = build_deps(Scenario(name="x"), sandbox_kind="docker")
    task = _a_task("tid-2", "cafe" * 8)
    deps.store.tasks[task.id] = task
    # 私有面，但这条接线就是本轨的靶心，值得直接钉住
    assert deps.gateway.inner._token_resolver("tid-2") == "cafe" * 8


def _a_task(task_id: str, token: str):
    from datetime import UTC, datetime

    from aite.contracts import Task

    now = datetime.now(UTC)
    return Task(
        id=task_id,
        session_id="s1",
        task_no="#A1",
        session_token=token,
        created_by="u1",
        created_at=now,
        updated_at=now,
    )


# --------------------------------------------------------------------------
# 探针的记账面与 in_flight
# --------------------------------------------------------------------------

async def test_sandbox_probe_records_the_same_method_names_as_the_fake():
    """`check: sandbox_calls` 两档下读的得是同一个键。"""
    inner = FakeSandbox()
    probe = SandboxProbe(FakeSandbox())
    from aite.contracts import SandboxSpec

    spec = SandboxSpec(image=IMAGE)
    for box in (inner, probe):
        sid = await box.acquire("t", spec)
        await box.put_file(sid, "/work/a.txt", b"hi")
        await box.list_files(sid)
        await box.touch(sid)
        await box.release(sid)
    assert probe.calls.methods() == inner.calls.methods()


async def test_sandbox_probe_reports_in_flight_during_a_slow_call():
    """settle() 的静默判据靠它减掉长 await —— 不减的话真沙箱起容器期间任务就被取消。"""
    import asyncio

    class Slow:
        def __init__(self):
            self.entered = asyncio.Event()
            self.release_me = asyncio.Event()

        async def acquire(self, task_id, spec):
            self.entered.set()
            await self.release_me.wait()
            return "sb-1"

    slow = Slow()
    probe = SandboxProbe(slow)
    deps = build_deps(Scenario(name="x"))
    deps.sandbox = probe

    assert sandbox_busy(deps) is False
    from aite.contracts import SandboxSpec

    task = asyncio.ensure_future(probe.acquire("t", SandboxSpec(image=IMAGE)))
    await slow.entered.wait()
    assert probe.in_flight == 1
    assert sandbox_busy(deps) is True

    slow.release_me.set()
    assert await task == "sb-1"
    assert probe.in_flight == 0
    assert sandbox_busy(deps) is False


async def test_sandbox_probe_keeps_the_record_when_the_call_throws():
    class Boom:
        async def exec(self, sandbox_id, req):
            raise RuntimeError("容器没了")

    probe = SandboxProbe(Boom())
    from aite.contracts import ExecRequest

    with pytest.raises(RuntimeError):
        await probe.exec("sb-1", ExecRequest(code="x", timeout_sec=1))
    assert probe.calls.count("exec") == 1
    assert probe.in_flight == 0
    assert "容器没了" in probe.calls.last("exec").error


def test_sandbox_probe_passes_unknown_attributes_through():
    box = DockerSandbox()
    probe = SandboxProbe(box)
    assert probe.reap_idle.__self__ is probe          # 显式实现的走自己
    assert probe._boxes is box._boxes                 # 其余透传


async def test_gateway_probe_mirrors_the_fake_assertion_surface():
    """`gateway_calls` / `gateway_result` 两个 check 读的是 count() / results_of()。"""
    from aite.contracts import ToolCallRequest, ToolContext, ToolResult

    class Stub:
        async def call(self, ctx, req):
            return ToolResult(call_id=req.call_id, name=req.name, ok=True, content="fine")

        def catalog(self, ctx):
            return []

        async def release_task(self, task_id):
            return task_id

    probe = GatewayProbe(Stub())
    ctx = ToolContext(
        tenant_id="t",
        workspace_id="w",
        chat_id="c",
        thread_id="th",
        session_id="s1",
        task_id="tid",
        session_token="tok",
    )
    probe.catalog(ctx)
    await probe.call(ctx, ToolCallRequest(call_id="c1", name="run_python", arguments={}))
    await probe.call(ctx, ToolCallRequest(call_id="c2", name="list_files", arguments={}))

    assert probe.count("run_python") == 1
    assert probe.count("download_attachment") == 0
    assert [r.name for r in probe.results_of("run_python")] == ["run_python"]
    assert probe.calls.count("call") == 2
    assert probe.calls.count("catalog") == 1
    # worker 与控制面都 getattr 这个方法，透传得接得上
    assert await probe.release_task("tid") == "tid"


# --------------------------------------------------------------------------
# exec_script 在 docker 档下的去向
# --------------------------------------------------------------------------

def test_exec_script_scenarios_are_named_not_rejected():
    """docker 档忽略 exec_script，但起飞时点名 —— 别让它静默失效。"""
    named = scenarios_with_exec_script(load_suite("evals/p0"))
    assert named == ["04_csv_to_chart", "07_commands"]


def test_docker_lane_ignores_exec_script_instead_of_erroring():
    sc = [s for s in load_suite("evals/p0") if s.name == "04_csv_to_chart"][0]
    assert sc.sandbox.exec_script                     # 场景确实带着台词
    deps = build_deps(sc, sandbox_kind="docker")      # 不抛，照样造得出来
    assert isinstance(deps.sandbox.inner, DockerSandbox)


# --------------------------------------------------------------------------
# 起飞前体检：daemon / 镜像缺席给人话
# --------------------------------------------------------------------------

def test_preflight_says_daemon_is_down_in_words(monkeypatch):
    from docker.client import DockerClient

    def down(*a, **k):
        raise DockerException("Error while fetching server API version")

    monkeypatch.setattr(DockerClient, "from_env", staticmethod(down))
    why = docker_preflight([IMAGE])
    assert why is not None
    assert "Docker daemon" in why and "Docker Desktop" in why
    assert "Traceback" not in why


def test_preflight_says_which_image_is_missing(monkeypatch):
    from docker.client import DockerClient

    class Images:
        def get(self, name):
            raise ImageNotFound(name)

    class Client:
        images = Images()

        def ping(self):
            return True

        def close(self):
            return None

    monkeypatch.setattr(DockerClient, "from_env", staticmethod(lambda *a, **k: Client()))
    why = docker_preflight(["aite-sandbox:nope"])
    assert why is not None
    assert "aite-sandbox:nope" in why
    assert "docker build" in why


def test_cli_reports_a_dead_daemon_as_one_line_and_exit_2(monkeypatch, capsys):
    """照 `--model live 起不来` 那条先例：一行中文 + 退出码 2，不是异常栈。"""
    import aite.evals.__main__ as cli

    monkeypatch.setattr(cli, "docker_preflight", lambda images: "连不上 Docker daemon：…")
    assert main(["run", "evals/p0", "--sandbox", "docker"]) == 2
    captured = capsys.readouterr()
    assert captured.out == ""                          # stdout 一个字都不吐
    assert "--sandbox docker 起不来" in captured.err
    assert "Traceback" not in captured.err


def test_cli_preflight_asks_about_the_images_the_scenarios_want(monkeypatch):
    import aite.evals.__main__ as cli

    seen = {}

    def spy(images):
        seen["images"] = list(images)
        return "停在这儿就行"

    monkeypatch.setattr(cli, "docker_preflight", spy)
    assert main(["run", "evals/p0", "--only", "04_csv_to_chart", "--sandbox", "docker"]) == 2
    assert seen["images"] == [IMAGE]


def test_default_lane_never_runs_the_preflight(monkeypatch, capsys):
    import aite.evals.__main__ as cli

    def boom(images):
        raise AssertionError("默认档不该查 Docker")

    monkeypatch.setattr(cli, "docker_preflight", boom)
    assert main(["run", "evals/p0", "--platform", "fake", "--model", "scripted"]) == 0
    assert capsys.readouterr().out.splitlines()[-1] == "passed 10/10"


# --------------------------------------------------------------------------
# 真容器（B4 那条路，环境缺席就 skip）
# --------------------------------------------------------------------------

def _docker(*args: str, timeout: int = 60) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["docker", *args], capture_output=True, text=True, timeout=timeout, check=False
    )


def _docker_ready() -> bool:
    try:
        if _docker("info", "--format", "{{.ServerVersion}}", timeout=20).returncode != 0:
            return False
        return _docker("image", "inspect", IMAGE, timeout=20).returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


requires_docker = pytest.mark.skipif(
    not _docker_ready(), reason=f"没有 Docker daemon 或 {IMAGE} 镜像"
)


@pytest.mark.docker
@requires_docker
async def test_m3_mainline_really_runs_in_a_container(demo_plane_factory):
    """M3 主干真跑一遍：CSV 进容器 → matplotlib 画图 → PNG 发回线程。

    用真 ControlPlane（运行时发现），不用 DemoPlane —— 验的就是整条接线。
    """
    sc = [s for s in load_suite("evals/p0") if s.name == "04_csv_to_chart"][0]

    import aite.evals.runner as runner_mod

    box = {}
    original = runner_mod.build_deps

    def spy(*a, **k):
        deps = original(*a, **k)
        box["deps"] = deps
        return deps

    runner_mod.build_deps = spy
    try:
        result = await run_scenario(sc, sandbox_kind="docker")
    finally:
        runner_mod.build_deps = original

    deps = box["deps"]
    tasks = deps.store.task_list
    assert [str(t.status) for t in tasks] == ["delivered"], result.failures

    # 那张 PNG 是镜像里的 matplotlib 真画出来的：替身那张 builtin 只有几十字节
    sent = deps.platform.sent_files
    assert len(sent) == 1
    assert sent[0].data[:8] == PNG_MAGIC
    assert len(sent[0].data) > 5000

    # 真沙箱真的被使唤过（替身档这几步是 FakeSandbox 在演）
    methods = deps.sandbox.calls.methods()
    assert "acquire" in methods and "exec" in methods and "get_file" in methods

    # 自己那个 task 的容器收干净了。不查整体为空：那个 label 是全机器共享的。
    for task in tasks:
        left = _docker("ps", "-a", "--filter", f"label={LABEL_TASK}={task.id}", "-q")
        assert left.stdout.strip() == "", f"task {task.id} 的容器没收干净"


@pytest.mark.docker
@requires_docker
async def test_docker_lane_releases_the_container_even_when_the_task_is_cut_short():
    """场景没走到终态时靠 Deps.aclose 收摊，别把容器留到 reap_idle 那 300s。"""
    from aite.contracts import SandboxSpec

    deps = build_deps(Scenario(name="x"), sandbox_kind="docker")
    task_id = "t23-aclose-check"
    sandbox_id = await deps.sandbox.acquire(task_id, SandboxSpec(image=IMAGE))
    assert _docker("ps", "-a", "--filter", f"label={LABEL_TASK}={task_id}", "-q").stdout.strip()

    await deps.aclose()

    left = _docker("ps", "-a", "--filter", f"label={LABEL_TASK}={task_id}", "-q").stdout.strip()
    assert left == "", f"{sandbox_id} 没被收走"
