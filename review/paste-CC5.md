# 派单 CC5：存储迁移机制 + P0 卫生 + Answering 孤儿恢复（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC5.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC5）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

本轨对应 Claude Tag 的 **CT02**（transcript 存服务端、会话可按消息找回）、**CT05**（重启后卡住的任务要收场）、
**CT26**（按群记成本 → tasks 表要有 chat / cost 查询列，并引入迁移机制）。总计划 §3 表 D2 行已拍板：
**SQLite + 迁移机制，由 CC5 建**。

今天的样子（行号基于 `98e4460`，均已核对）：

- **没有迁移机制**。`core/crates/store/src/lib.rs:36-73` 的 `SCHEMA` 是一串 `CREATE … IF NOT EXISTS`，
  `init()`（`lib.rs:208-211`）每次整串 `execute_batch`。加一列就没法对存量库做了。
- **`seen_events` 只增不减**（`lib.rs:70-72` 只有 `event_id` 一列）：没有时间，没法剪枝。
- **tasks 没有查询列**（`lib.rs:55-62`）：成本、token 只在 `data` JSON 里；按群查任务要 JOIN sessions（`lib.rs:416`）。
  注意 `Task` 结构体本身**没有 `chat_id`**（`core/crates/contracts/src/session.rs:114-154`，只有 `session_id`）。
- **Answering 孤儿没人收**。worker 在没发过卡片的那一路先落 `Answering`（`core/crates/worker/src/agent.rs:680-685`），
  之后才 `send_text`、最后落 `Delivered`。进程死在这中间，`recover_orphan_tasks`（`lib.rs:476-507`）
  只按下标绑 `ACTIVE_TASK_STATUSES[0..2]`（`lib.rs:482-484`，即 created/planning/working，
  见 contracts `session.rs:32-37`）→ 这条任务永远停在 answering，群里没人告诉用户。
- **没有「消息 → 会话/任务」索引**：以后钉钉 / 企微的 `#A` 锚点、引用解析都要它。

**谁用你的产物**（所以接口要稳、回执要写清）：

- **DD1**（W2，依赖 T0c + CC5）：把你的固有方法通过 T0 给 `SessionStore` 加的默认方法暴露出去
  （T0 卡上的新方法名：`index_message`、`find_session_by_message`、`prune_seen_events`、`list_tasks(&TaskQuery)` 等），
  并在你的迁移框架上加迁移 3+（五个领域存储的表）。
- **DD3**（W2）锚点解析里「消息索引命中」那一格查的就是你的消息索引表。
- **CT26 按群成本查询**用 `tasks.chat_id / cost / tokens_*`（DD1 / EE3 / FF7 可能直接用，也可能经 DD1 的 `UsageLedger`；EE3 原卡记的是 `UsageRecord`）。
- **EE7**（W3）负责 `awaiting_approval` 在启动时怎么处理（D17：v1 审批不跨重启，见总计划 §3 表 D17 行）——**所以你不许把它当孤儿**。
- **DD1** 的 trait 方法 `prune_seen_events` 委托你的固有方法 `prune_seen_events_before`。

## 2. 必读（按顺序）

1. `CLAUDE.md`（仓库约定：守卫、验收、B8、时序抖动测试清单、规则）。
2. 总计划（按节引用；计划还在被别的会话改，行号会漂）：§4.4 开场自检、§5.2「会话」一条（`ACTIVE_TASK_STATUSES` 不动、
   `OPEN_TASK_STATUSES` 含 `awaiting_approval`）、§6 开头的每波规则、§6.1 表 CC5 行、§9 解冻 / 仍冻结清单。
   英文原卡里 `contract_batches` 的 T0-p1.0 那条（SessionStore 默认方法清单）——读 JSON 用 Read 工具，或把 python3 脚本写成文件再跑。
3. `core/crates/store/src/lib.rs` 全文（508 行）。重点：模块头 `:1-16`、`SCHEMA :36-73`、`stamp :79-83`、`open :113-130`、
   `pragma / pragma_int :139-159`、`init :208-211`、`create_task / update_task :360-394`、`list_active_tasks :410-429`、
   `seen_event :454-469`、`recover_orphan_tasks :471-507`。
