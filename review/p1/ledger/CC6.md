# CC6 回执：国产模型加固 + 进程内思考字段回挂缓存

- 轨号：CC6（第 1 波 · Claude Code 云端）· 派单 `review/paste-CC6.md` · 日期 2026-09-28
- 分支：`claude/cc6-vendor-hardening`（基于 `main` = `30b00e52`，即 `8458435` + D0 文档提交）
- 结论：开场自检 5 项全对上（**情形 A**）；8 个工作项全做；新增 13 条测试（+1 条 ignored 的 live），14 次变异全红；
  `scripts/check.sh` 全部通过，`cargo passed=959 failed=0`（= 946 + 13）。
- **推送 / PR 没做成**：这个云端会话的仓库**没有配置任何 git remote**（`git remote -v` 为空，`git ls-remote origin` →
  `fatal: 'origin' does not appear to be a git repository`），环境里也**没有 `gh` 命令**（`gh: command not found`）。
  提交已落在本地分支 `claude/cc6-vendor-hardening` 上；draft PR 的描述已写好（见文末「PR 描述摘要」，同内容另存 `/tmp/cc6-pr.md`），
  等总管拉取分支后照 `gh pr create --draft --title "CC6: 国产模型加固 + 思考字段回挂缓存" --body-file /tmp/cc6-pr.md` 开 PR。

## 1. 开场自检原文

### 第 1 步：代码基线

```
$ git cat-file -e 8458435 || git fetch -q --unshallow origin      # cat-file 成功，没走 fetch
$ git diff --stat --no-renames --diff-filter=AM 8458435 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'
（空输出）
```

