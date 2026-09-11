# 开发派工 Spec — Aite Rust + Go 重写（P0 功能对等）

> 生成时间：2026-09-11
> 这是本轮并行开发的唯一权威文档。执行者只读这一个文件 + `review/inventory-*.md` 三份移植清单 + 自己那份派单。
> 本文件冻结后不再修改（守卫按 `docs/dev-spec-*.md` 通配保护）；需要变更请停下来找总管（沈思锴）。
> 上游文档：`docs/dev-spec-2026-09-09.md`（Python 版 P0 spec，功能规格与失败面仍以它为准）。本文与它冲突时以本文为准。

---

## 1. 问题定义

**要做什么**（一句话）：
把已经在 Python 上跑通的 Aite P0（T0–T24，`pytest` 1161 条、评测 `passed 10/10`、真模型 × 真沙箱端到端走通）**整体重写成 Rust + Go**，功能、失败面、回帖文案、证据链 hash 口径与 Python 版逐条对等；P0 的 10 个评测场景（`evals/p0/*.yaml`，**原文件不动**）在新实现上 `passed 10/10`。

**为什么要做**：
项目定了「aite 只能用 Rust 和 Go 搭建」。Python 树（`aite/`、`tests/`）是**规格与参考**，移植期间只读，RΩ 合流后整体删除。

**语言分工的原则**（每个模块归哪边只看这一条）：

| 归属 | 判据 | 模块 |
|---|---|---|
| **Go（`edge/`）** | 对外连接：有官方 Go SDK 或本身就是 Go 生态的外部系统 | 飞书长连接 + IM/文档 REST（lark oapi-sdk-go）、Docker 沙箱（docker client）、edge 守护进程 |
| **Rust（`core/`）** | 状态与判定：路由规则、任务状态机、hash 链、模型协议、评测 | 契约、ControlPlane、SQLite 会话存储、worker agent loop、Tool Gateway + 五个工具、evidence、模型客户端、测试替身、评测 runner、`aite` 命令面 |
| **proto（`proto/`）** | 两边唯一的跨语言契约 | events / outbound / capabilities / sandbox 四份数据形状 + edge.proto 四个服务 |

**明确不做**（写了就是越界）：
- 任何 P1 功能（Scope / 审批 / 记忆 / 多模型路由 / 管理台 / 钉钉企微）—— 与 Python spec §1 相同
- 「顺手优化」：Python 版里被测试钉住的行为（哪怕看着别扭，如卡片时间用 UTC 小时不补零、`!new <文本>` 不回帖）**原样搬**；想改 → 回执里提，不动手
- 改评测场景文件 `evals/p0/*.yaml`（它们是验收面）
- 改 Python 树（它是参考）
- 换持久化格式：SQLite 表结构与 evidence 落盘布局与 Python 版一致（`evidence show` 要能读旧目录）

---

## 2. 架构

### 2.1 进程模型

```
                    ┌──────────────────────────── edge（Go，aite-edge）────────────────────────────┐
  飞书 WS 长连接 ──▶│ feishu.normalize ──▶ ingress.Client.HandleEvent(core, 1s deadline) ───────────┼──▶ core
  飞书 REST     ◀──│ PlatformService（SendText/SendCard/UpdateCard/SendFile/AddReaction/ReadHistory/…）│◀── core
  Docker daemon ◀──│ SandboxService（Acquire/Exec/PutFile/GetFile/ListFiles/Touch/Release/ReapIdle） │◀── core
                    │ EdgeStatusService（版本、contract_version、连接态、docker 可达）              │
                    └──────────────── 监听 unix socket data/run/aite-edge.sock ───────────────────┘

                    ┌──────────────────────────── core（Rust，aite run）──────────────────────────┐
  IngressService ──▶│ ControlPlane.handle_event（R1–R8：去重 / 建会话 / 建任务 / 入队，1s 内返回）  │
                    │ run_forever：串行派发 → TaskWorker.run（agent loop）；reaper 每 60s reap_idle    │
                    │ worker → ToolGateway.call → tools → SandboxPort / PlatformPort（gRPC 到 edge）   │
                    │ SessionStore（SQLite）  EvidenceWriter（jsonl hash 链）  ModelPort（OpenAI 兼容）│
                    └──────────────── 监听 unix socket data/run/aite-core.sock ──────────────────┘
```