4. 契约（**用 Read 工具读，Bash 命令里别出现 contracts 路径**）：`core/crates/contracts/src/session.rs:18-50, 114-154`、
   `ports.rs:127-159`（`init` 的契约是「建表，幂等」）、`errors.rs:56-69`（`StoreError` 只有 5 个变体，不许加）。
5. 钉着你要动的行为的测试：
   - `core/crates/store/tests/concurrency.rs:322-388`（孤儿三条；`:357-379` 那条会被你解冻，见 §3）；
   - `core/crates/store/tests/persistence.rs:140-185`（`list_active_tasks_scope`：Answering 不算活跃——**保持绿**）；
   - `core/crates/store/tests/crash_recovery.rs:74-94`（`journal_mode == delete`）、`:179-217`（写失败必须 Err）；
   - `core/crates/store/tests/session_lookup.rs:133-164`（`busy_timeout == 5000`，每个新实例都设）；
   - `core/crates/store/tests/common/mod.rs:45-112`（造数工具）；
   - `core/crates/app/tests/startup_recovery.rs:474-499, 557-575`（`a_broken_store_query_does_not_block_takeoff` 与 `break_tasks_table`）；
   - `core/crates/app/tests/common/mod.rs:611-648`（`read_task_from_disk` / `tasks_from_disk`：**app 还活着时另开一个 store 调 `init()`**）；
   - `core/crates/app/tests/preflight_e2e.rs:1127-1151, 1174-1244`（`init()` 报错必须含 `not a database` / `readonly`）；
   - `core/crates/app/tests/signals.rs:222-340`（`sigterm_before_serve_still_exits_by_itself`，`:259`；只读）：钉着 `init()` 在外部
     EXCLUSIVE 锁下会排队等——`:265-268` 父进程 `BEGIN EXCLUSIVE`，`:279-282` 锁着时 `aite.up` 不许出现。你的只读快路径 `SELECT`
     在 `journal_mode=delete` 下撞 EXCLUSIVE 同样拿到 `SQLITE_BUSY`、在 `busy_timeout` 里排队，所以它照样绿；迁移器重写**不许**破坏这一点；
   - `core/crates/app/src/run.rs:156-165, 364-452`（起飞时 `init` → `recover_orphans`；只读，别动）；
   - `core/crates/worker/tests/test_final.rs:39-57`（Answering 那一格真的被走过、且不在活跃集）。

## 3. 工作区

- **分支**：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC5: 存储迁移机制 + Answering 孤儿恢复」。
  PR 描述先用 **Write 工具**写成 `/tmp/cc5-pr.md`（此时回执还不存在），再 `gh pr create --draft --title "CC5: 存储迁移机制 + Answering 孤儿恢复" --body-file /tmp/cc5-pr.md`；
  收尾时 `gh pr edit --body-file review/p1/ledger/CC5.md`（或先 Write 一份摘要文件再 `--body-file` 它）。**永远别**把多行正文塞进 `--body "…"` 或 heredoc（见第 6 节守卫）。
- **代码基线**：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**（只有这些）：
  - `core/crates/store/**`：现有 `Cargo.toml`、`src/lib.rs`、`tests/{common/mod.rs,concurrency.rs,crash_recovery.rs,persistence.rs,session_lookup.rs}`；
    可新建 `src/migrate.rs`、`tests/migration.rs` 等（新建）。**但 `store/Cargo.toml` 的依赖表一行都不改**——
    哪怕加的是 workspace 里已有的 crate，也会改动 `core/Cargo.lock` 里 aite-store 那一条，而 `Cargo.lock` 是 CC1 的 R0。
    现有依赖（rusqlite 0.40 bundled、serde_json、chrono、tracing、tempfile）够用。
  - `core/crates/app/tests/startup_recovery.rs`、`core/crates/app/tests/crash_recovery.rs`
  - `review/p1/ledger/CC5.md`（新建）
