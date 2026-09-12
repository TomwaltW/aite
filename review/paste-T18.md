# 任务 T18 — SQLite：并发写与崩溃恢复

## 背景：这轨是从哪来的

T1–T12 十二轨全部合进 main（最新 `0d6939c`）。全仓 **969 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过。P0 的代码面齐了，剩 §2.4 的 M1–M6 卡在飞书凭证。

spec §1 把持久化钉得很死：**P0 = Docker 沙箱 + 进程内队列 + SQLite**，
没有 PostgreSQL、没有 Redis。整个系统的状态就落在 `aite/control/store.py` 和
`data/aite.db` 这一个文件上。而 M6 明写：

> `systemctl restart` / 杀进程重启后在旧线程追问 —— 仍能续接

今天验这件事的一共两条半：`tests/integration/test_t8_sqlite_cross_process.py`（**2 条**：
换实例续接、去重存活）、`tests/control/test_persistence.py`（6 条，B6 那条在里面）。

这两条验的都是**顺序场景**：写完、关掉、重开、读到。没有任何一条验过：

### ① 并发写

§3.2 给 `SessionStore` 定了两条带并发含义的约定，都没人在并发下验过：

- `append_turn`：「seq 由调用方分配，**重复 (session_id, seq) 报错**」——
  两个协程同时给同一会话 append 同一个 seq，真的报错吗？还是静默覆盖/写重？
- `next_task_no`：「**原子递增** + encode_task_no」—— 并发调用会不会发出两个一样的 `#A3`？
  `task_no` 是人在群里 `!stop #A3` 时用的，撞号意味着停错任务。

还有 `seen_event`（去重键）：「首次调用记录并返回 False，之后 True」。
同一个 `event_id` 被两个协程同时问，会不会两个都拿到 False → 建两个 task？
R2 和场景 `10_duplicate_event` 靠的就是它，但那条测的是**顺序**投两次。

P0 是单 worker，但**事件投递不是单路的**：`on_event` 要在 1s 内返回（§3.3 第一条），
平台重连后可能一次重推一批，`handle_event` 之间没有串行化保证。

### ② 崩溃恢复（不是优雅停机）

`tests/integration/test_t8_graceful_shutdown.py` 验的是**优雅**停机 —— platform 先停、
任务收尾、store 最后关。M6 说的是 `systemctl restart` / **杀进程**，也就是
`kill -9`：没有收尾、没有 `store.close()`、写到一半的事务、可能还有半行 JSON 落在
`events.jsonl` 上。

重开之后要能续接。今天没人验过这条路。具体没验的：

- 任务在 `working` 状态时进程被杀 → 重开后这个任务什么状态？
  它会永远挂在 `!status` 上吗（`list_active_tasks` 认 `created/planning/working`）？
  有没有"启动时把上一条命的僵尸任务收拾掉"这一步？
- `events.jsonl` 最后一行写了一半 → `FileEvidenceWriter.verify()` 怎么办、
  下一次 `append` 会不会接在残行后面把整条链搞坏
- SQLite 的 journal 模式是什么（WAL 还是默认 delete）？崩溃后有没有 `-wal` / `-journal`
  残留文件、重开时能不能自动恢复
