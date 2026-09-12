# 任务 R7 — Rust 官方测试替身 + 评测 runner（场景 / checks / probe）+ demo-fixture

## 背景：这轨是从哪来的

Aite 决定整体用 **Rust + Go** 重写（权威文档 `docs/dev-spec-2026-09-11-rustgo.md`，先通读 §1–§3、§5–§7）。
Python 版 P0 已经跑通（T0–T24），**它是规格与参考，只读**。R0 已合入 main（`c9d96d2`）。

**你这轨是验收面本身**：`aite/testing/*.py`（约 1200 行）→ `core/crates/testing`；`aite/evals/*.py`（约 2300 行）+ `scripts/demo_fixture.py` → `core/crates/evals`。
`evals/p0/*.yaml` 十个场景文件**原样不动**，你的 runner 必须原样读得懂；RΩ 合流后 B8 要 `passed 10/10`。
并行期间真的 ControlPlane（R4）、worker（R5）不存在：runner 通过注入的 `PlaneFactory` 拿到 plane，你自己写一个 `DemoPlane` 把 01/02/09/10 跑绿证明 harness 不是空壳（Python T4 就是这么做的）。

移植清单：`review/inventory-gateway-evals.md` §5（**每个替身的语义与记账面，逐条**）、§6（场景 schema、runner 流程、checks 全集、CLI、probe、real_stack）、§7（10 个场景）、§8（demo_fixture）、§10（`tests/e2e` 与 `tests/tools` 逐条）、§11 第 13–28、35–36、40 条。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-r7
分支     : task-r7
基线     : c9d96d2   ← main 的 HEAD（完整 sha c9d96d29d6d7ad835deef9fc80ec365c786fe876）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin`（已写进 `~/.bash_profile`）。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r7
git log --oneline -1                                   # 期望 c9d96d2 ...
git status --short                                     # 期望空
scripts/check.sh --quick                               # 期望最后一行 "全部通过"（首次 cargo 全量编译 2–5 分钟）
core/target/debug/aite contracts lock --check          # 期望 OK 36 files
core/target/debug/aite evals run evals/p0 --list; echo "exit=$?"   # 期望 not implemented，exit=2（这就是你要替换的）
ls evals/p0 | wc -l                                    # 期望 10
```

`scripts/check.sh --quick` 关键行期望：`contracts passed=25 failed=0`、`cargo passed=35 failed=0`。

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦停下报告。

## 可写路径（白名单，之外一律只读）

```
core/crates/testing/**      ← Cargo.toml（只能引用 workspace 已钉的库）、src/**、tests/**
core/crates/evals/**        ← 同上；cli 子模块放这里
evals/README.md             ← 命令改成 Rust 口径
evals/live-report-*.md      ← 只许新增，不改旧的
```

**邻居（只读）**：
- `core/crates/contracts/src/ports.rs`：八个 trait —— 替身实现 `PlatformPort / ModelPort / SandboxPort / ToolGateway / SessionStore / EvidenceWriter`；runner 只依赖 `ControlPlane` trait。
- `core/crates/app/src/main.rs`（RΩ）：`aite evals <args>` 原样转发到 `aite_evals::cli::run(args: Vec<String>) -> i32`。你还要暴露 `cli::run_with_factory(args, Option<PlaneFactory>) -> i32`，`run(args) = run_with_factory(args, None)`；RΩ 会在 app 里把真 plane 的工厂传进来。`None` 时每个场景以 `phase="wiring"` 失败、原因写明「ControlPlane 未接线（RΩ）」，退出码 1，最后一行 `passed 0/10`。
- 依赖：workspace 已钉 `serde / serde_json / serde_yaml`、`tokio`、`chrono`、`clap`、`async-trait`、`thiserror`、`sha2/hex`（evals 若要 hash 指纹：在自己 Cargo.toml 引 workspace 里已有的 `sha2`、`hex`）。**png 手写**（`samples::png_bytes`：magic + IHDR + IDAT(zlib) + IEND + CRC32）—— 没有 `flate2` / `crc32fast`，需要自己实现 stored（非压缩）deflate 块 + adler32 + crc32 表，几十行；要引库 → 停下报告。
- `aite-testing` 是 R7 自己的，`aite-evals` 可以依赖它（workspace 里已允许）；**不要依赖 aite-control / aite-worker / aite-gateway**。
- Python 参考：`aite/testing/**`、`aite/evals/**`、`scripts/demo_fixture.py`、`tests/e2e/**`、`tests/tools/test_demo_fixture.py`、`evals/p0/*.yaml`、`evals/README.md`。

