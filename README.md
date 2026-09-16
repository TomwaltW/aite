# Aite

飞书单平台闭环 P0：群里 @Aite → 线程绑定会话 → Checklist 卡片原地更新 → 沙箱执行 →
结果与文件回到线程。

唯一权威文档是 [`docs/dev-spec-2026-09-11-rustgo.md`](docs/dev-spec-2026-09-11-rustgo.md)（已冻结）。
本 README 只讲「怎么跑起来」；口径冲突时以 spec 为准。

> **2026-09-12：Python 树（`aite/`、`tests/`、`pyproject.toml`、`scripts/*.py`）已整体删除。**
> 它当过一整轮的移植规格，代码层面的账记在
> [`review/inventory-core.md`](review/inventory-core.md)、
> [`review/inventory-feishu.md`](review/inventory-feishu.md)、
> [`review/inventory-gateway-evals.md`](review/inventory-gateway-evals.md) ——
> 那三份是**已删代码**唯一的存世记录，别删。
> （Python 时代那份 45KB 的规格 [`docs/dev-spec-2026-09-09.md`](docs/dev-spec-2026-09-09.md)
> **也还在树里、也还是冻结的**：P0 的人工验收 M1–M6 那张表（§2.4）和飞书权限待核实项（§3.7）
> 就在它里面，所以它不是残留，别当成可以清的东西。`scripts/` 现在只剩 `check.sh` 一个文件。）

## 两个进程

```
                    ┌──────────── edge（Go，aite-edge）────────────┐
  飞书 WS 长连接 ──▶│ 归一化 ──▶ IngressService.HandleEvent（1s）──┼──▶ core
  飞书 REST     ◀──│ PlatformService（发文本 / 卡片 / 文件 / 表情）│◀── core
  Docker daemon ◀──│ SandboxService（起容器、跑代码、收文件）      │◀── core
                    └──── 监听 data/run/aite-edge.sock ───────────┘

                    ┌──────────── core（Rust，aite run）───────────┐
                    │ ControlPlane 路由 → 串行派发 → AgentWorker   │
                    │ SQLite 会话存储 · 证据链落盘 · 模型客户端    │
                    └──── 监听 data/run/aite-core.sock ───────────┘
```

- **状态与判定在 core（Rust）**，**对外连接在 edge（Go）**，两边只通过
  [`proto/aite/v1/`](proto/aite/v1) 的 gRPC 契约说话（unix socket）。
- **启动顺序无关**：谁先起都行，另一边按 1→2→…→30s 退避重连，进程不退出。
  core 起飞时问一次 edge 的 `GetStatus`，`contract_version` 不等就拒绝起飞。
  **edge 完全没起时 `aite run` 照样起得来** —— 问不到就等满 5 次（约 4s）、记一行
  `aite.edge_unreachable` 继续走，一路走到 `ingress.listening`（core 侧那个监听是本地的，
  不需要 edge）；能力表问不到也只是一行 `edge.capabilities_unavailable` 的 warn。
  edge 后起时，重连探针拨通那一下会补比一次版本（契约闸门，`edge-client/src/gate.rs`）。
  这是**刻意**的，不是漏判；回归钉在 `core/crates/app/tests/signals.rs`。
- **沙箱不是第三个进程**：edge 用 Docker client 按任务起**兄弟容器**
  （标签 `aite.task=<task_id>`），所以只有 edge 需要 `/var/run/docker.sock`。
- **单副本**：同一飞书应用的多副本长连接只有一个能收到事件。

## 环境

- rustup（版本钉在 `core/rust-toolchain.toml`）、Go ≥ 1.27、Docker（跑沙箱才要）
- macOS 上 brew 装的 rustup 把 cargo 放在 `/opt/homebrew/opt/rustup/bin`，
  Go 插件在 `~/go/bin` —— 两个都要在 `PATH` 上
- 依赖表冻结在 spec §2.4，新增依赖要先找总管

## 跑起来

