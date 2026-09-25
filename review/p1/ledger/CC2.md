# CC2 回执：控制面零行为拆分 + 预埋桩 + 可选并发派发 + 健壮性 + 命令注册表

- 分支：`claude/intelligent-curie-nmk7nb` · PR：[TomwaltW/aite#3](https://github.com/TomwaltW/aite/pull/3)
- 基线：`d13ec57`（main，= `98e4460` + D0 文档提交 + CC1 合并）
- **CC3 依赖本轨 ⑩；H10 保持 CC2 先于 CC3 合并（plan H10 的顺序 CC1 → CC2–CC7，别调换）。**

## 0. 结论一句话

①–⑩ 全部做完，每项配回归测试 + 变异验证（撤回 → 红）；④ 的卡死替换在默认 N=1 下成立；CC1 记账转来的 R7 竞态一并修掉
（连跑 40 次 0 红，改前 8/40）。**越出可写面一处**：`core/crates/app/tests/common/mod.rs`（③ 让它必红，总管在会话里批准改，见 §5）。
`check.sh` 全部通过，`cargo passed=924 failed=0`（897 + 27）。

## 1. 开场自检

1. **代码基线**：`git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'`
   列出的恰好是**已合并的 CC1 那 19 个文件**（`scripts/`、`.github/`、五个骨架 crate、`core/Cargo.toml` 等），没有任何 P0-CLOSE 路径。
   判定：**情形 A + CC1**（P0-CLOSE 未落地；CC1 合并后 check.sh 多打 `go packages ok=N fail=M`、B9 那一行）。
2. **守卫**：Read `.claude/hooks/guard_bash.py` 被拦，原文：
   `blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。` —— 这一步被拦即通过。
3. **工具链**：`libprotoc 31.1`；`rustc 1.98.1 (48a229cea 2026-09-01)`；`go version go1.27.1 linux/amd64`。
4. **`scripts/check.sh`**（clean HEAD，冷编后第二遍）关键行：

```
OK 25 files
contracts passed=25 failed=0
cargo passed=897 failed=0
go packages ok=9 fail=0
passed 10/10
B9 skip：evals/p1 尚无场景
全部通过
real	1m19.247s
```

5. **拆分前逐二进制条数** `/tmp/cc2-counts-B0.txt`（与派单静态数出来的 116 逐项一致）：

```
     Running unittests src/lib.rs (target/debug/deps/aite_control)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/cancel.rs (target/debug/deps/cancel)
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/commands.rs (target/debug/deps/commands)
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/dispatch.rs (target/debug/deps/dispatch)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/drop_logs.rs (target/debug/deps/drop_logs)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/ingress.rs (target/debug/deps/ingress)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/ingress_failures.rs (target/debug/deps/ingress_failures)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/routing.rs (target/debug/deps/routing)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/steer_evidence.rs (target/debug/deps/steer_evidence)
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/steer_routing.rs (target/debug/deps/steer_routing)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/wording.rs (target/debug/deps/wording)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## 2. 工作项逐条

### ① 零行为变化拆分 + 预埋桩（`2e84409`）

搬家按派单 §5 ① 的映射表，一段不差（脚本按 `plane.rs` 行号区间切，断言每一行非空行恰好搬一次）：

| 去处 | 内容 |
|---|---|
| `plane.rs`（287 行） | `ControlDeps`、`PlaneState`、`Shared`（锁序注释原样）、结构体、`new` / `with_*` / `state` / `now`、`ControlPlane` impl（`cancel_task` 转调 `dispatch.rs`）、`WallClock` / `SleepFn` |
| `routing/mod.rs` | `route` R1–R8、`R4_KINDS`、`drop_log`、R3 `on_card_action`、R4 `on_non_message`、R6 `continue_session`、`Flow` |
| `sessions.rs` | `token_hex_16`、`initiator_of`、`new_session`、`start_task`、`thread_session`、`append_turn`、`reply`（+ `token_hex_16` 的单测） |
| `dispatch.rs` | 三个 guard、`notify_cancelled`、`dispatch_loop` / `run_one` / `run_task` / `dispatch_task`、`finalize_evidence`、`release_gateway_sandbox`、`steer_target`（+ 3 条单测）、`cancel_task` 的函数体 |
| `reaper.rs` | `REAPER_INTERVAL_SEC`、`reaper_loop` |
| `evidence_log.rs` | `ROUTE_NEW_TASK` / `ROUTE_STEER`、`event_payload` / `steer_payload` |
| `commands/mod.rs` | 原 `commands.rs` 全部 + 注册表分发（20 条）+ `on_command` + `StopTarget` + `status_tasks` |
| `commands/{status,stop,restart,new}.rs` | `cmd_status`+`dropped_note`；`cmd_stop`+`resolve_stop_target`/`resolve_task`+文案；`cmd_restart`+文案；`cmd_new` |

对外 API 不变：`lib.rs` 的全部 `pub use` 照旧可用，`aite_control::commands::{parse_command, normalize_task_no, UNKNOWN_COMMAND_TEXT}` 路径照旧，
`ControlDeps` 字段与 `new` 签名不动。单测跟着被测函数走，总数仍 13。

**桩清单**（形状：`async fn …(&InProcessControlPlane, &NormalizedEvent, …) -> Result<Flow, IngressError>`，以
`if let Flow::Handled = x::hook(…).await? { return Ok(()); }` 接入；不做 I/O、不计数、不打日志、没有 `_ =>`）：

| 文件 | 链上位置（`routing/mod.rs`） | 主人 |
|---|---|---|
| `gate.rs` `hook` | R2 之后、R3 之前（`:136`） | EE8 |
| `routing/card_actions.rs` `hook(…, &CardAction)` | R3 里、Stop / Evidence 之前（`:222`） | EE7（T0c 先加 log+drop） |
| `routing/external.rs` `hook` | R3 之后、`R4_KINDS` 判定之前，所有非卡片事件（`:146`） | EE11 |
| `mute.rs` `pre_r4` / `pre_r5` | 同上（`:149`）/ R5 之前（`:166`） | EE9 |
| `routing/edits.rs` `hook` | R4 的 `MessageEdited` 分支开头（`:271`） | EE13 |
| `intro.rs` `hook` | R4 的 `BotAdded` 分支开头（`:284`） | EE9 |
| `routing/resolver.rs` `resolve -> Option<Session>` | `thread_session` 没命中时（`:162`） | DD3 |
| `dm.rs` `hook` | R6 之前，只对 p2p（`:177`） | FF4 |
| `sessions_channel.rs` / `ambient.rs` `hook` | R7 与 R8 之间（`:201` / `:204`） | FF1 |
| `internal.rs` `submit_internal`、`approvals.rs` `decide` | 不进链，`#[allow(dead_code)] // 主人：DD3 / EE7` | DD3 / EE7 |
| `commands/<15 个>.rs` | 注册表，`ENABLED = false` | about/access/configure → EE8；mute/unmute/feedback/fork/connect → EE9；routines → EE2；memory → EE1；approve/reject → EE7；evidence → EE12；usage → EE3；model → FF4 |
| `commands/help.rs` | ① 停用桩（`ENABLED = false`，回未知命令）→ ⑤ 启用 | EE8（W3 起只补条目） |

`Flow::Handled` 今天没有桩构造，标了 `#[allow(dead_code)]` 注明主人。验证：逐二进制条数 diff 为空（§1 第 5 项 vs 下面），
clippy `-D warnings` / `cargo fmt --check` 绿，check.sh 全部通过（897/0），**没有改任何测试断言，app 测试一条没变**。

```
     Running unittests src/lib.rs (target/debug/deps/aite_control)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/cancel.rs (target/debug/deps/cancel)
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/commands.rs (target/debug/deps/commands)
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/dispatch.rs (target/debug/deps/dispatch)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/drop_logs.rs (target/debug/deps/drop_logs)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/ingress.rs (target/debug/deps/ingress)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/ingress_failures.rs (target/debug/deps/ingress_failures)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/routing.rs (target/debug/deps/routing)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/steer_evidence.rs (target/debug/deps/steer_evidence)
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/steer_routing.rs (target/debug/deps/steer_routing)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/wording.rs (target/debug/deps/wording)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### ② `with_max_parallel(n)`，默认 1（`e98f559`）

- `plane.rs:193` `with_max_parallel(n)`（`0` 当 `1`）；`run_forever` 在 `max_parallel <= 1` 时走**原 `dispatch_loop` 一字不改**，否则走
  `dispatch.rs:118` `dispatch_loop_parallel`：在飞任务是 `run_forever` 自己用 `poll_fn` 轮询的 `Pin<Box<dyn Future + Send + '_>>`，**不 spawn**，
  abort 时一起被丢、三个 guard 照原 drop 语义收尾。`run_pending` 仍串行。
- `queue.rs`：项带会话 key（`put_keyed`），`try_pop_where` 跳过忙会话、其余原序留在队里 —— 无队头阻塞，`pending()` / `state().queued` 口径不变。
- **run.rs 未改**。

### ③ 入口把存储 / 证据错误传出去（`a4a5aae`）

- `ingress.rs` `handler()` 对 `IngressError::Store` / `::Evidence` 返回 `Err`，其余 `Ok`；`on_event` 仍返回 `()`；模块注释改写（含「`seen_event`
  之后的失败重推仍会被 R2 吃掉」的边界）；`plane.rs` `handle_event` 里那段「到不了」的注释同步改正。
- **越出可写面**：`app/tests/common/mod.rs` 的 `GatedPlatform::emit` 对 `Err` 是 `expect("emit")`，③ 让
  `crash_recovery::store_failure_does_not_kill_the_process` 必红（只读盘那条事件现在如实返回 `Err`）。派单要求「停下报告」；我停下问了总管，
  总管选「扩范围、改 emit」。改法：`Err` 的文本以 `store: ` / `evidence: ` 开头时放行，其余照旧当夹具出错（断言）。改后 `crash_recovery` 4/4 绿。
  evals 的 `emit` 没有撞上（evals 全绿）。

### ④ `!restart` 回灌话题历史（`3724c9d`）；卡死任务下条消息替换（`69ef14c`）

- 回灌：`sessions.rs:128` `thread_history`：`supports_history` 为假不读；`read_history(chat_id, 50, Some(thread_id))`，读失败只 warn；
  正序，人 → `User`（带 `platform_user_id`），其余 → `SystemNote` 且内容前缀 `[发言人] `；排除 `!restart` 本条与空文本。
  只在「原来有会话」时回灌（顶层 @ 着发的 `!restart` 没有旧话题可读）。⑩ 起历史在建会话**之前**读好，与建会话、`rest` 那轮一起写进同一段临界区。
- 卡死：`plane.rs:32` `STUCK_AFTER_SEC = 15 * 60`（按 CC6 的 600s 模型超时上界取），`with_stuck_after_sec` 注入。判据
  `routing/mod.rs:366` `is_stuck`：在本进程 `running` 里、且 `now − updated_at > 阈值`。落到该会话的下一条消息（R6，`continue_session`）：
  `abandon_running`（`dispatch.rs:246`：查 `Shared.abandon` 表 → `notify_one` → 等它出 `running`，上限 5s）→ `cancel_task_by`（此时走「不在跑」分支：
  cancelled + finalize + 收卡片 + 还沙箱）→ 回 `stuck_task_replaced_text`（「任务 #A.. 超过 15 分钟没有进展，已停止，按这条消息重新开始。」，测试逐字钉）→
  `start_task`。`dispatch_task` 里 `worker.run(...)` 换成对该任务 `Notify` 的 `tokio::select!`（`dispatch.rs:233`）；`AbandonGuard` 声明在
  `RunningGuard` 之前，所以晚于它 drop；`abandon` 这把锁从不与别的锁同持（`Shared` 注释里记了一笔）。
- **默认 N=1 下成立**：测试 `stuck_task_is_replaced_on_next_message_with_notice` 不调 `with_max_parallel`，worker 永远不返回、不看取消标志。
- **select! 丢掉 worker future 时的一致性**：证据侧一致 —— `evidence/src/writer.rs` 的 append/finalize 在 `spawn_blocking` 内部取同步锁串行，
  被丢的 worker 若正卡在一次 append 上，那次 append 仍整条写完，我们的 `cancelled` 排在它之后，链的哈希照样连得上。
  **库侧有残余窗口**：store 的 `update_task` 同样是 `spawn_blocking`、不可取消；worker 被丢时若恰好有一笔 `update_task` 在飞，它可能落在我们的
  `Cancelled` 之后，把状态改回 `working`。卡死的定义是「很久没落库」，这个窗口极窄，但不是零；要堵得让 worker 配合或给 store 一个比较并交换的写法（见 §7）。
  worker 的 `in_flight` 条目会残留（`run.rs` 注释说明过），收尾时 `cancel_task` 按 id 重读会跳过终态。

### ⑤ 解析、注册表、`!help`、中文别名、未知命令文案（`b99f187`）

- `commands/mod.rs:110` `parse_command`：`！` 等价 `!`；按第一个 `char::is_whitespace` 切；`ALIASES`（状态/停止/重开/新话题/帮助）只在带 `!`/`！` 时生效；
  计数器 key 用规范名。`:100` `is_command` 供 R5（`routing/mod.rs:171`）用。
- `UNKNOWN_COMMAND_TEXT` → `未知命令，发 !help 看全部命令`；`KNOWN_COMMANDS` 删掉，由 `registry()`（`:65`，20 条，各文件自带 `ENABLED` + `HELP`）取代。
- `help.rs`：① 里是停用桩（`ENABLED = false`，`run` 回未知命令）；⑤ 里实现并 `ENABLED = true`，只列 `ENABLED` 的。`!help` 回帖原文：

```
可用命令：
!status　列出本群的活跃任务
!stop <任务号>　停止一个任务（本群只有一个活跃任务时可省略任务号）
!restart [要做的事]　重开当前会话
!new [要做的事]　另起一个新话题
!help　看这份命令列表
命令开头的 ! 也可以打全角的 ！；中文也行：！状态 ！停止 ！重开 ！新话题 ！帮助
```

### ⑥ R6 看 `supports_thread`；p2p 进 DM 桩（`3d71a22`）

`routing/mod.rs:182-188`：`chat_type != P2p && platform.capabilities().supports_thread && 有会话`。p2p 先过 `dm::hook`（关着 = `Pass`），不进 R6，
净行为不变。support 的假平台加 `set_capabilities`（默认仍 `feishu_p0()`）。B8 没动。

### ⑦ R6 追问补 ack（`3d71a22`）

`continue_session` 在追问落进 transcript、`update_session` 之后 `self.ack(ev)`（`routing/mod.rs:307`；`sessions.rs:342` 抽出的 `ack`，`new_session` 复用），
steer / 新建 / 卡死替换三条路都有；失败只 warn。bot 在 R1 就被丢。`reconnect_replay` / `cold_start_to_delivery` 数 reactions 的那几条没变红。

### ⑧ `!new` 之后同话题回复的路由（`7757b0f`）

`commands/new.rs` `cmd_new` 收 `session`：话题内的 `!new` 用该会话的 `thread_id` 建新会话，老会话不归档、任务不动；顶层发的照旧自己当 root。
**这条路由的正确性依赖 `created_at` 严格递增**：同一 `thread_id` 上挂着两个非归档会话，`find_session_by_thread` 取 `created_at` 最新的，
相等时按随机 uuid；SQLite 时间戳精度下同一时刻建两条会话同样会撞。复现测试不用固定钟。

### ⑨ `!stop` 记发起人；命令记 `route=command`（`2353440`）

- `cancel_task_inner` → `cancel_task_by(…, issuer)`（`dispatch.rs:327`）：`!stop` 与卡片 Stop 传 `Some(sender_id)`，写 `stopped_by`（`:444`）；
  trait 方法、`!restart`、卡死替换传 `None`，**整个不写这个键**（`cancel.rs` 的载荷钉照绿，新测试断言键集 = `["by", "steps"]`）。
- `commands/mod.rs:223` `record_command`：按 id 重读，终态或已有 `evidence_root_hash` 就跳过；写 `event_received{route: "command", command}`
  （`ROUTE_COMMAND` 导出）；写失败只 warn。**写的目标**：`!stop` 的 `Stoppable`、`!restart` 计进 `stopped` 的那些，都在 `cancel_task` **之前**写。
  **不写**：`Delivering`（`!stop` 与 `!restart` 都不写）、卡片按钮（不是命令）、没有目标任务的命令。
- **残余窗口**：重读与追加之间，在跑的 Stoppable 任务仍可能恰好收尾并 finalize，命令证据落在 manifest 之后。要修得动 worker（CC3 的面），本轨不修。

### ⑩ `append_turn` 撞 `DuplicateTurn` 重读 seq 重试 + R7 竞态（`239a13b`）

- `sessions.rs:308` `write_turn_locked`（调用方持 `turn_seq_lock`）：撞 `StoreError::DuplicateTurn` 就重读 seq 再写，**至多 3 次**（`APPEND_TURN_ATTEMPTS`，
  `:22`）；3 次都撞 → 把 `Store` 错误传出去，经 ③ 成 `Err` —— 与 §7「`seen_event` 之后失败」同一个缺口。其它 `StoreError` 不重试。
- 原 `:1312-1321` 注释（「P0 是单进程单副本，进程内一把锁就够」）改成：plane 自己的写者由锁串住，但 CC3 起 worker 在 `deliver()` 里用
  `next_turn_seq` 在锁外写 Assistant 轮，「一把锁就够」不再成立，所以撞号重试。
- **R7 竞态**（CC1 回执第 8 节、`docs/p1/cloud-runbook.md` §抖动记录）：`new_session` 把 `create_session` 与头几轮（回灌的历史 + 用户原话）放进
  **同一段** `turn_seq_lock` 临界区（`sessions.rs:95`），ack 挪到临界区之后，历史在建会话之前读好（不拿着锁做网络往返）。
  `reconnect_replay` 的 `a_root_and_its_thread_followup_replayed_together` 连跑 40 次：**0 红**（`runs=40 red=0`；改前 runbook 在 main 上实测 8/40）。
- support 的 `FakeStore` 加 `duplicate_next_append_turn(n)`：先用一条 Assistant 轮占掉调用方要写的 seq，再报 `DuplicateTurn`。

## 3. 新增测试与变异验证

| 测试（文件） | 钉什么 | 撤回哪处 |
|---|---|---|
| `default_max_parallel_is_strictly_serial`（cc2_dispatch） | 默认 1：worker 卡住第一个时另一个群的不被领走 | 默认改成 2 |
| `two_chats_dispatch_concurrently_with_max_parallel_2` | n=2：两个群同时在 worker 手上 | —（正向） |
| `same_session_stays_serial` | n=2 下同会话第二个等第一个收尾 | 去掉忙会话判断 |
| `aborting_run_forever_with_max_parallel_2_lets_join_return` | n=2 abort 后 `join` 立刻返回、`running`/`owned` 清空 | —（对照 `dispatch.rs`） |
| `stuck_task_is_replaced_on_next_message_with_notice` | 默认 1 下卡死替换：提示文案、cancelled、finalize、新任务跑完 | 去掉 `select!` 的放弃分支 |
| `ingress_store_failure_returns_internal`（cc2_routing） | `handler()` 对存储错误返回 `Err` | 恢复无条件 `Ok` |
| `r6_requires_supports_thread` | 无原生话题时 R6 不接 | 恢复 `ChatType::Group` 判定 |
| `p2p_goes_to_disabled_dm_stub` | p2p 有 @ 走 R7、无 @ 走 R8，不进 R6 | 去掉 p2p 排除 |
| `r6_followup_gets_ack` | R7 一次、R6 两条路各一次、bot 零次 | 去掉 R6 的 ack |
| `plane_append_turn_retries_duplicate_turn` | 撞号重试：`Ok`、`ingress.errors==0`、seq 接在被占号之后、`append_turn` 恰好 2 次 | 上限改成 1 |
| `append_turn_gives_up_after_three_duplicates` | 3 次都撞 → `Err(Store)` | —（上限的另一面） |
| `restart_seeds_thread_history`（cc2_commands） | 历史在前、`rest` 在后；人 User、其余 SystemNote | 不回灌 |
| `restart_skips_history_when_platform_cannot_read_it` | `supports_history=false` 不读 | —（正向） |
| `fullwidth_bang_and_ideographic_space_parse` | `！`、U+3000、制表符；路由层也认 `！` | 按 `' '` 切 |
| `chinese_alias_status` | 别名 → 规范名、计数器用规范名；不带 `!` 不是命令 | 别名表失效 |
| `help_lists_enabled_commands` | `!help` 原文；停用的一个都不出现；停用命令回新文案 | 不按 `ENABLED` 过滤 |
| `new_in_thread_routes_followups_to_the_new_session` | 话题内 `!new` 后的回复进新会话 | 退回自己当 root |
| `stop_records_issuer_and_command_evidence` | 链 = created, received, received(command), cancelled；`stopped_by` | 去掉 `stopped_by` / 去掉命令证据 |
| `card_stop_records_issuer_but_no_command_evidence` | 卡片记 `stopped_by`、不写 `route=command` | 去掉 `stopped_by` |
| `trait_cancel_writes_no_stopped_by_key` | trait 路径载荷键集 = `["by","steps"]` | —（字节不变的钉） |
| `restart_writes_command_evidence_on_the_tasks_it_stops` | `!restart` 停掉的写 `command=!restart`，不写 `stopped_by` | 去掉 `!restart` 的命令证据 |
| `stop_on_an_answering_task_writes_no_command_evidence`（tests/commands.rs） | Answering 任务的链长度不变 | Delivering 分支也写命令证据 |
| `try_pop_where_skips_busy_keys_without_reordering_the_rest`（queue 单测） | 跳过忙 key、其余原序 | — |
| `unknown_text_points_to_an_enabled_help` / `registry_names_match_their_help_lines`（commands 单测） | 取代 `known_commands_are_all_reachable_from_the_unknown_text` | — |

变异输出原文（`mutate.sh`：恰好一处替换 → 跑测试 → 原样写回并 `touch`，不保留旧 mtime）：

```
---- 变异：plane.rs：[            max_parallel: 1,] → [            max_parallel: 2,]
---- default_max_parallel_is_strictly_serial stdout ----
thread 'default_max_parallel_is_strictly_serial' (7723) panicked at crates/control/tests/cc2_dispatch.rs:66:5:
test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.30s
error: test failed, to rerun pass `-p aite-control --test cc2_dispatch`
---- 已还原
---- 变异：dispatch.rs：[try_pop_where(|k| busy.contains(k))] → [try_pop_where(|_| false)]
---- same_session_stays_serial stdout ----
thread 'same_session_stays_serial' (7303) panicked at crates/control/tests/cc2_dispatch.rs:147:5:
test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.30s
error: test failed, to rerun pass `-p aite-control --test cc2_dispatch`
---- 已还原
---- 变异：ingress.rs：[IngressError::Evidence(_))) => Err(e),] → [IngressError::Evidence(_))) => { let _ = e; Ok(()) }]
---- ingress_store_failure_returns_internal stdout ----
thread 'ingress_store_failure_returns_internal' (11553) panicked at crates/control/tests/cc2_routing.rs:19:5:
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_routing`
---- 已还原
---- 变异：restart.rs：[let seed = session.as_ref().map(|_| thread_id.as_str());] → [let seed: Option<&str> = None;]
---- restart_seeds_thread_history stdout ----
thread 'restart_seeds_thread_history' (7538) panicked at crates/control/tests/cc2_commands.rs:60:5:
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_commands`
---- 已还原
---- 变异：dispatch.rs：[            _ = abandon.notified() => {] → [            _ = std::future::pending::<()>() => { let _ = &abandon;]
---- stuck_task_is_replaced_on_next_message_with_notice stdout ----
thread 'stuck_task_is_replaced_on_next_message_with_notice' (8483) panicked at crates/control/tests/cc2_dispatch.rs:253:5:
test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.72s
error: test failed, to rerun pass `-p aite-control --test cc2_dispatch`
---- 已还原
---- 变异：mod.rs：[text.split_once(char::is_whitespace)] → [text.split_once(' ')]
---- fullwidth_bang_and_ideographic_space_parse stdout ----
thread 'fullwidth_bang_and_ideographic_space_parse' (11989) panicked at crates/control/tests/cc2_commands.rs:153:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_commands`
---- 已还原
---- 变异：mod.rs：[.and_then(|bare| ALIASES.iter().find(|(alias, _)| *alias == bare))] → [.and_then(|bare| ALIASES.iter().find(|(alias, _)| *alias == bare && false))]
---- chinese_alias_status stdout ----
thread 'chinese_alias_status' (12055) panicked at crates/control/tests/cc2_commands.rs:180:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_commands`
---- 已还原
---- 变异：help.rs：[.filter(|c| c.enabled)] → [.filter(|_| true)]
---- help_lists_enabled_commands stdout ----
thread 'help_lists_enabled_commands' (12120) panicked at crates/control/tests/cc2_commands.rs:213:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_commands`
---- 已还原
---- 变异：mod.rs：[if ev.chat_type != ChatType::P2p] → [if true]
---- p2p_goes_to_disabled_dm_stub stdout ----
thread 'p2p_goes_to_disabled_dm_stub' (13151) panicked at crates/control/tests/cc2_routing.rs:94:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.10s
error: test failed, to rerun pass `-p aite-control --test cc2_routing`
---- 已还原
---- 变异：mod.rs：[&& self.platform.capabilities().supports_thread] → [&& ev.chat_type == ChatType::Group]
---- r6_requires_supports_thread stdout ----
thread 'r6_requires_supports_thread' (13218) panicked at crates/control/tests/cc2_routing.rs:65:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_routing`
---- 已还原
---- 变异：mod.rs：[        self.ack(ev).await;] → []
---- r6_followup_gets_ack stdout ----
thread 'r6_followup_gets_ack' (13283) panicked at crates/control/tests/cc2_routing.rs:136:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_routing`
---- 已还原
---- 变异：new.rs：[.unwrap_or_else(|| ev.anchor.message_id.clone());] → [.filter(|_| false).unwrap_or_else(|| ev.anchor.message_id.clone());]
---- new_in_thread_routes_followups_to_the_new_session stdout ----
thread 'new_in_thread_routes_followups_to_the_new_session' (15706) panicked at crates/control/tests/cc2_commands.rs:287:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_commands`
---- 已还原
---- 变异：dispatch.rs：[payload.insert("stopped_by".into(), Value::String(issuer));] → [let _ = issuer;]
---- card_stop_records_issuer_but_no_command_evidence stdout ----
thread 'card_stop_records_issuer_but_no_command_evidence' (16691) panicked at crates/control/tests/cc2_commands.rs:396:5:
---- stop_records_issuer_and_command_evidence stdout ----
thread 'stop_records_issuer_and_command_evidence' (16692) panicked at crates/control/tests/cc2_commands.rs:357:5:
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_commands`
---- 已还原
---- 变异：stop.rs：[self.record_command(ev, &task, "!stop").await;] → []
---- stop_records_issuer_and_command_evidence stdout ----
thread 'stop_records_issuer_and_command_evidence' (16765) panicked at crates/control/tests/cc2_commands.rs:337:5:
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_commands`
---- 已还原
---- 变异：stop.rs：[StopTarget::Delivering(task) => {] → [StopTarget::Delivering(task) => { self.record_command(ev, &task, "!stop").await;]
---- stop_on_an_answering_task_writes_no_command_evidence stdout ----
thread 'stop_on_an_answering_task_writes_no_command_evidence' (16837) panicked at crates/control/tests/commands.rs:889:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 24 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test commands`
---- 已还原
---- 变异：restart.rs：[self.record_command(ev, &t, "!restart").await;] → []
---- restart_writes_command_evidence_on_the_tasks_it_stops stdout ----
thread 'restart_writes_command_evidence_on_the_tasks_it_stops' (16912) panicked at crates/control/tests/cc2_commands.rs:434:49:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.10s
error: test failed, to rerun pass `-p aite-control --test cc2_commands`
---- 已还原
---- 变异：sessions.rs：[pub(crate) const APPEND_TURN_ATTEMPTS: u32 = 3;] → [pub(crate) const APPEND_TURN_ATTEMPTS: u32 = 1;]
---- plane_append_turn_retries_duplicate_turn stdout ----
thread 'plane_append_turn_retries_duplicate_turn' (19436) panicked at crates/control/tests/cc2_routing.rs:178:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.09s
error: test failed, to rerun pass `-p aite-control --test cc2_routing`
---- 已还原
```

## 4. `check.sh` 完整输出（终版，HEAD = `239a13b`）

```

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
   Compiling aite-edge-client v0.0.1 (/home/user/aite/core/crates/edge-client)
   Compiling aite-gateway v0.0.1 (/home/user/aite/core/crates/gateway)
   Compiling aite-memory v0.0.1 (/home/user/aite/core/crates/memory)
   Compiling aite-routines v0.0.1 (/home/user/aite/core/crates/routines)
   Compiling aite-githost v0.0.1 (/home/user/aite/core/crates/githost)
   Compiling aite-search v0.0.1 (/home/user/aite/core/crates/search)
   Compiling aite v0.0.1 (/home/user/aite/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.93s
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
    Checking aite-models v0.0.1 (/home/user/aite/core/crates/models)
    Checking aite-gateway v0.0.1 (/home/user/aite/core/crates/gateway)
    Checking aite-search v0.0.1 (/home/user/aite/core/crates/search)
    Checking aite-routines v0.0.1 (/home/user/aite/core/crates/routines)
    Checking aite-githost v0.0.1 (/home/user/aite/core/crates/githost)
    Checking aite-memory v0.0.1 (/home/user/aite/core/crates/memory)
    Checking aite v0.0.1 (/home/user/aite/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 55.64s
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
  Executable tests/test_context.rs (target/debug/deps/test_context-f9e37917ca0f9199)
  Executable tests/test_final.rs (target/debug/deps/test_final-a7d6f2ccc8c655a8)
  Executable tests/test_in_flight.rs (target/debug/deps/test_in_flight-931a5c77157efb1e)
  Executable tests/test_limits.rs (target/debug/deps/test_limits-029e3eecbee11646)
  Executable tests/test_loop_fallbacks.rs (target/debug/deps/test_loop_fallbacks-13c22a3da37ccddb)
  Executable tests/test_prompts_checklist.rs (target/debug/deps/test_prompts_checklist-95800b50d62494fa)
  Executable tests/test_sandbox_handoff.rs (target/debug/deps/test_sandbox_handoff-18e25eacc1b8f353)
  Executable tests/test_steer.rs (target/debug/deps/test_steer-63c771c6cd3f40a3)
-> exit 0

=== C1 契约测试 ===
$ bash -c cd core && cargo test -p aite-contracts 2>&1 | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"contracts passed=\" p \" failed=\" f; exit (f>0)}"
contracts passed=25 failed=0
-> exit 0

=== B 全量 cargo test ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^(---- .* stdout ----|error(: test failed|: could not compile|\[E[0-9]+\]))" | sort -u | head -n 7; printf "%s\n" "$o" | grep -E "^test result" | awk -v c="$c" "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p+0 \" failed=\" f+0; exit (c != 0 || f > 0 || NR == 0)}"
cargo passed=924 failed=0
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

终版逐二进制条数（`aite-control`）：

```
     Running unittests src/lib.rs (target/debug/deps/aite_control)
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/cancel.rs (target/debug/deps/cancel)
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/cc2_commands.rs (target/debug/deps/cc2_commands)
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/cc2_dispatch.rs (target/debug/deps/cc2_dispatch)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/cc2_routing.rs (target/debug/deps/cc2_routing)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/commands.rs (target/debug/deps/commands)
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/dispatch.rs (target/debug/deps/dispatch)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/drop_logs.rs (target/debug/deps/drop_logs)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/ingress.rs (target/debug/deps/ingress)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/ingress_failures.rs (target/debug/deps/ingress_failures)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/routing.rs (target/debug/deps/routing)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/steer_evidence.rs (target/debug/deps/steer_evidence)
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/steer_routing.rs (target/debug/deps/steer_routing)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running tests/wording.rs (target/debug/deps/wording)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## 5. cargo passed 增量：897 → 924（Δ = +27，全在 `aite-control`）

| 二进制 | 前 | 后 | Δ | 来源 |
|---|---|---|---|---|
| `unittests src/lib.rs` | 13 | 15 | +2 | queue +1（`try_pop_where_…`）；commands −1（删 `known_commands_are_all_reachable_from_the_unknown_text`）+2（`unknown_text_points_to_an_enabled_help`、`registry_names_match_their_help_lines`） |
| `cc2_dispatch`（新） | 0 | 6 | +6 | 5 条 + support 的 `parking_counts_even_when_nobody_is_waiting_yet` |
| `cc2_routing`（新） | 0 | 7 | +7 | 6 条（含 ⑩ 的 `plane_append_turn_retries_duplicate_turn`）+ support 那条 |
| `cc2_commands`（新） | 0 | 11 | +11 | 10 条 + support 那条 |
| `commands` | 24 | 25 | +1 | `stop_on_an_answering_task_writes_no_command_evidence` |

**翻转 / 改写的钉**：

| 位置 | 改前 | 改后 | 为什么 |
|---|---|---|---|
| `tests/wording.rs` `module_constants_are_byte_exact` | `UNKNOWN_COMMAND_TEXT == "未知命令，可用：!status !stop <任务号> !restart !new"` | `== "未知命令，发 !help 看全部命令"` | ⑤ 解冻文案：可用命令由注册表决定、随各轨启用而变 |
| `src/commands` 单测 `known_commands_are_all_reachable_from_the_unknown_text` | `KNOWN_COMMANDS` 每个都在未知命令那句里 | 删掉，换成 `unknown_text_points_to_an_enabled_help`（那句含 `!help` 且 `!help` 启用） | `KNOWN_COMMANDS` 由注册表取代 |
| `tests/commands.rs` `unknown_command` | 那句含 `!status`、`!restart` | 含 `!help` | 同上 |
| `tests/commands.rs` `new_forces_fresh_session_inside_existing_thread` | 按 ROOT 查到老会话；`om_5` 成了新 root | 按 ROOT 查到新会话；`om_5` 查不到；老会话仍 `Active`（任务断言照旧） | ⑧ 修路由 |

## 6. 被守卫拦过的命令

除开场自检第 2 步外，被拦 5 次，**全是已知误拦「ASCII 引号跨行 → 无法解析」**（CLAUDE.md「已知误拦与绕法」第 1 条），改成先写脚本文件再跑：
多行 `python3 -c`（读 tracks JSON）、两次 heredoc（写补丁脚本）、两次 `mutate.sh` 的多行参数。拦截原文每次都是：
`blocked: 该操作触碰受保护面 <命令无法解析: No closing quotation>（解析失败）。停止当前工作并向人类报告。`
没有一次触碰受保护面。

## 7. 记账转出去的

| 事项 | 为什么不在本轨 | 建议归 |
|---|---|---|
| `docs/acceptance-M.md:932` 的旧命令表（还写着旧未知命令文案 / 四条命令） | `docs/**` 只读 | 文档轨 / 总管 |
| worker 写的 `cancelled` 没有发起人（在跑任务被 `!stop` 时由 worker 收尾） | `worker/**` 是 CC3 的面；本轨靠命令证据记了发起人 | CC3 / DD4 |
| `with_max_parallel` 接 `WorkerConfig.max_parallel_tasks`（默认 4），接管因此变红的 app / evals 串行钉 | `app/src/**` 是 CC4 / T0c 的面 | T0c |
| `app/tests/common/mod.rs` 的 `GatedPlatform::emit` 已改（③，总管批准越界） | 不在可写面 | 审 PR 时确认；CC5 / CC4 知悉 |
| ⑨ 的残余窗口（命令证据可能落在 worker 的 finalize 之后） | 要动 worker | CC3 |
| ④ 的库侧残余窗口（被丢的 worker 一笔在飞的 `update_task` 落在 `Cancelled` 之后） | 要 worker 配合或 store 比较并交换 | CC3 / CC5 |

## 8. 没做的与原因

- 无。①–⑩ 与 R7 竞态都做完。④ 的回灌只在「原来有会话」时做（顶层 `!restart` 没有旧话题可读），与派单字面「新会话建好后调 `read_history`」略有收窄，写在这里。

## 9. 契约缺口

- **撤销单条去重记录**：`seen_event` 之后失败的事件，③ 已能把 `Err` 传给平台重推，但重推回来 R2 认得它、当重复丢掉。要救回来需要
  `SessionStore::unsee_event(event_id)`（或 `seen_event` 改成「处理完才提交」的两段式）。开放通道绕不过去：去重表只有 store 能改。⑩ 撞满 3 次同理。
- **卡死阈值进 `WorkerConfig`**：建议 `worker.stuck_after_sec`（默认 900）；本轨先用 control 常量 + `with_stuck_after_sec`，T0c 接线即可。
- **在跑任务的 `cancelled` 带发起人**：worker 写 `cancelled` 时拿不到是谁停的；需要 `RunHooks` 多一个 `cancelled_by: Arc<dyn Fn() -> Option<String>>`
  （或 `is_cancelled` 改返回 `Option<String>`）。今天由命令证据补位。
- **store 比较并交换**：`update_task` 没有「仅当状态为 X 时写」的形状，④ / 取消路径与被丢的 worker 之间的最后一笔写只能靠时序。


## 10. 验收 §7 其余几条

- §7.2 `--exact` 那 10 个名字，每个恰好一次、全 `ok`：

```
test chinese_alias_status ... ok
test default_max_parallel_is_strictly_serial ... ok
test fullwidth_bang_and_ideographic_space_parse ... ok
test help_lists_enabled_commands ... ok
test ingress_store_failure_returns_internal ... ok
test plane_append_turn_retries_duplicate_turn ... ok
test r6_followup_gets_ack ... ok
test restart_seeds_thread_history ... ok
test same_session_stays_serial ... ok
test two_chats_dispatch_concurrently_with_max_parallel_2 ... ok
```

- §7.3 `cargo test -p aite --test reconnect_replay --test graceful_shutdown --test startup_recovery` → 11 / 11 / 9 passed，0 failed（一次过，没抖）。
- §7.4 clippy `-D warnings` → exit 0（check.sh A4a）。
- §7.6 `git diff --name-only origin/main...HEAD` 落在可写面外的只有 `core/crates/app/tests/common/mod.rs`（③，总管批准，见 §2 ③）。
