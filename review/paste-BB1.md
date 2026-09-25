# 任务 BB1 — `!status` 看得见、`!stop` 停不着；丢掉的事件一声不吭

## 背景：这轨是从哪来的

控制面有三条账，都挂了不止一轮：

**① `!stop` 省略任务号 + 本群有多个任务 → 回「没有这个任务」。**
`plane.rs:783`：

```rust
let found = if raw.is_empty() {
    (tasks.len() == 1).then(|| tasks.into_iter().next().expect("刚判过长度"))
} else { … };
```

多于一个就 `None` → `StopTarget::of(None)` → `StopTarget::NotFound` →
`plane.rs:641` 回 `NO_SUCH_TASK_TEXT`（"没有这个任务"）。
用户刚在 `!status` 里看见三个任务，敲一句 `!stop` 被告知「没有这个任务」。

W2 记过这条，AA2 回执「记账转出去的」里确认**还没销**
（原话：「回『没有这个任务』，而列表里明明有好几个。W2 记过，本轨没碰」）。

注意 V5→W2 已经把**找目标的那份列表**统一到 `status_tasks` 了（`plane.rs:775` 那段注释
讲的就是这件事），所以这轨**不是**再做一遍统一 —— 剩下的是「找不着之后说什么」。
`plane.rs:769` 那段注释还写着「改之前两条命令口径一致（都查不到）」，
而省略任务号这一格恰恰是它没覆盖到的。

**② 乱序重推的话题追问会被丢弃**（`README.md` §已知边界第二条）。
追问被平台重推在它的 root 之前时，到达那一刻话题会话还不存在、它自己又没 @，
于是命中 R8「其余丢弃」，用户那边**零回复**。飞书的重推通常保序，所以这是
「乱序时才炸」而不是常态。README 给的方向是「事件级的重排或缓冲，属于路由规则本身要改」。

**③ 丢弃的事件不留痕，core 侧 counters 没有查看入口**
（`docs/acceptance-M.md` §8 第 3 条）。
`handle_event` 出错时只 `self.shared.bump("events.dropped")`
（`plane.rs:1271`，另一处在 `:1378`），**INFO 级别没有任何日志**；
`counters()` 在产品代码里零调用方，只有 `dropped_note()` 把 `events.dropped`
一个数塞进 `!status` 的尾巴。M4 判「没投递 vs 投递了被丢」只能去开放平台翻推送记录。

> edge 侧这一条**已经不成立**（退出时打一行 `edge.counters`，见 `acceptance-M.md` §7 末）。
> **缺的只是 core 侧，而且只缺「跑着的时候查不到」这一半。**

## 必读（按顺序）

1. `core/crates/control/src/plane.rs` —— 至少这几段全文：
   `StopTarget`（`:75-110`）、`status_tasks`（`:558`）、`cmd_status`（`:614`）、
   `cmd_stop`（`:639`）、`resolve_stop_target` / `resolve_task`（`:775-810`）、
   `dropped_note`（`:754`）、`handle_event`（`:1265-1275`）、路由 R1–R8 全段。
   **那些注释是规格的一部分**，改行为之前先读懂它们为什么这么写。
2. `docs/acceptance-M.md` §8 第 3 条（观测缺口原文）、§7「日志关键字速查」
   —— 新日志的 target / 字段命名照那一份的既有风格，别自创一套。
3. `README.md` §「已知边界」第二条（乱序重推那条的完整描述）。
4. `review/review-findings-2026-09-12-vmerge.md` 第十五节（AA2 回执）——
   尤其「记账转出去的」和「没做的 / 拿不准的」，里面讲清楚了
   `cancel_task` 在 `Answering` 上那句注释为什么前提不成立。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-bb1
