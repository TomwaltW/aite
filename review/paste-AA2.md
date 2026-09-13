# 任务 AA2 — `!restart` 漏掉 `Answering` 的任务：它防的那件事它自己没防住

## 背景：这轨是从哪来的

`!status` / `!stop` / 卡片 stop 按钮在 V5 与 W2 两轮里被统一到了同一份列表 `status_tasks`。
**`cmd_restart` 没跟上** —— 它还在走 `list_active_tasks`（`core/crates/control/src/plane.rs:642`）。

两份列表差在哪，`status_tasks` 自己的文档注释（`:509-528`）写得很清楚：

```
库里那一半是 list_active_tasks（口径 status in ACTIVE_TASK_STATUSES = created/planning/working）
另一半是控制面自己的 running —— worker 的 deliver() 在「第一步就 final、一张卡片都没发过」
那一路把状态落成 Answering，而 Answering 不在活跃口径里
（roundtrip.rs 专门有一条断言钉着 !Answering.is_active()）
```

**于是 `!restart` 身上有一处自相矛盾。** `cmd_restart` 那段注释（`:639-641`）逐字写着：

> 归档的会话不该留着还在跑的任务：**它们的结果会落进一个已经不存在的会话里**，
> 而且**会继续出现在 `!status` 里**。§3.3 的 cancel 路径正好是它们该有的收尾。

而它取任务用的是 `list_active_tasks` —— 一个处在 `Answering` 的任务（正在逐个发产物、
send_text、写 delivered 证据、收卡片）**恰好两条都中**：既不会被停，结果落进已归档的会话；
又因为 `!status` 走的是 `status_tasks`（含 `running`）而**继续出现在 `!status` 里**。
注释点名要防的两件事，一件都没防住。

**W2 记这笔账时的判断是「按 ⑦ 的结论这不是缺陷（交付中本来就停不掉），但口径与 `!stop`
已经分家，值得……」。这句话你要自己判一遍，别照抄。** 「交付中停不掉」说的是 `!stop` 那一侧
对 `Delivering` 的处置（回一句「正在交付，停不了」），而 `Answering` 是不是同一回事、
`cancel_task` 打在一个 `Answering` 的任务上到底会发生什么 —— **这是本轨的第 ① 件事，
而且是先决条件**：不先判定，后面改什么都是猜的。

## 必读（按顺序）

1. **`core/crates/control/src/plane.rs`** —— `status_tasks`（`:509` 起，含那段最要紧的文档注释）、
   `cmd_stop` / `resolve_stop_target`（`:703` 起，`!stop` 的三态：NotFound / Delivering / Stoppable）、
   `cmd_restart`（`:632` 起，本轨要改的那一处）、`cancel_task`。
2. `core/crates/contracts/src/session.rs:33-42` —— `ACTIVE_TASK_STATUSES` 三值与 `is_active()`。
   **冻结面，只读。** 三个值被 `frozen_values.rs` 与 `roundtrip.rs` 逐值钉着，**不动它**。
3. `core/crates/worker/src/agent.rs` 里 `let answering = !ctx.card.sent()` —— `Answering`
   是怎么落上去的、落上去之后还会走哪几笔。
4. `review/review-findings-2026-09-12-vmerge.md` 的 W2 回执（搜「`cmd_restart`」）—— 这笔账的出处，
   连同它的 ⑦ 结论。
5. `docs/acceptance-M.md` §0.4 / §M1 排障表 / §7 —— `!status` 与 `!stop` 口径统一那段变更日志。
   你改完 `!restart`，**这里多半要跟着加一句**。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-aa2