- **两个常驻进程**，各自既是 gRPC server 也是 client，走 Unix domain socket（路径在 `config/aite.yaml` 的 `edge:` 段）。
- **启动顺序无关**：任一方连不上对方就指数退避重连（1s→2s→…→30s 封顶，无限），不退出。core 起飞时调 `GetStatus`，`contract_version != CONTRACT_VERSION` → 拒绝起飞并说清两边版本。
- **事件投递语义**：edge 收到平台事件 → 归一化 → `HandleEvent`（1s deadline）→ 成功才向平台确认；失败（core 不在 / 超时 / INTERNAL）→ 向平台返回错误让其重推；重推的重复事件由 core 靠 `event_id` 去重（R2）。**去重只在 core**，edge 不做。
- **`platform: fake`** 时 core 单进程跑（评测 / 集成测试），PlatformPort / SandboxPort 用 `aite-testing` 的替身，不需要 edge。
- 文件流：`download_attachment` = `PlatformService.DownloadFile` → bytes 经 core → `SandboxService.PutFile`。两跳过 core，P0 接受（附件 ≤ 几十 MB；`edge.max_message_mb` 默认 64）。
- 收尾：SIGTERM/SIGINT → core 先让 edge 停投递（`platform.stop`）→ `plane.join()` 等在跑任务善终（宽限 20s）→ 超时硬取消 + 给硬取消的任务写 cancelled 证据 → 释放沙箱 → 关 SQLite。edge：GracefulStop gRPC → 关长连接。第二次信号硬退（130）。
- **单副本**：同一飞书应用多副本长连接只有一个收到事件（Python spec 附录 A 末条）。

### 2.2 错误约定（冻结，也写在 `proto/aite/v1/edge.proto` 头注释）

| 情况 | gRPC status | core 侧 |
|---|---|---|
| 平台 429/5xx 重试 3 次仍失败、网络断、docker daemon 不可达 | `UNAVAILABLE` | `PlatformError{retryable:true}` / `SandboxError::Unavailable` |
| 超时 | `DEADLINE_EXCEEDED` | retryable:true / `Sandbox::Timeout` |
| 平台 4xx 非 429 | `FAILED_PRECONDITION`（404→`NOT_FOUND`，403→`PERMISSION_DENIED`，400→`INVALID_ARGUMENT`） | retryable:false，`http_status` 按 code 回填 |
| sandbox_id 不存在 / 文件不存在 | `NOT_FOUND`（message 以 `sandbox_not_found:` / `file_not_found:` 开头区分） | `Sandbox::NotFound` / `Sandbox::FileNotFound` |
| 路径不在 /work 下 / 含 `..` | `INVALID_ARGUMENT` | `Sandbox::InvalidPath` |
| 其余 | `INTERNAL` | retryable:false / `Sandbox::Internal` |
| 骨架期未实现 | `UNIMPLEMENTED` | retryable:false |
| core 拒收非法事件（anchor 缺失、枚举 UNSPECIFIED） | `INVALID_ARGUMENT`（edge 不重推，计 `ingress.invalid`） | — |
| core 处理失败（存储抖动等） | `INTERNAL`（edge 计 `ingress.errors`，让平台重推） | — |

`status.message` 形如 `"<code>: <detail>"`。core 映射：`UNAVAILABLE | DEADLINE_EXCEEDED | ABORTED | RESOURCE_EXHAUSTED` → retryable=true，其余 false（`aite-proto::status`，已有测试钉住）。

### 2.3 目录布局

```
proto/aite/v1/{capabilities,events,outbound,sandbox,edge}.proto   冻结契约（跨进程）
core/                               Rust workspace（rust-toolchain.toml 钉 1.98.1）
  crates/contracts   aite-contracts  冻结契约（domain 类型 + trait + 常量 + hash/编码函数 + config 形状）
  crates/proto       aite-proto      tonic 生成代码 + domain↔pb 互转 + status 映射（R0，只读）
  crates/store       aite-store      SqliteSessionStore                                  ← R3
  crates/evidence    aite-evidence   FileEvidenceWriter + `aite evidence show`           ← R3
  crates/control     aite-control    ControlPlane + Ingress + commands                   ← R4
  crates/worker      aite-worker     AgentWorker + card + context + prompts/platform.md  ← R5
  crates/models      aite-models     OpenAiCompatModel                                   ← R5
  crates/gateway     aite-gateway    P0ToolGateway + 五个工具 + schema 校验              ← R6
  crates/edge-client aite-edge-client EdgePlatform / EdgeSandbox（tonic client）+ IngressServer ← R6
  crates/testing     aite-testing    Fake* 替身 + CallLog + samples                      ← R7
  crates/evals       aite-evals      场景 / runner / checks / protocol probe / demo-fixture ← R7
  crates/app         aite（二进制）  run / contracts lock / evals / evidence / preflight  ← RΩ（lock 归 R0）
edge/                               Go module `aite/edge`（go 1.27）
  gen/aitepb/                       protoc 生成（已入库；重生成只在 AITE_RELOCK=1 下 make proto-gen）
  internal/aiteerr  PlatformError / SandboxError → gRPC status（R0，只读）
  internal/config   config/aite.yaml 的子集镜像（R0，只读）
  internal/server   PlatformPort / SandboxPort 接口 + gRPC 直通适配 + health（R0，只读）
  internal/pin      预钉三方库，让 go.mod 稳定（R0，只读）
  internal/feishu   飞书 adapter                                                        ← R1
  internal/sandbox  Docker 沙箱                                                          ← R2
  internal/ingress  edge → core 的 IngressService 客户端                                 ← R2
  cmd/aite-edge     守护进程入口                                                         ← R2
  testdata/feishu   归一化 fixture                                                       ← R1
evals/p0/*.yaml                     10 个场景（原文件不动）
config/aite.example.yaml            契约默认值样例（含 edge: 段）
docker/sandbox/                     沙箱镜像（内容不变）                                 ← R2
docs/dev-spec-2026-09-11-rustgo.md  本文（冻结）
review/inventory-{feishu,core,gateway-evals}.md   移植清单（只读探查 agent 生成）
review/paste-R*.md                  派单
```

