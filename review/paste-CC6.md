# 派单 CC6：国产模型加固 + 进程内思考字段回挂缓存（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC6.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC6）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

**对应 Claude Tag 的哪几条**：CT30（多模型可选）与 CT26（按量计费，这里只做「缓存命中读得出来」）。
总计划 §2.A 的 CT30 行、CT26 行，§0「改变计划的五个发现」第 4 条，附录 A「国产模型接入要点」（总计划仍在改，只按节名引，不引行号）。

**Aite 今天的样子**（`core/crates/models/src/lib.rs`，630 行，一个 OpenAI 兼容客户端通吃所有厂商）：

- 请求体只有一种形状：`request_body`（lib.rs:361-381）**永远**发 `temperature`（:374）、有工具就发 `tool_choice:"auto"`（:375-379），
  从不发 `parallel_tool_calls`。附录 A：Kimi 传 temperature 会 400、Qwen 的 `parallel_tool_calls` 默认关、GLM 的 `tool_choice` 只能 auto。
- 思考字段全丢：`turn_from_response`（lib.rs:132-214）只取 `content`（:192-196），`reasoning_content` / `encrypted_content` /
  `reasoning_details` 一个都没留；`to_openai_messages`（lib.rs:42-82）下一轮也就无从回传。MiniMax 的 `<think>` 混在 content 里，会原样进回帖。
- 缓存命中只认一种字段：`usage_from_openai`（lib.rs:105-125）只读 `prompt_tokens_details.cached_tokens`（:109-113）；
  DeepSeek 的 `prompt_cache_hit_tokens`、顶层 `usage.cached_tokens` 都读成 0。
- 超时 `REQUEST_TIMEOUT_SEC = 120`（lib.rs:30，用在 :329-332），思考模型一轮常常更久。
- 非 2xx 的错误文本是 `"HTTP {}：{}"`（lib.rs:438-444，:440 是**全角冒号**），不带 Retry-After；
  worker 那边（`worker/src/agent.rs:324-345`）因此只能一律重试。

**契约里还没有的**（锁定面，本轨不碰，T0 补丁带来）：`Message` 没有 `provider_extra`（`contracts/src/protocol.rs:155-169`）、
`Usage` 没有 `cache_write_tokens`（:183-191）、`ModelConfig` 没有 `vendor` / `timeout_sec`（`contracts/src/config.rs:34-46`）、
`ModelError` 只有 `Config` / `Upstream` / `BadResponse` 三个 `String` 变体（`contracts/src/errors.rs:71-80`）。
所以 CC6 只做**不改契约**的那一半：厂商从模型名 / 域名推断、按厂商调请求体、进程内按 `tool_call_id` 回挂思考字段、
把思考字段镜像进 `ModelTurn.raw["provider_extra"]`（`raw` 是开放 Map，protocol.rs:197-206）、错误文本定形。

**09-25 总管本机实测（DeepSeek，真 key；本轨只把它写成「有日期的事实」，不做 live 断言）**：

1. `GET /models` 只剩 `deepseek-flash` 与 `deepseek-v4-pro` 两个 id。
2. 配置里的 `deepseek-chat` 仍被接受，静默路由到 `deepseek-flash`（非思考模式），带工具能用。
3. `deepseek-flash` 思考模式返回了 `reasoning_content`；第二轮工具调用**不回传**它也返回 200 —— 调研里「会 400」没复现。

（来源：总计划 §0 第 4 条，只有这三条。超时 600 秒是卡片与总计划 §6.1 给定的值，不是实测出来的。）

**谁接你的东西**：CC3（同波，worker）按本轨定的错误文本分类重试（`HTTP 400/401/403` 不重试、429 按 `retry-after-ms` 退避）；
DD4（W2）把 `raw["provider_extra"]` 落进 `Turn.provider_extra` 并在追问时回放；DD7（W2）的厂商档案按 `ModelConfig.vendor` 选，
缺省回落到**你的推断函数**，你的进程内缓存仍是任务内的兜底；T0c（W2a）会改 `models/**` 里的结构体字面量，并把你的本地
`Vendor` 对到契约的 `ModelVendor`；FF9（W4）的 live 矩阵走 `scripts/live-matrix.sh`，与本轨的 live 测试互不依赖。

