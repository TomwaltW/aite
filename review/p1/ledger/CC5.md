# CC5 回执：存储迁移机制 + P0 卫生 + Answering 孤儿恢复

分支 `claude/cc5-store-migrations`（本地），基于 main `30b00e5`（= `8458435` + 总管 D0 文档提交）。
提交：`47b61f0` ① / `f5039bd` ② / `64530c1` ③ / `89cad49` ④ / 本回执一个提交。

> **推送 / draft PR 没做成**：本会话的仓库**没有配置任何 git remote**（`git remote -v` 输出为空），
> 容器里也**没有 `gh`**（`command -v gh` → 不存在）。按规则不猜 remote URL，所以分支只在本地；
> PR 描述就用本回执（派单 §3 允许 `--body-file review/p1/ledger/CC5.md`）。
> 需要总管配好 remote / 换一个带 GitHub 接入的会话后 `git push -u origin claude/cc5-store-migrations`
> 再 `gh pr create --draft --title "CC5: 存储迁移机制 + Answering 孤儿恢复" --body-file review/p1/ledger/CC5.md`。见 §8。

## 1. 开场自检

**情形 A**。

1. 代码基线：`git cat-file -e 8458435` 成功（无需 unshallow）；
   `git diff --stat --no-renames --diff-filter=AM 8458435 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'` → **输出为空**。
2. 守卫：Read `.claude/hooks/guard_bash.py` 被拦（= 通过），原文：

   ```text
   PreToolUse:Read hook error: [d=$(git rev-parse --show-toplevel 2>/dev/null); [ -f "$d/.claude/hooks/guard_bash.py" ] || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"]: blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。
   ```
3. 工具链：`libprotoc 31.1`；`rustc 1.98.1 (48a229cea 2026-09-01)`；`go version go1.27.1 linux/amd64`；
   `protoc-gen-go v1.36.12`；`protoc-gen-go-grpc 1.6.2`。4 vCPU、uid 0。
4. `scripts/check.sh`（基线）各行：`OK 25 files`、`contracts passed=25 failed=0`、`cargo passed=946 failed=0`、
   `go packages ok=9 fail=0`、`passed 10/10`、`B9 skip：evals/p1 尚无场景`、末行「全部通过」、`exit=0`。与情形 A 逐字一致。
5. 本轨附加：
   - `cargo test -p aite-store` → `0 / 13 / 7 / 6 / 6 / 0` = **32 passed**，与期望一致。
     注意：以 root 直接跑时 crash_recovery 是 `6 passed; 1 failed`（`store_failure_surfaces_as_an_exception_not_a_silent_false`
     靠只读目录，root 拦不住），带 `setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search`
     前缀后 7/7 —— 即 CLAUDE.md 说的「单跑只读类测试要自己带 setpriv」，不是基线问题。
   - `cargo test -p aite --test startup_recovery --test crash_recovery --test sqlite_cross_process` → crash_recovery **4** /
     sqlite_cross_process **2** / startup_recovery **9**，0 failed。

## 2. 工作项逐条（给 DD1 的接口说明）

### 改了哪些文件

| 文件 | 改动 |
|---|---|
| `core/crates/store/src/migrate.rs`（新） | 迁移器全部：`LATEST_SCHEMA_VERSION`（:29）、`MIGRATIONS`（:38）、`m1_p0_schema`（:90）、`m2_query_columns_and_message_index`（:102）、`current_version`（:143）、`refuse_newer`（:162）、`run`（:172） |
| `core/crates/store/src/lib.rs` | 模块头补「迁移与孤儿恢复显式开事务」例外（:15-16）；`mod migrate` / `pub use migrate::LATEST_SCHEMA_VERSION`（:19-21）；删掉原 `SCHEMA` 常量（逐字搬进 migrate.rs 当迁移 1）；`ORPHAN_TASK_STATUSES`（:44）；`IndexedMessage`（:77）；固有方法 `prune_seen_events_before`（:158）/ `index_message`（:176）/ `find_session_by_message`（:212）；`init`（:311）跑迁移器；`create_task`（:462）/ `update_task`（:490）写时同步查询列；`seen_event`（:576）写 `seen_at`；`recover_orphan_tasks`（:597）绑 4 个状态 |
| `core/crates/store/tests/migration.rs`（新） | 5 条测试 |
| `core/crates/store/tests/crash_recovery.rs` | +2 条测试与造数函数 `crash_with_a_task_stuck_at` |
| `core/crates/store/tests/concurrency.rs` | 解冻：删 `TaskStatus::Answering` 一格（原 :368） |
| `core/crates/app/tests/startup_recovery.rs` | 两处注释（`a_broken_store_query_does_not_block_takeoff` 与 `break_tasks_table`）改成新 `init()` 口径；代码不变 |

