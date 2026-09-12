# 任务 R6 — Rust Tool Gateway + 五个工具 + edge-client（gRPC 两个 Port 与 IngressServer）

## 背景：这轨是从哪来的

Aite 决定整体用 **Rust + Go** 重写（权威文档 `docs/dev-spec-2026-09-11-rustgo.md`，先通读 §1–§3、§5–§7）。
Python 版 P0 已经跑通（T0–T24），**它是规格与参考，只读**。R0 已合入 main（`c9d96d2`）。

**你这轨是 core 里"调工具"和"调 edge"两块**：
- `aite/gateway/{tool_gateway,schema}.py` + `aite/tools/*.py`（约 750 行）→ `core/crates/gateway`，实现 R0 冻结的 `ToolGateway` trait（含 `register_task / unregister_task / sandbox_id_of / release_task`），`tests/gateway/**` 76 条搬过来；
- 新写 `core/crates/edge-client`：用 tonic 把 `PlatformPort` / `SandboxPort` 实现成到 Go edge 的 gRPC 客户端，并提供 `IngressServer`（core 侧监听、edge 调 `HandleEvent`）。这块 Python 里没有（Python 是单进程），规格在 `proto/aite/v1/edge.proto` 头注释与 spec §2.1–§2.2。

移植清单：`review/inventory-gateway-evals.md` §1（call 顺序、错误码表、schema 三条策略、截断）、§2（**五个工具的 content 排版逐字**）、§11 第 1–2、10–15 条、§10（`tests/gateway` 76 条逐条）。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-r6
分支     : task-r6
基线     : c9d96d2   ← main 的 HEAD（完整 sha c9d96d29d6d7ad835deef9fc80ec365c786fe876）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、protoc 36.1（aite-proto 的 build.rs 用）
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin`（已写进 `~/.bash_profile`）。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r6
git log --oneline -1                                   # 期望 c9d96d2 ...
git status --short                                     # 期望空
scripts/check.sh --quick                               # 期望最后一行 "全部通过"（首次 cargo 全量编译 2–5 分钟）
core/target/debug/aite contracts lock --check          # 期望 OK 36 files
cd core && cargo test -p aite-proto                    # 期望 convert.rs 5 passed（互转与 status 映射，你会用到）
cargo test -p aite-gateway -p aite-edge-client         # 期望各 0 passed（占位 crate）
```

`scripts/check.sh --quick` 关键行期望：`contracts passed=25 failed=0`、`cargo passed=35 failed=0`。

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦停下报告。

## 可写路径（白名单，之外一律只读）

```
core/crates/gateway/**          ← Cargo.toml（只能引用 workspace 已钉的库）、src/**、tests/**
core/crates/edge-client/**      ← 同上
```

