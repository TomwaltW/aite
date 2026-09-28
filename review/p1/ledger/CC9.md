# CC9 回执：钉钉 Stream 适配器包（不接线）

- 轨号：CC9（第 1 波，云端）· 派单 `review/paste-CC9.md` · 分支 `claude/cc9-dingtalk-stream`
- 基线：`8458435`（HEAD 起点 `30b00e5` = 基线 + 总管 D0 文档提交），**情形 A**
- 可写面内新建：`edge/internal/dingtalk/**`（8 个源文件 + 8 个测试文件 + 6 份夹具）、`docs/p1/dingtalk.md`、本文件。可写面外一个字没动。
- **推送 / draft PR**：会话起初的仓库**没有 `origin` 远端**（`git push -u origin claude/cc9-dingtalk-stream` →
  `fatal: 'origin' does not appear to be a git repository`），也**没有 `gh` 命令**（`/bin/bash: line 1: gh: command not found`）。
  总管随后要求推送：经会话把 `TomwaltW/aite` 挂上推送权限后加了 `origin`（`origin/main` = `30b00e5`，与本分支起点一致），推送成功；
  draft PR 用 GitHub REST API 开（容器里仍没有 `gh`），见文末「交付状态」。

## 1. 开场自检原文

### 第 1 步：代码基线

```
$ git cat-file -e 8458435 && echo have
have
$ git diff --stat --no-renames --diff-filter=AM 8458435 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'
（空输出，exit 0）
```

判定：**情形 A**，基线行 = `cargo passed=946 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok。

### 第 2 步：守卫（被拦 = 通过）

用 Read 工具读 `.claude/hooks/guard_bash.py`，被拦，原文逐字：

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

全部对得上（本轨用不到 protoc 插件，照跑照记）。

### 第 4 步：`scripts/check.sh`（干净工作树）

说明：第一次 check.sh 是和写代码并行跑的，跑到 `A4c go vet` 时我已经在 `edge/internal/dingtalk/` 写了一半文件（`platform.go` 引用了还没写的 `apiClient`），
于是那一次 `A4c` 红、Go 一格数到 10 包 —— **那次作废，不算基线**（cargo 那几格 946 / 25 / OK 25 / 10/10 与下面一致）。
随后把 `edge/internal/dingtalk` 整个挪到 scratchpad、确认 `git status --short` 为空，在干净树上重跑，结果如下（原样）：

```
=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.28s
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
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.44s
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
（命令行略，同下文第 4 节）
cargo passed=946 failed=0
-> exit 0

=== B 全量 go test（-race） ===
（命令行略，同下文第 4 节）
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

与情形 A 基线逐字一致。check.sh 已是 CC1 新口径（末行 `go packages ok=N fail=M`），以那行为准：`ok=9 fail=0`。
单跑 `cd edge && go test -race ./cmd/... -count=1` → `ok  	aite/edge/cmd/aite-edge	6.069s`。

### 第 5 步：本轨附加

```
$ cd edge && go test -race ./... -count=1 > /tmp/cc9-go.txt 2>&1; grep -c '^ok' …; grep -cE '^(ok|\?)' …; grep -c '^FAIL' …
7
9
0
```

**N0 = 7**（与派单按源码静态数的 7 一致；原卡写的 9 是 `^(ok|\?)` 行数），`^(ok|\?)` = 9，`^FAIL` = 0。

```
$ grep -n 'gorilla/websocket' edge/internal/pin/pin.go
9:	_ "github.com/gorilla/websocket" // R1 的卡片帧测试直接 import 它造假 ws 服务端
$ ls edge/internal/dingtalk docs/p1/dingtalk.md
ls: cannot access 'edge/internal/dingtalk': No such file or directory
ls: cannot access 'docs/p1/dingtalk.md': No such file or directory
```

## 2. 工作项逐条

