# 任务 BB3 — 降权之后，Linux 上 `make compose-up` 起不来

## 背景：这轨是从哪来的

AA1 把两个容器降到非 root（`283f63e`）。代价是 Linux 上起飞前多了两件事：
先 `mkdir -p data`，再把 `AITE_UID` / `AITE_GID` / `AITE_DOCKER_GID` 三个变量传进去。

**而 `Makefile` 里那条 target 一件都没做**：

```make
compose-up: compose-build  ## 一条命令起飞：先把三个镜像建齐，再起两个常驻 service
	docker compose up -d
	@docker compose ps
```

于是 Linux 上 `make compose-up` 起不来：core 死在
`aite 起不来：建不出目录 data/evidence：Permission denied`，
配 `restart: unless-stopped` 就是崩溃循环。

现在靠 README 一句「要么按上面那样手敲，要么先 export 三个变量再 make compose-up」兜着
——**这是文档在替代码打补丁**。AA1 回执把这条明确转出来了（「`Makefile` 不在 AA1 可写面」）。

### 一并收的还有三条（都是 AA1 转出 / 没做的）

| # | 账 | 出处 |
|---|---|---|
| 2 | `data/.gitkeep` 那条治本改法没做（要同时动 `.gitignore`，不在 AA1 可写面） | AA1「记账转出去的」第 2 行 |
| 3 | edge 的 `group_add` 默认值 `0` 在 Linux 上给容器一个用不上的 root 组 | AA1「没做的」第 3 条 |
| 4 | `docker/edge/Dockerfile` **没有一次「用仓库原文建成功」的记录** | AA1「没做的」第 5 条 |

## ⚠️ 先把话说清楚：这一轨有一半你验不了

**本机是 macOS。** Docker Desktop 的 bind mount 过 VirtioFS 会双向翻译 uid
（容器里看见它自己的 uid，宿主机看见当前用户），所以
**macOS 上验不出这套东西对不对** —— README 自己就是这么写的。

这不是「那就别做了」，而是：

1. **能在本机验的，必须真跑**（`make compose-up` 在 macOS 上不许因为你的改动坏掉，
   `docker compose config` 解析得出来，`--dry-run` 式的变量展开对不对）；
2. **只能在 Linux 上验的，如实标成「没验过」** —— AA1 回执就是这么做的
   （它把「`AITE_DOCKER_GID` 在真 Linux 上的取值没验过」明写了出来）。
   **本轨最大的诚实风险就是把「逻辑上对」写成「验过了」。**
3. 仓库里有 `.github/workflows/ci.yml`（Linux runner），但**这个项目没有 CI 门禁习惯**
   —— 别把「CI 会兜住」当成验证。你改了 CI 就要说清楚「它下次 push 才会跑，
   跑之前没人知道对不对」。

## 必读（按顺序）

1. `Makefile` 全文 —— 尤其 `compose-*` 那一组，和 `make clean` 里那句
   `rm -rf edge/bin`（README 说它是历史遗留，顺手核实一下）。
2. `docker-compose.yml` 全文 —— `user:` / `group_add:` / 命名卷 `run:` 的 `1777`
   / 两个 healthcheck（**判据不一样，别照抄**）/ 刻意不写的 `depends_on`。
3. `README.md` 里「两个容器以非 root 跑（2026-09-13 起）」整段 ——
   那是当前的口径，你改完 `Makefile` 之后**它有几句会变成废话，要一起改**。
4. `.github/workflows/ci.yml` 的「容器身份：uid/gid 问 runner，docker 组 gid 问 socket，
   并把 ./data 先建出来」那一步 —— CI 已经是对的，**你的 `Makefile` 要和它同源**。
5. `review/review-findings-2026-09-12-vmerge.md` 第十四节（AA1 回执）全节 ——
   ①「改前基线（命名卷形态）」、②「core 降权 —— 每条决定与它的理由」、
   ④「新门禁『写得进』的自证」，以及「记账转出去的」/「没做的」两节。
6. `review/review-findings-2026-09-12-vmerge.md` 第十四节「没做的」第 5 条
   + 本机 memory 里那条 `edge 镜像本机建不出来`：
   **proxy.golang.org 本机连不上，`GOPROXY=off` 没用，要指到 cache mount 当 file:// proxy。**

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-bb3
分支     : task-bb3
基线     : 7019c48
本机     : macOS，Docker Desktop（Docker 29.6.1 / Compose v5.3.0）
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-bb3
git log --oneline -1        # 期望 7019c48
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0
make compose-config         # 期望能解析、不打印任何取值
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

### ① `make compose-up` 自己把三件事做齐

AA1 给的改法（原样抄自它的记账）：

```make
compose-up: compose-build
	mkdir -p data
	AITE_UID=$$(id -u) AITE_GID=$$(id -g) \
	AITE_DOCKER_GID=$$(docker run --rm -v /var/run/docker.sock:/var/run/docker.sock alpine stat -c %g /var/run/docker.sock) \
	docker compose up -d
```

**注意最后那个 —— AA1 特意点过：要问 daemon 的视角。**
macOS 上直接 `stat` 宿主机那个符号链接拿到的是 `0:1`，不是 VM 里的取值。

**但别照抄就交差，这里有几个真问题要你判断**：

1. 那条 `docker run --rm alpine` 每次 `compose-up` 都要拉一次 alpine、起一个容器 ——
   **这是 `make compose-up` 该付的代价吗？** 有没有更轻的办法（比如只在
   `/var/run/docker.sock` 是真 socket 而非符号链接时直接 `stat`）？给判断。
2. `compose-down` 要不要也传？（`down` 不建容器，但 compose 会解析整份 yml；
   变量缺了会怎样 —— **真跑一次看看**，别推测。）
