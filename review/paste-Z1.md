# 任务 Z1 — 收掉 Y1/Y2 转出来的四条，外加把「edge 没起也能起飞」钉成既有行为

## 背景：这轨是从哪来的

Y1/Y2 已合进 `main`（`cargo passed=834 failed=0`，门禁全绿）。**P0 的代码面没有拦路的账了** ——
没兑现的只剩 `docs/dev-spec-2026-09-09.md` §2.4 / `docs/dev-spec-2026-09-11-rustgo.md` §4.4
的 **M1–M6 真机验收**与 **3 分钟演示**，那是总管自己的活，等飞书凭证。

这一轨收的是 Y1/Y2 两份回执末尾「记账转出去的」那几行。**全是 low，但有三条不是文字活**：

- ① 是**第三个**「preflight 七组一组都没管、而 `aite run` 退出码 2」的口子（Y2 实证在案）；
- ② 是 Y1 刚治好那条病的**同一个形状**，在另一个文件里原样还在；
- ⑤ 是 Y1 那条进程级回归**正在依赖**的一条既有行为，而全仓没有任何文档或测试写过它。

派单里凡写「实证在案 / Y1 实测」的，出处都在
`review/review-findings-2026-09-12-vmerge.md` 第九、十两节，**自己开文件核，别信转述**。

## 必读（按顺序）

1. **`review/review-findings-2026-09-12-vmerge.md` 第九节（Y1）、第十节（Y2）** ——
   两份回执末尾的「记账转出去的」表就是本轨的全部来源。
   第九节的 ①（为什么选 `send_replace`）与 ④（`watch` send 家族的判断依据）是 ② 的模板；
   第十节的 ①②（怎么把判据折进第 ① 组、五条口径）是 ① 的模板。
2. `core/crates/app/src/app.rs` 的 `build_app` 文档注释（`:145-170`）——
   六步的顺序写在那儿，① 要插的判据跟第 6 步（`SqliteSessionStore::open`，`:226`）对应。
3. `core/crates/app/src/preflight.rs` 的模块头（1–45 行）——
   七组定义表、红线、「一项失败不阻断后面的」。X1/Y2 各往第 ① 组里折过一件事，**你是第三个**。
4. `core/crates/app/src/run.rs` 的 `StopSignal`（Y1 刚改的 `send_replace`）——
   ② 的药与它逐字同一条。
5. `docs/dev-spec-2026-09-11-rustgo.md` §2.1（进程模型、**启动顺序无关**）——
   ⑤ 的依据。**这份冻结、只读**，守卫会拦，**不许往里加东西**。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-z1
分支     : task-z1
基线     : 45728c1（= merge(task-y2) 并入 main 那一格）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-z1
git log --oneline -1        # 期望 45728c1（若多一格、只加了本派单文件，那也对，记下实际 sha）
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（总管 2026-09-13 在这个 worktree 里实跑过，原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=834 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

> ⚠️ **那三个抖动 target 现在应该是不抖的** —— `graceful_shutdown` / `reconnect_replay`
> 由 W2 与 Y1 治了病根（Y1 那条是 `StopSignal` 丢信号，产品缺陷），
> `startup_recovery` 由 X1 换掉了对负载敏感的判据。**开场自检撞到它们任何一个都是新信息，
> 贴进回执**（判据照旧：单独跑一遍那个 target，绿就是假红）。