**邻居（只读）**：
- `core/crates/contracts/src/ports.rs`：`ToolGateway`、`PlatformPort`、`SandboxPort`、`EventHandler`；`gateway.rs` 的 `ToolContext / ToolResult / ToolError / ToolErrorCode / MAX_TOOL_CONTENT_CHARS / DEFAULT_TOOL_TIMEOUT_SEC`；`protocol.rs` 的 `gateway_tools()`；`sandbox.rs` 的 `EXEC_TIMEOUT_EXIT_CODE`；`errors.rs` 的 `PlatformError / SandboxError{kind}` / `IngressError`；`config.rs` 的 `EdgeConfig`。
- `core/crates/proto/src/{lib,convert,status}.rs`：`pb` 模块（tonic 生成：`platform_service_client::PlatformServiceClient`、`sandbox_service_client::SandboxServiceClient`、`edge_status_service_client::EdgeStatusServiceClient`、`ingress_service_server::{IngressService, IngressServiceServer}`）；domain ↔ pb 的 `From / TryFrom`；`status::{platform_error_from_status, sandbox_error_from_status, status_from_ingress_error, is_retryable}`。**缺转换 / 缺映射 → 停下报告，不要在自己 crate 里复制一份。**
- 依赖：workspace 已钉 `tonic 0.14 / tonic-prost / prost`、`tokio / tokio-stream`、`jsonschema 0.56`、`serde_json`、`sha2/hex`、`tracing`、`async-trait`、`thiserror`。tonic 0.14 的 UDS 连法：`Endpoint::try_from("http://[::]:50051")?.connect_with_connector(tower::service_fn(|_| async { UnixStream::connect(path).await.map(hyper_util::rt::TokioIo::new) }))` 这类写法需要 `tower` / `hyper-util` —— **它们是 tonic 的传递依赖但没在 workspace 直接钉**；先看 tonic 0.14 是否 re-export 了所需类型（`tonic::transport::Endpoint::connect_with_connector` 接受 `tower::Service`；`tonic` 依赖里有 `hyper-util`）。如果确实需要把 `tower` / `hyper-util` 加进依赖表 → **停下报告**（这条很可能触发，先做 gateway，把 edge-client 的连接层放到最后，报告时附上你试过的写法）。
- Python 参考：`aite/gateway/**`、`aite/tools/**`、`tests/gateway/**`（conftest 里的私有替身保留真实现两条脾气：`read_history` 不过滤、`put_file` 走路径校验）。

## 要做什么

### ① `aite-gateway`：`P0ToolGateway`
- `P0ToolGateway::new(platform: Arc<dyn PlatformPort>, sandbox: Arc<dyn SandboxPort>, spec: SandboxSpec) -> Self`；`with_default_timeout(..)` / 时钟注入；`impl ToolGateway`。
- `call` 顺序（清单 §1）：计时 → token 校验（登记表；**常数时间比较**，逐字节 `xor` 累加，两边先 `as_bytes()`，长度不等也走完再判）→ 目录 → schema 校验（`schema::validate_arguments(&Value, &Map) -> Result<Map, SchemaViolation>`：用 `jsonschema` crate 校验 + **前置拒未知参数**（消息列出可用参数名）+ 补 `default` + 不做类型转换 + bool 不是 integer）→ 预算（`run_python` = `timeout_sec + 5s`，其余 60s）→ `tokio::time::timeout` 执行 → 成功 `_ok`（`clip` 到恰好 12000 字符，marker `"\n…[内容已截断]"`）/ 失败 `_failed`（content 也截断，error.message 不截）。错误码表逐条；工具实现里任何意外错误 → `upstream`。
- 沙箱归属：`_sandbox_ids: Mutex<HashMap<task_id, sandbox_id>>` + per-task 锁（并发两个 `run_python` 不会各建一个）；`sandbox_id_of`；`release_task` 幂等（撤 token + `sandbox.release`）；`register_task` 空 token 拒绝。
- 五个工具各一文件，**content / data 排版逐字照清单 §2**（`read_group_history` 的表头与「全被过滤」措辞、`read_document` 的「来源：」行、`download_attachment` 的 `_safe_name` 五步与 `/work/in/`、`run_python` 的段落格式与 `exit 124 → timeout`、`list_files` 不建沙箱）。`ToolFailure(code, message)` 内部类型自己定。
- 测试 76 条（清单 §10）：`catalog` 5、`denied` 6（含**非 ASCII 伪 token 是 denied 不是 upstream**）、`errors` 25（含执行顺序三条、`CancelledError` 对应「调用方取消 future 时不产生 ToolResult」）、`schema` 17、`tools` 23。