`store/Cargo.toml`、`Cargo.lock` 一行没动；没有新依赖。`app/tests/crash_recovery.rs` 没改（:166-168「Answering 不在活跃集」仍然成立）。

### 版本表与迁移器

- 表：`CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)`，一次迁移一行，
  `applied_at` 用 `stamp()`。当前版本 = `MAX(version)`；表不存在（先查 `sqlite_master`）或为空 = 0。其它错误经 `sq()` 原样上抛。
  这张表由 `run()` 在迁移事务里建，不属于任何一条迁移。
- 注释写明：说的是本 crate 的表，**不是** `PRAGMA user_version`（不用），也**不是** `PRAGMA schema_version`（SQLite 内置 cookie，
  preflight 拿它当只读探针，绝不写）。
- `init()` = `self.with_conn(migrate::run)`，流程：
  1. 普通 `SELECT` 读版本（不开写事务）；
  2. `> LATEST` → `StoreError::Other("库的 schema 版本 {v} 比本程序支持的 {LATEST} 新，拒绝打开")`，任何写之前；
  3. `== LATEST` → 返回，零写入；
  4. `< LATEST` → `Transaction::new_unchecked(c, TransactionBehavior::Immediate)` → 事务内重读版本（并再判一次 `> LATEST`）→
     建 `schema_version` → 逐条跑 `version > v` 的迁移、每条后插一行版本 → `commit`。任一 `?` 提前返回即 drop 事务 = 回滚。
- `init()` 仍不收孤儿；`open()` 的 `busy_timeout` 不动；不设任何 journal pragma。

### 迁移清单

| 版本 | 做了什么 |
|---|---|
| 1 | 原 `SCHEMA` 逐字（全是 `IF NOT EXISTS`，P0 老库上空操作） |
| 2 | 见下面 DDL |

**DD1 加迁移 3+ 的做法**：在 `migrate.rs` 的 `MIGRATIONS` 末尾追加 `Migration { version: 3, apply: m3_… }`，
并把 `LATEST_SCHEMA_VERSION` 改成 3。迁移函数签名 `fn(&Connection) -> Result<(), StoreError>`，在调用方的事务里跑、不自己 commit；
需要时间戳用 `crate::stamp(Utc::now())`，错误用 `crate::sq`。已发布的迁移 1 / 2 一个字不许改。
`tests/migration.rs` 的断言写的是 `1..=LATEST_SCHEMA_VERSION`，不会因为加迁移而红。

### 迁移 2 的最终 DDL