### 2.4 库选型（已在 `core/Cargo.toml` / `edge/go.mod` 里钉死；加依赖 → 停下报告）

| 用途 | Rust | Go |
|---|---|---|
| 异步运行时 / gRPC | tokio、tonic 0.14 + tonic-prost、prost | google.golang.org/grpc、protobuf |
| 飞书 | — | github.com/larksuite/oapi-sdk-go/v3 v3.12.0（长连接 `ws`、`event/dispatcher`、`service/im/v1`）；REST 出站用 `net/http` 直打（便于 httptest 断言方法与路径，同 Python 版用 httpx 的理由） |
| Docker | — | github.com/docker/docker v28.5.2（client、api/types/container） |
| 限速 | — | golang.org/x/time/rate |
| SQLite | rusqlite 0.40（bundled）+ `tokio::task::spawn_blocking` | — |
| JSON Schema | jsonschema 0.56 | — |
| HTTP 客户端（模型） | reqwest 0.13（rustls） | — |
| 序列化 | serde / serde_json / serde_yaml | encoding/json、gopkg.in/yaml.v3 |
| hash / id | sha2 + hex、uuid v4、rand | crypto/sha256 |
| 日志 | tracing（事件名沿用 Python 的 logger 消息名，如 `feishu.reconnected`、`ingress.handle_failed`） | log/slog（同上） |
| 测试 | cargo test + tempfile；HTTP 用 wiremock 不引入 —— 模型客户端测试自起 tokio TcpListener 假服务 | testing + httptest；Docker 测试打 build tag `docker` |

---

## 3. 冻结契约

以下在本轮内不可更改。任何任务需要改动 = 停下来报告，不要自己改。守卫（`.claude/hooks/guard_bash.py`）拦写入；`.contracts.lock` 记 sha256，`core/target/debug/aite contracts lock --check` 必须始终 `OK 36 files`。

### 3.0 锁定面与依赖表

- 锁定：`proto/aite/v1/**`、`core/crates/contracts/**`（Cargo.toml + src + tests）、`aite/contracts/**`（Python 旧契约，RΩ 删除时一并重锁）。
- 只读但不入锁：`core/crates/proto/**`、`edge/gen/**`、`edge/internal/{aiteerr,config,server,pin}/**`、`core/Cargo.toml`、`edge/go.mod`、`edge/go.sum`、`.claude/**`、`Makefile`、`scripts/check.sh`、`.github/**`。
- `CONTRACT_VERSION = "p0.2"`（`core/crates/contracts/src/lib.rs`）；edge 的 `server.ContractVersion` 同值，`GetStatus` 回报，core 起飞时比对。

### 3.1 数据形状（与 Python p0.1 逐字段对应）

| Python 契约文件 | proto（跨进程） | Rust domain（core 内部） | Go |
|---|---|---|---|
| capabilities.py | capabilities.proto `PlatformCapabilities` | `capabilities.rs`（`feishu_p0()` 每次返回新值） | pb 直接用；`feishu.FeishuP0()` |
| events.py | events.proto `NormalizedEvent` / `Anchor` / `Attachment` / `CardAction` + 五个枚举 | `events.rs`（serde snake_case 字符串枚举） | pb 直接用（归一化产物就是 pb） |
| outbound.py + ports.py 的 HistoryMessage/DocumentContent | outbound.proto | `outbound.rs` | pb |
| sandbox.py | sandbox.proto | `sandbox.rs`（`EXEC_TIMEOUT_EXIT_CODE = 124`） | pb |
| session.py | —（不跨进程） | `session.rs`（`encode_task_no`、`ACTIVE_TASK_STATUSES`、`TaskStatus::is_active/is_terminal`） | — |
| protocol.py | — | `protocol.rs`（`all_model_tools()` 等，JSON Schema 逐字节同 Python） | — |
| gateway.py | — | `gateway.rs`（`MAX_TOOL_CONTENT_CHARS=12000`，`DEFAULT_TOOL_TIMEOUT_SEC=60`） | — |
| evidence.py | — | `evidence.rs`（`canonical_json` = Python `json.dumps(sort_keys, ensure_ascii=False, separators=(",",":"))`，§3.1 两组向量 + 四组对拍向量已钉） | — |
| config.py | — | `config.rs`（新增 `edge:` 段；`deny_unknown_fields`；`worker.system_prompt_path` 默认改为 `core/crates/worker/prompts/platform.md`） | `internal/config`（子集镜像，`config_test.go` 用样例钉住与 Rust 默认值一致） |

