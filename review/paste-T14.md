# 任务 T14 — steer 机制：把 R6 那条「话题里追问」的路钉住

## 背景：这轨是从哪来的

T1–T12 十二轨全部合进 main（最新 `0d6939c`）。全仓 **969 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过。P0 的代码面齐了，剩 §2.4 的 M1–M6 卡在飞书凭证。

R6 是 P0 里最能体现"这不是个命令行机器人"的一条规则：

> `chat_type == group` 且 `anchor.thread_id` 命中 `find_session_by_thread` → 续接该会话
> （**不要求 mentioned**）：`append_turn(user)`；若该会话有活跃 task → **排队为 steer 消息
> （worker 每步开始前合并进上下文）**；否则新建 task 继续

M4 演示的就是它（"再按季度画一张"），而 3 分钟演示的第三幕靠它。

**但它今天只有 1 个测试用例。** `tests/worker/test_steer.py` 全文就一条
`test_steer_message_reaches_the_next_step`：第 1 步跑起来后往话题里塞一句不带 @ 的话，
断言它在下一步进了上下文。实现侧一共 16 处引用，分布在
`aite/control/plane.py`（排队）、`aite/worker/loop.py`（每步开头合并）。

一条用例覆盖不了这条路。我读实现时看到的几个没人验过的地方，按该关心的程度排：

### ① steer 队列只在内存里（这条最要紧）

```python
# aite/control/plane.py:100
self._steer: dict[str, list[str]] = defaultdict(list)
```

排队的追问是进程内存里的一个 dict。进程重启 → **队列全丢**。
`append_turn(user)` 那一半是落库的，所以重启后 transcript 里能看到用户说过这句话，
但"它本该被合并进正在跑的那个任务"这件事没了 —— 用户以为说了，系统再没反应。

而 M6 明写「杀进程重启后在旧线程追问仍能续接」。这两件事的边界在哪，今天没人定过。

**注意：这可能要动契约。** §3.2 的 `SessionStore` 协议里没有任何 steer 相关方法，
要落库就得加 —— 那是 T0 的地盘，**改契约 = 停下报告，不要自己加**（守卫也会拦你）。
所以这一条你的任务是**把行为钉清楚并给出判断**，不是自己拍板改设计：
写测试固定住"重启后排队的 steer 会丢"这个**当前事实**（用 xfail 或者直接断言现状 +
一句注释说明这是已知边界），在回执里写清代价和你建议的两三种修法，让总管定。

### ② 排给"最后一个"活跃任务的依据

```python
# aite/control/plane.py:277
self._steer[active[-1].id].append(ev.text)
```

一个会话有多个活跃 task 时，为什么是 `active[-1]`？`list_active_tasks` 的返回顺序
是不是保证了"最后一个就是最该收这句话的那个"？P0 单 worker，多活跃任务可能出现在
「一个在跑、一个在队列里」的时候 —— 这时追问该给谁？

### ③ 任务已经落终态之后到达的 steer

`plane.py:420` 在任务收尾时 `self._steer.pop(task_id, None)`。如果 steer 在这之后到达，
`defaultdict(list)` 会给一个已经死掉的 task_id **重新建一个条目**，然后永远没人来 drain
—— 内存里的一条泄漏，用户的那句话也石沉大海。R6 说"否则新建 task 继续"，
那么 steer 到得太晚时，该不该退化成"新建 task"？

### ④ 其余几条

- **多条堆积的顺序**：连发三句，`drain_steer()` 出来是不是原序，合并进上下文是三条还是一条
- **与 `!stop` 竞争**：steer 排着队时 `!stop` 掉那个任务，队列清没清
- **在 W1 上下文里的位置**：W1 规定了 system → transcript → 群历史 → 附件清单 → 工具目录
  的顺序，steer 插在哪一层？插错了模型会把它当成历史的一部分而不是新指令
- **写不写 evidence**：W8 列了 `model_call / tool_call / tool_result / checklist_op` 要写
  evidence，steer 不在其中 —— 那证据链里就看不到"任务跑到一半用户改了要求"这件事。
  §2.4 的 `evidence_show.py` 时间线上会缺这一段。这是不是有意的？
