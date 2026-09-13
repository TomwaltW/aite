# 任务 Y1 — `StopSignal::set()` 丢信号：真机上 `SIGTERM` 来早了就永远不退

## 背景：这轨是从哪来的

**这一轨不是记账，是一条会在生产上挂死进程的产品缺陷。**

X1 那一轨在查 `startup_recovery` 的抖动时挖出来的（台账第八节 ④）。它给了三条独立证据
（三份栈形状一致：两个 tokio worker 全 park、栈上零个 aite 帧；死锁那遍日志停在
`aite.orphans`、`aite.stopping` 一次都没打；把兜底从 10s 放到 60s 照样撞穿且**实际等满 60.0s**），
并落成一个五行探针。**总管 2026-09-13 独立复验过，确认成立**：

```
set() 之后 is_set() = false          ← 没人订阅时
有接收者时 set() 之后 is_set() = true  ← 有订阅者时
```

病根（`core/crates/app/src/run.rs:63-75`）：

```rust
pub fn new() -> Self {
    let (tx, _rx) = watch::channel(false);   // ← _rx 当场丢掉
    Self { tx: Arc::new(tx) }
}

pub fn set(&self) {
    let _ = self.tx.send(true);              // ← 没有接收者时返回 Err，且连内部值都不改
}
```

`tokio::sync::watch::Sender::send` 在**一个活跃接收者都没有**时返回 `Err`，**并且不更新内部值**。
而唯一订阅它的地方是 `serve()`（`StopSignal::wait()` 里的 `self.tx.subscribe()`）。所以在
`run_app` 走到 `serve()` 之前，`set()` 是**彻底的 no-op** —— 信号被吃掉，不留痕迹。

**为什么这不是测试专属问题**：`aite run` 把 `SIGINT` / `SIGTERM` 接到同一个 `StopSignal`
（`install_signal_handlers`，`run.rs:437`）。信号赶在 `serve()` 之前到达 —— compose 的
`stop_grace_period`、k8s 滚动更新、人手快按 Ctrl-C 都会 —— **进程永远不退，只能 `SIGKILL`**。
而 `docker-compose.yml` 配的是 `restart: unless-stopped`，`SIGKILL` 之后还会被拉起来。

X1 只能在测试侧兜住（`run.rs` 不在它可写面）：`tests/common/mod.rs` 的 `shutdown` 现在
**重试 `set()` 直到 `is_set()` 为真**（带 5s 死线）。**那是绷带，不是药。** 你的活是上药，
然后把绷带拆掉。

## 必读（按顺序）

1. **`review/review-findings-2026-09-12-vmerge.md` 第八节（X1 回执）的 ④** ——
   三条证据、探针、窗口在哪、以及「怎么区分它和真挂死」的那段判据，全在那儿。
   末尾「记账转出去的」第一行就是本轨。
2. `core/crates/app/src/run.rs` —— **模块头注释（1–50 行）里的 C-TΩ-1 收尾序列**
   是这一轨的宪法；`StopSignal`（`:53-90`）、`run_app` / `takeoff` / `shutdown`
   （`:100-230` 一带）、`install_signal_handlers`（`:437`）。
3. `core/crates/app/tests/common/mod.rs` 的 `shutdown` 与 `SHUTDOWN_FALLBACK_SEC`
   —— X1 加的那个重试循环 + 指向病根的注释，**修好之后连注释一起删**（X1 在注释里写明了）。
4. `docs/dev-spec-2026-09-11-rustgo.md` §2.1（进程模型）、§3.3（失败面）。
   **冻结、只读**，守卫会拦。
5. `docker-compose.yml` 的 `stop_grace_period` / `restart` 两处（**只读**）——
   真机上这条病怎么表现出来的出处。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-y1
分支     : task-y1
基线     : 18f30b6（= merge(task-x1) 并入 main 那一格）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin（check.sh 自己 export，手敲 cargo 要注意）
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-y1
git log --oneline -1        # 期望 18f30b6，记下当基线
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（总管 2026-09-13 在这个 worktree 里实跑过，原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=818 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

> ⚠️ **已知假红。** `-p aite --test` 的 `startup_recovery` / `graceful_shutdown` /
> `reconnect_replay` 三个 target 会偶发红一条。判据：把
> `error: test failed, to rerun pass …` 点名的 target **单独跑一遍**，绿就是假红。
>
> W2 修了后两个，X1 在测试侧兜住了 `startup_recovery`。**所以现在基线应该是不抖的** ——
> 如果你开场自检就撞到这三个里的任何一个，那是新信息，**贴进回执**。

> 💡 **写脚本注意**：`scripts/check.sh > log 2>&1; echo "EXIT=$?"` 在后台任务里会骗人 ——
> 报的是最后那个 `echo` 的退出码，永远 0。要 `rc=$?; …; exit $rc`。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
>
> 已知误拦（不是故障，换个写法绕开）：守卫 `PROBES` 里还留着指向已删文件的
> `aite/contracts/__init__.py`，命令里带 `*` 通配符时会被反向匹配误拦。
> 删它的补丁（`review/v6-guard-patch.py`）**只有人能跑**，至今没跑。

