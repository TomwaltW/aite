"""`scripts/demo_fixture.py` 的测试（owner: T15）。

这个脚本的存在理由只有一条：**演示不该每次都是一场赌博**。所以测试也就盯着那条 ——
不是"函数返回了个 list"，而是"照着 docs/demo-3min.md 演的人，会不会看到和彩排时不同的东西"：

* 同样参数跑两次，落盘字节一致（且换个 seed 真的会变，否则第一条是句空话）；
* 列名就是 `evals/p0/04_csv_to_chart.yaml` 里 `df.plot(x="month", y="amount")` 那两个；
* 最高最低月唯一且拉得开 —— 第二幕要当场拿模型的答案跟它对，含糊了就对不出来；
* 垫场文本里**一个 @ 都没有**：垫场消息 @ 一下就是一个任务，群里会多出一堆卡片；
* CSV 真能被 pandas 读 —— 而且是被**沙箱镜像里那个** pandas 读，因为演示时跑它的是那个。
"""
import subprocess

import demo_fixture as fx
import pytest

IMAGE = "aite-sandbox:p0"


# --------------------------------------------------------------------------
# 可复现
# --------------------------------------------------------------------------


def test_same_args_twice_are_byte_identical(tmp_path):
    """彩排看到的那张图和正式演示那张，是同一张 —— 全靠这条。"""
    a, b = tmp_path / "a.csv", tmp_path / "b.csv"
    assert fx.main(["csv", "-o", str(a)]) == 0
    assert fx.main(["csv", "-o", str(b)]) == 0
    assert a.read_bytes() == b.read_bytes()


def test_a_different_seed_really_changes_the_bytes(tmp_path):
    """没有这条，上一条可能只是因为 seed 压根没被用上。"""
    a, b = tmp_path / "a.csv", tmp_path / "b.csv"
    fx.main(["csv", "-o", str(a), "--seed", "1"])
    fx.main(["csv", "-o", str(b), "--seed", "2"])
    assert a.read_bytes() != b.read_bytes()


def test_start_month_is_a_frozen_constant_not_today():
    """默认起始月跟了今天，就没有"同样参数"这回事了。"""
    assert fx.DEFAULT_START == "2024-09"


def test_line_endings_are_lf_and_utf8(tmp_path):
    out = tmp_path / "sales.csv"
    fx.main(["csv", "-o", str(out)])
    raw = out.read_bytes()
    assert b"\r" not in raw
    assert raw.endswith(b"\n")
    raw.decode("utf-8")            # 解不开就是编码漂了


# --------------------------------------------------------------------------
# CSV 的形状
# --------------------------------------------------------------------------


def test_header_matches_the_eval_scenario(tmp_path):
    """04_csv_to_chart.yaml 的脚本化模型写死了 x="month", y="amount"。
    列名一变，脚本化彩排和真模型演示就不是同一副牌了。"""
    out = tmp_path / "sales.csv"
    fx.main(["csv", "-o", str(out)])
    assert out.read_text(encoding="utf-8").splitlines()[0] == "month,amount"
    assert fx.COLUMNS == ("month", "amount")


@pytest.mark.parametrize("months", [1, 12, 24, 36])
def test_row_count_follows_months(tmp_path, months):
    out = tmp_path / "sales.csv"
    fx.main(["csv", "-o", str(out), "--months", str(months)])
    lines = out.read_text(encoding="utf-8").splitlines()
    assert len(lines) == months + 1          # 表头 + 数据行


def test_default_size_is_previewable_in_feishu():
    """派单要求"几十行"：飞书里预览得下、模型也读得完。"""
    assert 12 <= fx.DEFAULT_MONTHS <= 60


def test_every_row_is_what_pandas_will_need(tmp_path):
    """不装 pandas 也能验的那部分：月份连续且可排序、金额是正整数。"""
    out = tmp_path / "sales.csv"
    fx.main(["csv", "-o", str(out), "--months", "24"])
    rows = [ln.split(",") for ln in out.read_text(encoding="utf-8").splitlines()[1:]]

    months = [r[0] for r in rows]
    assert months == sorted(months)                     # 字典序 == 时间序，YYYY-MM 才有这性质
    assert len(set(months)) == len(months)
    assert months[0] == "2024-09" and months[-1] == "2026-08"

    amounts = [int(r[1]) for r in rows]                 # 不是整数这里就炸
    assert all(v > 0 for v in amounts)


def test_the_line_is_not_flat(tmp_path):
    """全是直线的图演示效果差。用极差比均值粗略卡一道。"""
    rows = fx.build_rows(start=fx.parse_start("2024-09"), months=24, seed=fx.DEFAULT_SEED)
    values = [v for _, v in rows]
    assert (max(values) - min(values)) / (sum(values) / len(values)) > 0.5


def test_peak_and_trough_are_unambiguous():
    """第二幕要当场核对模型说的最高/最低月。并列或差之毫厘就核不成。"""
    rows = fx.build_rows(start=fx.parse_start(fx.DEFAULT_START),
                         months=fx.DEFAULT_MONTHS, seed=fx.DEFAULT_SEED)
    ordered = sorted(rows, key=lambda r: r[1])
    low, second_low = ordered[0], ordered[1]
    top, second_top = ordered[-1], ordered[-2]

    # 这两个月份就是剧本里写的那两个；改了 SEASON / 种子就等于改了演示的台词。
    assert top[0] == "2025-11"
    assert low[0] == "2025-07"
    # 亚军拉开 3% 以上，肉眼和读数都不会犹豫（实测：高点 +4.4%，低点 −10.5%）。
    assert top[1] > second_top[1] * 1.03
    assert low[1] < second_low[1] / 1.03


