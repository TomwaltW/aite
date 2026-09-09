"""DockerSandbox 真跑容器的测试（owner: T3）—— §2.2 B4。

B4 原文两条硬判据：
- `run_python` 跑 matplotlib 生成 `/work/out.png` → `get_file` 返回的前 8 字节是 PNG 魔数
- `reap_idle(1)` 后 `docker ps -a --filter label=aite.task` 为空

其余用例是 §3.2 SandboxPort 那八个方法各自的行为面。
"""
import asyncio

import pytest

from aite.contracts import MAX_EXEC_OUTPUT_CHARS, ExecRequest
from aite.sandbox import (
    SandboxError,
    SandboxFileNotFound,
    SandboxNotFound,
    SandboxPathError,
)
from aite.sandbox.docker_sandbox import LABEL_TASK, DockerSandbox

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


async def test_acquire_labels_the_container_with_task_id(sandbox, spec, docker_cli, labelled_ids):
    """§3.2：容器必须打标签 aite.task=<task_id> —— B4 的验收命令靠它找容器。"""
    task_id = "t3-label-check"
    sandbox_id = await sandbox.acquire(task_id, spec)

    inspect = docker_cli(
        "inspect", sandbox_id, "--format", f"{{{{index .Config.Labels \"{LABEL_TASK}\"}}}}"
    )
    assert inspect.returncode == 0, inspect.stderr
    assert inspect.stdout.strip() == task_id
    assert sandbox_id in labelled_ids()


async def test_acquire_container_has_no_network(sandbox, spec, docker_cli):
    """§3.1：SandboxSpec.network 是 Literal["none"]，凭证与平台调用都不在沙箱里发生。"""
    sandbox_id = await sandbox.acquire("t3-no-network", spec)

    mode = docker_cli(
        "inspect", sandbox_id, "--format", "{{.HostConfig.NetworkMode}}"
    ).stdout.strip()
    assert mode == "none"

    # 不止看配置，真在里面拨一次号
    result = await sandbox.exec(
        sandbox_id,
        ExecRequest(
            code=(
                "import urllib.request\n"
                "urllib.request.urlopen('https://example.com', timeout=3)\n"
            ),
            timeout_sec=20,
        ),
    )
    assert result.exit_code != 0
    assert "URLError" in result.stderr or "urlopen error" in result.stderr


async def test_acquire_rejects_missing_image(sandbox, spec):
    """镜像不存在要给一句能照着做的话，而不是让 SDK 去 pull 一个私有 tag。"""
    with pytest.raises(SandboxError, match="docker build"):
        await sandbox.acquire("t3-missing-image", spec.model_copy(update={"image": "aite-nope:x"}))


async def test_exec_returns_stdout_exit_code_and_files_out(sandbox, spec):
    sandbox_id = await sandbox.acquire("t3-exec", spec)

    result = await sandbox.exec(
        sandbox_id,
        ExecRequest(
            code=(
                "import os\n"
                "print('cwd', os.getcwd())\n"
                "open('/work/hello.txt', 'w').write('你好')\n"
            ),
            timeout_sec=30,
        ),
    )

    assert result.exit_code == 0, result.stderr
    assert "cwd /work" in result.stdout            # §3.2：工作目录是 /work
    assert result.truncated is False
    assert result.duration_ms > 0
    assert [f.path for f in result.files_out] == ["/work/hello.txt"]
    assert result.files_out[0].size == len("你好".encode())


async def test_exec_files_out_only_reports_this_run(sandbox, spec):
    """files_out 是「本次新增/修改」，上一轮写的文件不能再报一遍。"""
    sandbox_id = await sandbox.acquire("t3-files-out", spec)

    await sandbox.exec(sandbox_id, ExecRequest(code="open('/work/a.txt','w').write('1')"))
    second = await sandbox.exec(sandbox_id, ExecRequest(code="open('/work/b.txt','w').write('2')"))

    assert [f.path for f in second.files_out] == ["/work/b.txt"]

    third = await sandbox.exec(sandbox_id, ExecRequest(code="open('/work/a.txt','w').write('333')"))
    assert [f.path for f in third.files_out] == ["/work/a.txt"]   # 改了就要报


