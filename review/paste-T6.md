# 任务 T6 — 07_commands 场景重做（让任务卡得住，命令才有得可停）

## 背景：这轨是从哪来的

T1–T4 四轨已经合进 main。合并后第一次把四轨真串起来跑 §3.8 的 10 个场景，
结果是 **passed 8/10**，红的是 `02_thread_followup` 和 `07_commands`。

两条都不是产品缺陷，是**评测这一层的时序假设不成立**。02 归 T5，你这轨修 07。

### 07 现在为什么红

场景要验的是 §3.3 的 `!status` / `!stop`：起一个停不下来的任务，`!status` 列出它，
`!stop #A1` 之后任务 cancelled、沙箱被 release、卡片终态是 cancelled。

场景注释里写着这么一句假设：

> 所以第一个任务先 run_python 拿到沙箱，然后死循环 checklist_note（repeat: inf），
> 配合 max_steps: 200 保证它在命令到达前不会自己结束

**这个假设不成立。** 脚本化模型是瞬时返回的，200 步在毫秒级就跑完了。实测两个方向都不对：

| 事件间隔 | 结果 | 为什么 |
|---|---|---|
| 0ms（runner 现在的样子） | `model_calls=0`，卡片一张没发 | 三个事件在同一个 tick 里处理完，worker 协程还没被调度，任务就已经 cancelled 了 |
| ≥20ms | `model_calls=200`，跑满步数上限 | 模型瞬时返回，命令到达时任务早跑完了 |

失败的两条断言（`sandbox.release >= 1`、`卡片终态 cancelled`）本质都要求
**任务真的活着跑过**。所以要改的是「怎么让任务在命令到达时还活着」。

⚠️ 别去改 `aite/control/**` 或 `aite/worker/**` 来迁就场景 —— 那两轨的行为是对的。
另外 `sandbox.release` 那条**还有一半已经修好了**：main 上的 `a6cf260` 修了
「Gateway 建的沙箱没人还」的跨轨缺口，你的基线已经带着它。任务只要真跑起来过，
沙箱就会被 release。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t6
分支     : task-t6
基线     : a6cf260   ← main 的 HEAD，T5/T6 钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。文档正文里的 `python` 字样照原样保留，
只有本机执行时才换 `.venv/bin/python`。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 a6cf260
git rev-parse --abbrev-ref HEAD         # 期望 task-t6
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check   # 期望 OK 11 files，退出码 0
.venv/bin/ruff check .                            # 期望 All checks passed!，退出码 0
.venv/bin/python -m pytest -q --co                # 期望 838 tests collected
.venv/bin/python -m pytest tests/contracts -q     # 期望 335 passed
.venv/bin/python -m pytest -q -m "not docker"     # 期望 802 passed, 36 deselected，退出码 0
.venv/bin/python -m pytest tests/e2e -q           # 期望 134 passed
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
                                        # 期望 末行 passed 8/10，退出码 1