分支     : task-aa2
基线     : __BASE__
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-aa2
git log --oneline -1        # 期望 __BASE_SHORT__
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（总管 __TODAY__ 在合并后的 main 上实跑，原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=852 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok`（aiteerr/config/feishu/ingress/sandbox/server） |
| B8 评测 | `passed 10/10` |

> ⚠️ **如果你看到 `cargo passed=852 failed=1`、唯一红点是 `-p aite --test guard`** ——
> 那说明 `.claude/settings.json` 的修正那一格没进 git，**停下来喊人**，别当回归也别自己修。

> ⚠️ 三个抖动 target（`graceful_shutdown` / `reconnect_replay` / `startup_recovery`）的病根都
> 治过了，Z1 / Z2 / Z3 三轮开场收尾各跑一次都没撞到。撞到是新信息，贴进回执
> （判据：单独跑一遍那个 test target，绿就是假红）。

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明守卫在静默失效，
> **停下报告**。

## 要干的活

### ① 先判定：`cancel_task` 打在一个 `Answering` 的任务上会发生什么

**这一条不许靠读代码下结论，要有可跑的证据。** 写一条测试（或临时 harness）把任务推到
`Answering`，然后走 `cancel_task`，观察：

- 状态最后落成什么；
- 那几笔在途动作（逐个发产物、send_text、写 delivered 证据、收卡片）停没停、停在哪一笔；
- 证据链完不完整（`finalize` 有没有被调、链校验过不过）；
- 用户在群里看到的是什么。

**三种可能的结局，各自对应不同的改法**：

- **停得掉、且干净** → ② 直接把 `cmd_restart` 换成 `status_tasks`；
- **停不掉（像 `Delivering` 那样）** → ② 不是「换个列表」，而是要像 `!stop` 那样**分情况回话**，
  让用户知道有任务没停掉、而不是默默漏掉；
- **停得掉但留下半截**（证据链断、卡片卡住） → 那是比本轨更大的问题，**停下报告**，
  别在这一轨里顺手修。

**把这一条的结论写在最前面**，②③ 都建立在它上面。

### ② 按 ① 的结论改 `cmd_restart`

不管走哪条路，有三件事是共同的：

- `plane.rs:642` 那一处的口径要么统一到 `status_tasks`，要么**在注释里写明为什么不统一**
  （那段注释现在说的两件事必须与代码真实行为对得上 —— 现在它们不对得上，这是本轨的起点）；
- `stopped` 计数与回帖文案（「终止了 N 个进行中的任务。」）要跟着对：
  漏算的任务不该被算进去，停不掉的任务也不该被算进去却说停了；
- **`!restart` 与 `!stop` 之间的差别，改完之后必须是能一句话说清的**。
  两条命令对「什么算活跃」意见不一致这件事本身就是这笔账的由来，别修出第三种口径。

### ③ 回归：把这条行为钉住

至少两条：

- **口径那条** —— 一个处在 `Answering` 的任务，`!restart` 之后不再落进已归档的会话
  （或者按 ① 的结论，用户明确被告知）；
- **别把它改成恒真断言** —— 再来一条相反方向的：该被停的照常被停、不该被碰的没被碰。
  Z2 那条 `the_hook_command_recovers_outside_the_repo_instead_of_bricking_the_session` 是
  这种双向断言的范本，可以参考它的写法。

**每条新测试都做一次变异验证**：把你的改动摘掉，测试必须红；贴出红的那一行。

### ④ 顺手两件（都是纯注释，别扩大）

- **`core/crates/edge-client/src/lib.rs` 的 `contract_state()` 零调用方** ——
  产品代码里一个调用方都没有（`grep` 过：只有 `tests/contract_gate.rs` 的 9 处 + `link.rs`
  那个 `pub(crate)`）。X1 记账时说「要不要删不是本轨的决定」。**现在由你判**：
  删掉、还是留着并在文档注释里写明「它是给测试当观测口的，产品代码不走这条路」。
  删的话注意 `contract_gate.rs` 那 9 处要跟着改 —— 那是一整条测试路径，**代价可能比留着大，
  量一量再动手**。
- **`core/crates/app/src/app.rs:152` 的文档注释** —— 「`SqliteSessionStore::open`：打开连接，
  **库文件不在就建一个空的**（里面还没有表）」。这句是对的，但它没说「文件**在**而**不是库**」
  会怎样，而那正是 Z1 补的第 ① 组第四件事。补一句，口径以 `core/crates/app/src/preflight.rs`
  模块头那张表为准（**那里是唯一已经写对四件事的地方**）。

## 纪律

1. **可写面**：`core/crates/control/**`、`core/crates/edge-client/src/lib.rs`（只 ④ 那一处）、
   `core/crates/app/src/app.rs`（只 ④ 那一处注释）、`core/crates/control/tests/**`、
   `docs/acceptance-M.md`、`review/review-findings-2026-09-12-vmerge.md`（只许**追加**你自己那一节）。
2. **只读面**：`core/crates/worker/**`、`core/crates/app/src/**`（除 ④ 点名那一处）、`edge/**`。
   ① 的观测如果逼得你改 worker，改成测试侧的 harness，**别动产品代码**。
3. **冻结面**：`proto/**`、`core/crates/contracts/**`（含 `ACTIVE_TASK_STATUSES` 三值与那两条
   冻结测试）、`docs/dev-spec-2026-09-11-rustgo.md`。守卫会拦，别试。
   **本轨不需要动契约** —— `status_tasks` 那段注释自己说了：「补它也不需要动契约」。
4. **`.claude/**` 一个字节都不碰**（读也不行，守卫拦）。
5. `cargo fmt --all`（写模式）会被守卫拦（它碰冻结面）。对你改过的文件逐个跑
   `rustfmt --edition 2024 <file>`，然后 `git status` 复核没动到别的文件。
6. **落盘无残留**：测试建的临时库 / `data/` / `config/aite.yaml` 收尾前清干净并复核。

## 验收

```bash
scripts/check.sh                        # 全部通过，退出码 0
cd core && cargo clippy --workspace --all-targets -- -D warnings
```

**测试数会涨**（③ 至少 +2）。收尾时 `cargo passed=` 那个数与开场的差额，
**要能被你新加的测试条数逐条解释**，写进回执。

## 回执

写进 `review/review-findings-2026-09-12-vmerge.md`，**追加**一节「十五、AA2 回执 —— __TODAY__」
（**只追加，别动别人的节**）。要有：

- 基线与开场自检（五行关键值 + 守卫拦截那一条）；
- **① 的判定与它的证据**（最要紧的一节：命令、逐字输出、你据此选了三条路里的哪一条）；
- ② 改了什么、为什么；`!restart` 与 `!stop` 的差别用**一句话**说清；
- ③ 每条新测试的变异验证（摘掉改动之后红的那一行，逐字）；
- ④ 两处的处置与理由（`contract_state()` 删还是留，量出来的代价是多少）；
- 测试数差额的逐条解释；
- **记账转出去的**（表格：位置 / 病 / 归哪轨）；
- **没做的 / 拿不准的**（编号列表）。
