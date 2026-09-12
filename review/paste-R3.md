# 任务 R3 — Rust SqliteSessionStore + FileEvidenceWriter + `aite evidence show`

## 背景：这轨是从哪来的

Aite 决定整体用 **Rust + Go** 重写（权威文档 `docs/dev-spec-2026-09-11-rustgo.md`，先通读 §1–§3、§5–§7）。
Python 版 P0 已经跑通（T0–T24），**它是规格与参考，只读**。R0 已合入 main（`c9d96d2`）。

**你这轨是 core 的持久化面**：`aite/control/store.py`（281 行）→ `core/crates/store`；`aite/evidence/writer.py`（294 行）→ `core/crates/evidence`；
`scripts/evidence_show.py`（841 行）→ `aite evidence show` 子命令（`aite-evidence::cli`）。三者都实现 R0 冻结的 trait
（`core/crates/contracts/src/ports.rs` 的 `SessionStore` / `EvidenceWriter`）。

移植清单：`review/inventory-core.md` §1（store / writer 签名）、§3（SQLite 全部细节）、§6（EvidenceWriter 全部细节）、§8（测试）、§9 第 26–31、40 条；
`review/inventory-gateway-evals.md` §8（`evidence_show.py` 的 CLI 与三条纪律）、§10（`test_evidence_show.py` 32 条）。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-r3
分支     : task-r3
基线     : c9d96d2   ← main 的 HEAD（完整 sha c9d96d29d6d7ad835deef9fc80ec365c786fe876）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin，core/rust-toolchain.toml 钉死）、go 1.27.1
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin`（已写进 `~/.bash_profile`）。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r3
git log --oneline -1                                   # 期望 c9d96d2 ...
git status --short                                     # 期望空
scripts/check.sh --quick                               # 期望最后一行 "全部通过"（首次 cargo 全量编译 2–5 分钟）
core/target/debug/aite contracts lock --check          # 期望 OK 36 files
cd core && cargo test -p aite-store -p aite-evidence   # 期望各 0 passed（占位 crate）
```

`scripts/check.sh --quick` 关键行期望：`contracts passed=25 failed=0`、`cargo passed=35 failed=0`。

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦停下报告。

## 可写路径（白名单，之外一律只读）

```
core/crates/store/**        ← Cargo.toml（只能改 [dependencies] 里引用 workspace 已钉的库）、src/**、tests/**
core/crates/evidence/**     ← 同上；cli 子模块放这里
```

**邻居（只读）**：
- `core/crates/contracts/src/ports.rs`：`SessionStore`（含 `close / next_turn_seq / recover_orphan_tasks`）、`EvidenceWriter`（含 `task_dir`）—— 签名以文件为准；缺方法 → 停下报告。
- `core/crates/contracts/src/{session,evidence,errors}.rs`：`Session/Task/Turn` 的 serde 形状（就是落进 `data` 列的 JSON）；`EvidenceEvent`、`canonical_json`、`chain_hash`、`payload_hash_of`、`GENESIS`；`StoreError::DuplicateTurn{session_id, seq}`、`EvidenceError`。
- `core/crates/app/src/main.rs`（RΩ）：`aite evidence <args>` 原样转发到 `aite_evidence::cli::run(args: Vec<String>) -> i32`，你在自己 crate 里用 clap 解析。
- 依赖：workspace 已钉 `rusqlite 0.40（bundled）`、`serde_json`、`chrono`、`sha2/hex`、`clap`、`tokio`、`tempfile`（dev）。**要别的 → 停下报告**（守卫拦 `cargo add`）。
- Python 参考：`aite/control/store.py`、`aite/evidence/writer.py`、`scripts/evidence_show.py`、`tests/control/test_persistence.py`、`tests/control/test_store_concurrency.py`、`tests/evidence/**`、`tests/tools/test_evidence_show.py`、`tests/integration/test_t18_crash_recovery.py`（只看 store/evidence 相关的用例）。

## 要做什么

