# 任务 T13 — 冷启动到交付：给主干快乐路径补进程级贯通

## 背景：这轨是从哪来的

T1–T12 十二轨全部合进 main（最新 `0d6939c`）。全仓 **969 passed**（没有 xfailed 了），
§3.8 的 10 个评测场景 **passed 10/10**，`scripts/check.sh` 全部通过，`.contracts.lock` 是 `OK 11 files`。
P0 的代码面齐了，剩下 §2.4 的真机验收 M1–M6 卡在飞书凭证（总管手上）。

盘点现有覆盖时发现一个空白。`tests/integration/` 那 17 条用例，逐个看下来是这样分布的：

| 文件 | 条数 | 测什么 |
|---|---|---|
| `test_t8_build_app_contract.py` | 4 | C-TΩ-1 的三个签名、注入点、无副作用 |
| `test_t8_graceful_shutdown.py` | 3 | 停机顺序：platform 先停、store 最后关 |
| `test_t8_shutdown_grace_timeout.py` | 3 | 宽限期超时、硬取消后善终 |
| `test_t8_evidence_on_disk.py` | 5 | events.jsonl / manifest 落盘、篡改检测 |
| `test_t8_sqlite_cross_process.py` | 2 | 换实例续接、去重存活 |

**全部围绕停机、契约签名、落盘。从「进程起来」到「任务交付」这条主干，进程级一条都没有。**

它不是没被测过 —— 而是被测在**别的层**：`aite/evals/` 那 10 个场景验的是 in-process 装配
（自己拼 plane + worker，不走 `run_app`），`tests/worker/` 验的是 worker 内部。
`run_app` 这一层今天只被「怎么停」验过，没被「怎么跑起来并交付」验过。