| # | 文件:行 | 内容 |
|---|---|---|
| 1 | `platform.go:44-63` `Config` / `DefaultConfig`（镜像 T0 `DingtalkConfig`，yaml snake_case，注释写明 config 是 R0、DD8 映射）；`:66-78` `Options` / `EventSink`；`:123-183` `newPlatform`（注入面 `httpClient` / `dialer` / `sleep` / `clock` / `logger`）；`:186` `New`（只装配）；`:194-207` 能力值；`:210-214` `Capabilities` 副本；`:221-233` 退避；`:237-286` `Start`（照 feishu 循环：只有 ctx 取消才返回、1,2,4,8,16,30 封顶、成功重连才 +1）；`:289-292` `Connected` / `ReconnectCount` | 包骨架与生命周期 |
| 2 | `stream.go:50-56` 三条订阅；`:85-120` `openConnection`；`:123-146` `connect`（每轮新 open → `endpoint?ticket=`；拨号错误只报状态码，不带含 ticket 的 URL）；`:183-212` 读循环；`:215-246` 帧分发；`:249-272` ACK（写锁 + 回显 `messageId`）；`:275-323` 无界队列 + 串行 dispatcher | Stream 客户端 |
| 3 | `normalize.go:40-58` `ParseQuote`；`:69-122` `NormalizeMessage`；`:125-158` text / richText / picture；`:170-179` 剥 `@BotName`；`:184-197` `rawStruct` 剥 `sessionWebhook`；`:201-228` `handleBotMessage`；`:232-275` 会话缓存 | 归一化 |
| 4 | `anchor.go:16` Crockford 正则；`:22-31` `TaskNoOf`；`:38-57` `anchorOf`（先本文后引用；`thread_id = repliedMsg.msgId` 只作提示；注释写明 H9 前 `#A` 是主锚点） | `#A` 锚点 |
| 5 | `api.go:30-41` `Path*` 常量；`:93-126` token 缓存（提前 60 秒）；`:136-151` 401 作废重取一次；`:164-199` `send`；`:206-224` `sanitizeURLError` / `stripQuery`（传输错误先去 URL query 再进 `Msg` 与日志）；`:244-259` `errorFromResponse`。`outbound.go:28-69` `SendText`（webhook → 群 / 单聊）；`:72-88` webhook 发送；`:104-124` `DownloadFile` | 出站与下载 |
| 6 | `card.go:66-101` `SendCard`（`outTrackId = aite-` + 16 hex，群 / 单聊 openSpaceId，记 `outTrackId → {chat, chatType, corp}`）；`:104-111` `UpdateCard`（只 PUT instances）；`:118-135` `StreamCard`；`:138-161` 按 rune 切块；`:164-247` 模板变量与清单 markdown | AI 卡片 |
| 7 | `callback.go:22-49` 先 ACK 再异步投递（`p.inflight` 记账，`Start` 返回前等它们）；`:52-58` 取 `cardPrivateData.params`；`:61-116` Stop → `CARD_ACTION`（Anchor 必填、`MessageId = outTrackId`） | 卡片回调 |
| 8 | `platform.go:299-316` `ReadHistory` / `ReadDocument` / `SendFile` / `AddReaction` → `aiteerr.ErrNotImplemented` | 未实现方法 |
| 9 | `docs/p1/dingtalk.md` §0–§10 | 文档（含「待 H9 真机核实」9 条与空的探针结果表） |

**能力值**（今天 pb 的 9 个字段，`platform.go:194-207`）：`Platform "dingtalk"`；平台事实（卡片 / 总计划给出的）：`SupportsThread / History / PassiveListen = false`、
`SupportsCardEdit = true`、`InboundFileInGroup = false`；**保守取值、待核实**：`CardEditWindowSec = 0`、`ProactiveRequiresPriorMessage = false`、`OutboundRatePerMin = 20`
（docs §7 标「待核实，DD11 按 `dingtalk_v1()` 对齐」）。

