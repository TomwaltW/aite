# 派单 CC2：控制面零行为拆分 + 预埋桩 + 可选并发派发 + 健壮性 + 命令注册表（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC2.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC2）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

本轨覆盖 CT01 / CT04 / CT05 / CT13 / NEW01。今天的代码（行号基于 `98e4460`）：

- **CT04 全局串行**：`core/crates/control/src/plane.rs:1120-1125` 的 `dispatch_loop` 取一个、`await` 跑完才取下一个；`run_forever` 的注释（`:1426-1433`）写明「派发**串行**」。一个长任务卡住所有群。本轨做「同会话串行、跨会话至多 N 个」，**默认 N=1（与今天逐字节一致）**；T0c（W2a）从 `WorkerConfig.max_parallel_tasks` 翻成 4（plan §3 D13、§5.3）。
- **CT01 能力位没人用**：R6 写死 `ev.chat_type == ChatType::Group`（`plane.rs:539`）；`capabilities()` 只在 `core/crates/edge-client/src/platform.rs:47-75` 缓存，core 零消费者。
- **CT05**：`!restart`（`plane.rs:798-853`）归档后同 thread 重开，不重读话题；进程内卡死的任务不会被替换。
- **CT13**：ack 只在 `new_session(react = true)` 里加（R7 与带文本的 `!restart` / `!new`，`plane.rs:1044-1053`），R6 追问（`:538-543`）一个表情都没有。
- **NEW01**：解析只认 ASCII `!` + U+0020（`commands.rs:15-21`）；R5 只认 `'!'`（`plane.rs:534`）；未知命令文案逐字钉在 `commands.rs:6`、`control/tests/wording.rs:23-26`（规格原句 `docs/dev-spec-2026-09-09.md:742`，只被取代、不改原文）。
- **入口错误被吞**：`Ingress::handler()`（`ingress.rs:119-129`）无条件 `Ok(())`，所以 `edge-client/src/ingress.rs:149-153` 的 `Err → INTERNAL → edge 让平台重推` 永远走不到（`plane.rs:1416-1419` 注释写明了；`ingress.rs:11-12` 的模块注释与实情不符）。
- **`!new` 在话题里**：`cmd_new`（`plane.rs:855-865`）把新会话 root 在 `!new` 那条消息上，但飞书锚点取 `root_id`（`edge/internal/feishu/normalize.go:463-469`），之后话题里的回复带的仍是老 root → `thread_session`（`plane.rs:1289-1310`）命中**没归档的老会话**，新会话永远收不到回复。
- **证据**：`cancelled` 载荷只有 `by/steps`（`plane.rs:1563-1565`）；命令不留任何证据。

**本轨在计划里的位置**：W1 不碰契约的地基之一（plan §6.1）。W1 各轨可写面两两不交，`control/**` 这一波只有你一个主人。

**谁吃你的产出**：

- T0c（W2a）：把 `with_max_parallel` 接到 `WorkerConfig.max_parallel_tasks`（默认 4），接管因此变红的 app / evals 串行钉；给 `routing/card_actions.rs` 加新 `CardActionKind` 的 log+drop 分支；`EventKind::Reaction` 进 R4 只计数（排在 R5/R6 之前）。
- DD3（W2）：接管 `routing/mod.rs`、`routing/resolver.rs`、`sessions.rs`、`dispatch.rs`、`internal.rs`、`commands/{status,restart}.rs`（锚点解析、DM 会话、`submit_internal`、审批信箱）。
- W3/W4 各轨只改你预埋的那一个文件（清单见 §5 ①）。**桩的签名要一次给够**，否则后续轨得改 `routing/mod.rs`，而那时它已不归他们。

## 2. 必读（按顺序）

1. `CLAUDE.md` 全文（守卫、验收、B8、时序抖动清单、规则）。
2. 总计划 §4.4（开场自检）、§5.3（T0c 翻并发）、§6 开头「每一波都遵守的规则」、§6.1 CC2 行、§7（`run.rs`：CC2 → T0c）、§9（解冻 / 仍冻结）。
3. `review/p1/tracks-2026-09-25.json`：CC2 原卡，以及 DD3 / T0c / EE1 / EE2 / EE3 / EE7 / EE8 / EE9 / EE11 / EE12 / EE13 / FF1 / FF4 的 `goal`（决定每个桩要什么签名）。
4. `core/crates/control/src/plane.rs` 全文（1710 行，注释是规格的一部分）；`commands.rs`（69）、`queue.rs`（161）、`ingress.rs`（148）、`lib.rs`（43，`:23-35` 是对外 API）。
5. `core/crates/app/src/run.rs:1-34`（C-TΩ-1 退出序列）、`:246-301`（`shutdown`，`:266-289` 给 stranded 任务善终）。
6. 契约（只读；`core/crates/contracts/**`、`core/crates/proto/**`、`docs/dev-spec-*.md` 一律用 Read 工具读，Bash 里出现这些路径会被守卫拦）：`core/crates/contracts/src/ports.rs:43-75`（`PlatformPort`，`read_history` 在 `:63-68`）、`:181-241`（`RunHooks` / `TaskWorker` / 冻结的 `ControlPlane` trait）、`capabilities.rs:4-39`、`core/crates/proto/src/status.rs:63-69`（非 `Invalid` 一律 INTERNAL）。
7. 钉行为的测试：`control/tests/dispatch.rs`（全 9 条，尤其 `:288-321` abort 后 `join` 立即返回）、`wording.rs:18-29`、`:336-368`、`commands.rs:531-593`、`:820-826`、`routing.rs:276-396`、`ingress.rs:104-114`、`:144-158`、`ingress_failures.rs:37-130`、`:323-345`、`cancel.rs:20-52`；`control/tests/support/mod.rs`（`FakeStore::fail_next` `:333`、假平台 `capabilities` `:592-594` / `read_history` 恒空 `:666-673`、构造器 `:1200-1291`、**`:278` 那条测试会编进每个 `mod support;` 的测试二进制**）；`app/tests/evidence_on_disk.rs:240-322`（「P0 是单 worker」、链最后一条是 cancelled）、`app/tests/reconnect_replay.rs:33-36`、`:458-471`。

