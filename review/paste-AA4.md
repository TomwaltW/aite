# 任务 AA4 — 把那句谎话连根拔：源头在冻结的 proto 里

## 背景：这轨是从哪来的

**「`!status` 会显示 edge 侧的东西」这个错误信念，在这个仓库里活了很久。**
它最常见的说法是「`!status` 的健康行」—— 那条健康行**全仓不存在**：
`!status` 由 `core` 的 `control::cmd_status` 答（`plane.rs:587`），走 `status_tasks(&ev.chat_id)`，
**只从 store 列活跃任务，从头到尾不碰 edge**（函数体内 edge 引用数 = 0，Z3 复核过）。

这个说法一共 **7 个副本**，W2 / X1 / Y2 / Z1 / Z3 五轮清完了。**但 Z3 收口时发现，
同一句谎话换了个词还活着** —— 而剩下的两处都在**冻结面**里，谁都伸不了手：

| 位置 | 原话 | 为什么是同一句谎话 |
|---|---|---|
| `proto/aite/v1/edge.proto:151` | `// edge 自身健康：给 !status / preflight 用。` | **这是源头。** `preflight` 那半句是**对的**（第 6 组先问 daemon 可达）；`!status` 那半句是错的。两个 `.pb.go` 副本（`edge_grpc.pb.go:831` 与 `:858`）就是从它生成的 |
| `edge/cmd/aite-edge/main.go:58` | `// platform: fake 时压根没起长连接，connected 恒 false —— 别让 !status 误报「飞书在线」。` | `platform_connected` → `EdgeStatus` → 被 `app.rs:388` 打成日志、被 preflight 第 6 组读。**到不了 `!status`** |

**判据是核过的，不是推的**（Z3 记的）：`!status` 唯一会多报的计数是 core 控制面自己的
`events.dropped`（`plane.rs` 的 `dropped_note`，取自 `self.shared`），**edge 侧的计数一个都到不了**。

总管 __TODAY__ 拍板：**动**。这一轨就是把它连根拔掉。

## ⚠️ 这一轨的处境：你要改的每一个文件，守卫都拦着

四个目标文件里，**三个在冻结面**（`core/crates/contracts/tests/layout.rs` 的 `frozen_paths_exist`
列着，`.contracts.lock` 锁着，守卫的 `PROT_PATHS` 拦着）：

```
proto/aite/v1/edge.proto          ← 要改的那一行注释
edge/gen/aitepb/edge_grpc.pb.go   ← 生成产物，注释从 proto 抄下来
edge/cmd/aite-edge/main.go        ← 另一处
.contracts.lock                   ← 读和写都拦；重锁要 AITE_RELOCK=1
```

**所以你的交付形状是补丁脚本 + 一条给人跑的命令链，不是直接改。**
骨架照 `review/z2-guard-patch.py`：逐条精确替换、锚点必须唯一命中否则整份拒写、
语法/格式合法性检查、落盘后读回复验、没有 `AITE_RELOCK=1` 当场拒绝执行。

**但本轨的实质工作不在那份脚本上**（改的就是两行注释）。**在 ② 的可复现性验证上** ——
见下。

## ⚠️ 第二件事：本机没装 Go 侧的 codegen 插件

```
$ protoc --version
libprotoc 36.1                    ← 有
$ which protoc-gen-go protoc-gen-go-grpc
protoc-gen-go: command not found  ← 没有
protoc-gen-go-grpc: command not found
```

而 `edge/gen/aitepb/*.pb.go` 的文件头钉着生成它们的版本：

```
protoc-gen-go      v1.36.12
protoc-gen-go-grpc v1.6.2
protoc             v7.36.1
```

**装的时候必须对上这三个版本。** 版本不一致 → 重跑 codegen 会把整个文件按新版本的风格重写
→ 两个冻结文件面目全非 → 契约锁那一步变成一场灾难，而且**没人看得出哪些变化是本轨要的、
哪些是工具版本带来的**。这是本轨最大的风险。

> 注：文件头的 `protoc v7.36.1` 与本机 `libprotoc 36.1` 是同一个东西的两种版本号写法
> （protobuf 从 v4.x 起 `libprotoc` 报次版本号）。**这一条你要自己验证**，别信我这句话 ——
> 验法见 ②。

## 必读（按顺序）

1. **`proto/aite/v1/edge.proto`**（只读）——`:151` 那一行与它下面的 `EdgeStatusService`。
   看清楚 `EdgeStatus` 里到底有哪些字段，好把改后的注释写准。
