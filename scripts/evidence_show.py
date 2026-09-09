#!/usr/bin/env python3
"""把 {evidence_dir}/{task_id}/ 里的证据链渲染成人读的时间线（owner: T10）。

真机验收（§2.4 的 M1–M6）出问题时没有 pytest 告诉你哪一行断言红了，只有群里一条
没回的消息。每个任务留下的 events.jsonl 是唯一的第一现场，但它一行一个几百字节的
JSON、payload 还只有 hash —— 今天只有机器读得了。这个脚本把它变成人读得懂的时间线。

用法：

    python scripts/evidence_show.py <task_id>          # 从默认 evidence_dir 找
    python scripts/evidence_show.py --dir data/evidence/<task_id>
    python scripts/evidence_show.py --list             # 列出所有任务（最近写入在前）
    python scripts/evidence_show.py <task_id> --only model_call,tool_call --tail 20
    python scripts/evidence_show.py <task_id> --json

退出码：0 = 链校验通过；1 = 链断了 / manifest 对不上 / payload 缺失；2 = 找不到任务。

三条纪律：

* **hash 链校验是骨头，不是装饰。** 断了要说清断在第几条、期望什么、实际什么，
  并且非零退出码 —— 人在真机排障时最先要知道的就是「这份证据还可不可信」。
* **没有 manifest.json 也要能渲染**（任务还在跑、或进程被杀没 finalize），
  这种情况打印「未 finalize」，不算损坏。
* **不打印密钥、token、消息全文。** 每种 kind 只挑白名单里的字段渲染，
  工具参数里键名带 token/secret/key 的一律 ***。W8 已经保证证据里不存模型全文，
  这里不把口子开回来。
"""
from __future__ import annotations

import argparse
import json
import sys
import unicodedata
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from aite.contracts import (  # noqa: E402  （上面那段 sys.path 兜底得先跑）
    GENESIS,
    AiteConfig,
    EvidenceEvent,
    EvidenceKind,
    chain_hash,
    payload_hash_of,
)

EVENTS_FILE = "events.jsonl"
MANIFEST_FILE = "manifest.json"

#: 终态，渲染时要显眼（§2.4 排障第一眼看的就是它）
TERMINAL_KINDS = frozenset({EvidenceKind.delivered, EvidenceKind.failed, EvidenceKind.cancelled})

#: 工具参数里键名撞上这些词就只打 ***。证据里本不该有密钥，这是第二道闸。
SECRET_HINTS = ("token", "secret", "password", "passwd", "api_key", "apikey", "credential", "auth")

MARK_TERMINAL = "★"
MARK_BROKEN = "✗"


# ---------------------------------------------------------------- 小工具


def _clip(text: object, limit: int) -> str:
    s = str(text).replace("\n", " ").replace("\r", " ").strip()
    return s if len(s) <= limit else s[: limit - 1] + "…"


def _pad(text: str, width: int) -> str:
    """按终端显示宽度补空格 —— 中文是双宽，用 str.ljust 会把整张表排歪。"""
    shown = sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in text)
    return text + " " * max(0, width - shown)


def _fmt_elapsed(sec: float) -> str:
    if sec < 60:
        return f"+{sec:6.2f}s"
    return f"+{int(sec) // 60:d}m{sec % 60:04.1f}s"


def _fmt_money(amount: float) -> str:
    return f"¥{amount:.4f}"


def _redact(key: str, value: object) -> str:
    if any(h in key.lower() for h in SECRET_HINTS):
        return "***"
    if isinstance(value, str):
        return _clip(value, 40)
    if isinstance(value, list | dict):
        return _clip(json.dumps(value, ensure_ascii=False), 40)
    return _clip(value, 40)


def _args_summary(arguments: object, limit: int = 72) -> str:
    if not isinstance(arguments, dict) or not arguments:
        return ""
    parts = [f"{k}={_redact(k, v)}" for k, v in arguments.items()]
    return _clip(", ".join(parts), limit)


# ---------------------------------------------------------------- 数据形状


