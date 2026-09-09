"""EvidenceWriter 的落盘实现骨架（owner: T2，契约见 aite/contracts/ports.py §3.2）。"""
from ..contracts import EvidenceEvent, EvidenceKind


class FileEvidenceWriter:
    """EvidenceWriter（T2）。"""

    async def append(self, task_id: str, kind: EvidenceKind, payload: dict) -> EvidenceEvent:
        raise NotImplementedError("T2")

    async def finalize(self, task_id: str, manifest_extra: dict) -> str:
        raise NotImplementedError("T2")

    def verify(self, task_id: str) -> bool:
        raise NotImplementedError("T2")
