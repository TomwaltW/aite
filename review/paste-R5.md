# 任务 R5 — Rust AgentWorker（agent loop / 卡片合并 / 上下文 / 提示词）+ OpenAI 兼容 ModelPort

## 背景：这轨是从哪来的

Aite 决定整体用 **Rust + Go** 重写（权威文档 `docs/dev-spec-2026-09-11-rustgo.md`，先通读 §1–§3、§5–§7）。
Python 版 P0 已经跑通（T0–T24），**它是规格与参考，只读**。R0 已合入 main（`c9d96d2`）。

**你这轨是 core 里跟模型打交道的全部**：`aite/worker/{loop,card,context}.py`（约 850 行）+ `prompts/platform.md` → `core/crates/worker`，
实现 R0 冻结的 `TaskWorker` trait；`aite/models/openai_compat.py`（227 行）→ `core/crates/models`，实现 `ModelPort`。
把 `tests/worker/**` 67 条与 `tests/e2e/test_t4_model_openai_compat.py` 22 条搬过来。这里是 §3.3 失败面、W1–W9、T19 提示词、T20 重复检测的落点。

移植清单：`review/inventory-core.md` §1（loop / card / context 签名）、§4（**每一步顺序、本地工具表、卡片合并、final 产物、失败面表、T19、T20、steer 时机、沙箱归属**）、§5（全部文案原文）、§8（测试逐条）、§9 第 10–25 条；
`review/inventory-gateway-evals.md` §4（OpenAI 兼容客户端全部细节 + 真模型实测结论）。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-r5
分支     : task-r5
基线     : c9d96d2   ← main 的 HEAD（完整 sha c9d96d29d6d7ad835deef9fc80ec365c786fe876）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin`（已写进 `~/.bash_profile`）。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r5
git log --oneline -1                                   # 期望 c9d96d2 ...
git status --short                                     # 期望空
scripts/check.sh --quick                               # 期望最后一行 "全部通过"（首次 cargo 全量编译 2–5 分钟）
core/target/debug/aite contracts lock --check          # 期望 OK 36 files
cmp core/crates/worker/prompts/platform.md aite/worker/prompts/platform.md && echo prompt-identical   # 期望 prompt-identical
cd core && cargo test -p aite-worker -p aite-models    # 期望各 0 passed（占位 crate）
```

`scripts/check.sh --quick` 关键行期望：`contracts passed=25 failed=0`、`cargo passed=35 failed=0`。

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦停下报告。

## 可写路径（白名单，之外一律只读）

```
core/crates/worker/**       ← Cargo.toml（只能引用 workspace 已钉的库）、src/**、tests/**、prompts/platform.md（内容不许变：cmp 必须一直相等）
core/crates/models/**       ← 同上
```

**邻居（只读）**：
- `core/crates/contracts/src/ports.rs`：`TaskWorker::run(task, session, initiator, RunHooks) -> Task`、`RunHooks{drain_steer, is_cancelled}`、`ModelPort::{name, chat}`、`ToolGateway`（含 `sandbox_id_of / release_task`）、`SandboxPort`、`PlatformPort`、`SessionStore`、`EvidenceWriter`。真实现并行期间不存在，`tests/` 里私写替身（照 `tests/worker/worker_fakes.py`：ScriptedModel 带 `on_call` 钩子、FakeGateway 带 `sandbox_id_of / release_task / hold_sandbox`、FakeSandbox 记 `get_file_calls`）。
- `core/crates/contracts/src/{protocol,gateway,session,outbound,evidence,config,errors}.rs`：`Message / Role / ModelTurn / Usage / ToolSpec / ToolCallRequest / all_model_tools() / is_local_tool()`、`ToolContext / ToolResult / ToolErrorCode`、`ChecklistItem / Task / TaskStatus`、`ChecklistCard / CardStatus / OutboundFile`、`EvidenceKind`、`WorkerConfig / ModelConfig`、`ModelError`。
- 依赖：workspace 已钉 `tokio`、`reqwest 0.13（rustls, json）`、`serde/serde_json`、`chrono`、`sha2/hex`、`tracing`、`async-trait`、`thiserror`。**没有 wiremock**：模型客户端测试自起 `tokio::net::TcpListener` 做最小 HTTP/1.1 假服务（读到 `\r\n\r\n` + Content-Length，回 canned JSON）。要别的依赖 → 停下报告。
- Python 参考：`aite/worker/**`、`aite/models/openai_compat.py`、`tests/worker/**`、`tests/e2e/test_t4_model_openai_compat.py`、`evals/live-report-*.md`。