```sql
ALTER TABLE seen_events ADD COLUMN seen_at TEXT;
UPDATE seen_events SET seen_at = ?1;               -- ?1 = stamp(迁移时刻)，存量不留 NULL
CREATE INDEX IF NOT EXISTS idx_seen_events_seen_at ON seen_events (seen_at);

ALTER TABLE tasks ADD COLUMN chat_id TEXT;
ALTER TABLE tasks ADD COLUMN cost REAL NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN tokens_in INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN tokens_out INTEGER NOT NULL DEFAULT 0;
UPDATE tasks SET
    chat_id    = (SELECT s.chat_id FROM sessions s WHERE s.id = tasks.session_id),
    cost       = CASE WHEN json_valid(data) THEN COALESCE(json_extract(data, '$.cost'), 0) ELSE 0 END,
    tokens_in  = CASE WHEN json_valid(data) THEN COALESCE(json_extract(data, '$.tokens_in'), 0) ELSE 0 END,
    tokens_out = CASE WHEN json_valid(data) THEN COALESCE(json_extract(data, '$.tokens_out'), 0) ELSE 0 END;
CREATE INDEX IF NOT EXISTS idx_tasks_chat_created ON tasks (chat_id, created_at);

CREATE TABLE IF NOT EXISTS message_index (
    chat_id     TEXT NOT NULL,
    message_id  TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    task_id     TEXT,
    outbound    INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (chat_id, message_id)
);
```

- `json_valid` 守卫是多加的：`data` 万一不是合法 JSON，回填给 0 而不是让整条迁移（= 起飞）失败；这种行本来 `get_task` 也读不出来。
- `chat_id` 按派单改成 sessions 子查询回填；会话行没了留 NULL。
- 写时同步：`create_task` 插入时写 `chat_id = (SELECT chat_id FROM sessions WHERE id = ?2)`、`cost`、`tokens_in/out`（`u64 as i64`）；
  `update_task` 每次都重写这四列（chat_id 用同样的子查询按新的 `session_id` 取）。`recover_orphan_tasks` 的 UPDATE 只改状态和 data（cost 不变，无需同步）。
  `list_active_tasks` 的 JOIN、`get_task` 等读路径未改。

### 固有方法最终签名（`impl SqliteSessionStore`）

```rust
pub async fn prune_seen_events_before(&self, cutoff: DateTime<Utc>) -> Result<u64, StoreError>;
// DELETE FROM seen_events WHERE seen_at < stamp(cutoff)，返回删掉的行数

pub async fn index_message(
    &self,
    chat_id: &str,
    message_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    outbound: bool,
) -> Result<(), StoreError>;

pub async fn find_session_by_message(
    &self,
    chat_id: &str,
    message_id: &str,
) -> Result<Option<IndexedMessage>, StoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedMessage {
    pub chat_id: String,
    pub message_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub outbound: bool,
    pub created_at: DateTime<Utc>,
}
```

- `aite_store::LATEST_SCHEMA_VERSION: i64`（`pub use`，定义在 `migrate.rs:29`）、`aite_store::IndexedMessage` 均从 crate 根导出。
- DD1 委托：trait 默认方法 `prune_seen_events(cutoff)` → `self.prune_seen_events_before(cutoff)`；`index_message(...)` / `find_session_by_message(...)`
  同名一行委托（固有方法优先，不递归）。若 T0 的 trait 返回类型是 contracts 里的新类型，DD1 在委托处把 `IndexedMessage` 映射过去。
- **重复索引语义：保留首条**（`ON CONFLICT (chat_id, message_id) DO NOTHING`，不报错、不覆盖）。理由：一条消息属于第一次认领它的会话，
  平台重推 / 重连回放不许把它挪走。`message_index_roundtrip` 钉着。
- **`message_index.created_at` 加了**（超出原卡的一列）：以后按群清除 / 留存要按时间剪；值由 store 用 `stamp(Utc::now())` 盖，调用方不传。

## 3. 新增测试与变异验证

