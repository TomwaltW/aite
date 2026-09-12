# 任务 W2 — 收尾面拆弹、两条说谎的注释、三条从没生效过的断言，外加 `!stop` 跟不上 `!status`

## 背景：这轨是从哪来的

V1–V6 六轨 + W1 已全部合进 `main`。**P0 的代码面收完了** —— 两份 spec 查下来，没兑现的
只剩 `docs/dev-spec-2026-09-09.md` §2.4 / `docs/dev-spec-2026-09-11-rustgo.md` §4.4 的
**M1–M6 真机验收**与 **3 分钟演示**，那是总管自己的活；W1 已经把三份手册校到
「照着敲不会卡、排障表不会把人引错方向」。

所以**这一轨不在任何人的关键路径上** —— 它收的是 RΩ 审核台账里剩下的账。
但里面有两条不是「文字问题」：

- 一条**真炸**（`run.rs:197`，panic 会把整段收尾跳过，沙箱和库都不还）；
- 一条是**边界声明说了谎**（`app.rs:129`，C-TΩ-1 靠那句注释划边界，而它现在是假的）。

另外三条是**从没生效过的测试断言** —— 它们比没有断言更糟：给人「这里有人看着」的
错觉。台账这一轮已经抓到三条恒真断言了，不要再造第四条。

最后一条是 V5 留下的半截：`!status` 修了、`!stop` 没修，两条命令现在对「什么算活跃」
意见不一致。W1 已经把文档写对了，代码这边要给个说法。

## 必读（按顺序）

1. **`review/review-findings-2026-09-12-vmerge.md` §4.1** —— 本轨的账逐条列在那儿，
   连同 §3.1 那个引用框（`!stop` 那条的完整来龙去脉）。**§五是 W1 回执**，
   里面「又一次 `cargo` 假红」那一段与你的 ⑥ 直接相关。
2. `review/review-findings-2026-09-12-romega.md` §4.1 / §4.4 —— 这些账最初立在哪、
   当时说的是什么。
3. `docs/acceptance-M.md` §0.4 那段引用框 + §7 排障表那两行 —— **W1 刚按现状写死的**。
   你改完 ⑦ 之后这几处要跟着改（见「要做什么 · ⑦」）。
4. `docs/dev-spec-2026-09-11-rustgo.md` §2.1（进程模型）、§3.3（失败面）、§5（归属表）。
   **这份冻结，只读**，守卫会拦。
5. `core/crates/app/src/run.rs` 的**模块头注释**（1–40 行左右）—— C-TΩ-1 那套收尾序列
   写在那儿，⑴ 要动的正是它保护的那一段。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-w2
分支     : task-w2
基线     : abd93da 的代码面（worktree 建在它上面；HEAD 可能多一个只加派单文件的 commit）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin（check.sh 自己 export，手敲 cargo 时要注意）
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-w2
git log --oneline -1        # 记下 sha，回执里当基线
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（总管 2026-09-12 在这个 worktree 里实跑过，下面是原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=793 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

> ⚠️ **已知假红，别当回归。** `-p aite --test` 的 `startup_recovery` /
> `graceful_shutdown` / `reconnect_replay` 三个 target 会偶发红一条，**单轨也会撞上**
> （2026-09-12 记录在案四次：`716/2`、`791/2`、`792/1`、`792/1`）。判据：把
> `error: test failed, to rerun pass …` 点名的那个 target **单独跑一遍**，绿就是假红，
> 直接开工。
>
> **三个里有两个的抖动源就是你要修的东西**：`graceful_shutdown` 是 ⑥（计时区间量错），
> `reconnect_replay` 是 ⑤（守卫恒为 false）。`startup_recovery` 的源还没定位。
> 所以你这一轨收尾时的判据不只是「全绿」，还要**这三个 target 不再抖**。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。

## 可写路径