## 3. 工作区

- **分支**：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC2: 控制面拆分 + 并发派发（默认 1）+ 命令注册表」。
- **代码基线**：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**：`core/crates/control/src/**`、`core/crates/control/tests/**`、`core/crates/app/src/run.rs`、`core/crates/app/tests/cc2_*.rs`（新建）、`review/p1/ledger/CC2.md`（新建）。`core/crates/control/Cargo.toml` **不在**可写面 → 只能用它已声明的依赖（`futures` 在 workspace 里但 control 没声明，别加）。**`ControlDeps` 的字段（`plane.rs:278-289`）与 `InProcessControlPlane::new(deps: ControlDeps)` 签名（`:410`）一个不改**：`app/src/app.rs:279`、`app/src/wiring.rs:92` 按结构体字面量构造它（CC4 的面，加一个字段合并后就编不过），`control/tests/support/mod.rs:1269` 同样是字面量；本轨的新旋钮（并发数、卡死阈值、注入口）一律走 `with_*` builder。
- **只读面**：其余一切，特别是——
  - P0-CLOSE 的 14 个路径（总管 W1 期间本机打，= 开场自检第 1 步那张清单）：`proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`、`core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、`core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、`core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`、`.contracts.lock`、`.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`（连同整个 `core/crates/evidence/**`、`.claude/**`）；
  - T0 补丁文件：`config/aite.example.yaml`、`edge/internal/server/server.go`、`core/crates/proto/**`、`proto/**`、`core/crates/contracts/**`；以及 `.contracts.lock`、`edge/go.mod`、`edge/go.sum`、`evals/p0/*.yaml`；
  - 邻轨：CC3 `core/crates/worker/src/**`、`core/crates/worker/tests/**`、`app/tests/{reconnect_replay,sqlite_cross_process}.rs`（`worker/prompts/platform.md` 本波没人碰，W2 归 DD4）；CC4 `app/src/{app,wiring,lib}.rs`、`app/src/features/**`、`app/tests/build_app_contract.rs`（所以把 `with_max_parallel` 接进 app 不是你的活，是 T0c 的）；CC5 `core/crates/store/**`、`app/tests/{startup,crash}_recovery.rs`；CC7 `core/crates/evals/**`、`core/crates/testing/**`；`docs/**`（`docs/acceptance-M.md:932` 列着旧命令表，记账，不动）。
- **本轨解冻的冻结项**（plan §9，每项配回归测试 + 变异验证）：全局串行派发（本轨建开关、默认 1）；`UNKNOWN_COMMAND_TEXT` / `KNOWN_COMMANDS` 文案钉；证据 payload 追加新键。会因此翻转的钉：`wording.rs:23-26`、`src/commands.rs:60-68`（`known_commands_are_all_reachable_from_the_unknown_text`）、`tests/commands.rs:590-591`，以及 ⑧ 可能翻的 `tests/commands.rs:548-560`——每个翻转都在回执里逐条列「改前断言 / 改后断言 / 为什么」。
- **仍冻结**：停机顺序（`run.rs:3-13`）、`ACTIVE_TASK_STATUSES`、`evals/p0/*.yaml`、`ControlPlane` trait 签名（`ports.rs:219-241`）、**BB2 之后的证据哈希公式**（⑨ 只往 payload 里加键，公式与编码一个字不动）、`FakeToolGateway` 的输出格式、`journal_mode=delete`（plan §9）。

## 4. 开场自检（全部对上才开工；对不上就写回执停下）