proto 枚举带前缀（`SENDER_KIND_HUMAN`），零值 `*_UNSPECIFIED` 一律视为非法（`aite-proto::convert` 拒收）。proto3 `optional` 表示旧契约的 `| None`。`google.protobuf.Struct` 承载 `raw` / `value`，`Timestamp` 承载时间。

**与 p0.1 的差异表**（就这些，别的都逐字对应）：

| # | 差异 | 为什么 |
|---|---|---|
| D1 | `PlatformPort` 拆成 proto 服务（去掉 start/stop）+ Rust trait（保留 start/stop） | 长连接的生命周期归 edge 进程；core 的 `start(on_event)` 变成"起 IngressService 监听"，`stop` 变成"停止接收" |
| D2 | `ToolGateway` 显式加 `register_task / unregister_task / sandbox_id_of / release_task` | Python 版靠 `getattr` 鸭子探测，Rust 没有这条路，显式化 |
| D3 | `SessionStore` 显式加 `close / recover_orphan_tasks / next_turn_seq`；`EvidenceWriter` 加 `task_dir`；`SandboxPort` 加 `close_all`（默认空实现） | 同上 |
| D4 | 新增 `TaskWorker` trait + `RunHooks{drain_steer, is_cancelled}` | 旧 `AgentWorker.run(task, session, initiator=, drain_steer=, is_cancelled=)` 的形状显式化 |
| D5 | 新增 `ControlPlane` 的 `run_pending / pending / join / cancel_task / counters` | 旧版协议外方法，评测与 app 收尾都在用 |
| D6 | `AiteConfig` 新增 `edge:` 段；未知键报错 | 进程边界；防拼错静默失效（Python 版 pydantic 默认忽略未知键） |
| D7 | 错误类型显式化：`PlatformError{code,message,retryable,http_status}`、`SandboxError{kind,message}`、`StoreError::DuplicateTurn`、`ModelError`、`EvidenceError`、`IngressError` | Python 用异常类层次 |
| D8 | SQLite 加 `PRAGMA busy_timeout=5000` | Rust 里跨实例并发是真线程，不是 asyncio 交错；journal_mode 仍是 `delete`（T18 钉住"崩溃不留 -wal/-journal"） |

### 3.2 调用面（`core/crates/contracts/src/ports.rs`，全部 `#[async_trait]`）

`PlatformPort`、`ModelPort`、`SandboxPort`、`ToolGateway`、`SessionStore`、`EvidenceWriter`、`TaskWorker`、`ControlPlane`，签名以文件为准。实现者：

| trait | 实现 crate / 包 | 替身 |
|---|---|---|
| PlatformPort | aite-edge-client `EdgePlatform`（R6）；Go 侧 `server.PlatformPort` 由 `feishu.Platform` 实现（R1） | aite-testing `FakePlatform`（R7） |
| SandboxPort | aite-edge-client `EdgeSandbox`（R6）；Go 侧 `sandbox.Docker`（R2） | `FakeSandbox`（R7） |
| SessionStore | aite-store `SqliteSessionStore`（R3） | `FakeSessionStore`（R7） |
| EvidenceWriter | aite-evidence `FileEvidenceWriter`（R3） | `FakeEvidenceWriter`（R7） |
| ToolGateway | aite-gateway `P0ToolGateway`（R6） | `FakeToolGateway`（R7） |
| ModelPort | aite-models `OpenAiCompatModel`（R5） | `FakeModel`（脚本化，R7） |
| TaskWorker | aite-worker `AgentWorker`（R5） | 各轨私写 |
| ControlPlane | aite-control `InProcessControlPlane`（R4） | aite-evals 的 `DemoPlane`（R7，只为证明 harness 不是空壳） |

**并行期间各轨要的替身一律在自己 crate 的 `tests/` 或 `src/testing.rs`（`#[cfg(test)]`）里私写**，不要依赖 `aite-testing`（那是 R7 的，并行期间还不存在）。允许的跨 crate 依赖只有：`aite-contracts`（人人）、`aite-proto`（R6）、`aite-testing`（R7 的 evals）。别的 → 停下报告。

**各轨对外暴露的构造入口**（RΩ 组装时按这个接，名字别自己发明）：

