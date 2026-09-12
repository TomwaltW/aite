# 任务 R1 — Go 飞书 adapter：归一化、REST 出站、群历史与文档、长连接退避重连

## 背景：这轨是从哪来的

Aite 决定整体用 **Rust + Go** 重写（权威文档 `docs/dev-spec-2026-09-11-rustgo.md`，先通读 §1–§3、§5–§7）。
Python 版 P0 已经跑通（T0–T24，1161 条测试、评测 10/10、真模型 × 真沙箱端到端），**它是规格与参考，只读**。
R0 已合入 main（`c9d96d2`）：proto 契约、Rust 契约 crate、Go 模块骨架（config / server / aiteerr / pin / 占位包）、
守卫、Makefile、check.sh、CI、spec、三份移植清单。

**你这轨是 Go 侧的飞书 adapter**：把 `aite/adapters/feishu/**`（8 个 .py，约 1700 行）逐文件搬成
`edge/internal/feishu/**`，实现 R0 定好的 `server.PlatformPort` 接口（`edge/internal/server/ports.go`），
并把 `tests/adapters/**` 的 100 个测试定义（参数化后 146 条）搬成 Go 测试。

移植清单：`review/inventory-feishu.md`（8 个小节 + 35 条非显然行为）。拿不准时开 Python 源码，它是最终依据。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-r1
分支     : task-r1
基线     : c9d96d2   ← main 的 HEAD（完整 sha c9d96d29d6d7ad835deef9fc80ec365c786fe876）
工具链   : go 1.27.1（/opt/homebrew/bin）、cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、protoc 36.1
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`；新开的 shell 里 `cargo --version` 能出来才算对）。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r1
git log --oneline -1                                   # 期望 c9d96d2 ...
git status --short                                     # 期望空
scripts/check.sh --quick                               # 期望最后一行 "全部通过"，退出码 0（首次 cargo 全量编译 2–5 分钟）
core/target/debug/aite contracts lock --check          # 期望 OK 36 files
cd edge && go test ./... -count=1                      # 期望 aiteerr / config / server 三包 ok，其余 [no test files]
```

`scripts/check.sh --quick` 里的关键行期望：`contracts passed=25 failed=0`、`cargo passed=35 failed=0`、B8 那段是 `not implemented: aite-evals`（RΩ 才要求 10/10）。

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
被拦才说明 hook 挂上了；没被拦说明 `CLAUDE_PROJECT_DIR` 不对，所有守卫都在静默失效，停下报告。

## 可写路径（白名单，之外一律只读）

```
edge/internal/feishu/**          ← 你的全部实现与测试（*_test.go 放同一包）
edge/testdata/feishu/**          ← 7 对 fixture（x.json → x.expected.json）
```

**邻居（只读，接口在这里）**：
- `edge/internal/server/ports.go`：`PlatformPort` 接口（pb 类型直接用），你的 `feishu.Platform` 必须满足它；方法只返回 `*aiteerr.PlatformError` 或 nil。
- `edge/internal/aiteerr/errors.go`：`PlatformError{Code, HTTPStatus, Retryable, Msg}` 与 gRPC 映射（映射表冻结）。
- `edge/internal/config/config.go`：`config.Feishu`（`app_id_env` 等只是**变量名**，取值在 `feishu.Options` 里由 main 从环境变量读好传给你）。
- `edge/cmd/aite-edge/main.go`（**R2 的**）调你的四个入口，签名不许改：
  `feishu.New(cfg config.Feishu, opts feishu.Options, sink feishu.EventSink) (*Platform, error)`、
  `(*Platform).Start(ctx) error`（阻塞到 ctx 取消，内部无限重连）、`Connected() bool`、`ReconnectCount() int64`、`feishu.FeishuP0()`。
  `EventSink.HandleEvent(ctx, *pb.NormalizedEvent) error` 是 edge → core 的出口（R2 实现 `internal/ingress`），你只管调用；它返回 error 就向 SDK 返回 error 让平台重推。