@dataclass
class ChainIssue:
    """链上的一处问题。line 是 events.jsonl 的行号（1 起），排障时能直接 sed -n 定位。"""

    line: int
    seq: int | None
    problem: str
    expected: str = ""
    actual: str = ""

    def as_dict(self) -> dict[str, Any]:
        return {
            "line": self.line,
            "seq": self.seq,
            "problem": self.problem,
            "expected": self.expected,
            "actual": self.actual,
        }


@dataclass
class Row:
    seq: int
    line: int
    kind: str
    at: datetime | None
    elapsed: float
    detail: str
    fields: dict[str, Any]
    broken: bool = False
    note: str = ""

    def as_dict(self) -> dict[str, Any]:
        return {
            "seq": self.seq,
            "kind": self.kind,
            "at": self.at.isoformat() if self.at else None,
            "elapsed_sec": round(self.elapsed, 3),
            "detail": self.detail,
            "fields": self.fields,
            "broken": self.broken,
            "note": self.note,
        }


@dataclass
class Stats:
    events: int = 0
    span_sec: float = 0.0
    model_calls: int = 0
    tokens_in: int = 0
    tokens_out: int = 0
    cost: float = 0.0
    tool_calls: int = 0
    tool_failed: int = 0
    artifacts: int = 0
    terminal: str = ""
    unreadable: int = 0  # payload 拿不到的事件，不进上面任何一个计数

    def as_dict(self) -> dict[str, Any]:
        return {
            "events": self.events,
            "span_sec": round(self.span_sec, 3),
            "model_calls": self.model_calls,
            "tokens_in": self.tokens_in,
            "tokens_out": self.tokens_out,
            "tokens_total": self.tokens_in + self.tokens_out,
            "cost": round(self.cost, 6),
            "tool_calls": self.tool_calls,
            "tool_failed": self.tool_failed,
            "artifacts": self.artifacts,
            "terminal": self.terminal,
            "unreadable": self.unreadable,
        }


@dataclass
class Timeline:
    task_id: str
    task_dir: Path
    rows: list[Row] = field(default_factory=list)
    issues: list[ChainIssue] = field(default_factory=list)
    manifest: dict[str, Any] | None = None
    manifest_exists: bool = False
    manifest_problems: list[str] = field(default_factory=list)
    stats: Stats = field(default_factory=Stats)
    checked: int = 0
    root_hash: str = GENESIS

    @property
    def finalized(self) -> bool:
        """写过 manifest.json 就算 finalize 过 —— 哪怕它坏了（那是另一个问题，单独报）。"""
        return self.manifest_exists

    @property
    def ok(self) -> bool:
        """链可信 = 没有链上问题，也没有 manifest 对不上。未 finalize 不算不可信。"""
        return not self.issues and not self.manifest_problems

    def as_dict(self, rows: list[Row]) -> dict[str, Any]:
        return {
            "task_id": self.task_id,
            "dir": str(self.task_dir),
            "finalized": self.finalized,
            "manifest": self.manifest,
            "root_hash": self.root_hash,
            "chain": {
                "ok": self.ok,
                "checked": self.checked,
                "issues": [i.as_dict() for i in self.issues],
                "manifest_problems": self.manifest_problems,
            },
            "summary": self.stats.as_dict(),
            "events": [r.as_dict() for r in rows],
        }


# ---------------------------------------------------------------- 逐 kind 的详情