```bash
cp config/aite.example.yaml config/aite.yaml     # 按需改；aite.yaml 不入库
export FEISHU_APP_ID=... FEISHU_APP_SECRET=... FEISHU_BOT_OPEN_ID=...   # edge 用
export AITE_MODEL_API_KEY=...                    # core 用；变量名由 config 的 *_env 决定

docker build -t aite-sandbox:p0 docker/sandbox   # 沙箱镜像
make build                                       # 编 core + edge（**不产出 edge 二进制**，见下）
cd edge && go build -o bin/aite-edge ./cmd/aite-edge && cd ..   # edge 的二进制要自己 -o

core/target/debug/aite preflight                 # 起飞前自检，七项各一行结论
edge/bin/aite-edge --config config/aite.yaml &   # 两个进程都**在仓库根**跑，见下
core/target/debug/aite run                       # 组装并起飞
```

> ⚠️ **两条都必须在仓库根跑，别 `cd edge`。** config 里的相对路径按契约全相对仓库根，
> 而 edge 把 `edge.edge_socket` 原样交给 `ListenUnix`，后者会 `MkdirAll` 把目录
> **静默建在当前 cwd 下** —— 在 `edge/` 里起，socket 就落到 `edge/data/run/`，
> core 去连 `<仓库根>/data/run/`，**两个进程永远连不上，而且两边日志都不报错**。
> 判据：`ls edge/data` 必须是 `No such file or directory`。
> 完整复现与起飞日志长什么样见 [`docs/acceptance-M.md`](docs/acceptance-M.md) §0.2。
>
> ⚠️ **`make build` 不产出 `edge/bin/aite-edge`。** 它跑的是 `cd edge && go build ./...`，
> 而 Go 在包列表多于一个时只做编译检查、丢弃产物（`make clean` 里那句 `rm -rf edge/bin`
> 是个历史遗留）。要二进制就得像上面那样自己给 `-o`。`edge/bin/` 不入库。
> 不想编二进制就在**仓库根**敲 `go run ./edge/cmd/aite-edge --config config/aite.yaml`。

**密钥只走环境变量。** 配置文件里存的是变量名（`api_key_env: AITE_MODEL_API_KEY`），
不是取值 —— 别把密钥写进 `config/aite.yaml`，也别写进代码。
`aite preflight` 的任何输出都不会出现取值（只报变量在不在）。

容器方式（**两个多阶段真镜像，不是把仓库挂进去现编**）：

```bash
make compose-up      # = 先建三个镜像（core / edge / 沙箱），再 docker compose up -d，再 ps
make compose-ps      # 看谁就绪：STATUS 一栏带 healthy / Restarting
make compose-logs    # = docker compose logs -f --tail=200 core edge
make compose-down    # 停（要连命名卷一起收自己加 -v）
make compose-config  # 只校验编排能不能解析，不打印取值
```

> ⚠️ **镜像不在就起不来，别直接 `docker compose up -d`。** 运行层是
> `debian:trixie-slim` + 一个二进制，编译全在 builder 阶段
> （`docker/core/Dockerfile`、`docker/edge/Dockerfile`），仓库树一个字节都不碰。
> `make compose-up` 依赖 `compose-build`，走的是 `docker compose --profile images build`
> —— **连 `aite-sandbox:p0` 一起建**（它在 `images` profile 里，`docker compose up` 和
> `config --services` 都看不见它）。edge 只 `ImageInspect`、**不 pull**，镜像不在就报
> 「沙箱镜像不存在」。

两个 service 有 healthcheck，但**判据不一样**，别照抄：

- **edge** 探标准 gRPC health（`grpc-health-probe`，探针二进制烤在镜像里），
  与三个业务服务共用同一个 unix socket。
- **core 没有** gRPC health service，判据是 `nc -U -z` **真 connect** ingress socket。
  **不能用 `test -S`**：SIGKILL / panic / OOM 不走 `ingress.stop()`，socket 文件会留在
  命名卷里，`test -S` 返回 0 是**假绿**。
- 两个 healthcheck **都不许**被拿去当启动顺序用（刻意不写 `depends_on`）——
  启动顺序无关是设计，它们只是让人在 `docker compose ps` 里看得见谁就绪。

两个 service 都配了日志轮转 `max-size 10m` / `max-file 5`：默认 json-file 驱动无上限，
配上 `restart: unless-stopped`，崩溃循环时日志涨得很快。

