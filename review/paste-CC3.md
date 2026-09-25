# 派单 CC3：执行面拆分 + 助手回复入 transcript + 署名 / 重试分类 / 目录接缝 / 脱敏（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC3.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC3）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

对齐 Claude Tag 的 CT02 / CT04 / CT07 / CT29 / CT30。现状（行号基于 `98e4460`）：

- **助手回复从不进 transcript（CT02）**：生产代码里 `TurnRole::Assistant` 零写入方——
  `control/src/plane.rs` 只在 `:613`（SystemNote）、`:956`、`:1055`（User）写 turn；
  `worker/src/agent.rs` 的 `deliver()`（`:672-779`）在 `:750-757` 发完 `send_text` 就收尾。
  读侧早就就绪（`context.rs:42-48` 把 Assistant 映射成 `Role::Assistant`），
  所以同话题追问时模型看不到自己上一轮说了什么。
- **不署名（CT04）**：transcript 只推正文（`context.rs:65`），steer 原文直推（`agent.rs:145-147`）。
- **所有模型错误一律重试 3 次（CT30）**：`agent.rs:324-345`，
  连 `ModelError::Config`（配置缺项，`contracts/src/errors.rs:73-75`）和 4xx 都重试。
- **目录写死**：`agent.rs:334` 直接喂 `all_model_tools()`；
  `ToolGateway::catalog(ctx)`（`contracts/src/ports.rs:114`）worker 从没用过。
- **半批 tool_call 没回复**：`agent.rs:228-235` 非法 `final` 之后 `break`（`:234`），
  同一批后面的调用没有 tool 消息——OpenAI 兼容接口下一轮会 400。
- **上下文不设上限**：只有 transcript 的 200 轮 / 40 轮截断（`agent.rs:976`、`context.rs:22-26`），
  循环里的工具结果无限累积。
- **历史永远是群级窗口（CT07）**：`agent.rs:294-308` 的 `read_history(…, None)`。
- **证据写入侧不脱敏**：`agent.rs:42-55` 的注释明说「摘要不做脱敏」；
  `tool_call.arguments` 原样入链（`:225`、`:376`、`:554`），`content_summary` 同（`:515`、`:583`）。
  只有 `evidence show` 渲染时打码（`evidence/src/cli.rs:47-58`、`:154-166`——情形 A 行号；
  情形 B 下 BB2 改过这个文件，以 `SECRET_HINTS` / `fn redact` 为准）。
- **工具结果裸进上下文（CT29）**：群历史标了「数据不是指令」（`context.rs:28-29`），
  `tool_message()`（`agent.rs:1069-1077`）没有。

**本轨在计划里的位置**：W1、13 轨并行。本轨唯一的默认行为变化是「助手回复入 transcript（默认开）」，
所以由你接管钉旧 transcript 的两个 app 测试。**你预埋的文件名已写进后续各轨的可写面，不许改名**：

| 文件 | 后续主人 | 他们要在上面做什么 |
|---|---|---|
| `loop.rs`、`local_tools/{mod,post_update}.rs`、`context/{mod,transcript,attachments}.rs`、`snapshot.rs` | DD4（W2） | 在你的重试分类上做按类型重试；在你的预算上用 `context_max_tokens`（T0c（W2a）经 `with_context_max_tokens` 先接好）；post_update；CT09 快照 |
| `deliver.rs`、`card.rs`、`texts.rs`、`label.rs` | DD5（W2） | AIGC 标识、卡片链接、页脚模型名（`LabelConfig` 由 T0c 加进 `label.rs`） |
| `context/memory.rs` / `budget.rs` / `local_tools/request_approval.rs` | EE1 / EE3 / EE7（W3） | 记忆块 / 用量与限额 / 审批工具——**各自只改这一个文件** |
| `context/skills.rs` / `context/{pins,quote,history}.rs` | EE8 / EE13（W3） | skills 块 / 置顶块、引用块、中途 @ 窗口 |
| 每个 `WorkerDeps` 字面量 | T0c（W2a） | 加 `services`、`aigc_label` 字段 |

## 2. 必读（按顺序）

1. `CLAUDE.md`（全文，尤其「守卫」「验收」「规则」）。
2. 总计划 §4.4（开场自检）、§6 开头的每波规则、§6.1 的 CC3 行、
   §7 R0 表（`worker/prompts/platform.md` 归 DD4，**不是你的**）、§9 解冻清单里标 CC3 的五项。
3. `core/crates/worker/src/agent.rs` 全文（1107 行），`context.rs`、`card.rs`、`lib.rs`、`texts.rs` 全文。
4. 契约（**用 Read 工具读，别在 Bash 里 cat / grep 这个目录，守卫会拦**）：
   `core/crates/contracts/src/ports.rs:111-125`（ToolGateway）、`:141-146`（append_turn / next_turn_seq）、
   `:181-197`（RunHooks）；`session.rs:52-68`（TurnRole / Turn）；`errors.rs:56-80`；`protocol.rs:116-148`。