| 面 | 权限 |
|---|---|
| `core/crates/app/src/run.rs`、`app.rs` | 读写 |
| `core/crates/app/tests/crash_recovery.rs`、`reconnect_replay.rs`、`graceful_shutdown.rs` | 读写 |
| `core/crates/app/tests/common/**` | 读写（⑥ 可能要加计时辅助） |
| `core/crates/control/src/plane.rs` + `core/crates/control/tests/**` | 读写（⑦） |
| `docs/acceptance-M.md` | **只许改 ⑦ 点名的那三处**，别的段落别碰 |
| `core/crates/app/src/main.rs`、`cli.rs`、`wiring.rs` | **只读 —— 归 W3**（`--help` 那条是 W3 的） |
| `edge/**` | **只读 —— 归 W3** |
| `core/crates/contracts/**`、`proto/**`、`.contracts.lock`、`docs/dev-spec-*.md` | **冻结**，守卫会拦 |
| `scripts/check.sh`、`.github/**`、`Makefile`、`.claude/**` | 只读 |

---

## 要做什么

### ① `run.rs:197` —— 真拆弹（不是再挡一次门）

```rust
&& tokio::time::timeout(Duration::from_secs_f64(grace.max(0.0)), app.plane.join())
```

`Duration::from_secs_f64` 对 **NaN / 负数 / 超出 `u64::MAX` 秒的有限数**都 panic。
`grace.max(0.0)` 挡得住负数，**挡不住上溢，也挡不住 NaN**（`NaN.max(0.0)` 是 `0.0`，
这条碰巧不炸，自己验一下）。

V6 ④a 已经在 CLI 门口加了 `0..=86400` 的校验（实测 `--grace 100000` → 退出码 2），
**但那只是门口** —— V6 自己的提交里写着「**这是把炸弹挡在门口，不是拆弹 —— 拆弹归 V5**」，
V5 没做。`ServeOptions { shutdown_grace_sec: … }` 是 `pub` 字段（`run.rs:92`），
任何直接构造它的调用方（含测试、含未来的嵌入式用法）都能绕过 CLI。

**为什么这条不是「理论问题」**：panic 发生在 `shutdown()` 里，而它后面还有
`sandbox.close_all()` / `store.close()`（看 `run.rs` 模块头 C-TΩ-1 那段序列）——
**整段被跳过**：容器不还、库不关。

**改法自己定，但要满足三条**：
1. 非法取值**不许 panic**，走一条有说法的路（退到默认值并 `warn`、或者 `ServeOptions`
   改成带校验的构造函数 —— 后者更彻底，但会动调用点，自己权衡并在回执里说明理由）；
2. **C-TΩ-1 的收尾序列一步都不许少** —— 无论 grace 取什么值；
3. 补测试钉住：至少 `NaN`、`f64::INFINITY`、`1e300`、负数四个取值，
   断言**收尾跑完了**（不是只断言「没 panic」）。

### ② `app.rs:129` —— 那条注释现在是假的

```rust
/// 按 config 把零件装起来。**只组装**：不连网、不起容器、不发消息。
///
/// 唯一的副作用是建那三个落盘目录（C-TΩ-1 允许的那一条）。
```

实际上 `build_app` 在 `app.rs:156` 调 `EdgeClient::connect()`、`:168` 调
`check_contract_version()` —— **两件都连 edge**（后者还会重试最多 5 次、每次隔 1s，
最坏 4s 挂在那儿）。模块头 `app.rs:6` 自己都写着「core 侧换成 `EdgeClient::connect`
+ 一次 `contract_version` 比对」，跟这条注释直接打架。

**这不是措辞问题**：台账原话是「`build_app` 的副作用超出 C-TΩ-1 声明的范围
（连网 + 最多 4s 睡眠 + 建 SQLite 库文件），而钉这条的测试只覆盖注入路」。
注释是这条契约唯一的声明处。

**要做的**：把注释改成**说实话**（连什么、什么条件下连、最坏阻塞多久、建了哪些东西），
并**补一条测试钉住「注入路确实不连网」**（现有测试只覆盖注入路，但没有一条断言
「没注入时才连、注入了就不连」这个分界）。如果你认为该改的是代码而不是注释
（让 `build_app` 真的不连网、把连接推迟到 `run_app`），**停下报告** —— 那会动
C-TΩ-1 的形状，不是本轨能单方面定的。

### ③ `app.rs:78` —— 另一条说谎的注释

```rust
/// 真机那一路才有：留着它是为了 `!status` 的健康行与收尾时的连接态日志。
pub edge: Option<Arc<EdgeClient>>,
```

**这两处都不存在。** 全仓 grep 一遍确认（`!status` 的实现在
`core/crates/control/src/plane.rs:536` 的 `cmd_status`，它不碰 edge；收尾日志同理）。
V5 之后 `edge` 的真实用途是**契约闸门**（`core/crates/edge-client/src/gate.rs`）。
按实际用途重写这条注释。