**流式切块口径**（照派单，没拿不准的地方需要偏离）：每次都重发完整 markdown，不做增量 diff；按 rune 边界切成 ≤1024 字节的块；第一块 `isFull=true`、其余 `false`；
`finalize` 时只有最后一块 `isFinalize=true`；每个 PUT 一个新 `guid`（32 位 hex）。空串发一块空内容（`isFull=true`）。

**ACK 时序**：SYSTEM（ping / disconnect / 其它）、EVENT、卡片回调 → 读循环里当场 ACK（卡片回调的 sink 投递另起 goroutine）；机器人消息 → 串行 dispatcher 里 sink 返回后 ACK，
sink 出错回 `code 500`。读循环永不调 sink。

**几处派单没写死、我按保守方式定的**（都进了 docs）：
- 回调类 ACK 的 data 用 `{"response":{}}`（推断）；`disconnect` 帧先 ACK 再关（SDK 是直接关，多回一帧 ACK 无害）。
- 机器人消息 data 不是合法 JSON → 照样 200 ACK（重推也还是坏的），打 Warn。
- `SendText` 碰到缓存里没有的会话 → 按群走 `groupMessages/send`（主动发只有群接口能按 chat_id 寻址）。
- `SendCard` 在 `*OpenDeliverModel` 之外还带了 `imGroupOpenSpaceModel` / `imRobotOpenSpaceModel{supportForward:true}`（调研记忆里官方示例的形状，派单没点名），docs §5 标「待 H9 核实是否必需」。
- webhook 路的 markdown `title` 取正文第一行非空文本的前 20 字（去掉 `#*> -` 前缀），全空用 `Aite`。
- 连接断开时 dispatcher 队列里没处理的帧不再 ACK（平台会重推，core 按 `event_id` 去重）。
- 清单卡 markdown 的用户可见文案（`发起人`、`开始`、`进行中 / 已完成 / 失败 / 已停止`、✅⏳⬜❌）由 `TestSendCardCreateAndDeliverThenUpdateInPlace` 逐字钉住；Rust 侧 `tests/wording.rs` 不涉及。

## 3. 新增测试逐条 + 变异验证