3. **`make compose-up` 在 macOS 上不许因为这个改动变慢或变坏** —— 改完真跑一遍，
   `make compose-ps` 看两个 service 都 healthy，贴输出。

### ② `data/.gitkeep`

入库一个 `data/.gitkeep`，checkout 出来就归当前用户。要同时改 `.gitignore`
（现在 `:11` 是 `/data/`、`:23` 是 `/data/run/`）。

**判断题**：这条和 ① 的 `mkdir -p data` **是二选一还是都做**？
AA1 记账里写的是「与上一条二选一，或都做」，没定。**你定，并说清理由。**
（提示：`.gitkeep` 管的是「checkout 之后」，`mkdir -p` 管的是「已有的树上第一次起飞」——
它们覆盖的不是同一个时刻。）

改 `.gitignore` 时**别把 `data/` 整个放开** —— `data/evidence/`、`data/run/`、
库文件都不许入库。改完 `git status --short` 必须只有 `.gitkeep` 一个新文件。

### ③ `group_add: 0` 这个默认值

AA1「没做的」第 3 条把选择权交出来了：

> compose 没有条件表达式，做不到按平台取不同默认值；退而求其次是把后果写进注释与文档，
> 并让 preflight 第 6 组接住。**如果总管觉得默认值应该是「空/报错」而不是 0，
> 这是个可以翻的决定。**

**这一轨请把它翻过来审一次**，给一个带理由的推荐。三个候选：
保持 0（Desktop 友好、Linux 上多一个没用的 root 组）/ 改成空（Linux 正确、
Desktop 上要手传）/ 让 `make compose-up` 现问（① 已经在问了，那默认值就只在
「不走 make」时起作用）。

> ⚠️ **`preflight` 第 6 组归 BB6 那一轨的邻居，你别改 `preflight.rs`。**
> 要它接住的话，写进记账转给 BB6 或总管。

### ④ 用仓库原文建一次 edge 镜像

这是四条里**唯一一条有明确成败判据**的：`docker compose build edge`
（不注入任何东西）成不成功。

本机连不上 proxy.golang.org（AA1 实测两次 `dial tcp … i/o timeout`）。所以：

1. **先原样试一次**，把失败输出逐字贴进回执 —— 那本身就是这条账的证据；
2. 试着让它在本机建得出来，**但不许为此改 `docker/edge/Dockerfile` 的语义**。
   可以做的：给它一条**可选的** build arg / 一段文档，说清「离线环境怎么建」；
   不可以做的：把 `GOPROXY=file://...` 写死进 Dockerfile —— CI 的 runner 每次冷缓存，
   必须走真 proxy，那边才是这条依赖的门禁。
3. 建不出来就**如实写「本机建不出来，原因是 X，仓库原文一个字节没改」** ——
   这完全合格，AA1 就是这么交的。别为了让它绿而动 Dockerfile。

## 纪律

1. **可写面**：`Makefile`、`.gitignore`、`data/.gitkeep`、`docker-compose.yml`、
   `docker/edge/Dockerfile`、`docker/core/Dockerfile`、`.github/workflows/ci.yml`、
   `README.md` 里**与容器/起飞直接对应**的那几段、台账里**只追加**你自己那一节。
2. **只读面**：`core/**`、`edge/**`、`proto/**`、`config/**`、`docs/**`
   （`acceptance-M.md` §0.2 / §0.2.5 若确实过时，**记账转给总管，别自己改** ——
   那个文件 AA1/AA2 两轨都碰过，是冲突高发区）。
3. **不许动 spec 的依赖表**（`docs/dev-spec-2026-09-11-rustgo.md` §2.4 冻结）。
   要加依赖 → 停下报告。
4. **密钥纪律**：`docker compose config` 会把 `${VAR}` 解析成取值再打出来，
   **它的完整输出是带密钥的**。回执里要贴就贴 `--services`，或用
   `make compose-config`（已经是 `env -u` 清掉四个密钥变量之后的口径）。
5. **落盘无残留**：临时建的镜像标签（`aite-core:p0` / `aite-edge:p0` 是**全局的**，
   AA1 踩过这个 —— 从 `main` 跑 compose 的人会拿到你的镜像）要在回执里写明状态。

## 验收

```bash
scripts/check.sh              # 全部通过，退出码 0（本轨零 Rust/Go 代码改动，五行应逐字不变）
make compose-config           # 能解析，不打印取值
make compose-up && make compose-ps    # macOS 上两个 service 都 healthy
make compose-down
git status --short            # 只有你的改动
```

> **本轨零产品代码改动**，所以 `scripts/check.sh` 的五行关键值
> （含 `cargo passed=864`）**必须逐字不变**。变了说明你碰到了不该碰的东西。

## 回执

追加进台账，标题「二十、BB3 回执 —— 2026-09-15」。要有：

- 基线与开场自检（五行 + 守卫拦截逐字原话 + `make compose-config` 的输出）；
- ① 改前 / 改后的 `Makefile` 对照，三个判断题各自的答案与理由，
  **macOS 上 `compose-up` → `compose-ps` → `compose-down` 的完整实跑输出**；
- ② `.gitkeep` 与 `mkdir -p` 的二选一/都做，判断与理由，`git status` 复核；
- ③ `group_add` 默认值的推荐 + 三个候选各自的后果；
- ④ **`docker compose build edge` 用仓库原文的实跑结果**（成功或失败，逐字贴）；
- **哪些结论是 macOS 上量的、哪些只能在 Linux 上验** —— 做一张表，
  每一行标「实测 / 推断」。这张表是本轨最重要的交付之一；
- **记账转出去的**（表格：位置 / 病 / 归哪轨）；
- **没做的 / 拿不准的**（编号列表）。
