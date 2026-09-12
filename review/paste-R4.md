# 任务 R4 — Rust ControlPlane：路由 R1–R8、命令、派发、steer、reaper、取消、Ingress

## 背景：这轨是从哪来的

Aite 决定整体用 **Rust + Go** 重写（权威文档 `docs/dev-spec-2026-09-11-rustgo.md`，先通读 §1–§3、§5–§7）。
Python 版 P0 已经跑通（T0–T24），**它是规格与参考，只读**。R0 已合入 main（`c9d96d2`）。

**你这轨是 core 的控制面**：`aite/control/plane.py`（712 行）+ `commands.py` + `aite/ingress/handler.py` → `core/crates/control`，
实现 R0 冻结的 `ControlPlane` trait（含 `run_pending / pending / join / cancel_task / counters`），并把
`tests/control/**` 里除 store 两份之外的 55 个测试搬过来。它是 P0 里判定最密集的一块 —— T14 / T22 / T24 三轮修的 bug 都在这里。

移植清单：`review/inventory-core.md` §1（plane / commands / ingress 签名与并发结构）、§2（R1–R8 差异表、`_start_task` 顺序、steer 三态、`_owned`、cancel 双分支、seq 锁、**全部命令文案原文**）、§5、§8（测试逐条）、§9 第 1–9 条。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-r4
分支     : task-r4
基线     : c9d96d2   ← main 的 HEAD（完整 sha c9d96d29d6d7ad835deef9fc80ec365c786fe876）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin`（已写进 `~/.bash_profile`）。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r4
git log --oneline -1                                   # 期望 c9d96d2 ...
git status --short                                     # 期望空
scripts/check.sh --quick                               # 期望最后一行 "全部通过"（首次 cargo 全量编译 2–5 分钟）
core/target/debug/aite contracts lock --check          # 期望 OK 36 files
cd core && cargo test -p aite-control                  # 期望 0 passed（占位 crate）
```

`scripts/check.sh --quick` 关键行期望：`contracts passed=25 failed=0`、`cargo passed=35 failed=0`。

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦停下报告。

## 可写路径（白名单，之外一律只读）

```
core/crates/control/**      ← Cargo.toml（只能引用 workspace 已钉的库）、src/**、tests/**
```

**邻居（只读）**：
- `core/crates/contracts/src/ports.rs`：`ControlPlane`、`TaskWorker` + `RunHooks{drain_steer, is_cancelled}`、`SessionStore`、`EvidenceWriter`、`PlatformPort`、`SandboxPort`、`ToolGateway`（含 `release_task`）、`EventHandler`。**并行期间它们的真实现都不存在**（store 是 R3、worker 是 R5 …），你在 `tests/` 里私写替身（内存 store、记账 platform、脚本化 worker、假 sandbox / gateway / evidence）—— 照 `tests/control/control_fakes.py` 搬。
- `core/crates/contracts/src/{events,session,evidence,outbound,config}.rs`：数据形状；`IngressError`。
- 依赖：workspace 已钉 `tokio`、`serde_json`、`chrono`、`uuid`、`rand`、`hex`、`tracing`、`async-trait`、`thiserror`。**要别的 → 停下报告**。
- Python 参考：`aite/control/plane.py`、`commands.py`、`aite/ingress/handler.py`、`tests/control/**`（除 `test_persistence.py` / `test_store_concurrency.py` 归 R3）。

## 要做什么