async def test_exec_user_code_error_is_not_a_sandbox_failure(sandbox, spec):
    """代码自己抛异常 → 退出码非 0 + traceback 进 stderr，但 exec 本身不抛。"""
    sandbox_id = await sandbox.acquire("t3-user-error", spec)

    result = await sandbox.exec(sandbox_id, ExecRequest(code="raise ValueError('炸了')"))

    assert result.exit_code == 1
    assert "ValueError" in result.stderr
    assert "炸了" in result.stderr


async def test_exec_truncates_output_over_the_cap(sandbox, spec):
    """§3.2：stdout/stderr 超 MAX_EXEC_OUTPUT_CHARS 要截断并置 truncated=True。"""
    sandbox_id = await sandbox.acquire("t3-truncate", spec)

    result = await sandbox.exec(
        sandbox_id,
        ExecRequest(code=f"print('x' * {MAX_EXEC_OUTPUT_CHARS * 2})", timeout_sec=30),
    )

    assert result.exit_code == 0, result.stderr
    assert result.truncated is True
    assert len(result.stdout) == MAX_EXEC_OUTPUT_CHARS


async def test_exec_timeout_is_enforced_inside_the_container(sandbox, spec):
    """超时由容器内的 coreutils timeout 强制执行，退出码统一成 124。"""
    sandbox_id = await sandbox.acquire("t3-timeout", spec)

    result = await sandbox.exec(
        sandbox_id, ExecRequest(code="import time; time.sleep(60)", timeout_sec=2)
    )

    assert result.exit_code == 124
    assert "超过 2s" in result.stderr
    assert result.duration_ms < 30_000        # 真被杀掉了，不是等满 60 秒


async def test_put_and_get_file_round_trip(sandbox, spec):
    sandbox_id = await sandbox.acquire("t3-put-get", spec)
    payload = "月份,销量\n2026-01,120\n2026-02,140\n".encode()

    await sandbox.put_file(sandbox_id, "/work/in/sales.csv", payload)

    assert await sandbox.get_file(sandbox_id, "/work/in/sales.csv") == payload

    # 沙箱里的代码要读得动、也要改得动（文件属主必须是容器里那个 uid 1000 的用户）
    result = await sandbox.exec(
        sandbox_id,
        ExecRequest(
            code=(
                "import pandas as pd\n"
                "df = pd.read_csv('/work/in/sales.csv')\n"
                "print('rows', len(df))\n"
                "open('/work/in/sales.csv', 'a').write('2026-03,90\\n')\n"
            ),
            timeout_sec=60,
        ),
    )
    assert result.exit_code == 0, result.stderr
    assert "rows 2" in result.stdout


async def test_put_file_rejects_paths_outside_work(sandbox, spec):
    sandbox_id = await sandbox.acquire("t3-path-guard", spec)

    with pytest.raises(SandboxPathError):
        await sandbox.put_file(sandbox_id, "/etc/cron.d/pwn", b"x")
    with pytest.raises(SandboxPathError):
        await sandbox.get_file(sandbox_id, "/work/../etc/passwd")


async def test_get_file_missing_raises_file_not_found(sandbox, spec):
    """§3.3：final.artifacts 指到不存在的文件时 worker 要跳过，用标准异常接得住。"""
    sandbox_id = await sandbox.acquire("t3-missing-file", spec)

    with pytest.raises(SandboxFileNotFound):
        await sandbox.get_file(sandbox_id, "/work/never-written.png")
    assert issubclass(SandboxFileNotFound, FileNotFoundError)


async def test_list_files_lists_work_tree(sandbox, spec):
    sandbox_id = await sandbox.acquire("t3-list", spec)

    assert await sandbox.list_files(sandbox_id) == []

    await sandbox.put_file(sandbox_id, "/work/b.txt", b"2")
    await sandbox.put_file(sandbox_id, "/work/in/a.txt", b"1")

    assert await sandbox.list_files(sandbox_id) == ["/work/b.txt", "/work/in/a.txt"]