### ① `aite-store`：`SqliteSessionStore`
- `SqliteSessionStore::open(path: impl AsRef<Path>) -> Result<Self, StoreError>`（`:memory:` 也认；先建父目录）；`impl SessionStore`。
- 单连接 `Mutex<rusqlite::Connection>`，每个方法在 `tokio::task::spawn_blocking` 里持锁执行（trait 是 async 的）。
- `init()`：建表 SQL **逐字**照清单 §3（sessions / turns / tasks / task_counters / seen_events + 两个索引），幂等；`PRAGMA busy_timeout = 5000`（spec D8）；**journal_mode 保持默认 `delete`**（不要开 WAL；T18 钉了「崩溃后无 -wal/-journal」）。
- `data` 列存 `serde_json::to_string(&session/&task/&turn)`；冗余列（tenant_id/chat_id/thread_id/status/created_at）从对象取。
- `seen_event`：`INSERT INTO seen_events` 撞 PK → `Ok(true)`，否则 `Ok(false)`；**写失败必须 `Err`**（只读目录）。
- `next_task_no`：`INSERT … ON CONFLICT DO UPDATE SET n = n + 1 RETURNING n` → `encode_task_no`。
- `append_turn`：撞 `(session_id, seq)` → `StoreError::DuplicateTurn`，且**不牵连**同连接上别的写（每次写完立即 commit）。
- `list_turns(limit)`：`ORDER BY seq DESC LIMIT ?` 再反转；`next_turn_seq`；`find_session_by_thread` 排除 archived、`ORDER BY created_at DESC, id DESC LIMIT 1`；`list_active_tasks` JOIN sessions 反查 chat_id，`ORDER BY t.created_at ASC, t.id ASC`，只回 `ACTIVE_TASK_STATUSES`。
- `recover_orphan_tasks()`：活跃态全部改 `failed` + `result_summary = "进程重启前该任务仍在执行，已终止。请重新发起。"` + `updated_at = now`，返回列表；**不在 init 里自动调**。
- `close()`：关连接；关后再调任何方法 → `StoreError::NotInitialized`。
- 测试（`tests/`）：`test_persistence.py` 6 条（含 B6：同一文件两个实例）、`test_store_concurrency.py` 13 条（`tokio::spawn` 真并发：16 并发同 seq 恰好 1 成功、64 并发不同 seq 不丢、200 并发 task_no 零撞号且恰好 1..200、跨实例 2×50、64 并发 seen_event False 恰好 1 次、recover 三条）、t18 的 store 用例（`PRAGMA journal_mode` 查出来是 `delete`；写满 50 条后直接丢弃连接再新开 → `PRAGMA integrity_check == ok`、去重键全在、号不回退；只读目录 → `Err` 不假装 False）。

### ② `aite-evidence`：`FileEvidenceWriter`
- `FileEvidenceWriter::new(evidence_dir: impl Into<PathBuf>) -> Self`；`impl EvidenceWriter`；`task_dir / events_path / manifest_path`。
- `append`：`tokio::sync::Mutex` 串行；文件 IO 放 `spawn_blocking`；顺序 `heal_torn_tail → chain_tip（缓存 + 文件回读）→ canonical_json / payload_hash → >64KB 外置 payloads/{seq}.json → EvidenceEvent{created_at: Utc::now()} → append 模式一次 write(json + "\n") → tip 更新`。**jsonl 每行就是 `serde_json::to_string(&EvidenceEvent)`**，字段名与 Python 一致（`payload_ref` 内联时是 `null`，不是省略）。
- `finalize`：也先 heal；manifest **恰好 8 键**、缺省空串、`contract_version = CONTRACT_VERSION`、`root_hash` 最后一条 hash（空链 GENESIS）、`event_count`；写成 `serde_json` pretty（2 空格）+ 键排序 + 末尾换行。
- `verify`：同步、只读、不自愈、不加锁；逐行校验 task_id / seq 连续 / payload 可解析（内联或外置）/ payload_hash / prev_hash / hash；**空行让后续 seq 对不上 → false**（"events.jsonl 不许有空行"）。
- `heal_torn_tail`：口径逐字照 Python docstring（换行符是记录终止符；末段能解析且接得住链 → 补换行 `torn_tail_kept`；否则截掉 `torn_tail_dropped`；**按字节切不 decode 全文**；热路径一次 stat + 读末字节；处理后清 tip 缓存）；计数器可读；WARNING 日志含字节数、残行前缀（≤200）、路径。
- 测试：`test_chain.py` 10、`test_manifest.py` 3、`test_torn_tail.py` 15（含「健康文件 5 次 append 不读全文」—— 用注入的读计数或 `fs` 包装验证）。

