# 任务 AA1 — 容器降权：Linux 上 `./data` 归 root，宿主机写不进删不掉

## 背景：这轨是从哪来的

**W3 ⑤ 已经把这件事量完了，量完之后**故意**没修**，并写明「真要加的话 core 是先降的那个」。
你这一轨就是去修它 —— 材料是现成的，难的不是改 Dockerfile，是**证明你改对了**。

`docker/core/Dockerfile:107` 与 `docker/edge/Dockerfile:53` 两个小节记着 W3 的实测。
结论逐字（Linux 上真镜像真起飞，`./data` 换成命名卷绕开 VirtioFS 的映射）：

```
0:0 755 directory    data/            ← ./data 不入库，bind mount 时由 dockerd 建
0:0 644 regular file data/aite.db     ← 证据链主存（storage.sqlite_path）
0:0 755 directory    data/evidence/
0:0 755 directory    data/artifacts/

以宿主机用户（GitHub runner 是 uid 1001）去碰同一批文件：
  READ  OK      ← 原来那条担心不成立：0644 + 0755 对任何用户都开着读
  WRITE DENIED
  MKDIR DENIED  ← 宿主机写不进、删不掉，也没法在 ./data 里新建
```

**真后果**（W3 自己写的）：Linux 上 compose 跑过一次之后，宿主机直跑 `aite run` 会因为
落盘目录不可写起不来。这不是洁癖，是一条会咬人的路径。

**W3 当时不修的理由，逐条就是你要处理的**：

1. 命名卷 `run:` 的挂载点属主得跟着换（两个 socket 要建得出来）；
2. `./data` 这条 bind mount 在宿主机那边是当前用户、容器里是新 uid；
3. edge 那一侧还要读 `/var/run/docker.sock`（宿主机上归 `root:docker`），换非 root uid
   之后要么把 docker 组的 gid 从 compose 传进来，要么连不上 daemon ——
   而 **daemon 连不上 = `Ping` 失败 = `EdgeStatus.sandbox_ok` false = preflight 第 6 组 FAIL**。

**所以 core 和 edge 不必一视同仁。** 本轨的最低交付是 **core 降权 + 证明没弄坏任何东西**；
edge 那一侧你可以做、也可以给出判据说明为什么这一轮不做 —— **但必须是量出来的判据，不是感觉**。

## ⚠️ 这一轨最容易出的事故：在 macOS 上自欺欺人

**这台机器是 macOS，而 macOS 验不出这条病。** Docker Desktop 的 bind mount 走 VirtioFS，
会把 uid 映射回宿主用户 —— W3 实测「owner 是当前用户，容器里明明是 root 建的」。
也就是说**你在本机跑一遍 compose，看到 `./data` 属主正常，什么都证明不了**。

W3 拿到 Linux 语义的办法写在 Dockerfile 的注释里：**`./data` 换成命名卷，绕开 VirtioFS 的映射**。
照它做。你的每一条属主/权限结论都必须注明**是在哪种挂载形态下量的**，
在 bind mount 上量出来的属主一律不作数。

## 必读（按顺序）

1. **`docker/core/Dockerfile` 的「以谁的身份跑」小节**（`:105` 起）—— W3 的完整实测与判据。
2. **`docker/edge/Dockerfile` 的同名小节**（`:51` 起）—— 只记了「不一样的那半」，即 docker.sock。
3. `docker-compose.yml` —— 两个 service 的 `volumes`（`./config:ro`、`./data`、命名卷 `run:`）、
   健康检查、`restart` 策略。**`run:` 那个命名卷是两个 UDS socket 的家**，降权后它的
   挂载点属主是第一个坑。
4. `.github/workflows/ci.yml` 的 `compose-smoke` job —— **「读得动」这一条是硬门禁**，
   已经钉在那里（把 `data/aite.db` 的权限位改成 0600，那一步就红并点名它，W3 本机实跑验过）。
   你改完它必须还绿，而且**最好让它同时钉住「写得进」**。
5. `review/review-findings-2026-09-12-vmerge.md` 的 W3 回执（搜「W3」）—— 那笔账连后果一起记的地方。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-aa1
分支     : task-aa1
基线     : __BASE__
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-aa1
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
> 那说明 `.claude/settings.json` 的修正那一格没进 git，**停下来喊人**，别当回归也别自己修
> （那是守卫的保护面，只有人能改）。

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明守卫在静默失效，
> **停下报告**。

**本轨专属的第 0 步 —— 先确认 Docker 真的能用**：

```bash
docker info >/dev/null && echo "daemon OK"
docker compose version
env -u FEISHU_APP_ID -u FEISHU_APP_SECRET -u FEISHU_BOT_OPEN_ID -u AITE_MODEL_API_KEY \
  docker compose config -q && echo "compose 编排可解析"
```

三条里任何一条不成，先停下报告 —— 这一轨没有 Docker 就什么都验不了。

## 要干的活

### ① 先把现状在**命名卷**形态下量一遍（不许跳过）

改之前先拿到本机的基线，否则你改完不知道自己改动了什么。照 W3 的办法：把 `./data` 这条
bind mount 临时换成命名卷起一次，进容器里 `stat` 那四个路径，再从宿主侧以非 root 用户去
READ / WRITE / MKDIR 各试一次。**把命令与逐字输出记进回执。**