**两个容器以非 root 跑（2026-09-13 起）。Linux 上起飞前要多做两件事：**

```bash
mkdir -p data                                              # ① 先建出来，别让 dockerd 建
AITE_UID=$(id -u) AITE_GID=$(id -g) \
AITE_DOCKER_GID=$(stat -c '%g' /var/run/docker.sock) \
  docker compose up -d                                     # ② 三个变量传进去
```

- **① `mkdir -p data`** —— `data/` 不入库，不先建的话 bind mount 时由 dockerd 建成
  `root:root 0755`，非 root 的容器当场写不进去（症状：`aite 起不来：建不出目录
  data/evidence：Permission denied`，配 `restart: unless-stopped` 就是崩溃循环）。
  先建出来它就归当前用户，与 `user:` 的取值对得上。
- **② `AITE_UID` / `AITE_GID`** —— 镜像里的默认身份是 `1001:1001`，而 `./data` 是
  bind mount、宿主机那边归**当前用户**，uid 每台机器都不一样，只能从宿主机传进来。
  **两个 service 必须是同一个 uid**：它们互相 connect 对方的 unix socket，而 connect
  要 socket 文件的写权限，socket 是 `srwxr-xr-x`、只有属主有写位。
- **② `AITE_DOCKER_GID`** —— 只有 edge 用：它要读 `/var/run/docker.sock`。这个 gid
  每台宿主机不一样（Linux 上一般是 `docker` 组，Docker Desktop 上是 0），**默认值 0
  在 Linux 上是错的**。传错不是静默失败：`aite preflight` 第 6 组会 FAIL 并点名沙箱不可用。
- **macOS 上这三条都不用管**：Docker Desktop 的 bind mount 过 VirtioFS，会双向翻译 uid
  （容器里看见的是它自己的 uid，宿主机看见的是当前用户），`make compose-up` 照旧。
  也正因为翻译，**macOS 上验不出这套东西对不对** —— 判据在 CI（Linux runner）那一侧。
- 换了 `AITE_UID` 之后要 `docker compose down -v`：`run:` 那个命名卷是 sticky 的
  （`1777`），上一个 uid 留下的残留 socket 新 uid 删不掉。会响，不是静默。

> ℹ️ `make compose-up` **不传这三个变量**（`Makefile` 里那条 target 就是
> `docker compose up -d`）。macOS 上没影响；Linux 上要么按上面那样手敲，要么先
> `export AITE_UID=$(id -u) AITE_GID=$(id -g) AITE_DOCKER_GID=$(stat -c '%g' /var/run/docker.sock)`
> 再 `make compose-up`。

> ⚠️ `docker compose config` 会把 `${VAR}` **解析成取值**再打出来，它的完整输出是带密钥的。
> 要贴给别人用 `docker compose config --services`；要校验语法用 `make compose-config`，
> 它已经是 `env -u` 清掉四个密钥变量之后再跑 `config -q` 的口径（与 CI 同源）。

容器形态与手起飞的差别（socket 在命名卷里看不见、`docker compose exec` 不过 ENTRYPOINT、
四个观察窗各是什么命令）见 [`docs/acceptance-M.md`](docs/acceptance-M.md) §0.2.5。

停机：`SIGTERM` 走优雅退出（停投递 → 等在跑的任务善终，宽限 20s → 还沙箱 → 关库），
退出码 0；**再来一次**信号立刻硬退，退出码 130。
**起飞还没走完时收到也算数**（compose 的 `stop_grace_period`、k8s 滚动更新都会这么来）——
信号会被记住，起飞一走完立刻进收尾。回归见 `core/crates/app/tests/signals.rs`。

## 命令面

`aite` 是 core 侧唯一的可执行入口：

```bash
aite run [--config PATH] [--grace SEC] [--traceback]   # 组装并起飞
aite preflight [--offline] [--json] [--chat-id ID]     # 起飞前自检（七组）
aite evals run evals/p0 --platform fake --model scripted
aite evidence show <task_id> [--config PATH] [--list] [--only KINDS] [--tail N] [--json]
aite contracts lock --check
```

