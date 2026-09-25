# 派单 CC4：网关插件注册表 + 策略钩子 + app 功能接缝（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC4.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC4）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

对标 Claude Tag 的两条：**CT25**（连接 / 插件 / 自定义工具按 scope 配置）与 **CT16**（Scope + Access Bundle，目录按上下文裁剪）。
Aite 今天两条都是 absent，而且连「以后能加」的接缝都没有：

- **目录是写死的 5 个**：`core/crates/gateway/src/gateway.rs:164` `specs: gateway_tools().to_vec()`；实现表是
  `tools/mod.rs:182-190` `default_tools()` 里写死的 5 项。唯一的扩展口 `with_tool` / `with_tools`（`gateway.rs:191-201`）
  注释写着「测试用；键必须是 `gateway_tools()` 里的名字」—— 外部 crate 挂不进来。
- **目录忽略 ctx、调用前没有策略**：`catalog`（`gateway.rs:329-333`）注释原话「P0 不按 ctx 做任何裁剪（scope / access bundle 是 P1）」；
  `dispatch`（`gateway.rs:250-294`）只有 token → 查工具 → 校验参数 → 执行。
- **沙箱按 task_id 记账**：`Inner.sandbox_ids` / `acquire_locks` 的键都是 task_id（`gateway.rs:55-58`），
  `acquire_sandbox(task_id)`（`:95-125`），`ToolEnv` 攥的也是 task_id（`tools/mod.rs:97-137`、`gateway.rs:276`）。
  CT10「按话题一个沙箱」要换键，今天换键就得改所有调用点。
- **app 组装面没有接缝**：`core/crates/app/src/app.rs:255-291` 在 `build_app` 里就地 new `P0ToolGateway`、写死 `WorkerDeps` / `ControlDeps`
  字面量；评测那一路 `wiring.rs:82-90` 再写一遍 `WorkerDeps`，docker 档 `wiring.rs:236-237` 再造一次 `P0ToolGateway`。
  今天任何功能想加个工具、换个模型、翻个 worker 选项都得改这两个 R0 文件 —— W3 有 14 轨，就是 14 路冲突。

**本轨的位置**：§6.1 W1「不碰契约的地基」，规模 M。§7 R0：`app/src/{app,wiring,lib}.rs` 与 `features/mod.rs` 归 CC4（W1）→ T0c（W2a），
之后每个功能**只改自己那一个** `app/src/features/<x>.rs`。**零行为变化**：交付后目录、每次工具调用的结果、B8 一字不变。
（`all_model_tools()` 在 T0 之后仍是 10 个，新工具全部走你这个注册表 —— 总计划 §5.2「明确不做」那条。）

**谁吃你留的东西**（这些轨的派单会直接引用你的类型名与文件名，起名就照卡片）：

| 你留的 | 谁用（波次） | 怎么用 |
|---|---|---|
| `GatewayTool` + `ToolRegistry` | DD6 `run_shell` / `describe_access`（W2）；EE1 `memory_*`、EE4 `web_search`、EE6 `publish_page`（W3）；FF8 连接器 / `mcp_call`（W4） | 外部 crate 实现 trait，在 `features/<x>.rs` 登记（CC1 让骨架 crate memory / routines / search / githost 预先依赖 `aite-gateway`，所以 trait 与座子类型必须从 `aite_gateway` 根 `pub` 导出；`aite-gateway` 永远不许反过来依赖它们，否则成环） |
| `catalog(ctx)` 按谓词过滤 | CC3 `catalog_comes_from_gateway`（W1，worker 改读 `gateway.catalog(ctx)`）；CC7 探针按实际提供的工具判未知；DD6 按 bundle 过滤 | 读 |
| `policy.rs` 的 `before_call` | DD6（待审批工具先 Deny）→ EE7（改成「暂停等审批」，EE7 可写面含 `gateway/src/policy.rs`） | 换策略实现 |
| `SandboxKey` | DD6 加 `Session` 变体，经 builder 开关切换（`features/sandbox.rs` 在 `linger_sec > 0` 时推一条 `gateway_options`） | 加变体 + 开关 |
| `FeatureCtx` 模型覆盖槽 | DD7 `ModelRouter`（`features/models.rs`） | 写槽 |
| `worker_options` | EE12 `features/compliance.rs` 设 `WorkerDeps.aigc_label`（该字段由 T0c 加） | 推选项 |
| `start_all` | EE7 `features/approvals.rs` 起飞时把 AwaitingApproval 判失败；EE2 例程循环；DD12 管理台监听 | start 阶段 |
| `FeatureCtx.services` | **T0c 加字段**（`Services` 类型要 T0 才有），DD1 `features/stores.rs` 填 | 本轨不加 |