### ④ `crash_recovery.rs:53,125` —— 越界 panic 冒充断言失败

```rust
let mut task = app.store.list_active_tasks(CHAT).await.expect("list")[0].clone();
```

两处都是「先 emit 事件、再直接取 `[0]`」，**没有守卫**。任务已经交付时列表是空的，
报出来是 `index out of bounds`，**不是一句说人话的断言失败** —— 调试的人得先看懂
这行代码才知道它想说什么。

换成带消息的守卫（`wait_until` + `assert!(!tasks.is_empty(), "…")`，或等价物），
失败时要打出**它在等什么、当时列表里有什么**。

### ⑤ `reconnect_replay.rs:168` —— 死代码守卫

```rust
async fn wait_gate_reached(&self) {
    let notified = self.reached.notified();
    if self.inner.call_count() > 0 {   // ← 恒为 false
        return;
    }
    …
}
```

注释说这是「Notify 的通知是一次性的，可能早于 await —— 所以先看标志位再等」。
但**闸门期间 `call_count()` 恒为 0**，这个分支从没走到过 —— 也就是说它想防的那个
竞态根本没被防住。

> 🔴 **这条不是清死代码，它就是一个真在抖的测试的病根。**
> 2026-09-12 总管在**你这个 worktree** 里跑基线时撞到：
> `cargo passed=792 failed=1`，失败名逐字是
> `error: test failed, to rerun pass \`-p aite --test reconnect_replay\`` ——
> 单独连跑五遍 `10 passed; 0 failed` 全绿。
> **也就是说：守卫想防的那个「通知早于 await」的竞态，是真会发生的**，
> 只是平时撞不上；而那个本该兜住它的分支恒为 false。
> 所以 ⑤ 的第一选项（让守卫真生效）比第二选项（证明不可能发生并删掉）**证据更足** ——
> 但仍然要你自己跑一遍确认，别照抄这段话。

两条出路，**选哪条都要在回执里说明**：
- 换成真的能反映「已经到过闸门」的标志位（多半要新加一个 `AtomicBool`），让守卫真生效；
- 或者证明这个竞态不可能发生，**删掉**这个分支并把理由写进注释 ——
  但上面那条实测在打这条路的脸，走它要先解释 `792/1` 那次是怎么来的。

**不许原样留着。**

**验收要求**：改完 `cargo test -p aite --test reconnect_replay` **连跑二十遍**，
外加在**负载下**跑一遍（比如同时起一个 `cargo test --workspace`），二十遍全绿才算数。

### ⑥ `graceful_shutdown.rs:237,249` —— 量错的区间（也是那两个假红的抖动源）

```rust
let started = std::time::Instant::now();     // :237 —— 起点在建场之前
…
assert!(elapsed < 3.0, "run_app 花了 {elapsed:.3}s 才返回，宽限期只有 {TINY_GRACE_SEC}s");
```

`started` 起在建场之前，而建场里有两个各 5s 预算的 `wait_until`（`:71`、`:86` 一类）。
所以 `elapsed < 3.0` 量的是「建场 + 收尾」，**不是收尾**：机器一忙，建场那几秒就把
阈值吃光了 —— 这正是 `graceful_shutdown` / `startup_recovery` 反复假红的来源
（见 vmerge 台账 §五 W1 那段：`791/2`，单独跑 7 passed / 9 passed 全绿）。

**把计时起点挪到真正要量的那一段的正前方**，并复核阈值还合不合适。
改完**连跑十遍** `cargo test -p aite --test graceful_shutdown`，十遍都绿才算数
（这条要贴进回执）。

> 顺带：`startup_recovery` 也在假红名单里，但**它的抖动源还没定位**。
> 有余力就查一下并记账；查不出来别硬做，写进回执的「没做的」。

### ⑦ `!stop` 跟不上 `!status` —— V5 留下的半截

```
plane.rs:480  status_tasks()         list_active_tasks + 控制面 running（过滤终态 + 同群）   ← V5 改了
plane.rs:646  resolve_stop_target()  只有 list_active_tasks                                  ← 没改
plane.rs:663  resolve_task()         只有 list_active_tasks（卡片 stop 按钮走这条）          ← 没改
```