## 2. 必读（按顺序）

1. 仓库根 `CLAUDE.md`（云端唯一能读到的约定；与本派单冲突时以本派单为准）。
2. 总计划（按节名找，别信行号——文件还在改）：§4.4 开场自检、§5.2「模型」一条、§6 开头「每一波都遵守的规则」、§6.1 表里的 CC6 行、
   §9 解冻清单、§10「模型行为与调研不符」那一行、附录 A「国产模型接入要点」、§3 的 D9（主力模型；`selfhost` 与「显式 vendor 永远优先」）/ D19（key 只在本机）。
3. `core/crates/models/src/lib.rs` 全文。重点段：:30、:42-82、:105-125、:132-214、:217-245（`parse_arguments`，对象型参数已在 :235-236 照收）、
   :266-271（`cost_of`）、:304-340、:343-346（`with_base_url`）、:361-381、:407-465（`chat`）、:507-512（`redact`）、:524-630（单测）。
4. `core/crates/models/tests/test_openai_compat.rs`（24 条）与 `tests/common/mod.rs`（最小 HTTP 假服务，:56-101；响应头写死在 :86-90）。
5. 契约（**用 Read 工具读**，锁定面）：`contracts/src/protocol.rs:155-206`、`errors.rs:71-80`、`config.rs:30-61`。
6. 调用方（只读，确认公开 API 不能动）：`worker/src/agent.rs:150`（`self.chat(&messages)`）、:176（`messages.push(turn.message)`，
   assistant 消息原样进历史，`call_id` 不变）、:324-345（重试循环，:336 传 `config.model.temperature`）；`app/src/app.rs:356-367`（全进程一个客户端）；
   `app/src/wiring.rs:216-217`（评测每个场景一个客户端）；`app/src/preflight.rs:79`（用到 `OpenAiCompatModel, cost_of, env_snapshot, resolve_api_key`）、
   :125（`MODEL_TIMEOUT_SEC = 60`，外层 `tokio::time::timeout` 在 :1405-1407，与本轨无关，别碰）。

## 3. 工作区

- **分支**：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC6: 国产模型加固 + 思考字段回挂缓存」。
  PR 描述先用 Write 工具写成 `/tmp/cc6-pr.md`，再 `gh pr create --draft --title "CC6: 国产模型加固 + 思考字段回挂缓存" --body-file /tmp/cc6-pr.md`（详见 §8 末尾）。
- **代码基线**：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**（逐条核过）：
  - `core/crates/models/**` —— 现有文件：`Cargo.toml`、`src/lib.rs`、`tests/common/mod.rs`、`tests/test_openai_compat.rs`；
    建议新建：`src/vendor.rs`、`src/echo.rs`、`tests/cc6_vendors.rs`、`tests/cc6_live.rs`、`tests/fixtures/*.json` + `tests/fixtures/README.md`。
  - `review/p1/ledger/CC6.md`（新建）。
  - `models/Cargo.toml` 虽在可写面里，**不许加任何依赖**（哪怕 workspace 里已有的也不行：会改 `core/Cargo.lock`，那是 CC1 的 R0 文件）。
    取 host 用 `reqwest::Url::parse`（reqwest 自带），别加 `url`。