```

上面这些是我在同基线的 worktree 里实跑出来的值，不是估计。

**第 12 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 读到内容说明 PreToolUse 守卫没生效（多半是会话不是从
worktree 根启动的），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。
这种情况停下报告，别接着做。

## 先读

- `docs/dev-spec-2026-09-09.md` 的 §3.3（`!stop` 那一行）、§3.5 R5、§3.8 的 07 那行。**不许改**。
- `evals/p0/07_commands.yaml` —— 目标场景。
- `aite/testing/fake_model.py`（`FakeModel` / `ScriptStep`）—— 你的主战场。
- `aite/evals/scenario.py` 的 `EventSpec`、`aite/evals/runner.py` 的投递循环 —— 看懂就行，
  改动规矩见下面的冻结契约。

## 可写路径

```
aite/testing/fake_model.py     evals/p0/07_commands.yaml
tests/e2e/**
aite/evals/**   ← 只限冻结契约 C-T5T6-1 那部分，见下
```

其余只读。`aite/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` 有守卫拦着。
**特别注意**：`aite/control/**`、`aite/worker/**`、`aite/sandbox/**`、`aite/gateway/**`
不归你，别动。`evals/p0/` 下除 07 之外的 9 个 yaml 也别动（02 归 T5，其余 8 个现在
是绿的，你的改动必须保证它们**继续绿且行为不变**）。

## 冻结契约 C-T5T6-1：事件投递时序

⚠️ **这一节在 T5 与 T6 的派单里逐字一致。字段由 T5 实现，两轨共用。**

`EventSpec`（`aite/evals/scenario.py`）的两个字段，语义如下：

| 字段 | 取值 | 含义 |
|---|---|---|
| `after` | `"none"` | 不等，紧接上一条投。**默认值** |
| | `"idle"` | 等系统静默：在跑的任务都收了、待处理队列空了，再投 |
| | `"running"` | 等上一条事件起的那个任务真的被 worker 领走、开始跑了，再投 |
| `after_timeout_sec` | float，默认 `5.0` | 上面那个等待的上限 |

三条硬约束：

1. **默认值必须是 `"none"`，且 `"none"` 的行为与今天逐字节一致。** 现在绿的 8 个场景
   一个 yaml 都不许改，跑出来的 stats 也不许变。
2. **等不到就失败，不许继续投。** 超过 `after_timeout_sec` 还没等到，场景以
   `phase="dispatch"` 失败收场，`reason` 里写清等的是什么、等了多久。
   「等不到就接着投」测出来的绿是假的。
3. **`"running"` 判的是「worker 真的领走了」**，不是「store 里有一行 task」。
   任务建好但还躺在队列里不算 running —— 07 就是栽在这个区别上。

判据在 `aite/evals/wiring.py` 里实现（那里已经有 `settle()` 和 `Deps.activity`，
`"idle"` 的判据复用它那一套，别另起一套）。

### 你要怎么用它

T5 和你是**并行**的，你开工时他还没交。所以：**你自己也把这套字段实现一遍**，
照上面的表写，别改名、别改默认值。你的 07 需要 `after: running`。

### 并轨规程（合并时怎么缝）

`aite/evals/{scenario,runner,wiring}.py` 这三个文件 **T5 和 T6 都会改**，
合并时**以 T5 的实现为准**（他是这个契约的 owner）。你那份同名实现会被丢掉——
这是有意的，不是浪费：它让你这一轨自己就能跑绿，不用等 T5。

所以有两件事你必须做到，否则并轨时你的 07 会红：

- **契约字段名、取值、默认值、失败姿态照上表写死。** 你自己实现的那份和 T5 的那份
  行为必须一致，不然并轨后你的 yaml 对不上他的实现。
- **你新增的测试不要依赖你自己那份实现的内部细节**（函数名、私有属性、异常类型）。
  只测「yaml 里写了 `after: running`，事件就等到任务跑起来才投」这种从外面看得见的行为。
  依赖内部细节的测试并轨后必红。

## 目标

1. 让 `FakeModel` 能**卡住**一步：某一步的 `chat()` 挂起，直到有人取消这个任务
   （或者到达一个上限）。字段名你定，写进 `ScriptStep`，加注释说清为什么要有它。
   ⚠️ 别用真实时间 `sleep` 去糊——评测得是确定性的，跑多少次结果都一样。
   `08_step_limit` 用的 `repeat: inf` 那套是「无限出牌」，你要的是「这一步不返回」，
   两回事，别混。
2. 按 C-T5T6-1 实现 `after` / `after_timeout_sec`（见上面「你要怎么用它」）。
3. 重做 `evals/p0/07_commands.yaml`，让它验的东西不变、但不再依赖「200 步跑不完」
   这个不成立的假设：
   - e1 起的任务要在 `!status` / `!stop` 到达时**确实还活着**（沙箱已经建了、卡片已经发了）
   - e2/e3 用 `after: running` 保证投递时机
   - 把原来那段「配合 max_steps: 200 保证它不会自己结束」的注释改掉——它是错的，
     把新的机制和理由写进去，下一个人读到的得是真的
4. `tests/e2e/` 补回归测试，钉住「模型能卡住一步」这个能力本身（不依赖 07 场景）。

## 验收

```bash
.venv/bin/ruff check .                          # All checks passed!
.venv/bin/python -m aite.contracts.lock --check # OK 11 files
.venv/bin/python -m pytest -q -m "not docker"   # 802 + 你新增的条数，一条不许红
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
```

评测那条的期望：**07 转绿**，02 仍然红（那是 T5 的活，你别去动它），
其余 8 个保持绿 —— 也就是末行 **passed 9/10**。

07 转绿要求这两条断言真的过，别为了绿把它们删了或改松：

```
- {check: sandbox_calls, method: release, min: 1}
- {check: cards, final_status: cancelled}
```

⚠️ 如果你发现自己的改动让 02 顺带绿了，**停下来在回执里说清楚**，别把 02 的 yaml
也改了 —— 那样并轨时会和 T5 撞车。

## 回执格式

做完在最后贴一段，格式照这个：

```
RECEIPT T6 status=done commit=<短 sha> checks=<过了几项>/<共几项> files=<改了几个文件>
```

另外用人话写清楚：

- 你给 `FakeModel` 加的「卡住一步」是什么机制，为什么它是确定性的
- 07 现在靠什么保证任务在命令到达时还活着（一句话）
- 8 个原本绿的场景 stats 有没有变
- 契约 C-T5T6-1 你实现的那份，与派单表格有没有任何偏离 —— 有就逐条列出来，
  这直接决定并轨后你的 07 还绿不绿
- 你新增的测试里，有没有哪条依赖了你自己那份 evals 实现的内部细节（并轨后会被丢掉）

**不要 push，不要合 main。** 我这边统一并轨。
