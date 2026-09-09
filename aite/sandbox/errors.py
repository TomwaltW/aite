"""沙箱层的异常（owner: T3）。

Gateway 侧把这些一律翻成 `ToolResult(ok=False, code=sandbox)`（§3.3 失败面），
所以这里只负责把「为什么失败」说清楚，不负责决定错误码。
"""


class SandboxError(RuntimeError):
    """沙箱层一切失败的基类。"""


class SandboxPathError(SandboxError, ValueError):
    """路径不在 /work 下，或者是相对路径 / 越界的 `..`。"""


class SandboxNotFound(SandboxError):
    """给的 sandbox_id 不认识（没 acquire 过，或者已经 release 了）。"""


class SandboxFileNotFound(SandboxError, FileNotFoundError):
    """沙箱里没有这个文件。

    同时继承 FileNotFoundError：§3.3 里 `final.artifacts` 指到不存在的文件时
    worker 要「跳过该产物」，用标准异常类型接得住。
    """
