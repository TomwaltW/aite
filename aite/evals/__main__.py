"""评测 runner 的命令行入口（owner: T4，dev-spec §3.8 / §6.T4）。

    python -m aite.evals run evals/p0 --platform fake --model scripted
    python -m aite.evals run evals/p0 --list

`run` 跑完打印 JSON 摘要，最后一行是 `passed k/10`。退出码：全过 0，否则 1。
并行期间别的轨还没合进来，k 会是 0 —— 那时每个场景报的是「接不上 ControlPlane：…」
这类人话原因，不是异常栈（§6 T4）。TΩ 阶段才要求 `passed 10/10` 且退出码 0（B8）。

`--protocol-report` 是 §14.2 模型实测用的眼睛（T12）：

    python -m aite.evals run evals/p0 --model live --protocol-report live-protocol.json

它按 `aite/contracts/protocol.py` 逐步核对模型的出牌 —— 调了什么、参数合不合
`ToolSpec.parameters`、有没有协议外的名字、几步到 final、§3.3 的兜底触没触发。
**live 模式下 `expect` 判据必然大面积红，那是预期的**（判据是照 scripted 的台词写的，
§3.8），要看的是这份报告。摘要写 stderr、完整 JSON 写文件，stdout 那份 JSON 摘要
默认一个字段都不多 —— 不开这个开关，输出跟以前逐字节一样。

`--timeout-scale` 把场景的两个等待上限一起放大：yaml 里那些秒数是照替身的尺度定的，
真模型撑不下。`--model live` 默认就按 `LIVE_TIMEOUT_SCALE` 放大，不用每次手写。

`--sandbox docker` 把沙箱与 Gateway 这一段换成真的（T23，§2.4 M3）：

    python -m aite.evals run evals/p0 --only 04_csv_to_chart \
        --platform fake --model live --sandbox docker

代码在 `aite-sandbox:p0` 容器里真跑，PNG 是镜像里的 matplotlib 真画出来的。
默认仍是 `fake`，**默认路径逐字节不变**（B8 / CI / T4 的 200 条都吃那条路）。
起飞前先查 daemon 与镜像 —— 缺哪样都是一行人话 + 退出码 2，理由同
`_live_model_factory`。
"""
from __future__ import annotations

import argparse
import asyncio
import json
import sys
import traceback
from pathlib import Path

from .protocol_probe import render_digest
from .real_stack import docker_preflight, scenarios_with_exec_script
from .runner import run_suite
from .scenario import Scenario, ScenarioError, load_suite
from .wiring import SANDBOXES

PLATFORMS = ("fake",)
MODELS = ("scripted", "live")

#: `--model live` 时 `--timeout-scale` 的默认值。
#:
#: 场景 yaml 里的 `timeout_sec: 10` 和 `after_timeout_sec: 5` 是照替身的尺度定的 ——
#: 脚本化模型瞬时返回，十秒够跑几十步。真模型一次调用就 2–20s，10s 连一步都不一定
#: 收得回来，多步场景必然停在「10s 内没有停下来」。这个倍数把两个上限一起放大，
#: **不改任何一份场景文件**（那些数字是 scripted 路径的验收面）。
LIVE_TIMEOUT_SCALE = 12.0


def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="python -m aite.evals", description="Aite P0 评测 runner")
    sub = ap.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run", help="跑一个评测目录下的场景")
    run.add_argument("suite", help="场景目录，如 evals/p0")
    run.add_argument("--platform", default="fake", choices=PLATFORMS, help="平台替身（默认 fake）")
    run.add_argument("--model", default="scripted", choices=MODELS, help="模型来源（默认 scripted）")
    run.add_argument(
        "--sandbox",
        default="fake",
        choices=SANDBOXES,
        help="沙箱与 Gateway 接谁：fake（默认，替身按 exec_script 演）/ docker（真容器真跑）",
    )
    run.add_argument("--list", action="store_true", help="只列出场景名，不执行")
    run.add_argument("--only", action="append", default=[], metavar="NAME", help="只跑指定场景，可多次给")
    run.add_argument("--json", dest="json_out", metavar="PATH", help="把 JSON 摘要另存一份到文件")
    run.add_argument("--traceback", action="store_true", help="失败时把异常栈打到 stderr（默认不打）")
    run.add_argument("--config", default="config/aite.yaml", help="--model live 时读哪份配置")
    run.add_argument(
        "--protocol-report",
        dest="protocol_report",
        nargs="?",
        const="",
        default=None,
        metavar="PATH",
        help="出一份协议出牌报告：摘要打 stderr，给了 PATH 就把完整 JSON 另存过去（§14.2）",
    )
    run.add_argument(
        "--timeout-scale",
        dest="timeout_scale",
        type=float,
        default=None,
        metavar="K",
        help=f"把场景的 timeout_sec / after_timeout_sec 一起乘 K（scripted 默认 1.0，"
        f"live 默认 {LIVE_TIMEOUT_SCALE}）",
    )
    return ap