### ③ `aite evidence show`（对应 `scripts/evidence_show.py`）
- `aite_evidence::cli::run(args)`：`show [task_id] [--dir D] [--root R] [--config C] [--list] [--only k1,k2] [--tail N] [--json]`；退出码 0 链过 / 1 链断或 manifest 对不上或 payload 缺 / 2 找不到任务或参数错。
- `--list`：表格「最后写入 / 任务 / 事件数 / 终态 / manifest / 链」按 mtime 倒序；JSON `{root, tasks:[{task_id, dir, mtime, events, terminal, finalized, chain_ok}]}`；任一链断非零退出。
- 单任务：文本形态逐 kind 一行人话（checklist 的 id→文本跨事件记）；`--json` 形状照清单 §8。
- 三条纪律：链校验是骨头（断在第几条、期望/实际）；没 manifest 也能渲染（"未 finalize"，ok 不受影响）；不打印密钥/token/消息全文（白名单字段；工具参数键名撞 `token/secret/password/passwd/api_key/apikey/credential/auth` → `***`；长参数截断）。
- `--config` 用来取 `storage.evidence_dir` 与模型单价（`AiteConfig::from_yaml_str`）。
- 测试：`test_evidence_show.py` 32 条 —— **证据一律用你自己的真 `FileEvidenceWriter` 往 tempdir 写**，不手搓 jsonl。
- **互通验证**：用 Python 版写一份证据目录，Rust 的 `verify` 与 `show` 必须认：
  ```bash
  /Users/shensikai/Documents/Aite/.venv/bin/python - <<'PY'
  import asyncio, tempfile
  from aite.evidence.writer import FileEvidenceWriter
  d = tempfile.mkdtemp(prefix="aite-py-evidence-")
  async def main():
      w = FileEvidenceWriter(d)
      await w.append("t1", "task_created", {"session_id": "s1", "task_no": "#A1", "chat_id": "oc", "created_by": "ou", "title": "画图"})
      await w.append("t1", "model_call", {"model": "m", "step": 0, "messages_hash": "0"*64, "usage": {"input_tokens": 1, "output_tokens": 2, "cached_tokens": 0}, "finish_reason": "stop"})
      await w.append("t1", "delivered", {"artifacts": 0, "missing": [], "steps": 1})
      print(await w.finalize("t1", {"session_id": "s1", "task_no": "#A1", "created_by": "ou", "model": "m"}))
  asyncio.run(main()); print(d)
  PY
  core/target/debug/aite evidence show t1 --root <上面打印的目录>     # 期望退出 0，链 OK
  ```
  反过来也验一遍：Rust 写的目录用 Python 的 `FileEvidenceWriter(d).verify("t1")` 是 True。

## 纪律

1. 契约与锁：`OK 36 files` 全程不变。
2. 白名单之外只读；缺接口 / 要依赖 → **停下报告**。
3. 阻塞 IO 不在 async 上下文直接做；测试不靠真实 sleep（时钟注入或 `tokio::time::pause`：在自己 Cargo.toml 的 `[dev-dependencies]` 给 `tokio` 加 `features = ["test-util"]` 即可，不动 workspace 文件）。
4. `cargo clippy -p aite-store -p aite-evidence --all-targets -- -D warnings` 干净；格式化只对自己的文件：`rustfmt --edition 2024 $(git ls-files 'core/crates/store/**/*.rs' 'core/crates/evidence/**/*.rs')`（**整树 `cargo fmt` 守卫会拦**，`cargo fmt --check` 可以）。
5. 每条结论挂实测。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r3/core
cargo test -p aite-store -p aite-evidence 2>&1 | grep -E '^test result'        # 期望全部 ok，一条不许 failed
cargo clippy -p aite-store -p aite-evidence --all-targets -- -D warnings          # 期望退出 0
cd .. && scripts/check.sh --quick                                                 # 期望 全部通过
core/target/debug/aite contracts lock --check                                     # 期望 OK 36 files
core/target/debug/aite evidence show --help                                       # 期望打印用法，退出 0
```

## 回执格式

```
## R3 回执

基线 c9d96d2 → 提交 <短 sha>

### 移植对照表
| Python | Rust | 说明 |
（test_persistence 6 + test_store_concurrency 13 + t18 store 用例 N → aite-store M 条；tests/evidence 28 → aite-evidence M 条；test_evidence_show 32 → cli 测试 M 条）

### 与 Python 行为的差异（逐条；没有就写"没有"）
- busy_timeout 5000：<>
- 其他：<>

### 互通验证
Python 写 → Rust 读：<命令 + 输出>
Rust 写 → Python verify：<命令 + 输出>

### 改了什么
- <文件> <一句话>

### 实测输出（粘实际的）
$ cargo test -p aite-store -p aite-evidence | grep 'test result'
<粘>
$ scripts/check.sh --quick
<最后 3 行>
$ core/target/debug/aite contracts lock --check
<那一行>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-r3` 分支上，回执贴出来。