| 测试（文件:行） | 钉什么 |
|---|---|
| `TestPlatformImplementsPort`（`platform_test.go:16`；接口断言在 `helpers_test.go:27`） | `server.PlatformPort` 接口断言；`DefaultConfig` 6 个默认值逐字；`New` 不建连、默认 APIBase / TenantID；`Capabilities()` 9 个字段逐个 |
| `TestCapabilitiesReturnsACopy`（`platform_test.go:79`） | 改返回值不影响下一次读；16 个 goroutine 并发读改副本（`-race`） |
| `TestUnimplementedMethodsMapToUnimplemented`（`platform_test.go:107`） | 四个方法返回 `ErrNotImplemented`，经 `aiteerr.ToStatus` 都是 `codes.Unimplemented` |
| `TestStreamOpenConnectCallbackAckEchoesMessageID`（`stream_test.go:11`） | open 体凭证 + 三条订阅逐字；ws 握手带 `ticket-1`；机器人消息 → sink 收到归一化事件、ACK `code 200` + 同一 `messageId`；ping ACK data 原样回显；EVENT ACK SUCCESS 且不上送；sink 出错 → 非 200 ACK |
| `TestReconnectReopensWithFreshTicket`（`stream_test.go:95`） | 假网关 ticket 一次性、复用 401；`disconnect` 后用 `ticket-2`、服务端断开后用 `ticket-3`；`ReconnectCount` 0→1→2、`Connected` 跟着变；open 3 次、握手序列 `[ticket-1 ticket-2 ticket-3]`、0 次被拒；退避走注入 sleep `[1s 1s]` |
| `TestBackoffDelaySequence`（`stream_test.go:145`，派单外多加的一条） | 退避序列 1,2,4,8,16,30,30,30 |
| `TestNormalizeFixtures`（`normalize_test.go:28`） | 6 份夹具逐字段比期望（EventId / Kind / Platform / TenantId / WorkspaceId 回退 / ChatType / SenderId 回退 / SenderName / Text 剥 @ / RawText / Mentioned / OccurredAt 回退注入时钟 / Anchor 五字段 / 附件）；Raw 里没有 `sessionWebhook`；`ParseQuote` 两种 content 形状；子用例：file/audio/video → nil，怪形状 content 不 panic |
| `TestAnchorTaskNoFromOwnTextThenQuote`（`anchor_test.go:6`） | `#A1`、`#AH`、`#A10`、`#AZ8`、`#ah`→`#AH`、`#AH进展如何`→`#AH`、`#Awesome`→无、`#A`→无、跳过假锚点取后一个；本文与引用都有取本文、只在引用取引用、`thread_id = repliedMsg.msgId`、无引用 → nil |
| `TestAccessTokenIsCachedUntilExpiry`（`outbound_test.go:21`） | 两次只打一次 token；7139 秒不换、7141 秒换（提前 60 秒）；`x-acs-dingtalk-access-token` 头；401 作废重取一次；日志里没有 secret / token |
| `TestSendTextUsesSessionWebhookThenRobotAPI`（`outbound_test.go:114`） | 有效 webhook → 只打 webhook（markdown 体、不带 token 头、MessageId 空）；进入 60 秒余量 → 群走 groupMessages（`openConversationId` / `robotCode` / `msgKey` / `msgParam`）、单聊走 oToMessages（`userIds` 对、无 `openConversationId`），MessageId = `processQueryKey`；单聊缺 staffId → 不可重试；nil → `bad_request`；webhook 服务关掉后 `PlatformError` 可重试、`Msg` / `Error()` / 日志都不含会话令牌、保留去 query 的路径 |
| `TestDownloadFileViaDownloadCode`（`outbound_test.go:229`） | 第一步请求体逐字 + 带 token 头；第二步 GET 不带 token 头、签名 query 原样；返回字节逐字 |
| `TestSendCardCreateAndDeliverThenUpdateInPlace`（`card_test.go:16`） | outTrackId 形状、`CardId == MessageId`；群 / 单聊两种 `openSpaceId` 与 DeliverModel；`cardParamMap` 四个变量与 content 文案逐字；映射记录；`UpdateCard` 3 次只打 PUT instances、createAndDeliver 次数不变、`updateCardDataByKey=true`；没模板 → 不可重试且不发请求 |
| `TestCardStreamingChunksAtMost1KB`（`card_test.go:117`） | 3 KB 含中文 → ≥3 块、每块 ≤1024 字节且合法 UTF-8、拼起来逐字等于原文、只有第一块 `isFull`、只有最后一块 `isFinalize`、guid 各不相同；不 finalize 的短内容 |
| `TestCardCallbackAckedWithin2sWhileSinkBlocks`（`callback_test.go:25`） | sink 阻塞时 ACK < 2 s 先到；放开前 sink 已收到的事件字段逐字（含 Anchor 非 nil、`MessageId == outTrackId`、会话三字段来自映射、EventId = 帧 messageId、OccurredAt = 注入时钟）；未知动作有 ACK 无事件；子用例：机器人消息的 sink 阻塞时卡片回调照样 < 2 s ACK，且机器人消息的 ACK 在 sink 返回前不到 |

**变异验证**：代码先提交（`6be527c`），再用 `review` 外的临时脚本逐条改源码（锚点恰好命中一次）→ 跑对应测试 → `git checkout -- <文件>` 还原 → 复跑。10 条全红、全部还原后复绿，
`git status` 还原后只剩当时未提交的 docs。输出原样（`/tmp/cc9-mut.log`）：