## 2. 必读（按顺序）

1. 仓库根 `CLAUDE.md`（云端唯一能读到的仓库约定；本派单与它冲突时以本派单为准）。
2. 总计划（按标题找，别信行号 —— 计划在 D0 之前还在改）：§4.4「开场自检」、§5.1「P0-CLOSE」、§6 开头「每一波都遵守的规则」+ §6.1 表里 CC4 那行、
   §7 R0 表里 `core/crates/app/src/{app,wiring,lib}.rs`、`features/mod.rs` 那行、§9「仍冻结」、§5.2「明确不做」。
3. 网关：`core/crates/gateway/src/gateway.rs` 全文（536 行；模块头 3-8 行的执行顺序是 dev-spec §3.2 原话）、`src/tools/mod.rs` 全文、`src/lib.rs`。
4. 组装：`core/crates/app/src/app.rs:56-111`（`Injections` / `AiteApp`）与 `:142-306`（`build_app` 11 步）；`wiring.rs:65-135`（`plane_factory` / `evals_wiring`）、
   `:223-241`（docker 档）；`src/lib.rs`；`src/run.rs:150-176`（**只读**，看 start 阶段本该插在哪）；`src/cli.rs:134`（真机就是 `build_app(config, Injections::default())`）。
5. 钉住行为的测试（全部只许绿、不许改断言）：`gateway/tests/catalog.rs` 全 5 条（12-16 逐字等于 `gateway_tools()`、31-41 不依赖 ctx、57-71 五个名字）；
   `gateway/tests/errors.rs:321-363`（执行顺序三条）与 `:419-437`（「目录里有、表里没有」→ not_found「没有实现」）；`denied.rs`、`reaped_sandbox.rs`、`tools.rs`；
   `app/tests/build_app_contract.rs` 全文（尤其 113-161 无副作用、206-233 的 **2s 上限**）；`app/tests/cold_start_to_delivery.rs:400-470、653-690`（只读，跑不改）；`wiring.rs:478-600` 三条单测。
6. 契约（锁定，只读）：`core/crates/contracts/src/ports.rs:112-125`（`ToolGateway`；115 行写着调用顺序）、`contracts/src/gateway.rs:8-39`（`ToolErrorCode` / `ToolContext`）、
   `contracts/src/protocol.rs:135-146`（`gateway_tools` / `all_model_tools` / `local_tool_names`）；`worker/src/agent.rs:57-66`（`WorkerDeps`）；`control/src/plane.rs:278-289`（`ControlDeps`）。

## 3. 工作区

- 分支：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC4: 网关注册表 + 策略钩子 + 功能接缝」。
  PR 描述先用 **Write 工具**写成 `/tmp/cc4-pr.md`（此时回执还不存在），再 `gh pr create --draft --title "CC4: 网关注册表 + 策略钩子 + 功能接缝" --body-file /tmp/cc4-pr.md`；
  收尾时 `gh pr edit --body-file review/p1/ledger/CC4.md`（或先 Write 一份摘要文件再 `--body-file` 它）。**永远别**把多行正文塞进 `--body "…"` 或 heredoc（见第 6 节守卫）。
- 代码基线：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**（卡片原样）：
  - `core/crates/gateway/**`（已存在；新建 `src/registry.rs`、`src/policy.rs`、`src/tools/{pages,connections,shell,access}.rs`、`tests/registry.rs` 都在这里面）。
    `gateway/Cargo.toml` 虽在面内，**一个依赖都不许加**（加了就改 `core/Cargo.lock`，那是 CC1 的）。
  - `core/crates/app/src/app.rs`、`core/crates/app/src/wiring.rs`、`core/crates/app/src/lib.rs`（已存在）。
  - `core/crates/app/src/features/**`（新建目录：`mod.rs` + 18 个空壳）。
  - `core/crates/app/tests/build_app_contract.rs`（已存在；**只追加**，原 8 条一个断言不改）。
  - `review/p1/ledger/CC4.md`（新建；目录已有 `.gitkeep`）。