## 要做什么

### ① `aite-worker` 公开面
```rust
pub struct WorkerDeps { pub store: Arc<dyn SessionStore>, pub platform: Arc<dyn PlatformPort>, pub model: Arc<dyn ModelPort>,
    pub evidence: Arc<dyn EvidenceWriter>, pub config: AiteConfig, pub gateway: Option<Arc<dyn ToolGateway>>, pub sandbox: Option<Arc<dyn SandboxPort>> }
impl AgentWorker { pub fn new(deps: WorkerDeps) -> Self; pub fn with_clock(..) / with_sleep(..)（测试注入） }
impl TaskWorker for AgentWorker { run(...) ; in_flight() }
pub mod card    { pub const MAX_TITLE_CHARS = 40; pub const MAX_ITEM_CHARS = 20; pub fn clip(text, limit) -> String; pub fn render_card(task, session, initiator, status, note) -> ChecklistCard; pub struct CardCoalescer {…} }
pub mod context { pub fn load_system_prompt(path) -> Result<String>; pub fn build_context(system_prompt, turns, history, attachments) -> Vec<Message>; 常量 HISTORY_HEADER / ATTACHMENT_HEADER }
pub mod texts   { 所有固定文案常量（清单 §5），供测试与 RΩ 引用 }
```
`in_flight()`：`run` 期间把 `(task, session)` 放进表，`finally` 移除（旧 `AppWorker.in_flight`）；`run` 开头调 `gateway.register_task(task.id, session_token)`，`_finish` 里 `release_task`（旧 AppWorker 的两件事并进来）。

### ② agent loop（清单 §4 逐条，顺序不能变）
开跑前：`build_messages`（W1：system → transcript（40 轮截断规则）→ 群历史（只 human，`[id] 名字: 文本`）→ 附件清单，工具目录走 `chat()` 的 `tools`）；`title` 兜底；`status = planning`、`task.model = model.name()`；`_save`；计时。
每轮：取消 → 步数 → 墙钟 → `drain_steer` 追加 user 消息 → `chat`（3 次，退避 `[2s, 5s]`，注入 sleep）→ 计数 tokens / cost → `model_call` 证据（5 键，`messages_hash = sha256(每条 message 的 JSON 以 "\n" 连接)` —— 用 `serde_json::to_string(&Message)`，字段顺序与 contracts 定义一致）→ 无 tool_call 分支（`step_index == 0 && text` → Answering；否则 `NUDGE_TEXT` + `maybe_flush`）→ 有 tool_call 逐张：指纹 / `repeats` / 第 5 次原样重复先 `_fail` / `_ensure_card`（非 final）/ `final` 解析（坏 → invalid_args++ 并 `break`）/ 本地工具表 / gateway 工具 / 第 3 次重复追加 `REPEAT_NUDGE_TEXT` / 两个连续计数只在 `ok` 时清零 / 阈值 3 与 2 → `_fail` → 一步末尾再查 invalid_args、`_refresh_card`、`_save`。
`_deliver`（产物逐个 `_fetch_artifact` → `send_file` → `artifact` 证据；缺的追加 `产物 {x} 未找到`；`send_text`；`delivered` 证据；`_close_card`；`_finish`）、`_fail`、`_cancel`、`_finish`（`finalize` 四字段 → `release_task` → `_save`）。**全部文案逐字照清单 §5。**
`CardCoalescer`：纯拉取式无定时器（`ensure_card / update / maybe_flush / force_flush`），时钟注入；`render_card` 的 footer / `started_at`（UTC、小时不补零）照抄。
`run()` 外层兜底：任何错误 → `_fail("任务 {task_no} 执行出错：{err}")`；trait 承诺不外抛。