- **只读面**：其余一切。特别点名：
  - P0-CLOSE 的 14 个路径（总管在本波期间本机打；与 §4 第 1 步情形 B 那张表是同一张）：
    AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
    BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
    `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
    `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
    BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`。
    （外圈照总计划 §5.1 整体只读：`core/crates/evidence/**`、`.claude/**`。）
  - T0 补丁文件：`config/aite.example.yaml`、`edge/internal/server/server.go`、`core/crates/proto/**`（外加锁定面 `proto/aite/v1/*.proto`、`core/crates/contracts/**`）。
  - 离你最近的别轨面：CC3 的 `core/crates/worker/**`（你的错误文本的消费方）；CC4 的 `core/crates/app/src/{app,wiring,lib}.rs` 与 `app/src/features/**`
    （造模型的地方）；CC7 的 `core/crates/testing/**`（`FakeModel`）、`core/crates/evals/**`；CC1 的 `core/Cargo.toml`、`core/Cargo.lock`、
    `core/crates/app/Cargo.toml`、`scripts/check.sh`；以及本波没人拥有的 `core/crates/app/src/preflight.rs`。
- **本轨解冻的冻结项**：无（§9 没给 CC6 列解冻项）。若发现某条冻结规格或别处测试钉住了 120 秒超时、全角冒号的错误文本或请求体形状，停下写回执。

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
   回执里写明是 A 还是 B。
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（期望形如「blocked: 该操作触碰受保护面 …」；拦截原文逐字贴进回执）。
   **这一步被拦就是通过**：拦截原文里「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步。
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**。
   冷编译 10–15 分钟，超过 Bash 单条 600 秒上限：用 Bash 的 `run_in_background` 后台跑并落日志
   `scripts/check.sh > /tmp/cc6-check.log 2>&1; echo "exit=$?" >> /tmp/cc6-check.log`，跑完用 Read 工具读 `/tmp/cc6-check.log` 全文（回执要原样全文）；仍然不许 `| tail`。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认 → `ok`，8 + 1 = 9 包。
   若 CC1 已合并、check.sh 多打一行 `go packages ok=N fail=M`，以那行为准。
5. **本轨附加**：`cd core && cargo test -p aite-models` → 三行 `test result` 合计 `passed=27 failed=0`（lib 单测 3、`test_openai_compat` 24、doc-tests 0；
   这个数是按源码数的，本机没实跑 —— 对不上先把实际输出贴进回执，再判断是不是基线问题）。
   若第一次编译慢到逼近 600 秒，同第 4 步：后台跑、输出落 `/tmp/cc6-models-base.log`，再用 Read 读全文。

## 5. 工作项

**总纪律**：`lib.rs` 的公开面**签名不变**（`OpenAiCompatModel::{from_config, with_base_url, base_url, total_cost, last_usage}`、`cost_of`、
`resolve_api_key`、`env_snapshot`、`to_openai_messages`、`to_openai_tools`、`turn_from_response`、`usage_from_openai`、`MessageError`）——
`OpenAiCompatModel` / `env_snapshot` 被 app 用（`app/src/app.rs:20`、:366、:438，`app/src/wiring.rs:55`、:216-217），`cost_of` / `resolve_api_key`
另被 `app/src/preflight.rs:79` 用（那些文件不在你的可写面里）；`to_openai_messages` / `to_openai_tools` / `turn_from_response` / `usage_from_openai`
被 `tests/test_openai_compat.rs:16-19` 直接导入调用，`MessageError` 是 `to_openai_messages` 签名里的错误类型（lib.rs:39-42）——签名都不许动。
evals 不导入本 crate（`evals/src/cli.rs:46`、:390 与 `deps.rs:170` 只是注释）。现有 24 + 3 条测试一条不许改断言；要加能力就加**新函数 / 新方法**。
**Generic（认不出厂商）档：响应不带思考字段时，请求体与今天逐字节一致**；响应带思考字段 + `tool_calls` 时照样按第 4 项回挂（卡片没按厂商限定回挂）。
`preflight_e2e.rs` 用的是 `fake-model`（:70）+ `127.0.0.1`、响应不带思考字段，正是靠这条保证 models crate 外面的 897 条不动。
B8（`evals/p0`）走 `--model scripted`，不经过这个 crate。

1. **厂商识别**（新 `src/vendor.rs`）。`pub enum Vendor { Generic, Deepseek, Qwen, Glm, Kimi, Doubao, Minimax }`，`as_str()` 取
   `generic / deepseek / qwen / glm / kimi / doubao / minimax`（取 T0 `ModelVendor` 的前 7 个，字符串逐一相同，T0c 好一一对上；
   T0 还有 `selfhost`，它只能显式配置，`infer_vendor` 永不返回它，本轨不加）。
   `pub fn infer_vendor(model: &str, base_url: &str) -> Vendor`：**先看模型名前缀**（不分大小写：`deepseek`、`qwen`、`glm`、`kimi` / `moonshot`、
   `doubao`、`minimax`）—— 百炼上托管的 Kimi / GLM / DeepSeek 走 `*.maas.aliyuncs.com`，只看域名会误判成 Qwen；**再看 base_url 的 host**：
   `api.deepseek.com`、`*.maas.aliyuncs.com` 或 `dashscope*.aliyuncs.com`、`open.bigmodel.cn`、`api.moonshot.cn`、`ark.cn-beijing.volces.com`、`api.minimax.cn`；都不中 → `Generic`。
   `base_url` 用 `reqwest::Url::parse` 解析失败或没有 host → 只按模型名前缀判；前缀也不中 → `Generic`；不许 `unwrap` / `expect`（运行时路径）。
   **在 `from_config` 里按 `cfg.model` + `cfg.base_url` 算一次、存进结构体**，`with_base_url`（lib.rs:343-346）不重算 —— 测试才能在 cfg 里写厂商域名、再把流量指到假服务。
   加 `pub fn vendor(&self) -> Vendor`。显式 `ModelConfig.vendor` 随 T0 到来，由 T0c / DD7 接成「显式非 generic 优先」，本轨不做。
   模块头 `//!` 写一段「2026-09-25 DeepSeek 实测」，把 §1 的三条事实原样写进去（带日期、注明是总管本机真 key 测的、云端未复测）。
2. **按厂商调请求体**（`request_body`，lib.rs:361-381）：
   - Kimi：请求体里**没有** `temperature` 键（不是写 0）。
   - Qwen：`tools` 非空时加 `"parallel_tool_calls": true`；没有工具时不加（保住 `chat_without_tools_omits_the_tools_field`，test_openai_compat.rs:408-423）。
   - GLM：`tool_choice` 只会是 `"auto"`。**这是今天已有的行为**（lib.rs:378），本条是钉，不是改。
   - 其余厂商（含 Generic）不变。注意现有 `cfg()`（test_openai_compat.rs:23-34，`qwen-plus` @ `dashscope.example.com`）会按前缀认成 Qwen、多出
     `parallel_tool_calls`，而 `chat_sends_tools_and_accumulates_cost` 在 :368-373 断言 temperature≈0.3 —— 所以只有 Kimi 省 temperature，Qwen 照发。
   - 新测试：`kimi_request_omits_temperature`（钉：Kimi 两条识别路径 —— 百炼域名 + `kimi-` 前缀、`api.moonshot.cn` + 无前缀的模型名 —— 发出去的请求体都没有 `temperature`）；
     `qwen_sets_parallel_tool_calls`（钉：有工具 → `true`；无工具 → 键不存在）；`glm_tool_choice_auto_only`（钉：GLM 带工具时 `tool_choice == "auto"`，
     不带工具时键不存在；变异验证 = 把 GLM 的值改成 `"required"` 或删掉该键 → 红）。
3. **响应侧：思考字段入 raw、MiniMax 剥 `<think>`**（在 `chat()` 里 :452 之后做，或另写一个带厂商参数的新函数；`turn_from_response` 的签名不动）：
   - 从 `choices[0].message` 原样取 `reasoning_content` / `encrypted_content` / `reasoning_details`（有哪个取哪个，JSON 值原样），
     非空时写进 `turn.raw["provider_extra"]`（一个对象，键名就是这三个）；都没有 → 不写这个键。
   - MiniMax：content 开头的 `<think>…</think>` 剥掉（连同其后空白），剥下来的正文放进 `provider_extra["reasoning_content"]`；别的厂商的 content 一字不动。
   - 对象型 `arguments`：已在 lib.rs:235-236 照收，**不用实现**；在某个厂商夹具里让它以对象形态出现、顺带断言即可，不另起测试名。
   - 新测试：`reasoning_captured_in_raw`（钉：DeepSeek 夹具的 `reasoning_content` 与豆包夹具的 `encrypted_content` 都原样出现在 `raw["provider_extra"]`；
     无思考字段的响应没有这个键）；`minimax_think_stripped`（钉：`turn.message.content` 不含 `<think>`，剥下的文本在 `provider_extra` 里；非 MiniMax 的同样文本不剥）。
4. **进程内回挂缓存**（新 `src/echo.rs`，挂在 `OpenAiCompatModel` 上，lib.rs:304-310 加一个 `Mutex<…>` 字段，照 `acct` 的写法）：
   - 写入：`chat()` 拿到带 `tool_calls` 且有思考字段的响应时，对**每个** `call_id` 存一条（思考字段 + 指纹）。
     指纹 = `name` + `serde_json::to_string(&ToolCallRequest.arguments)`，即**解析后的 Map 再序列化**，与请求侧 lib.rs:60 同一写法；
     **不用**响应里的原始 `arguments` 字符串（厂商的空白与键序和重新序列化的对不上，真实流量里就永远回挂不上）。
   - 读出：`request_body` 里，`to_openai_messages` 翻完之后，逐个带 `tool_calls` 的 assistant 行按其 `call_id` 查缓存；命中且指纹一致才把字段插进那一行
     （行里已有同名键就不覆盖）。**读了不删**：worker 每一步都把整段历史重发一遍（agent.rs:150/176），删了第二步之后就回挂不上了。
     指纹核对的理由：有的厂商 tool_call id 是按序号编的，全进程共用一个客户端（app.rs:356-367），并发任务之间会撞 id。
   - MiniMax 的回挂形状：把剥下来的文本按 `<think>…</think>` 原样拼回该行 content 开头（这是它自己返回的形状），不加 `reasoning_content` 字段。拿不准就照做并在回执写明。
   - 有界：`const ECHO_CACHE_CAP`（建议 1024 条），满了先进先出淘汰；注释写清取值理由（`max_steps` 默认 40，见 `config/aite.example.yaml:34`；T0c 后并发默认 4）。不引入 LRU 库。
   - 新测试：`deepseek_thinking_tool_roundtrip_reattaches_reasoning`。用假服务跑两轮：第 1 轮夹具 = `deepseek-flash` 思考模式、带 `reasoning_content` + 一个 tool_call；
     第 1 轮夹具里 tool_call 的 `arguments` 用**带空格**的 JSON 串（如 `"{\"q\": 1}"`），指纹若误用原始串，这条测试就会红。
     测试像 worker 那样把 `turn.message` 与一条 tool 消息接进历史再调第 2 轮；断言第 2 个请求里那条 assistant 行带着原样的 `reasoning_content`、第 1 个请求里没有。
     同一条测试里再钉探测事实 2：请求 `deepseek-chat`、夹具回 `"model": "deepseek-flash"` → `raw["model"] == "deepseek-flash"`（lib.rs:181-184 已有）而 `name()` 仍是 `deepseek-chat`，
     且无思考字段时第 2 轮请求体不多出任何键。假服务第 2 轮照回 200（事实 3），别模拟 400。
5. **缓存命中三种字段**（`usage_from_openai`，lib.rs:105-125，签名不变）：`cached_tokens` 依次取 `prompt_tokens_details.cached_tokens` →
   `prompt_cache_hit_tokens`（DeepSeek）→ 顶层 `cached_tokens`，先出现的为准；都没有 → 0。**不动 `cost_of`**（lib.rs:266-271；按缓存命中 / 写入分档计价是 DD7 的）。
   新测试：`cache_hit_field_variants`（三种形状各一 + 都没有；现有 `usage_maps_including_cached_tokens` 保持绿）。
6. **超时 600 秒**：lib.rs:30 `120 → 600`，注释写：600 秒是卡片与总计划 §6.1 给定的值，与 T0 将加的 `ModelConfig.timeout_sec` 默认 600 一致；
   思考模型一轮常超过 120 秒。preflight 外层那 60 秒（preflight.rs:125）不是你的，不动。
   这是常量改动，可以在 lib.rs 单测模块里加一条常量钉（名字自定，计入 Δ）；不加就在回执写明理由。
7. **错误文本定形（与 CC3 的约定，逐字）**：非 2xx 时 `ModelError::Upstream` 的内文 =
   ```
   HTTP {status}: {detail}                       # 半角冒号 + 一个空格；detail 照旧 clip 500 字符 + redact
   HTTP 429: {detail} retry-after-ms={n}         # 仅 429 且拿得到 n 时追加；追加在 clip 之后，永不被截
   ```
   整条 `Display` 就是 `模型服务错误：HTTP 429: … retry-after-ms=2000`（外层前缀来自 errors.rs:76，不动）。lib.rs:440 的全角冒号改成半角。
   `n` 的来源：响应头 `retry-after-ms`（整数毫秒）优先，其次 `retry-after` 的整数秒 ×1000；HTTP-date 形式或解析不了 → 不追加（CC3 自己退避）。
   响应头要在 `response.text()`（lib.rs:430-434）消费响应之前取。CC3 那边按 `HTTP (\d{3}):` 判状态码、按 `retry-after-ms=(\d+)` 取退避；
   若你在仓库里的 `review/paste-CC3.md` 看到不同的形状，按本派单实现、在回执点名差异，总管合并前统一。
   假服务要能回响应头：给 `tests/common/mod.rs` 加一个带额外响应头的新构造函数（如 `start_with_headers`），`start` / `start_raw` 签名不动。
   新测试：`http_status_and_retry_after_in_error_text`（钉：429 + `Retry-After: 2` → 含 `HTTP 429: ` 与 `retry-after-ms=2000`；429 无头 → 不含 `retry-after-ms`；
   500 → 含 `HTTP 500: `；全程密钥不出现）。现有 `upstream_errors_never_leak_the_api_key` 只断言含 `401`（test_openai_compat.rs:449），照样绿。
8. **六家夹具 + 一条 live**：
   - `tests/fixtures/` 放 deepseek / qwen / glm / kimi / doubao / minimax 六家的响应 JSON（上面各条测试用 `include_str!` 读），配 `README.md`
     写明每份的来源（附录 A 的官方文档形状 + 09-25 实测的三条事实）与日期，并写明「不是云端真录的，云端没有 key」。
   - 事实 1（`GET /models` 只剩两个 id）只能以数据形式落地：`tests/fixtures/deepseek_models.json` 存那次 `/models` 列表（README 里写日期与来源）。
     本 crate 没有 `/models` 调用，**不新增 API**；回执「没做的与原因」写明事实 1 没有客户端测试的理由。
   - `tests/cc6_live.rs`：一条名字含 `live` 的测试（如 `live_model_tool_roundtrip`），`#[ignore = "…"]`，函数开头 `AITE_LIVE_MODEL != "1"` 就直接返回；
     key 只从 `AITE_MODEL_API_KEY` 环境变量取；端点 / 模型默认 `https://api.deepseek.com` + `deepseek-flash`，可用环境变量覆盖；跑一轮带工具 + 回挂的第 2 轮，
     `eprintln!` 打出响应的 `model` 字段。这个仓库平时不许 `#[ignore]`（台账有先例）—— **这里是卡片明令的例外**，ignored 的不进 `cargo passed`。

## 6. 规则

- **可写面 / 只读面**见 §3；需要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。
- **B8 不变量**（`evals/p0` 钉死，弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；
  `!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 一个字都不许改。
- **守卫**：被拦就停（开场自检第 2 步那次除外：那次被拦就是通过）、拦截原文进回执、不许换写法绕。云端命令里永不出现：relock 变量赋值、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`；多行脚本写成文件再跑（跨行引号会被判「无法解析」）；check.sh 不接 `| tail`。
  守卫也扫 heredoc 正文和命令参数：含上述路径的正文（回执、PR 描述）一律用 Write 工具落文件，再用 `--body-file` 之类的方式引用，别用 heredoc 或 `--body "…"`。
- **每个行为改动**：回归测试 + 变异验证（撤回改动 → 测试红 → 贴输出）。还原用 `git checkout -- <文件>` 或 `git stash pop`，**别用 `cp -p` / `shutil.copy2`**（旧 mtime 让 cargo 跳过重编，出假绿）。
  对「钉住已有行为」的测试（GLM auto、对象型参数），变异 = 人为注入违例（如 GLM 发 `"required"`）看它红。
- **格式化**：`rustfmt --edition 2024 <改过的文件>`，逐个跑（**别用 `cargo fmt --all`**）。check.sh 的 A4b 会跑 `cargo fmt --check`。
- **新第三方依赖、R0 文件（不在你可写面里的）、锁定面** → 停下报告。T0 已经会带来的契约字段（§1 列的那些）不算缺口，别在本轨绕着造。
- **密钥**：云端环境里没有、也不许放任何模型 key；live 只在总管本机跑。测试里的 key 一律是假值。
- Docker 测试封闭（只连本地测试服务器；本轨用不到 Docker）。云端 protoc 生成的 `edge/gen` 永不提交（本轨不跑 `make proto-gen`）。
- **已知时序抖动**（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、
  `aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。

## 7. 验收（命令 + 期望输出）

| # | 命令 | 期望 |
|---|---|---|
| 1 | `cd core && cargo test -p aite-models` | 各行 `failed=0`；passed 合计 = 27 + Δ_models；`1 ignored`（live 那条） |
| 2 | `cd core && cargo test -p aite-models -- --exact deepseek_thinking_tool_roundtrip_reattaches_reasoning kimi_request_omits_temperature qwen_sets_parallel_tool_calls glm_tool_choice_auto_only minimax_think_stripped cache_hit_field_variants reasoning_captured_in_raw http_status_and_retry_after_in_error_text` | 合计 8 passed、0 failed，名字逐字（`--exact` 按全路径匹配：这 8 条要放在集成测试文件顶层，别包 `mod`，否则一条也匹配不上） |
| 3 | `cd core && cargo test -p aite-models -- --exact upstream_errors_never_leak_the_api_key unparseable_responses_never_leak_the_api_key debug_never_leaks_the_key tests::a_key_echoed_in_the_url_is_still_redacted` | 合计 4 passed（前 3 条在 `test_openai_compat` 顶层，第 4 条在 lib.rs:525 的 `mod tests` 里，所以带 `tests::` 前缀；旧的密钥脱敏测试一条没动、全绿） |
| 4 | `cd core && cargo test -p aite-models live -- --ignored`（云端**不设** `AITE_LIVE_MODEL`） | live 那条 1 passed，立即返回、不发网络请求 |
| 5 | `cd core && cargo clippy -p aite-models --all-targets -- -D warnings` | exit 0 |
| 6 | `scripts/check.sh > /tmp/cc6-check.log 2>&1; echo "exit=$?" >> /tmp/cc6-check.log`（Bash 的 `run_in_background`，冷编译超过单条 600 秒上限；跑完用 Read 工具读全文，不接 `\| tail`） | 日志末行 `exit=0`，其上一行「全部通过」；`cargo passed=<897 或 901>+Δ failed=0`（Δ 逐条列名，= 8 条点名测试 + 你另加的每一条）、`contracts passed=<25 或 27> failed=0`、`OK 25 files`、`passed 10/10`；Go 那格 8 行 = 6 行 `ok` + 2 行 `? … [no test files]`（`gen/aitepb`、`internal/pin`；排第一的 `cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` → `ok`，合计 9 包 |
| 7 | `git diff --name-only origin/main...HEAD` | 每一行都以 `core/crates/models/` 开头，或正好是 `review/p1/ledger/CC6.md` |
| 8 | `git status --short` | 空（临时脚本、探针都清掉） |

- 第 6 行的基线数按开场自检第 1 步判定的情形取（A：897 / 25；B：901 / 27）。**别接 `| tail`**。
- Go 模块文件有没有被改，由总管审 PR 时看；你别在命令里 grep 它们。
- 本机人工步骤（写进回执，给总管照做）：`cd ~/Documents/Projects/Aite/core && AITE_LIVE_MODEL=1 cargo test -p aite-models live -- --ignored --nocapture`
  （key 放本机环境变量；`--nocapture` 才看得到打印的 `model` 字段），把响应的 `model` 字段记下来。

## 8. 回执（写 `review/p1/ledger/CC6.md`，PR 描述贴摘要）

1. **开场自检原文**（4 + 1 项）：第 1 步 diff 输出与判定（A / B）；守卫拦截原文（逐字）；三条工具链版本；check.sh 各行原样；`cargo test -p aite-models` 基线输出。
2. **工作项逐条**：1–8 每项改了哪些 `文件:行`；`Vendor` 字符串表；`ECHO_CACHE_CAP` 取值与理由；MiniMax 回挂形状的决定。
3. **新增测试逐条 + 变异验证输出**：每条测试钉什么；变异怎么做的；红的那段输出逐字贴。
4. **check.sh 完整输出**（原样，不截；即 `/tmp/cc6-check.log` 用 Read 读出的全文，含末行 `exit=`）。
5. **`cargo passed` 增量逐条**：哪个文件加了哪几条，合计 = Δ；ignored 的 live 那条单列（不进 passed）。
6. **错误文本约定**：贴三个真实样例（401 / 429 带头 / 500），给 CC3 对照。
7. **被守卫拦过的命令与拦截原文**（没有就写「无」）。
8. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）。至少考虑：按缓存命中 / 写入与峰谷计价（DD7）；显式 `ModelConfig.vendor` 优先（T0c / DD7）；
   Kimi `max_tokens ≥ 16000`、各家思考开关参数（DD7）；worker 按错误文本分类重试（CC3）；`Turn.provider_extra` 落库与追问回放（DD4）；
   以及这两行（照抄）：`selfhost` 档（显式 vendor 永远优先于前缀推断；不发云厂商专有参数）| 需要 T0 的 `ModelConfig.vendor` | DD7；
   海外端点默认拒绝（总计划 D9：只用大陆端点；`dashscope-intl`、`api.moonshot.ai`、`api.z.ai`、`api.minimax.io` 构造时报配置错，`allow_overseas_endpoint=true` 才降为告警）| 需要 T0 的 `ModelConfig.allow_overseas_endpoint` | DD7。
   （本轨 `infer_vendor` 只做识别，不因端点在海外而报错或告警。）
9. **没做的与原因**（包括 live 测试云端只验了开关、没真打端点；事实 1 只落了夹具数据、没有客户端测试，因为本 crate 没有 `/models` 调用）。
10. **契约缺口**（给 T0 / T0.1）：只报 T0 已计划内容**之外**的缺口（清单见总计划 §5.2「模型」一条与原卡 `contract_batches` 的 T0-p1.0）。模型侧 T0 已计划：
    `ModelConfig += name, display_name, filing_no, vendor, thinking, timeout_sec, stream, price_cached_in_per_mtok, price_cache_write_per_mtok, offpeak_price_multiplier, allow_overseas_endpoint`；
    `ModelsConfig { fallback, fast, catalog }`；`ModelVendor { generic, deepseek, qwen, glm, kimi, doubao, minimax, selfhost }`；`ThinkingMode { auto, on, off }`；
    `Message.provider_extra`；`Usage.cache_write_tokens`；`ModelError::RateLimited { retry_after_ms, message }` / `Auth` / `BadRequest`。
    每条缺口写清需要什么形状、为什么 `raw` / `provider_extra` 这类开放通道绕不过去。没有就写「无」。

**回执与 PR 描述怎么落**：回执用 **Write 工具**写 `review/p1/ledger/CC6.md`（它要逐字贴守卫拦截原文，里面有受保护路径）。
第一次提交时的 PR 描述先 Write 成 `/tmp/cc6-pr.md` 再 `gh pr create --draft --body-file /tmp/cc6-pr.md`（此时回执还不存在）；
收尾时再用 Write 写一份摘要 `/tmp/cc6-pr.md`（自检判定、工作项、新测试、Δ、转出项、缺口；不贴 check.sh 全文——回执全文可能超过 GitHub PR 描述 65536 字符上限），
然后 `gh pr edit --body-file /tmp/cc6-pr.md`；回执文件 `review/p1/ledger/CC6.md` 是正本，随 PR 提交。**不要**用 heredoc 或 `--body "…"` 传含受保护路径的正文——守卫会拦，拦了按规则就得停。