| crate | 入口 |
|---|---|
| aite-store | `SqliteSessionStore::open(path) -> Result<Self, StoreError>`（`:memory:` 也认）；`impl SessionStore` |
| aite-evidence | `FileEvidenceWriter::new(evidence_dir) -> Self`；`impl EvidenceWriter`；`cli::run(args) -> i32`（`aite evidence show …`） |
| aite-control | `InProcessControlPlane::new(ControlDeps{store, platform, evidence, config, worker: Option<Arc<dyn TaskWorker>>, gateway: Option<Arc<dyn ToolGateway>>, sandbox: Option<Arc<dyn SandboxPort>>, model_name: String})`；`impl ControlPlane`；`Ingress::new(plane)` + `Ingress::handler() -> EventHandler`（计数 `events.handled / ingress.errors / ingress.slow`） |
| aite-worker | `AgentWorker::new(WorkerDeps{store, platform, model, evidence, config, gateway: Option<…>, sandbox: Option<…>})`；`impl TaskWorker`；`card::{clip, render_card, CardCoalescer}`；`context::{build_context, load_system_prompt}` |
| aite-models | `OpenAiCompatModel::from_config(&ModelConfig, env: &HashMap<String,String>) -> Result<Self, ModelError>`；`impl ModelPort`；`OpenAiCompatModel::with_base_url(...)`（测试注入假端点） |
| aite-gateway | `P0ToolGateway::new(platform: Arc<dyn PlatformPort>, sandbox: Arc<dyn SandboxPort>, spec: SandboxSpec) -> Self`；`impl ToolGateway`；`schema::validate_arguments(&Value, &Map) -> Result<Map, SchemaViolation>` |
| aite-edge-client | `EdgeClient::connect(&EdgeConfig, repo_root: &Path) -> Result<EdgeClient, PlatformError>`（懒连接 + 退避）；`edge.platform() -> Arc<dyn PlatformPort>`、`edge.sandbox() -> Arc<dyn SandboxPort>`、`edge.status() -> Result<EdgeStatus>`；`EdgePlatform::start` 内部起 `IngressServer`（tonic）监听 `core_socket` |
| aite-testing | `FakePlatform / FakeModel / FakeSandbox / FakeToolGateway / FakeSessionStore / FakeEvidenceWriter / CallLog / samples::{png_bytes, PNG_1X1, CSV_SAMPLE}`，语义见 `review/inventory-gateway-evals.md` §5 |
| aite-evals | `cli::run(args) -> i32`；`runner::run_suite(suite_dir, opts, plane_factory: PlaneFactory) -> SuiteResult`；`PlaneFactory = Arc<dyn Fn(Deps) -> Result<Arc<dyn ControlPlane>, String> + Send + Sync>`；`DemoPlane`（R7 自证用） |

### 3.3 失败面、路由规则、Worker 规则

**逐条沿用 Python spec §3.3 / §3.5 / §3.6，外加 T5–T24 补的行为**。它们的实现细节（回帖文案原文、计数器名、证据 payload 形状、重试次数与退避、卡片合并、T19 提示词、T20 重复检测、T21 残行自愈、T22 孤儿收场、T24 seq 锁与 steer 证据）全部记录在 `review/inventory-core.md` 与 `review/inventory-gateway-evals.md`，**以 Python 源码为最终依据**。三条硬约束：

1. **给人看的固定文案逐字不变**（`inventory-core.md` §5 列全了）：评测 `text contains` 断言和 M1–M6 的验收剧本都依赖它们。
2. **证据 payload 的键与语义不变**（`inventory-core.md` §6）：`evidence show` 与旧证据目录互通。
3. **计数器 / 日志事件名不变**（`events.duplicate`、`ingress.errors`、`feishu.reconnected`……）：排障文档按名字找。

### 3.4 飞书权限

同 Python spec §3.7。`docs/feishu-api-diff.md` 是 T16 拿官方文档核对过的八条结论，R1 照抄其结论（含三处真机会炸的修法），Go SDK 与 Python SDK 行为不同处在回执里写明。

---

## 4. 验收标准（DoD）

以下命令全部通过 = 本轮完成。每条执行者自己能跑；真实飞书上的人工验收 M1–M6 只在 RΩ 之后由总管跑（剧本 `docs/acceptance-M.md`，命令改成本文的）。`PATH` 要有 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。

### 4.1 能跑（`scripts/check.sh` 一把过，回执贴它的输出）

| # | 命令 | 期望 |
|---|---|---|
| A1 | `cd core && cargo build --workspace` | 退出码 0 |
| A2 | `cd edge && go build ./...` | 退出码 0 |
| A3 | `core/target/debug/aite contracts lock --check` | `OK 36 files`，退出码 0 |
| A4 | `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --check` / `go vet ./...` / `gofmt -l .` 为空 | 全部退出码 0 |
| A5 | `cd core && cargo test --workspace --no-run` | 退出码 0（全部测试可编译，对应旧 A5「全仓可收集」） |

### 4.2 行为正确（全部用替身，不碰真实飞书；Docker 的两条单独标）