| 测试 | 文件 | 钉什么 |
|---|---|---|
| `migrates_p0_db_in_place` | `store/tests/migration.rs` | 用抄下来的 `P0_SCHEMA` 裸建老库（会话 / 花过钱的任务 / 两轮 turn / 计数器 / 两条去重键）→ `init` 后版本行 = `1..=LATEST`、数据原样读回、老去重键仍 true、号接着发（`#A2`）、`journal_mode=delete`、回填值 = JSON / 会话 chat_id、存量 `seen_at` 非 NULL 且落在迁移时刻区间、新表 / 新索引在、`update_task` / `create_task` 写时同步 |
| `migration_idempotent` | 同上 | 同一实例第二次 `init`、同文件第二实例 `init` 都 Ok、文件字节不变、版本行不变、数据不变；**另一连接持 RESERVED 写锁（`BEGIN IMMEDIATE`）时第二实例 `init` 必须 2s 内 Ok**（钉住「最新版本不抢写锁」的快路径 —— 单比字节钉不住：空事务 commit 不改文件） |
| `refuses_newer_schema_version` | 同上 | 插 `LATEST+1` 行后 `init` → `Err(StoreError::Other)`，消息含两个版本号，文件字节不变 |
| `seen_events_prune` | 同上 | `seen_event` 写非 NULL 的 `seen_at`；旧批 3 条被剪、再剪为 0；旧 id 重新 `seen_event` → false（再一次 → true），新 id 仍 true |
| `message_index_roundtrip` | 同上 | 入站（无 task_id）/ 出站（有 task_id）字段逐一读回、`created_at` 在写入区间；不存在键 → None；跨 chat 同 message_id 互不串；重复索引保留首条 |
| `answering_task_recovered_as_orphan` | `store/tests/crash_recovery.rs` | 停在 answering → 崩溃（只关 fd）→ 重开收成 failed、`ORPHAN_RESULT_SUMMARY`、`task_no` 带得出来；换实例重读已落盘；再收一次为空 |
| `awaiting_approval_not_recovered_as_orphan` | 同上 | 停在 awaiting_approval → 不在返回值里，状态与 `result_summary` 原封不动 |

变异验证做法：提交后用脚本（锚点恰好命中一次）改工作区一处 → 跑对应测试 → `git checkout -- core/crates/store/src` 还原（新 mtime）→ 全量 store 测试回绿（`0/13/9/5/6/6/0`）。

- **M1 快路径撤掉**（删 `if v == LATEST_SCHEMA_VERSION { return Ok(()); }`）→ `migration_idempotent` 红（撞写锁等满 busy_timeout）：
  ```text
  test migration_idempotent ... FAILED
  thread 'migration_idempotent' (18972) panicked at crates/store/tests/migration.rs:223:10:
  test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.26s
  ```
- **M2 拒绝更新撤掉**（`refuse_newer` 的判据改成 `v > i64::MAX - 1`）→ `refuses_newer_schema_version` 红：
  ```text
  test refuses_newer_schema_version ... FAILED
  thread 'refuses_newer_schema_version' (19280) panicked at crates/store/tests/migration.rs:262:34:
  库比程序新，必须拒绝: ()
  ```
- **M3 不写版本行** → `migrates_p0_db_in_place` 与 `migration_idempotent` 红：
  ```text
  thread 'migration_idempotent' (19587) panicked at crates/store/tests/migration.rs:205:5:
    left: []
  thread 'migrates_p0_db_in_place' (19586) panicked at crates/store/tests/migration.rs:165:5:
    left: []
  test result: FAILED. 1 passed; 2 failed; ...
  ```
- **M4 `seen_event` 不写 seen_at**（`VALUES (?1, NULLIF(?2, ?2))`）→ `seen_events_prune` 红：
  ```text
  thread 'seen_events_prune' (21753) panicked at crates/store/tests/migration.rs:350:5:
  assertion `left == right` failed: seen_event 必须写 seen_at
    left: 5
  ```
- **M5 迁移 2 不回填存量 seen_at**（`UPDATE … WHERE 0`）→ `migrates_p0_db_in_place` 红：
  ```text
  thread 'migrates_p0_db_in_place' (22075) panicked at crates/store/tests/migration.rs:222:30:
  存量行 seen_at 不许是 NULL
  ```
- **M6 `update_task` 不同步查询列** → `migrates_p0_db_in_place` 红：
  ```text
  thread 'migrates_p0_db_in_place' (22400) panicked at crates/store/tests/migration.rs:244:5:
    left: (Some("oc_chat"), 0.375, 1200, 345)
  ```
