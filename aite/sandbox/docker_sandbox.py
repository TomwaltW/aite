"""SandboxPort 的 Docker 实现骨架（owner: T3，契约见 aite/contracts/ports.py §3.2）。"""
from ..contracts import ExecRequest, ExecResult, SandboxSpec


class DockerSandbox:
    """SandboxPort（T3）。"""

    async def acquire(self, task_id: str, spec: SandboxSpec) -> str:
        raise NotImplementedError("T3")

    async def exec(self, sandbox_id: str, req: ExecRequest) -> ExecResult:
        raise NotImplementedError("T3")

    async def put_file(self, sandbox_id: str, path: str, data: bytes) -> None:
        raise NotImplementedError("T3")

    async def get_file(self, sandbox_id: str, path: str) -> bytes:
        raise NotImplementedError("T3")

    async def list_files(self, sandbox_id: str) -> list[str]:
        raise NotImplementedError("T3")

    async def touch(self, sandbox_id: str) -> None:
        raise NotImplementedError("T3")

    async def release(self, sandbox_id: str) -> None:
        raise NotImplementedError("T3")

    async def reap_idle(self, idle_sec: int) -> list[str]:
        raise NotImplementedError("T3")
