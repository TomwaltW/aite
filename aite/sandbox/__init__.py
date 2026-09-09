"""Docker 沙箱（T3）。

这里只 re-export 不依赖 docker SDK 的公共面；`DockerSandbox` 要从
`aite.sandbox.docker_sandbox` 显式 import，免得只想用路径规范化的调用方
（比如 Gateway 的工具层）被拖着一起 import docker。
"""
from .errors import (
    SandboxError,
    SandboxFileNotFound,
    SandboxNotFound,
    SandboxPathError,
)
from .workdir import (
    EXEC_TIMEOUT_EXIT_CODE,
    WORKDIR,
    normalize_work_path,
    require_file_path,
)

__all__ = [
    "EXEC_TIMEOUT_EXIT_CODE",
    "WORKDIR",
    "SandboxError",
    "SandboxFileNotFound",
    "SandboxNotFound",
    "SandboxPathError",
    "normalize_work_path",
    "require_file_path",
]
