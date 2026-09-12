# 任务 T24 — 事件入口的失败面：断网重推、存储抖动、追问的证据

## 背景：这轨是从哪来的

T13–T18 六轨全部合进 main（最新 `6ee30d4`）。全仓 **1061 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过，契约锁 `OK 11 files`。P0 的代码面齐了，剩 §2.4 的 M1–M6
卡在飞书凭证。

M1–M6 里有一条，代码面**只被验了一半**：

> M2 | 拔网线 30 秒再插回 | 服务自动重连；**断网期间群里发的 @ 在重连后被处理且只处理一次**

被验过的是前半句和 adapter 那一层：`tests/adapters/feishu/test_feishu_reconnect.py`
钉了退避序列 `[1,2,4,8,16,30,30]`、`feishu.reconnected` 的 INFO 日志、以及
「adapter 不去重、重推的重复事件交给 ControlPlane 按 `event_id` 认」。

**后半句在进程级一条都没有。** 去重本身有 `10_duplicate_event` 验，但那条验的是
「同一个 `event_id` **顺序**投两次」，而且走的是评测的 in-process 装配，不走 `run_app`。
真机上重连那一刻的形状是**一批**事件一起回来：几条不同的 @、夹着几条重复的、
可能还有断网前已经处理过的。这个形状没人造过。

顺带还有两件同一层的事（都是上一轮查出来、没人认领的）：

- **T18 发现**：`Ingress.on_event`（`aite/ingress/handler.py:46`）把异常兜住、计
  `ingress.errors`、打日志，`run_forever` 那条命脉不死 —— 这一半是对的。但异常如果发生在
  `seen_event`（去重那一步），此刻**连 task 都还没建**，于是没有 task 可标 `failed`、
  也没有帖可回，**事件被静默丢掉**。§3.3 最后一行要求「任何未捕获异常 → task `failed` +
  回帖 + evidence `failed`」，这条路上一件都做不到。磁盘抖一下 = 群里一句话石沉大海。
- **T14 的建议**：R6 排队的追问（steer）现在**不写 evidence**。`evidence_show` 的时间线上
  就缺「任务跑到一半用户改了要求」这一段 —— 而这恰恰是解释「为什么最后交的是季度图
  而不是月度图」的关键一环。`model_call` 只记 `messages_hash`，原文不落盘，从证据里
  反推不出来。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t24