5. 测试替身 `core/crates/worker/tests/common/mod.rs`：ScriptedModel `:264-390`
   （失败只会报 `Upstream("model 5xx")`，`:382`）、FakeGateway `:517-636`（catalog 写死 `gateway_tools()`，`:570-572`）、
   FakePlatform.read_history 忽略 thread_id（`:219-228`）、Harness `:1122-1335`。
6. 钉住你要改的行为的测试：`test_context.rs`（`:86`、`:124-144`）、`test_steer.rs`、
   `test_limits.rs:45-79`、`test_final.rs:183-315`、`test_loop_fallbacks.rs`。
7. 你接管的 app 测试：`core/crates/app/tests/reconnect_replay.rs:726-869`、`sqlite_cross_process.rs:19-124`。
   对照读（只读）：`plane.rs:950-999`（steer 入队）、`:1312-1344`（append_turn 与 `turn_seq_lock`）、
   `models/src/lib.rs:438-443`（今天的错误文本）。

## 3. 工作区

- 分支：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，
  标题「CC3: 执行面拆分 + 助手回复入 transcript」：先用 **Write 工具**把 PR 描述写成 `/tmp/cc3-pr.md`（此时回执还不存在），再
  `gh pr create --draft --title "CC3: 执行面拆分 + 助手回复入 transcript" --body-file /tmp/cc3-pr.md`；
  收尾时 `gh pr edit --body-file <摘要文件>`（先 Write 一份摘要）。**永远别**把多行正文塞进 `--body "…"` 或 heredoc（见第 6 节守卫）。
- 代码基线：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- 提交节奏：第 1 个提交只做 ① 的零行为拆分；之后每个工作项一个提交（变异验证好撤）。
- **可写面**：`core/crates/worker/src/**`（现有 7 个文件，其余新建）、`core/crates/worker/tests/**`、
  `core/crates/app/tests/reconnect_replay.rs`、`core/crates/app/tests/sqlite_cross_process.rs`、
  `review/p1/ledger/CC3.md`（新建）。
- **同目录但不在可写面**：`core/crates/worker/Cargo.toml`（所以**不能加任何依赖**，包括 regex；脱敏手写；
  总计划 §3 D12 批的新依赖没有一项归本轨）、
  `core/crates/worker/prompts/platform.md`（`lib.rs:10-11` 逐字节钉着，DD4 的）。
- **只读面**（其余一切），特别点名：
  - P0-CLOSE 文件（总管 W1 期间本机打）= 第 4 节第 1 步列出的 14 个路径（情形 B 就按那 14 个判）：
    `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`、
    `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
    `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
    `core/crates/app/tests/evidence_on_disk.rs`、**`core/crates/app/tests/cold_start_to_delivery.rs`**、`.contracts.lock`、
    `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；另外 `.claude/**`、`core/crates/evidence/**` 整片也一律只读。
  - T0 补丁文件：`proto/aite/v1/*.proto`、`core/crates/contracts/**`、`core/crates/proto/**`、
    `edge/internal/server/server.go`、`config/aite.example.yaml`；以及永不改的 `evals/p0/*.yaml`。
  - 同波邻居：CC1 `scripts/check.sh`、`core/Cargo.toml`、`core/Cargo.lock`、`core/crates/app/Cargo.toml`（依赖面归它）；
    CC2 `core/crates/control/**`、`app/src/run.rs`、`app/tests/cc2_*.rs`；
    CC4 `core/crates/gateway/**`、`app/src/{app,wiring,lib}.rs`、`app/src/features/**`、`app/tests/build_app_contract.rs`；
    CC5 `core/crates/store/**`、`app/tests/{startup,crash}_recovery.rs`；CC6 `core/crates/models/**`；
    CC7 `core/crates/{evals,testing}/**`、`evals/p1/**`、`evals/README.md`。
- **你改不到的调用方（决定了哪些名字不能动）**：
  `app/src/app.rs:268-276` 与 `app/src/wiring.rs:82-90` 按字面量构造 7 个字段的 `WorkerDeps`；
  `app/src/run.rs:40`、`app/tests/startup_recovery.rs:28` 用 `aite_worker::card::render_card`；
  `app/src/app.rs:22`、`app/src/preflight.rs:83` 用 `aite_worker::context::load_system_prompt`。
  → **`WorkerDeps` 字段一个不加不减**（T0c 统一加；CC4 的 `worker_options` 也按它的现形状写）；
  你的新旋钮一律做成 `AgentWorker::with_*` 构造器。
- **本轨解冻的冻结项**（§9，每项配回归测试 + 变异验证）：worker 固定喂 `all_model_tools()` 与完整目录顺序钉；
  助手回复不入 transcript；所有模型错误都重试；证据里工具参数不脱敏；证据 payload 追加新键（只在确有需要时用）。

## 4. 开场自检（全部对上才开工；对不上就写回执停下）

1. **代码基线**：先 `git cat-file -e 98e4460 || git fetch -q --unshallow origin`（浅 clone 时补历史），再
   `git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'`
   - 输出为空 → 情形 A：基线行 = `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok；
   - 恰好列出下面 14 个 P0-CLOSE 路径、一个不多 → 情形 B：基线行 = `cargo passed=901 failed=0`、`contracts passed=27 failed=0`、`OK 25 files`、`passed 10/10`（以实测为准，差异逐条解释）。
     14 个路径：AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
     BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
     `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
     `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
     BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；
   - 别的 → 停下，写回执。（D0 的改动全是删除或落在排除的路径里，所以情形 A 输出为空；`--no-renames` 让「挪文件」也现形。）
2. **守卫挂上了**：用 **Read 工具**读 `.claude/hooks/guard_bash.py`，**必须被拦**
   （期望形如「blocked: 该操作触碰受保护面 …（读取位置）。停止当前工作并向人类报告。」，原文贴回执）。
   **这一步被拦就是通过；拦截原文里「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步**
   （第 6 节「被拦就停」指的是其它任何一次被拦）。
   没被拦 = hook 没生效（云端只在单仓会话加载项目 hook，失败即静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`(cd core && rustc --version)` → 1.98.1；`(cd edge && go version)` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `(cd edge && go test -race ./cmd/... -count=1)` 确认 ok；
   数包：`(cd edge && go test -race ./... -count=1 2>&1 | grep -cE '^(ok|\?)')` → `9`
   （7 行 `ok` + 2 行 `? … [no test files]`：`gen/aitepb`、`internal/pin`）；`… | grep -c '^ok'` → `7`。
   CC1 合并之后 check.sh 会多打一行 `go packages ok=N fail=M`，从那以后以这行为准。
5. **（本轨加）worker 分文件条数**：
   `(cd core && cargo test -p aite-worker 2>&1 | grep -E '^     Running|^test result' | sed -E 's/ \(target[^)]*\)//; s/; finished in.*//' > /tmp/cc3-before.txt)`
   （`sed` 去掉会变的二进制哈希与耗时，否则拆完必然「对不上」、报假警）。
   本派单里所有 `cd core && …` / `cd edge && …` 都包在 `( … )` 子 shell 里：Bash 工具的工作目录跨命令保留，
   裸 `cd` 会让下一条 `scripts/check.sh` / `cd edge` 在 `core/` 里跑、找不到路径。
   期望 11 个集成测试文件合计 79 条：card_evidence 3、cancel 2、checklist 9、context 8、final 16、in_flight 2、
   limits 7、loop_fallbacks 13、prompts_checklist 6、sandbox_handoff 6、steer 7；`src/` 里没有单元测试。
   拆分提交要逐行对上这份。

