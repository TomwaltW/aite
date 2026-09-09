"""`run_python`（owner: T3）—— 在无网络沙箱里执行 Python，工作目录 `/work`。

两件容易搞混的事，这里定死：

1. **用户代码报错不是工具失败。** 代码抛异常、退出码非 0，工具照样 `ok=True`，
   traceback 原样进 `content` —— 模型看到就能自己改。反过来把它算成
   `code=sandbox` 会撞上 §3.3 的「连续 2 次沙箱失败 → task failed」，
   两个语法错就把任务打死了，显然不对。`code=sandbox` 只留给沙箱本身出问题
   （Docker 不可用、容器没了、写不进 /work）。

2. **超时是工具失败。** §3.3：「工具执行超时 → ToolResult(ok=False, code=timeout)」。
   代码的时限由 `DockerSandbox` 在容器内用 coreutils `timeout` 强制执行，超时会把
   退出码统一成 `EXEC_TIMEOUT_EXIT_CODE`(124)；这里见到 124 就翻成 timeout。
   Gateway 外层的 `asyncio.wait_for` 是同一件事的兜底（Docker daemon 卡住时用），
   两条路都走到 `code=timeout`，调用方看到的结果一致。
"""
import contextlib
import mimetypes
from typing import Any

from ..contracts import ArtifactRef, ExecRequest, ExecResult, ToolContext, ToolErrorCode
from ..sandbox import EXEC_TIMEOUT_EXIT_CODE, WORKDIR
from .base import ToolEnv, ToolFailure, ToolOutcome, require_sandbox

__all__ = ["run_python"]

_DEFAULT_TIMEOUT_SEC = 120        # 与 §3.1 GATEWAY_TOOLS 里 run_python 的 schema default 一致


async def run_python(env: ToolEnv, ctx: ToolContext, args: dict[str, Any]) -> ToolOutcome:
    sandbox = require_sandbox(env)
    code = args["code"]
    timeout_sec = int(args.get("timeout_sec", _DEFAULT_TIMEOUT_SEC))

    try:
        sandbox_id = await env.acquire_sandbox()
    except Exception as exc:
        raise ToolFailure(ToolErrorCode.sandbox, f"沙箱起不来：{exc}") from exc

    try:
        result = await sandbox.exec(sandbox_id, ExecRequest(code=code, timeout_sec=timeout_sec))
    except Exception as exc:
        raise ToolFailure(ToolErrorCode.sandbox, f"沙箱执行失败：{exc}") from exc

    with contextlib.suppress(Exception):
        await sandbox.touch(sandbox_id)       # 刷新空闲计时，别让 reaper 在任务中途收走

    if result.exit_code == EXEC_TIMEOUT_EXIT_CODE:
        raise ToolFailure(
            ToolErrorCode.timeout,
            f"代码执行超过 {timeout_sec}s 上限，已在沙箱内终止",
        )

    return ToolOutcome(
        content=_render(result),
        data={
            "exit_code": result.exit_code,
            "stdout": result.stdout,
            "stderr": result.stderr,
            "truncated": result.truncated,
            "duration_ms": result.duration_ms,
            "files_out": [f.model_dump(mode="json") for f in result.files_out],
        },
        artifacts=[
            ArtifactRef(
                path=f.path,
                title=f.path.rsplit("/", 1)[-1],
                mime=mimetypes.guess_type(f.path)[0],
            )
            for f in result.files_out
        ],
    )


def _render(result: ExecResult) -> str:
    parts: list[str] = []
    if result.exit_code == 0:
        parts.append(f"执行成功（{result.duration_ms} ms）")
    else:
        parts.append(f"代码以退出码 {result.exit_code} 结束（{result.duration_ms} ms）")

    parts.append(f"stdout:\n{result.stdout}" if result.stdout.strip() else "stdout: （空）")
    if result.stderr.strip():
        parts.append(f"stderr:\n{result.stderr}")
    if result.truncated:
        parts.append("注意：输出太长已被截断。")

    if result.files_out:
        listing = "\n".join(f"  {f.path}（{f.size} 字节）" for f in result.files_out)
        parts.append(f"{WORKDIR} 下本次新增/修改的文件：\n{listing}")
    else:
        parts.append(f"本次没有在 {WORKDIR} 下产出文件。")

    return "\n\n".join(parts)