分支     : task-t24
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
cd /Users/shensikai/Documents/Aite/.worktrees/task-t24
git log --oneline -1                            # 期望 6ee30d4 ...
git status --short                              # 期望空
.venv/bin/python -m pytest -q                   # 期望 1061 passed
.venv/bin/python -m pytest tests/control -q     # 期望 59 passed
.venv/bin/python -m pytest tests/integration -q # 期望 36 passed
scripts/check.sh                                # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
```

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
被拦才说明 hook 挂上了；没被拦说明 `CLAUDE_PROJECT_DIR` 不对，所有守卫都在静默失效，
停下报告。

**关于 `tests/sandbox` 假红（先看这条，能省你十分钟）**：六轨同时跑测试时
`tests/sandbox/test_docker_sandbox.py` 会红 1–3 条，每次红的用例还不一样。根因是
`docker ps -a --filter label=aite.task` 是**全机器**的命名空间（这一轮 T23 会长时间开着
真容器，串扰只会更明显）。**判据**：红的只在 `tests/sandbox/`、失败断言形如
`labelled_ids() == []` 或 `reap_idle(...) == []`、单跑 `pytest tests/sandbox -q` 是
**36 passed** → 串扰不是回归，等安静了重跑。

## 可写路径（白名单，之外一律只读）

```
aite/ingress/**                                    ← handler.py
aite/control/plane.py                              ← ③ 要动这里
tests/control/test_ingress_failures.py             ← 新建
tests/control/test_steer_evidence.py               ← 新建
tests/integration/test_t24_reconnect_replay.py     ← 新建
```

**邻居**：`tests/integration/` 下的 `conftest.py` / `integration_fakes.py` 归 **T22**
（它在接起飞收尾）—— **你要的测试替身写在自己那个文件里，别去改共用的那两份**，
重复远比冲突便宜（§3.4 的老规矩）。`tests/integration/test_t18_crash_recovery.py` 归 **T21**；
`aite/control/store.py` **只读**（T18 刚验过它的并发面，别动）；
`aite/evidence/**` 归 **T21**（它在修残行自愈，你只调用不修改）。

**plane.py 上刚落了 T14 的改动**（`_owned` / `_steer_target` / 孤儿分支 / 队列清理搬到
最外层 `finally`）。动手前先 `git log -1 --stat -- aite/control/plane.py` 看一眼那次改了
什么，**别把它撤回去**。

## 要做什么

### ① M2 的进程级贯通（主戏）

造一个「断线 → 攒了一批 → 重连后一次性重推」的形状，从 `build_app` + `run_app` 走
（参照 `tests/integration/test_t13_cold_start_to_delivery.py` 是怎么驱动一趟飞行的：
它等的是 `AppWorker.in_flight` 空掉，不是 `send_text` 返回 —— 后者返回时 evidence
还没 finalize、沙箱还没还）。

至少要钉住：

- 重推的一批里，**同一个 `event_id` 出现多次 → 只建一个 task、只交付一次**
  （出站消息数、`send_card` 数、task 数都要断言，不能只看 task）。
- 一批里**不同的 `event_id` 一个都不能丢** —— 去重不能误伤。
- 断网**之前**已经处理过的 `event_id` 混在这一批里重推 → 也只能被丢掉
  （`seen_event` 是落库的，跨断线仍然认得）。
- 一批同时到达（不是顺序 await 一条一条来）时，上面三条还成立吗？
  T18 已经证过 `SqliteSessionStore.seen_event` 在并发下 `False` 恰好一次，
  但**从 `Ingress.on_event` 到建 task 这一整段**没人在并发下验过。
- 重连之后，**断网期间在跑的任务**还在正常收尾吗？（M2 说的是「服务自动重连」，
  不是「重启」—— 进程一直活着，任务不该受影响。）

这一轨不要求你去动 adapter（`aite/adapters/**` 只读）：断线重连是 adapter 的事、
且已经验过；你要验的是**它上面那一层**在收到一批重推时的行为。

### ② `seen_event` 抛异常时事件被静默丢掉

先把当前行为**钉成测试**（T18 已经有一条 `test_store_failure_does_not_kill_the_process`
在 `test_t18_crash_recovery.py` 里，那个文件归 T21，你自己写一条在自己的文件里）。

然后回答：**P0 应该怎么办？** 几个方向，代价自己权衡，结论写进回执：

- **认了，但要看得见**：`ingress.errors` 已经在计数了，但没人看那个计数器。
  `!status` 里报一句？起飞日志里报一句？还是干脆什么都不做只留日志？
- **重试一次**：磁盘抖动多半是瞬时的。但 §3.3 第一行要求 `on_event` **1s 内返回**，
  重试要挤在这个预算里，且不能让重连风暴放大成一片重试。
- **回帖**：连 task 都没有，回到哪？`ev.chat_id` 是有的 —— 但一批事件同时失败就是一片
  刷屏，比静默更糟。

**别自己拍板做大改**：这条的正确交付可能就是「钉住 + 一条小改 + 一段说清代价的判断」。
如果你的结论是「P0 阶段认了」，那就把它写清楚，**这也是合格的交付**。

### ③ 给 steer 补一条 evidence

T14 的建议原文：在 `_continue_session` 排 steer 时，给目标任务补一条 `event_received`
（payload 带 `event_id` / `message_id` / `sender_id` / `text`，和 `_start_task` 那条同构）。

**边界**：

- **不许新增 `EvidenceKind`** —— 那是冻结契约（`aite/contracts/evidence.py`）。用现成的
  `event_received`。真做下来发现非新增不可 → **停下报告**。
- W8 明确列的是四类写入点（`model_call` / `tool_call` / `tool_result` / `checklist_op`），
  而 `_start_task` 已经在写 `event_received` 了 —— 你要在回执里论证「多这一个写入点
  算不算越 W8 的界」。
- `event_received` 在时间线上会承载两种语义（建任务的那条 + 中途追问的那条），
  **payload 里要能区分**，不然 `scripts/evidence_show.py`（只读）讲出来的故事是糊的。
- 模型消息全文不进 evidence（W8 后半句），但**用户说的话**进不进？`_start_task` 那条
  已经带了 `text`，所以口径上是一致的 —— 顺着它走，别自己发明。

## 纪律

1. 契约（`aite/contracts/**`）一个字都不许动，锁必须全程 `OK 11 files`。
2. 白名单之外的文件只读。要改别人的面 → **停下报告**，写清建议。
3. **别把 T14 刚落的改动撤回去**（见上面「邻居」那段）。
4. 每条结论挂实测。「应该会」「大概」一句不要。

## 验收

```bash
.venv/bin/python -m pytest tests/control -q       # 期望 >59 passed，一条不许红
.venv/bin/python -m pytest tests/integration -q   # 期望 >36 passed
.venv/bin/python -m pytest -q                     # 期望 ≥1061 passed
scripts/check.sh                                  # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted            # 期望 passed 10/10
.venv/bin/python -m aite.evals run evals/p0 --only 10_duplicate_event --platform fake --model scripted
.venv/bin/python -m aite.evals run evals/p0 --only 02_thread_followup --platform fake --model scripted
```

最后两条单独跑：`10` 验的是同一 `event_id` 投两次只建一个 task（你在 ① 里动的正是这条
路的上游）；`02` 验的是话题内第二条不带 @ → 同一 `session_id`（你在 ③ 里动的
`_continue_session` 就在它路上）。两条都必须还是绿的。

特别盯 `tests/control/test_routing.py`（R1–R8 全在那）和 `tests/control/test_steer_routing.py`
（T14 刚加的 6 条）。契约锁 `--check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T24 回执

基线 6ee30d4 → 提交 <短 sha>

### ① M2 进程级
造的形状：<怎么模拟断线 + 批量重推>
逐条结论：
  同 event_id 重复 → 只建一个 task、只交付一次：<成立 / 不成立，证据>
  不同 event_id 一个不丢：<>
  断网前处理过的混在批里：<>
  并发到达时仍成立：<>
  断网期间在跑的任务照常收尾：<>
发现的真 bug：<没有就写"没有">

### ② seen_event 抛异常
当前行为（钉成测试的那条）：<>
你的判断：<认了 / 重试 / 回帖 / 别的>，代价：<>
改了没有：<改了什么 / 没改，为什么>

### ③ steer 的 evidence
写在哪一步 / 用了什么 kind / payload 长什么样：<>
两种 event_received 怎么区分：<>
算不算越 W8 的界：<你的论证>
evidence_show 的时间线现在讲得清「中途改了要求」吗：<贴一段实际输出>

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

$ .venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
<最后一行>

### M2 现在过得了吗
<拿实测说话：代码面还差什么、哪些只能真机验>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t24` 分支上，回执贴出来。