- **只读面**：其余一切。点名这些：
  - P0-CLOSE 文件（总管在 W1 期间本机打；与总计划 §5.1 末条一致，14 个路径逐条见第 4 节第 1 步）：`proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`、
    contracts 的 `src/evidence.rs` / `src/lib.rs` / `tests/evidence_vectors.rs`、`core/crates/evidence/**`（P0-CLOSE 改其中的 `src/writer.rs`、`src/cli.rs`、`tests/chain.rs`）、
    `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`（**跑它、不改它**）、`.contracts.lock`、
    `.claude/**`（含 `.claude/hooks/guard_bash.py`）、`core/crates/app/tests/guard.rs`。
  - T0 补丁文件：`config/aite.example.yaml`、`edge/internal/server/server.go`、`core/crates/proto/**`、`proto/aite/v1/*.proto`；以及整个 `core/crates/contracts/**`（锁定）。
  - 离你最近的 W1 别轨：CC2 的 `core/crates/app/src/run.rs` 与 `control/**`；CC3 的 `worker/**`（含 `WorkerDeps` 定义）与 `app/tests/{reconnect_replay,sqlite_cross_process}.rs`；
    CC5 的 `store/**` 与 `app/tests/{startup_recovery,crash_recovery}.rs`；CC1 的 `core/Cargo.toml`、`core/Cargo.lock`、`core/crates/app/Cargo.toml` 与五个骨架 crate
    （admin / search / githost / routines / memory —— 你的空壳**不许引用它们**，你的基线上它们还不存在）；CC7 的 `evals/**`、`testing/**`（`FakeToolGateway`、`GatewayProbe` 都在那）；CC6 的 `models/**`。
  - 无人认领、也只读：`app/src/{cli,preflight,main,lock}.rs`、`app/tests/common/mod.rs` 及其余 app 测试。
- **本轨解冻的冻结项：无**（§9 没有给 CC4 的解冻项，卡片 `touches_r0_or_locked` 为空）。哪条现有测试非改不可才能绿 = 你改出了行为变化 → 停下写回执。

## 4. 开场自检（全部对上才开工；对不上就写回执停下）

以下命令都从仓库根起跑；带 `cd` 的一律写成子 shell `( cd core && … )`（Bash 的工作目录在两次调用之间是保留的，裸 `cd core && …` 第二次就进不去了）。