1. **代码基线**：先 `git cat-file -e 98e4460 || git fetch -q --unshallow origin`（浅 clone 时补历史），再
   `git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'`
   - 输出为空 → **情形 A**：基线行 = `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok（= 7 行 `ok` + 2 行 `? … [no test files]`，见第 4 步）；
   - 恰好列出下面 14 个 P0-CLOSE 路径、一个不多 → **情形 B**：基线行 = `cargo passed=901 failed=0`、`contracts passed=27 failed=0`、`OK 25 files`、`passed 10/10`（以实测为准，差异逐条解释）。
     14 个路径：AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
     BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
     `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
     `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
     BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；
   - 别的 → 停下，写回执。（D0 的改动全是删除或落在排除的路径里，所以情形 A 输出为空；`--no-renames` 让「挪文件」也现形。）回执里写明是 A 还是 B。
2. **守卫挂上了**：用 **Read 工具**读 `.claude/hooks/guard_bash.py`，**必须被拦**（原文形如 `blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。`，把拦截原文贴进回执）。**这一步被拦就是通过；拦截原文里「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步。**没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，失败静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`(cd core && rustc --version)` → `1.98.1`；`(cd edge && go version)` → ≥ `go1.27`（Bash 工具会记住上一条的 `cd`，本派单里的 `cd core` / `cd edge` 一律写成子 shell，每条命令都从仓库根起跑）；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可）。
4. **`scripts/check.sh`**（冷编译 10–15 分钟）：末行「全部通过」，各行与第 1 步的基线行逐字一致；**别接 `| tail`**。

   | check.sh 的格 | 情形 A 期望 | 情形 B 期望 |
   |---|---|---|
   | A3/C2 契约锁 --check | `OK 25 files` | `OK 25 files` |
   | C1 契约测试 | `contracts passed=25 failed=0` | `contracts passed=27 failed=0` |
   | B 全量 cargo test | `cargo passed=897 failed=0` | `cargo passed=901 failed=0` |
   | B 全量 go test（-race） | 8 行 = 6 行 `ok` + 2 行 `? … [no test files]`；`cmd/aite-edge` 被截掉 | 8 行 = 6 行 `ok` + 2 行 `? … [no test files]`；`cmd/aite-edge` 被截掉 |
   | B8 评测 | `passed 10/10` | `passed 10/10` |
   | 末行 | `全部通过` | `全部通过` |

   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被 `run()` 的 `tail -n 8` 截掉），单跑 `(cd edge && go test -race ./cmd/... -count=1)` → `ok`；数包：`(cd edge && go test -race ./... -count=1 2>&1) | grep -cE '^(ok|\?)'` → `9`
   = 7 行 `ok` + 2 行 `? … [no test files]`（`gen/aitepb`、`internal/pin` 没有 `_test.go`；只数 `^ok` 得 7，不是 9）。
   CC1 合并前 check.sh 没有 `go packages ok=N fail=M` 那一行，别等它；CC1 合并之后 check.sh 会多打一行 `go packages ok=N fail=M`，从那以后以这行为准。
5. **（本轨专属）存一份拆分前的逐二进制条数**，① 的提交要拿它对比：
   `(cd core && cargo test -p aite-control --no-fail-fast 2>&1 | grep -E '^ +Running|^test result' | sed -E 's/-[0-9a-f]+\)$/)/; s/; finished in .*//') > /tmp/cc2-counts-B0.txt`
   （第二段 `sed` 去掉每行 `test result` 尾巴上的 `; finished in 0.xxs` —— 那个耗时每跑一次都变，不去掉的话 ① 的 `diff` 对一次完美的零行为拆分也必红。Doc-tests 那行 `test result` 前面没有 `Running` 表头，是正常的。）
   静态数出来的期望（含 `support/mod.rs:278` 在每个二进制里各算 1 条；以实跑为准）：`unittests src/lib.rs` 13；`cancel` 8；`commands` 24；`dispatch` 10；`drop_logs` 4；`ingress` 6；`ingress_failures` 10；`routing` 13；`steer_evidence` 9；`steer_routing` 6；`wording` 13；Doc-tests 0 —— 合计 116。

## 5. 工作项

每项一个（或几个）提交；每个行为改动都做变异验证。新测试放 `core/crates/control/tests/cc2_*.rs`（建议 3 个文件：`cc2_dispatch.rs`、`cc2_commands.rs`、`cc2_routing.rs`），**每个带 `mod support;` 的新文件会多算 1 条 `parking_counts_even_when_nobody_is_waiting_yet`**，Δ 里要写明。原卡点名的 9 个测试名一字不改（⑩ 的 `plane_append_turn_retries_duplicate_turn` 是交叉检查追加的，不在原卡）。

### ① 零行为变化拆分 + 预埋桩（**单独的第一个提交**，然后开 draft PR）

按下表把 `plane.rs` 搬家（建议映射，拿不准就近放、在回执里写明）：

| 去处 | 从 `plane.rs` 搬过来的 |
|---|---|
| `plane.rs`（保留） | `ControlDeps` `:277-289`、`PlaneState` `:291-308`、`Shared`（锁序注释 `:315-319` 原样）`:310-345`、结构体 `:347-363`、`new` / `with_*` / `state` `:409-499`、`ControlPlane` impl `:1395-1603` |
| `routing/mod.rs` | `route` R1–R8 `:501-557`、`R4_KINDS` `:147-153`、`drop_log` `:155-214`、R3 `:559-603`、R4 `:605-622`、R6 `continue_session` `:950-999` |
| `sessions.rs` | `token_hex_16` / `initiator_of` `:259-275`、`new_session` / `start_task` `:1001-1116`、`thread_session` / `append_turn` / `reply` `:1289-1356` |
| `dispatch.rs` | 三个 guard `:365-407`、`notify_cancelled` `:702-721`、`dispatch_loop` … `dispatch_task` `:1118-1203`、`finalize_evidence` / `release_gateway_sandbox` `:1229-1285`、`steer_target` `:1359-1393`、`cancel_task` 的函数体（trait 方法留在 `plane.rs` 转调） |
| `reaper.rs` | `REAPER_INTERVAL_SEC` `:27-28`、`reaper_loop` `:1205-1227` |
| `evidence_log.rs` | `ROUTE_NEW_TASK` / `ROUTE_STEER` `:216-219` + `event_payload` / `steer_payload` `:226-257`（中间的 `WallClock` / `SleepFn` `:221-224` 不是证据助手，留在 `plane.rs`（或 builder 所在处），`lib.rs:31-35` 照旧从 crate 根导出它们） |
| `commands/mod.rs` | 原 `commands.rs` 全部（`parse_command`、`normalize_task_no`、`UNKNOWN_COMMAND_TEXT`、单测）+ 注册表 + `on_command` `:626-643` + `StopTarget` `:76-145` + `status_tasks` `:645-700` |
| `commands/{status,stop,restart,new}.rs` | `cmd_status` `:723-746` + `dropped_note` `:867-891`；`cmd_stop` `:748-772` + `resolve_stop_target` / `resolve_task` `:893-946` + 文案 `:30-39`、`:58-74`；`cmd_restart` `:774-853` + 文案 `:40-56`；`cmd_new` `:855-865` |

- **对外 API 一个字不变**：`lib.rs:29-35` 的全部 `pub use` 照旧可用；`ControlDeps` 字段与 `InProcessControlPlane::new` 签名不动（见 §3）；`aite_control::commands::{parse_command, normalize_task_no, UNKNOWN_COMMAND_TEXT}` 路径照旧（`commands.rs` → `commands/mod.rs`）；`plane.rs:1605-1710` 的 4 条单测跟着被测函数走（总数仍 13）。
- **预埋桩**（原卡清单，一个不少）：

| 文件 | 接在链的哪里（建议） | 以后归 |
|---|---|---|
| `gate.rs` | R2 之后、R3 之前（必须在 R5 命令路由之前：EE8 要让未启用的群 / 私聊里 `!connect <码>`、`!help`、`!about` 返回 `Pass` 放行到命令路由，D25 的外部群 restrict 也落在这里——都是 EE8 在桩内做的事） | EE8 |
| `routing/card_actions.rs` | R3 里、现有 Stop / Evidence 分支之前 | EE7（T0c 先加 log+drop 分支） |
| `routing/external.rs` | **R3 之后、`R4_KINDS.contains` 判定（`plane.rs:525`）之前**，对每个非卡片事件都调一次（今天一律返回 `Pass`）。不许放进 R4：R4 由 `R4_KINDS`（`plane.rs:148-153`）把门，T0 新增的 `EventKind::External` / `::Reaction` 不在里面，桩放在 R4 里 EE11 永远收不到事件，就得去改 `routing/mod.rs`（那时归 DD3） | EE11 |
| `routing/edits.rs` | R4 的 `MessageEdited` 分支（它在 `R4_KINDS` 里） | EE13 |
| `intro.rs` | R4 的 `BotAdded` 分支（它在 `R4_KINDS` 里） | EE9 |
| `routing/resolver.rs` | `thread_session` 没命中时（返回 `None`） | DD3 |
| `mute.rs` | **两个入口**：`mute::pre_r4` 与 `routing/external.rs` 同一位置（R3 之后、R4 判定之前，收 EE9 的 👎 reaction —— T0c 会把 `Reaction` 做成 R4 里只计数的分支，桩若只在 R5 前，EE9 永远看不到 reaction）；`mute::pre_r5` 在 R5 之前（收「回复 / @ 解除静音」这类消息侧逻辑） | EE9 |
| `dm.rs` | R6 之前，只收 p2p（关着 = 今天的行为） | FF4 |
| `sessions_channel.rs`、`ambient.rs` | R7 与 R8 之间 | FF1 |
| `internal.rs`（`submit_internal` 的落点）、`approvals.rs`（审批信箱） | 不进链 | DD3、EE7 |
| `commands/{about,access,configure,mute,unmute,feedback,routines,fork,memory,approve,reject,evidence,connect,usage,model}.rs` | 注册表，全部 `ENABLED = false` | about/access/configure → EE8；mute/unmute/feedback/fork/connect → EE9；routines → EE2；memory → EE1；approve/reject → EE7；evidence → EE12；usage → EE3；model → FF4 |
| `commands/help.rs` | 注册表；① 里先按停用桩建（`ENABLED = false`，保持零行为变化），⑤ 里**由本轨实现并启用**（`ENABLED = true`；原卡 [REVISION 2026-09-25]：`!help` 是 CC2 的，不是停用桩） | EE8（W3 只给新命令补条目） |
| `commands/{status,restart}.rs` / `{stop,new}.rs` | 注册表，启用 | DD3 / 无后续主人 |

- **桩的形状**（约束，不是建议）：进链的桩一律 `async fn …(&InProcessControlPlane, &NormalizedEvent, …) -> Result<Flow, IngressError>`（`enum Flow { Pass, Handled }`，或返回 `Option`），以 `if let Flow::Handled = x::hook(…).await? { return Ok(()); }` 接入；**不做 I/O、不 bump 计数器、不打日志**（否则替身调用记录会变）；**不写 `_ =>` 兜底分支**（契约枚举此刻是穷尽的，`unreachable_patterns` 会让 clippy `-D warnings` 红）；没有调用点的桩（`internal.rs`、`approvals.rs` 等）要么在 `lib.rs` 里声明成 `pub mod`（对外 API 只增不改，允许），要么在条目上写 `#[allow(dead_code)] // 主人：DD3/EE7` 这样注明主人轨——**光把函数写成 `pub` 不够**：私有模块里的 `pub fn` 从 crate 根够不着，照样报 `dead_code`，clippy `-D warnings`（check.sh A4a）会红。**R4 之前那两个钩子（`external::hook`、`mute::pre_r4`）对 `MessageEdited` / `BotAdded` 等 R4 事件也会被调到、返回 `Pass`**——这是零行为变化的，别为了「省一次调用」把它们挪到 R4 判定后面；按 kind 过滤是主人轨以后在桩内部做的事。每个命令文件自带 `pub(crate) const ENABLED: bool` 和统一签名的 `run`；`commands/mod.rs` 的分发一次写全 20 条，**以后启用命令只改那个命令自己的文件**。停用的命令走「未知命令」那条路，计数器仍是 `commands!<name>`。`routing/mod.rs` 头注释写清链的顺序、每个桩的位置和主人（含「`external` / `mute::pre_r4` 在 R3 之后、R4 判定之前，对所有非卡片事件调用，桩内自己按 kind 过滤」这一句）。
- **验证**：
  - `(cd core && cargo test -p aite-control --no-fail-fast)` → 0 failed；
  - 按开场自检第 5 步**同一条命令**（子 shell 那对括号、去掉 `; finished in …` 的那段 `sed`，一个字符都别改，只把重定向目标换成 split）存 `/tmp/cc2-counts-split.txt`，`diff /tmp/cc2-counts-B0.txt /tmp/cc2-counts-split.txt` → 无输出（两份清单都贴进回执）；
  - `(cd core && cargo clippy -p aite-control --all-targets -- -D warnings)` → exit 0；`(cd core && cargo fmt --check)` → exit 0；
  - 这个提交**不许**改任何测试文件的断言（搬单测不算）；`app` 的任何测试都不该因它变。
