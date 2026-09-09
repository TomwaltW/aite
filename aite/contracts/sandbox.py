# aite/contracts/sandbox.py
from typing import Literal

from pydantic import BaseModel, Field


class SandboxSpec(BaseModel):
    image: str
    cpu: float = 1.0
    mem_mb: int = 1024
    network: Literal["none"] = "none"   # P0 沙箱无网络，凭证/平台调用都不在沙箱里发生
    workdir: str = "/work"

class ExecRequest(BaseModel):
    language: Literal["python"] = "python"
    code: str
    timeout_sec: int = 120

class FileEntry(BaseModel):
    path: str
    size: int

class ExecResult(BaseModel):
    exit_code: int
    stdout: str
    stderr: str
    duration_ms: int
    truncated: bool = False              # stdout/stderr 超过 MAX_EXEC_OUTPUT_CHARS 被截断
    files_out: list[FileEntry] = Field(default_factory=list)   # /work 下本次新增/修改的文件

MAX_EXEC_OUTPUT_CHARS = 20000
