# 任务 T11 — 让集成测试落到生产组装 + 补齐 cancel 路径的 evidence finalize

## 背景：这轨是从哪来的

T1–T10 十轨全部合进 main（最新 `f9d45ab`）。全仓 **936 passed, 1 xfailed**，
§3.8 的 10 个评测场景 **passed 10/10**，`scripts/check.sh` 全部通过。P0 的代码面基本齐了，
剩下的是 §2.4 的真机验收 M1–M6（卡在飞书凭证，总管手上）。

但有一件事从没有人做过。`tests/integration/app_under_test.py:13` 至今写着：

```python
from t8_minimal_app import AiteApp, build_app, run_app
```

而这个文件自己的 docstring 就写着：

> 并轨后：`from aite.app import AiteApp, build_app, run_app`
> 并把 `tests/integration/t8_minimal_app.py` 删掉。

T7（真 `aite/app.py`）和 T8（集成测试）是同一批并行里各自交付的，基线都是 `34e9dee`，
当时 T7 还没合，T8 只能先接自己写的 227 行最小替身。两轨先后合进 main 之后，
**谁都没有做这个切换动作**。

后果：`tests/integration/` 下 16 条测试（号称"进程级接线"）测的全是 T8 那个替身，
**从没碰过 `aite/app.py`**。它们给出的绿灯，对生产组装不成立。这是这一轨的第一件事。

### 我已经实测过的两件事（你不用重做，但开场自检要对得上）

**一**：把那一行切到真 `aite.app`，16 条里 **15 passed，1 xfailed** —— 只有原本那条 xfail 还红。
也就是说 T7 的组装满足 C-TΩ-1 的三个签名，切换本身是安全的，不会引出一堆签名不匹配。

**二**：那条 xfail（`test_t8_shutdown_grace_timeout.py:122`，`strict=True`）的 reason 文字
是照替身的行为写的，**已经过时**。用 `--runxfail` 把三条断言的真假打出来，两种组装对比：

| 断言 | 真 `aite/app.py` | T8 替身 |
|---|---|---|
| 沙箱还回去了 | **True** | False |
| 任务落到 cancelled 终态 | **True** | False |
| 证据链 finalize 了（有 manifest.json） | **False** | False |

T7 的组装已经把前两条修好了（`aite/app.py:405` 那段兜底：宽限期超时先把 `worker.in_flight`
里在飞的任务抄进 `stranded`，`runner.cancel()` 之后逐个走 `plane.cancel_task(notify=False)`）。
**只剩 manifest.json 这一条。** 所以 reason 里"沙箱不还、任务状态停在 working"两句，
对真组装已经不成立，别照着它去修不存在的问题。

### 根因（已定位到函数，省你排查）

`aite/worker/loop.py:482` 的 `_finish()` 是三条终态路径的汇合点：

```
_deliver (409, delivered) ─┐
_fail    (457, failed)    ─┼─→ _finish() ─→ evidence.finalize() + _release_gateway_sandbox()
_cancel  (474, cancelled) ─┘
```

worker 自己走完的路都是对的 —— finalize 写 `manifest.json`、回写 `Task.evidence_root_hash`、
还沙箱，一个不少。

但 `aite/app.py` 的 `_shutdown()` 宽限期超时那一支走的不是 worker 的 `_cancel`
（worker 只在每步开头查标志位，硬取消时拿不到 `CancelledError`），而是
`aite/control/plane.py:445` 的 `cancel_task()`。那条路在 `not running` 分支里
做了四件事：`update_task` 置 cancelled、`_release_gateway_sandbox`、
`evidence.append(cancelled)`、`update_card`。**唯独没有 `finalize`。**

于是：没有 `manifest.json`，`Task.evidence_root_hash` 留空。

影响面不止停机。`!stop #A..` 命中"任务不在跑"那条分支时走的是同一个
`cancel_task`，所以真机上被 `!stop` 掉的任务同样没有 manifest。而 §2.4 的
`scripts/evidence_show.py`、`Task.evidence_root_hash` 字段、卡片上的证据按钮
全都指着 manifest 的 `root_hash`。