- `edge/internal/pin/pin.go`：依赖已预钉（`lark-oapi v3.12.0` 的 `ws` / `event/dispatcher` / `service/im/v1`、`x/time/rate`）。同一模块内的别的子包（如 `larkcard`、`core`）直接 import 即可；**要模块外的新依赖 → 停下报告**（守卫会拦 `go get` / `go mod`）。
- `proto/aite/v1/{events,outbound,capabilities}.proto` + `edge/gen/aitepb`：归一化的产物就是 `pb.NormalizedEvent`，枚举形如 `pb.SenderKind_SENDER_KIND_HUMAN`，`optional string` 是 `*string`，`raw`/`value` 是 `structpb.Struct`，`occurred_at` 是 `timestamppb`。

## 要做什么

按清单 §1 的文件顺序搬，每个 Go 文件头写明「对应 aite/adapters/feishu/xxx.py」：

### ① `normalize.go`（对应 `normalize.py`，清单 §2）
- 全部分支逐条搬：入口按 `header.event_type` 分流；`normalize_message` 逐字段映射表；`thread_id` 三选一取第一个非空；`extract_text` 的 text/post 两条路；`apply_mentions` 的三步（@自己连同紧跟空白/U+00A0 一起吃、@别人换 `@名字`、兜底删 `@_(user|all)_N`、最后 strip）；`sender_kind` 映射（未知一律 app）；附件五种 msg_type；post 拍平（title 第一行、`a` 变 markdown 链接、`at` 占位符原样、`img` 进 attachments）；`_post_mentions_bot`；`normalize_card_action`（未知 action 返回 nil；chat_type 固定 group；sender_kind 固定 human；mentioned true；anchor.thread_id 不填）；`to_datetime` 按 `>= 1e14` 判微秒。
- `raw`：原始事件整份进 `structpb.Struct`，**但剥掉 `header.token` 与 `event.token`**（Python 版只剥了前者，清单 §8 第 10 条是它遗留的审计卫生缺口，这次修掉并在回执写明）。
- fixture：把 `tests/fixtures/feishu/` 7 对搬到 `edge/testdata/feishu/`，`x.json` 原样，`x.expected.json` 用 **protojson（`UseProtoNames: true, EmitUnpopulated: false`）** 重生成，测试逐字节比对 + 6 组手写字面量断言（清单 §6 `test_normalize.py`）。

### ② `api.go`（对应 `api.py`，清单 §3.1 / §5）
- `net/http` 直打，端点表逐条；`tenant_access_token` 带过期缓存（提前 60s 刷新）与锁；`request(...)` 返回 `data`；`binary` 直接返回 body；`RETRY_DELAYS = 0.5/1/2s` 只对 429 / >=500 / 传输错误（共 4 次请求）；4xx 非 429 不重试；HTTP 200 业务码非 0 → `retryable=false, http_status=200`；**401 在上层 invalidate token 再打一次，不吃退避额度**；错误一律 `*aiteerr.PlatformError`。
- 时钟与 sleep 可注入（测试断言 `slept == [0.5, 1, 2]`）；日志 `feishu.retry method=.. path=.. status=.. attempt=..`（slog）。

### ③ `cards.go`（对应 `cards.py`，清单 §3.2）
`BuildChecklistCard(*pb.ChecklistCard) map[string]any`（元素顺序、状态色、`STATE_ICON`、note 的 `　—— `、空 items 文案、按钮 value round-trip、**30KB 裁剪**）、`DumpsCard`（`json` 紧凑无转义 → 用 `json.Encoder` + `SetEscapeHTML(false)`）、`BuildMarkdownCard(text)`。所有文本出站都是 interactive 卡片，不走 `msg_type=text`。

### ④ `ratelimit.go`（对应 `ratelimit.py`）
令牌桶：`rate_per_min` 来自 capabilities（60）、capacity 默认 = rate、持锁 sleep、clock/sleep 注入。上传与读接口不过桶。