| # | 轨 | 命令 | 期望 |
|---|---|---|---|
| B1 | R1 | `cd edge && go test ./internal/feishu/... -count=1 -v` | 全绿；`testdata/feishu/` 至少 7 对 fixture（同 Python 的 7 对），每个 `x.json` → `x.expected.json` 用 protojson（`UseProtoNames:true, EmitUnpopulated:false`）逐字节一致；`httptest` 断言 `update_card` 走 PATCH `/open-apis/im/v1/messages/{id}` 且 POST 发送两条路由零命中；退避序列 `[1,2,4,8,16,30,30]`；令牌桶、错误映射、401 换 token 一次、read_history 正序不过滤 |
| B2 | R4 | `cd core && cargo test -p aite-control` | 全绿；R1–R8 每条至少一个用例；`!status/!stop/!restart/!new`；派发 / reaper / steer 三态 / 孤儿不吃 steer / seq 并发 / ingress 失败面 / steer 证据 |
| B3 | R5 | `cd core && cargo test -p aite-worker -p aite-models` | 全绿；旧 B3：脚本化模型 `checklist_add`×3 → 逐项 `checklist_check` → `final` ⇒ `send_card`×1、`update_card`≥3、`send_text`×1、无第二条 `send_card`；limits / fallbacks（T20）/ steer / final 产物 / 上下文构造 / 提示词字面量 / OpenAI 兼容客户端 22 条 |
| B4 | R2 | `cd edge && go test -tags docker ./internal/sandbox/... -count=1 -v` | 全绿：`run_python` 跑 matplotlib 生成 `/work/out.png` → `GetFile` 前 8 字节为 PNG 魔数；`ReapIdle(1)` 后 `docker ps -a --filter label=aite.task` 为空；超时在容器内强制；`files_out` 只报本次；release 幂等；捡孤儿 |
| B5 | R6 | `cd core && cargo test -p aite-gateway -p aite-edge-client` | 全绿：未知工具 → `not_found`；参数不合 schema → `invalid_args`；超时 → `timeout`；token 不匹配 → `denied`（含非 ASCII 伪 token）；执行顺序三条；五个工具的 content 排版与 Python 逐字；edge-client 对着 Rust 假 tonic edge：错误映射、1s deadline、断线重连 |
| B6 | R3 | `cd core && cargo test -p aite-store` | 全绿：建会话 + 两轮 turn → 同一 SQLite 文件新建第二个 store → 同线程追问命中同一 `session_id`、`list_turns` 含此前两轮；T18 三条并发约定（同实例 + 跨实例）；`recover_orphan_tasks`；`journal_mode == delete` |
| B7 | R3 | `cd core && cargo test -p aite-evidence` | 全绿：§3.1 向量逐字节；篡改任一 payload → `verify` false；>64KB 外置；manifest 8 键；T21 残行 15 条 |
| B8 | R7→RΩ | `core/target/debug/aite evals run evals/p0 --platform fake --model scripted` | RΩ：最后一行 `passed 10/10`，退出码 0。R7 阶段：10 个场景都跑完并报人话原因（不是 panic），`--list` 列出 10 个名字，`DemoPlane` 下 01/02/09/10 绿 |

### 4.3 没弄坏已有

| # | 命令 | 期望 |
|---|---|---|
| C1 | `cd core && cargo test -p aite-contracts` | 通过数不少于 R0 基线（派单里写着 N0），任何轨不得减少 |
| C2 | `core/target/debug/aite contracts lock --check` | 始终 `OK 36 files`。任何轨让它变红 = 该轨失败 |

### 4.4 人工验收 M（真实飞书测试群，RΩ 之后由总管跑）

同 Python spec §2.4 的 M1–M6，进程换成 `aite-edge` + `aite run` 两个；M6 的重启要分别验「只重启 edge」「只重启 core」「两个都重启」三种。

---

## 5. 文件归属表

每个文件只有一个 owner。不在自己白名单里的文件一律只读。

| 路径 | owner | 其他轨 |
|---|---|---|
| `docs/dev-spec-*.md`、`proto/**`、`core/crates/contracts/**`、`.contracts.lock` | R0/总管 | 只读，改动即任务失败 |
| `core/crates/proto/**`、`core/Cargo.toml`、`edge/go.mod`、`edge/go.sum`、`edge/gen/**`、`edge/internal/{aiteerr,config,server,pin}/**`、`.claude/**`、`Makefile`、`scripts/check.sh`、`.github/**`、`config/aite.example.yaml` | R0 | 只读；要加依赖/接口 → 停下报告 |
| `core/Cargo.lock` | R0 | 各轨不要提交它的改动（`cargo build` 不该改它；改了说明加了依赖） |
| `edge/internal/feishu/**`、`edge/testdata/feishu/**` | R1 | 不可见 |
| `edge/internal/sandbox/**`、`edge/internal/ingress/**`、`edge/cmd/aite-edge/**`、`docker/sandbox/**` | R2 | 不可见 |
| `core/crates/store/**`、`core/crates/evidence/**` | R3 | 不可见 |
| `core/crates/control/**` | R4 | 不可见 |
| `core/crates/worker/**`、`core/crates/models/**` | R5 | 不可见 |
| `core/crates/gateway/**`、`core/crates/edge-client/**` | R6 | 不可见 |
| `core/crates/testing/**`、`core/crates/evals/**`、`evals/README.md`、`evals/live-report-*.md` | R7 | 不可见（`evals/p0/*.yaml` 谁都不动） |
| `core/crates/app/**`、`docker-compose.yml`、`README.md`、`docs/`（非 spec）、删除 Python 树、`core/Cargo.lock` 收敛 | RΩ | — |
| `aite/**`、`tests/**`、`pyproject.toml`、`scripts/*.py`（Python 树） | 只读参考 | 谁都不动，RΩ 删除 |

各轨自己的 crate / 包下面随便建子模块和 `tests/`；`review/paste-R*.md` 是派单，回执写在回复里。

---