---

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t11
分支     : task-t11
基线     : f9d45ab   ← main 的 HEAD（完整 sha f9d45ab1f2899a7f297911d60acafc1fa0c31b87）
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，要用 `python3.12`。
**venv 已经建好并 `pip install -e ".[dev]"` 过了，你不用再装。**
文档正文里的 `python` 字样照原样保留，只有本机执行时才换 `.venv/bin/python`。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 f9d45ab
git rev-parse --abbrev-ref HEAD         # 期望 task-t11
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check      # 期望 OK 11 files，退出码 0
.venv/bin/python -m pytest tests/integration -q      # 期望 15 passed, 1 xfailed
.venv/bin/python -m pytest -q                        # 期望 936 passed, 1 xfailed
scripts/check.sh                                     # 期望最后一行 全部通过，退出码 0
```

`check.sh` 里几个数字，对不上就是回归：A5 全仓可收集 **937 tests**、
C1 契约测试 **335 passed**、T4 替身自测 **159 passed**、B8 评测 **passed 10/10**。

**还有一条自检是验守卫的**（这条要"被拦"才算过）：

```
Read 工具读 .claude/hooks/guard_bash.py
```

期望**被 hook 拦下**，报 `blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）`。
被拦 = 守卫挂上了。**没被拦就说明 hook 失效了，停下报告** —— hook 执行失败是非阻塞放行且不报警，
守卫会静默失效，后面你改到契约都不会有人拦你。

## 守卫会拦你的几种写法（我这一轮实撞出来的，照着避开省时间）

| 写法 | 结果 |
|---|---|
| Bash 命令里出现 `aite/contracts`、`.contracts.lock`、`docs/dev-spec-*.md` 字面量 | 拦停，**哪怕只是 `[ -e ]` 只读探测**。想看目录就 `ls aite/`，别写全路径 |
| `find ... -delete` / `find ... -exec` | 拦停（覆盖面判不出来）。逐个写明确路径 |
| `git commit -m "多行\n带引号的 message"` | 拦停（`No closing quotation`，shlex 解析失败）。**用 `git commit -F <文件>`** |

被拦是守卫在干活，不是你写错了 —— 但**不要想办法绕过它**。真需要碰受保护面 = 停下报告。

## 先读

1. `tests/integration/app_under_test.py`（15 行，切换点就这一行）
2. `aite/app.py` 的 `_shutdown()`（377–420）—— 兜底怎么走到 `cancel_task`
3. `aite/control/plane.py` 的 `cancel_task()`（445–495）—— 缺 finalize 的那条路
4. `aite/worker/loop.py` 的 `_finish()`（482–497）—— **对照它，你要补的行为以它为准**
5. `tests/integration/test_t8_shutdown_grace_timeout.py`（那条 xfail 在 122–152）

## 可写路径

```
tests/integration/**          （含删掉 t8_minimal_app.py）
aite/control/plane.py
aite/app.py
```

**只读，改了就是任务失败**：`aite/contracts/**`、`.contracts.lock`（守卫也会拦）、
`docs/dev-spec-2026-09-09.md`。其余目录一律只读。

按 spec §7：需要的接口契约里没有 → **停，报告**，不要自己发明；
需要改的文件不在上面这三行里 → **停，报告**，不要"就改一行"。

## 目标一：让集成测试落到生产组装

1. `tests/integration/app_under_test.py` 那一行改成 `from aite.app import AiteApp, build_app, run_app`
2. 删掉 `tests/integration/t8_minimal_app.py`（227 行，它的使命到此结束）
3. 确认没有别的文件还在 import 它：`grep -rn t8_minimal_app tests/`

做完这步 `pytest tests/integration -q` 应该是 **15 passed, 1 xfailed**（我实测过）。
如果你看到的不是这个数，**先停下报告**，别急着改生产代码 —— 差异本身是信息。

## 目标二：`cancel_task` 补齐 evidence finalize

`aite/control/plane.py` 的 `cancel_task()`，在 `not running` 那条分支里补 finalize，
**行为以 `aite/worker/loop.py:_finish()` 为准**（同一件事在两条路上必须长得一样）：

- 调 `EvidenceWriter.finalize(task_id, manifest_extra)`，`manifest_extra` 的字段照
  `_finish()` 那份：`session_id` / `task_no` / `created_by` / `model`
- 把返回的 root_hash 回写 `Task.evidence_root_hash`，**注意落盘顺序**：
  现在 `update_task` 在函数很前面就调了，finalize 之后要让 `evidence_root_hash` 真的进库
- `finalize` 只能调一次。worker 已经收尾过的任务不要再 finalize 一遍
  （`running` 分支照旧交给 worker，别动）

顺带确认一件事并在回执里回答：**`_fail` 之外还有没有别的路径会让任务进终态却不过 `_finish()`**。
§3.3 里 failed 场景有十几条（超步数、模型不可用、沙箱连续 2 次失败、未捕获异常…），
它们是不是都汇到了 `_fail` → `_finish`。是就写一句"已确认都汇到 _finish"，
发现漏的就一并补上并在回执里点名。

## 目标三：摘掉那条 xfail

前两步做完后，`test_cancelled_task_should_land_on_cancelled_and_return_its_sandbox`
应该真的绿了。它是 `strict=True`，所以**修好之后不摘标记，pytest 会报 failed**
（XPASS strict）——这正是它设计成 strict 的用意，别把它改成 non-strict 来"让门禁过"。

- 摘掉 `@pytest.mark.xfail(...)` 整个装饰器
- 文件顶部 docstring 第 11 行提到"见本文件末尾那条 xfail 与回执 D-1 / D-2"，一起更新
- 函数 docstring 里"今天它是红的 —— 修好之后请把 xfail 摘掉"也要改掉

## 验收

```bash
.venv/bin/python -m pytest tests/integration -q     # 期望 16 passed（xfail 摘了，不再有 xfailed）
.venv/bin/python -m pytest -q                       # 期望 937 passed（936+1，不再有 xfailed）
scripts/check.sh                                    # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python -m pytest tests/control -q         # cancel_task 改了，这里不许回归
.venv/bin/python -m aite.evals run evals/p0 --only 07_commands --platform fake --model scripted
```

最后那条单独跑是因为 `07_commands` 验 `!stop` → task cancelled → `release` 被调用，
走的正是你改的 `cancel_task`。它必须还是绿的。

`.contracts.lock --check` 必须始终 `OK 11 files`（C2）。变红 = 这一轨失败。

## 回执格式

```
## T11 回执

基线 f9d45ab → 提交 <短 sha>

### 改了什么
- <文件:行> <一句话>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m pytest tests/integration -q
<最后一行>

$ scripts/check.sh
<最后 3 行>

$ .venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
<最后一行>

### 回答：终态路径有没有漏 _finish 的
<已确认都汇到 _finish / 发现 N 处漏，分别是…>

### 卡住的地方 / 要总管决定的
<没有就写"没有"。碰到要改契约的，写清卡在哪、试过什么、需要什么决定>
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t11` 分支上，回执贴出来，
合并由总管在主仓做。
