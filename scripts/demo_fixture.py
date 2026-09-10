#!/usr/bin/env python3
"""演示素材生成器（owner: T15）—— 给 `docs/demo-3min.md` 的第二幕和加演用。

演示要用一个 CSV。手捏一个每次都不一样的文件，等于每次演示都在赌：数据长什么样、
最高最低落在哪个月、画出来好不好看，全靠临场。这个脚本把这件事钉死：

* **同样参数跑两次，字节一致**（`tests/tools/test_demo_fixture.py` 断言这条）。
  彩排时看到的那张图，和正式演示时那张，是同一张。
* 数据带年度季节性 + 增长趋势 + 一处刻意的塌陷，**最高最低都不含糊**。脚本收尾会把
  这两个月份打出来 —— 演示时当场拿它核对模型说得对不对，核对这一下本身就是在演
  `platform.md` 的铁律 4（不得声称做了没做的事）。
* 只有两列 `month,amount`，和 `evals/p0/04_csv_to_chart.yaml` 里脚本化模型那句
  `df.plot(x="month", y="amount")` 是同一副牌 —— 脚本化彩排和真模型演示看到的是同一份数据。

默认落到 `/tmp/aite-demo/`，**不往 `data/` 里写**：`data/` 是运行时目录，演示前经常要
清空（见 `docs/demo-3min.md` §1 的检查清单），素材放那儿会被一起清掉。

用法：

    scripts/demo_fixture.py all                  # CSV + 群历史垫场文本，一起生成
    scripts/demo_fixture.py csv -o ~/demo.csv    # 只要 CSV，落到指定路径
    scripts/demo_fixture.py history --count 6    # 只要垫场文本，取前 6 条
"""
from __future__ import annotations

import argparse
import csv
import io
import sys
from pathlib import Path
from random import Random

# --------------------------------------------------------------------------
# 落盘位置
# --------------------------------------------------------------------------

#: 默认输出目录。刻意不是 data/：演示前的检查清单会把 data/ 清空。
DEFAULT_DIR = Path("/tmp/aite-demo")
CSV_NAME = "sales.csv"
HISTORY_NAME = "history.txt"

# --------------------------------------------------------------------------
# CSV 的形状与数据模型
# --------------------------------------------------------------------------

#: 列名对着 evals/p0/04_csv_to_chart.yaml 里那句 df.plot(x="month", y="amount")。
#: 改这里等于让脚本化彩排和真模型演示看的不是同一副牌，别改。
COLUMNS = ("month", "amount")

DEFAULT_START = "2024-09"
DEFAULT_MONTHS = 24            # 两年，飞书里预览得下，模型也读得完
DEFAULT_SEED = 20260910

BASE_AMOUNT = 86_000           # 元，第 0 个月的基准盘子
MONTHLY_GROWTH = 0.018         # 每月线性增长，24 个月累计 ×1.41
JITTER = 0.04                  # ±4% 的抖动，让折线不至于太"画出来的"

#: 自然月 → 季节系数。用查表不用正弦：正弦画出来对称得不像真数据，
#: 而双十一那根尖峰和春节那个坑正是这张图好看的原因。
SEASON = {
    1: 0.88,    # 元旦后淡季
    2: 0.72,    # 春节，全年最低的自然低点
    3: 0.98,
    4: 1.00,
    5: 1.06,    # 五一
    6: 1.12,    # 618
    7: 0.95,
    8: 0.97,
    9: 1.03,    # 开学季
    10: 1.08,   # 国庆
    11: 1.28,   # 双十一，全年最高的自然高点
    12: 1.15,   # 年末冲量
}

#: 刻意留一处塌陷（第 10 个月，默认参数下是 2025-07）。
#: 有它，"最低是哪个月"才有唯一答案，模型答对答错一眼看得出来；
#: 没它，最低点会落在春节，和相邻的 1 月/3 月拉不开差距。
ANOMALY_INDEX = 10
ANOMALY_FACTOR = 0.62


def parse_start(text: str) -> tuple[int, int]:
    """'2024-09' → (2024, 9)。"""
    try:
        year_s, month_s = text.split("-")
        year, month = int(year_s), int(month_s)
    except ValueError:
        raise argparse.ArgumentTypeError(f"起始月要写成 YYYY-MM，收到 {text!r}") from None
    if not 1 <= month <= 12:
        raise argparse.ArgumentTypeError(f"月份要在 1–12 之间，收到 {text!r}")
    if not 1970 <= year <= 2999:
        raise argparse.ArgumentTypeError(f"年份不像话，收到 {text!r}")
    return year, month


