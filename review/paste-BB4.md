# 任务 BB4 — 守卫有一个真漏拦，AA3 量准了没治

## 背景：这轨是从哪来的

AA3 拿 183 条 payload 把守卫（`.claude/hooks/guard_bash.py`）的行为边界黑盒量了一遍，
把结果落成 8 条新回归（10 → 18 条）。**但它明确没出补丁**，理由写在回执 ③：

> 本轨量到三条够格的候选……要替换的都是**判据代码**而不是字符串字面量，
> 黑盒给不出它的样子。**一份锚点靠猜的补丁比不写更坏。**

AA3 同时把「真要改需要什么」写清楚了，并点名：

> **这条建议单开一轨**，别顺手做。

这一轨就是那个单开的轨。**和 AA3 最大的不同：你可以读源码。**

## ⚠️ 读得了，但仍然写不了 —— 这一轨的形状

`.claude/**` 在守卫的 `PROT_PATHS` 里（`readable=False`，**读和写都拦**）。
所以：

- **Read / `cat` 守卫脚本会被当场拦下。** 这是设计如此。
- **`AITE_RELOCK=1` 不许自己用** —— 守卫故意拦 agent 自我授权
  （`relock_and_self_authorization_are_blocked` 钉着这条）。
- **交付形态是补丁脚本**：`review/bb4-guard-patch.py`，形状照
  `review/z2-guard-patch.py` 与 `review/v6-guard-patch.py`
  （逐条精确替换、锚点必须唯一命中否则整份拒写、语法合法性检查、
  落盘后读回复验、没有 `AITE_RELOCK=1` 当场拒绝执行）。

**那怎么「可以读源码」？** —— 由**总管**读给你，或者由总管跑一次
`--check` 把锚点命中情况反馈给你。派单流程是：

1. 你先把**判据量到极致**（AA3 的 183 条是起点，不是终点），
   把每条候选的「改成什么」写成**不依赖源码形状的规格**；
2. 出补丁脚本，锚点用**多候选 + 全部必须唯一命中**的写法
   （z2 那份认两个锚点，就是为了应对「应急版也得换掉」）；
3. **`--check` 模式必须能在不修改任何东西的前提下报出「锚点命中/未命中」** ——
   这是给总管的第一条命令，也是你和源码之间唯一的合法信道。

**下面这条 AA3 写过，本轨原样继承**：

> 技术上存在一条绕法，**本轨明令禁止**：编译后的测试代码用 `std::fs` 读文件
> 不受守卫管辖（守卫只管工具调用），所以一条临时测试可以把守卫源码整个 `println!` 出来。
> **不许这么用。** 守卫拦读的意图就是「agent 不要照着源码去改守卫」。
> 取**单个**必须知道的锚点字符串是可以的（Z2 那样，用完就删、回执里写明），
> **dump 源码不行。边界在这儿。**

## 要治的三条（AA3 已量准，逐条抄录判据）

### ① 真漏拦：`cd` 之后完全绕过保护面 —— **本轨的主交付**

```
cd core && echo x > crates/contracts/src/lib.rs     → 放行（真能改到契约）
Write(crates/contracts/src/lib.rs)                  → 放行
```

AA3 把它钉成了 characterization：
`protected_prefixes_match_the_literal_path_so_a_cd_first_slips_through`。

**这是守卫最严重的一个洞**：`core/crates/contracts/**` 是双重冻结面，
而绕过它只需要先 `cd`。

AA3 给的方向：**在匹配前把路径按 cwd 归一化**。并且警告：

> 风险在于 `cd` 的解析一旦做歪，误拦会成片出现 —— 上面那 18 条回归就是它的安全网，
> **改完必须全绿**。

### ② 真误拦：`cargo fmt --version` / `--help` 被判写模式

AA3 量出的精确判据：**「有 `cargo fmt` 且词里没有 `--check`」**。
改法（AA3 原文）：

> 把「有没有 `--check`」那个判据扩成「有没有 `--check` / `--version` / `--help`」。
> **按词匹配，别按子串** —— 子串匹配会让 `cargo fmt --all -- --help-xyz` 这种东西绕过去。
>
> 验收：`cargo test -p aite --test guard cargo_fmt_is_judged_by_the_check_flag_alone`
> 会**红在那两行 characterization 上**，把它们从 `blocked` 挪到 `allowed` 即为改完。

**收益最大、风险最小的一条，先做它。**

### ③ `PROT_PATHS` 的成员没量全 —— 而且今天又冒出来一个

AA3「没做的」第 4 条列了确认在里面的成员：

```
点名族（读写都拦）：.claude/hooks/guard_bash.py、.claude/settings.json、.contracts.lock
前缀族          ：core/crates/contracts/**、proto/**
可读不可写      ：docs/dev-spec-*.md
```

并说「**还有没有别的成员，量不出来** —— 只能一个个猜着试」。

**2026-09-15 又撞到一个没记过的成员**：

```
$ grep -n 'lark' edge/go.mod
blocked: 该操作触碰受保护面 edge/go.mod（读取位置）
```

`edge/go.mod` 在点名族里（读也拦），台账里**一个字都没记过**。
**这说明「猜着试」的覆盖率比 AA3 以为的还低。**

本轨要做的：按「依赖表 / 锁文件 / 配置」这三类去系统地试
（`core/Cargo.toml`、`core/Cargo.lock`、`edge/go.sum`、`core/rust-toolchain.toml`、
`config/aite.example.yaml`、`docker-compose.yml` …），**把成员表补到能写下来的程度**，
落成回归。

### ④ 台账里三条归因是错的，去改掉

AA3 量出了正确答案但没权限改别人的节。**这三条要你去改**
（改的是 `review/review-findings-2026-09-12-vmerge.md` 和
`core/crates/app/tests/guard.rs` 的模块头那张表）：