## 5. 工作项

**① 零行为拆分 + 预埋桩（单独一个提交，先于一切行为改动）**
- 把 `agent.rs` 拆空后删掉：
  - `loop.rs`：AgentWorker 结构体 / `new` / `with_clock` / `with_sleep`、`agent_loop`、`build_messages`、`chat`、`price`、
    工具分发、`tool_context`、`append_evidence`、`impl TaskWorker`、`RunContext` 与小工具（原 `:68-614`、`:889-1107`）；
  - `deliver.rs`：`deliver` / `fail` / `cancel` / `finish` / `save` / `fetch_artifact`（原 `:669-887`）；
  - `deps.rs`：`WorkerDeps`（原 `:57-66`，字段不动）；
  - `card.rs` 追加 `impl AgentWorker { ensure_card, refresh_card, close_card }`（原 `:616-667`）。
- `local_tools/`：`mod.rs`（本地工具分发 + 汇总「启用的额外本地工具」）、`checklist.rs`（原 `:380-496` 与
  `checklist_evidence`）、`final.rs`（`parse_final` / `FinalError` / `is_truthy` / `plain_string`）、
  `post_update.rs`（DD4）、`request_approval.rs`（EE7）。**每个文件一个 `pub const ENABLED: bool`**：
  checklist / final 为 `true`；两个桩为 `false`、不进目录、**名字也不进本地路由集合**：今天这两个名字不在 `is_local_tool` 里，
  `run_tool`（`agent.rs:360-364`）送 gateway、回 NotFound，被调用时照旧走这条路（`texts::unknown_tool`（`:490-494`）只接已判成本地的名字，
  本地拦下就改了工具内容与证据，破零行为）。`run_tool` 与 `:203` 的判定改成 `is_local_tool(name) || local_tools::enabled_names().contains(name)`，
  `ENABLED = true` 才本地处理；`mod.rs` 按 `ENABLED` 汇总额外本地工具的 spec 与名字——**EE7 只改 `request_approval.rs` 一个文件就能上线**。
- `context.rs` → `context/mod.rs`（`load_system_prompt`、`build_context`，并 `pub use` 子模块的全部公开名字）
  + `transcript.rs`（`transcript_messages`、三个截断常量、`role_of`）+ `history.rs`（`history_message`、`HISTORY_HEADER`）
  + `attachments.rs`（`attachments_message`、`ATTACHMENT_HEADER`）。
  四个桩块 `memory.rs`（EE1）/ `skills.rs`（EE8）/ `pins.rs`、`quote.rs`（EE13）各一个
  `pub async fn block(ctx: &BlockCtx<'_>) -> Option<Message>`，本轨恒 `None`，**在 `build_messages` 里按固定顺序真调**：
  system → skills → memory → transcript → 历史 → pins → quote → 附件（顺序写进 `context/mod.rs` 的模块文档）。
  `BlockCtx` 只放现成的（session / task / turns / history），T0c 贯通 Services 时再加字段。
