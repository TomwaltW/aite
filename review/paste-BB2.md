# 任务 BB2 — 证据链能证明的事，比它看起来能证明的少

## 背景：这轨是从哪来的

`docs/acceptance-M.md` §8 记了七条观测缺口，其中**五条是证据面的**。
写剧本的人当时明写「代码改动不属于本轨，现在只记录，留给下一轮」——
这一轮就是那个下一轮。

### ① `created_at` 不进 hash 链（最要紧的一条）

```rust
// core/crates/evidence/src/writer.rs:314
hash: chain_hash(&prev_hash, &p_hash),
created_at: now_micros(),
```

`payload_hash` 只覆盖 payload。**把 `events.jsonl` 里所有时间戳改掉，
`aite evidence verify` 照样全绿。** 而 M2（断网重连、只处理一次）
和 M3（卡片更新次数）的时序判断，全建立在这些时间戳上。

一条证明不了自己时序的证据链，在「只处理一次」这个问题上是说不了话的。

### ② 卡片的发送与更新，证据里一个字都没有

M3 明确要求「卡片至少更新 3 次且不新增消息」，而 `send_card`
（`core/crates/worker/src/card.rs:125`）/ `update_card`（`:173`）在证据里没有任何痕迹，
**也没有日志** —— 只能肉眼数群里的卡片。
§8 给的方向是「加 `card_sent` / `card_updated` 两类事件（或复用 `checklist_op`）」。

### ③ 三处「记了等于没记」

| 位置 | 病 | §8 原文 |
|---|---|---|
| `checklist_op` 的 check/fail | 只记 `id` + `state`，不带那一项的文本 | 「单看一条 `checklist_op` 是读不懂的」 |
| `tool_result` | 只有 `content_hash`，没有摘要 | 「工具失败时证据里只有 `error` 的错误码，拿不到那一行具体的报错文本。排 M3 的沙箱问题时这一点最疼」 |
| 沙箱 id | 不进证据 | 「任务和容器对不上号，M3 查『哪个容器该收没收』只能靠时间先后猜」 |

> `aite evidence show` 已经靠回放前面的 `add` 事件把 checklist 文本补了回来 ——
> 所以这一格的病不是「查不到」，是「**每个消费方都得自己回放**」。
> 改不改是权衡，见 ③ 的任务描述。

## ⚠️ 这一轨的硬约束：你要改的东西有一半写不了

`proto/**` 和 `core/crates/contracts/**` 都在守卫的 `PROT_PREFIXES` 里
（**读得了，写不了**）。新增事件类型、改 payload 形状、动 `payload_hash_of`
——**全都落在写不了的那一半**。

**交付形态照 AA4 那一轨**：出**补丁脚本** + 在临时副本上跑完整验证 + 给总管一条命令链。
形状照 `review/aa4-proto-patch.py`（那份是 25 个锚点、`--check` / `--root` 自验矩阵、
锚点不唯一命中就整份拒写、落盘后读回复验）。

**还有一条先后关系，必须遵守**：

> `review/aa4-proto-patch.py` **至今没跑**（`git log -- proto/` 最近一条还是
> `c9d96d2`，R0，09-11）。它改的是 `proto/aite/v1/edge.proto` 的注释。
> **你的补丁必须能在「AA4 已打」和「AA4 未打」两种树上都给出确定结果** ——
> 要么锚点避开 AA4 碰过的那几行，要么显式检测并拒绝执行并说清「先跑 AA4 那条链」。
> 两种做法都行，**选哪种、为什么、怎么验的，要写进回执**。

## 必读（按顺序）

1. `core/crates/evidence/src/writer.rs` 全文 —— 尤其 `:76` 那段关于
   `created_at` 精度的注释（Python 的 6 位小数是兼容约束）、`:217` 与 `:406` 两处校验。
2. `core/crates/evidence/src/cli.rs` 的 `verify` 段（`:697-845`）——
   **两处校验口径必须同时改，改一处就是埋雷**。
