"""ControlPlane 的实现骨架（owner: T2，契约见 aite/contracts/ports.py §3.2）。

路由规则 R1–R8 见 dev-spec §3.5。
"""
from ..contracts import NormalizedEvent


class InProcessControlPlane:
    """ControlPlane（T2）。"""

    async def handle_event(self, ev: NormalizedEvent) -> None:
        raise NotImplementedError("T2")

    async def run_forever(self) -> None:
        raise NotImplementedError("T2")