2. **`edge/gen/aitepb/edge_grpc.pb.go:831` 与 `:858`**（只读）—— 那句话的两个生成副本，
   确认它们确实是从 proto 的 leading comment 抄下来的（**这决定了改 proto 就够，不用手改产物**）。
3. `Makefile:47-48` —— `proto-gen` target 的原文。注意它的注释：
   「只有 R0/RΩ 在 `AITE_RELOCK=1` 下跑；Rust 侧由 `build.rs` 自动生成」。
4. `core/crates/proto/build.rs` —— Rust 侧走 `tonic_prost_build`，**每次重编自动生成**，
   产物不入库。所以 Rust 侧不需要你做任何事，但**它要 protoc 在 PATH 上**。
5. `core/crates/contracts/tests/layout.rs` 的 `frozen_paths_exist` —— 冻结面清单。
6. **`review/z2-guard-patch.py`** —— 补丁脚本的骨架，照它来。
7. `review/review-findings-2026-09-12-vmerge.md` 的「十三、Z3 回执」④ —— 这笔账的出处，
   连同「同一句谎话的四种说法」那张表。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-aa4
分支     : task-aa4
基线     : __BASE__
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-aa4
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
> 那说明 `.claude/settings.json` 的修正那一格没进 git，**停下来喊人**。

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明守卫在静默失效，
> **停下报告**。

## 要干的活

### ① 把那两行注释该改成什么，先定下来

**`proto/aite/v1/edge.proto:151`** —— `preflight` 那半句留着（它是对的），
`!status` 那半句要换成**真实的消费者**。先自己 grep 清楚 `EdgeStatus` / `GetStatus` 到底谁在读
（Z3 给的三个 Rust 调用方是 `app.rs` 的 `check_contract_version`、`preflight.rs` 第 6 组、
`wiring.rs` 的评测接线 —— **复核一遍，别照抄**），照着写。

**`edge/cmd/aite-edge/main.go:58`** —— 口径与 `edge/internal/ingress/client.go` 那三处对齐
（Z3 改的，`:40` / `:129` / `:202`），**别另起一套说法**。

两处都**只改注释，一行代码都不动**。改后的文本写进回执，让总管能先看一眼再决定跑不跑补丁。

### ② codegen 可复现性验证（本轨的主体）

**目标**：证明「装上正确版本的插件、重跑 `make proto-gen`，两个 `.pb.go` 除了那一行注释之外
**一个字节都不变**」。没有这条证明，总管不能跑那条命令链。

**怎么验而不碰冻结面** —— 在 `/tmp` 下做一份仓库副本，在副本里改、在副本里跑 codegen、
在副本里 diff：

- **`git worktree` 不行**：worktree 在仓库路径下，守卫照拦。用 `git archive` 或 `cp -R` 弄到
  `/tmp` 下的一个**目录名不含受保护路径子串**的地方；
- ⚠️ **小心命令里的路径字面量**：守卫扫的是命令文本里的受保护路径**子串**，
  `/tmp/xxx/proto/aite/v1/edge.proto` 里含 `proto/aite/v1/edge.proto` —— **会被误拦**。
  用变量拼路径，或者把副本的目录结构起个不一样的名字。这是已知的固有代价
  （见 `tests/guard.rs` 模块头那张误拦表第 4 行），撞上了换写法，**别碰守卫**。

**要拿到的三条证据**：

1. **不改任何东西、直接重跑 codegen，产物逐字节不变** —— 这一条先跑。
   变了就说明工具版本不对，**停在这里**，先把版本弄对再往下走；
2. **改了那一行注释之后重跑，`git diff` 只有注释行**（Go 侧两处 + proto 一处，无别的）；
3. **版本三元组逐字对上**（`protoc-gen-go v1.36.12` / `protoc-gen-go-grpc v1.6.2` /
   `protoc v7.36.1`）—— 装完之后 `--version` 的原始输出贴进回执。

**装插件的命令也要记进回执**（总管要在真仓库上跑同一套，装的东西必须一致）。

### ③ 补丁脚本 `review/aa4-proto-patch.py`

骨架照 `review/z2-guard-patch.py`。这一份要改两个文件，所以：

- 两条 `Edit`，**各自锚点唯一命中**，任一条对不上就整份拒写（别加 `--partial` 的侥幸）；
- 改完 `.proto` 与 `.go` 都要做**格式/语法可解析性检查**（Z2 那份查的是 JSON 合法性，
  你这份至少要保证 `gofmt -l` 空、proto 能被 `protoc` 解析）；