- **M7 重复索引改覆盖**（`DO UPDATE SET session_id = excluded.session_id`）→ `message_index_roundtrip` 红：
  ```text
  thread 'message_index_roundtrip' (22723) panicked at crates/store/tests/migration.rs:462:5:
  assertion `left == right` failed: 重复索引要保留首条
    left: Some(IndexedMessage { chat_id: "oc_chat", message_id: "om_in", session_id: "ses_X", task_id: None, outbound: false, created_at: 2026-09-28T01:41:59.589494Z })
  ```
- **M8 剪枝方向反**（`seen_at > ?1`）→ `seen_events_prune` 红：
  ```text
  thread 'seen_events_prune' (23056) panicked at crates/store/tests/migration.rs:359:5:
    left: 2
  ```
- **M9 answering 不算孤儿**（`ORPHAN_TASK_STATUSES` 的 `Answering` 换回 `Created`）→ `answering_task_recovered_as_orphan` 红：
  ```text
  test answering_task_recovered_as_orphan ... FAILED
  thread 'answering_task_recovered_as_orphan' (26151) panicked at crates/store/tests/crash_recovery.rs:96:5:
    left: []
  test result: FAILED. 8 passed; 1 failed; ...
  ```
- **M10 awaiting_approval 也当孤儿**（SQL 改 `IN (?1, ?2, ?3, ?4, 'awaiting_approval')`）→ `awaiting_approval_not_recovered_as_orphan` 红：
  ```text
  test awaiting_approval_not_recovered_as_orphan ... FAILED
  thread 'awaiting_approval_not_recovered_as_orphan' (29991) panicked at crates/store/tests/crash_recovery.rs:122:5:
  test result: FAILED. 8 passed; 1 failed; ...
  ```
  （第一次试 M10 时误把 `Planning` 换成了 `AwaitingApproval`，红的是 `concurrency.rs` 的 `recover_orphan_tasks_clears_the_active_three`——
  那是变异写错了，不算；改成上面的 SQL 形状重做。）

还原后 `cargo test -p aite-store`（setpriv）：`0 / 13 / 9 / 5 / 6 / 6 / 0` = 39 passed。

## 4. `scripts/check.sh` 完整输出（`89cad49`）