- 提交信息如「refactor(control): 零行为变化拆分 plane.rs + 预埋桩（CC2 ①）」。推送后立刻开 draft PR：先把 PR 描述写进 `/tmp/cc2-pr.md`，再 `gh pr create --draft --title "CC2: 控制面拆分 + 并发派发（默认 1）+ 命令注册表" --body-file /tmp/cc2-pr.md`。之后每个工作项推一次，CI 在 PR 上跑。

### ② 按会话串行、跨会话至多 N 并发：`with_max_parallel(n)`，默认 1

- `pub fn with_max_parallel(self, n: usize) -> Self`（`n == 0` 当 1），与 `with_clock`（`plane.rs:440`）同一种 builder。**默认 1 必须逐字节等价**：最稳的做法是 `n <= 1` 时走原 `dispatch_loop` 原样。唯一允许两条路共用的新东西是 ④ 的「放弃」钩子：它放在 `dispatch_task` 里（两条路都经过它），是对 `worker.run(...)` 与一个每任务 `Notify` 的 `tokio::select!`——`Notify` 不触发时与今天可观测地完全一样，逐字节等价的承诺不受影响。`run_pending` 保持串行。
- `n > 1`：同一 `session_id` 同时最多 1 个在跑；排在忙会话后面的任务不许挡住别的会话（无队头阻塞）；`pending()` / `state().queued` 口径不变。
- **结构化并发**：`run_forever(&self)` 是冻结签名（`ports.rs:224`），拿不到 `'static`，所以别 `tokio::spawn` 脱离的子任务——`app` 收尾靠 `runner.abort()` 把在飞的全部丢掉（`run.rs:258-261`），脱离的子任务会活过 abort。建议在 `run_forever` 这一个 future 里用 `std::future::poll_fn` 轮询一组 `Pin<Box<dyn Future + Send + '_>>`。准入临界区（`plane.rs:1160-1175`，cancelled → running）与三个 guard 的 drop 语义不变；收尾时 stranded 可能多于 1 条，`run.rs:287-289` 已逐个处理。**`run.rs` 不需要改就别改**，回执写「run.rs 未改」。
- 钉串行的现有测试（默认 1 下必须全绿、一行不改）：`control/tests/dispatch.rs`（全部）、`control/tests/commands.rs:820-826`、`app/tests/evidence_on_disk.rs:247`、`app/tests/reconnect_replay.rs:33-36`；B8 的 02（`after: idle`）/ 07（`after: running`）经 `evals/src/runner.rs:132` 跑真控制面。`evals/tests/dispatch_timing.rs` 用自带的 `SplitPlane`（`:41-60`），不直接受影响。
- 新测试：`default_max_parallel_is_strictly_serial`（不调 builder；两个群各一个任务，worker 卡住第一个时第二个不被领走）、`two_chats_dispatch_concurrently_with_max_parallel_2`（两个群同时在 worker 手上）、`same_session_stays_serial`（n=2，同会话第二个任务等第一个收尾）。建议再补一条 n=2 时 abort `run_forever` 后 `join()` 立刻返回（对照 `dispatch.rs:288-321`）。变异：把默认改成 2 → 第一条红；去掉同会话判断 → 第三条红。