### ① 公开面（RΩ 按这个组装，名字别自己发明）
```rust
pub struct ControlDeps {
    pub store: Arc<dyn SessionStore>, pub platform: Arc<dyn PlatformPort>, pub evidence: Arc<dyn EvidenceWriter>,
    pub config: AiteConfig,
    pub worker: Option<Arc<dyn TaskWorker>>, pub gateway: Option<Arc<dyn ToolGateway>>, pub sandbox: Option<Arc<dyn SandboxPort>>,
    pub model_name: String,            // manifest 的 model 退路（Python 用 getattr(model, "name")）
}
impl InProcessControlPlane { pub fn new(deps: ControlDeps) -> Self; pub fn with_clock(...) / with_sleep(...)（测试注入）; pub fn pending_steer(&self, task_id) -> Vec<String>; }
impl ControlPlane for InProcessControlPlane { … }
pub struct Ingress { … }   // Ingress::new(plane: Arc<dyn ControlPlane>) ; handler() -> EventHandler ; counters()
pub mod commands { pub const UNKNOWN_COMMAND_TEXT; pub fn parse_command; pub fn normalize_task_no; }
```

### ② `handle_event` = R1–R8，按编号顺序求值命中即停（清单 §2 表）
- R1 非 human → `events.nonhuman`；R2 `seen_event` → `events.duplicate`（**先落库再往下**）；R3 card_action（`action` 缺 → `events.bad_card_action`；stop 走 cancel；evidence 回帖 `任务 {task_id} 的证据目录：{evidence.task_dir(task_id)}`）；R4 编辑写 `system_note`（`[用户修改了消息] 新内容：{text}`）、删除无动作、其余审计计数；R5 `!` 命令（`mentioned || session.is_some()`；**session 在 R5 前就查好**）；R6 `_continue_session`（append_turn + `last_active_at` + steer 三态 + 孤儿计 `events.orphan_task` + 新建）；R7 `_new_session`（`thread_id = message_id`、`add_reaction(ack)` 失败不影响、`next_task_no`、入队）；R8 `events.ignored`。
- `handle_event` 外层：任何错误 `events.dropped += 1` 后**继续返回 Err**（T24）；只有 `Ingress` 那层才吞掉并计 `ingress.errors`。
- `_start_task` 六步顺序严格（uuid v4、`token_hex(16)` = 32 hex、`title = clip(text, 40)`、`create_task`、`task_created` 证据、`event_received{route:"new_task"}` 证据、`_owned.insert`、入队）。`clip` 与 worker 同语义（折叠空白、`limit-1 + "…"`）—— 你自己写一份，别跨轨依赖。
- `_append_turn` 用一把 `tokio::sync::Mutex` 包住 `list_turns(1) → seq → append_turn`（T24）。
- steer：`_steer: HashMap<task_id, Vec<String>>`；`_steer_target` 两判据（`_owned` + 优先 `_running`，`max by (created_at, id)`）；排 steer **先写** `event_received{route:"steer", text: clip(text, 40)}` 再入队；`_dispatch_task` 开跑前清掉该任务的 steer。
- 派发：无界 `mpsc`（或 `VecDeque + Notify`）；**串行**一次一个；`run_forever` = 起 reaper（先 sleep 后 reap，间隔 60s，`reap_idle(config.sandbox.idle_sec)`，异常 `control.reap_failed` 不打断，`sandbox.reaped` 计数）+ 取队列 → `_run_task`（`try … finally: _steer.remove; _owned.remove`）；`_dispatch_task` 四条提前 return；`worker.run(task, session, Some(initiator), RunHooks{drain_steer, is_cancelled})`；`run_pending` 只跑当前排队的；`join` 等队列空且无在跑；`pending`。
- `cancel_task` 双分支（清单 §2）：`running` 先抄；`_cancelled.insert`；`status=cancelled` 落库；`sandbox.release(task.sandbox_id)`；**不在跑** → `gateway.release_task` + `cancelled{by:"stop", steps}` 证据 + 卡片置 cancelled（`render_card` 你自己写一份最小版：Python 的 `render_card` 在 worker/card.py，两边输出要一致 —— 照 `inventory-core.md` §4 卡片合并段的 `render_card` 规则）+ `_finalize_evidence`（幂等守卫 `evidence_root_hash`；manifest 四字段；finalize 后 `update_task`）；在跑 → 什么都不写；`notify` → `任务 {task_no} 已停止。`。
- 命令：`!status`（含 `_dropped_note`）、`!stop`（`_resolve_stop_target` 三条）、`!restart`（归档 + 逐个 `cancel_task(notify=false)` + 回帖三态：空 rest / rest 非空且 note 非空 / **rest 非空且无 note 不回帖**）、`!new`（**rest 非空不回帖**）、未知命令。**文案逐字照清单 §5。**
- `counters()`：`events.*`、`commands!status` 这种拼出来的 key、`sandbox.reaped`、`ingress.*`。日志事件名照 Python（`control.no_worker`、`control.dispatch_failed`、`control.reap_failed`、`control.reaped`、`ingress.handle_failed`、`ingress.slow`）。