class _Detailer:
    """把 payload 翻成一行人话。checklist 的 id→文本要跨事件记，所以做成有状态的对象。"""

    def __init__(self, price_in: float = 0.0, price_out: float = 0.0) -> None:
        self.price_in = price_in
        self.price_out = price_out
        # checklist_check/fail 的 payload 只有 id 和 state，没有那一项的文本；文本只在
        # add 那条里出现过（items 是当时的全量清单，id 形如 c1/c2，位置即编号）。
        # 真机排障时光看到 "c3 → done" 是没用的，所以在这里把文本补回来。
        self.checklist: dict[str, str] = {}

    def item_text(self, item_id: object) -> str:
        text = self.checklist.get(str(item_id), "")
        return f"「{_clip(text, 24)}」" if text else ""

    def render(self, kind: str, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        fn = getattr(self, f"_k_{kind}", None)
        if fn is None:  # 契约以后加了新 kind，也不该让工具哑掉
            return _clip(json.dumps(p, ensure_ascii=False, sort_keys=True), 90), {}
        return fn(p)

    # -- 建任务 / 收事件 --------------------------------------------------

    def _k_task_created(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        keep = {k: p.get(k) for k in ("task_no", "title", "created_by", "chat_id", "session_id")}
        title = _clip(p.get("title", ""), 56)
        bits = [str(p.get("task_no", "")), f"「{title}」" if title else ""]
        bits += [f"by={p.get('created_by', '?')}", f"chat={p.get('chat_id', '?')}"]
        return " ".join(b for b in bits if b), keep

    def _k_event_received(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        keep = {k: p.get(k) for k in ("event_id", "kind", "message_id", "sender_id", "mentioned")}
        return (
            f"{p.get('kind', '?')} msg={p.get('message_id', '?')} "
            f"mentioned={p.get('mentioned')} event={p.get('event_id', '?')}"
        ), keep

    # -- 模型 -------------------------------------------------------------

    def _k_model_call(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        usage = p.get("usage") if isinstance(p.get("usage"), dict) else {}
        tin = int(usage.get("input_tokens", 0) or 0)
        tout = int(usage.get("output_tokens", 0) or 0)
        cached = int(usage.get("cached_tokens", 0) or 0)
        # 与 aite/worker/loop.py 的 _price 同一个式子：元/百万 token
        cost = (tin * self.price_in + tout * self.price_out) / 1_000_000
        keep = {
            "model": p.get("model"),
            "step": p.get("step"),
            "finish_reason": p.get("finish_reason"),
            "input_tokens": tin,
            "output_tokens": tout,
            "cached_tokens": cached,
            "total_tokens": tin + tout,
            "cost": round(cost, 6),
        }
        cached_bit = f" cached={cached}" if cached else ""
        return (
            f"{p.get('model', '?')} step={p.get('step', '?')} finish={p.get('finish_reason', '?')} "
            f"token in={tin} out={tout} 合计={tin + tout}{cached_bit} {_fmt_money(cost)}"
        ), keep

    # -- 工具 -------------------------------------------------------------

    def _k_tool_call(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        name = p.get("name", "?")
        summary = _args_summary(p.get("arguments"))
        keep = {"name": name, "call_id": p.get("call_id"), "arguments_summary": summary}
        return f"{name}({summary})", keep

    def _k_tool_result(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        ok = bool(p.get("ok"))
        code = p.get("error")
        ms = p.get("duration_ms", 0)
        keep = {
            "name": p.get("name"),
            "call_id": p.get("call_id"),
            "ok": ok,
            "error": code,
            "duration_ms": ms,
        }
        status = "ok" if ok else f"FAIL[{code or 'unknown'}]"
        return f"{p.get('name', '?')} → {status} {ms}ms", keep

    # -- 进度面 -----------------------------------------------------------

    def _k_checklist_op(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        op = str(p.get("op", "?"))
        keep: dict[str, Any] = {"op": op}
        if op == "add":
            items = p.get("items") if isinstance(p.get("items"), list) else []
            for idx, text in enumerate(items, start=1):
                self.checklist[f"c{idx}"] = str(text)
            ids = p.get("ids") if isinstance(p.get("ids"), list) else []
            keep |= {"ids": list(ids), "items": [str(t) for t in items]}
            added = "，".join(f"{i}{self.item_text(i)}" for i in ids) or "（无）"
            return f"add {added}", keep
        if op in ("check", "fail"):
            item_id = p.get("id")
            keep |= {"id": item_id, "state": p.get("state")}
            return f"{op} {item_id} → {p.get('state', '?')} {self.item_text(item_id)}".rstrip(), keep
        if op == "note":
            keep |= {"text": p.get("text")}
            return f"note 「{_clip(p.get('text', ''), 40)}」", keep
        return f"{op} {_clip(json.dumps(p, ensure_ascii=False, sort_keys=True), 70)}", keep

    # -- 产物 -------------------------------------------------------------

    def _k_artifact(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        sha = str(p.get("sha256", ""))
        size = p.get("size", 0)
        keep = {"title": p.get("title"), "mime": p.get("mime"), "size": size, "sha256": sha}
        return f"「{_clip(p.get('title', ''), 40)}」 {p.get('mime', '?')} {size}B sha={sha[:8]}", keep

    # -- 终态 -------------------------------------------------------------

    def _k_delivered(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        missing = p.get("missing") if isinstance(p.get("missing"), list) else []
        keep = {"artifacts": p.get("artifacts", 0), "missing": list(missing), "steps": p.get("steps")}
        tail = f"，产物缺失 {len(missing)} 个" if missing else ""
        return f"已交付 · 产出 {p.get('artifacts', 0)} 个 · {p.get('steps', '?')} 步{tail}", keep

    def _k_failed(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        keep = {"reason": p.get("reason"), "steps": p.get("steps")}
        return f"失败 · {_clip(p.get('reason', ''), 72)} · {p.get('steps', '?')} 步", keep

    def _k_cancelled(self, p: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        keep = {"by": p.get("by"), "steps": p.get("steps")}
        by = f" by={p['by']}" if p.get("by") else ""
        return f"已取消{by} · {p.get('steps', '?')} 步", keep


# ---------------------------------------------------------------- 读 + 校验


def _load_manifest(task_dir: Path) -> tuple[dict[str, Any] | None, list[str]]:
    path = task_dir / MANIFEST_FILE
    if not path.exists():
        return None, []
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except ValueError as exc:
        return None, [f"manifest.json 解析不了：{exc}"]
    if not isinstance(data, dict):
        return None, [f"manifest.json 顶层不是对象，是 {type(data).__name__}"]
    return data, []


def _resolve_payload(task_dir: Path, ev: EvidenceEvent) -> tuple[dict[str, Any] | None, str]:
    """返回 (payload, 说明)。payload 为 None 时说明里写清为什么拿不到。"""
    if ev.payload is not None:
        return ev.payload, ""
    if ev.payload_ref is None:
        return None, "payload 既没内联也没 payload_ref"
    ref = task_dir / ev.payload_ref
    if not ref.exists():
        return None, f"payload 缺失（{ev.payload_ref} 不在）"
    try:
        loaded = json.loads(ref.read_text(encoding="utf-8"))
    except ValueError as exc:
        return None, f"外置 payload 解析不了（{ev.payload_ref}）：{exc}"
    if not isinstance(loaded, dict):
        return None, f"外置 payload 顶层不是对象（{ev.payload_ref}）"
    return loaded, f"外置 payload {ev.payload_ref}"


def load_timeline(task_dir: Path, *, price_in: float = 0.0, price_out: float = 0.0) -> Timeline:
    """读一个任务目录，逐条重算 hash 链，产出可渲染的时间线。

    校验口径和 FileEvidenceWriter.verify 一致（seq 连续、payload 与 payload_hash 对得上、
    prev_hash 接得住、hash == chain_hash(prev, payload_hash)），差别是这里不提前 return：
    每条都查完、把问题全收起来，人才知道是断了一处还是整份都不对。
    往下走时 prev 用**落盘那条**的 hash，这样篡改一条只报一处，不会级联出一串假问题。
    """
    events_path = task_dir / EVENTS_FILE
    manifest, manifest_problems = _load_manifest(task_dir)
    tl = Timeline(
        task_id=task_dir.name,
        task_dir=task_dir,
        manifest=manifest,
        manifest_exists=(task_dir / MANIFEST_FILE).exists(),
    )
    tl.manifest_problems = list(manifest_problems)

    detailer = _Detailer(price_in, price_out)
    raw = events_path.read_text(encoding="utf-8") if events_path.exists() else ""
    prev = GENESIS
    first_at: datetime | None = None
    first_task_id: str | None = None
    index = 0  # 期望的 seq，跳过空行后连续

    for line_no, line in enumerate(raw.splitlines(), start=1):
        if not line.strip():
            continue
        try:
            ev = EvidenceEvent.model_validate_json(line)
        except ValueError as exc:
            tl.issues.append(
                ChainIssue(line=line_no, seq=None, problem=f"这一行不是合法的 EvidenceEvent：{exc}")
            )
            index += 1
            continue

        tl.checked += 1
        broken = False

        # task_id 以第一条为准（--dir 指到一个改过名的目录时，目录名不作数）；
        # 同一个文件里混进别的任务，那是串目录了，得报出来
        if first_task_id is None:
            first_task_id = ev.task_id
            tl.task_id = ev.task_id
        elif ev.task_id != first_task_id:
            tl.issues.append(
                ChainIssue(
                    line=line_no,
                    seq=ev.seq,
                    problem="这一条的 task_id 和文件里其他事件不是同一个任务",
                    expected=first_task_id,
                    actual=ev.task_id,
                )
            )
            broken = True

        if ev.seq != index:
            tl.issues.append(
                ChainIssue(
                    line=line_no,
                    seq=ev.seq,
                    problem="seq 不连续",
                    expected=str(index),
                    actual=str(ev.seq),
                )
            )
            broken = True

        payload, note = _resolve_payload(task_dir, ev)
        if payload is None:
            tl.issues.append(ChainIssue(line=line_no, seq=ev.seq, problem=note or "payload 拿不到"))
            broken = True
        else:
            actual_hash = payload_hash_of(payload)
            if actual_hash != ev.payload_hash:
                tl.issues.append(
                    ChainIssue(
                        line=line_no,
                        seq=ev.seq,
                        problem="payload 与 payload_hash 对不上（内容被改过）",
                        expected=ev.payload_hash,
                        actual=actual_hash,
                    )
                )
                broken = True

        if ev.prev_hash != prev:
            tl.issues.append(
                ChainIssue(
                    line=line_no,
                    seq=ev.seq,
                    problem="prev_hash 接不住上一条",
                    expected=prev,
                    actual=ev.prev_hash,
                )
            )
            broken = True

        want_hash = chain_hash(ev.prev_hash, ev.payload_hash)
        if ev.hash != want_hash:
            tl.issues.append(
                ChainIssue(
                    line=line_no,
                    seq=ev.seq,
                    problem="hash != chain_hash(prev_hash, payload_hash)",
                    expected=want_hash,
                    actual=ev.hash,
                )
            )
            broken = True

        if first_at is None:
            first_at = ev.created_at
        elapsed = (ev.created_at - first_at).total_seconds() if first_at else 0.0

        detail, fields = detailer.render(str(ev.kind), payload or {})
        if payload is None:
            detail = note or "payload 拿不到，无法渲染"
        tl.rows.append(
            Row(
                seq=ev.seq,
                line=line_no,
                kind=str(ev.kind),
                at=ev.created_at,
                elapsed=elapsed,
                detail=detail,
                fields=fields,
                broken=broken,
                note=note if payload is not None and note else "",
            )
        )
        _accumulate(tl.stats, str(ev.kind), fields, readable=payload is not None)
        prev = ev.hash
        index += 1

    tl.root_hash = prev
    tl.stats.events = len(tl.rows)
    tl.stats.span_sec = tl.rows[-1].elapsed if tl.rows else 0.0
    if tl.rows and tl.rows[-1].kind in {k.value for k in TERMINAL_KINDS}:
        tl.stats.terminal = tl.rows[-1].kind

    if manifest is not None:
        tl.manifest_problems += _check_manifest(manifest, tl)
    return tl


def _accumulate(stats: Stats, kind: str, fields: dict[str, Any], *, readable: bool = True) -> None:
    if not readable:
        # payload 读不到就什么都别算。不然一条丢了 payload 的 tool_result 会被当成
        # 「工具失败一次」—— 真机排障时这种假失败最误事。
        stats.unreadable += 1
        return
    if kind == EvidenceKind.model_call.value:
        stats.model_calls += 1
        stats.tokens_in += int(fields.get("input_tokens", 0) or 0)
        stats.tokens_out += int(fields.get("output_tokens", 0) or 0)
        stats.cost += float(fields.get("cost", 0.0) or 0.0)
    elif kind == EvidenceKind.tool_call.value:
        stats.tool_calls += 1
    elif kind == EvidenceKind.tool_result.value:
        if not fields.get("ok", False):
            stats.tool_failed += 1
    elif kind == EvidenceKind.artifact.value:
        stats.artifacts += 1


def _check_manifest(manifest: dict[str, Any], tl: Timeline) -> list[str]:
    problems: list[str] = []
    root = manifest.get("root_hash")
    if root != tl.root_hash:
        problems.append(
            f"manifest.root_hash 与最后一条事件的 hash 对不上："
            f"manifest={root} 实际={tl.root_hash}"
        )
    count = manifest.get("event_count")
    if isinstance(count, int) and count != len(tl.rows):
        problems.append(f"manifest.event_count={count}，events.jsonl 实际 {len(tl.rows)} 条")
    mid = manifest.get("task_id")
    if mid and tl.rows and mid != tl.task_id:
        problems.append(f"manifest.task_id={mid}，事件里的 task_id={tl.task_id}")
    return problems


# ---------------------------------------------------------------- 渲染


def _filter_rows(rows: list[Row], only: set[str] | None, tail: int | None) -> list[Row]:
    out = [r for r in rows if only is None or r.kind in only]
    if tail is not None and tail >= 0:
        out = out[-tail:] if tail else []
    return out


def render_text(tl: Timeline, rows: list[Row], *, filtered: bool) -> str:
    lines: list[str] = []
    lines.append(f"任务 {tl.task_id}")
    lines.append(f"目录 {tl.task_dir}")
    if tl.manifest is not None:
        m = tl.manifest
        lines.append(
            "manifest  "
            + " ".join(
                f"{k}={m.get(k)}"
                for k in ("session_id", "task_no", "created_by", "model", "contract_version")
                if m.get(k) not in (None, "")
            )
        )
    elif tl.manifest_exists:
        lines.append("manifest  有 manifest.json 但读不了（见下面的链校验），任务信息取不到")
    else:
        lines.append("manifest  未 finalize（没有 manifest.json —— 任务可能还在跑，或进程被杀了）")
    lines.append("")

    if not tl.rows:
        lines.append("（events.jsonl 里没有事件）")
    else:
        if filtered:
            lines.append(f"（已过滤：显示 {len(rows)}/{len(tl.rows)} 条；汇总与链校验仍按全量算）")
        lines.append("     seq       耗时  kind             详情")
        lines.append("  " + "─" * 106)
        for r in rows:
            mark = MARK_BROKEN if r.broken else (MARK_TERMINAL if r.kind in
                                                 {k.value for k in TERMINAL_KINDS} else " ")
            lines.append(f"{mark} {r.seq:5d}  {_fmt_elapsed(r.elapsed)}  {r.kind:<16} {r.detail}")
            if r.note:
                lines.append(f"        └─ {r.note}")

    lines.append("")
    lines.append("── 汇总 " + "─" * 98)
    s = tl.stats
    span = _fmt_elapsed(s.span_sec).lstrip("+").strip()
    tail = f"，终态 {s.terminal}" if s.terminal else "，无终态事件（任务没走到 delivered/failed/cancelled）"
    lines.append(f"事件      {s.events} 条，跨度 {span}{tail}")
    lines.append(
        f"模型调用  {s.model_calls} 次 · token in={s.tokens_in:,} out={s.tokens_out:,} "
        f"合计={s.tokens_in + s.tokens_out:,} · 花费 {_fmt_money(s.cost)}"
    )
    lines.append(f"工具调用  {s.tool_calls} 次（失败 {s.tool_failed} 次）")
    lines.append(f"产出文件  {s.artifacts} 个")
    if s.unreadable:
        lines.append(f"读不到    {s.unreadable} 条事件的 payload 拿不到，上面几行的数字不含它们")
    lines += _render_chain(tl)
    return "\n".join(lines)


def _render_chain(tl: Timeline) -> list[str]:
    lines: list[str] = []
    if tl.ok:
        tail = (
            "，root_hash 与 manifest 一致"
            if tl.manifest is not None
            else "（未 finalize，无 root_hash 可比）"
        )
        lines.append(f"hash 链   OK · {tl.checked} 条全部闭合{tail}")
        lines.append(f"          root {tl.root_hash}")
        return lines

    lines.append(f"hash 链   断了 {MARK_BROKEN} · 查了 {tl.checked} 条，发现 {len(tl.issues)} 处问题")
    for issue in tl.issues:
        where = f"第 {issue.line} 行" + (f"（seq={issue.seq}）" if issue.seq is not None else "")
        lines.append(f"          {where}：{issue.problem}")
        if issue.expected or issue.actual:
            lines.append(f"            期望 {issue.expected}")
            lines.append(f"            实际 {issue.actual}")
    for problem in tl.manifest_problems:
        lines.append(f"          manifest：{problem}")
    lines.append("          → 这份证据不可信，别拿它当验收依据；先确认目录有没有被人手改过。")
    return lines


# ---------------------------------------------------------------- --list


def list_tasks(root: Path, *, price_in: float = 0.0, price_out: float = 0.0) -> list[dict[str, Any]]:
    if not root.is_dir():
        return []
    found = []
    for d in root.iterdir():
        events = d / EVENTS_FILE
        if not d.is_dir() or not events.exists():
            continue
        tl = load_timeline(d, price_in=price_in, price_out=price_out)
        found.append(
            {
                "task_id": tl.task_id,
                "dir": str(d),
                "mtime": events.stat().st_mtime,
                "events": tl.stats.events,
                "terminal": tl.stats.terminal,
                "finalized": tl.finalized,
                "chain_ok": tl.ok,
            }
        )
    found.sort(key=lambda x: x["mtime"], reverse=True)
    return found


def render_list(root: Path, rows: list[dict[str, Any]]) -> str:
    lines = [f"证据根目录 {root}（{len(rows)} 个任务）", ""]
    if not root.is_dir():
        lines.append("（这个目录不存在 —— 对一下 config 里的 storage.evidence_dir，")
        lines.append("  以及起飞日志里打出来的那个 evidence 目录）")
        return "\n".join(lines)
    if not rows:
        lines.append("（目录在，但里面没有任何带 events.jsonl 的任务子目录）")
        return "\n".join(lines)
    lines.append(
        "  最后写入             " + _pad("任务", 32) + "  事件  " + _pad("终态", 12)
        + " " + _pad("manifest", 12) + " 链"
    )
    lines.append("  " + "─" * 92)
    for r in rows:
        when = datetime.fromtimestamp(r["mtime"]).strftime("%Y-%m-%d %H:%M:%S")
        state = r["terminal"] or "无终态"
        final = "已写" if r["finalized"] else "未 finalize"
        chain = "OK" if r["chain_ok"] else f"断了 {MARK_BROKEN}"
        lines.append(
            f"  {when}  {_pad(r['task_id'], 32)}  {r['events']:>4}  "
            f"{_pad(state, 12)} {_pad(final, 12)} {chain}"
        )
    broken = [r for r in rows if not r["chain_ok"]]
    if broken:
        lines.append("")
        lines.append(
            f"{MARK_BROKEN} 有 {len(broken)} 个任务的证据链不可信："
            + "、".join(r["task_id"] for r in broken)
        )
        lines.append("  逐个跑 scripts/evidence_show.py <task_id> 看断在哪一条。")
    return "\n".join(lines)


# ---------------------------------------------------------------- CLI


def _load_config(path: str) -> tuple[AiteConfig, str]:
    """读 config 只为拿 evidence_dir 和单价。读不到就用契约默认值，并说明一句。"""
    p = Path(path)
    if not p.exists():
        return AiteConfig(), f"{p} 不在"
    try:
        import yaml

        data = yaml.safe_load(p.read_text(encoding="utf-8")) or {}
        if not isinstance(data, dict):
            raise ValueError("顶层不是 mapping")
        return AiteConfig.model_validate(data), ""
    except Exception as exc:  # 配置坏了不该让排障工具也跟着哑
        return AiteConfig(), f"{p} 读不了：{exc}"


def _config_note(cfg_note: str, price_in: float, price_out: float) -> str:
    """只在配置真的影响输出时才唠叨一句 —— 单价没配，花费那一栏就是假的 0。"""
    if not price_in and not price_out:
        why = f"{cfg_note}；" if cfg_note else "config 里 price_in/out_per_mtok 都是 0；"
        return f"提示：{why}花费一栏按 0 元算，不代表真没花钱（用 --config 指到真配置）"
    return f"提示：{cfg_note}，evidence_dir 与单价用契约默认值" if cfg_note else ""


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="evidence_show.py",
        description="把任务的证据链渲染成人读的时间线，并校验 hash 链。",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "例子：\n"
            "  scripts/evidence_show.py --list\n"
            "  scripts/evidence_show.py tsk_0a1b2c\n"
            "  scripts/evidence_show.py --dir data/evidence/tsk_0a1b2c --only model_call,tool_call\n"
            "  scripts/evidence_show.py tsk_0a1b2c --tail 20 --json\n"
        ),
    )
    p.add_argument("task_id", nargs="?", help="任务 id（在 --root 下找同名目录）")
    p.add_argument("--dir", help="直接指向某个任务目录（里面有 events.jsonl）")
    p.add_argument("--root", help="证据根目录；默认取 config 里的 storage.evidence_dir")
    p.add_argument("--config", default="config/aite.yaml", help="配置文件，用来取 evidence_dir 与单价")
    p.add_argument("--list", action="store_true", help="列出根目录下所有任务（最近写入在前）")
    p.add_argument("--only", help="只显示这些 kind，逗号分隔（如 model_call,tool_call）")
    p.add_argument("--tail", type=int, help="只显示最后 N 条")
    p.add_argument("--json", action="store_true", help="输出机器可读的 JSON")
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    cfg, cfg_note = _load_config(args.config)
    price_in, price_out = cfg.model.price_in_per_mtok, cfg.model.price_out_per_mtok

    if args.list:
        if args.dir:
            print("--list 要的是证据根目录，用 --root；--dir 是单个任务目录。", file=sys.stderr)
            return 2
        root = Path(args.root) if args.root else Path(cfg.storage.evidence_dir)
        rows = list_tasks(root, price_in=price_in, price_out=price_out)
        if args.json:
            print(json.dumps({"root": str(root), "tasks": rows}, ensure_ascii=False, indent=2))
        else:
            if cfg_note and not args.root:
                print(f"提示：{cfg_note}，证据根目录用契约默认值")
            print(render_list(root, rows))
        # 有任何一份证据链断了就非零退出 —— --list 常被拿来一眼扫全场
        return 0 if all(r["chain_ok"] for r in rows) else 1

    if args.dir:
        task_dir = Path(args.dir)
    elif args.task_id:
        root = Path(args.root) if args.root else Path(cfg.storage.evidence_dir)
        task_dir = root / args.task_id
    else:
        print("要给一个 task_id，或用 --dir 指向任务目录，或用 --list 看有哪些任务。", file=sys.stderr)
        return 2

    if not (task_dir / EVENTS_FILE).exists():
        print(f"找不到证据：{task_dir / EVENTS_FILE} 不在。", file=sys.stderr)
        if not task_dir.exists():
            print(f"（目录 {task_dir} 本身就不存在；--list 看看根目录下有哪些任务）", file=sys.stderr)
        return 2

    if args.tail is not None and args.tail < 0:
        print("--tail 要一个 >= 0 的数。", file=sys.stderr)
        return 2

    only: set[str] | None = None
    if args.only:
        valid = {k.value for k in EvidenceKind}
        only = {s.strip() for s in args.only.split(",") if s.strip()}
        bad = sorted(only - valid)
        if bad:
            print(f"--only 里有不认识的 kind：{', '.join(bad)}", file=sys.stderr)
            print(f"可用：{', '.join(sorted(valid))}", file=sys.stderr)
            return 2

    tl = load_timeline(task_dir, price_in=price_in, price_out=price_out)
    rows = _filter_rows(tl.rows, only, args.tail)

    if args.json:
        print(json.dumps(tl.as_dict(rows), ensure_ascii=False, indent=2))
    else:
        note = _config_note(cfg_note, price_in, price_out)
        if note:
            print(note)
        print(render_text(tl, rows, filtered=len(rows) != len(tl.rows)))
    return 0 if tl.ok else 1


if __name__ == "__main__":
    sys.exit(main())