### ③ 入口把存储 / 证据错误传出去

- `Ingress::handler()` 对 `IngressError::Store` / `::Evidence` 返回 `Err`（计数 `ingress.errors` 与 `ingress.handle_failed` 日志照旧），其余错误维持今天的 `Ok`；`Ingress::on_event` 仍返回 `()`（`ingress.rs:104-114`、`ingress_failures.rs:323-345` 照绿）。修正 `ingress.rs:11-12` 的错误注释。
- 新测试 `ingress_store_failure_returns_internal`：`FakeStore::fail_next("seen_event", 1)` 后调 `handler()` 得 `Err(IngressError::Store(_))`（control 不依赖 `aite-proto`，断言到这里；注释里引 `status.rs:63-69`：非 `Invalid` 一律 INTERNAL）。变异：恢复无条件 `Ok` → 红。
- 边界：`seen_event` **之后**的失败，重推仍会被 R2 当重复吃掉（`ingress_failures.rs:106-130` 钉着，保持绿）。本轨只做「传出去」；让它真能救回来要「撤销去重记录」，`SessionStore` 没这个方法 → 写进回执「契约缺口」。app 测试的 `GatedPlatform::emit` 对 `Err` 是 `expect`（`app/tests/common/mod.rs:217-223`），evals 的 `emit` 出错判 dispatch 失败（`evals/src/runner.rs:337-341`）——哪条 app / evals 测试因此变红，停下报告，不改那些文件。

### ④ `!restart` 回灌话题历史；卡死任务下条消息替换

- `!restart`：新会话建好后、写入 `rest` 那一轮之前，调 `platform.read_history(chat_id, <常量上限>, Some(thread_id))`（`supports_history` 为假就跳过），按时间正序写进新会话 transcript（人 → `User` 带 `platform_user_id`；其它 → `SystemNote` 并标出发言人；排除 `!restart` 这条本身）。读失败只 warn，重开照走。`support/mod.rs:666-673` 的假平台恒返回空，所以现有 restart 测试不变；给 support 加一个注入历史的开关。新测试 `restart_seeds_thread_history`（断言新会话的 turns = 历史在前、`rest` 在后）。
- 卡死：本进程 `running` 里的任务，库里 `updated_at`（worker 每步 `save()` 刷新，`worker/src/agent.rs:839-841`）距 `self.now()` 超过 N 分钟 = 卡死。N 用 control 内常量 + `with_*` 注入口给测试（今天模型超时 120s，`models/src/lib.rs:30`，CC6 要改成 600s；阈值按 600s 上界取，建议 15 分钟）。下一条落到该会话的消息：回一句提示（新文案，测试逐字钉住，如「任务 #A.. 超过 N 分钟没有进展，已停止，按这条消息重新开始。」）、取消卡死任务、按这条消息新开。
- **替换在默认 N=1 下也必须成立**（原卡无条件要求；W1 到 T0c 之前默认都是 1）。默认 1 下全局串行，光 `cancel_task` 没用——worker 卡在某一步里看不到取消标志，新任务永远排在它后面。机制（从不触发时与今天逐字节一致）：
  1. `Shared` 里加一张每任务的放弃信号表（如 `HashMap<String, Arc<tokio::sync::Notify>>`），在 `dispatch_task` 的准入临界区（cancelled → running）**之后**单独取一次锁登记，收尾时也单独取锁删（放在 `RunningGuard::drop` 放掉 `running` 之后，或另起一个 guard）——不和现有的锁嵌套，就不产生新的多锁组合；仍在 `Shared` 的锁序注释（`plane.rs:315-319`）里记一笔这把新锁。替换路径查表得 `None`（任务在判卡死和查表之间自己收了尾）→ 不发信号，直接走 `cancel_task`，它按 id 重读会处理终态。
  2. `dispatch_task` 把 `worker.run(task, session, Some(initiator), hooks).await`（`plane.rs:1201`）换成 `tokio::select!`：`worker.run(...)` 对该任务的放弃信号。信号到 → 丢掉 worker 的 future，`RunningGuard` / `FinishGuard` / `TaskDoneGuard` 照今天的 drop 语义收尾，派发循环继续取下一个。
  3. 替换路径（落到该会话的下一条消息）：触发放弃信号 → 等该 id 从 `running` 里消失（`Notify` 或短轮询都行）→ `cancel_task`，这时走「不在跑」分支，写 cancelled + `finalize` + 收卡片 + 还沙箱 → 回提示 → 按这条消息新开。worker 的 `in_flight` 条目会残留（`run.rs:24-26` 说明了原因），收尾时 `cancel_task` 按 id 重读会跳过它（`plane.rs:1481-1491`）。
  4. 替换测试**不调 `with_max_parallel`**（跑在默认 1 上）；测试里把 `run_forever` 放到另一条 tokio 任务上跑（同 `dispatch.rs:288-321` 的布置；这是测试侧，不违反 ② 里「实现里别 spawn 脱离的子任务」），因为 `run_pending` 与 handler 是同一条 future，没法在 worker 卡住时并发投下一条消息。
  放弃信号用 `Notify::notify_one`（没人在等时也留一个 permit，不丢信号），别用 `notify_waiters`。
  做不到就如实写进「没做的」，别造假绿。测试名自拟，如 `stuck_task_is_replaced_on_next_message_with_notice`；变异：去掉 `select!` 的放弃分支 → 它红（新任务一直排着）。

