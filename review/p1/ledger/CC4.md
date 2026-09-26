# CC4 回执：网关插件注册表 + 策略钩子 + app 功能接缝

- 分支：`claude/intelligent-curie-nmk7nb` · PR：[TomwaltW/aite#5](https://github.com/TomwaltW/aite/pull/5)
- 基线：`a2e9377`（main = `98e4460` + D0 文档 + CC1 [#2] + CC2 [#3] + CC3 [#4]）
- 提交：`44d2ccb`（①）· `72ed78c`（②③④）· `c5d9f50`（⑤⑥⑦）

## 0. 结论一句话

①–⑦ 全部做完，⑧ 按派单只定义、只测 no-op，调用点记账转 T0c。零行为变化：原有每个测试文件条数不变、断言一字未改，
B8 `passed 10/10`。`cargo passed` 940 → 946（Δ = +6，正好是派单列的 6 条），`check.sh` 全部通过。

## 1. 开场自检

1. **代码基线**：不是派单写的情形 A / B 之一，而是「情形 A + CC1 + CC2 + CC3」（三轨已由总管合入 main，P0-CLOSE 未落地）。
   main（`a2e9377`）的树与 CC3 收尾时 `check.sh` 全绿那一版相同，基线取那一次：
   `cargo passed=940 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`go packages ok=9 fail=0`、`passed 10/10`、B9 skip。
   所以本轨的期望是 `cargo passed=946`（940 + 6），不是派单表里的 903 / 907。
2. **守卫**：Read `.claude/hooks/guard_bash.py` 被拦，原文：
   `blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。` —— 这一步被拦即通过。
3. **工具链**：`libprotoc 31.1`；`rustc 1.98.1 (48a229cea 2026-09-01)`；`go version go1.27.1 linux/amd64`。
4. **`check.sh`**（基线，= CC3 收尾那次）关键行：

```
OK 25 files
contracts passed=25 failed=0
cargo passed=940 failed=0
go packages ok=9 fail=0
passed 10/10
B9 skip：evals/p1 尚无场景
全部通过
```

5. **before 两个文件**（`/tmp/cc4-gw-before.txt`、`/tmp/cc4-app-before.txt`）：

gateway（97 条）：

```
     Running unittests src/lib.rs
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/catalog.rs
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/denied.rs
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/errors.rs
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/reaped_sandbox.rs
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/schema.rs
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/tools.rs
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

（`errors.rs` 那块 grep 顺带抓到一行测试名 `test result_echoes_call_id_and_name ... ok`，是测试名碰巧以 `result` 开头，不影响条数。）

app：

```
     Running unittests src/lib.rs
test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running unittests src/main.rs
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/build_app_contract.rs
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/cli_smoke.rs
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/cold_start_to_delivery.rs
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/counters_exit.rs
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/crash_recovery.rs
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/evidence_created_at_repro.rs
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/evidence_on_disk.rs
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/graceful_shutdown.rs
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/guard.rs
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/preflight_e2e.rs
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/reconnect_replay.rs
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/signals.rs
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/sqlite_cross_process.rs
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/startup_recovery.rs
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## 2. 工作项逐条

### ① `SandboxKey` + P0 五个工具进内建登记（`44d2ccb`，零行为变化）

- `gateway/src/sandbox_key.rs`（新）：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SandboxKey {
    /// 一个任务一个沙箱（P0 与今天唯一的变体）。
    Task(String),
}
impl SandboxKey {
    pub fn for_task(task_id: &str) -> Self;
}
```

- `gateway.rs`：`Inner.sandbox_ids` / `acquire_locks` 的键换成 `SandboxKey`；`acquire_sandbox(&self, key: &SandboxKey, task_id: &str)`、
  `ToolEnv { inner, key, task_id }`。**`SandboxPort::acquire` 的第一个参数仍逐字是 task_id**（edge 按它打 `aite.task` 标签，B8 07 看它）。
- `tools/mod.rs`：`pub(crate) struct Builtins { specs, impls, enabled }`，`Builtins::p0()` = `gateway_tools()` + `default_tools()` + 恒真谓词；
  `with_tool` 换单个实现、`with_tools` 整表换掉后目录仍列 5 个 —— 两层语义都保住（`errors.rs:419-437` 照绿）。
- 验证：`( cd core && cargo test -p aite-gateway --no-fail-fast )` 分文件条数 before / after `diff` **为空**（`/tmp/cc4-gw-after1.txt`）。

### ② `GatewayTool` + `ToolRegistry` + 按谓词过滤的目录（`72ed78c`）

- `gateway/src/registry.rs`（新），最终签名：

```rust
pub trait GatewayTool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn enabled(&self, ctx: &ToolContext) -> bool { true }
    fn call(
        &self,
        env: ToolEnv,
        ctx: ToolContext,
        args: Map<String, Value>,
    ) -> BoxFuture<'static, Result<ToolOutcome, ToolFailure>>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RegistryError(pub String);
impl From<RegistryError> for String;

#[derive(Clone, Default)]
pub struct ToolRegistry { tools: Vec<Arc<dyn GatewayTool>> }
impl ToolRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, tool: Arc<dyn GatewayTool>) -> Result<(), RegistryError>;
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;
    pub fn names(&self) -> Vec<String>;
}
```

- `register` 当场拒两种重名（人话写清撞了谁）：与 `all_model_tools()` 的 10 个名字 →「与内建工具重名」；与已登记的 →「已经有一个同名的工具登记过了」。
- `gateway.rs`：`with_registry(self, ToolRegistry) -> Self`；`fn visible(&self, ctx)` = 内建五个（原顺序、谓词）+ 登记且 `enabled(ctx)` 的（登记顺序），
  `catalog(ctx)` 与 `dispatch` 查工具**共用这一份**：谓词为假 → 目录里没有、调用回 `not_found`、「可用的是」里也没有它。没登记时目录逐字等于 `gateway_tools()`。
- 座子类型全部从 `aite_gateway` 根 `pub` 导出（`GatewayTool, RegistryError, ToolRegistry, ToolEnv, ToolFailure, ToolImpl, ToolOutcome`），外部 crate 不需新依赖。

### ③ 调用前策略钩子（`72ed78c`）

```rust
pub enum PolicyDecision { Allow, Deny(String) }
pub const DEFAULT_DENY_MESSAGE: &str = "这次工具调用被策略拒绝了";
#[async_trait]
pub trait ToolPolicy: Send + Sync {
    async fn before_call(&self, ctx: &ToolContext, req: &ToolCallRequest) -> PolicyDecision;
}
pub struct AllowAll; // 默认
// P0ToolGateway::with_policy(self, Arc<dyn ToolPolicy>) -> Self
```

位置：`校验 session_token → 查工具（含 enabled 谓词）→ before_call → 校验 arguments → 执行`。token 错 / 工具不存在的错误码不受策略影响
（`errors.rs` 三条顺序测试照绿）。`Deny(reason)` → `ok:false, code=denied`，message 用 reason（空串给 `DEFAULT_DENY_MESSAGE`）。

### ④ 四个桩文件（`72ed78c`）

`gateway/src/tools/{pages,connections,shell,access}.rs`：只有模块文档写明主人（pages→EE6、connections→FF8、shell / access→DD6），
不进 `default_tools()`、不进目录。`pub type GatewayBuilder = P0ToolGateway;`（它的 `with_*` 本来就按值链式，现有构造器签名不变）。

### ⑤ `features/` 接缝 + 18 个空壳（`c5d9f50`）

- `app/src/features/mod.rs`（新），最终签名：

```rust
pub type GatewayOption = Box<dyn FnOnce(GatewayBuilder) -> GatewayBuilder + Send>;
pub type WorkerOption = Box<dyn FnOnce(&mut WorkerDeps) + Send>;

pub struct FeatureCtx {
    pub config: AiteConfig,
    pub store: Arc<dyn SessionStore>,
    pub registry: ToolRegistry,
    pub model_override: Option<Arc<dyn ModelPort>>,
    pub gateway_options: Vec<GatewayOption>,
    pub worker_options: Vec<WorkerOption>,
}
impl FeatureCtx {
    pub fn new(config: AiteConfig, store: Arc<dyn SessionStore>) -> Self;
    pub fn build_gateway(registry: ToolRegistry, options: Vec<GatewayOption>, base: GatewayBuilder) -> GatewayBuilder;
}
pub struct StartCtx {
    pub plane: Arc<dyn ControlPlane>,
    pub platform: Arc<dyn PlatformPort>,
}
pub fn wire_all(ctx: &mut FeatureCtx) -> Result<(), String>;
pub fn start_all(ctx: &StartCtx);
pub fn wire_features(config: AiteConfig, store: Arc<dyn SessionStore>) -> Result<FeatureCtx, String>;
pub fn apply_worker_options(deps: &mut WorkerDeps, options: Vec<WorkerOption>);
```

- `wire_all` / `start_all` 的固定顺序（照卡片）：stores（DD1）→ models（DD7）→ admin（DD12）→ memory（EE1）→ routines（EE2）→ search（EE4）→ git（EE5）→
  pages（EE6 / HH2）→ budget（EE3）→ audit（EE12）→ retention（EE12）→ purge（FF6）→ connections（FF8）→ approvals（EE7）→ compliance（EE12）→
  sandbox（DD6）→ egress（EE10）→ personal（HH1）。`wire_all` 遇到第一个 `Err` 就原样返回。
- 18 个空壳各只有 `pub(crate) fn wire(_: &mut FeatureCtx) -> Result<(), String> { Ok(()) }`、`pub(crate) fn start(_: &StartCtx) {}` 与写明主人的模块文档。
- 登记出错的唯一通道写进 `mod.rs` 模块文档：`register` 的 `Err` → `wire` 的 `?` → `wire_all` → `build_app_with_features` 映射 `StartupError`（「功能接线失败：…」）/
  `plane_factory` 映射它自己的 `Err(String)`。`FeatureCtx.services` 不加（T0c）。

### ⑥ `app.rs` 两段式接线（`c5d9f50`）

```rust
pub async fn build_app(config: AiteConfig, inject: Injections) -> Result<Box<AiteApp>, StartupError>; // 签名不变 = 下面 + |_| Ok(())
pub async fn build_app_with_features(
    config: AiteConfig,
    inject: Injections,
    extra: impl FnOnce(&mut FeatureCtx) -> Result<(), String>,
) -> Result<Box<AiteApp>, StartupError>;
```

- 插入点：第 7 步（证据）之后、第 8 步（Gateway）之前；第 1–7 步一步没动。`extra` 在 `wire_all` 之后跑；两者的 `Err` 在这里一次映射成 `StartupError`。
- 应用顺序：模型覆盖 → `P0ToolGateway::new` → `with_registry` → `gateway_options` 依次 → 包 `Arc` → `WorkerDeps` → `worker_options` 依次 → `AgentWorker::new`。
- **模型覆盖的优先级**：注入的 model 优先（第 5 步记下 `model_injected`，给了就用给的，`Arc::ptr_eq` 那条照绿）；没注入时覆盖槽替掉按 config 造的那个，
  并且同一个 `model` 变量一路给到 worker、`ControlDeps.model_name`、`AiteApp.model`。
- `Injections` / `AiteApp` 没加字段；`lib.rs` 加 `pub mod features;` 并导出 `build_app_with_features`。全程纯内存，2s 上限那条照绿。

### ⑦ `wiring.rs`：评测那一路走同一个接缝（`c5d9f50`）

`plane_factory` 调同一个 `features::wire_features(deps.config, store)`，只把 `worker_options` 应用到它那份 `WorkerDeps`；模型覆盖与 gateway 槽不接（场景替身是断言面）。
`docker_sandbox_factory` 不接，在那里留了一行注释说明，记账转 T0c。B8 `passed 10/10`。

### ⑧ `start_all` 的调用点

只定义、只测 no-op，不接调用点（`run.rs` 是 CC2→T0c 的 R0），没有塞进 `build_app`。记账见 §7。

## 3. 新增测试与变异验证（6 条）

| 文件 → 测试 | 钉什么 | 变异 |
|---|---|---|
| `gateway/tests/registry.rs` → `registry_accepts_external_tool` | 外部 crate 登记的工具进目录（前 5 个逐字等于 `gateway_tools()`、登记的排末尾）、调用走外部实现、参数照 spec 校验；重名（已登记 / 内建 / 本地工具）当场拒 | `with_registry` 不存登记 |
| `gateway/tests/registry.rs` → `disabled_tool_hidden_and_call_returns_not_found` | 谓词为假：目录没有、调用 `not_found`、实现 0 次；换到谓词为真的群就在、能调通 | 目录不看谓词（`.filter(|_| true)`） |
| `gateway/tests/registry.rs` → `policy_hook_deny_returns_denied` | `Deny(reason)` → `denied` + reason 原样进 message，沙箱 0 次 exec、0 个盒子；同一网关别的工具照常 ok | 不调 `before_call` |
| `app/tests/build_app_contract.rs` → `features_wire_all_is_noop_by_default` | 默认 `wire_all` 后登记空、覆盖槽 `None`、两个槽都空；`build_app` 的目录逐字等于 `gateway_tools()`；`start_all` 对平台零调用 | `stores.rs` 的 `wire` 推一条 gateway 选项 |
| `app/tests/build_app_contract.rs` → `feature_worker_option_applied_before_build` | worker 槽在 `AgentWorker::new` 之前应用：换掉 `deps.model` 后真跑一条 @，换进去的被调 1 次、注入的 0 次 | 删掉 worker 应用循环（`option(deps)` → `drop(option)`） |
| `app/tests/build_app_contract.rs` → `feature_gateway_option_applied_before_build` | gateway 槽在包 `Arc` 之前应用：`with_tool("list_files", 计数器)` 后经 `app.gateway` 调到计数器 | 删掉 gateway 应用循环（fold 里不调 option） |

变异输出原文（`mutate.sh`：换恰好一处 → 跑测试 → 用新 mtime 写回原文件）：

```
---- 变异：gateway.rs：[            .filter(|t| t.enabled(ctx))] → [            .filter(|_| true)]
---- disabled_tool_hidden_and_call_returns_not_found stdout ----
thread 'disabled_tool_hidden_and_call_returns_not_found' (3356) panicked at crates/gateway/tests/registry.rs:130:5:
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
error: test failed, to rerun pass `-p aite-gateway --test registry`
---- 已还原
---- 变异：gateway.rs：[        self.registry = registry;] → [        let _ = registry;]
---- registry_accepts_external_tool stdout ----
thread 'registry_accepts_external_tool' (3413) panicked at crates/gateway/tests/registry.rs:102:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.10s
error: test failed, to rerun pass `-p aite-gateway --test registry`
---- 已还原
---- 变异：gateway.rs：[if let PolicyDecision::Deny(reason) = self.policy.before_call(ctx, req).await {] → [if let PolicyDecision::Deny(reason) = PolicyDecision::Allow {]
---- policy_hook_deny_returns_denied stdout ----
thread 'policy_hook_deny_returns_denied' (3466) panicked at crates/gateway/tests/common/mod.rs:727:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.11s
error: test failed, to rerun pass `-p aite-gateway --test registry`
---- 已还原
<<MUT6>>
```

## 4. `check.sh` 完整输出（终版）与验收其余几条

<<CHECK>>

## 5. cargo passed 增量：940 → 946（Δ = +6）

| 文件 | 新增测试 |
|---|---|
| `core/crates/gateway/tests/registry.rs`（新 target） | `registry_accepts_external_tool`、`disabled_tool_hidden_and_call_returns_not_found`、`policy_hook_deny_returns_denied`（+3） |
| `core/crates/app/tests/build_app_contract.rs`（只追加，原 8 条不动） | `features_wire_all_is_noop_by_default`、`feature_worker_option_applied_before_build`、`feature_gateway_option_applied_before_build`（+3） |

没有多出的条数。

## 6. 被守卫拦过的命令

只有开场第 2 步那次（见 §1.2）。其余多行脚本一律先落文件再 `python3 <文件>` 跑，未被拦。

## 7. 记账转出去的

| 事项 | 为什么不在本轨 | 建议归哪轨 |
|---|---|---|
| `run.rs` 的 `takeoff` 在 `platform.start` 之后调 `features::start_all` | `run.rs` 是 CC2→T0c 的 R0；EE7（起飞时把 AwaitingApproval 判失败）/ EE2（例程循环）/ DD12（管理台监听）依赖它 | T0c |
| `FeatureCtx.services` 字段 | `Services` 类型要 T0 才有；DD1 `features/stores.rs` 填 | T0c |
| docker 档（`wiring.rs` 的 `docker_sandbox_factory`）挂登记与 `gateway_options` | 要在同一场景里第二次跑 `wire_all`，本轨按派单不接，已在那里留注释 | T0c（wiring.rs 的下一任 R0） |

## 8. 没做的与原因 / 取舍

- ⑧ `start_all` 调用点：按派单不接（见 §7）。
- 名字：`build_app_with_features` 照派单原名；`GatewayBuilder` 用类型别名（`= P0ToolGateway`），没另立 struct。
- 两处组装共用的帮助函数是 `features::wire_features` + `features::apply_worker_options`（`FeatureCtx::build_gateway` 只有 `app.rs` 一处用；评测那路不接 gateway 槽）。
- `extra` 里推的选项与 `wire_all` 推的进同一组有序槽（排在后面），测试靠这个把替身塞进去。

## 9. 契约缺口

| 缺口 | 需要什么形状 | 为什么开放通道绕不过去 |
|---|---|---|
| `contracts/src/ports.rs:115` 的调用顺序注释没有「策略钩子」一步 | 注释补成 `token → 查工具（含 enabled 谓词）→ before_call → 校验参数 → 执行` | 锁定面，改不了注释；实际顺序由 `gateway/src/policy.rs` 模块文档与 `tests/registry.rs` 钉着 |
| `GatewayTool::enabled(ctx)` 只看得到 `ToolContext` 现有 8 个字段（`contracts/src/gateway.rs:26-39`） | T0 给 `ToolContext` 加 `initiator_id` / `chat_type` / `initiator_external` | 按发起人 / 群类型 / 外部人员过滤（DD6 access bundle）要这些字段；开放通道里只有 chat_id / thread_id 等，拼不出来 |
