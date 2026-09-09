"""调用记账（T4 官方替身共用）。

替身自己不做断言，只把「谁在什么时候被以什么参数调用了」记下来；断言留给
tests/e2e 与 evals 的 expect DSL。这样同一份替身既能给单测用，也能给场景
runner 用，不必为每种断言各写一个替身。
"""
from __future__ import annotations

import itertools
from dataclasses import dataclass, field
from typing import Any

_SEQ = itertools.count()


@dataclass(slots=True)
class Call:
    """一次被调用的记录。error 非空表示这次调用以异常收场。"""

    method: str
    kwargs: dict[str, Any] = field(default_factory=dict)
    result: Any = None
    error: str | None = None
    seq: int = 0

    def __repr__(self) -> str:  # 失败信息里要能一眼看出调了什么
        args = ", ".join(f"{k}={v!r}" for k, v in self.kwargs.items())
        tail = f" !{self.error}" if self.error else ""
        return f"<Call#{self.seq} {self.method}({args}){tail}>"


class CallLog:
    """按调用顺序保存 Call；seq 全局单调，跨替身也能排出先后。"""

    def __init__(self) -> None:
        self._calls: list[Call] = []

    def record(self, method: str, **kwargs: Any) -> Call:
        call = Call(method=method, kwargs=kwargs, seq=next(_SEQ))
        self._calls.append(call)
        return call

    def of(self, method: str) -> list[Call]:
        return [c for c in self._calls if c.method == method]

    def count(self, method: str) -> int:
        return sum(1 for c in self._calls if c.method == method)

    def methods(self) -> list[str]:
        return [c.method for c in self._calls]

    def last(self, method: str) -> Call | None:
        found = self.of(method)
        return found[-1] if found else None

    def clear(self) -> None:
        self._calls.clear()

    @property
    def calls(self) -> list[Call]:
        return list(self._calls)

    def __len__(self) -> int:
        return len(self._calls)

    def __iter__(self):
        return iter(self._calls)

    def __repr__(self) -> str:
        return f"<CallLog {len(self._calls)} calls: {self.methods()}>"
