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

容器方式：

```bash
docker compose up -d          # core + edge 两个 service，共享 data/run 卷
```

> ⚠️ `docker compose config` 会把 `${VAR}` **解析成取值**再打出来，它的完整输出是带密钥的。
> 要贴给别人用 `docker compose config --services`。

停机：`SIGTERM` 走优雅退出（停投递 → 等在跑的任务善终，宽限 20s → 还沙箱 → 关库），
退出码 0；**再来一次**信号立刻硬退，退出码 130。

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
`--offline` 只跑 1、2、7（不碰网络也不碰 docker，适合没凭证的机器）。
`--chat-id <测试群 chat_id>` 会顺带真调一次群历史，回答 spec §3.7(b) 那条待核实项。

> ⚠️ **`--offline` 全绿不等于起得来**：它跳过第 5 组，而配置样例里 `model.base_url` /
> `model.model` 是空的 —— 实测 `--offline` 报 `FAIL 0`，`aite run` 照样退出码 2。
> 真机起飞前那一遍必须不带 `--offline`。
>
> ℹ️ **CI 里目前没有 preflight 这一步**（`ci.yml` 十步全列在下面「CI」那一节）。
> 要不要加是另一回事，这里只如实说现状。

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

`.github/workflows/ci.yml`，一步一条判据：A1 编译 → A2 编译 → A3 契约锁 →
A4 lint → A5 测试可编译 → C1 契约测试 → B 全量测试 → B go test `-race` →
**B8 评测（硬门禁，要 `passed 10/10`）** → compose 双 service 可解析。

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
  渲染那条路（`buildActions`）没删，SDK 放开钩子后改回去即可。
- **乱序重推的话题追问会被丢弃**：追问被平台重推在它的 root 之前时，到达那一刻话题
  会话还不存在、它自己又没 @，于是命中路由 R8「其余丢弃」。飞书的重推通常保序，
  所以这是「乱序时才炸」而不是常态；要补得靠事件级的重排或缓冲，属于路由规则本身要改。
