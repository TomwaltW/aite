"""Adapter 抛给上层的错误（dev-spec §3.3 失败面那两行）。

§3.3 只给了签名 `PlatformError(code, retryable=True)`，`aite/contracts/**` 里没有
这个名字（T0 落的契约里确实没有，grep 过）。它是 adapter *抛出*的东西、不是跨轨
调用面，所以按 §3.4 归属表落在 T1 自己的目录里，不去动只读的契约包。
"""
from __future__ import annotations


class PlatformError(RuntimeError):
    """平台 API 调用失败。

    `retryable` 的含义按 §3.3 定死：

    * 429 / 5xx / 网络层错误 —— adapter 内部已经退避重试过 3 次（0.5s/1s/2s）仍失败，
      抛 `retryable=True`，上层可以再排队重来。
    * 4xx（非 429）—— 参数/权限问题，重试没有意义，抛 `retryable=False`。
    """

    def __init__(
        self,
        code: str | int,
        message: str = "",
        *,
        retryable: bool = False,
        http_status: int | None = None,
    ) -> None:
        self.code = code
        self.message = message
        self.retryable = retryable
        self.http_status = http_status
        super().__init__(f"[{code}] {message}" if message else f"[{code}]")

    def __repr__(self) -> str:  # pragma: no cover - 只影响报错可读性
        return (
            f"PlatformError(code={self.code!r}, message={self.message!r}, "
            f"retryable={self.retryable!r}, http_status={self.http_status!r})"
        )