量完把 compose 恢复原样（`git diff` 必须干净），这一步只是取基线。

### ② core 降权

`docker/core/Dockerfile` 加非 root 身份。**要处理的不止一行 `USER`**：

- 那个 uid/gid 取什么值、为什么（CI runner 是 uid 1001 —— 这是个已知锚点，但别硬编码到
  只在 CI 上对的程度）；
- `/app` 下那些 `COPY` 进来的东西属主要不要跟着改（`config/aite.example.yaml`、`evals/`）；
- `./data` 这条 bind mount：容器里是新 uid，宿主机那边是当前用户，**两边要怎么对上**；
- 命名卷 `run:` 的挂载点属主 —— **两个 UDS socket 要建得出来**，这是最容易忘的一条，
  而且症状是「core 起来了但 edge 连不上」，不是「起不来」。

**把每条决定的理由写进 Dockerfile 的那个小节**（W3 那段实测别删，它是病史；在它后面接着写
「__TODAY__ 降权那一轮做了什么」）。

### ③ edge：做，或者给出不做的判据

难点是 `/var/run/docker.sock`（宿主机 `root:docker`）。两条路：

- 把宿主机 docker 组的 gid 从 compose 传进容器（`group_add` 之类）—— 那个 gid 在不同宿主机上
  不一样，**这是真代价，不是借口**，要写清楚它对「换一台机器还跑不跑得起来」的影响；
- 不降，只降 core。

**选哪条都可以，但结论必须带实测**：不降的话，至少要量出「降了会怎样」——
起一次、看 `EdgeStatus.sandbox_ok`、看 preflight 第 6 组。拿不到 Linux 环境就如实说
「这一条在 macOS 上量不到」，别拿 macOS 的结果当 Linux 的结论。

### ④ 门禁：让 CI 同时钉住「写得进」

`compose-smoke` 现在只钉了「读得动」。降权之后「写得进」变成了新的承诺 ——
**没有门禁的承诺过两轮就会悄悄退回去**。加一步，判据要像现有那条一样能被证伪
（W3 那条的自证方式是「把权限位改成 0600，那一步就红并点名它」，你也照这个标准自证一次）。

### ⑤ 文档追平

`docs/acceptance-M.md` 与 `README.md` 里凡是提到容器身份 / `./data` 属主 / 「宿主机读得动
但写不进」的地方，改完之后跟着改。**先 grep 再改，别凭印象**。

## 纪律

1. **可写面**：`docker/core/Dockerfile`、`docker/edge/Dockerfile`、`docker-compose.yml`、
   `.github/workflows/ci.yml`、`docs/acceptance-M.md`、`README.md`、
   `review/review-findings-2026-09-12-vmerge.md`（只许**追加**你自己那一节）。
2. **只读面**：`core/**`、`edge/**`、`evals/**`、`scripts/**`。降权如果逼得你改 Rust/Go 代码，
   **停下来报告** —— 那说明这件事比 W3 估的大，要重新定范围。
3. **冻结面**：`proto/**`、`core/crates/contracts/**`、`docs/dev-spec-2026-09-11-rustgo.md`。
   守卫会拦，别试。
4. **`.claude/**` 一个字节都不碰**（读也不行，守卫拦）。
5. `cargo fmt --all`（写模式）会被守卫拦（它碰冻结面）。本轨大概率不需要它；真需要就对
   单个文件跑 `rustfmt --edition 2024 <file>`。
6. **落盘无残留**：量属主时建的命名卷、临时容器、`./data` 里的东西，收尾前清干净并复核。

## 验收

```bash
scripts/check.sh                                   # 五行关键值与开场逐字相同
env -u FEISHU_APP_ID -u FEISHU_APP_SECRET -u FEISHU_BOT_OPEN_ID -u AITE_MODEL_API_KEY \
  docker compose config -q                         # 编排仍可解析
```

外加**本轨自己的判据**（你设计，写进回执）：降权后在命名卷形态下重量一遍 ①，
四个路径的属主/权限 + 宿主侧 READ/WRITE/MKDIR 三试，逐字贴出来。

> **本轨零 Rust / 零 Go 改动，所以 check.sh 那五行必须逐字不变。** 变了说明你改到了不该改的地方。

## 回执

写进 `review/review-findings-2026-09-12-vmerge.md`，**追加**一节「十四、AA1 回执 —— __TODAY__」
（**只追加，别动别人的节**）。要有：

- 基线与开场自检（五行关键值 + 守卫拦截那一条）；
- ① 的改前基线（命令 + 逐字输出 + 挂载形态）；
- ② 每条决定的理由，尤其 uid/gid 取值与 `run:` 卷挂载点属主；
- ③ 的结论与它的实测（做了就贴改后的 `sandbox_ok`；不做就贴「降了会怎样」的量）；
- ④ 新门禁的自证（怎么让它红的）；
- **记账转出去的**（表格：位置 / 病 / 归哪轨）；
- **没做的 / 拿不准的**（编号列表。在 macOS 上量不到的东西，如实说量不到，
  **别拿 bind mount 的结果冒充 Linux 语义**）。