async def test_unknown_sandbox_id_is_rejected(sandbox):
    with pytest.raises(SandboxNotFound):
        await sandbox.exec("deadbeef" * 8, ExecRequest(code="print(1)"))
    with pytest.raises(SandboxNotFound):
        await sandbox.list_files("deadbeef" * 8)


async def test_release_is_idempotent(sandbox, spec, labelled_ids):
    """§3.2：release 幂等。"""
    sandbox_id = await sandbox.acquire("t3-release", spec)
    assert sandbox_id in labelled_ids()

    await sandbox.release(sandbox_id)
    await sandbox.release(sandbox_id)              # 第二遍不许炸
    await sandbox.release("no-such-sandbox-id")    # 压根不认识的也不许炸

    assert sandbox_id not in labelled_ids()


async def test_touch_pushes_back_the_reaper(sandbox, spec, labelled_ids):
    """§3.2：touch 刷新最近活动时间 —— 刷过之后就不该被 reap_idle 收走。"""
    sandbox_id = await sandbox.acquire("t3-touch", spec)

    await asyncio.sleep(1.2)
    await sandbox.touch(sandbox_id)
    assert await sandbox.reap_idle(1) == []
    assert sandbox_id in labelled_ids()

    await asyncio.sleep(1.2)
    assert await sandbox.reap_idle(1) == [sandbox_id]

    await sandbox.touch("no-such-sandbox-id")      # 提示性动作，不认识也不许炸


async def test_reap_idle_keeps_fresh_sandboxes(sandbox, spec, labelled_ids):
    """刚用过的沙箱不能被收 —— 否则任务跑一半容器就没了。"""
    sandbox_id = await sandbox.acquire("t3-fresh", spec)

    assert await sandbox.reap_idle(300) == []
    assert sandbox_id in labelled_ids()


async def test_reap_idle_collects_orphans_from_another_instance(sandbox, spec, labelled_ids):
    """进程重启后留下的孤儿容器（本进程没记账）也要能收 —— 这是 B4 那条断言的兜底。"""
    stranger = DockerSandbox()
    try:
        orphan = await stranger.acquire("t3-orphan", spec)
        stranger._boxes.clear()          # 假装这是上一个进程留下的，本对象不认识它
    finally:
        await stranger.aclose()

    assert orphan in labelled_ids()
    await asyncio.sleep(1.2)
    assert orphan in await sandbox.reap_idle(1)
    assert orphan not in labelled_ids()


async def test_b4_matplotlib_png_and_reap_leaves_nothing(sandbox, spec, labelled_ids):
    """§2.2 B4 逐条：PNG 魔数 + reap_idle(1) 后 docker ps -a 为空。"""
    sandbox_id = await sandbox.acquire("t3-b4", spec)

    result = await sandbox.exec(
        sandbox_id,
        ExecRequest(
            code=(
                "import matplotlib.pyplot as plt\n"
                "fig, ax = plt.subplots()\n"
                "ax.plot([1, 2, 3], [120, 140, 90], marker='o')\n"
                "ax.set_title('月度销量趋势')\n"
                "ax.set_xlabel('月份')\n"
                "ax.set_ylabel('销量（件）')\n"
                "fig.savefig('/work/out.png')\n"
                "print('saved')\n"
            ),
            timeout_sec=120,
        ),
    )

    assert result.exit_code == 0, result.stderr
    assert "saved" in result.stdout
    # 中文字体真的生效了：缺字形的话 matplotlib 会往 stderr 刷
    # "Glyph 26376 (\N{CJK UNIFIED IDEOGRAPH-6708}) missing from font(s)"
    assert "missing from font" not in result.stderr
    assert "/work/out.png" in [f.path for f in result.files_out]

    png = await sandbox.get_file(sandbox_id, "/work/out.png")
    assert png[:8] == PNG_MAGIC
    assert len(png) > 1000

    await asyncio.sleep(1.2)
    released = await sandbox.reap_idle(1)
    assert sandbox_id in released
    assert labelled_ids() == []
