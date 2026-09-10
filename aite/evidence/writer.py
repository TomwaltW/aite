"""EvidenceWriter 的落盘实现（owner: T2，契约见 aite/contracts/ports.py §3.2）。

目录形状（§3.1 末尾写死）：

    {evidence_dir}/{task_id}/events.jsonl     一行一个 EvidenceEvent
    {evidence_dir}/{task_id}/manifest.json    {task_id, session_id, task_no, created_by,
                                               model, contract_version, root_hash, event_count}
    {evidence_dir}/{task_id}/payloads/{seq}.json   仅当单条 payload > 64KB

链式 hash 全部走契约里的 `payload_hash_of` / `chain_hash`，本文件不自己算 sha256，
免得哪天两处实现漂开。

**崩溃残行（T21）**：进程在 `fh.write(...)` 中途没了、或者磁盘写满只落了一半，
`events.jsonl` 末尾会留下一段没有换行符收尾的字节。写入侧（`append` / `finalize`）
进门先走一次 `_heal_torn_tail` 把它收拾掉，否则 §3.3 要求的收尾链会自锁 ——
「任何未捕获异常 → task failed + 回帖 + evidence failed 事件」里，写那条 failed 证据
本身就是炸的那个操作，越出事越写不进去。

口径就一条，**换行符是记录终止符**：

* 文件以换行符收尾 → 每条记录都完整落盘过，末尾没有残行；此时任何解析不了的行
  都是**中间被改坏**的，一律照旧抛出去 / `verify()` 报 False，不许在这里「修复」。
* 文件不以换行符收尾 → 末段没有终止符。它要么是没写完的半条（丢弃），要么是整条
  写完了只差那个换行符（补上，一个字节都不丢）。两者靠「这段能不能解析成
  EvidenceEvent 并接得住前面的链」分开 —— 也就是 `verify()` 本来就用的那把尺子。

由此得到的性质：收拾只丢弃 `verify()` 本来就会拒绝的字节，一条它认可的记录都不动 ——
所以收拾只可能把 `verify()` 从 False 修回 True，绝不会反过来。`verify()` 自己**不**
收拾，它看到的永远是文件此刻的真实样子：残行在它那里照旧是 False。
"""
import asyncio
import json
import logging
from collections import defaultdict
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

log = logging.getLogger("aite.evidence")

# 超过这个大小的 payload 不内联，落到 payloads/{seq}.json（§3.1 EvidenceEvent.payload_ref）
INLINE_PAYLOAD_MAX_BYTES = 64 * 1024

EVENTS_FILE = "events.jsonl"
MANIFEST_FILE = "manifest.json"
PAYLOAD_DIR = "payloads"

# 残行原文进 WARNING 日志时截到这么长。截掉的那半行不另存文件（它从来没有效过），
# 日志里这段前缀加上字节数就是它留下的全部痕迹。
TORN_TAIL_LOG_CLIP = 200

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
        # 崩溃恢复留痕（§3.3 排障用）。静默截掉一条记录跟静默坏链一样难查，
        # 所以「发生过一次崩溃恢复」必须能被看见：这里计数，同时打 WARNING。
        self.counters: dict[str, int] = defaultdict(int)

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
            # 上一条命可能写到一半。这一步必须在算 tip 之前，也必须真去看文件 ——
            # 磁盘满那一路 `_tip` 还是热的、自以为写成功了，只有文件知道真相。
            self._heal_torn_tail(task_id)
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
            # finalize 也是写入侧入口，崩溃后最先被调到的往往就是它。收拾之后
            # event_count 少掉的那一条正是没写完的半行 —— 它从来不是一条事件。
            self._heal_torn_tail(task_id)
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
        """重算整条链：seq 连续、payload 与 payload_hash 对得上、hash 链闭合。

        **只读**：不做任何崩溃恢复。残行、被改坏的中间行，在这里一律是 False ——
        它回答的是「文件此刻可不可信」，不是「收拾一下还能不能救」。
        """
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

    # ---- 崩溃残行 --------------------------------------------------------

    def _heal_torn_tail(self, task_id: str) -> None:
        """收拾 `events.jsonl` 末尾那段没有换行符收尾的字节（口径见模块 docstring）。

        热路径的代价是一次 `stat` 加读最后 1 字节，与文件长度无关 —— 每条证据都会
        走这里，不能全文读一遍。只有真发现残行时才升级成读全文。
        """
        path = self.events_path(task_id)
        if not path.exists():
            return
        size = path.stat().st_size
        if size == 0:
            return                                   # 空文件没有残行
        with path.open("rb") as fh:
            fh.seek(size - 1)
            if fh.read(1) == b"\n":
                return                               # 每条记录都有终止符，正常路径到此为止

        # 按字节切，不 decode 全文：残行可能截在一个 UTF-8 多字节字符中间，
        # `read_text` 会先炸在 UnicodeDecodeError 上，那就等于没修。
        raw = path.read_bytes()
        cut = raw.rfind(b"\n") + 1                   # 末段起点；整个文件都没换行时为 0
        tail = raw[cut:]
        shown = tail.decode("utf-8", errors="replace")[:TORN_TAIL_LOG_CLIP]

        if self._tail_is_a_whole_record(task_id, raw[:cut], tail):
            # 整条写完了，只差那个换行符：补上，一个字节都不丢（`verify()` 本来就认它）
            with path.open("ab") as fh:
                fh.write(b"\n")
            self.counters["evidence.torn_tail_kept"] += 1
            log.warning(
                "证据链 %s 末尾少了换行符，末段是完整的一条，已补齐（%d 字节）：%s",
                path, len(tail), shown,
            )
        else:
            # 真残行：从来没写完，也就从来没有效过。截到最后一个完整记录的换行处。
            with path.open("r+b") as fh:
                fh.truncate(cut)
            self.counters["evidence.torn_tail_dropped"] += 1
            log.warning(
                "证据链 %s 末尾有 %d 字节没写完，已截掉 —— 上一个进程崩在 write 中途，"
                "或者磁盘写满了。残行原文：%s",
                path, len(tail), shown,
            )
        self._tip.pop(task_id, None)                 # 缓存跟文件对不上了，下一次重算

    def _tail_is_a_whole_record(self, task_id: str, head: bytes, tail: bytes) -> bool:
        """末段虽然没有换行符收尾，但它其实是完整的一条吗？

        判据用的就是 `verify()` 那把尺子（解析得了、task_id 对、seq 接得上、hash 链闭合），
        所以收拾前后 `verify()` 的答案不会变。只解析 head 的最后一行，不重算整条链 ——
        head 中间要是有被改坏的行，那是篡改，轮不到这里处理。
        """
        try:
            ev = EvidenceEvent.model_validate_json(tail)
        except ValueError:
            return False
        if ev.task_id != task_id:
            return False
        lines = [ln for ln in head.split(b"\n") if ln.strip()]
        if ev.seq != len(lines):
            return False
        if lines:
            try:
                prev_hash = EvidenceEvent.model_validate_json(lines[-1]).hash
            except ValueError:
                return False
        else:
            prev_hash = GENESIS
        return ev.prev_hash == prev_hash and ev.hash == chain_hash(prev_hash, ev.payload_hash)

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
        """读全文。行解析不了就抛 —— 调用方都先过了 `_heal_torn_tail`，所以走到这里
        还坏的行只可能是中间被改坏的，不该被吞掉。"""
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