### ⑤ `connection.go` + `platform.go`（对应 `connection.py` / `platform.py`，清单 §4）
- `Start(ctx)` 循环：首连不延迟；退避 `min(2^(attempt-1), 30)` → `[1,2,4,8,16,30,30]`；连上 attempt 归零；断开后从 1s 起；连接对象每轮新建；SDK 自带的自动重连关掉；`feishu.reconnected after=%d attempts`（INFO，只在重连时）、`feishu.reconnecting` / `feishu.connect_failed` / `feishu.connection_lost`（WARN）。连接对象做成接口，测试用假连接量退避序列。
- 每条原始事件 → `normalize` → `sink.HandleEvent(ctx, ev)`：**同步等结果再向 SDK 返回**；HandleEvent 返回 error → 向 SDK 返回 error（平台会重推），并打 `feishu.on_event_failed event_id=..`；耗时 > 1s 打 `feishu.on_event_slow`。**不做任何去重。**
- `Connected()` / `ReconnectCount()` 给 EdgeStatus 用。
- Go SDK（`larkws`）能否收到 `card.action.trigger` 帧要**实测**（Python SDK 1.7.3 会丢，T16 加了转接）：用 SDK 的 dispatcher 注册 `OnP2CardActionTrigger`（或等价），造帧或读源码证明卡片回传能到你的 handler；结论写回执。`envelope` 形状照 Python：`{schema, header{event_id, create_time, event_type, tenant_key, app_id}, event}`。
- capabilities：`FeishuP0()` 每次新对象；`SetPassiveListen(bool)` 只改实例。
- `REACTION_EMOJI = {ack: OnIt, done: DONE, fail: CRY}`（**禁止 `EYES`**）。

### ⑥ 测试（清单 §6，100 个定义逐条对照）
- 出站/读接口/错误：`httptest.Server` 记录方法、路径、query、body；断言 `update_card` 是 PATCH 且两条 POST 路由零命中、`send_card` 两条路、`send_file` 图/文件两步、`add_reaction` emoji 合法、限速 `slept`、分页 `page_token=pt2`、`limit=0` 不发请求、`read_history` 正序且不过滤、docx/wiki 两条路、401 换 token。
- 重连：假连接对象，真跑 `Start` 量出 `[1,2,4,8,16,30,30]`；连挂 50 次不退出；`feishu.reconnected` 是 INFO；stop 后 start 正常返回。
- 卡片：本地最小 schema 校验、四状态配色互不相同、30KB 裁剪、value round-trip。
- capabilities：默认等于 `FeishuP0()`、实例隔离、桶速率 60。
- 参数化用例拆成 `t.Run` 子测试；Go 测试名保留 Python 名（`TestUpdateCardUsesPatchNotSend` 这种直译）。

## 纪律

1. 契约（`proto/**`、`core/crates/contracts/**`）一个字都不许动，锁必须全程 `OK 36 files`。
2. 白名单之外的文件只读。要改别人的面（如 `server.PlatformPort` 缺方法、`aiteerr` 缺形状、要新依赖）→ **停下报告**，写清建议。
3. Python 树只读；它是规格。行为要改（例如你认为 `read_history` 的 4 倍 budget 静默少返回不对）→ 回执里提，不动手。
4. 回帖/日志事件名/错误码逐字沿用；`gofmt -w` 只格式化自己的文件；`go vet ./...` 必须干净。
5. 每条结论挂实测。「应该会」「大概」一句不要。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r1
cd edge && go test ./internal/feishu/... -count=1 -v 2>&1 | tail -5     # 期望 ok，一条不许 FAIL
cd edge && go test ./... -count=1                                       # 期望全部 ok
cd edge && go vet ./... && test -z "$(gofmt -l .)" && echo lint-ok      # 期望 lint-ok
cd .. && scripts/check.sh --quick                                       # 期望 全部通过，退出码 0
core/target/debug/aite contracts lock --check                           # 期望 OK 36 files
```

## 回执格式

```
## R1 回执

基线 c9d96d2 → 提交 <短 sha>

### 移植对照表
| Python 文件 | Go 文件 | 行数 |
| tests/adapters/... 测试 | Go 测试（Test… / 子测试数） | 差异说明 |
（Python 100 个定义 / 146 条 → Go N 个 / M 条；少的逐条说明为什么）

### Go SDK 与 Python SDK 的差异
卡片回传帧：<实测结论 + 证据>
ack 时机 / 重连：<>
其他：<>

### 与 Python 行为的差异（逐条；没有就写"没有"）
- <差异> —— 为什么

### 修掉的 Python 遗留问题
- event.token 不再进 raw：<证据>

### 改了什么
- <文件> <一句话>

### 实测输出（粘实际的，不要写"通过了"）
$ cd edge && go test ./internal/feishu/... -count=1 -v | tail -5
<粘>
$ scripts/check.sh --quick
<最后 3 行>
$ core/target/debug/aite contracts lock --check
<那一行>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-r1` 分支上，回执贴出来。
