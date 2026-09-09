"""tests/sandbox 的私有 fixture（owner: T3）。

按 §3.4 的测试替身规则：T3 自己的 fixture 放自己目录，不动 tests/conftest.py，
也不 import aite.testing（那是 T4 的，并行期间还是空包）。

这一层的测试是**真跑 Docker** 的，所以整份带 `docker` mark（B4 用 `-m docker` 选它）。
Docker 不在 / 镜像没 build 就整份 skip：环境问题不该表现成代码红。

辅助函数一律以 fixture 的形式交给用例，不让用例 `from conftest import ...`：
tests/ 下没有 __init__.py，那样写会把本文件当成顶层模块 `conftest` 再导一遍，
而四条轨都有自己的 conftest.py，撞名就是 pytest 的 import file mismatch。
"""
import subprocess
from collections.abc import Callable

import pytest

from aite.contracts import SandboxSpec
from aite.sandbox.docker_sandbox import LABEL_TASK, DockerSandbox

#: 与 §3.1 SandboxConfig.image 的默认值一致。
IMAGE = "aite-sandbox:p0"

BUILD_HINT = f"先跑 `docker build -t {IMAGE} docker/sandbox`"


def _docker(*args: str, timeout: int = 60) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["docker", *args], capture_output=True, text=True, timeout=timeout, check=False
    )


def _labelled_container_ids() -> list[str]:
    """B4 的验收命令：`docker ps -a --filter label=aite.task`（取全长 id 好比对）。"""
    proc = _docker("ps", "-a", "--filter", f"label={LABEL_TASK}", "-q", "--no-trunc")
    return [line.strip() for line in proc.stdout.splitlines() if line.strip()]


def _daemon_alive() -> bool:
    try:
        return _docker("info", "--format", "{{.ServerVersion}}", timeout=20).returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def _image_present() -> bool:
    return _docker("image", "inspect", IMAGE, timeout=20).returncode == 0


def pytest_collection_modifyitems(config, items):
    """给本目录下的每个用例自动补上 docker mark。

    B4 是 `pytest tests/sandbox -q -m docker`，哪个文件漏打 mark 就悄悄被 deselect，
    「全绿」里少了几条没人看得出来。所以不靠人记，按路径统一补。
    """
    here = str(config.rootpath / "tests" / "sandbox")
    for item in items:
        if str(item.path).startswith(here):
            item.add_marker(pytest.mark.docker)


@pytest.fixture(scope="session")
def docker_env() -> None:
    if not _daemon_alive():
        pytest.skip("本机 Docker daemon 没起来")
    if not _image_present():
        pytest.skip(f"没有 {IMAGE} 镜像：{BUILD_HINT}")


@pytest.fixture(scope="session")
def docker_cli() -> Callable[..., subprocess.CompletedProcess]:
    return _docker


@pytest.fixture(scope="session")
def labelled_ids() -> Callable[[], list[str]]:
    return _labelled_container_ids


@pytest.fixture
def spec() -> SandboxSpec:
    # network 是 Literal["none"]，不用也不能传别的值（§3.1）。
    return SandboxSpec(image=IMAGE, cpu=1.0, mem_mb=768)


@pytest.fixture
async def sandbox(docker_env):
    """一个干净的 DockerSandbox；用完把它记账的容器全删掉。"""
    box = DockerSandbox()
    try:
        yield box
    finally:
        await box.aclose()
