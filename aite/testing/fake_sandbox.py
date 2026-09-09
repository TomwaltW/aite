"""FakeSandbox —— SandboxPort 的替身（dev-spec §3.2），内存文件系统 + 脚本化 exec。

`exec` 不真跑 Python：场景在 `exec_script` 里声明「代码里出现某个子串时，返回什么
exit_code / stdout，并往 /work 下写哪些文件」。04_csv_to_chart 靠 `writes` 造出
`/work/out.png`，再由 `final(artifacts)` 走 `get_file` → `send_file`。

时钟可注入，`reap_idle` 因此能在测试里被确定性地触发，不用真等 5 分钟。
"""
from __future__ import annotations

import itertools
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from pydantic import BaseModel, Field

from ..contracts import ExecRequest, ExecResult, FileEntry, SandboxSpec
from .recorder import CallLog
from .samples import BUILTINS


class FakeSandboxError(RuntimeError):
    """沙箱替身自己判定的错（未知 sandbox_id、路径越界等）。"""


def as_bytes(value: Any) -> bytes:
    """场景 yaml 里的文件内容：str / bytes / "builtin:png" / {b64: ...}。"""
    if isinstance(value, bytes):
        return value
    if isinstance(value, dict):
        if "b64" in value:
            import base64

            return base64.b64decode(value["b64"])
        if "builtin" in value:
            return BUILTINS[value["builtin"]]
        raise FakeSandboxError(f"看不懂的文件内容声明：{value!r}")
    if isinstance(value, str):
        if value.startswith("builtin:"):
            key = value.split(":", 1)[1]
            if key not in BUILTINS:
                raise FakeSandboxError(f"没有内置样本 {key!r}，可用：{sorted(BUILTINS)}")
            return BUILTINS[key]
        return value.encode("utf-8")
    raise FakeSandboxError(f"看不懂的文件内容声明：{value!r}")


class ExecScriptStep(BaseModel):
    """一条 exec 脚本。match 为 None 时匹配任何代码。"""

    match: str | None = None
    exit_code: int = 0
    stdout: str = ""
    stderr: str = ""
    duration_ms: int = 5
    truncated: bool = False
    #: 沙箱内路径 -> 内容；执行「成功」时写进内存 FS
    writes: dict[str, Any] = Field(default_factory=dict)
    #: 抛异常而不是返回结果，演 §3.3 的「沙箱创建/执行失败」
    error: str | None = None
    #: 可复用几次；None = 无限
    times: int | None = None


@dataclass
class _Box:
    sandbox_id: str
    task_id: str
    spec: SandboxSpec
    files: dict[str, bytes] = field(default_factory=dict)
    last_active: float = 0.0
    released: bool = False


