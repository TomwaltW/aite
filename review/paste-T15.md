# 任务 T15 — 3 分钟演示：分镜脚本 + 可复现的演示素材

## 背景：这轨是从哪来的

T1–T12 十二轨全部合进 main（最新 `0d6939c`）。全仓 **969 passed**，
§3.8 的 10 个评测场景 **passed 10/10**，`scripts/check.sh` 全部通过。P0 的代码面齐了。

翻回 spec §1「要做什么」那一句，P0 的目标里明写着两个交付物：

> …能**录一段 3 分钟演示**，并且用 10 个脚本化场景自动验收。

后半句做完了（`passed 10/10`）。**前半句一个字都没有。**

现有的 `docs/acceptance-M.md` 不是它。那份是**验收**剧本 —— M1–M6 每条「操作 / 在哪看 /
期望 / 不对时查哪」，回答的是"对不对"。演示要回答的是另一个问题：**3 分钟里按什么顺序
让人看懂这东西凭什么值钱**，以及演到一半翻车了怎么在镜头前救回来。

这两件事写不到一份文档里：验收要穷尽失败面，演示要砍掉一切枝节。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t15
分支     : task-t15
基线     : 0d6939c   ← main 的 HEAD（完整 sha 0d6939ca119ed40bb3320746d357aae85ebb4c41）
Python   : python3.12（3.12.1）—— venv 要你自己建
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