def _scale_timeouts(sc: Scenario, k: float) -> Scenario:
    """按倍数放大一个场景的两个等待上限。只在内存里改，场景文件一个字都不动。"""
    events = [e.model_copy(update={"after_timeout_sec": e.after_timeout_sec * k}) for e in sc.events]
    return sc.model_copy(update={"timeout_sec": sc.timeout_sec * k, "events": events})


def _sandbox_images(scenarios: list[Scenario]) -> list[str]:
    """这一批场景各自要哪个镜像。`build_deps` 怎么算 config，这里就怎么算。"""
    from ..contracts import AiteConfig

    images = []
    for sc in scenarios:
        cfg = AiteConfig.model_validate({"platform": "fake", **sc.config})
        images.append(cfg.sandbox.image)
    return images


def _announce_ignored_exec_script(scenarios: list[Scenario]) -> None:
    """docker 档下 `sandbox.exec_script` 不生效 —— 起飞时点名，别让它静默失效。"""
    named = scenarios_with_exec_script(scenarios)
    if named:
        print(
            f"--sandbox docker：这些场景的 sandbox.exec_script 不生效"
            f"（代码交给真容器跑）：{'、'.join(named)}",
            file=sys.stderr,
        )


def _live_model_factory(config_path: str):
    """--model live：拿真模型跑一遍，用于 §14.2 的模型实测。不属于 P0 验收路径。

    起飞前先把客户端建出来验一遍配置。`OpenAICompatModel.__init__` 什么都不校验，
    `base_url` / `model` / 密钥环境变量是懒到第一次 `chat` 才查的 —— 不在这里拦，
    配置缺一样就会变成：每个场景各自跑到第一次 chat 才抛 `ModelConfigError`，被
    worker 的 §3.3 当成模型 5xx **白重试 2 次（2s + 5s）**，最后给用户一句
    「模型服务暂不可用」。10 个场景就是 10 次 7 秒空等，而且真正的原因
    （yaml 没填 / 环境变量没设）一个字都看不到。T17 实测撞的就是这一下。

    建客户端不发网络请求，所以这里只判配置、不判端点通不通。
    """
    from ..config import load_config
    from ..models import OpenAICompatModel

    cfg = load_config(config_path)
    OpenAICompatModel(cfg.model)._ensure_client()
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

    if args.sandbox == "docker":
        # 起飞前查一次 daemon 与镜像：不查的话十个场景各自烂在第一个工具调用上，
        # 真正的原因（Docker Desktop 没开 / 镜像没 build）一个字都看不到。
        why = docker_preflight(_sandbox_images(scenarios))
        if why is not None:
            print(f"--sandbox docker 起不来：{why}", file=sys.stderr)
            return 2
        _announce_ignored_exec_script(scenarios)

    scale = args.timeout_scale
    if scale is None:
        scale = LIVE_TIMEOUT_SCALE if args.model == "live" else 1.0
    if scale != 1.0:
        scenarios = [_scale_timeouts(s, scale) for s in scenarios]
        print(f"场景等待上限 x{scale}（--timeout-scale）", file=sys.stderr)

    model_factory = None
    if args.model == "live":
        try:
            model_factory = _live_model_factory(args.config)
        except Exception as exc:
            print(f"--model live 起不来：{type(exc).__name__}: {exc}", file=sys.stderr)
            return 2

    want_protocol = args.protocol_report is not None
    result = asyncio.run(
        run_suite(
            scenarios,
            suite=args.suite,
            platform=args.platform,
            model_name=args.model,
            model_factory=model_factory,
            sandbox_kind=args.sandbox,
            want_traceback=args.traceback,
            collect_protocol=want_protocol,
        )
    )

    payload = result.to_json()
    print(json.dumps(payload, ensure_ascii=False, indent=2))
    if args.json_out:
        Path(args.json_out).write_text(
            json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
    if want_protocol:
        rows = [(r.name, r.protocol) for r in result.results]
        # 摘要走 stderr：stdout 得保持「一份 JSON + 最后一行 passed k/n」，
        # check.sh 的 B8 和 T4 的 CI 都靠这两条读结果
        print(render_digest(rows), file=sys.stderr)
        if args.protocol_report:
            Path(args.protocol_report).write_text(
                json.dumps(
                    {
                        "suite": args.suite,
                        "platform": args.platform,
                        "model": args.model,
                        # stdout 的 JSON 摘要不加这一格（默认路径逐字节不变），
                        # 这份报告是 opt-in 的，加了才看得出实测跑的是哪档沙箱。
                        "sandbox": args.sandbox,
                        "contract_version": payload["contract_version"],
                        "scenarios": [{"name": n, **(p or {})} for n, p in rows],
                    },
                    ensure_ascii=False,
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
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
