# 任务 V1 — 部署与起飞包：真镜像 compose + 构建产物出仓库 + 健康/日志 + CI compose 冒烟

## 背景：这轨是从哪来的

`docs/dev-spec-2026-09-09.md` 是 P0 的权威文档，里面没兑现的只剩两件：§2.4 的 **M1–M6 真机验收**
（真实飞书测试群，总管亲自跑）和 §2.4 抬头那句「**能录一段 3 分钟演示**」。§1 又明令禁止
「任何『顺手做一点 P1』的行为」。所以这一批六轨的定位统一是：**把 P0 收尾到「总管可以坐下来跑
M1–M6、可以开录」，外加把审核留下的账清掉**，一行 P1 都不许做。

你这一轨管的是**总管坐下来那一刻，面前那台机器上跑的是什么**。RΩ 合入之前，`docker-compose.yml`
是一份从没被真跑过的编排 —— 合并前审核真 `up` 了一次，当场撞上四条 blocker（登录 shell 洗掉 PATH、
rust 镜像里没有 protoc、core 在 `/app/core` 里 exec 导致配置里所有相对仓库根的路径错位、edge 的
socket 落到 `/app/edge/data/run/` 而共享卷挂的是 `/app/data/run`）。四条全修掉了，现在它**能起来**，
台账 §2.1 贴着实测的四行起飞日志。

**但它只是「能起来」，不是「能用来验收和演示」。** 现在这份 compose 是 P0 的权宜形态 —— 抬头
`docker-compose.yml:38-40` 自己就写着「把仓库挂进去、在容器里现编，没到要做多阶段镜像的时候」。
代价被低估了三处：① core 的 `cargo build --workspace`（line 99）在 `./:/app`（line 108）里编，
`CARGO_TARGET_DIR` 没设，产物直接落进宿主机仓库的 `core/target` —— **本机实测 8.5G，整个仓库目录
23G**，而宿主机那份是 darwin/arm64 产物、容器里是 linux/arm64，同一个 target 目录两个 host triple，
谁编谁把对方的全量刷掉；② edge 的 `GOPATH=/go` 是容器层，`down` 之后再 `up` 把全部 Go 依赖重下一遍
（台账实测约 1 分钟）；③ `docker compose ps` 看不出谁就绪，日志无界，M1–M6 排障要的三个观察窗
（`docs/acceptance-M.md` §0.3）在 compose 形态下没有对应说法。

还有一条更难受的：**那四条 blocker，现有 CI 一条都抓不到。** `.github/workflows/ci.yml:63-66`
的 compose 步骤跑的是 `docker compose config --services`，纯解析 YAML —— 台账 §2.1 抬头第一句
就是这么写的。这一轨要把那个洞补上：CI 里加一条真起一次（或等价强度）的步骤，并且你要**先证明
现有那一步抓不到**，再证明新那一步抓得到。

边界先划清楚：compose 的**编排语义**是 spec §2.1 定死的，不许改 —— 刻意不写 `depends_on`
（两边都是懒连接 + 1→2→…→30s 退避重连，启动顺序无关）、单副本（同一飞书应用多副本长连接只有
一个收得到事件）、密钥按进程分（飞书三项只给 edge，模型密钥只给 core）、两个 unix socket 走命名卷。
你改的是**构建与运维形态**，不是编排语义。

## 必读（按顺序）

1. `review/review-findings-2026-09-12-romega.md` —— **这一份对你最要紧**。
   §2.1（四条 blocker 的病因、改法、实测起飞日志 —— 你要保证它们不会随形态改动回来）、
   §4.2 前两条（`CARGO_TARGET_DIR` 与 `gomod` 卷，就是你这一轨的主账）、§5（合并时的实测基线 = 你的起跑线）。
2. `docker-compose.yml` 的抬头注释 **1–45 行**，一行都别跳。它是现行编排的全部理由：
   line 25–36（密钥怎么分 + `docker compose config` 会把 `${VAR}` **解析成取值**打出来）、
   line 38–40（P0 现编形态的自述，也就是你要换掉的那个）、line 42–48（踩过坑才这么写的三条）。
3. `docs/acceptance-M.md` §0.2（起飞：两个终端各起一个进程）、§0.3（**三个观察窗**：进程日志 /
   `aite evidence show --list` / `docker ps --filter label=aite.task`）。总管拿你交的东西干的就是这件事 ——
   compose 形态下这三个窗口分别变成什么命令，是你要给的说法。