### ② `aite-edge-client`
- `EdgeClient::connect(cfg: &EdgeConfig, repo_root: &Path) -> Result<EdgeClient, PlatformError>`：懒连接（第一次 RPC 才拨）；连不上按 1→2→…→30s 退避重试拨号（无限），日志 `edge.connecting` / `edge.connected` / `edge.reconnecting`；`max_message_mb` 两个方向都设。
- `edge.platform() -> Arc<dyn PlatformPort>`（`EdgePlatform`）：每个方法 = domain → pb → RPC → pb → domain，`Status` 经 `status::platform_error_from_status`；`capabilities()` 缓存首次 `GetCapabilities`；**`start(handler)`**：起 `IngressServer`（tonic `Server::builder().add_service(IngressServiceServer::new(..)).serve_with_incoming(UnixListener)`）监听 `cfg.core_socket`（先删残留 socket、建父目录），收到 `HandleEvent` → `pb → domain`（失败 → `INVALID_ARGUMENT`）→ 调 `handler` → `Ok` / `status_from_ingress_error`；`stop()` 关服务；`started()`。
- `edge.sandbox() -> Arc<dyn SandboxPort>`（`EdgeSandbox`）：同理，`Status` 经 `sandbox_error_from_status`；`close_all` 是本地 no-op（沙箱记账在 edge）。
- `edge.status() -> Result<EdgeStatus>`（RΩ 起飞时比对 `contract_version`）。
- 测试：在 Rust 里起一个**假 edge**（实现 `PlatformService / SandboxService / EdgeStatusService` 的 tonic server，监听 tempdir 里的 UDS，可脚本化返回错误码）验：每个方法的往返；错误映射（UNAVAILABLE → retryable、NOT_FOUND 的两种前缀、INVALID_ARGUMENT）；`max_message_mb`（发 8MB 的 `send_file` 不炸）；edge 不在时 RPC 返回 retryable 错误且不 panic、edge 起来后自动恢复；`IngressServer`：用 tonic 客户端调 `HandleEvent`，handler 被调、非法事件 INVALID_ARGUMENT、handler 出错 INTERNAL、1s 内返回（handler 慢时由调用方 deadline 兜住 —— 这条在 Go 侧）。

## 纪律

1. 契约与锁：`OK 36 files` 全程不变；`proto/**`、`core/crates/proto/**` 只读。
2. 白名单之外只读；缺接口 / 缺转换 / 要依赖 → **停下报告**（edge-client 的传输层依赖问题按上面说的先报）。
3. 不 panic；`ToolGateway::call` 永远返回 `ToolResult`；`tokio::time::timeout` 而不是自旋。
4. 测试不靠真实 sleep 超过 100ms（超时测试用 `tokio::time::pause`：dev-dependencies 给 tokio 加 `test-util`，只改自己的 Cargo.toml）；不碰网络、不碰 Docker。
5. `cargo clippy -p aite-gateway -p aite-edge-client --all-targets -- -D warnings` 干净；格式化只对自己的文件：`rustfmt --edition 2024 $(git ls-files 'core/crates/gateway/**/*.rs' 'core/crates/edge-client/**/*.rs')`（整树 `cargo fmt` 守卫会拦）。
6. 每条结论挂实测。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r6/core
cargo test -p aite-gateway -p aite-edge-client 2>&1 | grep -E '^test result'    # 期望全部 ok，一条不许 failed
cargo clippy -p aite-gateway -p aite-edge-client --all-targets -- -D warnings      # 期望退出 0
cd .. && scripts/check.sh --quick                                                  # 期望 全部通过
core/target/debug/aite contracts lock --check                                      # 期望 OK 36 files
```

## 回执格式

```
## R6 回执

基线 c9d96d2 → 提交 <短 sha>

### 移植对照表
| Python 测试文件 | Rust 测试 | 条数 | 差异说明 |
（tests/gateway 76 → M 条；edge-client 新增 N 条）

### 与 Python 行为的差异（逐条；没有就写"没有"）

### edge-client 的传输层
UDS 连法 / 是否需要新依赖 / 重连策略：<>
假 edge 的形状：<>

### 改了什么
- <文件> <一句话>

### 实测输出（粘实际的）
$ cargo test -p aite-gateway -p aite-edge-client | grep 'test result'
<粘>
$ scripts/check.sh --quick
<最后 3 行>
$ core/target/debug/aite contracts lock --check
<那一行>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-r6` 分支上，回执贴出来。