- `budget.rs`（EE3）：`pub async fn before_step(..) -> Option<String>`（`Some(文案)` = 以该文案 fail，本轨恒 `None`）
  与 `pub async fn after_model_call(.., usage: &Usage)`（空实现），**在 `agent_loop` 每步开头与每次 `chat` 之后真调**，
  否则 EE3 没处可填。§3 D24 的人民币限额预设（`rmb_presets_exact`）是 EE3（W3）的，本轨**不写**任何预设或限额逻辑。
- `snapshot.rs`（DD4）、`label.rs`（DD5）：模块文档（写明主人轨与挂点）+ 至多一个空的 `pub fn`；
  **`label.rs` 不定义 `LabelConfig`**（T0c 在这个文件里加）。
- 注意：`loop`、`final` 是 Rust 关键字 → `mod r#loop;` / `mod r#final;`（文件名仍是 `loop.rs` / `final.rs`；
  编译器不认就用 `#[path = "loop.rs"]`，**文件名不许改**）。`lib.rs` 继续导出 `AgentWorker, WorkerDeps, Clock, Sleeper,
  MODEL_RETRY_DELAYS, MAX_CONSECUTIVE_INVALID_ARGS, MAX_CONSECUTIVE_REPEATS, MAX_CONSECUTIVE_SANDBOX_ERRORS, REPEAT_NUDGE_AT`，
  `card::*`、`context::*`、`fingerprint::*` 路径不变。桩函数要么 `pub`、要么挂 `#[allow(dead_code)]` 并注明主人轨——
  check.sh 跑 `clippy -D warnings`。新模块文档里的示例块一律用 ```` ```text ````（照 `context.rs:5-7`），
  别写 ```` ```rust ````——会多出 doc-test，分文件清单就对不上了；不带语言标记的 ```` ``` ```` 同样会被当成 doctest——一律写 ```` ```text ````。
- 验证：`/tmp/cc3-after.txt`（同第 4 节第 5 步的命令）与 before 逐行 `diff` 为空；check.sh 全绿。**两份清单都贴进回执。**

**② 目录接缝（全仓唯一的目录行）**
- `chat()` 的 tools = `checklist_tools()` + `final_tool()` + 启用的额外本地工具（今天为空）+ `gateway.catalog(&tool_context)`；
  `gateway` 为 `None` 时退回 `gateway_tools()`（保持今天的 10 个）。`chat` 签名随之改（内部函数）。
- 顺序与 `protocol.rs:116-121` 的 `ALL_MODEL_TOOLS` 一致，所以 `model_gets_the_full_tool_catalog`
  （`test_context.rs:124-144`）大概率仍绿——仍绿就别改它，回执里记「已解冻、断言仍成立」。
- 新测 **`catalog_comes_from_gateway`**：给 FakeGateway 加 `with_catalog(..)`，喂一份**不同于** `gateway_tools()`
  的目录，断言模型收到的是 4 + 1 + 那一份、顺序固定。

**③ 助手回复入 transcript（默认开）**
- `deliver()` 在 `send_text` 成功之后（`:750-757` 之后、`close_card` 之前）append 一条 `TurnRole::Assistant`：
  `seq = store.next_turn_seq(session)`，撞 `StoreError::DuplicateTurn` 就重取 seq 再写（至多 3 次）；
  `platform_user_id: None`、`attachments` 空；content = 发出去的正文（含「产物 X 未找到」行）+ 一行已发附件标题
  （格式函数放 `texts.rs`，由测试逐字钉住，DD4 会依赖它）。**没有已发附件时不加那一行**，content 逐字等于 `send_text` 的 `text`。
- 其它 `StoreError` 只打 `tracing::error!`（`worker.assistant_turn_failed`）、交付照常——回复已经发出去了，
  此时判失败会再发一条失败通知，破 B8 条数。`fail()` / `cancel()` 不写（回执注明）。
- 新测 **`assistant_turn_persisted_after_delivery`**（含附件标题那一行）、
  **`followup_context_contains_previous_answer`**（同会话第二个任务的首次 `chat` 里有一条 `Role::Assistant` = 上一轮回复；
  第二条 User turn 的 seq 要取 `store.next_turn_seq()`——`Harness::seed()` 用自己的计数器（`common/mod.rs:1210`），会和助手轮撞号）。
- **worker 测试必改**：`FakeSessionStore::append_turn`（`common/mod.rs:763-766`）和 `turn_texts`（`:688-696`）读写同一个 vec、
  不按角色过滤，交付后多出助手轮 → `test_steer.rs:73`（期望 `["按月画个图", STEER]`，实得末尾多 `"加上同比之后的结果。"`）、
  `:111`（末尾多 `"按季度算好了。"`）红。改法二选一：断言只取 `TurnRole::User` 的 turn，或期望值末尾加上回复文本；列入解冻清单。
