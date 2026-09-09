"""契约锁：记录并校验 aite/contracts/ 下每个契约文件的 sha256。

    python -m aite.contracts.lock --write    # 生成 / 刷新仓库根的 .contracts.lock
    python -m aite.contracts.lock --check    # 一致 -> "OK <n> files" 退出 0；否则打印差异退出 1

本文件自身与 __pycache__ 不参与锁定：lock.py 是工具不是契约，改它不构成契约变更。
"""
import argparse
import hashlib
import sys
from pathlib import Path

CONTRACTS_DIR = Path(__file__).resolve().parent
REPO_ROOT = CONTRACTS_DIR.parents[1]
LOCK_PATH = REPO_ROOT / ".contracts.lock"
SELF_NAME = Path(__file__).resolve().name

HEADER = "# aite contracts lock — sha256 of aite/contracts/*.py（lock.py 除外）"


def contract_files() -> list[Path]:
    """参与锁定的契约文件，按仓库相对路径排序。

    收**所有**文件而不只是 *.py：派单原文是「每个 contracts 文件的 sha256」。
    只锁 .py 的话，往 aite/contracts/ 丢一个 .pyi 覆盖类型、或塞个 .json 进去，
    --check 照样报 OK，保护面就有洞。排除项只有 lock.py 自身与 __pycache__。
    """
    out = []
    for p in CONTRACTS_DIR.rglob("*"):
        if not p.is_file():
            continue
        if p.name == SELF_NAME and p.parent == CONTRACTS_DIR:
            continue
        if "__pycache__" in p.parts:
            continue
        out.append(p)
    return sorted(out, key=lambda p: p.relative_to(REPO_ROOT).as_posix())


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def current() -> dict[str, str]:
    return {p.relative_to(REPO_ROOT).as_posix(): digest(p) for p in contract_files()}


def render(entries: dict[str, str]) -> str:
    body = "".join(f"{h}  {rel}\n" for rel, h in sorted(entries.items()))
    return HEADER + "\n" + body


def parse(text: str) -> dict[str, str]:
    entries = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        h, _, rel = line.partition("  ")
        if not rel:
            raise ValueError(f"无法解析的锁行：{line!r}")
        entries[rel.strip()] = h.strip()
    return entries


MIN_CONTRACT_FILES = 10   # §3.1/§3.2 的十个模块，加 __init__.py 共 11；少于这个数一定是出事了


def _guard_not_empty(entries: dict[str, str]) -> None:
    """空表守卫：没有这条，contract_files() 返空时 --check 会打印 `OK 0 files` 退出 0。

    「锁校验绿」是 §2.3 C2 给所有 track 的回归闸门，绿得毫无内容比红了更危险。"""
    if len(entries) < MIN_CONTRACT_FILES:
        raise SystemExit(
            f"契约文件只找到 {len(entries)} 个（期望 >= {MIN_CONTRACT_FILES}）："
            f"{sorted(entries)}\n{CONTRACTS_DIR} 是不是被删了或路径解析错了？"
        )


def cmd_write() -> int:
    entries = current()
    _guard_not_empty(entries)
    LOCK_PATH.write_text(render(entries), encoding="utf-8")
    print(f"wrote {len(entries)} files -> {LOCK_PATH.relative_to(REPO_ROOT).as_posix()}")
    return 0


def cmd_check() -> int:
    entries = current()
    _guard_not_empty(entries)
    if not LOCK_PATH.exists():
        print(f"MISSING {LOCK_PATH.relative_to(REPO_ROOT).as_posix()} 不存在，先跑 --write", file=sys.stderr)
        return 1
    locked = parse(LOCK_PATH.read_text(encoding="utf-8"))
    diffs = []
    for rel in sorted(set(locked) | set(entries)):
        if rel not in entries:
            diffs.append(f"  deleted  {rel}（在锁里但文件不存在）")
        elif rel not in locked:
            diffs.append(f"  added    {rel}（新文件未入锁）")
        elif locked[rel] != entries[rel]:
            diffs.append(f"  changed  {rel}\n    locked {locked[rel]}\n    actual {entries[rel]}")
    if diffs:
        print(f"MISMATCH {len(diffs)} file(s):", file=sys.stderr)
        for d in diffs:
            print(d, file=sys.stderr)
        return 1
    print(f"OK {len(entries)} files")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m aite.contracts.lock", description=__doc__)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--write", action="store_true", help="生成 / 刷新 .contracts.lock")
    g.add_argument("--check", action="store_true", help="校验 .contracts.lock 与实际文件一致")
    args = ap.parse_args(argv)
    return cmd_write() if args.write else cmd_check()


if __name__ == "__main__":
    sys.exit(main())