### ⑤ 命令解析、注册表、`!help`、中文别名、未知命令文案

- 认全角 `！`（U+FF01）与 ASCII `!`；按第一个 Unicode 空白切（U+3000、制表符等，`char::is_whitespace`）；别名只在带 `!`/`！` 时生效：状态→`!status`、停止→`!stop`、重开→`!restart`、新话题→`!new`、帮助→`!help`；计数器 key 用规范名（`commands!status`，`wording.rs:336-368` 照绿）。R5 的判定（`plane.rs:534`）同步认 `！`。
- `!help` **由本轨建成并启用**（原卡 [REVISION 2026-09-25]：`!help` 由 CC2 建并启用，不是停用桩；EE8 只给新命令补条目）：`commands/help.rs` 在 ① 里是停用桩（零行为变化），本条把它实现并改成 `ENABLED = true`，只做「列出已启用命令 + 一句说明」，是否列出按注册表里各命令的 `ENABLED` 判（`help_lists_enabled_commands` 要求停用的一个都不出现）；W3 起文件归 EE8。
- 未知命令文案改成 `未知命令，发 !help 看全部命令`；`KNOWN_COMMANDS` 由注册表取代。翻转的钉见 §3。
- 新测试：`fullwidth_bang_and_ideographic_space_parse`、`chinese_alias_status`、`help_lists_enabled_commands`（启用的都在、停用的一个都不在）。变异：各撤回一处 → 对应测试红。

### ⑥ 消费能力位：R6 看 `supports_thread`；p2p 进 DM 桩

- R6 的条件从 `ChatType::Group` 换成 `platform.capabilities().supports_thread`；p2p 事件先进 `dm.rs`（停用 = Pass），并且**不进 R6**——p2p 的净行为与今天一致（有 @ 走 R7，无 @ 走 R8）。飞书 / 假平台 `supports_thread = true`（`core/crates/contracts/src/capabilities.rs:30`、`core/crates/testing/src/fake_platform.rs:30-42`、`control/tests/support/mod.rs:592-594`），B8 不动。测试名自拟（如 `r6_requires_supports_thread`、`p2p_goes_to_disabled_dm_stub`）。
- 测试前提：`support/mod.rs:592-594` 的假平台写死 `feishu_p0()`（`supports_thread = true`），不加开关写不出 `r6_requires_supports_thread` → 给 support 的假平台加一个 capabilities 覆盖开关（support 在可写面内；默认仍是 `feishu_p0()`）。DM 桩按规矩不做 I/O、不计数、不打日志，测试观察不到「进了桩」这件事，所以 `p2p_goes_to_disabled_dm_stub` 定义成**净行为不变**：p2p 有 @ → R7 建会话，无 @ → R8 丢弃，且即使该 thread 有会话也**不进 R6**。变异：去掉 p2p 排除 → `p2p_goes_to_disabled_dm_stub` 红；恢复 `ChatType::Group` 判定 → `r6_requires_supports_thread` 红。

### ⑦ R6 追问补 ack（只对真人）

- R6 的 steer 与新建两条路都给该消息加 `ReactionKind::Ack`，失败只 warn（同 `plane.rs:1044-1053`）；机器人在 R1 就被丢（09 仍 `add_reaction == 0`）。新测试 `r6_followup_gets_ack`（两条路各一次 + bot 零次）。`app/tests/reconnect_replay.rs:458-471`、`cold_start_to_delivery.rs:252` 数 reactions——它们若变红，文件不是你的，停下报告。

### ⑧ 修 `!new` 之后同话题回复的路由

- 先写复现测试：话题里 `!new 另起一件事` 之后，同话题一条无 @ 的回复（`thread` = 老 root）应当进 `!new` 建的新会话。建议修法：话题内的 `!new` 用该话题的 `thread_id` 建新会话（`find_session_by_thread` 只找非归档的、多条取 `created_at` 最新，`ports.rs:133-138`），老会话不归档、其任务不动。这会翻 `tests/commands.rs:548-560` 的断言（「老话题没被动」→ 按 ROOT 查到的是新会话），逐条列进回执。测试名自拟。
- **注意**：这个修法让同一 `thread_id` 上同时挂着两个非归档会话，谁胜出全靠 `ORDER BY created_at DESC, id DESC`（`store/src/lib.rs:253-254`；FakeStore 同口径 `support/mod.rs:388-389`），而会话 id 是 `uuid::Uuid::new_v4()`（`plane.rs:1024`）——`created_at` 一旦相等，胜负是随机的。所以 ⑧ 的复现测试**不许用固定时钟**（用了 `with_clock` 就得在老会话建好之后、`!new` 之前把钟往前拨），回执里写明「这条路由的正确性依赖 `created_at` 严格递增」（SQLite 时间戳精度下同一时刻建两条会话同样会撞）。

### ⑨ `!stop` 记发起人；命令记 `event_received route=command`