- 接管 `sqlite_cross_process.rs`：`:44`（`the_only_task_from_disk`）与 `:46`（`run1.shutdown()`）之间加
  `wait_until(|| app1.worker.in_flight().is_empty(), …)`，保证第一轮助手轮落库后才关库（CPU 吃紧的云端 VM 上不能指望 shutdown 宽限期）；
  `:78` 只等 `send_text == 1` 时助手轮可能还没落库 → 读 turns 前改等 `app2.worker.in_flight().is_empty()`；`:98-102` 变成 4 轮
  `["第一问","北京今天晴，最高 28℃。","第二问","好的，按季度再画一张。"]`、seq `[0,1,2,3]`；
  `:104-111` 再加一条：model2 的上下文里要有上一轮的**回答**「北京今天晴」。
- 接管 `reconnect_replay.rs`：`:762-807` 与 `:853-863` 里助手轮的位置取决于追问成了 steer 还是新任务 →
  原有正文断言改成只看 `TurnRole::User` 的 turn；seq 改断言「从 0 连续递增」；
  另加「Assistant 轮条数 = 已交付任务数、内容含 `第 1 个活干完了。`」。`ingress.errors == 0`（`:774-778`、`:866`）不许放宽。
- **反向竞态（已由同波 CC2 ⑩ 修，H10 先合 CC2；不在本轨修）**：plane 的 `append_turn`（`plane.rs:1322-1344`）只在 plane 内部持 `turn_seq_lock`、
  撞 `DuplicateTurn` 不重试；worker 抢先写了 seq N，同一刻进来的追问就报错丢失（正是 `:1315-1319` 注释描述的 M2 病）。
  CC2 ⑩ 让 plane 的 `append_turn` 撞 `DuplicateTurn` 重读 seq 重试（至多 3 次），合并顺序定为 CC2 先于 CC3（H10）；
  但你的分支从 `98e4460` 起、**没有**那个修，所以本分支上这个竞态仍在。
  `reconnect_replay` 单跑 5 遍；**若出现由 DuplicateTurn 引起的红** → 不改断言、不放宽 `ingress.errors`、**别动 control/**，
  输出逐字贴进回执；PR 描述顶部标「依赖 CC2 ⑩（plane `append_turn` 撞 DuplicateTurn 重试）与 CC2 ③（IngressError 传播），H10 须先合 CC2」；
  按「停下报告」处理：不自行另找修法，回执写明第 7 节「reconnect_replay 5 遍全 0 failed」那条未过。

**④ 署名 `[<名字或 id>] 正文`**
- transcript：`context/transcript.rs` 渲染 User turn 时加前缀——`platform_user_id == session.created_by` 用发起人显示名
  （`run()` 的 `initiator`，即 `plane.rs:1188` 取的 `config_snapshot.initiator_name`），否则用本次 `read_history`
  里同 id 的 `sender_name`，再不行用 id；`platform_user_id` 为 `None` 不加。存库的 turn 正文**不改**（写入方是 plane）。
  `transcript_messages(turns)` 的签名与输出保持纯截断（`test_context.rs:32-58` 不动），署名作为单独一步。
- **标题别串味**：`agent.rs:117-123` 用 `last_user_text(&messages)` 起任务标题，署名后会变成「[张三] 帮我出个图」
  → 改从原始 turn 正文取。
- steer：`RunHooks.drain_steer` 只给 `Vec<String>`（`ports.rs:185`，锁定面），不带发言人。
  可行路线：plane 是**先 append_turn（`plane.rs:956`）再入队（`:984-987`）**，所以 drain 到文本后用
  `store.list_turns(session, 50)` 从新到旧找「未被认领、正文相同的 User turn」取发言人。
  **若匹配不可靠（同文多人、对不上）则转出**：steer 不署名、保留 transcript 署名，写进「契约缺口」
  （RunHooks 带发言人）与「记账转出去的」（CC2 入队时加前缀）。做了的话**只取决于 steer 署名**的
  `test_steer.rs` 逐字断言（`:63`、`:69`、`:101`、`:149`、`:208`、`:244`、`:266`）随之改，列入解冻清单；
  其中 `:69`、`:208` 是否定断言（`!any(content == STEER)`），署名后不会红、而是变空转——也要改成比对带前缀的文本，
  否则等于删了这两条防线。
- **transcript 署名必改（与 steer 做没做无关）**：Harness 的会话 `created_by` 与所有 seed / push 的 turn 都是 `"ou_user"`
  （`common/mod.rs:1161`、`:1215`、`:1259`），`run()` 传的发起人是 `"张三"`（`:1330`），所以 transcript 的 User 行都会变成
  `[张三] …`：
  - `test_context.rs:86`（`context_order_and_content`，`sent[1].content == "按月画个图"`）；
  - `test_steer.rs:156`（`step3[1].content == "按月画个图"`）、`:178`（`step1[1].content == "按月画个图"`）；
  - `test_steer.rs:179`（`step1[2].content == STEER`——队列已被清，这一份来自 transcript）、
    `:174`（`hits == 1` 数的正是 transcript 那一份，会掉到 0）。
  这 5 处列入解冻清单。
  补充测试（原卡未列名，按「每个行为改动配回归」补）：`transcript_lines_are_attributed`（含「标题不带前缀」一条断言）；
  做了 steer 的话再加 `steer_lines_are_attributed`。

**⑤ 重试分类**（放 `loop.rs`，DD4 要在上面做按类型重试）
- `ModelError::Config` → 不重试；`Upstream` 文本以 `HTTP 4xx` 打头（`429` 除外）→ 不重试（总管 2026-09-25 修订：覆盖原卡的 400/401/403，与总计划 CT30「4xx 不重试」一致；CC6 的错误文本格式对任何状态码都是 `HTTP <status>: <detail>`）；
  `HTTP 429` → 在文本里找 `retry-after-ms=<n>`，找到就按它退避（封顶 60 秒，常量写进回执），找不到用 `MODEL_RETRY_DELAYS`；
  其余（5xx、传输错、`BadResponse`）不变（`BadResponse` 原卡未涉及，保持重试）。失败文案仍是 `texts::model_unavailable`。
- **与 CC6 的格式约定**：CC6 并行把错误文本改成 `HTTP <status>: <detail>`（ASCII 冒号）+ 429 时带 `retry-after-ms=<n>`；
  今天 `models/src/lib.rs:439-443` 是全角 `HTTP {}：{}`。**`:` 与 `：` 两种都要认**；typed 变体等 T0。
  按 `ModelError` 变体里的字符串判，别按 Display（带「模型服务错误：」前缀）。
- ScriptedModel（tests/common）加按步脚本化错误的能力（`ModelError` 不是 Clone，存构造闭包或种类枚举）。
- 新测 **`config_error_not_retried`**（`call_count == 1`）、**`http_429_backs_off_with_retry_after`**
  （`retry-after-ms=1500` → 第 2 次成功，FakeClock 恰好前进 1.5 秒）；补充 `http_4xx_not_retried`（400 / 401 / 403 / 404 / 422 各 1 次调用，429 不在内）。
  `model_failure_retries_twice_then_fails`（`test_limits.rs:46-62`，`Upstream("model 5xx")`）必须仍绿。

**⑥ 每个 tool_call 都有回复**：`:234` 的 `break` 之后，给同一批里没执行的每个调用补一条 tool 消息
（`tool_call_id` 对上；固定文案进 `texts.rs`，意思是「同批的 final 参数不合法，本调用未执行」）；不执行、不写证据、不计 invalid_args。
新测 **`every_tool_call_gets_a_reply`**：一步里 `[非法 final, list_files, checklist_note]` → 下一次 `chat` 里三个 call_id
各有一条 tool 消息，gateway 零调用。

**⑦ 上下文 token 预算**（放 `context/mod.rs`；T0c（W2a）把 `WorkerConfig.context_max_tokens` 经 `with_context_max_tokens` 接进来，DD4 在其上用）
- `pub const CONTEXT_MAX_TOKENS: usize = 96_000;` + `AgentWorker::with_context_max_tokens(n)`（本轨是测试旋钮，不进 `WorkerDeps`；T0c 接配置时用的就是它）。
  估算 = 各消息正文 + tool_calls 参数序列化后的字符数（1 字符 ≈ 1 token，保守；换算写进回执）。
- 每次 `chat` 前：超了就从**最旧的 `Role::Tool` 消息**起把正文**替换**成占位文案（`texts.rs`，带原长），直到不超；
  **绝不删消息**（保住 tool_call ↔ tool 配对，与 ⑥ 同一条约束）；system / user / assistant 不动；全裁完仍超就打一行日志照跑。
- 新测 **`context_budget_trims_oldest_tool_results`**：小预算 + 三步大块工具结果 → 最后一次 `chat` 里最旧那条是占位、
  最新那条原样、每条 tool 消息的 `tool_call_id` 都在。默认 96k 下现有测试一条都不该变。

**⑧ 话题里用话题历史窗口（CT07，放 `context/history.rs`，EE13 以后改成中途 @ 窗口）**
- `session.anchor.thread_id` 有值就 `read_history(chat, window, Some(thread_id))`，否则仍 `None`。
- **后果要写进回执**：R7 给每个会话都设了 thread_id（`plane.rs:547`、`:1033`），所以实际上**每个任务**的注入历史都从群窗口
  变成话题窗口，顶层新 @ 的第一个任务只剩 root 一条（飞书侧按 root 过滤，`edge/internal/feishu/platform.go` 的 `ReadHistory`（`:480` 起，含话题过滤））。
  B8 应不受影响（05 的 `[om_h1]` 断言读的是 `read_group_history` 工具结果，不是注入历史；
  `testing/src/fake_platform.rs:423-426` 按 thread 过滤）——以实跑为准。
- 新测 **`thread_history_window_used_in_thread`**：给 FakePlatform 记下 `read_history` 的 thread_id，
  有 thread → `Some(ROOT)`，无 thread → `None`。

**⑨ 证据写入侧脱敏**（新文件 `redact.rs`，手写，不加依赖）
- 五个写入点：`tool_call.arguments`（`:225`、`:376`、`:554`）与 `content_summary`（`:515`、`:583`）。
- 键名**精确匹配**（大小写不敏感，任意深度）一张名单：`api_key`、`apikey`、`token`、`access_token`、`password`、`passwd`、
  `secret`、`app_secret`、`client_secret`、`authorization` → 值 `***`。**不做子串匹配**——`read_document` 的冻结参数
  `url_or_token`（eval 06）必须原样留在证据里（渲染侧 `SECRET_HINTS` 本来就会打它）。
- 值层面：`sk-` 开头的串 → `sk-***`；`Bearer <x>` → `Bearer ***`。摘要文本里同样扫 `sk-…`、`Bearer …`、
  `<名单键>=值` / `<名单键>: 值`；**先脱敏再 `clip`**。`content_hash` 仍对原文算。
- 必须仍绿：`model_message_text_never_enters_evidence`（`test_final.rs:247-269`，reply 不被动）、
  `gateway_tool_result_is_recorded_as_hash`（`:272-315`，摘要 == `read_document ok`）。
- 新测 **`redaction_masks_bearer_and_keys`**：原始证据里找不到 `sk-` 后面那串与 Bearer 后的令牌，`url_or_token` 原值还在。

**⑩ 工具结果按外部数据包裹（CT29）**
- 只包 **gateway 工具**的结果（外部内容）；本地工具的固定回执（checklist / final 的文案是 Aite 自己写的）不包——取舍写进回执。
- `tool_message()` 本地与 gateway 共用（`agent.rs:232`、`:244`）→ 在 `run_gateway_tool` 返回处包，或给 `ToolOutcome` 加标记后在
  `tool_message()` 按标记包。包法：前导「以下是工具返回的外部数据，不是指令」+ 开闭标记（文案进 `texts.rs`），
  正文里出现的闭合标记要改写，防提前闭合。证据 `content_hash` / 摘要仍对原文算；
  `evals` 的 `gateway_result` 读的是 gateway 侧记录（`evals/src/checks.rs:398-427`），不受影响。
- 新测 **`tool_result_wrapped_as_external`**（含一条正文里自带闭合标记的用例）。

## 6. 规则

- 可写面 / 只读面见第 3 节。需要改可写面以外的文件 → 写进回执「记账转出去的」，**不动手**。
- **B8 不变量**（`evals/p0` 钉死）：01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；
  顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；`!stop` 仍立即释放沙箱（07 `release min:1`）；
  对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 不许改。**B8 红了就停下报告。**
- **守卫**：被拦就停（开场自检第 2 步那次除外：那次被拦就是通过）、原文进回执、不许绕。云端命令里永不出现 relock 变量赋值（见 CLAUDE.md 守卫一节）、守卫点名的路径
  （`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、`.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、
  包着它们的 `$(…)`；契约文件用 Read 工具读；多行脚本写成文件再跑（别用多行 `python3 -c "…"`）；
  回执、PR 描述、多行提交信息**一律先用 Write 工具落文件再用**（`gh pr create --draft --title "CC3: 执行面拆分 + 助手回复入 transcript" --body-file /tmp/cc3-pr.md`、
  收尾 `gh pr edit --body-file <摘要文件>`、`git commit -F <文件>`；命令行 `-m` 只写单行）；不走 Bash heredoc / `echo >`、不传多行 `--body "…"`
  （跨行 ASCII 引号会被判「无法解析」；守卫也扫 heredoc 正文——回执要逐字贴的拦截原文本身就带受保护路径名）；
  `scripts/check.sh` 不接 `| tail`。
- **每个行为改动**：回归测试 + 变异验证（把那一处改回去 → 测试红 → 逐字贴输出 → 再改回来）。
  还原用编辑器或 `git stash` / `git checkout`，**别用 `cp -p` / `shutil.copy2`**（旧 mtime 让 cargo 跳过重编，跑出假绿）。
- 格式化：`rustfmt --edition 2024 <改过的文件>`（**不要 `cargo fmt --all`**）；
  check.sh 会跑全工作区的 `cargo fmt --check` 与 `clippy -D warnings`。
- 新第三方依赖、R0 文件、锁定面 → 停下报告。Docker 测试封闭（本轨用不到）；云端 protoc 生成的 `edge/gen` 永不提交。
- 已知时序抖动（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、
  `aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。
  **`reconnect_replay` 是你接管的**：红了先单跑 5 遍，稳定红就是你的。

## 7. 验收（命令 + 期望输出）

```bash
(cd core && cargo test -p aite-worker)
#   0 failed；passed = 79 + 本轨 worker 新增条数
(cd core && cargo test -p aite-worker -- catalog_comes_from_gateway assistant_turn_persisted_after_delivery followup_context_contains_previous_answer config_error_not_retried http_429_backs_off_with_retry_after every_tool_call_gets_a_reply context_budget_trims_oldest_tool_results thread_history_window_used_in_thread redaction_masks_bearer_and_keys tool_result_wrapped_as_external)
#   各文件合计 10 passed（原卡十条，逐条做过变异验证）
(cd core && cargo test -p aite --test reconnect_replay --test sqlite_cross_process --test crash_recovery)
#   0 failed；前两个已按 ③ 改，crash_recovery 一字未动（它的任务停在 Working，不产生助手轮）
(cd core && for i in 1 2 3 4 5; do cargo test -p aite --test reconnect_replay 2>&1 | grep -E '^test result'; done)
#   5 行全 0 failed
scripts/check.sh
#   末行「全部通过」，exit 0（见下表）；在仓库根跑（上面各行都在子 shell 里 cd，不会把工作目录留在 core/）
(cd edge && go test -race ./cmd/... -count=1)
#   ok（check.sh 那格被截掉的第 9 个包）
git diff --name-only origin/main...HEAD
#   每一行都落在第 3 节可写面内
```

check.sh 期望行（情形 A；情形 B 把 897 / 25 换成 901 / 27）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files`（逐字不变；变了说明碰到契约，停） |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=897+Δ failed=0`。Δ = 原卡 10 条 + 补充的 `transcript_lines_are_attributed`、`http_4xx_not_retried`（+ 做了 steer 署名时的 `steer_lines_are_attributed`）= 12 或 13；两个 app 测试文件只改断言、条数不变。**Δ 按测试名逐条列** |
| B 全量 go test | 可见 8 行 = 6 行 `ok` + 2 行 `? … [no test files]`（`gen/aitepb`、`internal/pin`；`cmd/aite-edge` 被截掉）+ 单跑 `cmd/...` ok，共 9 包 |
| B8 | `passed 10/10` |

- 解冻 / 改写的旧断言逐条列：
  - ③ 助手轮：`test_steer.rs:73`、`:111`（`turn_texts` 多了助手轮）；`sqlite_cross_process.rs:44-46`（加等待）/ `:78` / `:98-102` / `:104-111`；
    `reconnect_replay.rs:762-807` / `:853-863`；
  - ④ transcript 署名（必改）：`test_context.rs:86`；`test_steer.rs:156`、`:174`、`:178`、`:179`；
  - ④ steer 署名（做了才改）：`test_steer.rs:63`、`:69`、`:101`、`:149`、`:208`、`:244`、`:266`（`:69` / `:208` 是否定断言，写明怎么改的）；
  - ② `model_gets_the_full_tool_catalog`（仍绿则写「解冻但断言仍成立」）。
- Go 模块文件有没有被改：**不要**在命令里点它们的名字，由总管审 PR 时看（`git diff --name-only` 那一行已覆盖）。

## 8. 回执（写 `review/p1/ledger/CC3.md`，PR 描述贴摘要；两者都先 Write 成文件，PR 用 `--body-file`）

1. **开场自检原文**（4 项 + 第 5 步 worker 分文件清单）：情形 A / B；守卫拦截原文逐字；工具链三行；check.sh 完整输出。
2. **工作项逐条**：①–⑩ 各改了哪些文件:行；① 的 before / after 分文件清单与 `diff` 结果；
   预埋桩清单（文件 → 主人轨 → 挂点）。
3. **新增测试逐条**：测试名 → 钉住什么 → 变异验证（撤回哪一处、哪条红、逐字失败输出）。
4. **check.sh 完整输出**（收尾那次）+ reconnect_replay 5 遍结果。
5. **cargo passed 增量逐条**：哪个文件加了哪几条；解冻 / 改写的旧断言清单（照第 7 节那份逐行对，
   至少含 `test_steer.rs:73` / `:111`（③）、`test_context.rs:86` 与 `test_steer.rs:156` / `:174` / `:178` / `:179`（④ transcript 署名），
   每行写旧期望 → 新期望）。
6. **被守卫拦过的命令与拦截原文**（没有就写「无」；开场自检第 2 步那次除外，它记在第 1 项）。
7. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨），至少过一遍这几条：
   plane `append_turn` 撞 DuplicateTurn 不重试（已由同波 CC2 ⑩ 修，H10 先合 CC2；这里只写依赖，5 遍里真撞到了就把逐字输出附上）；steer 署名若没做（→ CC2 入队时加前缀）；
   `BadResponse` 仍重试（原卡未涉及），是否改归 DD4 的按类型重试（→ DD4，开放问题）；
   4xx 不重试的口径已按总管修订扩到全部 4xx（429 除外），与总计划 CT30 一致；顶层新 @ 首任务丢了群窗口（→ EE13）；非发起人只能署 id（→ DD9 的 UserInfo）。
8. **没做的与原因**：含 ⑩ 本地工具不包、③ fail / cancel 不写助手轮、⑤ 60 秒封顶、⑦ 字符换算这类取舍。
9. **契约缺口**（给 T0 / T0.1）：写清需要什么形状、为什么开放通道绕不过去。候选：`RunHooks.drain_steer` 只给
   `Vec<String>`、没有发言人；`Turn` 没有发言人显示名（只有 `platform_user_id`）。
   T0 已计划的（`ModelError += RateLimited/Auth/BadRequest`、`Turn += message_id`）不算缺口，别重复报。
