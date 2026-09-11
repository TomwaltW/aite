//! aite-evidence —— EvidenceWriter 的文件实现 + `aite evidence show`
//! （对应旧 `aite/evidence/writer.py` 与 `scripts/evidence_show.py`）。owner: R3

pub mod cli;
pub mod writer;

pub use writer::{
    COUNTER_TORN_TAIL_DROPPED, COUNTER_TORN_TAIL_KEPT, EVENTS_FILE, FileEvidenceWriter,
    INLINE_PAYLOAD_MAX_BYTES, MANIFEST_FILE, PAYLOAD_DIR, TORN_TAIL_LOG_CLIP,
};