- **只读面**：其余一切。特别点名：
  - P0-CLOSE 的 14 个路径（总管在 W1 期间本机打；与 §4 开场自检第 1 步情形 B 那张表是同一张）：
    `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`、
    `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
    `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
    `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`、`.contracts.lock`、
    `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；
    另外 `.claude/**`、`core/crates/evidence/**` 整片也一律只读。
  - T0 补丁文件：`proto/aite/v1/*.proto`、`core/crates/contracts/**`、`core/crates/proto/**`、`edge/internal/server/server.go`、`config/aite.example.yaml`。
  - 邻轨：**CC2** 的 `core/crates/app/src/run.rs`（`recover_orphans :364-379`、孤儿回帖文案 `:444`）与 `core/crates/control/{src,tests}/**`；
    **CC3** 的 `core/crates/app/tests/sqlite_cross_process.rs`（你要跑它，但一个字不改）、`reconnect_replay.rs`、`core/crates/worker/{src,tests}/**`
    （`core/crates/worker/prompts/platform.md` 不归 CC3，W2 归 DD4，本波同样只读）；
    **CC4** 的 `core/crates/app/src/{app,wiring,lib}.rs`（`app.rs:244` 打开 store）；**CC7** 的 `core/crates/testing/**`（`FakeSessionStore`）；
    **CC1** 的 `core/Cargo.toml`、`core/Cargo.lock`、`core/crates/app/Cargo.toml`、`scripts/check.sh`。
  - 本波无主、一律只读：`core/crates/app/tests/common/mod.rs`、`preflight_e2e.rs`、`cli_smoke.rs`、`signals.rs`、`core/crates/app/src/preflight.rs`、`evals/p0/*.yaml`。
- **本轨解冻 / 仍冻结**：
  - 计划 §9 的解冻清单里没有 CC5 的条目；本轨按原卡**解冻一个测试钉**：「Answering 不算孤儿」——
    `core/crates/store/tests/concurrency.rs:357-379` `recover_orphan_tasks_leaves_finished_tasks_alone` 的状态列表（`:364-369`）里
    `TaskStatus::Answering`（`:368`）那一格。回归测试 = `answering_task_recovered_as_orphan`，要做变异验证；回执「lifted pins」里点名，并在「记账转出去的」里请总管把它补进计划 §9（见 §8 第 7 项）。
  - **仍冻结**（碰了就停）：`ACTIVE_TASK_STATUSES`（contracts `session.rs:33-37`，连同 `list_active_tasks` 的绑定 `lib.rs:419-424`）、
    `journal_mode=delete`（不设任何 journal pragma）、`busy_timeout 5000`（`lib.rs:33, 124-125`）、`ORPHAN_RESULT_SUMMARY` 文案（`lib.rs:30`）、
    SessionStore trait（contracts 锁定，不改签名、不加方法）。

## 4. 开场自检（全部对上才开工；对不上就写回执停下）

第 1-4 步逐字照抄总计划 §4.4（第 2 步另补一句「被拦就是通过」）；第 5 步是本轨附加。

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
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（原文形如 `blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。`，把拦截原文贴进回执）。
   **这一步被拦就是通过；拦截原文里「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步。**
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认。
   CC1 合并之后 check.sh 会多打一行 `go packages ok=N fail=M`，从那以后以这行为准。

- 记下你是情形 A 还是 B（回执第 1 节要写）。
- 以上命令每条都从仓库根起跑（Bash 工具会记住上一条的 `cd`）；`cd core && …` 写成 `(cd core && …)` 也行。

5. **本轨附加**（按源码数出来的期望，未实跑；对不上先写进回执再判断）：
   - `(cd core && cargo test -p aite-store 2>&1 | grep -E '^test result')` → **6 行**，按顺序 0（lib 单测）/ 13 / 7 / 6 / 6 / 0（doctest），
     合计 **32 passed**（中间四行 = concurrency / crash_recovery / persistence / session_lookup，cargo 按测试目标名排序）；
   - `(cd core && cargo test -p aite --test startup_recovery --test crash_recovery --test sqlite_cross_process 2>&1 | grep -E '^\s+Running|^test result')`
     → 按文件名排序输出：crash_recovery **4** / sqlite_cross_process **2** / startup_recovery **9**，0 failed（顺序与 `--test` 的书写顺序无关）。

## 5. 工作项

建议顺序 ①→②→③→④，每项一个提交。本卡**没有**「先零行为拆分」那一步。

### ① 迁移机制（`schema_version` 表 + 有序、事务化的迁移器，拒绝更新的库）

- **表**：`schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)`，一次迁移一行；当前版本 = `MAX(version)`，
  表不存在或为空 = 0（= 任何 P0 的库）。**先查 `sqlite_master WHERE type='table' AND name='schema_version'` 判表在不在**
  （或只把 `no such table` 当 0）；其它任何错误都经 `sq()` 原样上抛——别把「读版本出错」一律当 0，
  否则 `not a database` 这类错误要到后面才冒出来。版本号用 `pub const LATEST_SCHEMA_VERSION`（store 导出）表示「本程序认识的最新版本」。
  **别用 `PRAGMA user_version`**（卡上写的是表）；也**别碰 `PRAGMA schema_version`**——
  那是 SQLite 内置的 schema cookie，`app/src/preflight.rs:597-599` 拿它当只读探针，写它会把库弄坏。注释里写清你说的是哪一个。
- **迁移清单**：一个按版本号严格递增的静态数组（建议放新文件 `core/crates/store/src/migrate.rs`），每条是 Rust 函数
  （迁移 2 要绑参数，纯 SQL 串不够）。**迁移 1 = 现在的 `SCHEMA`（`lib.rs:36-73`）逐字不改**——全是 `IF NOT EXISTS`，
  所以在 P0 老库上是空操作。
- **`init()`（`lib.rs:208-211`）里跑迁移器**，流程定死：
  1. 快路径：普通 `SELECT` 读版本，**不开写事务**。等于最新 → 直接返回，**零写入**。
     理由有二：`app/tests/common/mod.rs:611-648` 在 app 连接还活着时另开 store 调 `init()`，几乎每条 app 测试都走这里，
     init 一抢写锁就会撞 `busy_timeout` 5 秒、把时序抖动测试弄得更抖；`preflight_e2e.rs:1174-1180` 记着
     「已建完表的只读库上 `init()` 成功」这一既有行为。
  2. 大于本程序认识的最新版本 → **在任何写之前**返回 `StoreError::Other(…)`，中文、带上两个版本号（如「库的 schema 版本 3 比本程序支持的 2 新，拒绝打开」）。不加新 `StoreError` 变体。
  3. 小于 → `rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)`（`with_conn` 只给 `&Connection`；
     别用默认 deferred 的 `unchecked_transaction`，读后升级写锁会撞 BUSY），**在事务里重读一次版本**（防两个实例同时起飞，
     见 `concurrency.rs:13-15` 说的 `systemctl restart` 叠在一起的形状），逐条执行待跑迁移并写 `schema_version` 行，最后 commit。
     任一步失败 → 回滚、版本不变、返回错误。
  4. 错误一律经 `sq()`（`lib.rs:75-77`）保留 rusqlite 原文：`preflight_e2e.rs:1147-1150` 要 `not a database`、`:1237-1240` 要 `readonly`。
- 保持不变：`init()` 仍然**不**收孤儿（`lib.rs:471-475` 的理由照旧）；`open()` 里设 `busy_timeout`（`lib.rs:124-125`）不挪。
  模块头 `lib.rs:12-16`「不显式 commit」那段要补一句迁移与孤儿恢复是例外。
- **新测试**（新建 `core/crates/store/tests/migration.rs`）：
  - `migrates_p0_db_in_place`：夹具**用 P0 schema 建库**——把 `lib.rs:36-73` 的 SQL 原样抄成测试里的常量 `P0_SCHEMA`
    （**别引用 lib 里的常量**，否则以后改迁移 1 夹具会跟着悄悄变），用裸 `rusqlite::Connection` 建表并插入：一个会话、
    一个 `cost`/`tokens_in`/`tokens_out` 非零的任务（JSON 用 `serde_json::to_string(&common::make_task(..))`）、两轮 turn、
    `task_counters`、两条 `seen_events`。然后 `open + init`，断言：版本行正好 = `1..=LATEST_SCHEMA_VERSION`（别写死 {1, 2}——
    DD1 在 W2 加迁移 3+，写死的数会因为与本测试无关的原因红）；`get_session` / `get_task` / `list_turns` /
    `find_session_by_thread` 读回的与写入的相同；老的 `seen_event` 仍返回 true；`next_task_no` 接着发；新列回填值（裸 SQL 查）
    等于 JSON 里的值、`chat_id` 等于会话的 `chat_id`；`sqlite_master` 里有新索引与新表；`pragma("journal_mode") == "delete"`。
    最后 `update_task` 改一次 cost、再 `create_task` 一个新任务，断言新列跟着写（②的「写时同步」就钉在这里）。
  - `migration_idempotent`：同一实例 `init()` 两次、同一文件第二个实例再 `init()` 一次，都 Ok；版本行仍正好 = `1..=LATEST_SCHEMA_VERSION`；数据不变；
    **第二次起的 `init()` 前后库文件字节完全相同**（比整文件字节，别靠 `chmod 444`——root 下拦不住写，变异验证会假绿）。
  - `refuses_newer_schema_version`：迁好之后裸 SQL 插一行 `version = LATEST_SCHEMA_VERSION + 1`，重开 `init()` → `Err(StoreError::Other(_))`，
    消息含两个版本号；库文件字节不变。

### ② 迁移 2：`seen_events.seen_at` + 剪枝；tasks 查询列 + 回填 + 索引；消息索引表

- **`seen_events.seen_at`**：`ALTER TABLE … ADD COLUMN seen_at TEXT`（SQLite 的 ADD COLUMN 不许用 `CURRENT_TIMESTAMP` 作默认值），
  随后把存量行回填成**迁移时刻**（用 `stamp()`，`lib.rs:79-83`，定长 6 位小数 + `Z`，字典序 = 时间序；别用 SQLite `strftime` 的 3 位小数，
  格式不一样就比不对）。存量行不留 NULL——NULL 永远不满足 `<`，会永远剪不掉。加 `seen_at` 索引。
  `seen_event()`（`lib.rs:454-469`）插入时写 `seen_at = stamp(Utc::now())`；「首次 false / 之后 true / 写失败 Err」语义一字不改
  （`store/tests/crash_recovery.rs:179-217`、`concurrency.rs` 那几条并发去重钉着）。
- **固有方法** `prune_seen_events_before(&self, cutoff: DateTime<Utc>) -> Result<u64, StoreError>`（名字照卡）：
  `DELETE … WHERE seen_at < stamp(cutoff)`，返回删掉的行数。W1 没有调用方（DD1 经 trait 的 `prune_seen_events` 暴露）。
  - 新测试 `seen_events_prune`（放 `tests/migration.rs`）：造新旧两批 `seen_at`，剪枝返回旧批条数；旧 id 再 `seen_event` 返回 false（重新记录），新 id 仍 true。
- **tasks 查询列**：`chat_id TEXT`（**可空**）、`cost REAL NOT NULL DEFAULT 0`、`tokens_in INTEGER NOT NULL DEFAULT 0`、
  `tokens_out INTEGER NOT NULL DEFAULT 0`（卡上写 tokens；拆两列是因为 `Task` 本来就分 `tokens_in/out`（`session.rs:137-140`），
  EE3 按输入 / 输出分别计价）。**回填**：`cost` / `tokens_*` 从 `data` 取（`json_extract`，缺省 0）；**`chat_id` 不在 JSON 里**，
  要用 `(SELECT chat_id FROM sessions WHERE id = tasks.session_id)` 回填——卡上「从 data JSON 回填」对 chat_id 不成立，按这里做。
  会话行没了就留 NULL（`startup_recovery.rs:435-472` 专门造这种残库，起飞必须照常）。加索引 `tasks (chat_id, created_at)`。
  **写时同步**：`create_task` / `update_task`（`lib.rs:360-394`）每次都写这四列（chat_id 用同样的子查询按 `session_id` 取），
  不然列从第二天起就是陈的。`list_active_tasks`（`lib.rs:410-429`）的 JOIN、`get_task` 等读路径**一律不改**（零行为变化）。
- **消息索引表**（建议表名 `message_index`，自定就写进回执）：`chat_id TEXT NOT NULL, message_id TEXT NOT NULL,
  session_id TEXT NOT NULL, task_id TEXT, outbound INTEGER NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (chat_id, message_id)`
  （`created_at` 用 `stamp()`。**这是超出原卡的一列**——原卡只写了 `(chat_id, message_id -> session_id, task_id, outbound)`；
  理由：以后按群清除 / 留存要按时间剪；加不加自己定，加了就在回执写明理由）。
  **固有方法**：`index_message(...)` 与 `find_session_by_message(chat_id, message_id) -> Result<Option<…>, StoreError>`——
  与 T0 卡上 trait 默认方法同名，DD1 一行委托即可（固有方法在方法解析上优先于 trait 方法，同名不会递归）；
  返回类型用本 crate 自定义的 `pub struct`（contracts 锁定，别指望那边加类型），参数用 `&str` / `Option<&str>` / `bool` 这类原始类型。
  同一 `(chat_id, message_id)` 重复索引必须幂等、不报错；「保留首条」还是「覆盖」自己定，钉进测试、写进回执。W1 没有调用方。
  - 新测试 `message_index_roundtrip`（放 `tests/migration.rs`）：入站 / 出站各一条（一条带 task_id、一条不带）写入再读回，字段逐一相等；
    查不存在的键返回 None；跨 chat 同 message_id 互不串；重复索引按你定的语义。

### ③ `recover_orphan_tasks` 纳入 answering（显式状态列表，`awaiting_approval` 不算）

- 在 store 里定义私有常量 `ORPHAN_TASK_STATUSES: [TaskStatus; 4] = [Created, Planning, Answering, Working]`，
  文档注释写清：「= T0 的 `OPEN_TASK_STATUSES` 去掉 `AwaitingApproval`；`awaiting_approval` 在启动时怎么处理归 EE7（D17）」。
  把 `lib.rs:480-485` 的 SQL 改成 `IN (?1, ?2, ?3, ?4)` 并逐个绑 `.as_str()`。**只改这一处**：`list_active_tasks` 的绑定
  （`lib.rs:419-424`）与它的注释 `:410` 不动，`ACTIVE_TASK_STATUSES` 不动。**别等 T0 落地后改用 `OPEN_TASK_STATUSES`**——它含 `awaiting_approval`（总计划 §5.2「会话」一条）。
- 改测试：`concurrency.rs:364-369` 的列表去掉 `TaskStatus::Answering`（`:368`），测试本身保留（解冻，见 §3）。
  `persistence.rs:140-185`、`worker/tests/test_final.rs:53-56`、contracts 的 roundtrip 都是在说「Answering 不**活跃**」，与孤儿口径无关，保持绿。
- 新测试（放 `core/crates/store/tests/crash_recovery.rs`，照 `:21-37` 的崩溃手法：造数 → 改状态 → 只关 fd → 同文件重开 → 收残局）：
  - `answering_task_recovered_as_orphan`：停在 answering 的任务被收成 failed、`result_summary == ORPHAN_RESULT_SUMMARY`、落盘（重读一次）、任务号带得出来。
  - `awaiting_approval_not_recovered_as_orphan`：停在 awaiting_approval 的任务不在返回值里，状态与 `result_summary` 原封不动。
- app 层：`run.rs:364-379` 不改就会给 answering 孤儿回帖（answering 那一路没发过卡片，`close_orphan` 的卡片分支自然跳过）。
  **不新增 app 层测试名**；真要加，只能加进 `core/crates/app/tests/startup_recovery.rs`（可写面内），点名、算进 Δ；
  **不新建 `cc5_*.rs`**（总计划 §6 说新集成测试放 `app/tests/<轨号>_*.rs`，但它不在本卡可写面）——加不进去就记账转出。

### ④ 两个 app 测试文件的同步

- `startup_recovery.rs:474-479` 与 `:557-560` 的注释描述的是旧 `init()`（每次 `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX`）。
  按新机制重写注释：库已是最新版本 → `init()` 不碰 tasks 表 → 只有 `recover_orphan_tasks` 那一支红，起飞照常。
  `a_broken_store_query_does_not_block_takeoff` 的**意图不许变**；你的设计若让 `init()` 碰到 tasks，就改 `break_tasks_table` 的造坏手法保住意图，并写进回执。
- `core/crates/app/tests/crash_recovery.rs`：预期不用改（`:166-168` 说 Answering 不在活跃集，仍然成立）。真改了就在回执说为什么。

## 6. 规则

- **可写面 / 只读面**以 §3 为准；需要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。
- **B8 不变量**（`evals/p0` 钉死的，任何一轨弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；
  顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；`!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）。
  `evals/p0/*.yaml` 不许改。B8 红了 → 停下写回执。
- **守卫**：被拦就停（开场自检第 2 步那次除外：那次被拦就是通过）、原文进回执、不许绕。云端命令里永不出现 `AITE_RELOCK=1`、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`。读 contracts 用 Read 工具；
  grep 只对 `core/crates/store`、`core/crates/app/tests` 这类非保护路径跑。`scripts/check.sh` 不接 `| tail`。
  多行脚本、PR 描述、回执、多行提交信息**一律先用 Write 工具落文件再用**（`python3 <文件>` / `--body-file <文件>` / `git commit -F <文件>`；命令行 `-m` 只写单行）：
  守卫对跨行引号一律判「无法解析」，也扫 heredoc 正文 —— 回执要逐字贴守卫拦截原文（里面有 `guard_bash.py` 路径），走 heredoc / `echo >` / `--body "…"` 必被拦，被拦按规则就得停。
- **每个行为改动：回归测试 + 变异验证**。做法：先提交 → 在工作区手改撤回那一处 → 跑对应测试看红（贴失败输出）→
  `git checkout -- <文件>` 还原（git 会写出新 mtime）→ 再跑一次看绿。**还原别用 `cp -p` / `shutil.copy2`**：旧 mtime 让 cargo 跳过重编，变异验证假绿。
- **格式化**：`(cd core && rustfmt --edition 2024 <改过的文件，core 起的相对路径>)`（不要 `cargo fmt --all`；在仓库根跑会找不到工具链，见 §7）。check.sh 的 A4b 跑 `cargo fmt --check`，提交前自查。
- **doc 注释里的代码块用 ` ```text `**（照 `concurrency.rs:4`）：裸 ` ``` ` 会变成 doctest，让 `cargo passed` 凭空多出来。
- 新第三方依赖、R0 文件、锁定面 → 停下报告。Docker 测试封闭（本轨用不到 Docker）。云端 protoc 生成的 `edge/gen` 永不提交。
- **已知时序抖动**（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、
  `aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。
  单跑五遍全绿 = 假红，记一笔继续；稳定复现 = 你踩到了东西，停下报告。

## 7. 验收（命令 + 期望输出）

```bash
# 每一行都从仓库根起跑（Bash 工具会记住 cd，所以一律用子 shell）
(cd core && cargo test -p aite-store 2>&1 | grep -E '^test result')
#   期望 7 行，按顺序 0（lib 单测）/ 13 concurrency / 9 crash_recovery / 5 migration / 6 persistence / 6 session_lookup / 0（doctest），
#   全部 0 failed，合计 39 passed = 32 + 本轨 7 条
(cd core && cargo test -p aite-store --test migration)      # 5 passed（migrates_p0_db_in_place / migration_idempotent /
                                                            #   refuses_newer_schema_version / seen_events_prune / message_index_roundtrip）
(cd core && cargo test -p aite-store --test crash_recovery) # 9 passed（7 + answering_task_recovered_as_orphan + awaiting_approval_not_recovered_as_orphan）
(cd core && cargo test -p aite-store --test concurrency)    # 13 passed（条数不变；:368 那一格解冻）
(cd core && cargo test -p aite --test startup_recovery --test crash_recovery --test sqlite_cross_process 2>&1 | grep -E '^\s+Running|^test result')
#   期望按文件名排序：crash_recovery 4 / sqlite_cross_process 2 / startup_recovery 9 passed，0 failed（红了先按 §6 单跑五遍）
(cd core && cargo test -p aite-contracts 2>&1 | grep -E '^test result')   # 合计与基线逐字相同（25 或 27）——变了说明碰到契约了
(cd core && cargo clippy -p aite-store --all-targets -- -D warnings)       # 0 warning
(cd core && rustfmt --edition 2024 --check crates/store/src/lib.rs crates/store/src/migrate.rs …)   # 列全你改过的每个 .rs（core 起的相对路径）；无输出、退出 0
#   必须在 core/ 下跑：云端 rustup 没有默认工具链，rustfmt 只在 core/rust-toolchain.toml 管得到的目录里解析得到
scripts/check.sh
#   期望末行「全部通过」、退出 0；各行：
#   cargo passed=<基线>+7 failed=0（情形 A：904；情形 B：908）—— Δ 逐条列（7 个新测试名 + 所在文件）；lifted pins：concurrency.rs:368
#   contracts passed=25 failed=0（情形 B：27）· OK 25 files · passed 10/10 · Go 全 ok（该格 8 行；cmd/aite-edge 单跑；
#   CC1 合并之后以 check.sh 多打的 `go packages ok=N fail=M` 那行为准）
(cd edge && gofmt -l . | wc -l)                              # 0（本轨不碰 Go，只是确认）
git diff --name-only origin/main...HEAD                      # 每一行都在 §3 可写面内
```

- `journal_mode=delete` / `busy_timeout 5000` 不变，由现有 `store/tests/crash_recovery.rs` 的 `journal_mode_is_delete_and_crash_leaves_no_residue`
  与 `session_lookup.rs` 的 `busy_timeout_is_five_seconds` / `every_new_store_instance_sets_busy_timeout` 钉着，必须仍绿。
- Go 模块文件有没有被改，由总管审 PR 时看（你的命令里别出现那两个文件名）。

## 8. 回执（写 `review/p1/ledger/CC5.md`，PR 描述贴摘要；两者都先 Write 成文件，PR 用 `--body-file`，见 §3、§6）

1. **开场自检原文**（4 项 + 附加项）：情形 A/B 与判定输出、守卫拦截原文、三个版本号、check.sh 各行原样、aite-store 基线条数。
2. **工作项逐条**：改了哪些文件:行；迁移清单（版本 → 做了什么）；新表 / 新列 / 新索引的最终 DDL；两个固有方法与 `prune_seen_events_before` 的最终签名；
   `LATEST_SCHEMA_VERSION` 的位置与「DD1 加迁移 3+ 时只追加清单、改这个常量」的做法；重复索引的语义选择；`message_index.created_at` 加没加、理由。
   **这一节是给 DD1 的接口说明**，写成它能照着委托的样子。
3. **新增测试逐条 + 变异验证输出**：7 个测试名、各钉什么、撤回哪一处、贴红的输出。
4. **`scripts/check.sh` 完整输出**（不截断）。
5. **`cargo passed` 增量逐条**：`+7` 按文件列测试名；lifted pins：`concurrency.rs:368`（Answering 移出「不算孤儿」列表）。
6. **被守卫拦过的命令与拦截原文**（没有就写「无」）。
7. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）。至少评估这几条：
   - 两个假 store 仍按活跃口径收孤儿，与真库口径分叉：`core/crates/testing/src/fake_store.rs:355-366`（`ACTIVE_TASK_STATUSES.contains`，CC7 的面）、
     `core/crates/control/tests/support/mod.rs:504-522`（`t.status.is_active()`，CC2 的面）；`core/crates/worker/tests/common/mod.rs:828`（CC3 的面）
     恒返回空、不收孤儿，不算分叉 → 建议 DD2 / DD1；
   - 总计划 §9 的解冻清单应补一条「Answering 不算孤儿（CC5 解冻，`concurrency.rs:368`）」——计划文件不在本轨可写面 → 建议总管；
   - 孤儿回帖文案 `run.rs:444` 对 answering 孤儿（答案可能已经发出去、只差落 Delivered）同样说「已终止。请重新发起。」——working 早就有同样的窗口 → 建议 DD3 判断（CC2 同波看不到本回执；注意 `run.rs` 在 W2a 归 T0c、不在 DD3 可写面，真要改文案由总管定归属）；
   - 其它你发现的。
8. **没做的与原因**。
9. **契约缺口**（给 T0 / T0.1；写清需要什么形状、为什么开放通道绕不过去；绕得过去就写「不是缺口」）：
   - contracts `ports.rs:156-157` 的 `recover_orphan_tasks` 文档还写「所有活跃态任务」，本轨之后实际是 created/planning/answering/working（不含 awaiting_approval）→ 建议总管在 H11 审 T0 补丁 / 设计文档时并入（T0 同波看不到本回执，T0 派单也没含这一条）；
   - 总计划 §5.2「会话」一条与 T0 原卡写的 `lib.rs:419-421` / `:481-483` 与实际行号（`98e4460` 上是 `:421-423` / `:482-484`；本轨之后 `:482-484` 那处已不存在）有漂移 → 同上，由总管在 H11 审 T0 设计文档时改成按名字引用；
   - 新 schema 错误只能走 `StoreError::Other`（contracts 无对应变体）：你认为够不够用。