### ③ 测试（55 条逐条对照，清单 §8）
`test_routing.py` 12、`test_commands.py` 10、`test_dispatch.py` 5（reaper 用注入 sleep 断言 `[60, 60]` 与 `reap_calls == [300, 300]`）、`test_ingress.py` 5、`test_steer_routing.py` 6（含白盒 `_steer_target` 三条 —— 做成 `pub(crate)` 或在 `src` 里的 `#[cfg(test)]`）、`test_steer_evidence.py` 8、`test_ingress_failures.py` 9（store 方法可注入失败；6 条同话题 `join_all` 并发追问 seq 0..6；20 条 burst）。
替身放 `tests/support/mod.rs`（或 `src/testing.rs` + `#[cfg(test)]`）：`FakeStore`（内存、深拷贝语义、可注入失败）、`FakePlatform`（记 texts / cards / card_updates / files / reactions）、`ScriptedWorker: TaskWorker`（按脚本改 status、可在跑到一半调 `drain_steer` / `is_cancelled`）、`FakeSandbox`、`FakeGateway`（记 `release_task`）、`FakeEvidence`（真算 hash 链，照契约函数）。

## 纪律

1. 契约与锁：`OK 36 files` 全程不变。
2. 白名单之外只读；缺接口 / 要依赖 → **停下报告**（临时需要的内部类型放自己 crate）。
3. 不 panic：`unwrap/expect` 不许落在运行期可失败路径；锁不跨 await 持有 `std::sync::Mutex`。
4. 测试不靠真实 sleep（时钟 / sleep 注入，或 dev-dependencies 给 `tokio` 加 `features = ["test-util"]` 用 `tokio::time::pause`，只改自己的 Cargo.toml）。
5. `cargo clippy -p aite-control --all-targets -- -D warnings` 干净；格式化只对自己的文件：`rustfmt --edition 2024 $(git ls-files 'core/crates/control/**/*.rs')`（整树 `cargo fmt` 守卫会拦）。
6. 每条结论挂实测。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r4/core
cargo test -p aite-control 2>&1 | grep -E '^test result'           # 期望全部 ok，一条不许 failed
cargo clippy -p aite-control --all-targets -- -D warnings             # 期望退出 0
cd .. && scripts/check.sh --quick                                     # 期望 全部通过
core/target/debug/aite contracts lock --check                         # 期望 OK 36 files
```

## 回执格式

```
## R4 回执

基线 c9d96d2 → 提交 <短 sha>

### 移植对照表
| Python 测试文件 | Rust 测试 | 条数 | 差异说明 |
（55 条 → M 条）

### 与 Python 行为的差异（逐条；没有就写"没有"）

### 并发结构怎么落的
队列 / 串行派发 / join / steer 表 / seq 锁 / reaper：<各一句>

### 改了什么
- <文件> <一句话>

### 实测输出（粘实际的）
$ cargo test -p aite-control | grep 'test result'
<粘>
$ scripts/check.sh --quick
<最后 3 行>
$ core/target/debug/aite contracts lock --check
<那一行>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-r4` 分支上，回执贴出来。