### ③ `aite-models`：`OpenAiCompatModel`
- `OpenAiCompatModel::from_config(&ModelConfig, env: &HashMap<String, String>) -> Result<Self, ModelError>`（缺 base_url / model / 密钥变量 → `ModelError::Config`，**消息只出现变量名**）；`with_base_url` 之类的测试注入；`impl ModelPort`。
- 请求体 / `to_openai_messages`（assistant 带 tool_calls 时 content 空串、arguments 不转义中文 —— `serde_json` 默认就不转义）/ `tool_choice: "auto"` / tools 空时不带两个键；响应解析 `turn_from_response`（arguments 是字符串 JSON；解析失败不抛 → `args = {}` + `raw["arg_parse_errors"]`；`call_id` 缺省 `call_{i}`；`finish_reason` 缺省 `stop`；usage 三项含 `cached_tokens`；`raw = {id, model[, arg_parse_errors], cost_cny}`）；`cost_of`；`total_cost` / `last_usage`；`Debug` 不泄露 key；这一层不重试、不做纯文本兜底、不设超时以外的花活（reqwest 默认超时可给 120s）。
- 测试 22 条（清单 §10 `test_t4_model_openai_compat.py`）用本地假 HTTP 服务断言请求体与解析。

### ④ 测试（67 + 22，清单 §8）
`test_checklist.py` 9（B3）、`test_context.py` 8、`test_final.py` 9、`test_limits.py` 7、`test_loop_fallbacks.py` 13（含指纹与 `protocol_probe._stringify` 逐字同构 —— Rust 里把指纹算法放 `worker::fingerprint` 公开，R7 的 probe 复用；测试对拍 5 组参数的字符串）、`test_prompts_checklist.py` 6、`test_sandbox_handoff.py` 6、`test_steer.py` 7、`test_cancel.py` 2。
Python 的 worker 测试走完整链路（ControlPlane 收事件 → run_pending）；你没有 ControlPlane，直接构造 `Task/Session` 调 `worker.run(...)`，`RunHooks` 用闭包注入 steer / cancel；断言面在你的替身上。

## 纪律

1. 契约与锁：`OK 36 files` 全程不变；`prompts/platform.md` 与 Python 版 `cmp` 必须一直相等。
2. 白名单之外只读；缺接口 / 要依赖 → **停下报告**。
3. 不 panic；不在 async 里阻塞；测试不靠真实 sleep（时钟 / sleep 注入或 `tokio::time::pause`，dev-dependencies 给 tokio 加 `test-util`，只改自己的 Cargo.toml）。
4. `cargo clippy -p aite-worker -p aite-models --all-targets -- -D warnings` 干净；格式化只对自己的文件：`rustfmt --edition 2024 $(git ls-files 'core/crates/worker/**/*.rs' 'core/crates/models/**/*.rs')`（整树 `cargo fmt` 守卫会拦）。
5. 密钥只从配置点名的环境变量读，任何日志 / 错误 / Debug 输出不得出现取值。
6. 每条结论挂实测。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r5/core
cargo test -p aite-worker -p aite-models 2>&1 | grep -E '^test result'    # 期望全部 ok，一条不许 failed
cargo clippy -p aite-worker -p aite-models --all-targets -- -D warnings      # 期望退出 0
cd .. && cmp core/crates/worker/prompts/platform.md aite/worker/prompts/platform.md && echo prompt-identical
scripts/check.sh --quick                                                     # 期望 全部通过
core/target/debug/aite contracts lock --check                                # 期望 OK 36 files
```

## 回执格式

```
## R5 回执

基线 c9d96d2 → 提交 <短 sha>

### 移植对照表
| Python 测试文件 | Rust 测试 | 条数 | 差异说明 |
（worker 67 + models 22 → M 条）

### 与 Python 行为的差异（逐条；没有就写"没有"）

### 指纹算法（T20）
<公开的函数签名 + 5 组对拍向量>

### 改了什么
- <文件> <一句话>

### 实测输出（粘实际的）
$ cargo test -p aite-worker -p aite-models | grep 'test result'
<粘>
$ scripts/check.sh --quick
<最后 3 行>
$ cmp … && echo prompt-identical
<>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-r5` 分支上，回执贴出来。
