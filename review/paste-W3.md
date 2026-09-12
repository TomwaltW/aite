# 任务 W3 — `--help` 被吞掉、两个纯函数在门禁里裸奔、`Release("")` 三方矛盾，外加容器以 root 跑

## 背景：这轨是从哪来的

V1–V6 六轨 + W1 已全部合进 `main`。**P0 的代码面收完了** —— 没兑现的只剩
`docs/dev-spec-2026-09-09.md` §2.4 / `docs/dev-spec-2026-09-11-rustgo.md` §4.4 的
**M1–M6 真机验收**与 **3 分钟演示**，那是总管自己的活。

这一轨收的是台账里剩的 edge 侧账 + 一条**归口 R0 至今没人接**的横切账。
四条里有两条值得单独说一句：

- **`--help` 那条不是小事**：`aite run --help` 和 `aite preflight --help` 打出来的是
  不含任何真实选项的帮助，**而且退出码是 0** —— 脚本判不出来，人眼也未必看得出来。
  V6 ④b 只修了多打一个 `--` 的写法，根治要动 `main.rs`，归口 R0，一直没人接。
  W1 刚在 `docs/acceptance-M.md` 里拿一张四行实测表把现状写死了（带 `<!-- 台账 -->`），
  **你修完要回去改那张表**。
- **Dockerfile 没有 `USER` 那条是 W1 新报的，只在 macOS 上验过。** Linux 上
  `data/evidence` 会是 root 拥有，宿主机那条 `aite evidence show` 可能读不了 ——
  而 CI runner 就是 Linux。**先证伪或证实，再决定改不改。**

## 必读（按顺序）

1. **`review/review-findings-2026-09-12-vmerge.md` §4.2**（本轨的三条）**+ §五 W1 回执
   末尾那张「记账转出去的」表**（第 3、4 行是本轨新增的两条）。
2. `review/review-findings-2026-09-12-romega.md` §4.4 的「记账-Go」两行 —— 这两条账
   最初立在哪。
3. `docs/acceptance-M.md` **§0.1 那张四行 `--help` 实测表**（W1 刚写的，带
   `<!-- 台账：不带 -- 的两个写法仍未修，归 R0 / W3 -->`）—— ③ 改完要回来改它。
4. `docs/dev-spec-2026-09-11-rustgo.md` §2.2（错误约定，**冻结**）、§5 归属表。
   `proto/aite/v1/edge.proto` 的**头注释**与 `Release` 那一段 —— ① 的三方矛盾里，
   proto 注释是第三方。**proto 冻结，只读，守卫会拦。**