于是第一步就 `final` 的短任务（`Answering` 状态，不在 `ACTIVE_TASK_STATUSES` 里）
**在 `!status` 里列得出来、`!stop` 却回「没有这个任务」**。改之前两条口径一致（都查不到），
改之后**互相矛盾**：用户看见它在列表里、伸手去停却被告知不存在。

**两条出路，你来定，但必须选一条并补测试钉住**：

- **(a) `!stop` 跟上** —— `resolve_stop_target` / `resolve_task` 改走 `status_tasks`
  的同一套口径。要想清楚：停一个正在交付的任务意味着什么？`cancel_task` 落刀前
  按 id 重读、终态不改写这道防线是 V5 加的（`plane.rs` 里找），它够不够？
- **(b) 写成刻意的约定** —— 「`Answering` 不可停」是设计，那就在代码里**显式**这么写
  （不是靠 `list_active_tasks` 碰巧查不到），并且让 `!stop` 回一句**说得通的话**
  （「这个任务正在交付，停不了」），而不是「没有这个任务」。

**无论选哪条**：`ACTIVE_TASK_STATUSES` 是**双重冻结**的（`frozen_values.rs` +
`roundtrip.rs`，V5 提交里点名过），**不许动它**。

改完要同步 `docs/acceptance-M.md` 的三处（W1 刚按现状写死的，行号自己 grep 确认）：
§0.4 那段引用框里的三行表、§7 M1 排障表**裂开的那两行**、§8「`!status` / `!stop`
的两条使用口径」。**只改这三处**，别的段落别碰。

---

## 纪律

1. **每条改动都要有测试钉着。** 这一轮台账已经抓到三条恒真断言（⑤ 就是其中一条），
   不要再造第四条 —— 新断言写完自己验一遍：**把产品代码改坏，它必须红**。
2. **不许 skip。** 跑不了的环境该红就红，别用 skip 分支咽掉。
3. **契约锁必须始终 `OK 25 files`**。变红 = 本轨失败。`ACTIVE_TASK_STATUSES` 和
   `ModelError` 这类冻结值一个都别动。
4. **别越界**：`main.rs` / `cli.rs` / `wiring.rs` / `edge/**` 归 W3，`--help` 那条别顺手改。
   W3 与你同时在跑，改了会冲突。
5. **测试数只许涨。** `cargo passed=793` 是起跑线，收尾报新数并说明每一条多在哪。
6. 卡住了：要改的文件不在白名单 → 停下报告；②/⑦ 里「该改代码还是改注释/约定」
   拿不准 → **先在回执里把两条路各自的代价写清楚**，选一条做，别停在那儿不动。

## 验收

```bash
scripts/check.sh                       # 「全部通过」，退出码 0；五行关键值除 cargo 计数外不变
cd core && cargo test -p aite --test graceful_shutdown    # ⑥ 之后连跑十遍，十遍全绿
cd core && cargo test -p aite-control                     # ⑦ 的主战场
cd core && cargo clippy --workspace --all-targets -- -D warnings
```

外加**自己交叉验一遍**：

- [ ] ① 的四个非法取值各有一条测试，断言的是**收尾跑完了**，不是「没 panic」。
- [ ] ②③ 两条注释改完后，拿注释里的每一句话去代码里对一遍，句句成立。
- [ ] ⑤ 选的那条路在回执里有理由；如果是「让守卫真生效」，把产品代码改坏它要红。
- [ ] ⑦ 改完后 `!status` 与 `!stop` 对同一个任务的答复**一致**（或者不一致是刻意的
      且话说得通），并且 `acceptance-M.md` 那三处与代码逐字对得上。
- [ ] 新加的每条断言都做过「改坏产品代码 → 它必须红」这一步。

## 回执格式

在回复里写（**不要**新建回执文件；台账 `review-findings-2026-09-12-vmerge.md`
只许**追加**一节「W2 回执」）：

```
## W2 回执

基线 : <开场自检那个 sha>
check.sh : <五行关键值>  退出码 <n>
假红 : <撞到没有；撞到了贴失败名 + 单独跑的结果>

### ①–⑦ 逐条
（每条写：病在哪 / 改法 / 为什么选这条 / 钉它的测试是哪几条 / 改坏产品代码验过没）

### 测试数
793 → <N>，每条多在哪

### ⑥ 的十遍
（十遍的输出）

### 记账转出去的
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
```
