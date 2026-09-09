"""评测 runner 的命令行入口（owner: T4，dev-spec §3.8 / §6.T4）。

    python -m aite.evals run evals/p0 --platform fake --model scripted
    python -m aite.evals run evals/p0 --list

`run` 跑完打印 JSON 摘要，最后一行是 `passed k/10`。退出码：全过 0，否则 1。
并行期间别的轨还没合进来，k 会是 0 —— 那时每个场景报的是「接不上 ControlPlane：…」
这类人话原因，不是异常栈（§6 T4）。TΩ 阶段才要求 `passed 10/10` 且退出码 0（B8）。
"""
from __future__ import annotations

import argparse
import asyncio
import json
import sys
import traceback
from pathlib import Path

from .runner import run_suite
from .scenario import ScenarioError, load_suite

PLATFORMS = ("fake",)
MODELS = ("scripted", "live")


def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="python -m aite.evals", description="Aite P0 评测 runner")
    sub = ap.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run", help="跑一个评测目录下的场景")
    run.add_argument("suite", help="场景目录，如 evals/p0")
    run.add_argument("--platform", default="fake", choices=PLATFORMS, help="平台替身（默认 fake）")
    run.add_argument("--model", default="scripted", choices=MODELS, help="模型来源（默认 scripted）")
    run.add_argument("--list", action="store_true", help="只列出场景名，不执行")
    run.add_argument("--only", action="append", default=[], metavar="NAME", help="只跑指定场景，可多次给")
    run.add_argument("--json", dest="json_out", metavar="PATH", help="把 JSON 摘要另存一份到文件")
    run.add_argument("--traceback", action="store_true", help="失败时把异常栈打到 stderr（默认不打）")
    run.add_argument("--config", default="config/aite.yaml", help="--model live 时读哪份配置")
    return ap


def _live_model_factory(config_path: str):
    """--model live：拿真模型跑一遍，用于 §14.2 的模型实测。不属于 P0 验收路径。"""
    from ..config import load_config
    from ..models import OpenAICompatModel

    cfg = load_config(config_path)
    return lambda: OpenAICompatModel(cfg.model)


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.cmd != "run":
        return 2

    try:
        scenarios = load_suite(args.suite)
    except ScenarioError as exc:
        print(f"场景加载失败：{exc}", file=sys.stderr)
        return 2

    if args.only:
        wanted = set(args.only)
        unknown = wanted - {s.name for s in scenarios}
        if unknown:
            print(f"没有这些场景：{sorted(unknown)}", file=sys.stderr)
            return 2
        scenarios = [s for s in scenarios if s.name in wanted]

    if args.list:
        print(json.dumps([s.name for s in scenarios], ensure_ascii=False, indent=2))
        return 0

    model_factory = None
    if args.model == "live":
        try:
            model_factory = _live_model_factory(args.config)
        except Exception as exc:
            print(f"--model live 起不来：{type(exc).__name__}: {exc}", file=sys.stderr)
            return 2

    result = asyncio.run(
        run_suite(
            scenarios,
            suite=args.suite,
            platform=args.platform,
            model_name=args.model,
            model_factory=model_factory,
            want_traceback=args.traceback,
        )
    )

    payload = result.to_json()
    print(json.dumps(payload, ensure_ascii=False, indent=2))
    if args.json_out:
        Path(args.json_out).write_text(
            json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
    if args.traceback:
        for r in result.results:
            if r.traceback:
                print(f"\n=== {r.name} ===\n{r.traceback}", file=sys.stderr)

    print(f"passed {result.passed}/{result.total}")
    return 0 if result.passed == result.total else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as exc:                       # runner 自己炸了也不吐栈到 stdout
        print(f"runner 异常：{type(exc).__name__}: {exc}", file=sys.stderr)
        traceback.print_exc(file=sys.stderr)
        sys.exit(2)
