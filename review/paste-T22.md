# 任务 T22 — 起飞时把上一条命的残局收干净（M6）

## 背景：这轨是从哪来的

T13–T18 六轨全部合进 main（最新 `6ee30d4`）。全仓 **1061 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过，契约锁 `OK 11 files`。P0 的代码面齐了，剩 §2.4 的 M1–M6
卡在飞书凭证。

T18 去验「`kill -9` 之后重开会怎样」，查出一个真问题并**改了一半**：

> 崩溃时处于 `created` / `planning` / `working` 的任务，重开后**仍是那个状态**。
> `init()` 里只有建表、没有任何收拾逻辑，而 `list_active_tasks` 正认这三个状态（§3.2），
> 于是它**永远挂在群里的 `!status` 上** —— 卡片停在「进行中」，人以为还在跑。

T18 在 `aite/control/store.py:222` 加了 `recover_orphan_tasks()`：查出活跃态残留、标
`failed`、写 `ORPHAN_RESULT_SUMMARY`，**把任务列表交回调用方**。它**故意没有**塞进
`init()`，理由写在那个方法的 docstring 里，值得你先读一遍：

- `init()` 的契约是「建表，幂等」（§3.2），塞状态变更是扩契约；而且会误伤「同进程第二个
  store 实例」这个合法场景（B6 就是那个形状）。
- §3.3 要求失败要「task `failed` + 回帖 + evidence `failed` 事件」，**后两件 store 做不了**。
  自动改状态而没人回帖，用户以为还在跑 —— 比挂着更糟。

**所以现在 `recover_orphan_tasks()` 合进来了，但全仓没有一个生产调用点。**
接线在 `aite/app.py`，那是这一轨的面。