## 可写路径

> **Y2 与你同时在跑。** 下面这张表两轨**逐字相同**，是同一份划分 ——
> 上一轮 W2/W3 出过「同一个文件在两份派单里互相指给对方」的事故（`cli.rs` 因此谁都没改、
> 留下两条反的注释），所以这次把边界写死在两边。

| 面 | Y1（你） | Y2 |
|---|---|---|
| `core/crates/app/src/run.rs` | **读写** | 只读 |
| `core/crates/app/src/preflight.rs` | 只读 | **读写** |
| `core/crates/app/tests/preflight_e2e.rs`、`cli_smoke.rs` | 只读 | **读写** |
| `core/crates/app/tests/` 其余全部（含 `common/mod.rs`） | **读写** | 只读 |
| `core/crates/app/tests/signals.rs`（**你新建**） | **读写** | 不存在于它的视野 |
| `core/crates/edge-client/src/link.rs` | 只读 | **读写** |
| `README.md`、`docs/**`（非 spec）、`review/inventory-*.md` | **只读**（见下） | **读写** |
| `core/crates/app/src/{app,cli,wiring,main}.rs` | 只读 | 只读 |
| `core/crates/control/**`、`edge/**`、`.github/**`、`Makefile`、`scripts/**`、`.claude/**` | 只读 | 只读 |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` | **冻结** | **冻结** |

**`core/crates/app/src/cli.rs` 两轨都只读，归总管** —— 上一轮就是把它互相指给对方才出的事。
要改它 → 写进回执，别伸手。

**文档你不许改。** `README.md` 的「停机」那一段（`SIGTERM` 走优雅退出…）与
`docs/acceptance-M.md` 的 M6 会因为你这条改动而需要复核，但文档面归 Y2 ——
**把你想改的那几句原文 + 改成什么写进回执**，总管合并时落。

**台账：你只许追加一节「九、Y1 回执」。** Y2 追加的是「十」。
（上一轮 W2/W3 都写「六」，合并时撞了，这次预先错开。）

---

## 要做什么

### ① 上药：让 `set()` 在没有接收者时也算数

两条现成的路，**选哪条都要说理由**：

- **(a) `send_replace(true)`** —— `watch::Sender::send_replace` 不管有没有接收者都更新值。
  一行改动，语义最直白。
- **(b) `new()` 里自己留一个 `rx`** —— 让接收者永远存在，`send` 就不会 `Err`。
  要想清楚：那个 `rx` 存哪、会不会让 `wait()` 的语义变味（`watch` 的「已读/未读」标记
  是按接收者算的）。

**判断依据要落在实证上**，不是读文档：两条都试一遍，看 `wait()` 的行为有没有变
（尤其「`set()` 在 `wait()` 之前」与「`set()` 在 `wait()` 之后」两种时序都要过）。

**顺带核这三处**（改完口径要一致）：
- `is_set()` 读的是 `*self.tx.borrow()` —— 上药之后它才真的能反映「置过了」；
- `wait()` 里的 `borrow_and_update()` 早退分支 —— `set()` 先发生时它必须立刻返回；
- `install_signal_handlers` 里第二次信号硬退那条路（`run.rs:437` 往下）——
  确认它不依赖 `set()` 的返回值。

### ② 补一条**本来就该抓住它**的回归

这是本轨的核心交付。X1 那个五行探针能抓住单元层面，但**它抓不到真正的后果**
（进程收到 `SIGTERM` 却不退）。要两层：

**(a) 单元层**：`set()` 在任何订阅者出现之前调用，`is_set()` 必须为真；
`wait()` 在 `set()` 之后调用必须立刻返回。放哪你定（`run.rs` 的 `mod tests`
或 `tests/signals.rs`），**新建 `tests/signals.rs` 是允许的**。

**(b) 进程层**：**起真二进制、在它走到 `serve()` 之前发 `SIGTERM`、断言它自己退出**
（不是被 `SIGKILL`）。这是唯一能钉住「真机不会挂死」的写法。
注意 `tests/cli_smoke.rs` **不在你的可写面**（归 Y2）—— 新建 `tests/signals.rs` 放这条。

> 时序是这条测试最难的部分：要可靠地在 `serve()` 之前把信号送到。
> 可用的钩子：`ServeOptions` 的字段、注入面（`Injections`）、
> 或者让平台的 `start()` 阻塞一下把窗口撑开（X1 就是这么复现的：
> `RunningApp::start` 等的是 `platform.start()` 被调，而 `takeoff()` 里 `start()`
> 之后还有 `spawn(run_forever)` 和 `serve()` 两步，测试落在这两步之间）。
> **别用固定 sleep 赌时序** —— 那会造出第四个抖动 target。

**两条都要做「改坏产品代码 → 它必须红」**：把 ① 的改动退回去，(a)(b) 都得红。
(b) 退回去之后**要真的挂住**（撞兜底而不是通过），把那次的输出也贴进回执。

### ③ 拆绷带

X1 在 `tests/common/mod.rs` 的 `shutdown` 里加了「重试 `set()` 直到 `is_set()`」的循环
（带 5s 死线）和指向病根的注释，注释里写明「`run.rs` 修好之后该删」。**删掉它。**

删完要复验 X1 报的那个数：`orphans_are_closed_before_the_platform_starts` 在
**8 路并发 120 遍**下红 0 遍。X1 改前是 24 遍红 3 遍（12.5%）——
**你拆了绷带之后必须靠 ① 的药达到同一个数**，达不到说明药没上对。

顺带复核 X1 同时改的另两处（都在 `common/mod.rs`）：
- `SHUTDOWN_FALLBACK_SEC` 被它改回 10s（放 60s 毫无意义的理由写在回执里，同意就别动）；
- 超时那句 panic 消息指向「看最后一条 `aite.*` 日志停在 `aite.stopping` 之前还是之后」——
  ① 上药之后这句判据还成立吗？成立就留，不成立就改准。

### ④ 顺着病根扫一遍同族

`watch::Sender::send` 的返回值被 `let _ =` 丢掉的地方，全仓还有没有别处？
`send` / `send_if_modified` / `send_modify` 一起扫。

**扫到的都列进回执**（位置 + 有没有接收者保证 + 丢了返回值会怎样）。
在你可写面内且确实有病的就改；不在的记账。
**别把「返回值被忽略」一律当病** —— 有接收者保证的地方忽略它是对的，
判断依据写清楚。

---

## 纪律

1. **②(b) 是本轨的交付重点。** 只改 ① 不补进程层回归 = 没交活：那条病能活这么久，
   正因为没有任何测试站在进程这一层。
2. **不许用固定 sleep 赌时序。** 这几轮台账已经抓到四条恒真断言和三个抖动 target，
   别造第五条/第四个。
3. **新断言写完自己验一遍：把产品代码改坏，它必须红。**
4. **契约锁必须始终 `OK 25 files`**，冻结面一个字别动。
5. **测试数只许涨。** `cargo passed=818` 是起跑线。
6. **不许改文档** —— 想改的写进回执（见「可写路径」那一节）。
7. 卡住了：要改的文件不在白名单 → 停下报告；②(b) 的时序想不出可靠钩子 →
   **把试过的写进回执**，先交 ①③④ 和 ②(a)，别拿固定 sleep 凑一条。

## 验收

```bash
scripts/check.sh                                          # 「全部通过」，退出码 0
cd core && cargo test -p aite --test signals              # ② 的新回归
cd core && cargo test -p aite --test startup_recovery     # ③ 拆绷带之后
cd core && cargo test -p aite --test graceful_shutdown
cd core && cargo clippy --workspace --all-targets -- -D warnings
```

外加**自己交叉验一遍**：

- [ ] ① 两条路都试过，选的那条有实证理由（不是读文档）；`is_set()` / `wait()` 两种时序都过。
- [ ] ②(a)(b) 都做过「改坏 → 必须红」；(b) 退回去时**真的挂住**，输出贴了。
- [ ] ③ 绷带删干净（`common/mod.rs` 里那个重试循环和注释一起走），
      `orphans_are_closed_before_the_platform_starts` 在 8 路并发 120 遍下红 0。
- [ ] ④ 全仓扫过 `watch` 的 send 家族，清单在回执里，判断依据写清楚了。
- [ ] `README.md` 的「停机」段与 `acceptance-M.md` M6 需要怎么改，**原文 + 改法**写进回执。

## 回执格式

在回复里写（**不要**新建回执文件；台账只许追加**「九、Y1 回执」**这一节）：

```
## Y1 回执

基线 : <开场自检那个 sha>
check.sh : <五行关键值>  退出码 <n>
假红 : <撞到没有；撞到了贴失败名 + 单独跑的结果>
守卫 : Read .claude/hooks/guard_bash.py 被拦 ✅

### ① 选了哪条路、为什么
（两条各自试出来什么；is_set() / wait() 两种时序的实测）

### ② 两层回归
（(a)(b) 各是什么形状、时序靠什么钩子钉住的、改坏之后各红成什么样 ——
 (b) 挂住那次的输出原样贴）

### ③ 拆绷带之后的数
（8 路并发 120 遍的结果；SHUTDOWN_FALLBACK_SEC 与那句 panic 消息的处置）

### ④ watch send 家族全仓清单
| 位置 | 有接收者保证？ | 丢返回值会怎样 | 改了没 |

### 要 Y2 / 总管落的文档改动
| 文件 | 现在的原文 | 应该改成 |

### 测试数
818 → <N>，每条多在哪

### 记账转出去的
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
```