## 6. 任务表

| 任务 | 目标 | 可写路径 | 时序 |
|---|---|---|---|
| R0 | proto + Rust 契约 crate + aite-proto 互转 + Go 模块骨架（config / server / aiteerr / pin / 占位包 / 守护进程骨架）+ `aite` 二进制（`contracts lock`）+ 守卫 + Makefile/check.sh/CI + 本文 + 移植清单 | §5 里 owner=R0 的行 | **已完成**（基线 sha 见派单） |
| R1 | Go 飞书 adapter：归一化（7 对 fixture）、REST 出站（卡片 / 文本 / 文件 / 表情）、群历史与文档读取、长连接 + 退避重连、令牌桶、错误映射 | owner=R1 | 并行 |
| R2 | Go Docker 沙箱（acquire/exec/put/get/list/touch/release/reap，B4）+ ingress 客户端退避 + `aite-edge` 守护进程（信号、health、EdgeStatus、`platform: fake` 模式） | owner=R2 | 并行 |
| R3 | Rust SqliteSessionStore（表结构同 Python、并发三约定、孤儿恢复）+ FileEvidenceWriter（hash 链、外置 payload、manifest、verify、T21 残行自愈）+ `aite evidence show` | owner=R3 | 并行 |
| R4 | Rust ControlPlane（R1–R8、命令、串行派发、steer 三态、reaper、cancel 双分支、`_owned` 孤儿判定、seq 锁、steer 证据、dropped 计数）+ Ingress 计数 | owner=R4 | 并行 |
| R5 | Rust AgentWorker（W1–W9、失败面全部、T19 提示词、T20 重复检测、卡片合并、产物流转、沙箱归属）+ OpenAI 兼容 ModelPort | owner=R5 | 并行 |
| R6 | Rust P0ToolGateway（token / schema / 超时 / 截断 / 沙箱归属）+ 五个工具（content 排版逐字）+ edge-client（tonic 客户端实现两个 Port、IngressServer、重连） | owner=R6 | 并行 |
| R7 | Rust 官方替身（语义同 Python `aite/testing`）+ 评测 runner（场景 YAML 原样可读、`after` 三档、settle、checks 全集、`--only/--list/--json/--sandbox/--model live/--protocol-report/--timeout-scale`）+ protocol probe + real_stack 接线 + `aite evals demo-fixture` | owner=R7 | 并行 |
| RΩ | `aite run` 组装（build_app / run_app / 信号 / 收尾序列 / 孤儿收场 / contract_version 校验）+ `aite preflight`（七组自检）+ compose 双服务 + CI 硬门禁 + B8 `passed 10/10` + 删除 Python 树 + 重锁 + 全量验收 + M1–M6 | 全仓 | 最后 |

为什么这么切：按 crate / 包纵切，七轨可写集合两两不相交；它们只通过 §3 的契约对话；每轨的验收都能用私有替身跑完；R6 把「调 edge」与「被 edge 调（Ingress）」放一起是因为两头共享 tonic 连接管理；R7 的评测 harness 在 R0 骨架上就能用 `DemoPlane` 自证，RΩ 只需换成真 plane。

### 6.1 每个任务的验收（执行者只对自己这一格负责）

| 任务 | 必须通过 | 独有的额外验收 |
|---|---|---|
| R1 | A1–A5, C1, C2, B1 | 移植 `tests/adapters/**` 全部 100 个 test 定义（Go 里同名 `Test…`，参数化拆成子测试）；`docs/feishu-api-diff.md` 八条结论逐条对上；Go SDK 长连接是否投递 `card.action.trigger` 帧要实测（T16 发现 Python SDK 会丢，Go 侧结论写回执）；`event.token` 不进 `raw`（Python 版遗留的审计卫生缺口，这次修掉并写明） |
| R2 | A1–A5, C1, C2, B4 | 移植 `tests/sandbox/**` 23 条 + `tests/integration` 里与进程收尾相关的语义（信号、GracefulStop）；`docker build -t aite-sandbox:p0 docker/sandbox` 成功；镜像四条硬要求；`aite-edge --config config/aite.example.yaml` 在 `platform: fake` 下能起、`grpc_health_probe`/`GetStatus` 可达、SIGTERM 退出码 0 |
| R3 | A1–A5, C1, C2, B6, B7 | 移植 `tests/control/test_persistence.py`、`test_store_concurrency.py`、`tests/evidence/**`（28）、`tests/tools/test_evidence_show.py`（32）；`aite evidence show` 能读 Python 版写出的旧证据目录（用 `.venv` 跑一遍旧评测产出一份来验） |
| R4 | A1–A5, C1, C2, B2 | 移植 `tests/control/**` 里除 store 两份外的全部（routing 12 / commands 10 / dispatch 5 / ingress 5 / steer_routing 6 / steer_evidence 8 / ingress_failures 9） |
| R5 | A1–A5, C1, C2, B3 | 移植 `tests/worker/**` 67 条 + `tests/e2e/test_t4_model_openai_compat.py` 22 条；`prompts/platform.md` 逐字节等于 Python 版（`cmp`） |
| R6 | A1–A5, C1, C2, B5 | 移植 `tests/gateway/**` 76 条；edge-client 自带假 tonic edge 服务做 1s deadline / 重连 / 错误映射测试；`schema::validate_arguments` 的"拒未知参数、不做类型转换、bool 不是 integer"三条策略 |
| R7 | A1–A5, C1, C2 | 移植 `tests/e2e/test_t4_fake_*.py`（76）、`test_t4_evals_runner.py`（22）、`test_t4_evals_scenarios.py`（19）、`test_t4_dispatch_timing.py` + `test_t5_event_timing.py`（17）、`test_t12_protocol_probe.py` 里不依赖真 plane 的部分、`tests/tools/test_demo_fixture.py`（20）；`aite evals run evals/p0 --list` 列 10 个名；scripted 路径 stderr 为空；stdout = 一份 JSON + 最后一行 `passed k/10` |
| RΩ | 全部 A/B/C | B8 `passed 10/10`；`tests/integration/**` 53 条移植为 `core/crates/app/tests/`；`aite evals run … --sandbox docker` 04 场景 delivered；`--model live` 跑通（`docs/evals/live-report-*.md` 补一份 Rust 版）；`docker compose config` 通过；删除 Python 树后 `scripts/check.sh` 全绿、锁重生成；然后总管跑 M1–M6 |