顺带还有一条 T18 在探针里踩到两次、但没改的：**异常路径下没 `close()` 的 store 会让进程
想退退不出去** —— aiosqlite 的连接跑在非 daemon 线程上，`threading._shutdown` join 它会
永久阻塞。跟「进程不退出」正相反，是**想退退不出去**，Ctrl-C 都救不回来。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t22
分支     : task-t22
基线     : 6ee30d4   ← main 的 HEAD（完整 sha 6ee30d4e6013782ff7cdd4e69ec8efa101f4b2db）
Python   : python3.12（3.12.1）—— venv 要你自己建
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-t22
git log --oneline -1                            # 期望 6ee30d4 ...
git status --short                              # 期望空
.venv/bin/python -m pytest -q                   # 期望 1061 passed
.venv/bin/python -m pytest tests/integration -q # 期望 36 passed
scripts/check.sh                                # 期望 全部通过，退出码 0
```

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
被拦才说明 hook 挂上了；没被拦说明 `CLAUDE_PROJECT_DIR` 不对，所有守卫都在静默失效，
停下报告。

**关于 `tests/sandbox` 假红（先看这条，能省你十分钟）**：六轨同时跑测试时
`tests/sandbox/test_docker_sandbox.py` 会红 1–3 条，每次红的用例还不一样。根因是
`docker ps -a --filter label=aite.task` 是**全机器**的命名空间。**判据**：红的只在
`tests/sandbox/`、失败断言形如 `labelled_ids() == []` 或 `reap_idle(...) == []`、
单跑 `pytest tests/sandbox -q` 是 **36 passed** → 串扰不是回归，等别轨安静了重跑。

## 可写路径（白名单，之外一律只读）

```
aite/app.py                                    ← 主战场
tests/integration/test_t22_startup_recovery.py ← 新建
tests/integration/conftest.py                  ← 可加 fixture，别动现有的
tests/integration/integration_fakes.py         ← 可加能力，别改现有断言面
```

**邻居**：`tests/integration/test_t18_crash_recovery.py` 归 **T21**（它在修 evidence 残行）；
`aite/control/store.py` 只读（`recover_orphan_tasks` 已经在那儿了，别改它）；
`aite/control/plane.py` 归 **T24**；`aite/evidence/**` 归 **T21**。

## 要做什么

### ① 把 `recover_orphan_tasks()` 接进起飞流程

现在的 `run_app`（`aite/app.py:183`）是这样开头的：

```python
    # 建表。事件在 `platform.start` 返回的下一刻就可能到，表得先在（§3.2 init 幂等）。
    await app.store.init()
    _log_takeoff(app)

    runner: asyncio.Task | None = None
    try:
        await app.platform.start(app.ingress.on_event)
        ...
```

要收拾的每个孤儿任务需要三件事（§3.3 的口径）：**状态 `failed`**（store 已经做了）、
**回帖**、**evidence `failed` 事件**。

几个必须自己判的点：

- **放在哪一步？** T18 建议 `store.init()` 之后、`platform.start()` 之前。但**回帖要用
  platform** —— 飞书那边发消息走的是 HTTP API 客户端而不是长连接，所以可能在 `start()`
  之前就能发；也可能不能。**去把这件事看清楚再定**（`aite/adapters/feishu/platform.py`），
  别猜。放在 `start()` 之后的代价是：这中间新事件已经能进来了，孤儿还没收拾完，
  `!status` 会短暂地还列着它 —— 你要权衡。
- **回帖回到哪里？** 任务的 `anchor`（`Session.anchor`）。孤儿任务的 session 还在库里，
  但 `recover_orphan_tasks()` 只还给你 `Task` —— session 要自己去取。
  取不到（数据不全）时怎么办，也要有个说法：**不能因为一个收不掉的孤儿让整个进程起不来。**
- **回帖说什么？** store 里那句 `ORPHAN_RESULT_SUMMARY` 是「进程重启前该任务仍在执行，
  已终止。请重新发起。」—— 群里那句用不用同一句、要不要带任务号（`#A3`），你定。
- **卡片怎么办？** 任务卡片停在「进行中」。W4 说「任务结束时必定再调一次 `update_card`，
  status 置 delivered/failed/cancelled」。孤儿的 `card_id` 在 `Task` 上，能不能顺手把卡片
  置成 failed？能的话就置 —— 群里那张绿不了的卡才是人最先看见的东西。
- **evidence `failed` 怎么写？** `EvidenceWriter.append(task_id, EvidenceKind.failed, ...)`
  之后要不要 `finalize()`？（我倾向要 —— 没有 manifest 的证据目录在 `evidence_show`
  时间线上是残的。但注意 **T21 正在修 evidence 的残行自愈**，崩溃留下的证据文件很可能
  正好是残的：`append` 在基线上会抛。你这轨**不要**去修那个，但要保证
  **一个孤儿的 evidence 写不进去，不能让别的孤儿收不掉、更不能让进程起不来**。）

### ② 修「想退退不出去」

`run_app` 里 `await app.store.init()` 和 `_log_takeoff(app)` 都在 `try` **外面**。
init 成功之后、进 try 之前的任何异常，都会让那条 aiosqlite 连接留着 —— 进程 join
非 daemon 线程时永久阻塞。①你新加的收拾逻辑正好也落在这个窗口里，等于把这个窗口撑大了，
所以这条得一起修。

修法不难（把 init 也纳入 finally 的覆盖范围），但**要有测试证明它真的能退**：
造一个「init 之后立刻抛」的场景，断言 store 被 close 了。

### ③ 顺手更新一处过时的注释

`aite/app.py:86` 那段 docstring 写着「§3.2 的 `SessionStore` 没有『列出全部活跃任务』
的口子（`list_active_tasks` 要 chat_id）」。T18 加的 `recover_orphan_tasks()` 正好填了
这个口（虽然它同时还改状态）。如果你接线之后那句话不准确了，顺手刷一下。**只刷注释，
别顺手重构 `AppWorker`。**

## 纪律

1. 契约（`aite/contracts/**`）一个字都不许动，锁必须全程 `OK 11 files`。
   C-TΩ-1 冻结的三个签名（`build_app` / `run_app` / `main`）也不许改，
   `tests/integration/test_t8_build_app_contract.py` 那 4 条钉着它们。
2. **`build_app` 的「只组装、不产生副作用」不许破**（C-TΩ-1 硬约束 1，同一份测试钉着）：
   收拾孤儿是 `run_app` 的事，别塞进 `build_app`。
3. 停机顺序是写死的（platform → plane.join → sandbox.aclose → store.close），
   `test_t8_graceful_shutdown.py` 3 条 + `test_t8_shutdown_grace_timeout.py` 3 条钉着，
   别动。
4. 白名单之外的文件只读。要改别人的面 → **停下报告**。

## 验收

```bash
.venv/bin/python -m pytest tests/integration -q   # 期望 >36 passed，一条不许红
.venv/bin/python -m pytest -q                     # 期望 ≥1061 passed
scripts/check.sh                                  # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python -m aite.app --help                # 退出码 0
```

特别盯 `tests/integration/test_t8_*.py`（基线 17 条：契约签名、停机顺序、宽限期、
落盘、跨进程）和 `test_t13_cold_start_to_delivery.py`（8 条，冷启动到交付全程走
`build_app` + `run_app`）—— 你动的正是它们脚下那段路。
契约锁 `--check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T22 回执

基线 6ee30d4 → 提交 <短 sha>

### 接线位置
放在哪一步：<start() 之前 / 之后>，为什么
platform 在那一刻能不能发消息：<看代码看出来的结论 + 依据行号>

### 每个孤儿做了哪几件事
状态：<store 已做>
回帖：<回到哪 / 原文 / 取不到 session 时怎么办>
卡片：<置了没有 / 为什么>
evidence：<写了什么 kind / finalize 了没有 / 写不进去时怎么办>

### 一个孤儿失败会不会拖垮起飞
<给出实现依据 + 测试>

### 「想退退不出去」那条
<修法 / 测试怎么证明进程真的能退>

### 改了什么
- <文件:行> <一句话>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m pytest tests/integration -q
<最后一行>

$ scripts/check.sh
<最后 3 行>

### M6 现在过得了吗
<「杀进程重启后在旧线程追问仍能续接」这条，你的判断 + 依据。
 注意 T14 已经修掉了「孤儿吞掉追问」那一半，你补的是「孤儿被收干净」那一半>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t22` 分支上，回执贴出来。