1. **代码基线**：先 `git cat-file -e 98e4460 || git fetch -q --unshallow origin`（浅 clone 时补历史），再
   `git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'`
   - 输出为空 → **情形 A**：基线行 = `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok；
   - 恰好列出下面 14 个 P0-CLOSE 路径、一个不多 → **情形 B**：基线行 = `cargo passed=901 failed=0`、`contracts passed=27 failed=0`、`OK 25 files`、`passed 10/10`（以实测为准，差异逐条解释）。
     14 个路径：AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
     BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
     `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
     `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
     BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；
   - 别的 → 停下，写回执。（D0 的改动全是删除或落在排除的路径里，所以情形 A 输出为空；`--no-renames` 让「挪文件」也现形。）
   判定结果（A 或 B）写进回执第一行。
2. **守卫挂上了**：用 **Read 工具**读 `.claude/hooks/guard_bash.py`，**必须被拦**（期望形如「blocked: 该操作触碰受保护面 …（读取位置）…」，把拦截原文逐字贴进回执）。
   拦截原文里那句「停止当前工作并向人类报告」**在这一步不适用** —— 被拦正是期望结果，贴完继续。
   **没被拦** = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`( cd core && rustc --version )` → 1.98.1；`( cd edge && go version )` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**（首次全量编译 5–10 分钟）。
   Go 那一格只显示 8 行是正常的：6 行 `ok` + 2 行 `? … [no test files]`（`gen/aitepb`、`internal/pin`），排第一的 `cmd/aite-edge` 被截掉，单跑 `( cd edge && go test -race ./cmd/... -count=1 )` 确认。
   CC1 合并之后 check.sh 会多打一行 `go packages ok=N fail=M`，从那以后以这行为准。
5. **本轨追加：逐文件条数基线**（零行为变化的判据，动手前留底）：
   `( cd core && cargo test -p aite-gateway --no-fail-fast 2>&1 | grep -E "Running|test result" | sed -E 's/ \(.*\)$//; s/; finished in .*//' > /tmp/cc4-gw-before.txt )`
   `( cd core && cargo test -p aite --no-fail-fast 2>&1 | grep -E "Running|test result" | sed -E 's/ \(.*\)$//; s/; finished in .*//' > /tmp/cc4-app-before.txt )`
   （`--no-fail-fast` 不能省：`-p aite` 里有已知抖动的 `graceful_shutdown` / `startup_recovery` / `reconnect_replay`，少了它一条抖动就截断后面所有 target，
   前后 `diff` 出假差异。`sed` 去掉产物路径与耗时，否则 `diff` 全是噪声；after 用同一条命令存成 `*-after.txt`。）
   before / after 里某个文件是 `FAILED` 行 → 按第 6 节「已知时序抖动」单跑该 target 两遍再比，别直接拿来 `diff`。
   参考（grep 数的，不是实跑，以你的实跑为准）：gateway 单测 `gateway.rs` 4、`tools/attachments.rs` 7；`tests/` 下 catalog 5、denied 6、errors 27、reaped_sandbox 4、schema 18、tools 26；
   app 的 `build_app_contract` 8、`cold_start_to_delivery` 8；app lib 单测（`unittests src/lib.rs` 那块）= `preflight.rs` 49 + `wiring.rs` 3 = 52；bin 单测（`src/main.rs`）= `lock.rs` 2。

## 5. 工作项

**① 网关内部改造：P0 五个工具进「内建登记」+ `SandboxKey`（零行为变化，单独一个提交，提交后开 draft PR）**
- `SandboxKey`（建议 `gateway/src/sandbox_key.rs` 新建，或放 `gateway.rs`）：今天只有 `Task(String)` 一个变体；`Inner` 的两张表、`acquire_sandbox`、
  `current_sandbox_id`、`forget_sandbox`、`ToolEnv` 全部改成按 key 记账。**传给 `SandboxPort::acquire` 的第一个参数必须仍然逐字是 task_id**
  （edge 按它打 `aite.task` 标签，B8 07 的 `release min:1` 和真容器那组都看它）。`ToolGateway::sandbox_id_of(task_id)` / `release_task(task_id)` 是锁定契约的签名，
  内部换成 `SandboxKey::Task(task_id)` 查。DD6 以后加 `Session` 变体 + builder 开关，**本轨不加开关**。
- 把 P0 五个工具的「规格 + 实现」组织成内建登记，但**两层语义都要保住**：`with_tool` 换单个实现、`with_tools` 整表换掉后目录仍列 5 个、调用走
  「在本次运行里没有实现」的 not_found（`errors.rs:419-437`）；「没有名为 … 的工具；可用的是 …」那句也不改。
- `P0ToolGateway::new` / `with_optional_ports` / `with_*` 的签名一个都不变（gateway 测试与 `app.rs` / `wiring.rs` 都在用）。
- 验证：`( cd core && cargo test -p aite-gateway )` 全绿；按第 4 节第 5 步再留一份 after，`diff /tmp/cc4-gw-before.txt /tmp/cc4-gw-after.txt` 应**无差异**（这一提交不加测试）；贴进回执。

**② `GatewayTool` + `ToolRegistry` + 按谓词过滤的 `catalog(ctx)`**（`gateway/src/registry.rs` 新建，`lib.rs` 导出）
- 形状（卡片：ToolSpec + impl + `enabled(ctx)` 谓词；具体签名你定，写进文档注释）：建议 `pub trait GatewayTool: Send + Sync { fn spec(&self) -> ToolSpec;
  fn enabled(&self, ctx: &ToolContext) -> bool { true } fn call(&self, env: ToolEnv, ctx: ToolContext, args: Map<String, Value>) -> BoxFuture<'static, Result<ToolOutcome, ToolFailure>>; }`
  —— 复用现有 `ToolEnv` / `ToolOutcome` / `ToolFailure`（已 `pub`），外部 crate 不需要新依赖就能实现。
- `ToolRegistry::register` 拒绝重名：与已登记的重名、与 `all_model_tools()` 里 10 个名字（含本地工具 `checklist_*` / `final`）重名一律**当场**回 `Err`（人话写清哪个名字撞了谁）。
  这个 `Err` 怎么一路传到 `StartupError`，第 ⑤ / ⑥ 项定死了（`wire` 返回 `Result`），**别**在 `wire` 里 `expect()`，也别吞掉。
  `P0ToolGateway` 加一个吃注册表的 builder 方法（如 `with_registry(ToolRegistry)`，按值链式、不返回 `Result` —— 重名在 `register` 那一刻已经拦下了）。
- `catalog(ctx)` = P0 五个（`gateway_tools()` 原顺序，始终在前）+ 已登记且 `enabled(ctx)` 为真的（按登记顺序）。**没登记任何东西时逐字等于 `gateway_tools()`**。
  P0 五个作为内建条目也走同一个 `enabled(ctx)` 谓词（默认恒真），只是排序固定在前 —— DD6 以后要按 bundle 把 `run_python` 这类内建工具也藏掉；
  `with_tools` 换表不改变目录这一条仍照 `errors.rs:419-437` 保住。
- `dispatch` 查工具那一步（`gateway.rs:257-270`）要跟 `catalog(ctx)` 用**同一个**过滤：谓词为假的工具 → `not_found`，「可用的是」列表里也不出现它。token 校验仍在最前。
- 新测试（放 `gateway/tests/registry.rs` 新建，测试文件本身就是「网关之外的 crate」）：
  - `registry_accepts_external_tool`：外部 `GatewayTool` 登记后出现在目录末尾、前 5 个逐字等于 `gateway_tools()`、调用 ok 且内容来自它；顺带断言重名登记被拒。
  - `disabled_tool_hidden_and_call_returns_not_found`：谓词为假时目录里没有它、调用回 `NotFound` 且实现一次都没被执行；**双向**：换一个谓词为真的 ctx，它就在、能调通。

**③ `gateway/src/policy.rs`（新建）：调用前策略钩子**
- `before_call(ctx, req) -> Allow | Deny(reason)`，默认 Allow（建议 `pub trait ToolPolicy` + `pub enum PolicyDecision` + 默认实现 + `with_policy(...)` builder 方法）。
- 位置：**token 校验 → 查工具（含谓词）→ `before_call` → 校验参数 → 执行**。这样 `errors.rs:321-363` 三条顺序测试不受影响；同步更新 `gateway.rs:3-8` 模块头与 `gateway/src/lib.rs:8` 的注释。
  策略实现 panic 也要落进现有的 `catch_unwind` 保护圈（`gateway.rs:342-359`），回 upstream。
- Deny → `ToolResult{ok:false, error.code = Denied}`，message 用 reason（空串时给一句默认话，`assert_failed` 要求 message / content 非空）。
- 新测试（`gateway/tests/registry.rs`）：`policy_hook_deny_returns_denied` —— 拒绝某个工具时回 `Denied`、reason 原样进 message、工具实现没被执行（沙箱 0 次 exec）；
  **双向**：同一 gateway 调另一个没被拒的工具照常 ok。

**④ 四个未登记的桩文件**：`gateway/src/tools/{pages,connections,shell,access}.rs`，在 `tools/mod.rs:20-24` 旁边声明成 `pub mod`（外部要能按路径引用），
**不进 `default_tools()`、不进目录**。每个文件只写模块文档：归谁、干什么 —— `pages.rs` → EE6（W3，`publish_page`）；`connections.rs` → FF8（W4）；
`shell.rs` → DD6（W2，`run_shell`，ExecLanguage::Bash 要 T0）；`access.rs` → DD6（W2，`describe_access`）。

**⑤ `core/crates/app/src/features/`（新建）：`mod.rs` + 18 个空壳**
- `mod.rs`：`FeatureCtx`（装配前）与 `StartCtx { plane: Arc<dyn ControlPlane>, platform: Arc<dyn PlatformPort> }`（装配后）；
  `wire_all(&mut FeatureCtx) -> Result<(), String>` 按**固定顺序**调每个文件的 `wire`，遇到第一个 `Err` 就 `?` 返回；`start_all(&StartCtx)` 同理调 `start`
  （返回 `()`；同步函数；要起循环就 `tokio::spawn`，不许阻塞）。顺序写死在 mod.rs 并在文档注释里说明（建议照卡片顺序）。
- **登记出错的唯一通道**（写进 mod.rs 文档注释，后面各轨派单照抄这个签名）：`ToolRegistry::register` 当场回 `Err` → 功能文件的 `wire` 用 `?` 往上抛 →
  `wire_all` 原样传出 → `build_app_with_features` 统一映射成 `StartupError`、`plane_factory` 映射成它自己的 `Err(String)`。不在 `wire` 里 `expect()`、不吞。
- `FeatureCtx` 字段（卡片四样，名字你定、写进文档）：配置（`AiteConfig`）、store 句柄（`Arc<dyn SessionStore>`，就是 `build_app` 第 6 步开的那个）、
  工具登记（`ToolRegistry`）、模型覆盖槽（`Option<Arc<dyn ModelPort>>`），以及两个**有序**变更槽：
  `gateway_options: Vec<Box<dyn FnOnce(GatewayBuilder) -> GatewayBuilder>>`、`worker_options: Vec<Box<dyn FnOnce(&mut WorkerDeps)>>`（卡片原样；建议加 `+ Send`，
  并各起一个 `pub type` 别名，免得 clippy `type_complexity` 在 `-D warnings` 下红）。`GatewayBuilder` 由 `aite_gateway` 导出：最省事的是 `pub type GatewayBuilder = P0ToolGateway;`
  （它的 `with_*` 本来就按值链式）；另立 struct 也行，前提是 ① 里那句「现有构造器签名不变」。`services` 字段**不加**（T0c 加）。
- 18 个空壳：`features/{stores,models,admin,memory,routines,search,git,pages,budget,audit,retention,purge,connections,approvals,compliance,sandbox,egress,personal}.rs`，
  每个只有 `pub(crate) fn wire(_: &mut FeatureCtx) -> Result<(), String> { Ok(()) }`、`pub(crate) fn start(_: &StartCtx) {}` 和一段模块文档写明**未来主人**：
  stores→DD1、models→DD7、admin→DD12、memory→EE1、routines→EE2、search→EE4、git→EE5、pages→EE6（W6 的 HH2 也写它）、budget→EE3、
  audit / retention / compliance→EE12、purge→FF6、connections→FF8、approvals→EE7、sandbox→DD6、egress→EE10（W3：出网代理与 CA 就绪后在这里把有效默认网络级别翻成 trusted；DD8 是 Go 侧，不碰这个文件）、personal→HH1。
  以后谁都不改 `mod.rs`，只改自己那一个文件。
- **wire 阶段不许做任何 I/O**（`build_app` 硬约束「只组装」由 `build_app_contract.rs:113-161` 钉着；store 此刻还没 `init()`，建表在 `run.rs:156`）。

**⑥ `app.rs` 接两段式接线；`lib.rs` 声明 `pub mod features;`**
- `build_app` 签名不变（C-TΩ-1；`cli.rs:134` 与全部 app 测试都在调它）。**`Injections` 与 `AiteApp` 不加字段**：`Injections` 有全字面量构造点在只读面上
  （`app/tests/common/mod.rs:767-772`、`:790-795`），加字段整个 app 测试编不过。
- 新增 `pub async fn build_app_with_features(config, inject, extra: impl FnOnce(&mut FeatureCtx) -> Result<(), String>)`（名字可调，写进回执）；
  `build_app` = 它 + `|_| Ok(())`。`extra` 在 `wire_all` **之后**跑（测试往同一组有序槽里追加）；两者的 `Err` 都在这里**一次**映射成 `StartupError`。
- 插入点：第 7 步（证据，`app.rs:251-253`）之后、第 8 步（Gateway，`:255-265`）之前。**第 1–7 步的先后一步都不动**（拒绝起飞时先报哪条错是行为，
  `preflight_e2e.rs:803-815`、`:952-962` 钉着 `build_app` 的拒绝）。
- 应用顺序：模型覆盖（注入的 model 仍优先 —— 「给了就用给的」，`build_app_contract.rs:50-76` 钉着 `Arc::ptr_eq`；覆盖只替掉 config 造的那个，
  并且同一个 model 一路给到 worker、`ControlDeps.model_name`、`AiteApp.model`）→ `P0ToolGateway::new(...)` → 登记 → `gateway_options` 依次 → 包 `Arc` →
  `WorkerDeps` 字面量 → `worker_options` 依次 → `AgentWorker::new`。登记重名 → `register` 的 `Err` 经 `wire` / `wire_all`（或 `extra`）传上来 → `StartupError`（人话写清哪个名字撞了谁）。
- 新测试（追加进 `build_app_contract.rs`，用 `tests/common` 现成的 `make_config` / `GatedPlatform` / `RecordingModel` / `final_step` / `RunningApp` 等）：
  - `features_wire_all_is_noop_by_default`：默认 `wire_all` 之后登记为空、覆盖槽为 `None`、两个槽都空；`build_app` 出来的 `gateway.catalog(ctx)` 逐字等于 `gateway_tools()`；
    `start_all` 对 `GatedPlatform` 零调用。
  - `feature_worker_option_applied_before_build`：经 `extra` 推一条 worker 选项，判据必须**只有在 build 之前应用才成立** —— 例：把 `deps.model` 换成第二个 `RecordingModel`，
    起飞后发一条 @，断言第二个被调、注入的那个 0 次。只断言「闭包被调过」是恒真的，不算数。
  - `feature_gateway_option_applied_before_build`：推一条 gateway 选项（例：`with_tool("list_files", 计数器实现)`），`register_task` 后经 `app.gateway` 调 `list_files`，断言走到了计数器。
- **计时**：`build_app_does_not_touch_edge_when_all_three_are_injected` 有 2s 上限（`build_app_contract.rs:199`、`:212-233`），新接线必须是纯内存操作。

**⑦ `wiring.rs`：评测那一路也走同一个接缝**
- 卡片原文是「two ordered mutation slots that wiring.rs applies before build」。仓库实情：真机组装在 `app.rs`，`wiring.rs` 是 `aite evals` 的接线。本派单的解释：
  **两处都接，调同一个帮助函数**（放 `features/mod.rs`），别各写一份。
- `plane_factory`（`wiring.rs:72-104`）：用 `deps.config` / `deps.store` 建 `FeatureCtx`、`wire_all`、把 `worker_options` 应用到 `:82-90` 那个 `WorkerDeps`。
  **模型覆盖和 gateway 槽在这里不接**：评测里模型与网关是场景给的替身，是断言面。这一路就是 B8，改完必须 `passed 10/10`。
- docker 档（`docker_sandbox_factory`，`:228-241`）：要接就得在同一场景里第二次跑 `wire_all`。**本轨不接**，在那里写一行注释说明，并记进「记账转出去的」（→ T0c，wiring.rs 的下一任 R0）。

**⑧ `start_all` 的调用点 —— 若要调用则转出**：它本该在 `run.rs` 的 `takeoff` 里、`platform.start` 之后（`run.rs:166-173`），但 `run.rs` 是 CC2（W1）→ T0c 的。
本轨**只定义、只测 no-op，不接调用点**（W1 全是空壳，不调它行为不变）；记账转 T0c，并写明 EE7 / EE2 / DD12 依赖它。**不许**为此把 `start_all` 塞进 `build_app`（违反「只组装」）。

## 6. 规则

- 只写第 3 节的可写面；需要改面外文件 → 写进回执「记账转出去的」，不动手。**`gateway/Cargo.toml` / `app/Cargo.toml` 不加依赖**。
- **每条命令都从仓库根起跑**；进子目录一律用子 shell `( cd core && … )` / `( cd edge && … )`（Bash 的工作目录在两次调用之间保留，裸 `cd core && …` 会让下一条落错目录）。
- **B8 不变量**（`evals/p0` 钉死，弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals: 1`）；
  `!stop` 仍立即释放沙箱（07 `release min: 1`）；对 bot 不加表情（09 `add_reaction equals: 0`）。`evals/p0/*.yaml` 不许改。
- **守卫**：被拦就停、原文进回执、不许绕（开场第 2 步那次除外）。云端命令里永不出现 `AITE_RELOCK=1`、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`；`check.sh` 不接 `| tail`。
  多行脚本、PR 描述、回执、多行提交信息**一律先用 Write 工具落文件再用**（`python3 <文件>` / `--body-file <文件>` / `git commit -F <文件>`；命令行 `-m` 只写单行）：
  守卫对跨行引号一律判「无法解析」，也扫 heredoc 正文 —— 回执要逐字贴守卫拦截原文（里面有受保护路径），走 heredoc / `echo >` / `--body "…"` 必被拦。
- **每个行为改动 = 回归测试 + 变异验证**：把改动撤回 → 对应测试红 → 贴红的输出 → 改回来。撤回 / 还原用 Edit 工具手改，**别用 `cp -p` / `shutil.copy2`**（旧 mtime 让 cargo 跳过重编，拿旧产物跑出假绿）。
  建议的变异：② 去掉登记 / 目录不看谓词；③ 不调 `before_call`；⑤ 让某个空壳推一条选项；⑥ 删掉两个应用循环之一。
- **格式化**：`( cd core && rustfmt --edition 2024 crates/gateway/src/<x>.rs crates/app/src/<y>.rs … )`（**不要** `cargo fmt --all`；路径相对 `core/`，
  测试文件同理，如 `crates/gateway/tests/registry.rs`）。必须在 `core/` 下跑：云端 rustup 没有默认工具链，rustfmt 只在 `core/` 下才按 `core/rust-toolchain.toml` 解析得到。rustfmt 会顺着 `mod` 声明递归进子模块，跑完 `git status --short` 确认只动了可写面。
  check.sh 的 A4b 是全工作区 `cargo fmt --check`，新文件漏格式化就红。
- **文档注释里的代码块一律标成 text 语言**（围栏写 `` ```text ``，照 `gateway.rs:5-8` 的样子）：lib crate 里不标语言的代码块会变成 doctest，改变 `cargo passed` 条数甚至编不过。
- 新第三方依赖、R0 文件（不在可写面里的）、锁定面 → 停下报告。本轨不需要 Docker；Docker 测试若跑必须封闭（只连本地测试服务器）。云端 protoc 生成的 `edge/gen` 永不提交。
- **已知时序抖动**（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、`aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、
  Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。**你的改动在 `build_app` 与网关调用路径上**，这几条要是红了，单跑两遍仍红就当成你的问题。

## 7. 验收（命令 + 期望输出）

| # | 命令 | 期望 |
|---|---|---|
| 1 | `( cd core && cargo test -p aite-gateway )` | 0 failed；`catalog.rs` 5 条全绿（`the_five_p0_tool_names_are_exactly_these`、`catalog_is_gateway_tools_verbatim` 一字未改） |
| 2 | `( cd core && cargo test -p aite-gateway --test registry )` | `3 passed; 0 failed`（`registry_accepts_external_tool`、`disabled_tool_hidden_and_call_returns_not_found`、`policy_hook_deny_returns_denied`） |
| 3 | `( cd core && cargo test -p aite --test build_app_contract --test cold_start_to_delivery )` | 0 failed；`build_app_contract` = 8 + 3 = 11，`cold_start_to_delivery` = 8（后者只读） |
| 4 | 逐条：`( cd core && cargo test -p aite --test build_app_contract <测试名> )`（三条 app 新测试各一次） | 各 `1 passed`；每条配一次变异验证的红输出 |
| 5 | 按第 4 节第 5 步重跑那两条（子 shell、带 `--no-fail-fast`），存 `/tmp/cc4-*-after.txt`，与 before `diff` | 只多出 `registry` 那个 Running 块（3 条）与 `build_app_contract` 的 +3；其余每个文件条数不变（lib 单测那块仍是 52、bin 单测仍是 2）。某文件是 `FAILED` 行 → 按第 6 节单跑该 target 两遍再比 |
| 6 | `scripts/check.sh` | 末行「全部通过」、退出码 0。行：`OK 25 files`；情形 A `contracts passed=25 failed=0`、`cargo passed=903 failed=0`（897 + 6）；情形 B `contracts passed=27 failed=0`、`cargo passed=907 failed=0`（901 + 6）；`passed 10/10`；Go 那格 8 行 = 6 行 `ok` + 2 行 `? … [no test files]`（`gen/aitepb`、`internal/pin`；`cmd/aite-edge` 被截掉，见第 7 行），没有 `FAIL`（若基线已有 CC1 的 `go packages ok=N fail=M` 行，以它为准：`ok=9 fail=0`） |
| 7 | `( cd edge && go test -race ./cmd/... -count=1 )` | `ok`（check.sh 那格看不到它） |
| 8 | `git fetch origin main && git diff --name-only origin/main...HEAD` | 每一行都落在第 3 节可写面内（Go 模块文件有没有被动由总管审 PR 时看，你不用 grep 它们） |

`cargo passed` 的 Δ 期望正好 +6，逐条列名（上面 6 个）；多出的每一条都要点名、说明为什么需要。解冻的钉：无。本轨不碰 Go；要自查格式用 `( cd edge && gofmt -l . | wc -l )` → `0`。

## 8. 回执（写 `review/p1/ledger/CC4.md`，PR 描述贴摘要；两者都先 Write 成文件，PR 用 `--body-file`）

1. **开场自检原文（4 项 + 第 5 步留底）**：情形 A/B 判定与 diff 输出；守卫拦截原文（逐字）；三条工具链版本；check.sh 完整输出；before 两个文件内容。
2. **工作项逐条**：①–⑧ 各改了哪些文件:行；① 那个提交的 sha 与 before/after 无差异的 diff 输出；`GatewayTool` / `ToolRegistry` / `ToolPolicy` / `SandboxKey` / `FeatureCtx` /
   `StartCtx` / `GatewayBuilder` / `wire_all` / `build_app_with_features` 的最终签名（原样贴）；`wire_all` 的固定顺序；模型覆盖的优先级。
3. **新增测试逐条 + 变异验证输出**：6 条，每条写它钉什么、怎么变异、红的输出原文。
4. **`scripts/check.sh` 完整输出**（不截断）+ 第 7 步单跑 `cmd/aite-edge` 的输出。
5. **`cargo passed` 增量逐条**：897（或 901）→ 实测值，按「文件 → 测试名」列。
6. **被守卫拦过的命令与拦截原文**（开场第 2 步那次也列上）。
7. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）。至少会有：
   `run.rs` 的 `takeoff` 调 `features::start_all`（run.rs 是 CC2→T0c 的 R0；EE7 / EE2 / DD12 依赖）→ T0c；`FeatureCtx.services`（`Services` 类型要 T0）→ T0c；
   docker 档（`wiring.rs:228-241`）挂登记与 `gateway_options` → T0c。
8. **没做的与原因**：卡片里做不到或与别轨面冲突的，照「若…则转出」写清楚。
9. **契约缺口**（给 T0 / T0.1；写清需要什么形状、为什么开放通道绕不过去）。已知可写的：`contracts/src/ports.rs:115` 的调用顺序注释没有「策略钩子」一步（锁定面，改不了注释）；
   `enabled(ctx)` 只看得到 `ToolContext` 现有 8 个字段（`contracts/src/gateway.rs:26-39`），按发起人 / 群类型过滤要等 T0 加 `initiator_id` / `chat_type` / `initiator_external`。
   你实际撞上的别的缺口照同样格式补。