| 错在哪 | 正确答案（AA3 实测） |
|---|---|
| 台账第十节 Y2：「一次『不透明载荷』（**正文太长**）」 | 判据是**命令替换** `$(…)` / 反引号 / `$((…))`。Z2 给的替代归因「判不出读/写的位置」也不对 —— 那种情况实测是 `写入/执行位置` |
| 台账第十二节 Z2 误拦表第 7 行：「heredoc 正文里有配不平的引号 / **中文引号**」 | **两半都错**：中文引号不触发（四条实测放行）；这条跟 heredoc 无关（`echo '\nhi\n'` 复现）。真判据是「**某一行内 ASCII 引号未闭合**」 |
| 同上那条的「标准绕法」 | 不是「别用 heredoc」，是「**别让引号跨行**」。**派单模板里那句也要跟着改** |

AA3 当时没改，是怕和 Z2 的叙述打架。**本轨的处理口径**：
在原表格那一行**就地改正**，并在行末标 `（AA3 实测更正，2026-09-13）`，
让读的人知道这一行被改过、依据是哪一节。别再留两段并存 —— 已经并存一轮了。

## 必读（按顺序）

1. `core/crates/app/tests/guard.rs` 全文 —— 18 条现行回归是行为规格的全部；
   `run_guard()` / `bash()` / `tool()` 是你的探针；模块头那张表是 ④ 要改的对象。
2. `review/review-findings-2026-09-12-vmerge.md` 第十六节（AA3 回执）**全节**——
   ① 的测量矩阵（183 条 payload 的结果）是你的起点，③ 的三条候选、
   ④ 的 fail-closed 复核、「没做的」七条，每一条都要读。
3. 同一份台账第十二节（Z2 回执）—— 补丁脚本的完整方法论
   （`--check` / 真写 / 八格自验矩阵 A–H）。
4. `review/z2-guard-patch.py`、`review/v6-guard-patch.py` —— **两份骨架，照它们来**。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-bb4
分支     : task-bb4
基线     : 7019c48
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-bb4
git log --oneline -1        # 期望 7019c48
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0
cd core && cargo test -p aite --test guard    # 期望 18 条全绿
```

**关键行期望**（2026-09-15 在 `task-bb1` 这个 worktree 里实跑的原样抄录）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=864 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

> ⚠️ 看到 `cargo passed=863 failed=1` 且唯一红点是 `-p aite --test guard` ——
> **停下来喊人**：那说明守卫的配置面出了事，而那正是你要研究的保护面，
> **本轨尤其不许自己动手修**。

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
> 把拦截原话**逐字**记下来 —— 本轨要拿它当对照组。没被拦就是守卫静默失效，**停下报告**。

## 纪律

1. **可写面**：`core/crates/app/tests/guard.rs`、`review/bb4-guard-patch.py`、
   `review/review-findings-2026-09-12-vmerge.md`（④ 那三条**就地改正**，
   外加**追加**你自己那一节）。
2. **只读面**：其余一切。**本轨零产品代码改动。**
3. **`.claude/**` 读写都被拦，不许用测试当读取通道 dump 源码**（见上面那条硬约束）。
4. **`AITE_RELOCK=1` 不许自己用。** 补丁脚本没有那个变量要当场拒绝执行。
5. **fail-closed 是底线，任何提案先过这一关**：收窄之后，守卫失效时还 fail-closed 吗？
   不是就废掉那条提案，不管它能省多少麻烦。
   （2026-09-12 真踩过：hook 执行失败是非阻塞放行且不报警，整场守卫静默失效。）
6. `cargo fmt --all` 会被守卫拦（本轨正好要治这一格），改用
   `rustfmt --edition 2024 crates/app/tests/guard.rs`。

## 验收

```bash
scripts/check.sh                              # 全部通过，退出码 0
cd core && cargo test -p aite --test guard    # 18 条 + 你新加的，全绿
cd core && cargo clippy --workspace --all-targets -- -D warnings
```

- **除 `cargo passed=` 外四行必须逐字不变**（本轨零产品代码改动）；
- `cargo passed=` 的涨幅逐条解释。

补丁脚本**不许在本轨跑**。交付形态：
脚本 + `--check` / `--root <临时目录>` 自验矩阵的逐字输出 + 给总管的一行运行命令。

**② 那条的验收 AA3 已经写死了**：改完之后
`cargo_fmt_is_judged_by_the_check_flag_alone` 会红在那两行 characterization 上，
把它们从 `blocked` 挪到 `allowed` 即为改完 —— **这是补丁生效的判据，
写进给总管的命令链里**。

## 回执

追加进台账，标题「二十一、BB4 回执 —— 2026-09-15」。要有：

- 基线与开场自检（五行 + 守卫拦截逐字原话 + guard 18 条的结果）；
- ① `cd` 归一化那条：判据规格（不依赖源码形状的那一版）、补丁锚点策略、
  **为什么你认为它不会造出成片误拦**，以及 18 条回归的全绿证明；
- ② `cargo fmt` 那条的补丁 + characterization 从 `blocked` 挪到 `allowed` 的 diff；
- ③ **补全后的 `PROT_PATHS` 成员表**（每一行标「实测命中 / 实测未命中」），
  以及你试过但没命中的那些（也要列 —— 那是下一个人不用再试的部分）；
- ④ 三条归因更正的 diff（台账 + `guard.rs` 模块头）；
- **每条提案过 fail-closed 这一关的结果**（照 AA3 ④ 那张表的形式）；
- **记账转出去的**（表格：位置 / 病 / 归哪轨）；
- **没做的 / 拿不准的** —— 尤其「判定顺序是推断不是测量」这类，
  **本轨最大的诚实风险还是把推测写成测量**。