- `cancelled` 载荷追加 `stopped_by`（发起人 `sender_id`），`by: "stop"` 与 `steps` 不动（`cancel.rs:49-52` 钉着）。`ControlPlane::cancel_task` 签名冻结 → 加 crate 内的 `cancel_task_by(…, issuer: Option<String>)`：`cmd_stop`（`plane.rs:761-762`）与 R3 卡片 Stop 分支（`plane.rs:583-591`，点按钮的也是真人）都传 `Some(ev.sender_id.clone())`；trait 方法转调时传 `None`（收尾那条路不变）。**`issuer` 为 `None` 时整个不写 `stopped_by` 键**（不写 `null`），让 `cancel.rs:20-52` 与收尾那条路的载荷逐字节不变。在跑的任务的 `cancelled` 由 worker 写（`worker/src/agent.rs:808-812`，CC3 的面）→ 那一支靠下面的命令证据记发起人，缺口记账转出。
- 命令作用到具体任务时，在该任务链上追加 `event_received`，`route = "command"`（导出 `ROUTE_COMMAND`，与 `ROUTE_NEW_TASK` / `ROUTE_STEER` 并列）+ `command` 键。**只写真会走 `cancel_task` 的目标**：`!stop` 解析出的 `StopTarget::Stoppable`，以及 `!restart` 里经 `of_existing` 判为 `Stoppable`、在 `plane.rs:813` 被 `cancel_task` 的那些（即计进 `stopped` 的）。卡片 Stop 按钮不写 `route=command`（它不是命令，`command` 键没有值可填），只按上一条记 `stopped_by`。**`StopTarget::Delivering` 一律不写**——`Answering` 任务在 `deliver()` 里马上会 `finish()` 落 manifest（`worker/src/agent.rs:818-836`），命令证据可能落在 manifest 之后，让 `manifest.json` 的 `root_hash` / `event_count` 过期（`evidence/src/writer.rs:379` 的 `verify_sync` 查不出这个）；`tests/commands.rs:790-812` 已钉「交付中任务的链不许动」。没有目标任务的命令、以及 Delivering 目标，都不写证据，回执写明。
- **顺序硬约束**：追加之前按 id 重读一次任务，已是终态或已有 `evidence_root_hash` 就跳过（不写、也照常往下走原逻辑）；然后先写命令证据、再 `cancel_task`（它自己还会再按 id 重读一次，`plane.rs:1481-1491`）——「不在跑」分支会 `finalize`，写在后面会让 manifest 对不上（`app/tests/evidence_on_disk.rs:311-322` 钉「最后一条是 cancelled」，P0-CLOSE 文件，碰不得）；建任务链前两条仍是 `task_created, event_received`（`plane.rs:1061-1062`）。重读与追加之间，在跑的 Stoppable 任务仍可能恰好收尾——这个残余窗口记进回执，不在本轨修（要修得动 worker，CC3 的面）。
- 测试名自拟；至少一条回归：对 `Answering` 任务发 `!stop`（照 `tests/commands.rs:602` 的 `a_task_stuck_in_answering` 那种布置；它是那个文件的私有函数，新文件里要抄一份或把测试写进 `tests/commands.rs`），断言该任务证据链长度不变、没有 `route=command` 那条。

### ⑩ plane 的 `append_turn` 撞 `DuplicateTurn` 就重读 seq 重试（交叉检查追加，不在原卡 9 项里）

- **为什么归本轨**：CC3 ③ 让 worker 在 `deliver()` 里用 `store.next_turn_seq` 写 `TurnRole::Assistant` 轮——那是在 plane 的 `turn_seq_lock` **之外**写。`plane.rs:1322-1344` 的锁只串住 plane 自己的写者、撞 `DuplicateTurn` 不重试，`:1312-1321` 注释里「进程内一把锁就够」随之失效：交付那一刻恰好进来的追问会在 `seen_event` 落库**之后**撞号——线上是这句追问丢失（M2 病复发，见 `ingress_failures.rs:211-219`）；本轨 ③ 把它传成 `Err` 之后，app 测试的 `GatedPlatform::emit` 是 `expect("emit")`（`app/tests/common/mod.rs:217-223`）→ panic。CC3 的派单把它记账给「CC2 / DD3」且不许 CC3 动 `control/`，而 DD3 在 W2，W1 合并后到 DD3 之间没人管；它保的正是原卡验收里 `cargo test -p aite --test reconnect_replay … → 0 failed` 那一条，所以本轨做。
- **做法**：plane 的 `append_turn` 仍在 `turn_seq_lock` 内，撞 `StoreError::DuplicateTurn` 就重读 seq（`list_turns(…, 1)` 或 `store.next_turn_seq`，`ports.rs:146`）再写，**至多 3 次**；3 次都撞 → 照旧把 `Store` 错误传出去（经 ③ 成 `Err`，与 §8「契约缺口」里 `seen_event` 之后失败的那条是同一个缺口，回执注明）。其它 `StoreError` 不重试。同步改 `:1312-1321` 的注释（写明 worker 在锁外写 Assistant 轮、plane 靠重试兜住）。
- **测试前提**：`FakeStore::fail_next` 只会报 `StoreError::Other`（`support/mod.rs:337-345`），造不出 `DuplicateTurn` → 给 support 的 `FakeStore` 加一个注入开关（如 `duplicate_next("append_turn", 1)`：触发时先替「别的写者」占掉该 seq、再报 `DuplicateTurn`，模拟 worker 抢先写；默认不触发）。
- **新测试 `plane_append_turn_retries_duplicate_turn`**（写成 `cc2_routing.rs` 的顶层函数，Δ 只 +1、不另多 support 那条）：私有的 `append_turn` 调不到 → 走 R6 追问（话题命中 → `continue_session` → `append_turn`，`plane.rs:956`）；注入一次后断言 `handler()` 得 `Ok`、`ingress.errors == 0`、追问落进 transcript 且 seq 接在被占的那个号后面、注入之后 `store.attempts` 里新增的 `append_turn` 恰好 2 次。变异：去掉重试 → 红。
- **合并顺序**：PR 描述顶部标「CC3 依赖本轨 ⑩；H10 保持 CC2 先于 CC3 合并（plan H10 的顺序 CC1 → CC2–CC7，别调换）」。

## 6. 规则