5. `.github/workflows/ci.yml` 的 `compose-smoke` job 全文 —— ④ 要在这儿加判据。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-w3
分支     : task-w3
基线     : abd93da 的代码面（worktree 建在它上面）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
宿主机   : darwin/arm64；CI runner 是 linux/amd64。**⑤ 那条的分歧正在这里。**
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-w3
git log --oneline -1        # 记下 sha，回执里当基线
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0
```

**关键行期望**（总管 2026-09-12 实跑过，原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=793 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

另外单独跑一次（不在 check.sh 里，① ② 都要用）：

```bash
cd edge && go test -tags docker ./internal/sandbox/... -count=1   # 期望 ok
docker ps -a --filter label=aite.task -q | wc -l                  # 期望 0
```

> ⚠️ **已知假红，别当回归。** `-p aite --test` 的 `startup_recovery` /
> `graceful_shutdown` / `reconnect_replay` 三个 target 会偶发红一条
> （2026-09-12 记录在案四次，**单轨也会撞上**）。判据：把
> `error: test failed, to rerun pass …` 点名的 target **单独跑一遍**，绿就是假红。
> **修它归 W2，不归你** —— 后两个的抖动源正是 W2 的 ⑤ 和 ⑥。
>
> ⚠️ **W2 与你同时在跑**，它的可写面是 `core/crates/app/src/{run,app}.rs` +
> `core/crates/app/tests/{crash_recovery,reconnect_replay,graceful_shutdown}.rs` +
> `core/crates/control/**`。docker 档沙箱测试在两轨都跑真容器时会互相挤，
> 红了先单独跑一遍再说。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。

## 可写路径

| 面 | 权限 |
|---|---|
| `edge/internal/sandbox/**` | 读写 |
| `core/crates/app/src/main.rs` | 读写（③ 的主战场） |
| `core/crates/app/tests/cli_smoke.rs` | 读写（③ 的回归在这儿） |
| `.github/workflows/ci.yml` | 读写（④⑤） |
| `docker/core/Dockerfile`、`docker/edge/Dockerfile` | 读写（⑤，**先证实再改**） |
| `docs/acceptance-M.md` | **只许改 §0.1 那张 `--help` 表**，别的段落别碰 |
| `README.md` | 只许改 ③⑤ 波及的句子 |
| `core/crates/app/src/{run,app,cli,wiring}.rs`、`core/crates/control/**` | **只读 —— 归 W2** |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` | **冻结**，守卫会拦 |
| `docker-compose.yml`、`Makefile`、`scripts/check.sh`、`.claude/**` | 只读 |

---

## 要做什么

### ① `docker.go:381-397` —— `Release("")` 与三处说法矛盾

```go
// Release 幂等：不认识的 id、已经没了的容器，都当成已经释放。
func (d *Docker) Release(ctx context.Context, sandboxID string) error {
	d.mu.Lock()
	_, known := d.boxes[sandboxID]
	delete(d.boxes, sandboxID)
	d.mu.Unlock()

	cli, err := d.dockerClient()
	if err != nil { return err }
	if err := cli.ContainerRemove(ctx, sandboxID, container.RemoveOptions{Force: true}); err != nil {
		if client.IsErrNotFound(err) { return nil }
		return dockerErr(err, aiteerr.SandboxInternal, "释放沙箱失败（%s）：%v", short(sandboxID), err)
	}
	…
```

空 id 走到 `ContainerRemove(ctx, "", …)`，daemon 回的**不是** `IsErrNotFound`，
于是返回 `SandboxInternal`。三处说法互相矛盾：
**函数自己的注释**（`:381`）、**包头注释**（`docker.go:16` 那行「Release 幂等」）、
**`proto/aite/v1/edge.proto` 里 `Release` 那一段的注释**（proto 冻结，只读 ——
它说什么就是契约说什么）。

**第一步是判定谁对**：去 proto 里读那段注释，它规定的语义是什么。
- 如果契约说「空 id / 不认识的 id 都当成已释放」→ **改实现**（早返回，别走 daemon），
  补测试（纯函数档，不需要 daemon）。
- 如果契约说的是别的 → **改两处注释**说实话，并且**照样补测试**把现行行为钉住。

**不许**「注释和实现各留一半」—— 现在就是那个状态。

### ② `docker_pure_test.go` —— 两个函数在无 daemon 门禁里裸奔

RΩ 已经把 `clip` / `diffFiles` / `parseDockerTime` / `reapVictims` / `short` 纯化进了
无 daemon 门禁（13 条测试）。**漏了两个**：

- **`inspectStamps`（`docker.go:765`）** —— 自由函数，入参是 `container.InspectResponse`，
  **完全纯**，造个结构体就能测。现在一条测试都没有。
- **`orphans`（`docker.go:723`）** —— 是 `*Docker` 的方法且要问 daemon，只能部分纯化。
  台账原话是「台账点名的 orphans 仍只在 docker tag 下被跑到」。
  **先看能不能把判定逻辑（哪些算孤儿、idle 怎么算）抽成纯函数**再测；
  抽不动就说明理由，别硬拆。

**为什么要紧**：CI runner 上没有 daemon，整份 `docker_test.go` 压根不编译
（`//go:build docker`）。这两个函数改错了**门禁一声不响**。

给 `inspectStamps` 的测试至少覆盖：正常时间戳、零值哨兵、字段缺失、时间格式非法。

### ③ `main.rs` —— clap 把 `--help` 吞了（归口 R0，至今没人接）

```rust
// main.rs:23 / :33 / :38 / :43 —— 四处
#[arg(trailing_var_arg = true, allow_hyphen_values = true)]
```

`allow_hyphen_values` 让 `--help` 被当成普通位置参数收进 `ARGS`，于是 clap 不再
生成帮助，打出来的是一句不含任何真实选项的 `[ARGS]...`。**实测（当前 main）**：

| 写法 | 打出什么 | 走哪 | 退出码 |
|---|---|---|---|
| `aite preflight -- --help` | ✅ 手写用法 | stdout | 0 |
| `aite run -- --help` | ✅ 手写用法 | stdout | 0 |
| `aite preflight --help` | ❌ 只回 `[ARGS]...` | stdout | **0** |
| `aite run --help` | ❌ 同上 | stdout | **0** |

**退出码 0 是最坏的部分** —— 坏掉的帮助和好的帮助在脚本里长得一模一样。

`trailing_var_arg` 这套是**刻意**的（子命令的参数由各自的手写解析器处理，不是 clap），
所以**别把它整个拆掉** —— 那会动 `cli.rs` / `wiring.rs`（归 W2 / 只读）。
在 `main.rs` 这一层认出 `--help` / `-h` 并转给手写 usage 就够了。

**三条硬约束**：
1. `aite run -- --help` / `aite preflight -- --help` 现在的行为（stdout + 0）**一个字节都不许变**
   —— V6 刚修好的，`core/crates/app/tests/cli_smoke.rs` 钉着；
2. **`--help` 不能被误当成业务参数吃掉** —— 比如将来某个子命令真有个叫 `--help` 的
   传参场景（现在没有，但要想清楚边界并写进注释）；
3. `aite evals` / `aite contracts` 那两个子命令（`:38` / `:43`）**一视同仁**，
   别只修 run 和 preflight。

**回归写在 `core/crates/app/tests/cli_smoke.rs`（进程级，走 main.rs）** ——
V6 的提交里明写「原来那条回归就是从这一层躲过去的」，单元测试直调 `run_capture`
看不见 `main.rs`。八个写法（四个子命令 × 带不带 `--`）各一条，断言
**stdout/stderr 走向 + 退出码 + 输出里有没有真实选项名**。

改完回 `docs/acceptance-M.md` §0.1 把那张四行表按新实测重写，
`<!-- 台账 -->` 注释**删掉**（这条修完就没了）。

### ④ `ci.yml:112-113` 的注释引了一个漂掉的行号

```yaml
# README.md:90 写着「--offline 只跑 1、2、7，不碰网络也不碰 docker，CI 用这一档」——
# 在此之前 CI 里并没有对应步骤。这一条让那句话成立。
```

`README.md:90` 那句话在 V3 改写时挪了位置，「CI 用这一档」的字样也一度掉了
（W1 已经在 `README.md` 把语义补回来了，行号自己 grep 确认）。
**把注释里的行号与措辞刷成现在的真相**，或者干脆不写行号（更耐改）——
你来定，在回执里说理由。

> 这是 W1 留的账：`ci.yml` 是 W1 的只读面，它改不了。

### ⑤ 两个 Dockerfile 都没有 `USER` —— 容器以 root 跑（**先证实，再改**）

`docker/core/Dockerfile` 与 `docker/edge/Dockerfile` 的运行层（`debian:trixie-slim`）
**都没有 `USER` 指令**，两个进程都以 root 跑。

W1 在 **macOS** 上实测：Docker Desktop 把 uid 映射回宿主用户，`./data` 里的文件
宿主机读写正常（owner 是当前用户）。**但 Linux 上不是这样** —— `data/evidence` /
`data/artifacts` 会是 root 拥有，宿主机那条 `aite evidence show` 可能读不了。
**W1 只在 macOS 上验过，Linux 未验证。**

**你的活分三步，别跳过第一步**：

1. **证实或证伪。** CI runner 就是 linux/amd64 —— 在 `compose-smoke` 里加一步
   断言 `./data` 下新建文件的 owner（`stat -c '%u:%g'`），把 Linux 上的真实结果拿到手。
   **这一步本身就有价值**，不管后面改不改。
2. **如果 Linux 上确实是 root 拥有** —— 加非 root `USER`。要处理的坑至少有：
   命名卷 `run:` 的属主（socket 要建得出来）、`./data` bind mount 的属主
   （宿主机那边是当前用户，容器里是新 uid）、edge 还要读 `/var/run/docker.sock`
   （**这个多半要 root 或 docker 组，可能只有 core 能降权**）。
   **两个 service 不必一视同仁** —— core 不碰 Docker，降权代价小得多。
3. **如果 Linux 上没问题** —— 把结论写进 `docker/*/Dockerfile` 的注释和回执，销账。

> ⚠️ **这一条最容易做过头。** 它是 low，不是安全加固任务。目标是「宿主机拿得到自己的
> 证据文件」，不是「容器安全基线」。做不动就停在第 1 步 + 记账，**那也是合格交付**。

---

## 纪律

1. **每条改动都要有测试钉着**，而且新断言写完自己验一遍：**把产品代码改坏，它必须红**。
   这一轮台账已经抓到三条恒真断言了。
2. **不许 skip。** 尤其 ②：跑不了就该红。
3. **契约锁必须始终 `OK 25 files`**。`proto/**` 一个字都别动 —— ① 里 proto 是**判据**，不是改动面。
4. **别越界**：`core/crates/app/src/{run,app,cli,wiring}.rs` 与 `core/crates/control/**` 归 W2，
   W2 与你同时在跑。`main.rs` 和 `tests/cli_smoke.rs` 是你的，别的别碰。
5. **⑤ 先证实再改。** 没拿到 Linux 上的真实 owner 之前不许动 Dockerfile。
6. 卡住了：要改的文件不在白名单 → 停下报告；① 里 proto 注释与实现到底谁对读不出来 →
   **停下报告**，那是契约解释权，不归本轨。

## 验收

```bash
scripts/check.sh                                              # 「全部通过」，退出码 0
cd edge && go test ./internal/sandbox/... -count=1             # ② 的主战场（无 daemon 档）
cd edge && go test -tags docker ./internal/sandbox/... -count=1 # ① 别弄坏 docker 档
cd edge && go test -race ./... -count=1
cd core && cargo test -p aite --test cli_smoke                 # ③ 的回归
cd core && cargo clippy --workspace --all-targets -- -D warnings
docker ps -a --filter label=aite.task -q | wc -l               # 期望 0
```

外加**自己交叉验一遍**：

- [ ] ① 改完后，函数注释 / 包头注释 / proto 注释三处**说的是同一件事**，且有测试钉着。
- [ ] ② `inspectStamps` 的测试在**无 daemon**档下跑得到（不带 `-tags docker`）。
- [ ] ③ 八个写法各一条回归，且「把 `main.rs` 改回去」时它们必须红。
- [ ] ③ 改完 `docs/acceptance-M.md` §0.1 那张表与实测逐字对得上，台账注释已删。
- [ ] ⑤ 拿到了 Linux 上 `./data` 文件 owner 的**真实输出**（贴进回执），
      再决定改不改；改了的话 CI 的 compose-smoke 仍然全绿。

## 回执格式

在回复里写（**不要**新建回执文件；台账 `review-findings-2026-09-12-vmerge.md`
只许**追加**一节「W3 回执」）：

```
## W3 回执

基线 : <开场自检那个 sha>
check.sh : <五行关键值>  退出码 <n>
假红 : <撞到没有；撞到了贴失败名 + 单独跑的结果>

### ①–⑤ 逐条
（每条写：病在哪 / 判据是什么 / 改法 / 钉它的测试 / 改坏产品代码验过没）

### ③ 的八个写法实测表
| 写法 | 打出什么 | 走哪 | 退出码 |

### ⑤ 的 Linux 实测
（CI 上 ./data 文件的 owner 原始输出；改了的话改了什么、为什么两个 service 不一样）

### 测试数
793 → <N>（Rust）；Go 侧 sandbox 包 <旧> → <新>

### 记账转出去的
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
```