4. `docs/dev-spec-2026-09-11-rustgo.md` §2.1（进程模型、启动顺序无关 —— 这是「不许加 `depends_on`」
   的出处）、§5 归属表 line 274（`docker-compose.yml` / `README.md` 原属 RΩ）。**这份文件冻结，只读。**
5. `review/paste-ROMEGA.md` §④（compose 上一轮的原始要求）+ `docs/dev-spec-2026-09-09.md` §1（禁 P1）、
   §2.4（M1–M6 与 3 分钟演示 —— 你这轨为什么存在）。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-v1
分支     : task-v1
基线     : 0bc8d55（已建好，git worktree add 时钉的就是这个 sha）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
宿主机   : darwin/arm64 —— 你 build 出来的是 linux/arm64；CI runner 是 linux/amd64。**两边都要能编。**
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。


> **基线说明**：`0bc8d55` = RΩ 合入 main 那次（`11322b3`）**再往前一格**。
> 那一格只改了两个文件：`scripts/check.sh`（B 全量 cargo test 那步改成「失败测试名在前、计数在后」）
> 与 `review/review-findings-2026-09-12-romega.md`（收窄 Answering 那条 + 补记一次未复现的 717/1）。
> `git diff --stat 11322b3..0bc8d55` → `2 files changed, 11 insertions(+), 2 deletions(-)`。
> 本派单正文里凡是写「在 `11322b3` 上核过 / 实测」的，指的是核对当时那一格，**代码面与你的基线逐字相同**。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v1
git log --oneline -1                                   # 期望 0bc8d55（记下它，回执里当基线）
git status --short                                     # 期望空
scripts/check.sh                                       # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

`scripts/check.sh` 的关键行期望（**这就是起跑线，任何一条变小都是回归**）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=718 failed=0` |
| B 全量 go test（-race） | 八个包全 `ok`（`-race` 是硬门禁，别去掉） |
| B8 评测 | `passed 10/10` —— **RΩ 起它是硬门禁**，check.sh 会计分 |

另外单独跑一次（不在 check.sh 里）：

```bash
cd edge && go test -tags docker ./internal/sandbox/... -count=1   # 期望 ok
docker ps -a --filter label=aite.task -q | wc -l                  # 期望 0
```

> **已知的一条假红**：docker 档沙箱测试在多轨并行时会假红（六个 worktree 同时跑真容器会互相挤）。
> 它红了先看两件事：是不是**只有这一个包**红、**单独跑**是不是绿。是的话就是并行挤的，不是回归。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
> （2026-09-12 总管这边真踩过：会话在仓库子目录里起，`CLAUDE_PROJECT_DIR` 就定死在那个子目录，
> hook 命令 `python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"` 找不到文件 →
> 执行失败 → **非阻塞放行、不报警** → 整个会话守卫静默失效。台账第六节记的就是这件事。）

## 可写路径