```text
=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
   Compiling aite-store v0.0.1 (/home/user/repo/core/crates/store)
   Compiling aite v0.0.1 (/home/user/repo/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.19s
-> exit 0

=== A2 go build ./... ===
$ bash -c cd edge && go build ./...

-> exit 0

=== A3/C2 契约锁 --check ===
$ core/target/debug/aite contracts lock --check
OK 25 files
-> exit 0

=== A4a cargo clippy -D warnings ===
$ bash -c cd core && cargo clippy --workspace --all-targets -- -D warnings
    Checking aite-store v0.0.1 (/home/user/repo/core/crates/store)
    Checking aite v0.0.1 (/home/user/repo/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.57s
-> exit 0

=== A4b cargo fmt --check ===
$ bash -c cd core && cargo fmt --check

-> exit 0

=== A4c go vet ===
$ bash -c cd edge && go vet ./...

-> exit 0

=== A4d gofmt ===
$ bash -c cd edge && test -z "$(gofmt -l .)"

-> exit 0

=== A5 cargo test --no-run（全部测试可编译） ===
$ bash -c cd core && cargo test --workspace --no-run
  Executable tests/test_context.rs (target/debug/deps/test_context-0c9eaf634dbf20e3)
  Executable tests/test_final.rs (target/debug/deps/test_final-6e70a09cb9f97835)
  Executable tests/test_in_flight.rs (target/debug/deps/test_in_flight-9cf74d9da741dd8b)
  Executable tests/test_limits.rs (target/debug/deps/test_limits-37cdb5e9a29c39a5)
  Executable tests/test_loop_fallbacks.rs (target/debug/deps/test_loop_fallbacks-4493b78700529fd8)
  Executable tests/test_prompts_checklist.rs (target/debug/deps/test_prompts_checklist-e9d7c93d76b8d9d7)
  Executable tests/test_sandbox_handoff.rs (target/debug/deps/test_sandbox_handoff-0384a5fa7bf2bfc7)
  Executable tests/test_steer.rs (target/debug/deps/test_steer-0e71edf6a13bb207)
-> exit 0

=== C1 契约测试 ===
$ bash -c cd core && cargo test -p aite-contracts 2>&1 | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"contracts passed=\" p \" failed=\" f; exit (f>0)}"
contracts passed=25 failed=0
-> exit 0

=== B 全量 cargo test ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^(---- .* stdout ----|error(: test failed|: could not compile|\[E[0-9]+\]))" | sort -u | head -n 7; printf "%s\n" "$o" | grep -E "^test result" | awk -v c="$c" "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p+0 \" failed=\" f+0; exit (c != 0 || f > 0 || NR == 0)}"
cargo passed=953 failed=0
-> exit 0

=== B 全量 go test（-race） ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd edge && o=$(go test -race ./... -count=1 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^FAIL[[:space:]]+aite/edge/" | head -n 5; ok=$(printf "%s\n" "$o" | grep -cE "^(ok|\?)[[:space:]]"); fail=$(printf "%s\n" "$o" | grep -cE "^FAIL[[:space:]]+aite/edge/"); echo "go packages ok=${ok} fail=${fail}"; exit "$c"
go packages ok=9 fail=0
-> exit 0

=== B8 评测（passed 10/10） ===
$ bash -c o=$(core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | grep -qx "passed 10/10"
}
passed 10/10
-> exit 0

=== B9 评测 evals/p1 ===
B9 skip：evals/p1 尚无场景

全部通过
exit=0
```

其余 §7 验收：`cargo test -p aite-store` → `0/13/9/5/6/6/0` = 39；`-p aite --test startup_recovery --test crash_recovery --test sqlite_cross_process`
→ 4 / 2 / 9，0 failed（改完后跑过；另跑了 `preflight_e2e` 30 passed、`signals` 6 passed，`init()` 的 `not a database` / `readonly` 原文与
外部 EXCLUSIVE 锁下排队两条都绿）；`cargo test -p aite-contracts` 合计 25（不变）；`cargo clippy -p aite-store --all-targets -- -D warnings` 0 warning；
`rustfmt --edition 2024 --check`（6 个改过的 .rs）无输出、退出 0；`gofmt -l . | wc -l` → 0。
`git diff --name-only 8458435...HEAD` 除本轨 6 个代码文件 + 本回执外，还列出 `CLAUDE.md` 与 `review/paste-*.md` —— 那是总管的 D0 文档提交
`30b00e5`（在 `8458435` 之后、本分支的起点），不是本轨改的；`git diff --name-only 30b00e5...HEAD` 只有可写面内的文件。

**派单 §7 的算术**：「情形 A：904」是刷新前 `897 + 7` 的旧数；刷新后情形 A 的期望是 `946 + 7 = 953`，实测 953。

## 5. `cargo passed` 增量（946 → 953，+7）

- `core/crates/store/tests/migration.rs`（新）+5：`migrates_p0_db_in_place`、`migration_idempotent`、`refuses_newer_schema_version`、
  `seen_events_prune`、`message_index_roundtrip`
- `core/crates/store/tests/crash_recovery.rs` +2：`answering_task_recovered_as_orphan`、`awaiting_approval_not_recovered_as_orphan`
- lifted pins：`core/crates/store/tests/concurrency.rs:368`（`recover_orphan_tasks_leaves_finished_tasks_alone` 的列表里去掉
  `TaskStatus::Answering`；测试本身保留，条数不变）

## 6. 被守卫拦过的命令

除开场自检第 2 步那次（预期被拦，原文见 §1）外：无。

## 7. 记账转出去的