3. `core/crates/contracts/src/` 里 `payload_hash_of` / `chain_hash` / `EvidenceEvent`
   / `EvidenceKind` 的定义（**只读**）。
4. `core/crates/worker/src/card.rs` 全文 —— W4 的合并规则
   （「同一任务 500ms 内多次变更只调一次 `update_card`，任务结束时必定再调一次」）
   决定了 `card_updated` 事件该在哪一层发。
5. `review/aa4-proto-patch.py` —— **补丁脚本的骨架，照它来**。
6. `review/review-findings-2026-09-12-vmerge.md` 第十七节（AA4 回执）②
   「codegen 可复现性验证」——**改 proto 之后怎么证明产物是干净的，那一节给了完整方法**。
7. `docs/dev-spec-2026-09-11-rustgo.md` §3.1「数据形状（与 Python p0.1 逐字段对应）」
   —— **冻结面，读它是为了知道边界在哪**。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-bb2
分支     : task-bb2
基线     : 7019c48
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin；protoc 走 brew（本机 libprotoc 36.1）
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-bb2
git log --oneline -1        # 期望 7019c48
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0
```

**关键行期望**（2026-09-15 在 `task-bb1` 这个 worktree 里实跑的原样抄录）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=864 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
> 没被拦说明守卫在静默失效，**停下报告**。

## 要干的活

### ① `created_at` 进 hash 链 —— 先回答向后兼容，再动手

这条改的是**证据链的定义**，不是加个字段。动手前先把这三个问题答了（写进回执）：

1. **已经落盘的旧证据怎么办？** 改完之后 `verify` 会不会把所有旧目录判成红？
   （`data/evidence/` 不入库，但总管本机有实跑留下的目录，M1–M6 还没跑完。）
2. **兼容策略选哪个**：链里加一个版本位 / 只对新事件生效 / 一刀切并接受旧目录失效？
   **给一个推荐 + 理由**，别把三个选项摆出来让人选。
3. **`writer.rs:76` 那段精度注释**（Python 的 `datetime` 只有微秒，
   旧证据目录一律 6 位小数）在新口径下还成不成立？
   时间戳进了 hash，**它的规范化形式就成了契约的一部分** —— 两台机器算出不同的
   字符串就是两条不同的链。`canonical_json` 现在怎么处理它，核实清楚。

**`verify` 的两处实现（`writer.rs:406` 与 `cli.rs:834`）必须同时改，
并且要有一条测试专门钉「两处口径一致」** —— 现在没有这样的测试，这就是为什么它们能漂。

**这条的成功判据是一个可复现的攻击**：拿一份真证据目录，改掉里面一个
`created_at`，改前 `verify` 绿、改后 `verify` 红，两次输出都逐字贴进回执。
**改前那次绿，是这条病存在的唯一证明** —— 先跑它，再动代码。

### ② 卡片进证据

`card.rs` 的 `send_card` 至多一次、`update_card` 按 W4 合并 —— **证据要记的是
「真调出去了几次」，不是「worker 想更新几次」**。这两个数不一样（合并把后者压小了），
而 M3 要数的是前者。想清楚事件发在哪一层。

新增事件类型要动 `EvidenceKind`（契约面）→ 走补丁脚本。
**复用 `checklist_op` 那条路 §8 也提了** —— 如果你判断复用更好（不动枚举、
不动契约锁），**那是更优解，选它并说清为什么**。

### ③ 三处「记了等于没记」，逐条判断做不做

- **`checklist_op` 带文本**：代价是每条 check/fail 都要带一份文本副本，
  证据文件会变大（checklist 项的文本不短）。而 `aite evidence show` 已经能回放补上。
  **给判断**：是改格式，还是把「消费方必须回放」写成明文约定 + 在
  `acceptance-M.md` 里写清楚怎么读？后者不动契约。
- **`tool_result` 带摘要**：这条 §8 说「排 M3 的沙箱问题时最疼」，
  收益最明确。注意摘要**不能带进 `content_hash`** 的计算，否则改摘要就改哈希；
  也要考虑截断长度与脱敏（`Redactor` 认不认识它）。
- **`sandbox_id` 进证据**：`Task` 上本来就有 `sandbox_id`
  （`plane.rs:1384` 那段用它 release），进证据基本是顺手。**优先做这条。**

三条都要在回执里给「做了 / 没做 + 理由」，**没做也算交付**，理由说清楚就行。

### ④ 补丁脚本 + 在副本上验证

照 AA4 的方法：`git archive HEAD` 出一份副本 → 打补丁 → 重跑 codegen →
**diff 里不许有非注释行之外的意外变化** → 跑全量。

**你验不到的那一步要如实说**：重锁（`AITE_RELOCK=1`）守卫故意拦 agent 自我授权，
**不许绕**（连在临时副本里也不许 —— 把 `export AITE_RELOCK=1` 藏进脚本文件
躲开文本扫描，那是钻空子）。所以「补丁落地 + 重锁之后 check.sh 全绿」这句话
你只能验到重锁之前，**回执里就这么写**，别装作验过了。

## 纪律

1. **可写面**：`core/crates/evidence/**`、`core/crates/worker/**`、
   `core/crates/app/tests/` 下**你新建的**测试文件、
   `review/bb2-*.py`（补丁脚本）、台账里**只追加**你自己那一节。
   文档面只许改 `docs/acceptance-M.md` §8 里**与你改动直接对应**的那几条。
2. **只读面**：其余一切。特别是 `core/crates/control/**`（BB1 的面）、
   `core/crates/app/src/{run,preflight}.rs`（BB1 / BB6 的面）、
   `core/crates/app/tests/guard.rs`（BB4 的面）、`edge/**`（BB5 的面）。
3. **`proto/**` 与 `core/crates/contracts/**` 写不了** —— 出补丁脚本，
   没有 `AITE_RELOCK=1` 就当场拒绝执行（照 AA4 那份）。
4. **`.contracts.lock` 读写都被拦**，别去碰它，重锁是人的事。
5. `cargo fmt --all` 会被守卫拦，改用 `rustfmt --edition 2024 <改过的文件>`。
6. **落盘无残留**：临时副本可以留（写清路径给总管复核），worktree 里
   `git status --short` 必须只有你的改动。

## 验收

```bash
scripts/check.sh                                   # 全部通过，退出码 0
cd core && cargo test -p aite-evidence             # 全绿
cd core && cargo clippy --workspace --all-targets -- -D warnings
```

- **没打补丁的树上，`OK 25 files` 与 `contracts passed=25 failed=0` 必须逐字不变**；
- `cargo passed=` 的涨幅逐条解释；
- `passed 10/10` 不许掉。

补丁脚本**不许在本轨跑**。交付形态：脚本 + `--check` / `--root <临时目录>`
自验矩阵的逐字输出 + 给总管的命令链（每步带期望输出，照 AA4 第十七节 ④）。

## 回执

追加进台账，标题「十九、BB2 回执 —— 2026-09-15」。要有：

- 基线与开场自检（五行 + 守卫拦截逐字原话）；
- **① 的那个攻击复现**（改 `created_at` → 改前绿改后红，两次输出逐字）；
- ① 三个兼容问题的答案 + 选了哪条路 + 为什么；
- 「两处 verify 口径一致」那条新测试的变异验证；
- ② 事件发在哪一层的判断 + 新枚举 vs 复用 `checklist_op` 的取舍；
- ③ 三条逐条「做了 / 没做 + 理由」；
- ④ 补丁脚本的自验矩阵 + **与 AA4 补丁的先后关系怎么处理的、怎么验的**；
- **记账转出去的**（表格：位置 / 病 / 归哪轨）；
- **没做的 / 拿不准的** —— 尤其「重锁之后」那一段你验不到，明写。