def month_labels(start: tuple[int, int], months: int) -> list[str]:
    """从 start 起连续 months 个月的 'YYYY-MM' 标签。"""
    year, month = start
    out = []
    for _ in range(months):
        out.append(f"{year:04d}-{month:02d}")
        month += 1
        if month == 13:
            year, month = year + 1, 1
    return out


def build_rows(
    *, start: tuple[int, int], months: int, seed: int
) -> list[tuple[str, int]]:
    """生成 (month, amount) 行。同样参数必然得到同样结果。

    抖动用 `Random(seed).uniform()`（梅森旋转，CPython 跨版本稳定），
    最后一律 `round()` 成整数 —— 不留浮点，就不会有 repr 差异带来的字节漂移。
    """
    rng = Random(seed)
    rows: list[tuple[str, int]] = []
    for index, label in enumerate(month_labels(start, months)):
        calendar_month = int(label.split("-")[1])
        value = BASE_AMOUNT
        value *= 1.0 + MONTHLY_GROWTH * index
        value *= SEASON[calendar_month]
        value *= 1.0 + rng.uniform(-JITTER, JITTER)
        if index == ANOMALY_INDEX:
            value *= ANOMALY_FACTOR
        rows.append((label, round(value)))
    return rows


def render_csv(rows: list[tuple[str, int]]) -> str:
    """行 → CSV 文本。行尾钉成 LF，不跟宿主平台走。"""
    buf = io.StringIO()
    writer = csv.writer(buf, lineterminator="\n")
    writer.writerow(COLUMNS)
    writer.writerows(rows)
    return buf.getvalue()


def summarize(rows: list[tuple[str, int]]) -> list[str]:
    """演示时用来核对模型的那几行 —— 它说的最高最低，得和这里对得上。"""
    top = max(rows, key=lambda r: r[1])
    low = min(rows, key=lambda r: r[1])
    total = sum(v for _, v in rows)
    return [
        f"  区间   {rows[0][0]} … {rows[-1][0]}（{len(rows)} 个月）",
        f"  最高   {top[0]}  {top[1]:,} 元",
        f"  最低   {low[0]}  {low[1]:,} 元",
        f"  合计   {total:,} 元",
    ]


# --------------------------------------------------------------------------
# 群历史垫场文本（加演那一段 / M5 用）
# --------------------------------------------------------------------------

#: (发送人, 消息正文)。前两条之外故意混了「已经办完的」和「纯闲聊」，
#: 汇总时它俩不该出现在开放事项里 —— 这是加演那段真正想让人看见的东西。
HISTORY_POOL: list[tuple[str, str]] = [
    ("Alice（产品）", "对账脚本这周能上线吗？月底前要给财务一个说法"),
    ("Bob（设计）", "我这边等设计稿，周四之前给你们"),
    ("Carol（法务）", "客户合同还差法务盖章，我今天催一下"),
    ("Dave（运维）", "上周那台机器的告警已经处理完了，可以关掉了"),
    ("Alice（产品）", "618 复盘的数据谁来出？下周一评审要用"),
    ("Erin（财务）", "报销单我提交了，还等审批"),
    ("Bob（设计）", "中午一起吃饭吗"),
    ("Frank（销售）", "华东那个客户要一版报价，最晚周五给"),
]

HISTORY_HEADER = """\
# Aite 演示 · 群历史垫场文本
#
# 用途：演示的加演那一段（「汇总本群本周开放事项」）要有真实群消息才有东西可汇总。
#      空群里问这句，模型只能编 —— 那正是演示最不该出现的画面。
#
# 用法：演示开始前 5 分钟，把每条**缩进的那一行**原样发进测试群，一行一条。
#      有替身账号 → 按「发送人」分开发，汇总里能出现不同名字，效果更好；
#      只有一个账号 → 全用自己发也成立（拉历史只过滤非真人消息，不看是谁）。
#
# 三条注意：
#   1. 这些消息**一条都不要 @Aite** —— @ 一下就是一个任务，群里会多出一堆卡片。
#   2. 发在群里，不要发在话题（线程）里 —— 拉的是群历史。
#   3. 第 4 条和第 7 条是干扰项（一条已办完、一条纯闲聊）。汇总里**不该**出现它俩，
#      这两条不出现，才说明它是在筛选而不是在复述。
"""