- `data/aite.db` 被删掉 / 权限不对 / 磁盘满 → §3.3 最后一行要求"任何未捕获异常 →
  task failed + 回帖 + evidence failed；**进程不退出**"，这条在存储层失败时成立吗

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t18
分支     : task-t18
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
git rev-parse --abbrev-ref HEAD         # 期望 task-t18
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check    # 期望 OK 11 files，退出码 0
.venv/bin/python -m pytest -q                      # 期望 969 passed（没有 xfailed）
.venv/bin/python -m pytest tests/control -q        # 期望 40 passed
.venv/bin/python -m pytest tests/integration -q    # 期望 17 passed
scripts/check.sh                                   # 期望最后一行 全部通过，退出码 0
```

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

## 先读

1. `aite/control/store.py` 全文 —— 建表、事务边界、`next_task_no` / `seen_event` /
   `append_turn` 三个带并发含义的方法怎么实现的、连接是怎么管的
2. `tests/control/test_persistence.py` —— B6 的写法，你的并发测试照它的 fixture
3. `tests/integration/test_t8_sqlite_cross_process.py` —— 现成的"换实例"手法（**只读**，
   T13 也在 `tests/integration/` 里加东西，你只许新建自己的文件）
4. `aite/evidence/writer.py` —— `events.jsonl` 怎么追加、`verify()` 怎么重算链
5. spec §3.2 的 `SessionStore` 协议注释（那三条约定的原文）、§3.3 最后一行

## 可写路径

```
aite/control/store.py
tests/control/test_store_concurrency.py       （新建）
tests/integration/test_t18_crash_recovery.py  （新建）
```

**只读，改了就是任务失败**：`aite/contracts/**`、`.contracts.lock`、
`docs/dev-spec-2026-09-09.md`、`aite/control/plane.py`（T14 在动）、
`aite/control/commands.py`、`tests/integration/integration_fakes.py` 与
`tests/integration/conftest.py`（**T13 独占**，你要辅助就写在自己的新文件里）。

并行的另外五轨在动这些地方，**别碰**：`aite/app.py` 与 `tests/integration/` 下已有文件（T13）、
`aite/worker/**`（T14）、`scripts/demo_*` `docs/demo-3min.md`（T15）、
`aite/adapters/feishu/**`（T16）、`aite/evals/**` `aite/models/**`（T17）。

按 spec §7：接口契约里没有 → **停，报告**；要改的文件不在白名单 → **停，报告**；
要加新依赖 → **停，报告**（`aiosqlite` 已经在依赖里，够用）。

## 目标一：并发写（`tests/control/test_store_concurrency.py`）

用 `asyncio.gather` 把这三条约定各压一遍：

1. **`append_turn` 同 seq**：并发写同一 `(session_id, seq)` → 必须有一方报错（§3.2 原文
   "重复 (session_id, seq) 报错"），不许静默写重。顺带验：并发写**不同** seq 时一条不丢
2. **`next_task_no` 原子性**：并发要 N 个号 → 必须拿到 N 个**互不相同**的号，
   且形状都合 `encode_task_no`（`#A1` `#A2` …）。撞号就是 bug
3. **`seen_event` 去重**：同一 `event_id` 并发问 N 次 → **只有一次**返回 False

发现不满足的，`store.py` 归你，直接修（事务、`INSERT OR ABORT`、唯一索引、
`BEGIN IMMEDIATE` 之类都在你手上）。修完在回执里写清"原来什么行为、为什么不对、怎么改的"。

## 目标二：崩溃恢复（`tests/integration/test_t18_crash_recovery.py`）

**别真 `kill -9`**（测试里不好控）。用"不走收尾路径"来模拟崩溃：拿到 app 之后直接丢掉
引用、不调 `stop()` / 不 `await` 停机序列，然后在同一个 db 文件上重开一个 app。
现成的 `test_t8_sqlite_cross_process.py` 里"换一套组装"的手法可以借（但别改那个文件）。

要钉的：

- **僵尸任务**：崩溃时处于 `working` 的任务，重开后是什么状态、`!status` 会不会
  永远列着它。**这条大概率会暴露一个真问题** —— 如果启动时没人收拾它，M6 之后
  群里会一直挂着一个跑不动的任务。发现了就在回执里写清，能修就修
  （收拾逻辑该放哪要判断：`store.init()` 里？还是 `app.py` 起飞时？
  **`app.py` 是 T13 的，你别碰** —— 那就写在 `store.py` 能覆盖的范围内，
  或者只钉住问题、把修法建议写进回执让总管定序）
- **半行 evidence**：手工往 `events.jsonl` 末尾写半行 JSON，然后
  `verify()` 该报 False（现成测试验过完整篡改，没验过"残行"）；
  再 `append` 一条，链会不会被这半行搞坏
- **journal 模式**：把实际的 `PRAGMA journal_mode` 查出来写进回执（是 WAL 还是 delete），
  崩溃后有没有残留 `-wal`/`-journal`，重开能不能自动恢复
- **存储层失败不炸进程**：db 文件被设为只读 / 目录被删 → §3.3 要求"进程不退出"。
  这条如果不成立，写清现象（能不能修取决于要不要碰 `app.py`，同上）

## 验收

```bash
.venv/bin/python -m pytest tests/control -q        # 期望 >40 passed，一条不许红
.venv/bin/python -m pytest tests/integration -q    # 期望 >17 passed
.venv/bin/python -m pytest -q                      # 期望 >969 passed
scripts/check.sh                                   # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python -m aite.evals run evals/p0 --only 10_duplicate_event --platform fake --model scripted
```

最后那条单独跑：`10_duplicate_event` 验的是同一 `event_id` 投两次只建一个 task，
走的正是你可能改的 `seen_event`。它必须还是绿的。

改了 `store.py` 要特别盯 `tests/control/test_persistence.py`（B6）和
`tests/integration/test_t8_sqlite_cross_process.py`（基线那 2 条不许弄红）。

`.contracts.lock --check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T18 回执

基线 0d6939c → 提交 <短 sha>

### 并发三条
1 append_turn 同 seq：<报错了 / 静默写重，改了什么>
2 next_task_no 原子性：<N 个号有没有撞，改了什么>
3 seen_event 并发去重：<只有一次 False 吗，改了什么>

### 崩溃恢复
僵尸任务：<重开后什么状态 / !status 会不会一直列着 / 修法建议>
半行 evidence：<verify 报什么 / append 会不会坏链>
journal 模式：<实际是什么 / 残留文件 / 能否自动恢复>
存储层失败不炸进程：<成立 / 不成立，现象>

### 改了什么
- <文件:行> <一句话>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m pytest tests/control -q
<最后一行>

$ .venv/bin/python -m pytest tests/integration -q
<最后一行>

$ scripts/check.sh
<最后 3 行>

### 发现的真 bug（这轨的价值在这一栏）
<没有就写"没有"。每条：现象、复现方式、改没改、为什么>

### 要总管决定的
<涉及 aite/app.py 的修法建议写这里，别自己碰那个文件>
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t18` 分支上，回执贴出来。