分支     : task-bb1
基线     : 7019c48（`docs(acceptance): W1 那张表的表头归属分成两批`）
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-bb1
git log --oneline -1        # 期望 7019c48
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（2026-09-15 在**这个 worktree 里**实跑的原样抄录，不是转述）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=864 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok`（aiteerr / config / feishu / ingress / sandbox / server） |
| B8 评测 | `passed 10/10` |

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来**，
> 报的是「触碰受保护面 …（读取位置）」。**没被拦说明守卫在静默失效，停下报告。**
> 这是本轨唯一能验证「hook 真的挂上了」的办法（2026-09-12 真踩过：hook 执行失败
> 是非阻塞放行且不报警，整场守卫静默失效，没有任何人知道）。

## 要干的活

### ① `!stop` 找不着目标时，别再说「没有这个任务」

三种「找不着」现在共用一句话，而它们是三件不同的事：

| 情形 | 现在 | 该说什么 |
|---|---|---|
| 本群一个活跃任务都没有 | `cmd_status` 走 `NO_ACTIVE_TASK_TEXT`，`cmd_stop` 走 `NO_SUCH_TASK_TEXT` | 两条命令该一致 |
| 省略任务号，本群**有多个** | 「没有这个任务」 | 点名要任务号，**并把可选的列出来** |
| 带了任务号，但对不上 | 「没有这个任务」 | 保持 —— 这句在这一格是对的 |

**做法**：`resolve_stop_target` 现在返回 `StopTarget`，而 `StopTarget::NotFound`
把上面三格压成了一格。加一格（形如 `Ambiguous(Vec<Task>)`）把「多个候选」
从 `NotFound` 里分出来，文案走 `wording.rs` 的既有路子（那里的文案是逐字钉着的，
照 `#A17`/`#A18` 那批的写法加）。

**边界，别越**：
- `ACTIVE_TASK_STATUSES` 是**双重冻结**的（`contracts` 的 `frozen_values.rs` +
  `roundtrip.rs`），一个字都不许动，这条路本来也走不通。
- `status_tasks` 的口径不动 —— ① 只改「找不着之后说什么」。
- 卡片那条路（`resolve_task`，按 `task_id` / `card_id` 找）**不受影响**：
  它没有「省略」这个形状。别顺手给它也加。

### ② 丢弃留痕：每一条被丢的事件都要在日志里留下名字

现在两处 `bump("events.dropped")`（`:1271` 与 `:1378`）都没有日志。

- `:1271`（`handle_event` 的错误分支）：那段注释说「日志由 Ingress 打（那里有完整的
  event_id / kind），这里不打第二遍」—— **先去核实这句话是不是真的**，
  `grep` `ingress.handle_failed` 看它打了什么字段。真打全了就别加第二条，
  改成在注释里说清「要查被丢的事件去看哪个 target」；**没打全**才补。
- `:1378`（`cancel_save_failed` 之后那一笔）：那里已经有 `tracing::error!`，
  但它数的是同一个 `events.dropped` 计数器 —— **两件不同的事共用一个计数器**
  是不是对的，给个判断并写进回执。
- **R1/R2/R8 三条路由规则自己丢弃事件时**（去重命中、非群消息、其余丢弃），
  现在连计数器都没有。观测缺口原文说的就是这一批。按 `acceptance-M.md` §7
  的命名风格补 INFO 日志 + 计数器，**每条规则一个名字**，别合并成一个
  `events.dropped` 大杂烩 —— M4 要分的就是「没投递」和「投递了被丢」。

### ③ core 侧 counters 要有一个查得到的出口

edge 的做法是退出时打一行 `edge.counters`（`acceptance-M.md` §7 末）。
core 侧至少要对齐到这个程度。**优先选不动契约的做法**：

- 收尾时打一行 `aite.counters`（改 `core/crates/app/src/run.rs` 的优雅退出段）——
  **这条在本轨可写面里，是底线交付**；
- `!status` 尾巴现在只漏 `events.dropped` 一个数（`dropped_note`）。要不要让它
  多说几个，是产品决定 —— **给建议，别擅自扩**：`!status` 的文案在
  `wording.rs` 里逐字钉着，扩它要连带改一批测试。

> ⚠️ `core/crates/app/src/run.rs` 与 `preflight.rs` 分属两轨：**BB6 动 `preflight.rs`，
> 你动 `run.rs`**，两边不碰对方的文件。

### ④ 乱序重推（R8）—— 先判断，再决定做不做