- 落盘后读回复验；
- **没有 `AITE_RELOCK=1` 当场拒绝**；
- **`--root <临时目录>` 自验模式**，对着 ② 那份副本跑一遍，矩阵贴进回执。

**注意 `.pb.go` 那两处不进补丁脚本** —— 它们是生成产物，由 `make proto-gen` 重跑出来。
补丁脚本手改生成产物 = 下一次 codegen 把你的改动冲掉，且没人知道。

### ④ 给总管的命令链

写一条完整的、按顺序的、每步都有判据的命令链，放在回执末尾。大致形状（你要补全并实测每一步的期望输出）：

```bash
# 0. 装插件（版本见 ②）
# 1. 干跑补丁
AITE_RELOCK=1 python3 review/aa4-proto-patch.py --check
# 2. 真写
AITE_RELOCK=1 python3 review/aa4-proto-patch.py
# 3. 重生成 Go 侧产物
make proto-gen
# 4. 此时契约锁必红，且只该点名那两/三个文件 —— 先看清楚再重锁
core/target/debug/aite contracts lock --check
# 5. 重锁
AITE_RELOCK=1 core/target/debug/aite contracts lock --write
# 6. 全量
scripts/check.sh
```

**第 4 步是最要紧的一道闸**：重锁之前必须先看清楚锁在抱怨哪些文件。
如果它点名的不止你改的那几个，**说明有别的东西被动了**，那时候重锁就是把问题锁进去。
把「第 4 步该看到什么」写成一条明确的期望，总管照着核。

### ⑤ 收口检查

改完之后再 grep 一遍：`!status` 在**源码里**（排除 `review/`）还有没有别的说法在断言
它能看到 edge 侧的东西。Z3 数过改前 12 处 / 落类过，**你要在改后重新数一遍**并逐条落类，
别只 grep「健康行」三个字 —— Z3 就是因为多 grep 了一层才发现这两处的。

## 纪律

1. **可写面**：`review/aa4-proto-patch.py`（新建）、
   `review/review-findings-2026-09-12-vmerge.md`（只许**追加**你自己那一节）。
   **本轨在 worktree 里一行产品代码都不改** —— 所有改动都通过补丁脚本、由人落地。
2. **只读面 / 冻结面**：其余一切，尤其 `proto/**`、`edge/gen/**`、
   `edge/cmd/aite-edge/main.go`、`.contracts.lock`。守卫会拦，**别试着绕**。
3. **`AITE_RELOCK=1` 不许自己用**（守卫故意拦自我授权，`relock_and_self_authorization_are_blocked`
   钉着）。`/tmp` 下的副本不在保护面里，那边随便跑 —— **补丁脚本的 `--root` 自验模式就是为这个设计的**。
4. **`.claude/**` 一个字节都不碰**（读也不行）。
5. **落盘无残留**：`/tmp` 下的副本、装插件产生的东西，收尾前交代清楚（副本可以留着让总管复核，
   但要在回执里写明路径）。worktree 里 `git status` 必须只有你那两个文件。

## 验收

```bash
scripts/check.sh        # 五行关键值与开场逐字相同（本轨在 worktree 里零产品代码改动）
```

**本轨的真正验收在副本里**，即 ② 的三条证据。check.sh 只是证明你没碰到不该碰的。

> ⚠️ **补丁跑完之后的验收不归你**（那要 `AITE_RELOCK=1`）。你要做的是把 ④ 那条命令链的
> **每一步期望输出**写准，让总管跑的时候能一眼看出哪步不对。

## 回执

写进 `review/review-findings-2026-09-12-vmerge.md`，**追加**一节「十七、AA4 回执 —— __TODAY__」
（**只追加，别动别人的节**）。要有：

- 基线与开场自检（五行关键值 + 守卫拦截那一条）；
- ① 两处注释的**改后全文**（总管要先看再决定跑不跑）；
- **② 的三条证据**（本轨主交付）：装插件的命令与版本原始输出、「不改也重跑、产物逐字节不变」
  的验证命令与结果、「改了之后只有注释行变」的 diff 逐字；
- ③ 补丁脚本的 `--root` 自验矩阵；
- ④ 完整命令链，**每步带期望输出**，尤其第 4 步该看到哪几个文件；
- ⑤ 改后重新数的 `!status` 落类表；
- **记账转出去的**（表格：位置 / 病 / 归哪轨）；
- **没做的 / 拿不准的**（编号列表。**本轨最大的诚实风险是把「副本里验过」说成「真仓库验过」**
  —— 这两件事差一个量级，说清楚）。