- **超长 steer**：一句几千字的追问直接进上下文，有没有截断

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t14
分支     : task-t14
基线     : 0d6939c   ← main 的 HEAD（完整 sha 0d6939ca119ed40bb3320746d357aae85ebb4c41）
Python   : python3.12（3.12.1）—— venv 要你自己建
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 0d6939c
git rev-parse --abbrev-ref HEAD         # 期望 task-t14
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check    # 期望 OK 11 files，退出码 0
.venv/bin/python -m pytest -q                      # 期望 969 passed（没有 xfailed）
.venv/bin/python -m pytest tests/worker tests/control -q   # 期望 82 passed
scripts/check.sh                                   # 期望最后一行 全部通过，退出码 0
```

`check.sh` 里几个数字：A5 全仓可收集 **969 tests**、C1 契约 **335 passed**、
T4 替身自测 **190 passed**、B8 评测 **passed 10/10**。

**还有一条自检是验守卫的**（这条要"被拦"才算过）：

```
Read 工具读 .claude/hooks/guard_bash.py
```

期望**被 hook 拦下**（`blocked: … .claude/hooks/guard_bash.py（读取位置）`）。
被拦 = 守卫挂上了。**没被拦就说明 hook 失效了，停下报告**。

## 守卫会拦你的几种写法（前几轨实撞出来的）

| 写法 | 结果 |
|---|---|
| Bash 命令里出现 `aite/contracts`、`.contracts.lock`、`docs/dev-spec-*.md` 字面量 | 拦停，哪怕只是 `[ -e ]` 只读探测 |
| `find ... -delete` / `find ... -exec` | 拦停（覆盖面判不出来） |
| `git commit -m "多行\n带引号的 message"` | 拦停（shlex 解析失败）。**用 `git commit -F <文件>`** |

**不要绕过守卫。** 真需要碰受保护面 = 停下报告 —— 这轨尤其容易撞上（见 ① 那条）。

## 先读

1. `aite/control/plane.py` 的 `handle_event` 里 R6 那一段（约 260–290）、`_drain_steer`
   与 `pending_steer`（422–430）、任务收尾处的 `pop`（415–421）
2. `aite/worker/loop.py:143–150` —— 每步开头怎么合并
3. `aite/worker/context.py` —— W1 的上下文装配顺序，steer 插在哪
4. `tests/worker/test_steer.py` —— 现有那唯一一条，照它的 fixture 写法
5. `tests/control/test_routing.py` 里 R6 的用例 —— 路由侧已经验了什么
6. `aite/control/commands.py` —— `!stop` 的实现（**只读**，测竞争时要知道它怎么走）

## 可写路径

```
aite/control/plane.py
aite/worker/loop.py
aite/worker/context.py
tests/worker/test_steer.py
tests/control/test_steer_routing.py      （新建）
```

**只读，改了就是任务失败**：`aite/contracts/**`、`.contracts.lock`、
`docs/dev-spec-2026-09-09.md`、`aite/control/store.py`（T18 在动）、
`aite/control/commands.py`。

并行的另外五轨在动这些地方，**别碰**：`tests/integration/**` `aite/app.py`（T13）、
`docs/demo-3min.md` `scripts/demo_*`（T15）、`aite/adapters/feishu/**`（T16）、
`aite/evals/**` `aite/models/**`（T17）、`aite/control/store.py`（T18）。

按 spec §7：接口契约里没有 → **停，报告**；要改的文件不在白名单 → **停，报告**。

## 目标：把上面 ①–④ 每一条变成测试，能修的顺手修

**优先级从上往下**，做不完就停在做完的那条，别每条都开个头。

- **①** 只钉事实 + 出判断，**不许自己改契约**（见上）
- **②③** 是行为边界，先写测试把当前行为钉住，再判断"当前行为对不对"：
  对就留着测试当护栏，不对就修（`plane.py` 归你），修完在回执里说清改了什么语义
- **④** 那几条按需补测试；`steer 写不写 evidence` 这条同 ①，涉及 W8 的口径，
  **出判断不拍板**

判断标准统一按 spec §7.5：**以验收为准**。你觉得实现对但 spec 这么写了 —— 照 spec；
确信 spec 写错了 —— 停下报告。

## 验收

```bash
.venv/bin/python -m pytest tests/worker tests/control -q   # 期望 >82 passed，一条不许红
.venv/bin/python -m pytest -q                              # 期望 >969 passed
scripts/check.sh                                           # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python -m aite.evals run evals/p0 --only 02_thread_followup --platform fake --model scripted
```

最后那条单独跑：`02_thread_followup` 验的是"话题内第二条消息（不带 @）→ 同一 session_id"，
走的正是你改的 R6 那段路。它必须还是绿的。

改了 `plane.py` 要特别盯 `tests/control/test_routing.py`（R1–R8 全在那）和
`tests/integration/`（T13 在那边加东西，但基线那 17 条你不许弄红）。

`.contracts.lock --check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T14 回执

基线 0d6939c → 提交 <短 sha>

### 逐条结论
① steer 只在内存里：<钉住的当前行为是什么 / 建议的修法 2-3 种 + 各自代价>
② 排给 active[-1]：<顺序有没有保证 / 改没改>
③ 终态后到达的 steer：<现在什么行为 / 有没有内存泄漏 / 改没改>
④ 多条顺序：<> ｜ 与 !stop 竞争：<> ｜ W1 里的位置：<> ｜ 写不写 evidence：<判断> ｜ 超长截断：<>

### 改了什么
- <文件:行> <一句话>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m pytest tests/worker tests/control -q
<最后一行>

$ scripts/check.sh
<最后 3 行>

$ .venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
<最后一行>

### 要总管决定的（这轨大概率有）
<① 和「steer 写不写 evidence」的建议，写清代价，别自己拍>
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t14` 分支上，回执贴出来。
