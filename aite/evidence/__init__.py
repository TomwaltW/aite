"""证据链落盘与校验（T2）。"""
from .writer import (
    EVENTS_FILE,
    INLINE_PAYLOAD_MAX_BYTES,
    MANIFEST_FILE,
    FileEvidenceWriter,
)

__all__ = [
    "EVENTS_FILE",
    "INLINE_PAYLOAD_MAX_BYTES",
    "MANIFEST_FILE",
    "FileEvidenceWriter",
]