`aite preflight` 的七组：配置可加载 / 环境变量齐 / 飞书凭证有效 / 机器人身份对得上 /
模型端点通 / 沙箱可用 / 落盘目录可写。任一 FAIL → 退出 1，**一项失败不阻断后面的**；
第 1 组问的是「这份配置**起得来**吗」而不只是「yaml 解析得出来吗」，四件事一次报齐：
yaml 解析得出来、`worker.system_prompt_path` **指到的文件真的在**、
`platform` / `model.provider` 的取值**不需要注入**、`storage.sqlite_path` 上**已经有的
那个文件真能当库打开**（后三件读不到 / 要注入 / 读得到但不是库都是 FAIL ——
它们是硬起飞前提，`aite run` 遇上就拒绝起飞，退出码 2。最后那条**纯读**，
文件不在不算问题：那是正常路径，起飞时自己建一个空库）；
`platform: fake` 那一档下第 2/3/4 组 SKIP（fake 平台不连飞书，那三组的判据不适用）。
`--offline` 只跑 1、2、7（不碰网络也不碰 docker，适合没凭证的机器；**CI 用的就是这一档**）。
`--chat-id <测试群 chat_id>` 会顺带真调一次群历史，回答 spec §3.7(b) 那条待核实项。

> ⚠️ **`--offline` 全绿不等于起得来**：它跳过第 5 组，而配置样例里 `model.base_url` /
> `model.model` 是空的 —— 实测 `--offline` 报 `FAIL 0`，`aite run` 照样退出码 2。
> 真机起飞前那一遍必须不带 `--offline`。
>
> 同族的口子当场咬过人三次，**现在都补上了**，三次都归第 1 组、**`--offline` 下照样跑**
> （它不碰网络也不碰 docker）：
>
> * 2026-09-12：配置里的 `worker.system_prompt_path` 指着当天删掉的 Python 树
>   （`aite/worker/prompts/`），七组当时没有一组碰它 —— preflight 报「可以起飞」，
>   `aite run` 退出码 2。
> * 2026-09-13：配置是 `platform: fake`（或 `model.provider: scripted`）。这两个取值
>   **只能被注入着用**，而 `aite run` 不注入任何实现，`build_app` 明文拒绝 —— 当时七组
>   一样没有一组问「这个取值自己起得来吗」，`--offline` 报「全部没红，可以起飞」退出 0，
>   `aite run` 退出码 2。同一份配置还在要飞书凭证（fake 平台压根不连飞书），
>   所以那一档下第 2/3/4 组现在一并 SKIP。
> * 2026-09-13：`storage.sqlite_path` 指着一个**已经存在、但内容不是 SQLite** 的文件。
>   `--offline` 与全跑**都全绿**，而 `aite run` 退出码 2 ——
>   `aite 起不来：建表失败（…）：sqlite: file is not a database`。第 7 组接不住它：
>   那一组问的是三个路径的最近已存在祖先**目录**写不写得进去，不是这个**文件**是不是个库。
>
> ℹ️ **CI 里跑的是 `--offline` 这一档，而且只在 `compose-smoke` 那个 job 里**
> （`.github/workflows/ci.yml` 里那一步就叫「preflight --offline」，命令是
> `docker compose run --rm core preflight --offline` —— 在真镜像里跑，不是在 runner 上）。
> **刻意不写行号**：这两份文件已经在互指，再钉一个行号只会把「漂了不会红」的面翻倍
> （同一条理由见 `ci.yml` 里引 README 那句的注释）。2026-09-13 这个行号就漂了一次
> ——`compose-smoke` 前面插了「容器身份」那一步。
> `checks` 那个 job 从 A1 到 B8 **没有 preflight**。
> 原因是 `config/aite.yaml` 不入库，CI 得自己造一份，而造配置那一步本来就只有
> `compose-smoke` 需要。

## 验收

spec §4 那几条，`make` 里一条一个 target：

```bash
make check          # = scripts/check.sh，把每条的实际输出都打出来（写回执用这个）
make build          # A1/A2
make lock           # A3/C2 契约锁 --check
make lint           # A4 clippy -D warnings / fmt --check / go vet / gofmt
make test           # B 全量 cargo test + go test
make evals          # B8 十个 P0 场景，最后一行 passed 10/10
```

