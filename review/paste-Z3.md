# 任务 Z3 — 第 ① 组现在管四件事而四处文档还写三件；外加「`!status` 健康行」最后两个副本

## 背景：这轨是从哪来的

**这一轨全是文字活，零行为改动，但它修的是「文档说的和代码做的不一样」——
而总管接下来就要照着这几份文档跑 M1–M6。**

`aite preflight` 第 ① 组「配置可加载」在三轮里被折进了三件事：

| 谁 | 折进了什么 | 病史 |
|---|---|---|
| X1（2026-09-13） | `worker.system_prompt_path` 指到的文件真的在 | 旧配置指着 2026-09-12 删掉的 Python 树时，改前七组一组都不碰它，preflight 报「可以起飞」而 `aite run` 退出码 2 |
| Y2（同日） | `platform` / `model.provider` 的取值不需要注入 | `platform: fake` 的配置 `--offline` 报「可以起飞」退出 0，而 `aite run` 退出码 2；`provider: scripted` 同病 |
| **Z1（同日）** | **`storage.sqlite_path` 上已有的那个文件真能当库打开** | 指着一个非 SQLite 文件时，`--offline` 与全跑**都全绿**，`aite run` 退出码 2 `建表失败` |

**所以它现在管四件事**（yaml 解析 + 上面三条）。Z1 那一轨被派单钉住了文档面
（「只许改点名的那两处」），**没伸手**，于是四处文档还停在「三件事」——
逐字写着「三件事一次报齐」并只列了 prompt / 注入两条。

另一半是一句谎话的最后两个副本。「`!status` 的健康行」**全仓不存在** ——
`!status` 这条命令是真的（`control/src/plane.rs` 的 `cmd_status`），假的是「健康行」那半句：
它走 `status_tasks(&ev.chat_id)`，**只从 store 列活跃任务，从头到尾不碰 edge**。
这句话一共 **7 个副本**，W2 / X1 / Y2 / Z1 各清掉一批，**还剩 2 个**，都在他们的可写面外。

## 必读（按顺序）

1. **`review/review-findings-2026-09-12-vmerge.md` 第十一节（Z1 回执）的「记账转出去的」表** ——
   本轨前四行就是那张表的第 1、2、3、4 行，连**建议的补法原文**都在里面。
   第八节（X1）与第十节（Y2）的 ① 是那两条病史的出处。
2. `core/crates/app/src/preflight.rs` 的**模块头那张表**（1–45 行）——
   **它是唯一已经写对四件事的地方**（Z1 改过），你要让另外四处跟它对齐。
   **以它为准**，不要自己重新组织措辞。
3. `core/crates/edge-client/src/link.rs:145-151` 与 `core/crates/edge-client/src/lib.rs:81/99`
   —— Y2 / X1 改好的那几处「健康行」副本。**②③ 照它们的口径改，别另起一套说法。**
4. `docs/dev-spec-2026-09-11-rustgo.md` line 308（「`aite preflight`（**七组**自检）」）。
   **冻结、只读**，守卫会拦 —— 提醒你：**「七组」这个说法一处都不许改成八组**。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-z3
分支     : task-z3
基线     : f3bc017（= merge(task-z1) 并入 main 那一格）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-z3
git log --oneline -1        # 期望 f3bc017（若多一格、只加了本派单文件，那也对）
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（总管 2026-09-13 在 `task-z2` 那个 worktree 里实跑过，同一格代码，原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=852 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

> **本轨零行为改动，所以收尾时这五行必须逐字不变。** 变了说明你改到了不该改的地方。

> ⚠️ 那三个抖动 target 的病根都治过了，Z1 那轮开场收尾各跑一次都没撞到。
> 撞到是新信息，贴进回执（判据：单独跑一遍那个 target，绿就是假红）。

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明守卫在静默失效，
> **停下报告**。

> 💡 **② 改的是 Rust 文件，所以会撞上一条**（Z1 收尾时撞过）：`cargo fmt --all`（写模式）
> **会被守卫拦** —— 它会碰 `core/crates/contracts/**` 这个冻结面。解法：对你改过的那个文件
> 直接跑 `rustfmt --edition 2024 <file>`，然后 `git status` 复核没动到别的文件。
> ③ 改的是 Go 文件，收尾要 `gofmt`，那条不受影响。
>
> 其余已知误拦（换写法，别碰守卫）：命令里出现受保护路径**字面量**（`git add` 用 `-u`）；
> heredoc 正文里有配不平的引号、中文引号、markdown `**` 加粗、或正文太长
> （→ 改用 Write 工具落文件）；`find … -delete` / `-exec`（→ `ls` + 点名 `rm -f`）。

## 可写路径