文档正文里的 `python` 字样照原样保留 —— 你写的剧本是给人照着敲的，**里面就该写 `python`
还是 `.venv/bin/python`，取决于总管起飞时怎么跑**，这一点自己判断，别机械替换。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 0d6939c
git rev-parse --abbrev-ref HEAD         # 期望 task-t15
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check    # 期望 OK 11 files，退出码 0
.venv/bin/python -m pytest -q                      # 期望 969 passed（没有 xfailed）
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python scripts/preflight.py --offline    # 期望退出码 0
```

**还有一条自检是验守卫的**（这条要"被拦"才算过）：

```
Read 工具读 .claude/hooks/guard_bash.py
```

期望**被 hook 拦下**，报 `blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）`。
被拦 = 守卫挂上了。**没被拦就说明 hook 失效了，停下报告**。

## 守卫会拦你的几种写法（前几轨实撞出来的）

| 写法 | 结果 |
|---|---|
| Bash 命令里出现 `aite/contracts`、`.contracts.lock`、`docs/dev-spec-*.md` 字面量 | 拦停，哪怕只是 `[ -e ]` 只读探测 |
| `find ... -delete` / `find ... -exec` | 拦停（覆盖面判不出来） |
| `git commit -m "多行\n带引号的 message"` | 拦停（shlex 解析失败）。**用 `git commit -F <文件>`** |

**不要绕过守卫。** 真需要碰受保护面 = 停下报告。

## 先读（这轨的功课主要在读，不在写代码）

1. `docs/acceptance-M.md` —— **只读**。看它已经把什么写透了，你不要重复；
   尤其 0.3「三个观察窗」和每个 M 的「不对时查哪」，演示救场可以引它而不是抄它
2. `scripts/preflight.py --help` 与 `scripts/evidence_show.py --help` —— 演示前后要用的两把工具
3. `evals/p0/03_checklist_progress.yaml`、`04_csv_to_chart.yaml`、`05_history_summary.yaml`
   —— 演示的主戏就是这三段，脚本化版本长什么样先看清
4. `aite/worker/prompts/platform.md` —— 模型被要求怎么说话，决定了演示里它的输出长相
5. spec §2.4 的 M1–M6 —— 演示挑哪几段、砍哪几段，从这里选

## 可写路径

```
docs/demo-3min.md                  （新建，这轨的主产出）
scripts/demo_fixture.py            （新建，演示素材生成器）
tests/tools/test_demo_fixture.py   （新建，给素材生成器配测试）
```

**只读，改了就是任务失败**：`docs/acceptance-M.md`（T10 的）、`aite/contracts/**`、
`.contracts.lock`、`docs/dev-spec-2026-09-09.md`、`scripts/` 下已有的三个文件
（`check.sh` / `preflight.py` / `evidence_show.py`）。

并行的另外五轨在动这些地方，**别碰**：`tests/integration/**` `aite/app.py`（T13）、
`aite/control/plane.py` `aite/worker/**`（T14）、`aite/adapters/feishu/**`
`docs/feishu-api-diff.md`（T16）、`aite/evals/**` `aite/models/**`（T17）、
`aite/control/store.py`（T18）。

## 目标一：`docs/demo-3min.md` 分镜脚本

按**时间轴**写，不是按功能列表写。建议的骨架（时长可调，总长压在 3 分钟内）：

```
0:00–0:15  开场：这是什么（一句话），屏幕上放什么
0:15–0:45  第一幕 · @ 一下就有人应（M1）
0:45–1:45  第二幕 · 把 CSV 画成图（M3）—— 主戏，卡片原地更新是卖点
1:45–2:15  第三幕 · 话题里追问，它记得上文（M4）
2:15–2:40  第四幕 · 证据链：刚才那一分钟它到底做了什么（evidence_show）
2:40–3:00  收尾：一句话说清 P0 验掉了什么
```

每一幕都要写足这几栏，缺一栏这轨就没做完：

- **屏幕怎么摆**：飞书群窗口 / 终端 / 两个都要？分屏还是切换？
- **手上做什么**：群里发的原话（**逐字写出来**，演示时照着念，不要临场编）
- **几秒内该出现什么**：观众看得见的信号（👀 表情、卡片出现、卡片第 3 次更新、PNG 出现）
- **同时终端在滚什么**：哪条日志能证明它没作弊
- **说什么**：旁白要点（不用写成台词，写要点，但要写"这里必须说到 XX"）
- **翻车怎么救**：这一幕最可能出什么问题、镜头前怎么圆、什么情况必须停下重录

主戏那一幕（M3）要特别写清 **卡片原地更新**怎么让观众看见 —— 这是 Claude Tag
区别于「机器人刷屏」的核心，如果观众没注意到"消息没有变多，只是那张卡在变"，演示就白演了。

### 还要写的两段

**演示前 5 分钟的检查清单**：`preflight` 跑哪个模式、Docker 起没起、镜像在不在、
`data/` 要清空还是保留（清空则 `task_no` 从 `#A1` 开始，画面干净；保留则能演 M6 续接）、
群里要不要先垫几条历史消息（M5 的「汇总本周开放事项」需要真实历史才有东西可汇总）。

**不演什么，以及为什么**：M2（拔网线）和 M6（重启续接）在镜头前很难拍得好看且耗时间 ——
写清楚它们留给验收、不进演示，免得下次有人问"为什么没演断网"。

## 目标二：`scripts/demo_fixture.py` 让演示可复现

演示要用一个 CSV（M3 那一幕）。手捏一个每次都不一样的文件，等于每次演示都在赌。
写一个生成器：固定随机种子、生成一份**月度数据**的 CSV（列名和数据要让"画月度趋势图"
这个要求成立），落到指定路径。

要求：

- 固定 seed，同样参数跑两次字节一致（配测试断言这条）
- 数据要有点起伏，画出来的图好看（全是直线的图演示效果差）
- 行数适中（几十行，飞书里预览得下，模型读得完）
- `--help` 说清怎么用；不写进 `data/`，默认落到 `/tmp` 或参数指定的路径
- 顺带可以生成 M5 需要的几条"群历史"文本（给总管照着往群里发，垫出可汇总的内容）

配 `tests/tools/test_demo_fixture.py`：字节可复现、列名对得上、行数在范围内、
CSV 能被 pandas 读（沙箱镜像里有 pandas，演示时模型会用它）。

## 验收

```bash
.venv/bin/python -m pytest -q                     # 期望 ≥969 passed（你加了测试就更多），一条不许红
.venv/bin/python -m pytest tests/tools -q         # 期望 ≥32 passed
scripts/check.sh                                  # 期望 全部通过，退出码 0
.venv/bin/python scripts/demo_fixture.py --help   # 退出码 0，说明清楚
```

剧本本身没法用 pytest 验，所以自己走一遍这个检查：**把 `docs/demo-3min.md` 从头读到尾，
每一幕问自己"照着这段，一个没参与过开发的人能不能独立演完"**。答不上就是没写够。
回执里要写明你这么自查过，以及哪一幕最没底。

`.contracts.lock --check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T15 回执

基线 0d6939c → 提交 <短 sha>

### 产出
- docs/demo-3min.md      <多少行，几幕>
- scripts/demo_fixture.py <多少行>
- tests/tools/test_demo_fixture.py <几条>

### 分镜表（一行一幕）
| 时间 | 演什么 | 观众看见的信号 | 最可能翻车的点 |

### 自查
照着剧本能不能让没参与开发的人独立演完：<结论>
最没底的一幕：<哪幕，为什么>

### 实测输出（粘实际的）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python scripts/demo_fixture.py --help
<前几行>

$ scripts/check.sh
<最后 3 行>

### 需要总管决定的
<比如：演示用真模型还是脚本化替身？群里垫历史消息谁来发？没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t15` 分支上，回执贴出来。