# --------------------------------------------------------------------------
# 群历史垫场文本
# --------------------------------------------------------------------------


def test_history_is_byte_reproducible(tmp_path):
    a, b = tmp_path / "a.txt", tmp_path / "b.txt"
    fx.main(["history", "-o", str(a)])
    fx.main(["history", "-o", str(b)])
    assert a.read_bytes() == b.read_bytes()


def test_history_bodies_have_no_at_mention():
    """垫场消息里出现 @ = 每条都触发一个任务，群里瞬间多出一堆卡片。

    只管正文：文件头部那句「一条都不要 @Aite」的告警本身当然带 @，
    而真正会被发进群的，只有渲染出来的缩进行。
    """
    assert not [body for _, body in fx.HISTORY_POOL if "@" in body]

    rendered = fx.render_history(len(fx.HISTORY_POOL))
    bodies = [ln for ln in rendered.splitlines() if ln.startswith("    ")]
    assert len(bodies) == len(fx.HISTORY_POOL)
    assert not [ln for ln in bodies if "@" in ln]


def test_history_keeps_the_two_distractors():
    """一条已办完、一条纯闲聊。汇总里不该出现它俩 —— 这是加演那段的看点。"""
    bodies = [body for _, body in fx.HISTORY_POOL]
    assert "已经处理完了" in bodies[3]
    assert "中午一起吃饭" in bodies[6]


def test_history_count_slices_from_the_front(tmp_path):
    out = tmp_path / "h.txt"
    fx.main(["history", "-o", str(out), "--count", "3"])
    text = out.read_text(encoding="utf-8")
    assert fx.HISTORY_POOL[2][1] in text
    assert fx.HISTORY_POOL[3][1] not in text


# --------------------------------------------------------------------------
# 命令行
# --------------------------------------------------------------------------


def test_all_writes_both_files(tmp_path, capsys):
    assert fx.main(["all", "-d", str(tmp_path)]) == 0
    csv_path = tmp_path / fx.CSV_NAME
    his_path = tmp_path / fx.HISTORY_NAME
    assert csv_path.exists() and his_path.exists()

    out = capsys.readouterr().out
    assert str(csv_path) in out and str(his_path) in out
    # 演示时要拿这两行去核对模型，不打出来这脚本就白写了
    assert "最高" in out and "最低" in out


def test_it_creates_missing_parent_dirs(tmp_path):
    out = tmp_path / "nested" / "deeper" / "sales.csv"
    assert fx.main(["csv", "-o", str(out)]) == 0
    assert out.exists()


@pytest.mark.parametrize("bad", ["0", "121"])
def test_rejects_out_of_range_months(tmp_path, bad):
    with pytest.raises(SystemExit):
        fx.main(["csv", "-o", str(tmp_path / "x.csv"), "--months", bad])


def test_rejects_out_of_range_count(tmp_path):
    with pytest.raises(SystemExit):
        fx.main(["history", "-o", str(tmp_path / "x.txt"), "--count", "99"])


@pytest.mark.parametrize("bad", ["2024/09", "2024-13", "九月"])
def test_rejects_malformed_start(tmp_path, bad):
    with pytest.raises(SystemExit):
        fx.main(["csv", "-o", str(tmp_path / "x.csv"), "--start", bad])


# --------------------------------------------------------------------------
# 真的能被沙箱镜像里的 pandas 读
# --------------------------------------------------------------------------


def _docker_ready() -> bool:
    def run(*args):
        return subprocess.run(["docker", *args], capture_output=True, text=True, timeout=20)

    try:
        if run("info", "--format", "{{.ServerVersion}}").returncode != 0:
            return False
        return run("image", "inspect", IMAGE).returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


READ_IT = (
    "import pandas as pd;"
    "df = pd.read_csv('/tmp/sales.csv');"
    "print(list(df.columns), len(df), str(df['amount'].dtype),"
    " df.loc[df['amount'].idxmax(), 'month'], df.loc[df['amount'].idxmin(), 'month'])"
)


@pytest.mark.docker
def test_sandbox_pandas_reads_it(tmp_path):
    """演示时读这份 CSV 的是镜像里那个 pandas，不是宿主上的（宿主上压根没装）。

    刻意不打 `aite.task` 标签：`tests/sandbox` 那边有几条断言是
    `docker ps -a --filter label=aite.task` 为空，打了标签就会去撞它们。
    """
    if not _docker_ready():
        pytest.skip(f"没有 Docker daemon 或没有 {IMAGE} 镜像")

    csv_path = tmp_path / "sales.csv"
    fx.main(["csv", "-o", str(csv_path)])

    proc = subprocess.run(
        ["docker", "run", "--rm", "--network", "none",
         "-v", f"{csv_path}:/tmp/sales.csv:ro", IMAGE, "python", "-c", READ_IT],
        capture_output=True, text=True, timeout=120,
    )
    assert proc.returncode == 0, proc.stderr
    assert proc.stdout.strip() == "['month', 'amount'] 24 int64 2025-11 2025-07"