M1（@ 一下有反应）和 M3（CSV 画图、卡片更新、产物回线程）在真机上走的正是这条路。
真机上出问题没有断言告诉你哪一行红了 —— 只有群里一条没回的消息。**这轨就是把这条路钉住。**

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t13
分支     : task-t13
基线     : 0d6939c   ← main 的 HEAD（完整 sha 0d6939ca119ed40bb3320746d357aae85ebb4c41）
Python   : python3.12（3.12.1）
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**。
venv 已经给这一轨建好并 `pip install -e ".[dev]"` 过了（六轨里只有你这个装好了），
直接用 `.venv/bin/python`。文档正文里的 `python` 字样照原样保留。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 0d6939c
git rev-parse --abbrev-ref HEAD         # 期望 task-t13
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check    # 期望 OK 11 files，退出码 0
.venv/bin/python -m pytest tests/integration -q    # 期望 17 passed
.venv/bin/python -m pytest -q                      # 期望 969 passed（没有 xfailed）
scripts/check.sh                                   # 期望最后一行 全部通过，退出码 0
```

`check.sh` 里几个数字，对不上就是回归：A5 全仓可收集 **969 tests**、
C1 契约测试 **335 passed**、T4 替身自测 **190 passed**、B8 评测 **passed 10/10**。

**还有一条自检是验守卫的**（这条要"被拦"才算过）：

```
Read 工具读 .claude/hooks/guard_bash.py
```

期望**被 hook 拦下**，报 `blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）`。
被拦 = 守卫挂上了。**没被拦就说明 hook 失效了，停下报告** —— hook 失败是非阻塞放行且不报警，
守卫会静默失效，后面你改到契约都不会有人拦。

## 守卫会拦你的几种写法（前几轨实撞出来的，照着避开省时间）

| 写法 | 结果 |
|---|---|
| Bash 命令里出现 `aite/contracts`、`.contracts.lock`、`docs/dev-spec-*.md` 字面量 | 拦停，**哪怕只是 `[ -e ]` 只读探测**。想看目录就 `ls aite/` |
| `find ... -delete` / `find ... -exec` | 拦停（覆盖面判不出来）。逐个写明确路径 |
| `git commit -m "多行\n带引号的 message"` | 拦停（`No closing quotation`，shlex 解析失败）。**用 `git commit -F <文件>`** |

被拦是守卫在干活 —— **不要绕过它**。真需要碰受保护面 = 停下报告。

## 先读

1. `tests/integration/app_under_test.py` —— 被测组装的唯一入口，指的是真 `aite/app.py`（T11 切的）
2. `tests/integration/integration_fakes.py` —— 现成的替身与 `running_app()` 辅助，**你独占这个文件**
3. `tests/integration/test_t8_evidence_on_disk.py` —— 现成的落盘断言写法，照它的口径
4. `aite/app.py` 的 `build_app` / `run_app` —— 尤其 data/ 目录是怎么建的
5. `evals/p0/03_checklist_progress.yaml` 和 `04_csv_to_chart.yaml` —— in-process 层已经验过的判据，
   你要在 `run_app` 层把同样的事再钉一遍（不是抄，是换一层）

## 可写路径

```
tests/integration/**        （新建 test_t13_*.py；integration_fakes.py / conftest.py 你独占）
aite/app.py                 （只有你这一轨碰它）
```

**只读，改了就是任务失败**：`aite/contracts/**`、`.contracts.lock`、`docs/dev-spec-2026-09-09.md`。

并行的另外五轨在动这些地方，**别碰**：
`aite/control/plane.py`（T14）、`aite/control/store.py`（T18）、`aite/worker/**`（T14）、
`aite/adapters/feishu/**`（T16）、`aite/evals/**` `aite/models/**`（T17）、
`docs/demo-3min.md` `scripts/demo_*`（T15）、`docs/feishu-api-diff.md`（T16）。

按 spec §7：接口契约里没有 → **停，报告**；要改的文件不在白名单 → **停，报告**。

## 目标：一条贯通用例（可以拆成几条，但必须走同一个 `run_app`）

从**空的 data/ 目录**开始，到任务 `delivered`、进程干净停机为止，一路断言。要覆盖的节点：

1. **冷启动**：`storage.sqlite_path` / `evidence_dir` / `artifacts_dir` 的父目录都不存在时，
   `run_app` 起来后它们被建出来（preflight 说的"data 待建，起飞时自动 mkdir"要真的成立）
2. **R7**：@ 事件进来 → `add_reaction(ack)` → 建 session + task、`task_no` 是 `#A1`
3. **W3**：第一个非 `final` 的 tool_call 之前先 `send_card`（status=working），且**只有一张卡**
4. **W4**：过程中 `update_card` ≥3 次，且**没有第二条 `send_card`**、没有新增消息
5. **工具链**：`download_attachment` → `run_python` → 沙箱里产出文件 → `list_files` 看得见
6. **W5**：`final(artifacts)` → `get_file` → `send_file`（`reply_to` = 话题 root）→ 写 evidence
   `artifact` → `send_text(reply)` → task `delivered`
7. **evidence**：`events.jsonl` 链完整、`manifest.json` 的 `root_hash` == 最后一条 `hash`、
   `verify()` 为 True、`Task.evidence_root_hash` 进了库
8. **停机**：`run.stop` 之后进程收干净，沙箱还回去了

用 `FakeSandbox` 就行，**不要依赖真 Docker**（那是 B4 的事，跑在 `-m docker` 里）。
模型用 `integration_fakes` 里的脚本化替身，脚本要覆盖「checklist_add 3 项 → 逐项 check →
run_python → final(artifacts)」这条完整出牌。

### 两个坑，先说在这

- **别把评测的判据抄过来当断言**。in-process 那层已经验过一遍了，你这层要验的是
  「同样的行为在 `run_app` 装配下也成立」。同一件事在两层都红时，你的用例要能指出是哪一层的问题。
- **`data/` 一个字节都不许写进仓库**。现成的 `test_evidence_stays_inside_tmp_path`
  钉的就是这条，照它的做法用 `tmp_path`。

## 目标二（顺带）：`aite/app.py` 的 mkdir 行为如果不成立就修

如果第 1 条断言（冷启动建目录）红了，那是真 bug —— `aite/app.py` 归你这轨，直接修。
修之前先在回执里记一句「app.py 原来没有 mkdir / mkdir 漏了哪个目录」，别默默改掉。

## 验收

```bash
.venv/bin/python -m pytest tests/integration -q     # 期望 ≥18 passed（你加了几条就多几条）
.venv/bin/python -m pytest -q                       # 期望 ≥970 passed，一条不许红
scripts/check.sh                                    # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
```

`.contracts.lock --check` 必须始终 `OK 11 files`（C2）。变红 = 这一轨失败。

## 回执格式

```
## T13 回执

基线 0d6939c → 提交 <短 sha>

### 加了什么
- <文件:行> <一句话>

### 八个节点各自的结论
1 冷启动建目录：<成立 / 原来不成立，改了 app.py 哪里>
2 R7 ack + task_no：<>
3 W3 只有一张卡：<>
4 W4 update_card ≥3 且无新增消息：<>
5 工具链：<>
6 W5 产物与交付：<>
7 evidence 链与 manifest：<>
8 停机与还沙箱：<>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m pytest tests/integration -q
<最后一行>

$ scripts/check.sh
<最后 3 行>

### 发现的真 bug（这轨的价值在这一栏）
<没有就写"没有"。每条写清：现象、在哪一层、改没改、为什么>

### 卡住的地方 / 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t13` 分支上，回执贴出来，
合并由总管在主仓做。