```
===== M1 ACK 写死空 messageId（stream.go）→ go test -run TestStreamOpenConnectCallbackAckEchoesMessageID：exit=1 =====
--- FAIL: TestStreamOpenConnectCallbackAckEchoesMessageID (5.02s)
    stream_test.go:46: 5s 内没等到 messageId="frame-msg-001" 的 ACK
FAIL
FAIL	aite/edge/internal/dingtalk	5.041s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.093s
===== M2 缓存首个 ticket 复用（stream.go）→ go test -run TestReconnectReopensWithFreshTicket：exit=1 =====
--- FAIL: TestReconnectReopensWithFreshTicket (5.01s)
    stream_test.go:110: 5 秒内没等到客户端建 ws 连接
FAIL
FAIL	aite/edge/internal/dingtalk	5.044s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.056s
===== M3a ChatType 判反（normalize.go）→ go test -run TestNormalizeFixtures：exit=1 =====
--- FAIL: TestNormalizeFixtures (0.00s)
    --- FAIL: TestNormalizeFixtures/picture.json (0.00s)
        normalize_test.go:107: ChatType = CHAT_TYPE_GROUP，期望 CHAT_TYPE_P2P
    --- FAIL: TestNormalizeFixtures/quote_reply.json (0.00s)
        normalize_test.go:107: ChatType = CHAT_TYPE_P2P，期望 CHAT_TYPE_GROUP
    --- FAIL: TestNormalizeFixtures/quote_reply_string_content.json (0.00s)
        normalize_test.go:107: ChatType = CHAT_TYPE_P2P，期望 CHAT_TYPE_GROUP
    --- FAIL: TestNormalizeFixtures/text_group.json (0.00s)
        normalize_test.go:107: ChatType = CHAT_TYPE_P2P，期望 CHAT_TYPE_GROUP
    --- FAIL: TestNormalizeFixtures/text_p2p.json (0.00s)
        normalize_test.go:107: ChatType = CHAT_TYPE_GROUP，期望 CHAT_TYPE_P2P
    --- FAIL: TestNormalizeFixtures/rich_text.json (0.00s)
        normalize_test.go:107: ChatType = CHAT_TYPE_P2P，期望 CHAT_TYPE_GROUP
FAIL
FAIL	aite/edge/internal/dingtalk	0.032s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.034s
===== M3b 删掉剥 sessionWebhook（normalize.go）→ go test -run TestNormalizeFixtures：exit=1 =====
--- FAIL: TestNormalizeFixtures (0.00s)
    --- FAIL: TestNormalizeFixtures/rich_text.json (0.00s)
        normalize_test.go:148: Raw 里还留着 sessionWebhook
    --- FAIL: TestNormalizeFixtures/picture.json (0.00s)
        normalize_test.go:148: Raw 里还留着 sessionWebhook
    --- FAIL: TestNormalizeFixtures/quote_reply.json (0.00s)
        normalize_test.go:148: Raw 里还留着 sessionWebhook
    --- FAIL: TestNormalizeFixtures/quote_reply_string_content.json (0.00s)
        normalize_test.go:148: Raw 里还留着 sessionWebhook
    --- FAIL: TestNormalizeFixtures/text_group.json (0.00s)
        normalize_test.go:148: Raw 里还留着 sessionWebhook
    --- FAIL: TestNormalizeFixtures/text_p2p.json (0.00s)
        normalize_test.go:148: Raw 里还留着 sessionWebhook
FAIL
FAIL	aite/edge/internal/dingtalk	0.028s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.040s
===== M4 正则改成 #A\d+（anchor.go）→ go test -run TestAnchorTaskNoFromOwnTextThenQuote：exit=1 =====
--- FAIL: TestAnchorTaskNoFromOwnTextThenQuote (0.00s)
    anchor_test.go:30: TaskNoOf("#AH") = "",false，期望 "#AH"
    anchor_test.go:30: TaskNoOf("#AZ8") = "",false，期望 "#AZ8"
    anchor_test.go:30: TaskNoOf("看看 #ah 呢") = "",false，期望 "#AH"
    anchor_test.go:30: TaskNoOf("#AH进展如何") = "",false，期望 "#AH"
    anchor_test.go:48: 本文与引用都有时 TaskNo = <nil>，期望 #AZ8
    anchor_test.go:60: 只在引用里时 TaskNo = <nil>，期望 #AH
FAIL
FAIL	aite/edge/internal/dingtalk	0.030s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.032s
===== M5a 忽略 webhook 过期（outbound.go）→ go test -run TestSendTextUsesSessionWebhookThenRobotAPI：exit=1 =====
--- FAIL: TestSendTextUsesSessionWebhookThenRobotAPI (0.01s)
    outbound_test.go:165: 过期后群消息：MessageId="" webhook=2 group=0
FAIL
FAIL	aite/edge/internal/dingtalk	0.033s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.041s
===== M5b 去掉 token 缓存（api.go）→ go test -run TestAccessTokenIsCachedUntilExpiry：exit=1 =====
--- FAIL: TestAccessTokenIsCachedUntilExpiry (0.01s)
    outbound_test.go:36: 两次调用打了 2 次 token 端点，期望 1
FAIL
FAIL	aite/edge/internal/dingtalk	0.034s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.040s
===== M6 去掉切块整段发（card.go）→ go test -run TestCardStreamingChunksAtMost1KB：exit=1 =====
--- FAIL: TestCardStreamingChunksAtMost1KB (0.01s)
    card_test.go:136: 3 KB 只打了 1 次 PUT，期望 ≥3
FAIL
FAIL	aite/edge/internal/dingtalk	0.032s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.044s
===== M7a 卡片回调 ACK 挪到 HandleEvent 之后（callback.go）→ go test -run TestCardCallbackAckedWithin2sWhileSinkBlocks：exit=1 =====
--- FAIL: TestCardCallbackAckedWithin2sWhileSinkBlocks (2.01s)
    callback_test.go:53: 2s 内没等到 messageId="frame-card-001" 的 ACK
FAIL
FAIL	aite/edge/internal/dingtalk	2.038s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.197s
===== M7b 机器人消息在读循环里同步调 sink（stream.go）→ go test -run TestCardCallbackAckedWithin2sWhileSinkBlocks：exit=1 =====
--- FAIL: TestCardCallbackAckedWithin2sWhileSinkBlocks (2.11s)
    --- FAIL: TestCardCallbackAckedWithin2sWhileSinkBlocks/card_callback_not_blocked_by_slow_bot_message_sink (2.00s)
        callback_test.go:114: 2s 内没等到 messageId="frame-card-003" 的 ACK
FAIL
FAIL	aite/edge/internal/dingtalk	2.140s
FAIL
----- 还原后复跑：exit=0 ok  	aite/edge/internal/dingtalk	1.196s
git status --short 还原后：?? docs/p1/dingtalk.md
异常 0 条
exit=0
```