> 💡 **写脚本注意**：`scripts/check.sh > log 2>&1; echo "EXIT=$?"` 在后台任务里会骗人 ——
> 报的是最后那个 `echo` 的退出码，永远 0。要 `rc=$?; …; exit $rc`。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR`（现在是 `git rev-parse --show-toplevel`）不对、所有守卫都在静默失效，
> **停下报告**。
>
> 守卫 2026-09-13 打过补丁，`PROBES` 里那个指向已删文件的死条目清掉了，通配符误拦少了一类。
> **仍然会撞的两种**（换写法绕开，别碰守卫）：命令里出现受保护路径的**字面量**
> （`git add <那个路径>` 会被判写入位置 —— 用 `git add -u`）；heredoc 正文里有配不平的引号、
> markdown 的 `**` 加粗、或正文太长（改用 Write 工具落文件，别用 heredoc）。

## 可写路径

| 面 | 权限 |
|---|---|
| `core/crates/app/src/preflight.rs`、`app.rs` | 读写（①⑤） |
| `core/crates/app/tests/preflight_e2e.rs`、`cli_smoke.rs`、`build_app_contract.rs` | 读写（①⑤ 的回归） |
| `core/crates/app/tests/signals.rs` | 读写（⑤ 可能要在这儿补一条 —— Y1 新建的，那条进程级回归就在里面） |
| `core/crates/control/tests/support/mod.rs` | 读写（②，**只许改 `ParkedSleep`**） |
| `core/crates/edge-client/tests/contract_gate.rs` | 读写（③，**只许改注释**） |
| `README.md`、`docs/acceptance-M.md` | 读写（④，只许改点名的那两处） |
| `core/crates/app/src/run.rs` | **只读** —— Y1 刚改完，② 只是照它的药抄一遍到别处 |
| `core/crates/app/src/cli.rs` | **只读，归总管**（W2/W3 那一轮把它互相指给对方出过事故） |
| `core/crates/control/src/**`、`edge/**`、`.github/**`、`Makefile`、`scripts/**`、`.claude/**` | 只读 |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` | **冻结**，守卫会拦 |

**台账：只许追加一节「十一、Z1 回执」。**

---

## 要做什么

### ① `storage.sqlite_path` 指着一个不是 SQLite 的文件 —— 第三个同形状的口子

**Y2 的实证（在案，自己复现一遍再动手）**：把 `sqlite_path` 指到一个内容是
`this is definitely not a sqlite database` 的文件，

| | 结果 |
|---|---|
| `preflight --offline` | **全绿，退出码 0** |
| `preflight` 全跑 | **全绿**（第 7 组 `OK 落盘目录可写`） |
| `aite run` | 退出码 2，`aite 起不来：建表失败（…）` |

病在两件事被混成一件：第 7 组验的是「三个路径的**最近已存在祖先**写得进去」，
而 `SqliteSessionStore::open`（`app.rs:226`，`build_app` 第 6 步）要的是
「这个文件**真能当库打开**」。`app.rs:152` 那句文档注释写着「库文件不在就建一个空的
（里面还没有表）」—— 它没说「文件在但不是库」会怎样。

> **隔离它要先绕过一步**（Y2 记了）：`build_app` 第 5 步（模型）在第 6 步（SQLite）**前面**，
> 所以得先把 `base_url` / `model` 填上才够得着这条判据。

**改法两条，选一条并说理由**：

- **(a) 折进第 ① 组**（与 X1 的 prompt 判据、Y2 的注入判据同一处）——
  试着打开一次、跑完就关。要想清楚副作用：**不许把一个空库文件建出来**
  （第 ① 组是「配置可加载」，建文件不该是它的事；X1 那条 prompt 判据是纯读）。
- **(b) 扩第 7 组的判据** —— 它已经是「落盘」那一组，语义上更贴。
  但它现在**有写副作用**（真建目录再删，V2 留的，理由写在注释里），
  再加一个「试开库」要想清楚会不会把副作用摊大。

> ⚠️ **不许新开第 8 组。** `docs/dev-spec-2026-09-11-rustgo.md:308` 冻结着「七组」，
> 而这个说法还散在 `README.md`（3 处）、`docs/acceptance-M.md`（多处，含一份逐行实测输出）、
> `docs/demo-3min.md`、`preflight.rs` 模块头、`tests/cli_smoke.rs`、
> `tests/preflight_e2e.rs`（**「七项齐、顺序固定」是硬断言**）、
> `review/inventory-gateway-evals.md`。

**照 X1/Y2 立下的五条口径**（第十节 ② 那一段逐条抄的，别另发明）：
FAIL 也把 `cfg` 交出去 / 那一组仍然只有一行结论 /「怎么补」说清三件 /
多条同时命中要**一次报齐**（Y2 的 `faults`+`fixes` 收集式写法，别撞上第一条就早退）/ 红线照旧。

**回归 + 变异**：`preflight_e2e.rs` 至少两条（非 SQLite 文件 → FAIL；正常库不误伤）、
`cli_smoke.rs` 一条（真二进制退出码）。变异至少三组，每组都要被抓到 ——
**而且把你自己的断言也变异一遍**（Y2 踩过：`fix.contains("feishu")` 对 `feishuu` 也成立，
差点造出第五条恒真断言）。

### ② `ParkedSleep` —— Y1 治好那条病的同一个形状，原样还在

```rust
// core/crates/control/tests/support/mod.rs
:229    let (tx, _rx) = watch::channel(false);   // ← _rx 当场丢掉
:258    let _ = me.parked.send(true);            // ← 零接收者时 Err 且不改值
```

**与 `StopSignal` 逐字同一个陷阱**：`watch::Sender::send` 在一个活跃接收者都没有时返回 `Err`
**并且不更新内部值**。闭包先跑到 `send(true)`、而 `wait_until_parked()` 的 `wait_for`
后到时，那个等待**永远不返回** —— 用例挂死。

Y1 的药是 `send_replace(true)`，理由（第九节 ①）：另一条路（`new()` 里留 `rx`）的全部效力
挂在一个看起来完全没用的 `_keepalive` 字段上，review 时最容易被顺手清掉，**而删掉之后是静默复活**。
**照它的判断来，别重新论证一遍。**

**要做的**：上药 + 补一条**本来就该抓住它**的断言（`send` 在任何 `wait_until_parked()`
出现之前调用，后到的等待必须立刻返回），并做「改坏 → 必须红」。

> Y1 的原话：「目前没见它抖，但窗口是真的」。所以你要**先把窗口撑开复现一次**
> （照 Y1 的手法：让闭包先跑到），拿到原病的输出再改。**只说「改好了」不算。**

### ③ 「`!status` 的健康行」—— 第 5 个副本

`core/crates/edge-client/tests/contract_gate.rs:214`：

```
/// `GetStatus` 本身不许过闸门：拦住它，闸门就永远开不了，`!status` 的健康行也问不出来。
```

**那条健康行全仓不存在。** `!status` 这条命令是真的（`control/src/plane.rs` 的 `cmd_status`），
假的是「健康行」那半句 —— 它走 `status_tasks(&ev.chat_id)`，**只从 store 列活跃任务，
从头到尾不碰 edge**。

前四个副本分别由 W2（`app.rs`）、X1（`lib.rs` 两处）、Y2（`link.rs` 两处）改过 ——
**照它们的口径改，别另起一套说法**（去读一遍那三处现在写的是什么）。
这一行前半句（`GetStatus` 不许过闸门）**是对的，留着**。

**只改注释，代码一个字不动。** 改完全仓再 grep 一遍「健康行」，**确认这是最后一个副本**
（是就写进回执；还有就列出来）。

### ④ 落 Y1 交回来的两处文档

Y1 按规矩没伸手（文档面当时归 Y2），把原文和改法写进了回执。**两处都不是错字，
是「病治好之后才配这么写」的边界补充** —— 改之前那两句在起飞半路收到信号时是假的。

| 文件 | 现在的原文 | Y1 建议改成 |
|---|---|---|
| `README.md:119` | 停机：`SIGTERM` 走优雅退出（… 宽限 20s → 还沙箱 → 关库），退出码 0；**再来一次**信号立刻硬退，退出码 130。 | 同前，句末加：**起飞还没走完时收到也算数**（compose 的 `stop_grace_period`、k8s 滚动更新都会这么来）—— 信号会被记住，起飞一走完立刻进收尾。回归见 `core/crates/app/tests/signals.rs`。 |
| `docs/acceptance-M.md` M6 第 2 步 | **停掉要重启的那个**（Ctrl-C，或 `kill <pid>`；SIGTERM 走同一条优雅退出路径）。 | 同前，补：**刚起飞就按也可以**，不必等 `aite.up` 出来 —— 信号落在起飞半路照样走优雅退出（2026-09-13 之前不是这样：那时会卡住，第 3 步的 `pgrep` 一直能看到它）。 |

**照着落，但先自己验一遍那两句成立**（起真二进制、起飞半路发 `SIGTERM`、看它自己退）——
Y1 的 `tests/signals.rs` 里那条就是干这个的，跑一遍看结论对不对得上。
措辞可以润，**事实不许改**。

### ⑤ 把「edge 完全没起来时 `aite run` 照样起飞到 `serve()`」钉住

Y1 做进程级回归时实测出来的，它在回执里点名要总管确认：

> `platform.start()` 里只有 `ingress.start()`（本地监听）是硬要求，能力表问不到只 warn 一行，
> `check_contract_version` 等满 5 次也照常返回 `Ok`。

**总管的判断：这是既有行为，合 §2.1「启动顺序无关」（两边都是懒连接 + 退避重连），不是 bug。**
问题是**全仓没有任何文档或测试写过它**，而 Y1 那条进程级回归**现在依赖它** ——
哪天有人把「edge 不可达就拒绝起飞」当成改进加进去，那条回归会莫名其妙地红，
而红的理由跟它要测的事（信号）毫无关系。

**要做的**：
1. **钉成测试** —— 一条断言「`aite-edge` 完全没起时 `aite run` 仍然走到 `ingress.listening`」。
   放 `tests/signals.rs` 还是 `build_app_contract.rs` 你定，但要让人一眼看出
   「这是刻意的，不是凑巧」。
2. **写进注释** —— `app.rs` 里 `check_contract_version` 与 `build_app` 文档注释那一带，
   说清楚：edge 不可达时起飞**继续**，依据是 §2.1；以及它对 `tests/signals.rs` 的意义。
3. **不许往 `docs/dev-spec-*.md` 里加** —— 冻结面，守卫会拦。要写就写进 `README.md`
   「两个进程」那一节（那是非冻结面，且本轨可写）。

---

## 纪律

1. **①②④ 都要先复现 / 先验，再动手。** 只说「改好了」不算 —— 原病的输出与改后的输出都要贴。
2. **新断言写完自己验一遍：把产品代码改坏，它必须红；并且把断言本身也变异一遍**
   （Y2 踩过 `contains` 的子串陷阱）。这几轮台账一共抓到四条恒真断言，别造第五条。
3. **不许新开第 8 组**，冻结面一个字别动，契约锁始终 `OK 25 files`。
4. **② 照 Y1 的判断上药**（`send_replace`），别重新论证两条路。
5. **测试数只许涨。** `cargo passed=834` 是起跑线。
6. 卡住了：要改的文件不在白名单 → 停下报告；① 的 (a)/(b) 算不过账 →
   **把两边代价写清楚**，选一条做。

## 验收

```bash
scripts/check.sh                                          # 「全部通过」，退出码 0
cd core && cargo test -p aite --test preflight_e2e         # ① 的主战场
cd core && cargo test -p aite --test cli_smoke
cd core && cargo test -p aite --test signals               # ④⑤ 要用
cd core && cargo test -p aite-control                      # ② 的主战场
cd core && cargo test -p aite-edge-client                   # ③ 别弄坏
cd core && cargo clippy --workspace --all-targets -- -D warnings
```

外加**自己交叉验一遍**：

- [ ] ① 复现过 Y2 那三格（`--offline` / 全跑 / `aite run`），改后再跑一遍，六格都填了。
- [ ] ① 选的那条路说明了副作用（尤其 (a) 不许把空库文件建出来）。
- [ ] ② 先撑开窗口复现了原病（用例挂死），改后复现不出来。
- [ ] ③ 全仓 grep「健康行」，确认是最后一个副本（或列出还剩哪些）。
- [ ] ④ 那两句改之前自己验过（起真二进制、起飞半路发信号）。
- [ ] ⑤ 的测试让人一眼看出「这是刻意的」，注释里写了依据是 §2.1。
- [ ] 每条新断言都做过「改坏产品代码 → 必须红」+「断言自己变异一遍」。

## 回执格式

在回复里写（**不要**新建回执文件；台账只许追加**「十一、Z1 回执」**这一节）：

```
## Z1 回执

基线 : <开场自检那个 sha>
check.sh : <五行关键值>  退出码 <n>
假红 : <撞到没有；撞到了贴失败名 + 单独跑的结果>
守卫 : Read .claude/hooks/guard_bash.py 被拦 ✅

### ①–⑤ 逐条
（每条：病在哪 / 判据 / 改法 / 为什么选这条 / 钉它的测试 / 改坏验过没 / 断言本身变异过没）

### ① 的六格对照
| 配置 | preflight --offline | preflight 全跑 | aite run |
| 非 SQLite 文件 改前 / 改后 | | | |

### ② 原病复现记录
（撑开窗口之后挂死的输出原样 + 改后的输出）

### ③ 「健康行」全仓 grep 结果
（是不是最后一个副本）

### ⑤ 钉法
（测试放哪、注释写了什么、README 加了没）

### 测试数
834 → <N>，每条多在哪

### 记账转出去的
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
```