---

## 7. 移植纪律

1. **Python 源码是规格**。每个 Rust/Go 文件头写明对应的 Python 文件；每个 Python 测试函数在新语言里有一个同名（或明显同义）的测试。回执里贴「Python N 条 → 新语言 M 条」的对照表，少的要说明为什么（例如语言层面不可能的用例）。
2. **三份清单是索引不是规格**：`review/inventory-*.md` 帮你定位与提醒非显然行为；拿不准时开 Python 源码。
3. **替身私有，重复远比冲突便宜**。
4. **依赖冻结**：`core/Cargo.toml` 的 `[workspace.dependencies]` 与 `edge/internal/pin/pin.go` 已经预钉了各轨要用的库；`cargo add` / `go get` / `go mod` 守卫直接拦。
5. **格式与 lint 是硬门禁**：`cargo clippy -D warnings`、`cargo fmt --check`（只能用 `--check`；整树 `cargo fmt` 会碰契约 crate，守卫拦；格式化自己的文件用 `rustfmt <file>`）、`go vet`、`gofmt`。
6. **异步与并发**：Rust 用 tokio；trait 对象 `Arc<dyn …>`；阻塞 IO（rusqlite、evidence 文件写）放 `spawn_blocking` 或专用线程；不在 async 里 `std::thread::sleep`。测试里时钟与 sleep 可注入（Python 版全部这么做，否则 500ms 合并与 60s reaper 测不动）。
7. **不 panic**：库代码里不许 `unwrap()/expect()` 落在运行期可失败的路径上；worker 的一切失败收敛成 `failed`；gateway 永远返回 `ToolResult`。
8. **日志事件名、计数器名、回帖文案、证据 payload 键**逐字沿用（§3.3 三条硬约束）。
9. **每条结论挂实测**。回执里贴命令的实际输出，不写"通过了"。
10. **不 commit 到 main，不 merge，不 push**。做完提交在自己的 `task-rN` 分支上，回执贴出来。

## 7.1 卡住了怎么办

按顺序：
1. 需要的接口在契约里找不到 → **停，报告**。不要自己发明一个（临时需要的内部类型放自己 crate 里，不放契约）。
2. 需要改的文件不在自己的白名单里 → **停，报告**。不要"就改一行"。
3. 需要新的第三方依赖 → **停，报告**。
4. 验收命令本身跑不起来（环境问题，如本机没 Docker、cargo 不在 PATH）→ 先自己排查环境，排查不动 → 报告。
5. 验收过不了但代码逻辑自认为对 → **以验收为准**，继续改；确信是验收标准写错了 → 停，报告。
6. Python 版行为与 Python spec 冲突 → 以 Python **代码**为准（它是被 1161 条测试钉住的现实），回执里写明。
7. 飞书 Go SDK 与 Python SDK 行为不一致 → 以官方文档为准，回执里写明；只要契约不变就不用停。

报告 = 在回执里写清楚卡在哪、试过什么、需要什么决定，然后结束这一轮，不要继续硬做。

---

## 8. 派单形式

- 每轨一个 worktree：`.worktrees/task-r<N>`，分支 `task-r<N>`，基线钉 R0 合入后 main HEAD 的具体 sha。
- 派单：`review/paste-R<N>.md`，含开场自检期望值（实测得出）、可写路径、要做什么、验收命令、回执格式。
- 启动：`claude-fleet --repo ~/Documents/Aite r1..r7`（7 轨网格 4 列 × 2 行；嫌窄就分两批 `r1..r4` 与 `r5..r7`）。
- 开场自检第一条：Read `.claude/hooks/guard_bash.py` 必须被拦 —— 被拦才说明 hook 挂上了。