## 要做什么

### ① `aite-testing`（清单 §5 逐条，替身只记账不断言）
- `CallLog` / `Call{method, kwargs(JSON), result, error, seq}`，**seq 全局单调**（跨替身排序）。
- `FakePlatform`：`FAKE_P0`；注入 `history / documents / files`；**单一共享计数器**假 id（`msg-1 / card-2 / file-3`）；`send_card` 返回 `card_id == message_id`；`cards` 快照（深拷贝追加）；三条做严（未知 card_id 报错、`read_history` 不过滤、缺文档/附件报错并列出已备键）；`fail_next`；`emit(ev)`（没 `start` 过报错）；断言面 `count / card_count / update_count / outbound_count / texts / card_snapshots`。
- `FakeModel`：`ScriptStep{text, tool_calls[{name, arguments, call_id}], usage, finish_reason, repeat(1|"inf"), error, hold_ticks}`（serde 从 YAML 反序列化：`repeat` 接受整数或字符串 `"inf"`，负 `hold_ticks` 加载期拒绝）；`hold_ticks` 让出 N 个 `tokio::task::yield_now()`（不是墙钟；`release_holds()` 可提前放行）；用尽 → `ScriptExhausted` 人话；记账与断言面。
- `FakeSandbox`：内存 FS + `ExecScriptStep{match, exit_code, stdout, stderr, duration_ms, truncated, writes, error, times}`；未匹配返回无害默认值；`as_bytes` 四种写法（`builtin:png` / `{"b64":..}` / `{"builtin":..}` / str）；路径只查 `/work` 前缀；`release` 幂等、已 release 再用报错；时钟注入；`alive / all_files / released_ids`。
- `FakeToolGateway`：token 可选校验；`jsonschema_mini`（关键字集合与不拒未知参数照 Python）；超时；错误分类；**content 排版逐字照清单 §5 那张表**（场景断言耦合它：`run_python` 是 `exit_code=0\n--- stdout ---\n…`）；实现契约里的 `register_task / unregister_task / sandbox_id_of / release_task`。
- `FakeSessionStore`（深拷贝语义、四条保证）、`FakeEvidenceWriter`（真算链、`verify`、`finalize`）。
- `samples`：`png_bytes / PNG_1X1 / CSV_SAMPLE / BUILTINS`。
- 测试：`test_t4_fake_platform.py` 14、`fake_model` 20、`fake_sandbox` 12、`fake_store` 12、`fake_gateway` 18（= 76）。

### ② `aite-evals`
- `scenario.rs`：YAML schema 全键（清单 §6）用 serde 结构体 + 默认值；`name` 必须等于文件名 stem；`config` 片段叠在 `AiteConfig`（`platform: fake`）上；`BASE_TIME = 2026-09-09T09:00:00Z`；events 能构造成合法 `NormalizedEvent`。
- `runner.rs`：`Deps{platform, model, sandbox, gateway, store, evidence, config}` + 探针（`ModelProbe` 的 `in_flight / awaiting_retry / observations`）；`PlaneFactory = Arc<dyn Fn(&Deps) -> Result<Arc<dyn ControlPlane>, String> + Send + Sync>`；流程 `build_deps → plane → store.init + platform.start(plane.handle_event 包成 EventHandler) → spawn run_forever（让两个 tick，立刻带错收场 → drive）→ 逐条投事件（`after` 三档：none / idle / running；等不到 → phase dispatch）→ settle（优先 `run_pending`/`join`；否则轮询 5ms 记账不变 + 不 busy 持续 150ms → quiesce；超时 → drive）→ 取消 loop → checks（assert / ok）→ 收尾`；`ScenarioResult / SuiteResult` JSON 形状逐字；**每个场景报人话原因不 panic**；`--timeout-scale`。
- `checks.rs`：全部 check 类型与比较子（清单 §6 表），未知 check / 畸形条目 / 缺比较子 → 失败行不抛。
- `protocol_probe.rs`：`ModelProbe` 透明包装 + `Observation` + 切轮两判据 + `analyze` 输出形状 + `render_digest` 到 stderr + 指纹算法（与 worker 的 `_call_signature` 逐字同构：`"{name}:{json(顶层 key 排序的 arguments, 不转义中文)}"`；R5 会公开 `worker::fingerprint`，RΩ 对拍）。
- `real_stack.rs`：`--sandbox docker` 的接线点做成注入（`SandboxFactory / GatewayFactory`），并行期间没有真实现 → 一行人话 + 退出 2；`--model live` 同理（`ModelFactory` 注入；RΩ 接 `aite-models`）；探针 `SandboxProbe / GatewayProbe` 现在就写好。
- `cli.rs`：`aite evals run <suite> [--platform fake] [--model scripted|live] [--sandbox fake|docker] [--list] [--only NAME]… [--json PATH] [--traceback] [--config PATH] [--protocol-report [PATH]] [--timeout-scale K]`；stdout = JSON 摘要 + 最后一行 `passed k/n`；退出码 0/1/2 规则；scripted 路径 stderr 为空；`aite evals demo-fixture csv|history|all …`（对应 `scripts/demo_fixture.py`，同参字节一致，默认落 `/tmp/aite-demo/`）。
- `DemoPlane`（`src/demo_plane.rs`，只为自证）：能跑 01 / 02 / 09 / 10（去重、忽略 bot、单步 final、话题续接）。
- 测试：`test_t4_evals_runner.py` 22、`test_t4_evals_scenarios.py` 19、`test_t4_dispatch_timing.py` 6 + `test_t5_event_timing.py` 11、`test_t12_protocol_probe.py` 里不需要真 plane 的部分（探针转发 / 切轮 / 报告序列化 / CLI 五条 / `repeat_loops` 四条）、`test_demo_fixture.py` 20。