def render_history(count: int) -> str:
    parts = [HISTORY_HEADER]
    for i, (who, text) in enumerate(HISTORY_POOL[:count], start=1):
        parts.append(f"{i:2d}. {who}\n    {text}\n")
    return "\n".join(parts).rstrip("\n") + "\n"


# --------------------------------------------------------------------------
# 落盘
# --------------------------------------------------------------------------


def write_text(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")
    return path


# --------------------------------------------------------------------------
# 命令行
# --------------------------------------------------------------------------


def _add_csv_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--start", default=DEFAULT_START, type=parse_start, metavar="YYYY-MM",
                        help=f"起始月（默认 {DEFAULT_START}）。默认值是写死的常量，"
                             "不跟今天走 —— 跟了就不可复现了")
    parser.add_argument("--months", default=DEFAULT_MONTHS, type=int, metavar="N",
                        help=f"生成几个月（默认 {DEFAULT_MONTHS}，允许 1–120）")
    parser.add_argument("--seed", default=DEFAULT_SEED, type=int, metavar="N",
                        help=f"抖动的随机种子（默认 {DEFAULT_SEED}）。种子一样，字节就一样")


def _build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(
        prog="scripts/demo_fixture.py",
        description="生成 3 分钟演示要用的素材：一份可复现的月度 CSV，和一份群历史垫场文本。",
        epilog="剧本见 docs/demo-3min.md。素材默认落到 /tmp/aite-demo/，不写 data/。",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = ap.add_subparsers(dest="cmd", required=True, metavar="{csv,history,all}")

    csv_cmd = sub.add_parser("csv", help="只生成月度 CSV（第二幕的附件）")
    csv_cmd.add_argument("-o", "--out", type=Path, default=DEFAULT_DIR / CSV_NAME,
                         metavar="PATH", help=f"落到哪（默认 {DEFAULT_DIR / CSV_NAME}）")
    _add_csv_options(csv_cmd)

    his_cmd = sub.add_parser("history", help="只生成群历史垫场文本（加演那一段）")
    his_cmd.add_argument("-o", "--out", type=Path, default=DEFAULT_DIR / HISTORY_NAME,
                         metavar="PATH", help=f"落到哪（默认 {DEFAULT_DIR / HISTORY_NAME}）")
    his_cmd.add_argument("--count", type=int, default=len(HISTORY_POOL), metavar="N",
                         help=f"取前几条（默认 {len(HISTORY_POOL)}，也就是全部）")

    all_cmd = sub.add_parser("all", help="两样都生成到同一个目录")
    all_cmd.add_argument("-d", "--dir", type=Path, default=DEFAULT_DIR, metavar="DIR",
                         help=f"落到哪个目录（默认 {DEFAULT_DIR}）")
    all_cmd.add_argument("--count", type=int, default=len(HISTORY_POOL), metavar="N",
                         help="群历史取前几条")
    _add_csv_options(all_cmd)

    return ap


def _check_months(months: int) -> None:
    if not 1 <= months <= 120:
        raise SystemExit(f"--months 要在 1–120 之间，收到 {months}")


def _check_count(count: int) -> None:
    if not 1 <= count <= len(HISTORY_POOL):
        raise SystemExit(f"--count 要在 1–{len(HISTORY_POOL)} 之间，收到 {count}")


def _emit_csv(path: Path, *, start: tuple[int, int], months: int, seed: int) -> None:
    _check_months(months)
    rows = build_rows(start=start, months=months, seed=seed)
    write_text(path, render_csv(rows))
    print(f"CSV      {path}")
    for line in summarize(rows):
        print(line)


def _emit_history(path: Path, *, count: int) -> None:
    _check_count(count)
    write_text(path, render_history(count))
    print(f"群历史   {path}（{count} 条，其中 2 条是干扰项）")


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)

    if args.cmd == "csv":
        _emit_csv(args.out, start=args.start, months=args.months, seed=args.seed)
    elif args.cmd == "history":
        _emit_history(args.out, count=args.count)
    else:
        _emit_csv(args.dir / CSV_NAME, start=args.start, months=args.months, seed=args.seed)
        _emit_history(args.dir / HISTORY_NAME, count=args.count)

    print()
    print("下一步：把 CSV 拖进飞书测试群那条 @ 消息里；垫场文本按里面的说明发。剧本见 docs/demo-3min.md。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