M7a 的具体改法：`c.ack(…)` 改成 `defer c.ack(…)`，并把投递的 `go func()` 改成同步 `func()` —— 即 ACK 发生在 `HandleEvent` 返回之后。
M4 里「本文 / 引用组合」两格也用了字母号（`#AZ8` / `#AH`），一并红，符合派单预期。

## 4. 验收（§7）与 check.sh 完整输出

原始日志 `/tmp/cc9-accept.log`、`/tmp/cc9-check2.log`（在代码提交 `6be527c` + docs 提交之上跑；之后只改了 `stream_test.go` 一行注释与本回执，已复跑第 1 条确认）。

1. `cd edge && go test -race ./internal/dingtalk/... -count=1` →
   ```
   ok  	aite/edge/internal/dingtalk	1.339s
   ```
2. `-count=3 -v -run '<6 个名字>'`（按名字计数，去掉耗时列）→ 6 个名字各 3 次 `--- PASS`，`FAIL` 行数 0：
   ```
         3 --- PASS: TestAnchorTaskNoFromOwnTextThenQuote
         3 --- PASS: TestCardCallbackAckedWithin2sWhileSinkBlocks
         3 --- PASS: TestCardStreamingChunksAtMost1KB
         3 --- PASS: TestNormalizeFixtures
         3 --- PASS: TestReconnectReopensWithFreshTicket
         3 --- PASS: TestStreamOpenConnectCallbackAckEchoesMessageID
   FAIL 行数：0
   ```