README 说要补「得靠事件级的重排或缓冲，属于路由规则本身要改」。
**这条的判断权在你，但要先拿出判断依据**：

1. 复现它：造一条话题追问（有 `thread_id`、没 @、root 的会话还不存在），
   走一遍路由，确认它确实落到 R8 且用户侧零回复。**这一步必须真跑**，
   不是读代码推出来的。
2. 然后回答一个问题：**缓冲多久、缓冲多少条、root 一直不来怎么办？**
   答不上来就别做 —— 一个没有上界的缓冲区是新的病，不是药。
3. 判断「做」的话，做最小的那一版并钉死边界；判断「不做」也**完全合格**，
   把 ①②③ 交干净，然后把复现记录 + 三个问题的答案写进回执，
   下一轮就有依据了。

**不许**为了绕开这条去改 R5 的投递条件（「已 @ 机器人」或「已在话题内」）——
那条是路由规则的冻结面，动它会让「群里随便一句话都进来」。

## 纪律

1. **可写面**：`core/crates/control/**`、`core/crates/app/src/run.rs`、
   `core/crates/app/tests/` 下**你新建的**测试文件、
   `review/review-findings-2026-09-12-vmerge.md`（只许**追加**你自己那一节）。
   文档面只许改 `README.md` §已知边界与 `docs/acceptance-M.md` §8 里**与你改动直接对应**的那几句。
2. **只读面**：其余一切。特别是 `core/crates/contracts/**`、`proto/**`
   （守卫的保护面，读得了写不了）、`core/crates/app/src/preflight.rs`（BB6 的面）、
   `core/crates/app/tests/guard.rs`（BB4 的面）、`core/crates/evidence/**`（BB2 的面）。
3. **改了行为就要有回归，而且每条都要做变异验证**：把药摘掉，看测试是不是真的变红。
   只断「该拦的拦」是恒真断言 —— 双向断言，范本见 `tests/signals.rs`（Y1 加的那两层）。
4. `cargo fmt --all` 会被守卫拦（它会碰冻结面），改用
   `rustfmt --edition 2024 <改过的文件>` 逐个跑。
5. **落盘无残留**：临时探针、复现脚本收尾前清干净，`git status --short` 复核。

## 验收

```bash
scripts/check.sh                                       # 全部通过，退出码 0
cd core && cargo test -p aite-control                  # 全绿
cd core && cargo clippy --workspace --all-targets -- -D warnings
```

- `cargo passed=` 的涨幅要能被你新加的测试**逐条**解释；
- `contracts passed=25 failed=0` 与 `OK 25 files` **必须逐字不变** ——
  变了说明你碰到契约了，停下报告；
- `passed 10/10` 不许掉。

> ⚠️ **假红提醒**：`cargo test --workspace` 偶发 `passed=863 failed=1`，
> 失败名多半是 `-p aite --test reconnect_replay`（台账第四节 4.1 记过它的病根：
> `reconnect_replay.rs:168` 那个守卫恒为 false，而它想防的竞态是真会发生的）。
> **单独连跑五遍**：全绿就是假红，记一笔继续；稳定复现就是你踩到了新东西，停下报告。

## 回执

追加进 `review/review-findings-2026-09-12-vmerge.md`，标题
「十八、BB1 回执 —— 2026-09-15」（**只追加，别动别人的节**）。要有：

- 基线与开场自检（五行关键值 + 守卫拦截那一条的**逐字原话**）；
- ① 三格文案的改前/改后对照，每格都要有**真跑出来**的输出，不是设计稿；
- ② 对「日志由 Ingress 打第二遍」那句话的核实结果（贴 grep 到的字段），
  以及「两件事共用一个计数器」的判断；
- ③ `aite.counters` 那一行的真实样子（贴一次收尾日志）；
- ④ 乱序重推的复现记录 + 三个问题的答案 + 做/不做的判断与理由；
- 每条新回归的**变异验证**（摘掉药 → 哪条测试变红 → 逐字贴失败输出）；
- **记账转出去的**（表格：位置 / 病 / 归哪轨）；
- **没做的 / 拿不准的**（编号列表，包括你觉得该做但越界了的）。