要 Docker 的那两条单独跑：

```bash
cd edge && go test -tags docker ./internal/sandbox/... -count=1   # B4 真容器
docker ps -a --filter label=aite.task -q | wc -l                  # 跑完必须是 0
```

## 目录

```
proto/aite/v1/          冻结契约（跨进程）：events / outbound / capabilities / sandbox / edge
core/                   Rust workspace
  crates/contracts      冻结契约：domain 类型 + trait + 常量 + hash / 配置形状
  crates/proto          tonic 生成代码 + domain↔pb 互转 + gRPC status 映射
  crates/store          SqliteSessionStore
  crates/evidence       FileEvidenceWriter + `aite evidence show`
  crates/control        ControlPlane（路由 R1–R8）+ Ingress + 命令
  crates/worker         AgentWorker（W1–W9）+ 卡片 + 上下文 + prompts/platform.md
  crates/models         OpenAI 兼容 ModelPort
  crates/gateway        P0ToolGateway + 五个工具 + schema 校验
  crates/edge-client    到 edge 的 gRPC 客户端（两个 Port）+ core 侧 IngressServer
  crates/testing        官方测试替身 + CallLog + samples
  crates/evals          评测 runner / 场景 / checks / protocol probe
  crates/app            `aite` 二进制：组装起飞、preflight、契约锁
edge/                   Go module
  internal/feishu       飞书 adapter：长连接、归一化、REST 出站
  internal/sandbox      Docker 沙箱
  internal/ingress      edge → core 的客户端
  cmd/aite-edge         守护进程入口
evals/p0/               十个验收场景（原文件不动）
docker/sandbox/         沙箱镜像
review/inventory-*.md   三份移植清单（已删 Python 代码唯一的存世记录）
docs/dev-spec-*.md      两份冻结规格：2026-09-09 那份带 M1–M6 与飞书权限，09-11 那份是 Rust/Go 重写
docs/acceptance-M.md    M1–M6 人工验收剧本（真实飞书测试群）
docs/demo-3min.md       3 分钟演示分镜
```

## 组装面（`core/crates/app`）

`build_app(config, injections)` **只组装**：不连网、不起容器、不发消息，唯一的副作用是
按 `StorageConfig` 建那三个落盘目录。三个口子（platform / model / sandbox）给了就用给的，
不给才按 config 去连 edge —— `core/crates/app/tests/` 那 50 条集成测试全靠它注入替身，
测的是**真实接线**而不是替身之间的默契。

`config.platform: fake` 是「**必须注入**」的标记，不是「内建替身」：不注入就
`StartupError`，不会去连真实飞书。

`run_app` 的退出序列是冻结的，一步都不跳：

```
platform.stop()                     先闭嘴，不再收新事件
plane.join() 限时 grace（默认 20s）  在跑 / 排队的任务收尾
  超时 → 先抄 worker.in_flight      取消之后就再也问不出它们是谁了
runner.abort()
cancel_task(notify=false) × stranded 给硬取消的任务善终（证据 + 卡片 + 还沙箱）
sandbox.close_all()
store.close()
```

起飞时还会把**上一条命的残局**收干净（库里 failed + 群里回帖 + 卡片置 failed +
证据链收口），而且排在 `platform.start()` **之前**。

## 评测（`core/crates/evals`）

```bash
aite evals run evals/p0 --list                            # 列出场景名
aite evals run evals/p0 --platform fake --model scripted  # 最后一行 passed k/10
aite evals run evals/p0 --only 04_csv_to_chart --traceback
aite evals run evals/p0 --only 04_csv_to_chart --sandbox docker   # 真容器真跑
aite evals run evals/p0 --only 04_csv_to_chart --model live       # 真模型
```

stdout 是「一份 JSON 摘要 + 最后一行 `passed k/10`」；全过退出 0，否则 1，
参数 / 环境问题 2。**scripted 那一档 stderr 一个字节都没有**（CI 与 `check.sh` 读的就是它）。
每个没过的场景给一句人话原因和阶段标记（`wiring` / `dispatch` / `drive` / `assert` /
`error` / `ok`），不吐异常栈。