> **Z2 与你同时在跑。** 下面这张表两轨**逐字相同**，是同一份划分 ——
> W2/W3 那一轮出过「同一个文件在两份派单里互相指给对方」的事故（`cli.rs` 因此谁都没改、
> 留下两条反的注释），所以这次把边界写死在两边。

| 面 | Z2 | Z3（你） |
|---|---|---|
| `review/z2-guard-patch.py`（Z2 新建） | **读写** | 别碰 |
| `core/crates/app/tests/guard.rs` | **读写** | 只读 |
| `core/crates/edge-client/src/gate.rs` | 只读 | **读写**（②，只许改注释） |
| `edge/internal/ingress/client.go` | 只读 | **读写**（③，只许改注释） |
| `README.md`、`docs/acceptance-M.md`、`docs/demo-3min.md`、`review/inventory-gateway-evals.md` | 只读 | **读写**（①） |
| `.claude/hooks/guard_bash.py`、`.claude/settings.json` | 读写不了（守卫 `readable=False`） | 同 |
| `core/crates/app/src/**`、`core/crates/control/**`、`core/crates/edge-client/src/`（除 `gate.rs`）、`edge/`（除 `client.go`） | 只读 | 只读 |
| `.github/**`、`Makefile`、`scripts/**`、`docker-compose.yml`、`docker/**` | 只读 | 只读 |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` | **冻结** | **冻结** |

**文档面归你**，包括 Z2 那一轨想改而没伸手的部分 —— **它会把原文 + 改法写进它的回执**，
由总管在合并时落，**不要你去猜它想改什么**，也别主动去改守卫相关的文档。

**台账：只许追加一节「十三、Z3 回执」。** Z2 追加的是「十二」。

---

## 要做什么

### ① 四处文档：第 ① 组从「三件事」改成「四件事」

**基准是 `core/crates/app/src/preflight.rs` 的模块头那张表**（Z1 已经写对）。
四处要对齐的位置（行号自己 `grep` 确认，本轨之前已经漂过好几轮）：

| 文件 | 现在写的 |
|---|---|
| `README.md:143` 一带 | 「第 1 组问的是『这份配置**起得来**吗』而不只是『yaml 解析得出来吗』，**三件事**一次报齐：…」+ 下面那个病史块只有两条 |
| `docs/acceptance-M.md:125-126` / `:154` 一带 | §0.1 第 ① 组的括注 + 末尾 ⚠️ 块（现在两条病史） |
| `docs/demo-3min.md:119-124` 一带 | 第 1 组那段 ⚠️ 只说了 prompt 与 fake/scripted 两族，没有 sqlite 这族 |
| `review/inventory-gateway-evals.md:142` | 那一长行里第 1 组的逐条描述，只列了两条 |

**Z1 在回执里给了建议补法的原文**，每处加一句：

> 以及 `storage.sqlite_path` 上已经有的那个文件真能当库打开（读得到但不是库是 FAIL；
> 病史：2026-09-13，`--offline` 与全跑都全绿而 `aite run` 退出码 2 `建表失败`）

**照它落，但三件事要自己做**：

1. **先把事实核一遍**（别照抄病史）：拿一份 `sqlite_path` 指着非 SQLite 文件的配置，
   实跑 `preflight --offline` / `preflight` / `aite run` 三格，确认现在的行为与那句病史对得上。
   **这一步的输出要贴进回执。**
2. **`docs/acceptance-M.md` §0.1 里那份逐行实测输出要重跑一遍再判**（X1 / Y2 各重跑过一次，
   结论都是「不用改」——因为 `config/aite.yaml` 由样例复制、三条判据都不命中）。
   **你这一轮同样要跑，别继承上一轮的结论。**
3. **「七组」一处不许动。** 这是硬约束：`docs/dev-spec-2026-09-11-rustgo.md:308` 冻结着
   「七组」，而这个说法还散在 `README.md`（3 处）、`docs/acceptance-M.md`（多处）、
   `docs/demo-3min.md`、`preflight.rs` 模块头、`tests/cli_smoke.rs`、
   `tests/preflight_e2e.rs`（**「七项齐、顺序固定」是硬断言**）、本轨改的那份 inventory。
   改完 `grep -c` 核一遍数没变。

> **顺带判一件事**：四处的措辞要不要统一成同一句？`README` 面向「怎么跑起来」、
> `acceptance-M` 面向排障、`demo-3min` 面向演示、`inventory` 是清单 ——
> 四种读者。**别为了统一把话说得都一样**；判断写进回执。

### ② `core/crates/edge-client/src/gate.rs:18` —— 第 6 个副本

```
//! 和健康行走的那条）每答上来一次也顺手记一次。比出不一致就落闸，此后每发 platform /
```

括号里那半句（`EdgeClient::status()`「起飞体检**和健康行走的那条**」）在断言健康行存在。

**真调用方**（Y2 / Z1 都 grep 过，你再核一遍）：`status()` 有三个 ——
`app.rs` 的 `check_contract_version`（起飞比版本）、`preflight.rs` 第 6 组（沙箱可用，先问 daemon
可达）、`wiring.rs` 的评测接线。**没有健康行。**

按 `link.rs:145-151` 的口径改（去读一遍那处现在怎么写的）。**只改注释，代码一个字不动。**

### ③ `edge/internal/ingress/client.go:200` —— 第 7 个副本，Go 侧

```go
// 跑通了，`!status` 的健康行会在这中间报「没连上」。
```

**这条特别值得说一句**：那段注释自己写着「core 侧 R6 的 `link.note_ok()` 是同一个修法，
这边补齐」—— 也就是说**连病一起抄过去了**。Y2 改掉了 core 侧那个，Go 侧这个没人管。

同样只改注释。改完 `gofmt -l .` 必须为空。

### ④ 收口：确认这是最后两个

改完**全仓 grep 一遍「健康行」**（排除 `review/`，那里面是历史记录不用动）。
现在有 40 处命中，Z1 说其余都是**已经改正、正在解释病史**的文本
（`app.rs:83`、`lib.rs:81/99`、`link.rs:87/88/94/149`、`contract_gate.rs:217`）。

**逐条过一遍，把每一处落到两类之一**：
- 「在解释病史 / 已经说准了」→ 不动；
- 「还在断言健康行存在」→ **那就是第 8 个副本，你漏了**，改掉。

结论写进回执：**这句谎话一共几个副本、现在全清了没有**。

---

## 纪律

1. **本轨零行为改动。** 收尾 `check.sh` 的五行关键值必须逐字不变（`cargo passed=852` 也不变，
   因为不加测试）。
2. **①.1 要先实跑核事实**，别照抄 Z1 的病史原文。
3. **①.2 那份实测输出要自己重跑**，别继承上一轮「不用改」的结论。
4. **「七组」一处不许改**，冻结面一个字别动，契约锁始终 `OK 25 files`。
5. **②③ 只改注释。** 代码一个字不动 —— 尤其别顺手「清理」那些注释提到的函数
   （`contract_state()` 在产品代码里零调用方，X1 记过；**要不要删不是本轨的决定**）。
6. **别越界**：`guard.rs` 与 `.claude/**` 相关的一切归 Z2，它同时在跑。
7. 卡住了：④ 里判不出某处属于哪一类 → **列出来写进回执**，别硬判。

## 验收

```bash
scripts/check.sh                    # 「全部通过」，退出码 0；五行关键值逐字不变
cd edge && gofmt -l .               # ③ 之后必须为空
cd edge && go vet ./...
rustfmt --edition 2024 crates/edge-client/src/gate.rs   # ② 之后；别用 cargo fmt --all
core/target/debug/aite preflight --offline              # ①.2 要用
```

外加自己交叉验一遍：

- [ ] ①.1 三格实跑过（非 SQLite 文件 × `--offline` / 全跑 / `aite run`），输出贴了。
- [ ] ①.2 `acceptance-M.md` §0.1 那份逐行输出重跑过，逐字比对了。
- [ ] ① 四处与 `preflight.rs` 模块头那张表**说的是同一件事**（措辞可以按读者调，事实不许差）。
- [ ] 「七组」`grep -c` 改前改后数一样。
- [ ] ②③ 只动了注释（`git diff` 自己看一眼，没有代码行）。
- [ ] ④ 40 处命中逐条落类，结论是「一共 N 个副本、现在全清了」或「还剩哪些、归谁」。

## 回执格式

在回复里写（**不要**新建回执文件；台账只许追加**「十三、Z3 回执」**这一节）：

```
## Z3 回执

基线 : <开场自检那个 sha>
check.sh : <五行关键值>  退出码 <n>
假红 : <撞到没有>
守卫 : Read .claude/hooks/guard_bash.py 被拦 ✅

### ①.1 事实核对（三格实跑）
| 配置 | preflight --offline | preflight 全跑 | aite run |

### ①.2 acceptance-M §0.1 那份实测输出
（重跑结论：改 / 不用改，逐字比对情况）

### ① 四处改了什么
| 文件 | 原话 | 改成 | 措辞为什么这么调（四种读者） |

### 「七组」计数
改前 <n> 处 / 改后 <n> 处

### ②③ 两个副本
（真调用方 grep 结论；改成什么；git diff 里有没有代码行）

### ④ 收口
（40 处命中逐条落类；这句谎话一共几个副本、清完了没）

### 测试数
852 → 852（本轨不加测试）

### 记账转出去的
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
```