| 事项 | 为什么不在本轨 | 建议归哪轨 |
|---|---|---|
| 假 store 仍按活跃口径收孤儿，与真库分叉：`core/crates/testing/src/fake_store.rs:365`（`ACTIVE_TASK_STATUSES.contains`）、`core/crates/control/tests/support/mod.rs:519`（`t.status.is_active()`）。`worker/tests/common/mod.rs` 的那个恒返回空、不算分叉 | 分别是 CC7 / CC2 的面 | DD2 / DD1（改成 created/planning/answering/working，不含 awaiting_approval） |
| 总计划 §9 解冻清单补一条「Answering 不算孤儿（CC5 解冻，`store/tests/concurrency.rs:368`）」 | 计划文件不在可写面 | 总管 |
| 孤儿回帖文案 `app/src/run.rs:444`（`任务 {task_no}：{ORPHAN_RESULT_SUMMARY}`）对 answering 孤儿（答案可能已经发出去、只差落 delivered）同样说「已终止。请重新发起。」；working 早就有同样的窗口 | `run.rs` 与文案不在可写面 | DD3 判断；`run.rs` 在 W2a 归 T0c，改文案由总管定归属 |
| 只读的 **P0 老库**（版本 0、文件或卷只读）上 `init()` 现在会失败（要写 `schema_version` 与迁移 2），以前 `CREATE TABLE IF NOT EXISTS` 空操作能过；已迁到最新的只读库上 `init()` 仍然成功（`preflight_e2e.rs:1174-1180` 记着的那条既有行为不变）。没有测试钉「版本 0 的只读库」这一格 | 行为后果，不是缺陷：老库总要写一次才能升级；preflight 第 ① 组本来就把「文件写不动」判红 | 总管知悉；需要的话 DD1 在 preflight 文案里点一句「首次升级要写库」 |
| `aite contracts` / 运维侧没有「查库 schema 版本」的入口（排障只能 `sqlite3 … 'SELECT * FROM schema_version'`） | 超出本卡 | DD1 / 运维文档 |

## 8. 没做的与原因

- **没推送、没开 draft PR**：本会话仓库 `git remote -v` 为空、容器无 `gh`。分支 `claude/cc5-store-migrations` 与全部提交只在本地容器里；
  PR 描述用本回执。**容器回收后本地提交会丢**——需要总管在有 remote 的会话里拉这个分支，或让我在配好 remote 后推送。
- 没新增 app 层测试（派单说不新增；`run.rs` 对 answering 孤儿不改就会回帖，现有 `startup_recovery` 的 working 孤儿用例覆盖同一路径）。
- 没做 `create_task` 写时同步的单独变异（M6 只撤了 `update_task` 那一处）；`migrates_p0_db_in_place` 末尾对 `tsk_2` 的断言钉着它。

## 9. 契约缺口

- contracts `ports.rs:156-157` 的 `recover_orphan_tasks` 文档仍写「所有活跃态任务」，本轨之后实际是 created/planning/answering/working
  （不含 awaiting_approval）→ 建议总管在 H11 审 T0 补丁 / 设计文档时并入。
- 总计划 §5.2「会话」一条与 T0 原卡引用的 `lib.rs:419-421` / `:481-483` 已漂移（本轨之后 `list_active_tasks` 的绑定在 `lib.rs` 的
  `list_active_tasks` 函数里，`recover_orphan_tasks` 已不再引用 `ACTIVE_TASK_STATUSES`）→ 建议改成按函数名 / 常量名引用。
- 新 schema 错误走 `StoreError::Other`：**够用，不是缺口**。「库比程序新」是起飞时一次性的致命错误，调用方（`run.rs` 的起飞）只需要把原话打出来退出，
  不需要按变体分支；消息里带两个版本号，排障信息完整。
- `IndexedMessage` 定义在 aite-store：T0 若在 contracts 里给 `find_session_by_message` 定返回类型，DD1 委托时做一次字段映射即可（字段一一对应），不是缺口。

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01J5HNw6zW8zP6CxWXDXLQQe