class FakeSandbox:
    """SandboxPort 的替身。"""

    def __init__(
        self,
        *,
        exec_script: list[ExecScriptStep | dict[str, Any]] | None = None,
        clock: Callable[[], float] | None = None,
    ) -> None:
        self.calls = CallLog()
        self.exec_script: list[ExecScriptStep] = [
            s if isinstance(s, ExecScriptStep) else ExecScriptStep.model_validate(s)
            for s in (exec_script or [])
        ]
        self._used: dict[int, int] = {}
        self.clock = clock or time.monotonic
        self.boxes: dict[str, _Box] = {}
        self.released_ids: list[str] = []
        self._ids = itertools.count(1)

    # ---- SandboxPort ----------------------------------------------------

    async def acquire(self, task_id: str, spec: SandboxSpec) -> str:
        sandbox_id = f"sb-{next(self._ids)}"
        self.calls.record("acquire", task_id=task_id, image=spec.image, sandbox_id=sandbox_id)
        self.boxes[sandbox_id] = _Box(
            sandbox_id=sandbox_id, task_id=task_id, spec=spec, last_active=self.clock()
        )
        return sandbox_id

    async def exec(self, sandbox_id: str, req: ExecRequest) -> ExecResult:
        call = self.calls.record("exec", sandbox_id=sandbox_id, timeout_sec=req.timeout_sec)
        box = self._box(sandbox_id)
        box.last_active = self.clock()
        step = self._match(req.code)
        if step is None:
            call.result = "default"
            return ExecResult(exit_code=0, stdout="", stderr="", duration_ms=1)
        if step.error:
            call.error = step.error
            raise FakeSandboxError(step.error)
        files_out: list[FileEntry] = []
        for path, value in step.writes.items():
            data = as_bytes(value)
            self._check_path(path)
            box.files[path] = data
            files_out.append(FileEntry(path=path, size=len(data)))
        call.result = f"exit={step.exit_code} files={[f.path for f in files_out]}"
        return ExecResult(
            exit_code=step.exit_code,
            stdout=step.stdout,
            stderr=step.stderr,
            duration_ms=step.duration_ms,
            truncated=step.truncated,
            files_out=files_out,
        )

    async def put_file(self, sandbox_id: str, path: str, data: bytes) -> None:
        self.calls.record("put_file", sandbox_id=sandbox_id, path=path, size=len(data))
        self._check_path(path)
        box = self._box(sandbox_id)
        box.files[path] = data
        box.last_active = self.clock()

    async def get_file(self, sandbox_id: str, path: str) -> bytes:
        call = self.calls.record("get_file", sandbox_id=sandbox_id, path=path)
        box = self._box(sandbox_id)
        if path not in box.files:
            call.error = "not found"
            raise FileNotFoundError(f"沙箱 {sandbox_id} 里没有 {path}（有的是：{sorted(box.files)}）")
        box.last_active = self.clock()
        return box.files[path]

    async def list_files(self, sandbox_id: str) -> list[str]:
        self.calls.record("list_files", sandbox_id=sandbox_id)
        return sorted(self._box(sandbox_id).files)

    async def touch(self, sandbox_id: str) -> None:
        self.calls.record("touch", sandbox_id=sandbox_id)
        self._box(sandbox_id).last_active = self.clock()

    async def release(self, sandbox_id: str) -> None:
        self.calls.record("release", sandbox_id=sandbox_id)
        box = self.boxes.get(sandbox_id)
        if box is None or box.released:      # 契约要求幂等
            return
        box.released = True
        self.released_ids.append(sandbox_id)

    async def reap_idle(self, idle_sec: int) -> list[str]:
        self.calls.record("reap_idle", idle_sec=idle_sec)
        now = self.clock()
        reaped = []
        for box in list(self.boxes.values()):
            if not box.released and now - box.last_active >= idle_sec:
                await self.release(box.sandbox_id)
                reaped.append(box.sandbox_id)
        return reaped

    # ---- 内部 -----------------------------------------------------------

    def _box(self, sandbox_id: str) -> _Box:
        box = self.boxes.get(sandbox_id)
        if box is None:
            raise FakeSandboxError(f"未知 sandbox_id={sandbox_id!r}（在册：{sorted(self.boxes)}）")
        if box.released:
            raise FakeSandboxError(f"沙箱 {sandbox_id} 已经 release 过了")
        return box

    @staticmethod
    def _check_path(path: str) -> None:
        if not path.startswith("/work"):
            raise FakeSandboxError(f"路径必须在 /work 下（§3.2 put_file）：{path!r}")

    def _match(self, code: str) -> ExecScriptStep | None:
        for i, step in enumerate(self.exec_script):
            if step.match is not None and step.match not in code:
                continue
            used = self._used.get(i, 0)
            if step.times is not None and used >= step.times:
                continue
            self._used[i] = used + 1
            return step
        return None

    # ---- 给断言用 --------------------------------------------------------

    @property
    def alive(self) -> list[str]:
        return sorted(sid for sid, b in self.boxes.items() if not b.released)

    def all_files(self) -> dict[str, dict[str, int]]:
        return {sid: {p: len(d) for p, d in b.files.items()} for sid, b in self.boxes.items()}