3. `go test -race ./internal/dingtalk/... -count=1 -v 2>&1 | grep -c '^--- PASS'` → `14`（≥ 13：派单点名的 13 个 + `TestBackoffDelaySequence`）。
4. `cd edge && go vet ./... && gofmt -l . | wc -l` → 输出 `0`，exit 0。
5. 全量 Go（只跑这一遍）：
   ```
   8
   10
   0
   ok  	aite/edge/cmd/aite-edge	6.405s
   ?   	aite/edge/gen/aitepb	[no test files]
   ok  	aite/edge/internal/aiteerr	1.019s
   ok  	aite/edge/internal/config	1.020s
   ok  	aite/edge/internal/dingtalk	1.329s
   ok  	aite/edge/internal/feishu	4.905s
   ok  	aite/edge/internal/ingress	1.341s
   ?   	aite/edge/internal/pin	[no test files]
   ok  	aite/edge/internal/sandbox	1.035s
   ok  	aite/edge/internal/server	1.044s
   ```
   即 N0 + 1 = 8、`10`、`0`。
6. `scripts/check.sh` 完整输出（原样）：
   ```
   === A1 cargo build --workspace ===
   $ bash -c cd core && cargo build --workspace
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.33s
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
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.37s
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
   go packages ok=10 fail=0
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
   `cargo passed=946+0 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`（B8 不变量未动）、`go packages ok=10 fail=0`。
   单跑 `cd edge && go test -race ./cmd/... -count=1` → `ok  	aite/edge/cmd/aite-edge	6.012s`。
7. 没有 `origin` 远端，`origin/main` 不存在，改用本地 `main`（= `30b00e5`，与派单基线同一提交）：`git diff --name-only main...HEAD` →
   `docs/p1/dingtalk.md` + 22 个 `edge/internal/dingtalk/…` 路径（8 源文件、8 测试、6 夹具）；加上本回执提交后多一行 `review/p1/ledger/CC9.md`。全部在可写面内。
8. `grep -rn '"aite/edge/internal/feishu"' edge/internal/dingtalk` → 无输出（exit 1）；`ls edge/internal/dingtalk` 里没有 `stream_card.go`、`stream_card_test.go`、`dws.go`、`dws_test.go`。
9. `git status --short` → 空（验收时）。临时脚本都在会话 scratchpad，不在仓库里。

## 5. `cargo passed` 增量

Δ = 0（本轨只动 Go 与文档，不碰 Rust）：`cargo passed=946` → `946`，`contracts passed=25` → `25`。
Go 侧 `^ok` 从 N0 = 7 → 8，多出来的那一行是 `ok  	aite/edge/internal/dingtalk`；`^(ok|\?)` 从 9 → 10；check.sh 的 `go packages ok=9` → `ok=10`。
（原卡写的「10 = 9 + 本包」指 `^ok` 行数，按源码实测是 7 → 8；两个数都在这里。）

## 6. 被守卫拦过的命令

无（开场自检第 2 步那条除外，已在第 1 节）。

## 7. 记账转出去的

| 事项 | 为什么不在本轨 | 建议归哪轨 |
|---|---|---|
| 接线：平台工厂按 `Start / Connected / ReconnectCount` 起本包、EdgeStatus 读连接状态 | `edge/cmd/aite-edge/**` 是 P0-CLOSE / R0 面 | DD8 |
| `config` 放行 `platform: dingtalk`、`config.Dingtalk` → 本包 `Config` / `Options`（从环境变量解析凭证） | `edge/internal/config/**` 是 R0 | DD8 |
| 能力值对齐契约 `dingtalk_v1()`（含 14 个新能力位；本包三个保守值 `CardEditWindowSec=0` / `ProactiveRequiresPriorMessage=false` / `OutboundRatePerMin=20`） | 新能力位的 pb 字段 T0 之后才有 | DD11 |
| `NormalizedEvent.quote` 用 `ParseQuote` 填 | `quote` 字段 T0 之后才有；原卡没写归谁 | DD11 |
| Approve / Reject 卡片回调 | `CardActionKind` 新值 T0 之后才有 | DD11 |
| @ 人（`atUserIds`，对应 `OutboundText.mentions`） | 契约字段 T0 之后才有 | DD11 |
| `SendFile`（今天 `ErrNotImplemented`） | 原卡没列 | DD11 |
| `AddReaction`（今天 `ErrNotImplemented`；core 失败只 warn） | 原卡没列；钉钉机器人可能根本没有对应接口，待确认 | DD11 |
| file / audio / video 消息归一化（今天返回 nil 不上送） | 原卡只列 text / richText / picture | DD11 |
| 单聊主动发（缓存里没有该会话的 `senderStaffId` 时今天直接报不可重试错误） | 需要 staffId 来源（通讯录 / DirectSender） | DD11 |
| 客户端保活 ping（今天只被动回应服务端 ping） | 原卡没列；SDK 有客户端 keepalive | DD11 |
| OAuth | 原卡没列 | DD11 |
| 增量流式（`StreamCard` 今天每次全量重发） | 原卡把增量留给 FF3；`stream_card.go` 名字留给它 | FF3 |
| 可见 `#A` 前缀（Aite 回帖 / 卡片里带 `#AH`，让用户引用或手打时能续接） | 出站文案是 core 的 | DD3 / DD5 |
| 按 `Anchor.task_no` 路由 | core 路由面 | DD3 |
| `dws.go` / `dws_test.go` | 名字留给它 | HH3 |
| H9 探针结果并进 `docs/p1/dingtalk.md` §10 | W2 起 docs 归 DD11 | 总管（探针）→ DD11 |

## 8. 没做的与原因

- **推送与 draft PR**：起初做不成（见文首），总管要求后已补上推送与 draft PR。
- **一切只在假件上验过**：httptest 假 REST、假 Stream 网关、gorilla 假 ws 服务端；没连过真钉钉（云端打不开 open.dingtalk.com，也没有凭证）。
  等 H9 真机核实的点逐条列在 `docs/p1/dingtalk.md` §9（9 条）：`repliedMsg` 真实形状；`repliedMsg.msgId` 能否对上 `processQueryKey` / `outTrackId` / 原 `msgId`；
  群里 `text.content` 带不带 `@名`；失败 ACK 会不会重推、回调 ACK data 形状；卡片回调时延与 data 形状；`PUT /v1.0/card/instances` 能更新多久；
  全部 REST 路径 / 请求头 / 请求体键名（来自调研记忆）；`card/streaming` 追加语义与 1 KB 计法；三个保守能力值。
- 没做客户端 keepalive、没做 REST 退避重试（重试由 core 按 retryable 决定；feishu 自带 3 次退避，本包没照搬 —— 派单只要求 401 重取一次）。
- 没做出站令牌桶限速（`OutboundRatePerMin=20` 只是声明值；feishu 有 `TokenBucket`，本包没照搬，DD11 对齐能力值时一并考虑）。

## 9. 契约缺口（T0 已计划内容之外）

无。本包用今天的 pb 字段全部表达得了；`quote` / Approve·Reject / `mentions` / 新能力位都在 T0 已计划清单里。

## 交付状态

| 项 | 状态 |
|---|---|
| 本地分支 `claude/cc9-dingtalk-stream` | 提交见下 |
| 推送 | 已推到 `origin/claude/cc9-dingtalk-stream` |
| draft PR | 标题「CC9: 钉钉 Stream 适配器包（不接线）」，正文 = 本文件 |

`git log --oneline main..claude/cc9-dingtalk-stream` 共四个提交：代码包、`docs/p1/dingtalk.md`、本回执（附带 `stream_test.go` 一行注释修正）、回执交付状态更新（推送 / PR）。