HEAD = `30b00e52 docs(p1): 第 1 波剩余 9 份派单刷新到 CC1–CC4 合并后的基线 8458435（946）+ T0 并入 W1 契约缺口`。
判定：**情形 A**，基线行 = `cargo passed=946 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包。

### 第 2 步：守卫（被拦 = 通过）

Read 工具读 `.claude/hooks/guard_bash.py`，拦截原文逐字：

```
PreToolUse:Read hook error: [d=$(git rev-parse --show-toplevel 2>/dev/null); [ -f "$d/.claude/hooks/guard_bash.py" ] || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"]: blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。
```

### 第 3 步：工具链

```
libprotoc 31.1
rustc 1.98.1 (48a229cea 2026-09-01)
go version go1.27.1 linux/amd64
protoc-gen-go v1.36.12
protoc-gen-go-grpc 1.6.2
```

### 第 4 步：基线 `scripts/check.sh`（在 `30b00e52` 上，改代码之前；Read 读出的全文）

```

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
   Compiling aite-models v0.0.1 (/home/user/repo/core/crates/models)
   Compiling aite-gateway v0.0.1 (/home/user/repo/core/crates/gateway)
   Compiling aite-search v0.0.1 (/home/user/repo/core/crates/search)
   Compiling aite-githost v0.0.1 (/home/user/repo/core/crates/githost)
   Compiling aite-memory v0.0.1 (/home/user/repo/core/crates/memory)
   Compiling aite-routines v0.0.1 (/home/user/repo/core/crates/routines)
   Compiling aite v0.0.1 (/home/user/repo/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 53s
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
    Checking aite-models v0.0.1 (/home/user/repo/core/crates/models)
    Checking aite-gateway v0.0.1 (/home/user/repo/core/crates/gateway)
    Checking aite-routines v0.0.1 (/home/user/repo/core/crates/routines)
    Checking aite-memory v0.0.1 (/home/user/repo/core/crates/memory)
    Checking aite-githost v0.0.1 (/home/user/repo/core/crates/githost)
    Checking aite-search v0.0.1 (/home/user/repo/core/crates/search)
    Checking aite v0.0.1 (/home/user/repo/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 13s
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
cargo passed=946 failed=0
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

各行与情形 A 基线逐字一致。check.sh 已是 CC1 之后的新口径，Go 那格直接打 `go packages ok=9 fail=0`，以这行为准（不再需要单跑 `cmd/...` 补数）。

### 第 5 步：`cargo test -p aite-models` 基线

```
     Running unittests src/lib.rs (target/debug/deps/aite_models-b3e44b2f4fabad88)
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
     Running tests/test_openai_compat.rs (target/debug/deps/test_openai_compat-d37c74a0fe25dc98)
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.13s
   Doc-tests aite_models
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

合计 `passed=27 failed=0`，与派单按源码数的一致。

## 2. 工作项逐条

（行号是本分支提交后的。）

| # | 做了什么 | 文件:行 |
|---|---|---|
| 1 厂商识别 | 新 `Vendor` 七档 + `as_str()`；`infer_vendor(model, base_url)`：先模型名前缀（不分大小写），再 `reqwest::Url::parse` 取 host；解析失败 / 无 host → 只按前缀；都不中 → `Generic`；无 `unwrap`/`expect`。模块头 `//!` 写了 2026-09-25 DeepSeek 实测三条（注明总管本机真 key、云端未复测）。`from_config` 算一次存进结构体，`with_base_url` 不重算；加 `pub fn vendor()`；`lib.rs` 另 `pub use vendor::{Vendor, infer_vendor}` | `src/vendor.rs:1-84`；`src/lib.rs:23-26`（mod / re-export）、`:327`（字段）、`:359`（算一次）、`:365`（`vendor()`） |
| 2 请求体 | Kimi：不插 `temperature` 键；Qwen：有工具时加 `parallel_tool_calls: true`，无工具不加；GLM：`tool_choice` 仍恒为 `"auto"`（今天就是，这里只加注释 + 测试钉住）；其余厂商（含 Generic）不变 | `src/lib.rs:403-415` |
| 3 响应侧 | `chat()` 在 `turn_from_response` 之后调 `capture_thinking`：从 `choices[0].message` 原样取 `reasoning_content` / `encrypted_content` / `reasoning_details`（null / 空串 / 空数组 / 空对象视为没有），非空写 `raw["provider_extra"]`；MiniMax 剥 content 开头的 `<think>…</think>`（连同其后空白），剥下的正文进 `provider_extra["reasoning_content"]`；别家 content 一字不动。`turn_from_response` 签名未动 | `src/lib.rs:441-461`、`:547`；`src/echo.rs:58-78`（`capture`）、`:82-90`（`split_think`） |
| 4 回挂缓存 | `OpenAiCompatModel` 加 `echo: Mutex<EchoCache>`（照 `acct` 的写法）。写入：带 `tool_calls` 且有思考字段时，每个 `call_id` 存一条（字段 + 指纹 + MiniMax 前缀）。读出：`request_body` 里 `to_openai_messages` 翻完后（与输入一一对应）逐个带 `tool_calls` 的 assistant 行查缓存，命中且指纹一致才插，已有同名键不覆盖；读了不删 | `src/lib.rs:329`、`:360`、`:397`、`:421-437`；`src/echo.rs:103-106`（`fingerprint`）、`:125-145`（`remember`）、`:150-170`（`reattach`） |
| 5 缓存命中 | `usage_from_openai`：`prompt_tokens_details.cached_tokens` → `prompt_cache_hit_tokens` → 顶层 `cached_tokens`，先出现的为准，都没有 → 0；签名不变；`cost_of` 未动 | `src/lib.rs:121-129` |
| 6 超时 | `REQUEST_TIMEOUT_SEC` 120 → 600，注释写明来源（卡片 / 总计划 §6.1，与 T0 `timeout_sec` 默认一致；思考模型常超 120 秒）；加常量钉 `tests::request_timeout_is_600_seconds`。preflight 的 60 秒未碰 | `src/lib.rs:38-42`、`:661-664` |
| 7 错误文本 | 非 2xx：`HTTP {status}: {detail}`（半角冒号 + 空格；detail 照旧 clip 500 + redact）；429 且拿得到 n 时在 clip 之后追加 ` retry-after-ms={n}`。n：`retry-after-ms` 整数毫秒优先，其次 `retry-after` 整数秒 ×1000（`checked_mul`）；HTTP-date / 解析不了 → 不追加。响应头在 `text()` 之前取。假服务加 `start_with_headers`（`start` / `start_raw` 签名不动，`start_raw` 改为委托给它） | `src/lib.rs:511-515`、`:525-538`、`:565-573`；`tests/common/mod.rs:35-36`、`:59-68`、`:88-106` |
| 8 夹具 + live | `tests/fixtures/` 六家响应 + `deepseek_models.json` + `README.md`（每份来源、日期，写明「不是云端真录的，云端没有 key」）；`tests/cc6_live.rs` 的 `live_model_tool_roundtrip`：`#[ignore = "…"]`，`AITE_LIVE_MODEL != "1"` 立即返回；key 只从 `AITE_MODEL_API_KEY`；端点 / 模型默认 `https://api.deepseek.com` + `deepseek-flash`，可用 `AITE_LIVE_BASE_URL` / `AITE_LIVE_MODEL_NAME` 覆盖；跑一轮带工具 + 回挂的第 2 轮，`eprintln!` 打两轮响应的 `model` 字段 | `tests/fixtures/*`、`tests/cc6_live.rs` |

**`Vendor` 字符串表**（与 T0 `ModelVendor` 前 7 个逐一相同；`selfhost` 不加，`infer_vendor` 永不返回它）：

| 变体 | `as_str()` | 前缀（不分大小写） | host |
|---|---|---|---|
| `Generic` | `generic` | — | 其余一切 / 解析不了 |
| `Deepseek` | `deepseek` | `deepseek` | `api.deepseek.com` |
| `Qwen` | `qwen` | `qwen` | `*.maas.aliyuncs.com`、`dashscope*.aliyuncs.com` |
| `Glm` | `glm` | `glm` | `open.bigmodel.cn` |
| `Kimi` | `kimi` | `kimi`、`moonshot` | `api.moonshot.cn` |
| `Doubao` | `doubao` | `doubao` | `ark.cn-beijing.volces.com` |
| `Minimax` | `minimax` | `minimax` | `api.minimax.cn` |

**`ECHO_CACHE_CAP = 1024`**（`src/echo.rs:26`）：一个任务最多 `max_steps` 步（默认 40，`config/aite.example.yaml`），每步至多一批 tool_call；
T0c 之后并发默认 4 → 40 × 4 = 160 条装得下所有在跑任务的整段历史；1024 留约 6 倍余量（一步多个并行 tool_call、任务首尾交叠）。
满了先进先出淘汰；同一 `call_id` 再来一次（按序号编 id 的厂商）→ 覆盖并挪到队尾，队列里不留旧键。不引入 LRU 库。

**MiniMax 回挂形状的决定**：照派单做——把剥下来的**原样前缀**（`<think>…</think>` 连同其后的空白，逐字节就是它返回的样子）拼回该 assistant 行 content 开头，
**不**加 `reasoning_content` 字段；行 content 已以 `<think>` 开头就不再拼。`provider_extra["reasoning_content"]` 里放的是标签之间的正文（不含标签、不 trim）。
另外几个边界的取舍（拿不准，照最字面的读法做了）：
- `<think>` 必须在 content 的**最开头**（前面有空白就不剥）；没有闭合标签也不剥。
- MiniMax 若同时返回原生 `reasoning_content`（或 `reasoning_details`）：原生的进 `provider_extra` 且作为字段回挂；剥下的正文**不覆盖**原生 `reasoning_content`，但仍以 content 前缀回挂。
- 一条 assistant 行有多个 tool_call：任一 `call_id` 命中且指纹一致即回挂（同一批 tool_call 存的是同一份思考字段）。

**Generic 档字节一致**：Generic 的请求体构造路径与 CC6 之前是同一段代码（`temperature` / `tools` / `tool_choice` 照旧；Qwen / Kimi 的分支对它不生效）；
回挂只在缓存有条目时插键，而缓存只在响应带思考字段 + `tool_calls` 时写入，所以「Generic 且响应从不带思考字段」时请求体不变。
全量 946 条原测试（含 `preflight_e2e.rs`）照绿即是佐证。

## 3. 新增测试逐条 + 变异验证

| 测试 | 位置 | 钉什么 |
|---|---|---|
| `kimi_request_omits_temperature` | `tests/cc6_vendors.rs:112` | 两条识别路径（百炼域名 + `kimi-` 前缀；`api.moonshot.cn` + 无前缀 `k2-turbo-preview`）`vendor()==Kimi` 且请求体无 `temperature` 键；对照：Generic 照发 |
| `qwen_sets_parallel_tool_calls` | `tests/cc6_vendors.rs:148` | 有工具 → `parallel_tool_calls == true`；无工具 → 键不存在。顺带：Qwen 夹具的 `arguments` 是**对象**，照收成 `{"q":"北京天气"}`、无 `arg_parse_errors` |
| `glm_tool_choice_auto_only` | `tests/cc6_vendors.rs:181` | GLM 带工具 `tool_choice == "auto"`，不带工具键不存在 |
| `reasoning_captured_in_raw` | `tests/cc6_vendors.rs:203` | DeepSeek `reasoning_content`、豆包 `encrypted_content` 原样进 `raw["provider_extra"]`（整个对象相等）；GLM 无思考字段 → 无此键；顺带 DeepSeek 夹具 `cached_tokens == 1024` |
| `minimax_think_stripped` | `tests/cc6_vendors.rs:250` | MiniMax：content 剥净、剥下正文在 `provider_extra`；第 2 轮 assistant 行 content 原样拼回 `<think>…</think>\n\n…`、无 `reasoning_content` 字段；Generic 收到同样文本不剥、无 `provider_extra` |
| `deepseek_thinking_tool_roundtrip_reattaches_reasoning` | `tests/cc6_vendors.rs:297` | 两轮：第 1 请求无思考字段；第 2 请求 assistant 行带原样 `reasoning_content`；夹具 `arguments` 是带空格的 `"{\"q\": 1}"`、请求侧重新序列化为 `{"q":1}`（指纹误用原始串就回挂不上）；探测事实 2：`raw["model"]=="deepseek-flash"` 而 `name()=="deepseek-chat"`；第 2 轮 200（事实 3）；顶层键集合两轮相同；无思考字段时第 2 轮 assistant 行键集合恰为 `{content, role, tool_calls}` |
| `cache_hit_field_variants` | `tests/cc6_vendors.rs:375` | 三种形状各一 + 都没有 → 0 + 优先级（先出现的为准）+ details 在但缺 `cached_tokens` 时往下找 + Kimi / DeepSeek 夹具实形 |
| `http_status_and_retry_after_in_error_text` | `tests/cc6_vendors.rs:439` | 429 + `Retry-After: 2` → `HTTP 429: ` 开头、` retry-after-ms=2000` 结尾、外层前缀 `模型服务错误：HTTP 429: `；429 无头不追加；500 → `HTTP 500: `；`retry-after-ms` 头优先于 `Retry-After`；HTTP-date 不追加；401 → `HTTP 401: `、无全角冒号；全程密钥不出现（detail 里的回显被抹成 `***`） |
| `retry_after_survives_a_long_detail`（另加） | `tests/cc6_vendors.rs:509` | 2000 字符的 429 响应体：detail 截到 500，` retry-after-ms=3000` 追加在截断之后、永不被截（整串相等） |
| `tests::request_timeout_is_600_seconds`（另加） | `src/lib.rs:661` | 常量钉 600 |
| `vendor::tests::vendor_inference_prefix_wins_over_host`（另加） | `src/vendor.rs:91` | 百炼域名下 Kimi / GLM / DeepSeek 前缀优先；六家 host；非法 URL / 空串 → 只按前缀、否则 Generic；`as_str` 七个字符串 |
| `echo::tests::echo_cache_evicts_oldest_first_and_stays_bounded`（另加） | `src/echo.rs:205` | 写入 1034 条后恰 1024 条、最老的先淘汰、读了不删、同 id 覆盖并挪到队尾 |
| `echo::tests::echo_cache_requires_matching_fingerprint`（另加） | `src/echo.rs:230` | 同 id（Kimi 式 `search:0`）不同参数不挂；一致才挂；行里已有同名键不覆盖 |

**变异验证**：脚本逐条注入变异 → `cargo test -p aite-models -- --exact <点名测试>` → 期望红 → `git checkout -- <文件>` 还原（没用 `cp -p` / `shutil.copy2`）；
跑完 `git status --short` 为空。14 次变异全红，输出原样（每条只保留 test / panicked / assertion / result 行；M1、M9 的 `assert!` 消息另跑一次补全）：

```
## M1 Kimi 照发 temperature（core/crates/models/src/lib.rs）exit=101        # `if self.vendor != Vendor::Kimi {` → `if true {`
test kimi_request_omits_temperature ... FAILED
thread 'kimi_request_omits_temperature' (24555) panicked at crates/models/tests/cc6_vendors.rs:131:9:
Kimi 传 temperature 会 400：kimi-k2-turbo-preview @ https://ws-abc.cn-beijing.maas.aliyuncs.com/compatible-mode/v1 发了 Some(Number(0.3))
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.20s

## M2 Qwen 不加 parallel_tool_calls（core/crates/models/src/lib.rs）exit=101   # `if self.vendor == Vendor::Qwen {` → `if false {`
test qwen_sets_parallel_tool_calls ... FAILED
thread 'qwen_sets_parallel_tool_calls' (15092) panicked at crates/models/tests/cc6_vendors.rs:161:5:
assertion `left == right` failed: Qwen 的 parallel_tool_calls 默认关，有工具要显式打开
  left: None
 right: Some(Bool(true))
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.19s

## M3 GLM tool_choice 注入 required（core/crates/models/src/lib.rs）exit=101     # 人为注入违例
test glm_tool_choice_auto_only ... FAILED
thread 'glm_tool_choice_auto_only' (15820) panicked at crates/models/tests/cc6_vendors.rs:190:5:
assertion `left == right` failed: GLM 的 tool_choice 只支持 auto
  left: Some(String("required"))
 right: Some(String("auto"))
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.18s

## M4 不捕获思考字段（core/crates/models/src/lib.rs）exit=101               # 删掉 `self.capture_thinking(&payload, &mut turn);`
test minimax_think_stripped ... FAILED
test deepseek_thinking_tool_roundtrip_reattaches_reasoning ... FAILED
test reasoning_captured_in_raw ... FAILED
thread 'minimax_think_stripped' (16546) panicked at crates/models/tests/cc6_vendors.rs:265:5:
assertion `left == right` failed
  left: "<think>\n用户问北京天气，先查一下。\n</think>\n\n我先查一下北京的天气。"
 right: "我先查一下北京的天气。"
thread 'deepseek_thinking_tool_roundtrip_reattaches_reasoning' (16545) panicked at crates/models/tests/cc6_vendors.rs:328:5:
assertion `left == right` failed: 第 2 个请求里那条 assistant 行要带着原样的 reasoning_content：{"content":"","role":"assistant","tool_calls":[{"function":{"arguments":"{\"q\":1}","name":"search"},"id":"call_00_ds","type":"function"}]}
  left: Null
 right: String("用户问北京天气，先调 search 查一下。")
thread 'reasoning_captured_in_raw' (16547) panicked at crates/models/tests/cc6_vendors.rs:213:5:
assertion `left == right` failed: DeepSeek 的 reasoning_content 原样进 raw
  left: None
 right: Some(Object {"reasoning_content": String("用户问北京天气，先调 search 查一下。")})
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.18s

## M5 MiniMax 不剥 <think>（core/crates/models/src/echo.rs）exit=101          # `if vendor == Vendor::Minimax` → `if false`
test minimax_think_stripped ... FAILED
thread 'minimax_think_stripped' (17275) panicked at crates/models/tests/cc6_vendors.rs:265:5:
assertion `left == right` failed
  left: "<think>\n用户问北京天气，先查一下。\n</think>\n\n我先查一下北京的天气。"
 right: "我先查一下北京的天气。"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.18s

## M6 不回挂（core/crates/models/src/lib.rs）exit=101                     # 删掉 `self.reattach_thinking(messages, &mut rows);`
test minimax_think_stripped ... FAILED
test deepseek_thinking_tool_roundtrip_reattaches_reasoning ... FAILED
thread 'minimax_think_stripped' (18001) panicked at crates/models/tests/cc6_vendors.rs:280:5:
assertion `left == right` failed
  left: String("我先查一下北京的天气。")
 right: String("<think>\n用户问北京天气，先查一下。\n</think>\n\n我先查一下北京的天气。")
thread 'deepseek_thinking_tool_roundtrip_reattaches_reasoning' (18000) panicked at crates/models/tests/cc6_vendors.rs:328:5:
assertion `left == right` failed: 第 2 个请求里那条 assistant 行要带着原样的 reasoning_content：{"content":"","role":"assistant","tool_calls":[{"function":{"arguments":"{\"q\":1}","name":"search"},"id":"call_00_ds","type":"function"}]}
  left: Null
 right: String("用户问北京天气，先调 search 查一下。")
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 7 filtered out; finished in 0.19s

## M7 写入侧指纹换一种序列化（模拟误用原始串）（core/crates/models/src/echo.rs）exit=101   # remember 里指纹改用 to_string_pretty
test deepseek_thinking_tool_roundtrip_reattaches_reasoning ... FAILED
thread 'deepseek_thinking_tool_roundtrip_reattaches_reasoning' (18729) panicked at crates/models/tests/cc6_vendors.rs:328:5:
assertion `left == right` failed: 第 2 个请求里那条 assistant 行要带着原样的 reasoning_content：{"content":"","role":"assistant","tool_calls":[{"function":{"arguments":"{\"q\":1}","name":"search"},"id":"call_00_ds","type":"function"}]}
  left: Null
 right: String("用户问北京天气，先调 search 查一下。")
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.19s

## M8 缓存命中只认 details（core/crates/models/src/lib.rs）exit=101          # 删掉两行 or_else
test cache_hit_field_variants ... FAILED
thread 'cache_hit_field_variants' (19457) panicked at crates/models/tests/cc6_vendors.rs:389:5:
assertion `left == right` failed
  left: 0
 right: 700
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.13s

## M9 错误文本退回全角冒号（core/crates/models/src/lib.rs）exit=101          # `"HTTP {}: {}"` → `"HTTP {}：{}"`
test http_status_and_retry_after_in_error_text ... FAILED
thread 'http_status_and_retry_after_in_error_text' (26017) panicked at crates/models/tests/cc6_vendors.rs:477:5:
HTTP 429：{"error":{"message":"slow down","echo":"Bearer ***"}} retry-after-ms=2000
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.19s

## M10 不追加 retry-after-ms（core/crates/models/src/lib.rs）exit=101        # `retry_after_ms` → `retry_after_ms.filter(|_| false)`
test retry_after_survives_a_long_detail ... FAILED
test http_status_and_retry_after_in_error_text ... FAILED
thread 'retry_after_survives_a_long_detail' (20913) panicked at crates/models/tests/cc6_vendors.rs:524:5:
assertion `left == right` failed
  left: "HTTP 429: xxxx…（500 个 x，此处省略）"
 right: "HTTP 429: xxxx…（500 个 x，此处省略） retry-after-ms=3000"
thread 'http_status_and_retry_after_in_error_text' (20912) panicked at crates/models/tests/cc6_vendors.rs:478:5:
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 7 filtered out; finished in 0.18s

## M11 超时退回 120（core/crates/models/src/lib.rs）exit=101
test tests::request_timeout_is_600_seconds ... FAILED
thread 'tests::request_timeout_is_600_seconds' (21637) panicked at crates/models/src/lib.rs:663:9:
assertion `left == right` failed
  left: 120
 right: 600
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.16s

## M12 缓存不淘汰（core/crates/models/src/echo.rs）exit=101                # `while self.order.len() > ECHO_CACHE_CAP {` → `while false {`
test echo::tests::echo_cache_evicts_oldest_first_and_stays_bounded ... FAILED
thread 'echo::tests::echo_cache_evicts_oldest_first_and_stays_bounded' (22366) panicked at crates/models/src/echo.rs:210:9:
assertion `left == right` failed
  left: 1034
 right: 1024
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.14s

## M13 不核指纹（core/crates/models/src/echo.rs）exit=101                  # 删掉 `.filter(|e| e.fingerprint == fingerprint(tc))`
test echo::tests::echo_cache_requires_matching_fingerprint ... FAILED
thread 'echo::tests::echo_cache_requires_matching_fingerprint' (23090) panicked at crates/models/src/echo.rs:233:9:
assertion `left == right` failed
  left: Some(String("任务 A 的思考"))
 right: None
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.12s

## M14 去掉 kimi 前缀（只剩域名判）（core/crates/models/src/vendor.rs）exit=101
test vendor::tests::vendor_inference_prefix_wins_over_host ... FAILED
thread 'vendor::tests::vendor_inference_prefix_wins_over_host' (23814) panicked at crates/models/src/vendor.rs:93:9:
assertion `left == right` failed
  left: Qwen
 right: Kimi
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.13s

git status after restore: ''
```

（M10 那条 left/right 原文各是 500 个 `x`，回执里省略成「…」，其余逐字。M14 lib 单测先红，cargo 没接着跑集成测试。）

## 4. 验收

| # | 命令 | 实测 |
|---|---|---|
| 1 | `cd core && cargo test -p aite-models` | lib `7 passed`；`cc6_live` `0 passed; 1 ignored`；`cc6_vendors` `9 passed`；`test_openai_compat` `24 passed`；doc-tests `0`；合计 40 = 27 + 13，`failed=0`，`1 ignored` |
| 2 | 8 条点名 `--exact` | `cc6_vendors`：`8 passed; 0 failed; … 1 filtered out`（其余二进制 0/0） |
| 3 | 4 条旧密钥测试 `--exact` | lib `1 passed`、`test_openai_compat` `3 passed` → 合计 4 passed |
| 4 | `cargo test -p aite-models live -- --ignored`（未设 `AITE_LIVE_MODEL`） | `test live_model_tool_roundtrip ... ok`、`1 passed`，0.00s 立即返回 |
| 5 | `cargo clippy -p aite-models --all-targets -- -D warnings` | exit 0（首跑有一条 `type_complexity`，已把假服务的元组提成 `pub type Canned` 修掉） |
| 6 | `scripts/check.sh` | 见下节全文：`cargo passed=959 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`go packages ok=9 fail=0`、`passed 10/10`、B9 skip、`全部通过`、`exit=0` |
| 7 | `git diff --name-only main...HEAD`（没有 `origin`，用本地 `main`） | 全部以 `core/crates/models/` 开头，外加 `review/p1/ledger/CC6.md` |
| 8 | `git status --short` | 空（变异脚本、样例探针都在会话 scratchpad，仓库里的临时测试文件已删） |

## 5. check.sh 完整输出（最终树；`/tmp/cc6-check.log` 用 Read 读出的全文）

```

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
   Compiling aite-models v0.0.1 (/home/user/repo/core/crates/models)
   Compiling aite v0.0.1 (/home/user/repo/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.77s
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
    Checking aite-models v0.0.1 (/home/user/repo/core/crates/models)
    Checking aite v0.0.1 (/home/user/repo/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.90s
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
cargo passed=959 failed=0
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

B8 不变量未动（`passed 10/10`；B8 走 `--model scripted`，不经过本 crate）。

## 6. `cargo passed` 增量逐条（Δ = 13）

| 文件 | 新增测试 | 条数 |
|---|---|---|
| `core/crates/models/tests/cc6_vendors.rs` | 8 条点名：`kimi_request_omits_temperature`、`qwen_sets_parallel_tool_calls`、`glm_tool_choice_auto_only`、`reasoning_captured_in_raw`、`minimax_think_stripped`、`deepseek_thinking_tool_roundtrip_reattaches_reasoning`、`cache_hit_field_variants`、`http_status_and_retry_after_in_error_text`；另加 `retry_after_survives_a_long_detail` | 9 |
| `core/crates/models/src/lib.rs`（`mod tests`） | `request_timeout_is_600_seconds` | 1 |
| `core/crates/models/src/vendor.rs`（`mod tests`） | `vendor_inference_prefix_wins_over_host` | 1 |
| `core/crates/models/src/echo.rs`（`mod tests`） | `echo_cache_evicts_oldest_first_and_stays_bounded`、`echo_cache_requires_matching_fingerprint` | 2 |
| **合计** | | **13**（946 → 959） |
| `core/crates/models/tests/cc6_live.rs` | `live_model_tool_roundtrip`（`#[ignore]`，**不进 passed**） | 1 ignored |

现有 24 + 3 条一条断言没改。doc-tests 仍是 0（新模块文档里没有代码块）。

## 7. 错误文本约定（给 CC3 对照）

三个真实样例（假服务 + 本分支 `chat()` 实打，`Display` 全文；样例里 401 的响应体故意回显了假 key，已被抹成 `***`）：

```
模型服务错误：HTTP 401: {"error":{"message":"Authentication Fails, Your api key: *** is invalid","type":"authentication_error"}}
模型服务错误：HTTP 429: {"error":{"message":"Rate limit reached for requests","type":"rate_limit_error"}} retry-after-ms=2000
模型服务错误：HTTP 500: {"error":{"message":"The server had an error while processing your request","type":"server_error"}}
```

（429 那条的响应头是 `Retry-After: 2`。）与 `review/paste-CC3.md`（:233-236）及 main 上 CC3 已落的解析（`worker/src/loop.rs:73` 同时认半角 / 全角冒号、`:86` 取 `retry-after-ms=`；
`worker/tests/test_limits.rs:157` 用的正是 `HTTP 429: rate limited retry-after-ms=1500`）**无差异**。

## 8. 被守卫拦过的命令与拦截原文

只有开场自检第 2 步那一次（预期内，原文见 §1 第 2 步）。其余：**无**。

## 9. 记账转出去的

| 事项 | 为什么不在本轨 | 建议归哪轨 |
|---|---|---|
| 按缓存命中 / 写入与峰谷计价（`cost_of` 分档） | 需要 T0 的 `price_cached_in_per_mtok` / `price_cache_write_per_mtok` / `offpeak_price_multiplier` 与 `Usage.cache_write_tokens`；派单明令不动 `cost_of` | DD7 |
| 显式 `ModelConfig.vendor` 优先于推断（「显式非 generic 优先」），本地 `Vendor` 对到契约 `ModelVendor` | 需要 T0 的 `ModelConfig.vendor` / `ModelVendor` | T0c / DD7 |
| Kimi `max_tokens ≥ 16000`、各家思考开关参数（`thinking` / `enable_thinking` 等） | 需要 T0 的 `ThinkingMode` 与厂商档案 | DD7 |
| worker 按错误文本分类重试（400/401/403 不重试、429 按 `retry-after-ms` 退避） | 在 `core/crates/worker/**`，CC3 已在 main 上落了解析，本轨只定形错误文本 | CC3（已落，核对即可） |
| `Turn.provider_extra` 落库与追问回放（本轨只镜像进 `raw["provider_extra"]`、进程内回挂） | 需要 T0 的 `Message.provider_extra` 与存储改动 | DD4 |
| `selfhost` 档（显式 vendor 永远优先于前缀推断；不发云厂商专有参数） | 需要 T0 的 `ModelConfig.vendor` | DD7 |
| 海外端点默认拒绝（总计划 D9：只用大陆端点；`dashscope-intl`、`api.moonshot.ai`、`api.z.ai`、`api.minimax.io` 构造时报配置错，`allow_overseas_endpoint=true` 才降为告警） | 需要 T0 的 `ModelConfig.allow_overseas_endpoint` | DD7 |
| 超时改为读 `ModelConfig.timeout_sec`（本轨是常量 600） | 需要 T0 的 `timeout_sec` | T0c / DD7 |
| 429 / 401 / 400 改走结构化 `ModelError::RateLimited { retry_after_ms, .. }` / `Auth` / `BadRequest` | 契约锁定面，T0 已计划；届时本 crate 的文本形状可以退役或并存 | T0c（本 crate）+ CC3 / 后继（worker） |

（本轨 `infer_vendor` 只做识别，不因端点在海外而报错或告警。）

## 10. 没做的与原因

- **live 测试云端只验了开关**：`cargo test -p aite-models live -- --ignored` 在未设 `AITE_LIVE_MODEL` 时 `1 passed` 立即返回，没真打端点（云端没有 key，也不许放）。
  **本机人工步骤（给总管）**：`cd ~/Documents/Projects/Aite/core && AITE_LIVE_MODEL=1 cargo test -p aite-models live -- --ignored --nocapture`
  （key 放本机 `AITE_MODEL_API_KEY`；可选 `AITE_LIVE_BASE_URL` / `AITE_LIVE_MODEL_NAME` 覆盖端点 / 模型），把打印的两轮 `model` 字段记下来。
- **事实 1（`GET /models` 只剩两个 id）只落了夹具数据**：`tests/fixtures/deepseek_models.json` 存档、README 写明日期与来源，没有客户端测试 ——
  本 crate 没有 `/models` 调用，派单明令不新增 API。
- **夹具不是真录的**：六家响应都是照附录 A 官方文档形状 + 09-25 三条事实手写的最小样本（README 写明）。
- **推送与 PR**：会话仓库无 remote、无 `gh`（见文首），分支只在本地；PR 描述已备好。

## 11. 契约缺口（给 T0 / T0.1）

**无。** 本轨用到而契约暂缺的全部在 T0 已计划之内：思考字段回放靠 `Message.provider_extra`（在此之前 `ModelTurn.raw` 这个开放 Map 足够镜像，进程内缓存兜底回挂）；
缓存写入计价靠 `Usage.cache_write_tokens` + `price_cache_write_per_mtok`；厂商 / 超时靠 `ModelConfig.vendor` / `timeout_sec`；
结构化限流错误靠 `ModelError::RateLimited { retry_after_ms, message }`。

## PR 描述摘要

（同内容另存会话里的 `/tmp/cc6-pr.md`。）

- 标题：`CC6: 国产模型加固 + 思考字段回挂缓存`
- 开场自检：情形 A（`8458435` 之后只有 D0 文档提交）；守卫生效；check.sh 基线 `946/0`、`25/0`、`OK 25 files`、`ok=9 fail=0`、`10/10`；models 基线 27。
- 工作项：厂商识别（`vendor.rs`）；Kimi 省 `temperature`、Qwen `parallel_tool_calls`、GLM `auto` 钉住；思考字段进 `raw["provider_extra"]`、MiniMax 剥 `<think>`；
  按 `call_id` + 指纹进程内回挂（`echo.rs`，cap 1024 先进先出）；缓存命中三种字段；超时 600；错误文本 `HTTP {status}: {detail}` + 429 `retry-after-ms`；六家夹具 + live（ignored）。
- 新测试 13 条（8 条点名 + 5 条另加），14 次变异全红；Δ = 13，`cargo passed=959 failed=0`；live 1 ignored。
- 转出：DD7（计价分档、显式 vendor、selfhost、海外端点、思考开关、Kimi max_tokens）、T0c、DD4、CC3。契约缺口：无。
