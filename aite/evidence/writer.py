"""EvidenceWriter 的落盘实现（owner: T2，契约见 aite/contracts/ports.py §3.2）。

目录形状（§3.1 末尾写死）：

    {evidence_dir}/{task_id}/events.jsonl     一行一个 EvidenceEvent
    {evidence_dir}/{task_id}/manifest.json    {task_id, session_id, task_no, created_by,
                                               model, contract_version, root_hash, event_count}
    {evidence_dir}/{task_id}/payloads/{seq}.json   仅当单条 payload > 64KB

链式 hash 全部走契约里的 `payload_hash_of` / `chain_hash`，本文件不自己算 sha256，
免得哪天两处实现漂开。
"""
import asyncio
import json
from datetime import UTC, datetime
from pathlib import Path

from ..contracts import (
    CONTRACT_VERSION,
    GENESIS,
    EvidenceEvent,
    EvidenceKind,
    canonical_json,
    chain_hash,
    payload_hash_of,
)

# 超过这个大小的 payload 不内联，落到 payloads/{seq}.json（§3.1 EvidenceEvent.payload_ref）
INLINE_PAYLOAD_MAX_BYTES = 64 * 1024

EVENTS_FILE = "events.jsonl"
MANIFEST_FILE = "manifest.json"
PAYLOAD_DIR = "payloads"

# manifest 的字段就是 §3.1 写死的这一组，不多不少
_MANIFEST_DESCRIPTIVE_FIELDS = ("session_id", "task_no", "created_by", "model")


class FileEvidenceWriter:
    """EvidenceWriter（T2）。"""

    def __init__(self, evidence_dir: str | Path = "data/evidence") -> None:
        self._root = Path(evidence_dir)
        self._lock = asyncio.Lock()
        # task_id -> (下一个 seq, 上一条 hash)。只是省掉每次 append 重读整个 jsonl
        # （40 步的任务有一百多条事件，不缓存就是 O(n²) 次解析）。
        # 首次接触某个任务时仍从文件恢复，所以进程重启、换实例都接得上；
        # P0 里一个任务只有一个 writer 实例在写，缓存不会和别人打架。
        self._tip: dict[str, tuple[int, str]] = {}

    # ---- 路径 ------------------------------------------------------------

    def task_dir(self, task_id: str) -> Path:
        """某个任务的证据目录。R3 的 evidence 按钮要把这个路径回帖出去。"""
        return self._root / task_id

    def events_path(self, task_id: str) -> Path:
        return self.task_dir(task_id) / EVENTS_FILE

    def manifest_path(self, task_id: str) -> Path:
        return self.task_dir(task_id) / MANIFEST_FILE

    # ---- 写 --------------------------------------------------------------

    async def append(self, task_id: str, kind: EvidenceKind, payload: dict) -> EvidenceEvent:
        """追加一条证据。seq / prev_hash 由已落盘的文件推出来，进程重启也接得上。"""
        async with self._lock:
            seq, prev_hash = self._chain_tip(task_id)
            body = canonical_json(payload)
            p_hash = payload_hash_of(payload)

            payload_ref: str | None = None
            inline: dict | None = payload
            if len(body.encode("utf-8")) > INLINE_PAYLOAD_MAX_BYTES:
                rel = f"{PAYLOAD_DIR}/{seq}.json"
                out = self.task_dir(task_id) / rel
                out.parent.mkdir(parents=True, exist_ok=True)
                out.write_text(body, encoding="utf-8")
                payload_ref, inline = rel, None

            ev = EvidenceEvent(
                task_id=task_id,
                seq=seq,
                kind=kind,
                payload_hash=p_hash,
                # payload_ref / payload 在契约里没写 `= None`，pydantic 下两者都是必填，
                # 省略会 ValidationError。这是契约原样，不是笔误。
                payload_ref=payload_ref,
                payload=inline,
                prev_hash=prev_hash,
                hash=chain_hash(prev_hash, p_hash),
                created_at=datetime.now(UTC),
            )
            path = self.events_path(task_id)
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("a", encoding="utf-8") as fh:
                fh.write(ev.model_dump_json() + "\n")
            self._tip[task_id] = (ev.seq + 1, ev.hash)
            return ev

    async def finalize(self, task_id: str, manifest_extra: dict) -> str:
        """写 manifest.json，返回 root_hash（= 最后一条的 hash；没有事件则为 GENESIS）。"""
        async with self._lock:
            events = self._read_events(task_id)
            root_hash = events[-1].hash if events else GENESIS
            manifest = {
                "task_id": task_id,
                **{f: manifest_extra.get(f, "") for f in _MANIFEST_DESCRIPTIVE_FIELDS},
                "contract_version": CONTRACT_VERSION,
                "root_hash": root_hash,
                "event_count": len(events),
            }
            path = self.manifest_path(task_id)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(
                json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            return root_hash

    # ---- 校验 ------------------------------------------------------------

    def verify(self, task_id: str) -> bool:
        """重算整条链：seq 连续、payload 与 payload_hash 对得上、hash 链闭合。"""
        path = self.events_path(task_id)
        if not path.exists():
            return False
        prev = GENESIS
        for i, line in enumerate(path.read_text(encoding="utf-8").splitlines()):
            if not line.strip():
                continue
            try:
                ev = EvidenceEvent.model_validate_json(line)
            except ValueError:
                return False
            if ev.task_id != task_id or ev.seq != i:
                return False
            payload = self._resolve_payload(task_id, ev)
            if payload is None:
                return False
            if payload_hash_of(payload) != ev.payload_hash:
                return False
            if ev.prev_hash != prev or ev.hash != chain_hash(prev, ev.payload_hash):
                return False
            prev = ev.hash
        return True

    # ---- 内部 ------------------------------------------------------------

    def _chain_tip(self, task_id: str) -> tuple[int, str]:
        cached = self._tip.get(task_id)
        if cached is not None:
            return cached
        events = self._read_events(task_id)
        tip = (0, GENESIS) if not events else (events[-1].seq + 1, events[-1].hash)
        self._tip[task_id] = tip
        return tip

    def _read_events(self, task_id: str) -> list[EvidenceEvent]:
        path = self.events_path(task_id)
        if not path.exists():
            return []
        return [
            EvidenceEvent.model_validate_json(line)
            for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]

    def _resolve_payload(self, task_id: str, ev: EvidenceEvent) -> dict | None:
        if ev.payload is not None:
            return ev.payload
        if ev.payload_ref is None:
            return None
        ref = self.task_dir(task_id) / ev.payload_ref
        if not ref.exists():
            return None
        try:
            loaded = json.loads(ref.read_text(encoding="utf-8"))
        except ValueError:
            return None
        return loaded if isinstance(loaded, dict) else None