- 可写面 / 只读面见 §3；要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。
- **B8 不变量**：evals/p0 场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；`!stop` 仍立即释放沙箱（07 `release min:1`，且 07 的 `!status` 回帖里仍有 `#A1`）；对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 不许改。B8 红了就停下报告。
- **守卫**：被拦就停、原文进回执、不许绕。云端命令里永不出现：`AITE_RELOCK=1`（重锁变量赋值）、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、`.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`；受保护面还有 `core/crates/contracts/**`、`.claude/**`、`docs/dev-spec-*.md`——要看就用 Read 工具）、包着它们的 `$(…)`；多行脚本与多行文本（提交信息、PR 描述）先写成文件再用（`git commit -F <文件>`、`--body-file`；ASCII 引号跨行会被判「无法解析」）；`check.sh` 不接 `| tail`。
- 每个行为改动：回归测试 + 变异验证（撤回改动 → 测试红 → 贴输出）；还原时别用 `cp -p` / `shutil.copy2`（旧 mtime 让 cargo 跳过重编，出假绿）。
- 格式化：`(cd core && rustfmt --edition 2024 <core 下的相对路径>)`（必须在 `core/` 下跑：工具链只由 `core/rust-toolchain.toml` 钉住，仓库根可能没有默认工具链或用上别的 stable），**不要 `cargo fmt --all`**；`check.sh` 的 A4b（`cargo fmt --check`）与 A4a（clippy `-D warnings`）都要过。
- 新第三方依赖、R0 文件（不在你可写面里的）、锁定面 → 停下报告。Docker 测试封闭（只连本地测试服务器）；云端 protoc 生成的 `edge/gen` 永不提交（本轨也不该碰到）。
- 已知时序抖动测试先单跑再下结论：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、`aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。

## 7. 验收（命令 + 期望输出）

1. `(cd core && cargo test -p aite-control --no-fail-fast)` → 每个二进制 `0 failed`；① 的提交单独检出时，两份快照都用开场自检第 5 步那条命令生成（同一个子 shell 形式，`grep -E '^ +Running|^test result' | sed -E 's/-[0-9a-f]+\)$/)/; s/; finished in .*//'`），`diff /tmp/cc2-counts-B0.txt /tmp/cc2-counts-split.txt` 无输出（两份清单贴回执）。
2. `(cd core && cargo test -p aite-control -- default_max_parallel_is_strictly_serial two_chats_dispatch_concurrently_with_max_parallel_2 same_session_stays_serial ingress_store_failure_returns_internal fullwidth_bang_and_ideographic_space_parse chinese_alias_status help_lists_enabled_commands restart_seeds_thread_history r6_followup_gets_ack plane_append_turn_retries_duplicate_turn --exact)` → 这 10 个名字（原卡 9 个 + ⑩ 的 1 个）每个恰好出现一次且为 `ok`（libtest 过滤默认是子串匹配，`--exact` 防同前缀的测试混进来；它要求全名相等，所以这 10 条要写成 `tests/cc2_*.rs` 的顶层函数，别套在 `mod` 里）；每条的变异验证输出贴回执（同样适用于 ④⑥⑧⑨ 自拟名字的测试）。
3. `(cd core && cargo test -p aite --test reconnect_replay --test graceful_shutdown --test startup_recovery)` → `0 failed`（只读；抖了就单跑那一个 target，两次输出都贴）。
4. `(cd core && cargo clippy -p aite-control --all-targets -- -D warnings)` → exit 0。
5. `scripts/check.sh` → 末行 `全部通过`、exit 0；行：`cargo passed=897+Δ failed=0`（情形 B：`901+Δ`；Δ 按测试名逐条列，含每个新测试二进制多出的那条 support 测试、以及删掉或合并的钉）、`contracts passed=25 failed=0`（情形 B：27）、`OK 25 files`、`passed 10/10`、Go 格 6 行 `ok` + 2 行 `?`，`cmd/aite-edge` 单跑 ok，`grep -cE '^(ok|\?)'` 得 9（跑的 check.sh 若已含 CC1 的 `go packages ok=N fail=M` 行，以那行为准：`fail=0`，基线上是 `ok=9 fail=0`）。
6. `git diff --name-only origin/main...HEAD` → 每一行都落在 §3 可写面内。Go 模块文件有没有变由审 PR 的人看，你的命令里不要出现那两个路径。

## 8. 回执（写 `review/p1/ledger/CC2.md`，PR 描述贴摘要）

- **开场自检原文（4 项 + 第 5 项）**：第 1 步输出与判定（A / B）、守卫拦截原文、三条版本号、`check.sh` 各关键行原样、`/tmp/cc2-counts-B0.txt` 内容。
- **工作项逐条**：①–⑩ 各改了哪些文件:行；桩清单（文件 → 链上位置 → 主人轨）；④ 卡死替换做到哪一步（默认 N=1 下是否成立；`select!` 丢掉 worker future 时若正落在 `store.update_task` / 证据追加中间，证据链与库是否仍一致）；⑤ `help.rs` 在 ①（停用桩）与 ⑤（启用）两个提交里各是什么状态、`!help` 回帖原文；⑧「路由正确性依赖 `created_at` 严格递增」；⑨ 哪些目标写了命令证据、Delivering 不写、重读与追加之间的残余窗口；⑩ 重试上限、3 次都撞时走哪条错误路、`:1312-1321` 注释改成了什么、「CC3 依赖本轨 ⑩，H10 须 CC2 先于 CC3 合并」（PR 描述顶部同样标）；「run.rs 改了什么 / 未改」。
- **新增测试逐条 + 变异验证输出**：测试名 → 钉什么 → 撤回哪处 → 红的原文。
- **`check.sh` 完整输出**（原样，不截）。
- **cargo passed 增量逐条**：哪个文件加了哪几条（含 support 那条的重复计数、⑩ 的 `plane_append_turn_retries_duplicate_turn`）、哪些钉被改写 / 删除（改前 / 改后）。
- **被守卫拦过的命令与拦截原文**（没有就写「无」）。
- **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）：至少考虑 `docs/acceptance-M.md:932` 的旧命令表；worker 写的 `cancelled` 没有发起人（CC3 / DD4）；`with_max_parallel` 接配置（T0c）。
- **没做的与原因**。
- **契约缺口**（给 T0 / T0.1；写清要什么形状、为什么开放通道绕不过去）：至少评估——让 `seen_event` 之后失败的事件能被重推救回（撤销单条去重记录）；卡死阈值是否进 `WorkerConfig`；在跑任务的 `cancelled` 要带发起人是否需要 `RunHooks` 多一个字段。
