"""/work 这个工作面的规矩（owner: T3）。

§3.2 只写了一句「path 必须在 /work 下」，这里把它变成一个能被测试咬住的函数：
路径必须是绝对路径，`.` / `..` 先在字符串层面归一，归一后必须落在 /work 里面。
不做符号链接解析 —— 那要进容器才知道；归一化是纯函数，进容器之前先挡一道。
"""
from pathlib import PurePosixPath

from .errors import SandboxPathError

WORKDIR = "/work"

# coreutils `timeout` 超时时的退出码（POSIX 惯例）。DockerSandbox 用它统一表达
# 「代码跑超时了」，Gateway 见到它就翻成 ToolErrorCode.timeout。
EXEC_TIMEOUT_EXIT_CODE = 124


def normalize_work_path(path: str, *, workdir: str = WORKDIR) -> PurePosixPath:
    """把沙箱内路径归一到 /work 下的绝对路径；越界就抛 SandboxPathError。"""
    if not isinstance(path, str) or not path.strip():
        raise SandboxPathError(f"路径不能为空：{path!r}")
    if not path.startswith("/"):
        raise SandboxPathError(f"必须是绝对路径：{path!r}")

    parts: list[str] = []
    for seg in PurePosixPath(path).parts[1:]:
        if seg == ".":
            continue
        if seg == "..":
            if not parts:
                raise SandboxPathError(f"路径越过了根目录：{path!r}")
            parts.pop()
            continue
        parts.append(seg)

    normalized = PurePosixPath("/", *parts)
    root = PurePosixPath(workdir)
    if normalized != root and root not in normalized.parents:
        raise SandboxPathError(f"路径必须在 {workdir} 下：{path!r}（归一化后是 {normalized}）")
    return normalized


def require_file_path(path: str, *, workdir: str = WORKDIR) -> PurePosixPath:
    """同上，但额外要求它是 /work 下的一个**文件**路径，不能就是 /work 本身。"""
    normalized = normalize_work_path(path, workdir=workdir)
    if normalized == PurePosixPath(workdir):
        raise SandboxPathError(f"{workdir} 是目录，不是文件")
    return normalized