`--sandbox docker` 与 `--model live` 需要 `aite-edge` 在跑（真沙箱在 Go 那一侧）；
前者起飞前会连 daemon、真起一个容器跑一遍四个 import，不行就一行人话 + 退出 2，
不会让十个场景各自烂在第一个工具调用上。

场景文件长什么样、`expect` 有哪些 check —— 见 [`evals/README.md`](evals/README.md)。

## CI

`.github/workflows/ci.yml`，**两个并行 job**，各拿一份时间预算。

**`checks`**（timeout 30 分钟）—— 一步一条判据：A1 编译 → A2 编译 → A3 契约锁 →
A4 lint → A5 测试可编译 → C1 契约测试 → B 全量测试 → B go test `-race` →
**B8 评测（硬门禁，要 `passed 10/10`）**。

**`compose-smoke`**（timeout 25 分钟）—— 真把两个容器起起来，九步：

1. **双 service 可解析** —— `env -u` 清掉四个密钥变量再 `docker compose config --services`，
   断言正好是 `core edge`（沙箱那个 service 在 `images` profile 里，这里看不见它）。
   这一步很便宜，留着是为了在花 6 分钟 build 之前先抓到 YAML 写坏。
2. **造 CI 用的两份配置** —— `config/aite.yaml` 不入库，checkout 里根本没有。两份而不是
   一份：core 与 edge 读的是**同一个 `platform` 字段**，含义却相反 —— core 配 `fake` 直接
   拒绝起飞（所以 core 必须 `feishu`，且要把 example 里空着的 `base_url` / `model` 填上），
   edge 配 `feishu` + 假凭证会崩溃循环（所以 edge 必须 `fake`）。
3. **容器身份** —— 两个容器以非 root 跑，所以 `AITE_UID` / `AITE_GID` 问 runner 自己
   （`id -u` / `id -g`）、`AITE_DOCKER_GID` 问 `/var/run/docker.sock` 自己
   （`stat -c '%g'`），**三个都是问出来的、不写死**；再 `mkdir -p data`，
   否则 bind mount 时由 dockerd 建成 `root:root`，非 root 的容器写不进去。
4. **两个镜像真编出来** —— `docker compose build`。这一步就是「rust 镜像里没 protoc」那条
   blocker 的门禁：protoc 缺了 `build.rs` 当场炸。沙箱镜像刻意不建（起飞与冒烟都不需要它）。
5. **`preflight --offline`** —— `docker compose run --rm core preflight --offline`，
   在真镜像里跑（镜像的 `ENTRYPOINT` 就是 `aite`，命令里不用再写一遍）。
6. **起飞冒烟：两个 socket 真连上** —— `up -d` 之后轮询两边 healthcheck 转 `healthy`
   （上限 40 × 3s），然后逐条断言：两个 socket 都在共享卷 `/app/data/run` 里、
   两个进程 cwd 都是 `/app`、运行层里**压根没有工具链**（`cargo` / `protoc` / `go` 都不在）、
   core 侧四行起飞日志（`edge.connected` / `aite.edge_status` / `ingress.listening` / `aite.up`，
   先剥 ANSI 再 grep —— `--no-color` 只关 compose 自己的行前缀，管不到应用吐的字节）、
   两个容器的 `RestartCount` 都是 0。
7. **⑤ `./data` 的产物宿主机读得动** —— 打印每个产物的 `%u:%g %a`，**硬断言每个文件
   `head -c1` 得动**。守的是权限位与 umask，跟 owner 是谁无关。
8. **⑥ 宿主机往 `./data` 里写得进** —— 降权之后新增的承诺。两条硬判据：产物的 owner
   必须是 runner 自己；宿主机真做得了「直跑 `aite run`」要做的三件事（建目录、建文件、
   往 `data/aite.db` 里写）。
9. **收干净** —— `always()` 跑 `down -v`，并断言 `label=aite.task` 的容器数是 0。
   （另有一条 `failure()` 才跑的步骤，把两边日志、`ps -a`、退出码与 `RestartCount`
   都打出来 —— 它不是判据，是红了之后不用重跑就能看现场。）

