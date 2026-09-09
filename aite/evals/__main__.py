"""评测 runner 的 stub（真实实现归 T4，dev-spec §3.8 / §6.T4）。

    python -m aite.evals run evals/p0 --platform fake --model scripted   # not implemented，退出码 2
    python -m aite.evals run evals/p0 --list                             # 场景名列表（T0 阶段为空）
"""
import argparse
import json
import sys


def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="python -m aite.evals", description="Aite P0 评测 runner")
    sub = ap.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run", help="跑一个评测目录下的场景")
    run.add_argument("suite", help="场景目录，如 evals/p0")
    run.add_argument("--platform", default="fake", help="平台替身（默认 fake）")
    run.add_argument("--model", default="scripted", help="模型替身（默认 scripted）")
    run.add_argument("--list", action="store_true", help="只列出场景名，不执行")
    return ap


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.cmd == "run":
        if args.list:
            print(json.dumps([], ensure_ascii=False))
            return 0
        print("not implemented")
        return 2
    return 2


if __name__ == "__main__":
    sys.exit(main())