## 纪律

1. 契约与锁：`OK 36 files` 全程不变；`evals/p0/*.yaml` 一个字不动。
2. 白名单之外只读；缺接口 / 要依赖 → **停下报告**。
3. 替身只记账不断言；`aite-evals` 不依赖 R4/R5/R6 的 crate。
4. 测试不靠真实 sleep（`hold_ticks` 是 tick；settle 的轮询用 `tokio::time::pause` 或注入时钟，dev-dependencies 给 tokio 加 `test-util`，只改自己的 Cargo.toml）。
5. `cargo clippy -p aite-testing -p aite-evals --all-targets -- -D warnings` 干净；格式化只对自己的文件：`rustfmt --edition 2024 $(git ls-files 'core/crates/testing/**/*.rs' 'core/crates/evals/**/*.rs')`（整树 `cargo fmt` 守卫会拦）。
6. 每条结论挂实测。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r7/core
cargo test -p aite-testing -p aite-evals 2>&1 | grep -E '^test result'     # 期望全部 ok，一条不许 failed
cargo clippy -p aite-testing -p aite-evals --all-targets -- -D warnings       # 期望退出 0
cargo build --workspace && cd ..
core/target/debug/aite evals run evals/p0 --list                             # 期望 JSON 数组 10 个名字，退出 0
core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>/tmp/r7.err; echo "exit=$?"; wc -c /tmp/r7.err
                                                                             # 期望：JSON 摘要 + 最后一行 passed 0/10，exit=1，stderr 0 字节，每个场景 phase=wiring 且 reason 提到 RΩ
core/target/debug/aite evals demo-fixture all -d /tmp/aite-demo && ls -la /tmp/aite-demo   # 期望 sales.csv 与 history.txt
scripts/check.sh --quick                                                     # 期望 全部通过
core/target/debug/aite contracts lock --check                                # 期望 OK 36 files
```

## 回执格式

```
## R7 回执

基线 c9d96d2 → 提交 <短 sha>

### 移植对照表
| Python 测试文件 | Rust 测试 | 条数 | 差异说明 |
（fake_* 76 + runner 22 + scenarios 19 + timing 17 + probe 部分 + demo_fixture 20 → M 条）

### DemoPlane 跑绿了哪几个场景
<01/02/09/10 的实测输出>

### 与 Python 行为的差异（逐条；没有就写"没有"）
- 场景 YAML 里哪些键你解析时做了什么假设：<>

### 给 RΩ 的接线说明
PlaneFactory / ModelFactory / SandboxFactory / GatewayFactory 的签名与调用点：<>

### 改了什么
- <文件> <一句话>

### 实测输出（粘实际的）
$ cargo test -p aite-testing -p aite-evals | grep 'test result'
<粘>
$ core/target/debug/aite evals run evals/p0 --platform fake --model scripted | tail -1
<>
$ scripts/check.sh --quick
<最后 3 行>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-r7` 分支上，回执贴出来。