| 面 | 权限 |
|---|---|
| `docker/core/**`、`docker/edge/**`（新建）、`.dockerignore`（新建） | **自由写** |
| `docker-compose.yml`、`.github/workflows/ci.yml`、`Makefile` | **自由写** |
| `docker/sandbox/**` | **一个字不许改** —— 那是沙箱镜像，是 B4 的验收面。你只能在别处引用它 |
| `README.md` | **归 V3**。要改 CI 那一节就在回执里写清改哪几行、建议文案是什么，交出去，自己别动 |
| `docs/acceptance-M.md`、`docs/demo-3min.md` | 不在你面上，**归 V3**（真机验收剧本那一轨）。要补 compose 形态的观察窗，写进回执点名交给 V3，总管合流时路由 |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock` | 契约冻结。碰到了说明你改错地方了，**停下报告** |
| `evals/p0/*.yaml` | 十个场景是验收面，一个字不动 |
| `docs/dev-spec-2026-09-09.md`、`docs/dev-spec-2026-09-11-rustgo.md` | 冻结（守卫也拦着） |
| `core/**`、`edge/**` 的源码 | **默认不动。** 下面 ③ 和 ④ 各有一处"要动就得动源码"的口子，那两处的倾向都是**别动、交出去** |

## 要做什么

### ① compose 从「容器里现编译」改成真镜像

**病在哪**：`docker-compose.yml:82` `image: rust:1.98` + line 99 `cd core && cargo build --workspace`，
在 line 108 `./:/app` 这个整仓 bind mount 里编。没有 `CARGO_TARGET_DIR`，产物落进宿主机的
`core/target`。edge 同理（line 51 `image: golang:1.27`、line 60 `go build`）。

**怎么验证它确实病着**（两次输出都贴回执）：

```bash
du -sh core/target ; ls -ld core/target        # 记下体积和 mtime
docker compose up core                          # 让它在容器里编一次（起不来没关系，看编译）
du -sh core/target ; ls -ld core/target        # 变了
cd core && cargo build --workspace              # 回宿主机：看它是不是从头再全量编一遍
```

**改法方向**：`docker/core/Dockerfile` + `docker/edge/Dockerfile`，各一个多阶段（builder + 瘦运行层），
compose 两个 service 从 `image:` 改成 `build: {context: ., dockerfile: docker/<x>/Dockerfile}`。
下面这些约束我都核过，是硬的：

- **build context 必须是仓库根。** `core/crates/proto/build.rs:3` 走的是
  `CARGO_MANIFEST_DIR/../../../proto` —— 只 COPY `core/` 的话 build.rs 当场炸。
- **先加 `.dockerignore`**（仓库根现在没有，`ls -a` 实测无此文件）。不加的话 context 会把 8.5G 的
  `core/target`、202M 的 `.venv`、六个 worktree（`/.worktrees/`）、`data/` 全打包送给 daemon，
  第一次 build 等到天荒地老。**但别把 `proto/` 排掉**（见上一条）。
- **运行层必须有 `ca-certificates`。** `reqwest` 是 `default-features=false, features=["json","rustls"]`
  （`core/Cargo.toml:48`），而 `core/Cargo.lock:2082` 里有 `rustls-native-certs` —— 它读**系统信任库**。
  运行层不装根证书的话，CI 冒烟（不发 HTTPS）根本不会红，**M1 第一次调模型端点才炸**。
- `rusqlite` 是 `bundled`（`core/Cargo.toml:46`），sqlite 静态进二进制，运行层不需要 libsqlite3；
  但 glibc 要同代（builder 用 `rust:1.98*` = bookworm → 运行层也用 bookworm 系）。
- **`worker.system_prompt_path` 默认是 `core/crates/worker/prompts/platform.md`**（见
  `config/aite.example.yaml`），相对仓库根。不再整仓挂载之后，这个文件要么烤进镜像的同一个相对路径
  （`/app/core/crates/worker/prompts/platform.md`），要么在挂进去的配置里改路径。**选哪条你定，回执说明。**
- `config/aite.yaml` **不入库**（`.gitignore:10`），只能 bind mount（建议 `:ro`）；`data/` 也要 bind
  mount —— 否则 sqlite 和证据链落在容器里，总管在宿主机上看不到，`docs/acceptance-M.md` §0.3 的
  第二个观察窗直接废掉。
- **tag 钉到 patch 位**：现在是 `rust:1.98`（line 82）/ `golang:1.27`（line 51），而
  `core/rust-toolchain.toml:3` 钉 1.98.1、`edge/go.mod:3` 写 `go 1.27.1`。第一次 build 的日志里看
  有没有 `downloading component` / `downloading go1.x` 这类行，有就把 tag 钉死。
- **跨架构**：现行 compose 用 `uname -m` 分支装 protoc（line 92）。Dockerfile 里换成 `TARGETARCH`，
  或者直接 `apt-get install -y protobuf-compiler`（bookworm 是 3.21.x，与 CI 的
  `arduino/setup-protoc@v3 version "31.x"`（`ci.yml:32-34`）不同代 —— **能不能编你实测**，
  编得过就用它，编不过就照官方 release 装、和 CI 同源）。

**三个坑「怎么证明它们没回来」** —— 这张表回执里要逐条给证据，不许只写"改成真镜像自然就没了"：

| 坑 | 为什么不会回来 | 你要贴的证据 |
|---|---|---|
| ① 登录 shell 洗掉 PATH | 运行层直接 exec 二进制，不过 shell | `docker inspect --format '{{json .Config.Entrypoint}}' <容器>`；运行层里 `command -v cargo` / `command -v go` 都为空 |
| ② rust 镜像里没 protoc | 编译搬进 builder 阶段，运行层压根不编 | `docker compose build --no-cache core` 从零成功的最后几行 |
| ③ cwd 必须是 `/app` | `WORKDIR /app` + compose `working_dir: /app` | `docker compose exec core pwd` = `/app`；起飞日志 `aite.up` 那行的 sqlite / evidence 路径 |
| ④ socket 落对地方（blocker 4） | 共享命名卷仍挂 `/app/data/run` | `docker compose exec edge ls -l /app/data/run/` 两个 `.sock` 都在；core 日志出现 `edge.connected socket=/app/data/run/aite-edge.sock` |

**顺带一条你绕不开的**：`aite-sandbox:p0` 不由 compose 保证。edge 只 `ImageInspect`、**不 pull**
（`edge/internal/sandbox/docker.go:168-172`，镜像不在就报「沙箱镜像不存在：… 先跑 `docker build …`」）。
「一条命令起得来」这个目标要求你给个说法：compose 里加一个 build-only 的 service/profile，或者
Makefile 的 up target 先建镜像。**选哪条你定**，但 `docker/sandbox/**` 的内容一个字不许改。

### ② 构建产物不许写进仓库

**病在哪**：台账 §4.2 前两条。core 缺 `CARGO_TARGET_DIR`；edge 的 module 缓存在容器层
（compose line 66 只给了 `GOCACHE: /gocache` + line 73 `gocache` 卷，**没有 `/go/pkg/mod`**）。

**怎么验证它确实病着**：`docker compose down` 之后再 `up`，掐表看 `go mod download` 那段耗时
（台账实测约 1 分钟）；core 那半用 ① 里那四条命令。

**改法方向**：真镜像之后，缓存的正确位置是 **builder 阶段的 BuildKit cache mount**
（`--mount=type=cache,target=/usr/local/cargo/registry`、`target=/go/pkg/mod`），不是运行期命名卷。
**如果你决定保留一个「开发档」**（继续挂源码现编，改一行不用重建镜像 —— 这一档对总管调试真有用），
那一档**必须**有 `CARGO_TARGET_DIR` 指到命名卷。要不要保留开发档你定，但：

> **硬判据（回执必须有）**：跑完一整轮 build + up + down 之后，`git status --short` 仍然为空，
> 且 `core/target` 的体积与 mtime 与开跑前一致。任何一档往宿主机 `core/target` 写都算没做完。

### ③ 健康检查与重启策略

**病在哪**：`docker compose ps` 看不出谁就绪。`restart: unless-stopped`（line 74 / 111）之下，
崩溃循环和正常运行长得一样 —— 审核那次是靠 `docker inspect --format '{{.RestartCount}}'` 手动看出来的。

**边界（这条别改）**：**不许加 `depends_on`**，spec §2.1「启动顺序无关」，两边都是懒连接 + 退避重连；
更不许加 `depends_on: condition: service_healthy`。healthcheck 只是让人看得见，不是用来排序的。
另外 core 的健康判据**不许依赖 edge** —— edge 没起时 core 记一行 `aite.edge_unreachable` 照常起飞，
那是正常状态，不是不健康。

**两边的现状我核过了**：

- **edge 有现成的**：`edge/internal/server/server.go:38-40` 注册了标准 gRPC health
  （`SetServingStatus("", SERVING)`），跟三个业务服务同一个 unix socket。`grpc_health_probe` 支持
  `-addr=unix:///…`（**你先自己验一次再写进 compose**）；运行层里没有这个二进制，要在 builder 阶段
  `go install` 再 COPY 进去。
- **core 没有**：`core/crates/edge-client/src/ingress.rs:96-106` 只 `add_service(IngressServiceServer)`，
  没有 health service，`aite` 也没有 `status` 这类子命令（`core/crates/app/src/cli.rs:7-13` 的 USAGE 就三个选项）。
  三条路：**(a)** 给 core 的 ingress 加 tonic-health —— 要动 core 源码，不在你可写面；
  **(b)** healthcheck 只判「`/app/data/run/aite-core.sock` 在，且 connect 得上」—— 运行层里要有能做这件事的东西
  （distroless 里没有 shell，选运行基底时就得想好）；**(c)** 只给 edge 写 healthcheck，core 不写。
  **总管的倾向是 (b)，退而求其次 (c)** —— 理由是 §1 禁 P1、这一批不碰业务代码，为一个观测便利去改
  core 的服务面不划算。**你选哪条，在回执里说清楚，并说明它在「core 起飞失败崩溃循环」这个真场景下
  会不会变红**（这才是它唯一要抓的东西）。

### ④ 日志：M1–M6 要看得见

**病在哪**：两个进程都写 **stderr**（`core/crates/app/src/cli.rs:116-122` 的 tracing fmt、
`edge/cmd/aite-edge/main.go:75` 的 slog TextHandler），compose 没写 `logging:` 段，默认 json-file
驱动**无上限**。`restart: unless-stopped` + 崩溃循环时日志涨得很快。

**要给的说法**（三样都要）：

1. 两个 service 加 `logging` 段（json-file + `max-size` / `max-file`）。
2. 观察窗命令：`docker compose logs -f --tail=200 --timestamps core edge`（并说明加不加 `--no-color`
   对贴回执的影响）。
3. **把 `docs/acceptance-M.md` §0.3 那张「三个观察窗」表翻译成 compose 形态**，写进回执**交给 V3**（它拥有 `docs/**` 与 `README.md`）：
   进程日志 → `docker compose logs -f`；证据时间线 → 宿主机 `core/target/debug/aite evidence show --list`
   （`data/` 是 bind mount，宿主机上那个 darwin 二进制读得到）**或** `docker compose exec core …`；
   沙箱 → 宿主机 `docker ps --filter label=aite.task`（沙箱是兄弟容器，直接在宿主机 daemon 上）。

**顺带记一条你改不了的**：edge 的日志级别是硬编码 `slog.LevelInfo`（`main.go:75`），没有 `RUST_LOG`
那样的旋钮 —— 真机排障时 edge 这半调不高。动它要改 edge 源码，不在你可写面：**在回执里写清楚交出去。**

### ⑤ CI 补一条 compose 冒烟

**病在哪**：`.github/workflows/ci.yml:63-66` 只跑
`docker compose config --services | sort | … | grep -qx 'core edge '`。纯解析 YAML，四条 blocker 一条都没抓到。

**怎么验证它确实病着（这一步你要真做）**：把 compose 的两条 command 改回 `bash -lc`（blocker 1 的原样），
在本地跑一遍现行那一步的命令 —— **它照样绿**。把这个输出贴回执，这是「现有门禁抓不到」的硬证据。
然后换成你新写的那一步，**必须红**；还原，**必须绿**。

**边界（先量清楚再动手，不然会写出一条在 CI 里永远起不来的步骤）** —— 下面五条我都核过：

| 事实 | 出处 |
|---|---|
| `config/aite.yaml` 不入库，CI checkout 里根本没有；而两条 command 都写死 `--config config/aite.yaml` | `.gitignore:10`；compose line 61 / 100 |
| `platform: fake` 时 core **拒绝起飞**（fake 是「必须由调用方注入」的标记，不是内建替身） | `core/crates/app/src/app.rs:143-146` |
| `platform: feishu` + 假凭证时 edge 的 `platform.Start` 返回 err → `edge.component_failed` → **进程退出**，配 `restart: unless-stopped` 就是崩溃循环 | `edge/cmd/aite-edge/main.go:128-141, 151-157` |
| `model.base_url` / `model.model` 为空 → core `StartupError` 退 2；而 example 配置里这两项就是空的 | `core/crates/models/src/lib.rs:318-327`；`config/aite.example.yaml` |
| `aite-sandbox:p0` 不在 CI 的 daemon 上，edge 不 pull 只 inspect | `edge/internal/sandbox/docker.go:168-172` |

**三条可选路线，选一条，回执说明选了哪条、为什么**：

- **A**：`docker compose build`（两个镜像都真编）+ `docker compose run --rm core aite preflight --offline`
  （CI 先 `cp config/aite.example.yaml config/aite.yaml`、给占位环境变量）。
  抓得到：镜像能编（protoc / PATH / 依赖）、cwd 与相对仓库根的路径、落盘目录可写。抓不到：两个 socket 真连上。
- **B**：A + `docker compose up -d edge`（edge 吃一份 `platform: fake` 的 CI 配置）→ 断言
  `/app/data/run/aite-edge.sock` 落在共享卷里、health 探针 SERVING。**多抓到 blocker 4 的原病。**
- **C**：B + core 也真起。注意 core 和 edge 读的是同一个 `platform` 字段 —— 要两边同时真起，
  得让两个 service 吃**不同的配置文件**。能不能这么配、值不值，你自己量。抓得到的是审核那次贴出来的
  `edge.connected` + `aite.up` 两行，也就是最强的那档。

**总管的倾向是至少 B。** 判据要落在「这四条 blocker 各自抓不抓得到」上 —— 回执里给一张
四行的对照表（blocker × 抓得到 / 抓不到 / 为什么）。

**还要量一个数**：`ci.yml:17` 是 `timeout-minutes: 30`。真 build 两个镜像会顶上限，
所以缓存策略（BuildKit cache mount + Actions 的 gha cache 或 `--cache-from`）要一并给出，
并在回执里贴新 CI 那一段的**预计耗时依据**（本地冷/热两次 build 的实测秒数就够）。

### ⑥ Makefile / README 的 CI 口径（台账 low）

- **`Makefile:47-48` 的 `compose-config` 跑的是裸 `docker compose config`** —— 它会把 `${VAR}`
  **解析成取值**打到 stdout（compose 抬头 line 33-36 和 `README.md:70-71` 都专门写着这条），
  而 `ci.yml:64-66` 是拿 `env -u` 清了四个变量的。同一件事两个口径，且 Makefile 这个口径**会把密钥打出来**。
  按 ci.yml 的口径统一。
- Makefile 现在没有任何 compose 相关的 build / up / down / logs target（只有 `docker-image`
  建沙箱镜像，line 44-45）。这一轨交的东西要能「一条命令起得来」，缺的 target 你补。
- `Makefile:3` 抬头写「protoc（只有 proto-gen 用）」—— 真镜像之后 protoc 挪进 builder 阶段，
  宿主机侧这句话大概仍然成立，**自己核一遍再决定改不改**。
- **`README.md:90`**「`--offline` 只跑 1、2、7（不碰网络也不碰 docker，**CI 用这一档**）」在
  `ci.yml` 里没有对应步骤，且和 `README.md:185-189` 自己的 CI 章节冲突。**你要是在 CI 里真加了
  `preflight --offline`（路线 A 就带着它），这句话当场变成真的。** README 归 V3：在回执里写清
  「第 90 行现在成立 / 仍不成立」、CI 章节那三行该改成什么，交出去，**自己别动 README**。
- **这两条台账 low 已经在 RΩ 合并时修掉了，别再报**（我核过）：`Makefile:24`（`make test` 的 go 侧
  已有 `-race`，第 24 行就是 `cd edge && $(GO) test -race ./... -count=1`）、`.gitignore:1`
  （Python 条目已清空，现在整个文件 19 行里一条都没有）。

## 纪律

1. **契约与锁**：`proto/**`、`core/crates/contracts/**`、`.contracts.lock` 冻结，全程 `OK 25 files`。
   你这一轨本来就碰不到它们 —— 真碰到了说明改错地方了，停下报告。
   （`.dockerignore` 别把 `proto/` 排掉：那会让 build 红，不会让锁红，容易误诊。）
2. `evals/p0/*.yaml` 十个场景是验收面，**一个字不动**。
3. `docs/dev-spec-2026-09-09.md` 与 `docs/dev-spec-2026-09-11-rustgo.md` 冻结。
4. 不 panic；不在 async 里阻塞；**测试不靠真实 sleep**，也不靠墙钟阈值当判据。
   你这轨的形态是：CI 里等就绪一律**轮询 + 上限重试**，不许写 `sleep 60` 当判据；healthcheck 的
   `interval/retries/start_period` 是 Docker 的重试机制，不算墙钟断言，但别把它当计时器用。
5. `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check`、`go vet`、`gofmt`、
   `go test -race` 全干净。你多半不动 Rust/Go 源码，但 `scripts/check.sh` 照样要绿。
6. 密钥只从配置点名的环境变量读，任何日志 / 错误 / Debug 输出不得出现取值。
   **这一轨有个专属形态**：`docker compose config` 会把 `${VAR}` 解析成取值 —— 回执、CI 日志、
   Makefile target 里都不许出现裸的 `docker compose config`；`ci.yml:65` 的 `env -u` 写法是对的口径。
7. **每条结论挂实测。**「应该会」「大概」一句不要。改了 CI / compose 的，要能说出「把被测行为破坏掉，
   这条会不会红」，并把**破坏→红、还原→绿**两次输出贴出来。**这一轨最要紧的那一次是 ⑤ 里的
   `bash -lc` 回退实验。**
8. **不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v1` 分支上，回执贴出来。
9. **六轨同时在跑，你这一轨吃 Docker 最狠。** cargo 构建锁与 Docker daemon 是全机共享的，
   你的命令可能排队几分钟，这是正常的，别以为卡死。另外三条硬的：
   **① 绝对不要 `docker system prune` / `docker builder prune -a` / `docker image prune -a`** ——
   会把别的轨正在用的镜像和缓存一起清掉；
   **② 绝对不要按 `label=aite.task` 批量 `docker rm -f`** —— 那是别的轨沙箱测试正在跑的容器；
   **③ `docker compose down -v` 只收 `name: aite`（compose line 46）这个 project 的卷**，
   收之前确认没有别的轨正在用 compose。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v1

scripts/check.sh                                     # 期望最后一行「全部通过」，退出码 0
git status --short                                   # 期望空 —— 跑完整轮 compose 之后也必须空
du -sh core/target                                   # 期望与开跑前一致（构建产物没写进仓库）

docker compose build                                 # 期望两个镜像都出来，退出码 0
docker compose up -d                                 # 期望两个 service 都 running
docker compose ps                                    # 期望看得出健康状态
docker compose logs --tail=50 core edge              # 期望两边的起飞日志都在
docker inspect --format '{{.RestartCount}} {{.State.Status}}' aite-core-1 aite-edge-1
                                                     # 期望 0 running / 0 running（对齐台账 §2.1 的实测）
docker compose down -v
docker ps -a --filter label=aite.task -q | wc -l     # 期望 0

env -u FEISHU_APP_ID -u FEISHU_APP_SECRET -u FEISHU_BOT_OPEN_ID -u AITE_MODEL_API_KEY \
  docker compose config -q ; echo $?                 # 期望 0（**别跑不带 -q / 不带 env -u 的版本**）
```

CI 那一步在本地过一遍（把 `ci.yml` 新那一步的命令逐条手跑），并做 ⑤ 的破坏→还原实验。

冷/热启动各掐一次表，两个数进回执：

```bash
docker compose down -v && time docker compose build --no-cache   # 冷
docker compose down    && time docker compose up -d              # 热（第二次起要快）
```

## 回执格式

```
## V1 回执

基线 0bc8d55 → 提交 <短 sha>

### ① 真镜像
形态：<多阶段的分层怎么切；运行基底选了什么、为什么>
system_prompt_path 怎么处理的：<烤进镜像同路径 / 改配置路径>
.dockerignore 排了什么：<>
沙箱镜像怎么保证：<compose profile / Makefile target / 别的>

三个坑没回来的证据（逐条贴命令输出）：
① PATH  ：<>
② protoc：<>
③ cwd   ：<>
④ socket：<>

### ② 构建产物出仓库
病着的证据（改前）：$ du -sh core/target 前后 + 宿主机重编的输出
改后：$ git status --short → <空>
      $ du -sh core/target → <与开跑前一致>
Go 依赖 down→up 的耗时：<改前 X 秒 → 改后 Y 秒>

### ③ healthcheck
edge：<怎么探的>
core：选了 (a)/(b)/(c) 哪条 —— <为什么>；它在「core 崩溃循环」下会不会变红：<实测>
确认没加 depends_on：$ grep -n depends_on docker-compose.yml → <空>

### ④ 日志
logging 段怎么写的：<>
三个观察窗的 compose 版命令（交文档轨）：<三行>
交出去的：edge 日志级别硬编码（main.go:75），compose 里调不高 —— 归 <哪一轨/总管>

### ⑤ CI compose 冒烟
选了路线 <A/B/C>，因为 <>
现有那一步抓不到的硬证据：
$ <把 compose 改回 bash -lc 后跑现行 ci 那一步>
<照样绿的输出>
新那一步：破坏 → 红 <输出>；还原 → 绿 <输出>
四条 blocker 对照：
| blocker | 新步骤抓得到？ | 说明 |
耗时：冷 build <> / 热 <>；相对 ci.yml:17 的 30 分钟上限还剩 <>

### ⑥ Makefile / README
Makefile 改了什么：<>
交给 V3 的 README 改动：<第 90 行现在成立/不成立；CI 章节 185-189 建议改成什么>

### 实测输出（粘实际的）
$ scripts/check.sh
<最后 3 行>
$ docker compose ps
<>
$ docker inspect --format '{{.RestartCount}} {{.State.Status}}' aite-core-1 aite-edge-1
<>
$ docker ps -a --filter label=aite.task -q | wc -l
<>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v1` 分支上，回执贴出来。
