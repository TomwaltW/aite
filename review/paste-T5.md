# 任务 T5 — 评测的事件投递时序（修 02_thread_followup）

## 背景：这轨是从哪来的

T1–T4 四轨已经合进 main。合并后第一次把四轨真串起来跑 §3.8 的 10 个场景，
结果是 **passed 8/10**，红的是 `02_thread_followup` 和 `07_commands`。

查下来两条都不是产品缺陷，是**评测 runner 没有时序模型**：
`aite/evals/runner.py` 的投递循环把所有事件零间隔连着投，事件之间不等系统消化。
真实平台上两条消息之间必然隔着人打字的时间，评测里却是同一个事件循环 tick。

你这轨修 02 并**建立时序机制**；07 归 T6，用你建的同一套机制。

### 02 现在为什么红

场景是「顶层 @ 起一个任务 → 同话题里不带 @ 的追问续接同一会话、新建 task」，
期望 `tasks == 2`、`send_text == 2`。实际 `tasks == 1`。

根因（已实测确认，不用重查）：e2 投到时 e1 的任务还在跑，控制面按 §3.5 R6 把它
当成 **steer（追加指令）** 合进当前任务，不建新 task —— `plane.counters` 里能看到
`events.steer == 1`。这是 T2 对 R6 的正确实现：任务跑着的时候用户又说一句，是在
补充要求，不是要开第二个任务。

把事件间隔放到 5ms 以上，这个场景就 `tasks == 2` 全绿。所以**要改的是 runner 怎么投，
不是控制面怎么判**。

⚠️ 别去改 `aite/control/plane.py` 的 R6 语义来迁就场景 —— 那会把 T2 正确的行为改坏。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t5
分支     : task-t5
基线     : a6cf260   ← main 的 HEAD，T5/T6 钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。文档正文里的 `python` 字样照原样保留，
只有本机执行时才换 `.venv/bin/python`。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 a6cf260
git rev-parse --abbrev-ref HEAD         # 期望 task-t5
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

上面这些是我在这个 worktree 里实跑出来的值，不是估计。

**第 12 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 读到内容说明 PreToolUse 守卫没生效（多半是会话不是从
worktree 根启动的），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。
这种情况停下报告，别接着做。

## 先读

- `docs/dev-spec-2026-09-09.md` 的 §3.5 R5/R6/R7、§3.8 的场景表。**不许改**。
- `aite/evals/runner.py`、`aite/evals/wiring.py`、`aite/evals/scenario.py` —— 你要动的三个文件。
- `evals/p0/02_thread_followup.yaml` —— 目标场景，注释里已经写清了它想验什么。

## 可写路径

```
aite/evals/**        evals/p0/02_thread_followup.yaml
tests/e2e/**
```

其余只读。`aite/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` 有守卫拦着。
**特别注意**：`aite/control/**`、`aite/worker/**`、`aite/testing/fake_model.py` 不归你，
别动 —— 02 的修法完全在 evals 这一层。

`evals/p0/` 下除 02 之外的 9 个 yaml 也别动（07 归 T6，其余 8 个现在是绿的，
你的改动必须保证它们**继续绿且行为不变**）。

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

### 并轨规程（合并时怎么缝）

`aite/evals/{scenario,runner,wiring}.py` 这三个文件 **T5 和 T6 都会改**。
合并时**以 T5 的实现为准** —— 你是这个契约的 owner。T6 那边只保留他自己的
`evals/p0/07_commands.yaml`、`aite/testing/fake_model.py` 和 `tests/e2e/` 里他新增的测试。
所以：**契约字段名、取值、默认值、失败姿态照上表写死，别自己改名**，
不然 T6 的 07 场景 yaml 会对不上你的实现。

## 目标

1. 按 C-T5T6-1 实现 `after` / `after_timeout_sec`。
2. `evals/p0/02_thread_followup.yaml` 里给 e2 标上合适的 `after`，让 02 转绿。
   e2 要的是「e1 那个任务已经收了」——`after: idle`。
3. `tests/e2e/` 补回归测试，至少钉住这几条：
   - `after: none` 是默认，且行为与改动前一致（拿一个现有场景做对照）
   - `after: idle` 真的等到了静默才投（构造一个不等就会被并成 steer 的用例）
   - `after: running` 真的等到 worker 领走才投
   - 等不到 → `phase="dispatch"` 失败，且 `reason` 说得清
4. 顺手把 runner 的一处小毛病修了（可选，但推荐）：`--platform fake --model scripted`
   的 stdout 现在是「JSON 摘要 + 末行 `passed k/10`」，末行让整段 stdout 不是合法 JSON，
   下游想解析得先 `sed '$d'`。如果你能在不破坏 `scripts/check.sh`（它 `tail -1` 取那一行）
   和 `tests/e2e/test_t4_evals_runner.py::test_cli_run_ends_with_passed_k_of_10` 的前提下
   把摘要写 stdout、把 `passed k/10` 写 stderr，就一起改了；做不到就写进回执，别硬改。

## 验收

```bash
.venv/bin/ruff check .                          # All checks passed!
.venv/bin/python -m aite.contracts.lock --check # OK 11 files
.venv/bin/python -m pytest -q -m "not docker"   # 802 + 你新增的条数，一条不许红
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
```

评测那条的期望：**02 转绿**，07 仍然红（那是 T6 的活，你别去动它），
其余 8 个保持绿 —— 也就是末行 **passed 9/10**。

⚠️ 如果你发现自己的改动让 07 顺带绿了，**停下来在回执里说清楚**，别把 07 的 yaml
也改了 —— 那样并轨时会和 T6 撞车。

## 回执格式

做完在最后贴一段，格式照这个：

```
RECEIPT T5 status=done commit=<短 sha> checks=<过了几项>/<共几项> files=<改了几个文件>
```

另外用人话写清楚：

- 02 现在为什么绿了（一句话说清 `after: idle` 在这个场景里等的是什么）
- 8 个原本绿的场景 stats 有没有变（有变就是你破坏了 `after: none` 的默认行为）
- 契约 C-T5T6-1 你有没有原样落地；有任何偏离，逐条列出来 —— T6 的场景依赖它
- 第 4 条那个 stdout/stderr 的小毛病，做了还是没做，为什么

**不要 push，不要合 main。** 我这边统一并轨。