> 第 6 步为什么值得单列：到 RΩ 合并前 compose 的门禁只有第 1 步那条纯解析，
> 而那次真 `up` 一撞就是四条 blocker（登录 shell 洗掉 PATH、rust 镜像里没 protoc、
> cwd 不是仓库根、socket 落到非共享卷），**纯解析一条都抓不到** ——
> 实测把 compose 退回坏的那一份，第 1 步退出码照样 0。
>
> 第 7、8 两步为什么不合并：它们守的是两件会**各自单独退化**的事。实测把两个 `user:`
> 改回 `"0:0"`（= 容器退回 root）：第 8 步红并点名 `data/aite.db` / `data/evidence` /
> `data/artifacts`，而**第 7 步照样绿**（0644 + 0755 对任何用户仍然开着读）。
> 反过来把 `data/aite.db` 改成 0600：第 7 步红，第 8 步的 owner 那一半照样过。

## 已知边界

- **卡片上的「停止」「证据」按钮当前不渲染。** lark-oapi-go v3.12.0 在长连接上把非
  event 帧整条丢弃（`ws/client_message.go:79`），唯一的 `WithCardHandler` 钩子是
  注释掉的 —— 渲染出来的按钮点了一定没反应，那比不渲染更糟。卡片上改成一行提示
  ——「要停这个任务：在本话题里回复 `!stop <任务号>`，或在群里发 `@我 !stop <任务号>`」。
  **提示里的投递条件是必须的**：路由 R5 收命令的条件是「已 @ 机器人」或「已在话题内」，
  两者都不满足的一条群消息会一路落到 R8「其余丢弃」，用户那边零回复 —— 一句
  「请在群里发 `!stop`」等于又造了一个点了没反应的按钮。卡片本身是
  `reply_in_thread=true` 发进任务话题的，所以「在本话题里回复」这条路一定走得通。
  `!stop` / `!status` 走的是普通消息事件，不受 SDK 那个缺陷影响。
  渲染那条路（`buildActions`）没删，但**「SDK 放开钩子后改回去即可」这句不准**
  （BB5 2026-09-15 一手复核 v3.12.0 源码，模块 zip 的 h1 与 sum.golang.org 逐字一致）：
  上游要改的是**两处** —— 只取消注释 `WithCardHandler`（`ws/client.go:56-60`）没用，
  `ws/client_message.go:79` 那道 `type` 闸门也得放行（`cardHandler` 字段现在没人写也没人读）。
  且 v3.12.0 已是 proxy.golang.org 上的最新版、开发主干 `v3_main` 里那五行仍是注释，
  上游自己的 card 例子走的是 HTTP webhook —— **当前没有可升的版本**。
- **「看证据」在群里够不着，而这一条不是 SDK 的错。** 契约 R3 的 `evidence` 动作 core 侧
  实现着也测着（`plane.rs` 回一行「任务 <task_id> 的证据目录：<路径>」），但**没有任何
  生产者往卡片的 `actions` 里写它** —— `worker/src/card.rs:83` 与 `control/src/card.rs:83`
  都硬编码 `vec![Stop]`。所以就算 SDK 闸门放开、按钮接回来，「证据」也一个都不会渲染出来：
  按钮那条路修好了也交付不了这个能力。群里唯一够得着的入口只能是命令，而当前只有 CLI
  （`aite evidence show <task_id>` —— 它吃的是 `task_id`，卡片上只有 `#A17` 这种任务号，
  所以要先 `--list` 对一遍）。补法（新增 `!evidence <任务号>`）的规格见
  `review/spec-bb5-evidence-command.md`；core 侧的路由与文案尚未落地。
- **乱序重推的话题追问会被丢弃**：追问被平台重推在它的 root 之前时，到达那一刻话题
  会话还不存在、它自己又没 @，于是命中路由 R8「其余丢弃」。飞书的重推通常保序，
  所以这是「乱序时才炸」而不是常态；要补得靠事件级的重排或缓冲，属于路由规则本身要改。
