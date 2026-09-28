# CC8 回执：飞书适配器整理 + 新事件 + 卡片按钮开关

- 分支：`claude/cc8-feishu-adapter`（本地 9 个提交：①–⑧ + 本回执）· PR：**未能开**（见「⚠ 推送与 PR」）
- 代码基线：`8458435`（情形 A）· 可写面：`edge/internal/feishu/**`、`review/p1/ledger/CC8.md`
- 结论：8 项全部做完，每项有回归测试与变异验证；`scripts/check.sh` 全部通过，`cargo passed=946`（Δ=0）；
  卡片按钮**默认关**，只有 `AITE_FEISHU_CARD_BUTTONS` 恰好等于 `"1"` 才渲染。

## ⚠ 推送与 PR：没推上去

会话容器里的仓库**没有 `origin` 远端**，也**没有 `gh`**。我按仓库里记着的地址（`review/p1/ledger/CC2.md` 等：`TomwaltW/aite`）
加了 `origin` 再推，被会话的 git 代理拒了（原文）：

```
remote: access denied by the git proxy: TomwaltW/aite is not in this session's authorized repository set, so the proxy will not inject a credential for it. To fix, add the repository to the session's sources.
fatal: unable to access 'https://github.com/TomwaltW/aite.git/': The requested URL returned error: 403
```

这是会话的权限设置，没有绕。提交全在本地分支 `claude/cc8-feishu-adapter` 上（`git log --oneline main..HEAD` 见文末）。
**要做的事**：把 `TomwaltW/aite` 加进这个会话（或新会话）的 sources 后，`git push -u origin claude/cc8-feishu-adapter`，
再 `gh pr create --draft --title "CC8: 飞书适配器整理 + 新事件 + 卡片按钮开关" --body-file review/p1/ledger/CC8.md`
（`gh` 也要装）。PR 描述草稿在 `/tmp/cc8-pr.md`（容器回收即丢，以本回执为准）。

## 1. 开场自检

### 第 1 步：代码基线 → 情形 A

```
$ git cat-file -e 8458435 || git fetch -q --unshallow origin      # 对象在，没触发 fetch
$ git diff --stat --no-renames --diff-filter=AM 8458435 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'
（空）
```

HEAD = `30b00e5`（总管 D0 文档提交，在 8458435 之上）。输出为空 → **情形 A**，基线行 `cargo passed=946`、`contracts passed=25`。

### 第 2 步：守卫挂上了（被拦 = 通过）

用 Read 工具读 `.claude/hooks/guard_bash.py`，拦截原文：

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

### 第 4 步：`scripts/check.sh`（开工前，原样全文）

```

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
   Compiling aite-models v0.0.1 (/home/user/repo/core/crates/models)
   Compiling aite-gateway v0.0.1 (/home/user/repo/core/crates/gateway)
   Compiling aite-githost v0.0.1 (/home/user/repo/core/crates/githost)
   Compiling aite-routines v0.0.1 (/home/user/repo/core/crates/routines)
   Compiling aite-memory v0.0.1 (/home/user/repo/core/crates/memory)
   Compiling aite-search v0.0.1 (/home/user/repo/core/crates/search)
   Compiling aite v0.0.1 (/home/user/repo/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 12s
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
    Checking aite-githost v0.0.1 (/home/user/repo/core/crates/githost)
    Checking aite-memory v0.0.1 (/home/user/repo/core/crates/memory)
    Checking aite-routines v0.0.1 (/home/user/repo/core/crates/routines)
    Checking aite-search v0.0.1 (/home/user/repo/core/crates/search)
    Checking aite v0.0.1 (/home/user/repo/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 01s
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

各判定行与情形 A 逐字一致。CC1 之后 Go 那格已是 `go packages ok=N fail=M` 单行口径。

### 第 5 步：本轨附加底

```
$ (cd edge && go test -race ./... -count=1) > /tmp/cc8-go-before.txt 2>&1; grep -cE '^(ok|\?)' …; grep -c '^ok' …; grep -c FAIL …
9
7
0
ok  	aite/edge/cmd/aite-edge	5.743s
?   	aite/edge/gen/aitepb	[no test files]
ok  	aite/edge/internal/aiteerr	1.015s
ok  	aite/edge/internal/config	1.028s
ok  	aite/edge/internal/feishu	4.791s
ok  	aite/edge/internal/ingress	1.340s
?   	aite/edge/internal/pin	[no test files]
ok  	aite/edge/internal/sandbox	1.028s
ok  	aite/edge/internal/server	1.043s
```

**注**：原卡写的「`grep -c '^ok'` → 9」有误，按 **7 ok + 2 ?** 验（`?` 两行不以 `ok` 开头）。

- `go test -list` 顶层测试：**115**（与派单预期一致）。
- `--- PASS` 总数 P0 = **172**（`--- FAIL` 0）。
- 逐文件：capabilities 14 / card_frames 5 / cards 18 / errors 14 / helpers 0 / normalize 22 / outbound 14 / read 14 / reconnect 14。
- 导出面快照 `/tmp/cc8-api-before.txt`（23 行）。

## 2. 工作项逐条

下文 `文件:行` 指本分支 HEAD。

### ① 零行为变化拆文件（`9ae4147`）

用一个按 8458435 行号切片的脚本搬代码（只搬、不改逻辑、不改名），能 `git mv` 的先 `git mv`：
`connection.go→events.go`、`errors_test.go→api_test.go`、`read_test.go→reads_test.go`、`reconnect_test.go→platform_test.go`、
`card_frames_test.go→events_test.go`；`ratelimit.go` 删除（并进 `api.go` 尾部「令牌桶（原 ratelimit.go）」一节）。
新建 `capabilities.go / reads.go / outbound.go`。之后只手修了 import 块与 gofmt 的多余空行。

四样判据（原样）：

```
$ git show --stat HEAD
 edge/internal/feishu/api.go                        | 117 +++++
 .../feishu/{errors_test.go => api_test.go}         | 105 +++++
 edge/internal/feishu/capabilities.go               |  41 ++
 edge/internal/feishu/capabilities_test.go          | 197 ---------
 edge/internal/feishu/{connection.go => events.go}  |  71 ++-
 .../feishu/{card_frames_test.go => events_test.go} | 167 +++++++
 edge/internal/feishu/helpers_test.go               |  43 ++
 edge/internal/feishu/normalize.go                  |  17 -
 edge/internal/feishu/outbound.go                   | 184 ++++++++
 edge/internal/feishu/platform.go                   | 488 +--------------------
 .../feishu/{reconnect_test.go => platform_test.go} | 254 +++--------
 edge/internal/feishu/ratelimit.go                  | 120 -----
 edge/internal/feishu/reads.go                      | 244 +++++++++++
 .../feishu/{read_test.go => reads_test.go}         |   0
 14 files changed, 1037 insertions(+), 1011 deletions(-)

$ diff /tmp/cc8-tests-before.txt <拆分后 go test -list 排序>   → 空（TESTLIST-SAME，115 条）
$ --- PASS 总数                                                 → 172 = P0（--- FAIL 0）
$ diff /tmp/cc8-api-before.txt <拆分后 go doc -short 排序>      → 空（API-SAME）
$ (cd edge && go vet ./... && gofmt -l . | wc -l)               → 0
```

逐文件条数对照：

| 新文件 | 条数 | 来源 |
|---|---|---|
| api_test.go | 19 | errors_test 14 + capabilities_test 令牌桶 5 |
| platform_test.go | 12 | reconnect_test 退避 / 重连 9 + capabilities_test 装配 3 |
| reads_test.go | 14 | read_test 14 |
| events_test.go | 10 | card_frames_test 5 + reconnect_test 投递 5 |
| capabilities_test.go | 6 | 原 14 − 5 − 3 |
| cards_test.go | 18 | 不动 |
| normalize_test.go | 22 | 不动 |
| outbound_test.go | 14 | 不动 |
| **合计** | **115** | |

测试辅助：`recordingSink`、`dispatchPlatform` → `helpers_test.go`；`expectedBackoff / recordingSleep / fakeConnection / connectionScript / reconnectHarness` 留 `platform_test.go`。

### ② 事件分发表（`7865330`，零行为变化）

- `events.go:39-99`：`normalizeFunc`、`eventEntry{normalize, callback, onDropped}`、`eventTable` + `sync.RWMutex`、
  `registerEvent`（:69，重复登记 / 非 card.action.trigger 的 callback 项都 panic）、`lookupEvent`（:85）、`eventTableSnapshot`（:93，读侧取快照再用）。
- `buildDispatcher`（`events.go:193`）遍历快照：`callback` 项走 `OnP2CardActionTrigger`，其余走 `OnCustomizedEvent`。
- `Normalize`（`events.go:421`）查表；`dispatchRaw`（`events.go:386`）在 normalize 返回 nil 时调该项的 `onDropped`（没有就照旧 `feishu.event_ignored`）。
- 两项 P0 事件在 `normalize.go:43` 的 `init()` 登记。测试专用 `registerTestEvent / unregisterTestEvent` 在 `helpers_test.go`（不导出，撤销挂 `t.Cleanup`）。

### ③ 订阅新事件（`8983f2a`）

| 事件 | → | 归一化 |
|---|---|---|
| `im.message.recalled_v1` | `EVENT_KIND_MESSAGE_DELETED` | `normalize.go:700` `normalizeRecalled` |
| `im.chat.member.bot.added_v1` | `EVENT_KIND_BOT_ADDED` | `normalize.go:709` `normalizeBotAdded` |
| `im.chat.member.user.added_v1` / `deleted_v1` | `EVENT_KIND_MEMBER_CHANGED` | `normalize.go:720` `normalizeMemberChanged` |
| `im.message.reaction.created_v1` / `deleted_v1` | 不上送 | `normalize.go:739-780` `reactionEvent / parseReaction / dropReaction / logReactionDropped` |

共用形状 `chatEvent`（`normalize.go:662`）：`event_id` 只取 `header.event_id`、缺了返回 nil（`onDropped` = `logMalformed` 打 Debug `feishu.event_malformed`）；
`anchor` 恒非 nil（`platform=feishu`、`chat_id`，入群 / 成员事件 `message_id` 留空）；`chat_type=GROUP`；`occurred_at = toTime(header.create_time)`、解析不了回退 `timeNow()`；
`mentioned=false`、`text=""`、`raw=rawStruct(raw)`；`workspace_id` 同老口径（`header.app_id` 优先）。

**字段路径 ← SDK（lark-oapi-go v3.12.0，`service/im/v1/model.go`）**（云端打不开 open.feishu.cn 文档，不贴 URL）：

- 撤回：`event.message_id`、`event.chat_id` ← `P2MessageRecalledV1Data`（:16640；另有 `recall_time / recall_type`，未用）
- 机器人入群：`event.chat_id`、`event.operator_id.open_id` ← `P2ChatMemberBotAddedV1Data`（:16498）；`UserId.open_id` ← `UserId`（:8453）
- 成员加入：同上 ← `P2ChatMemberUserAddedV1Data`（:16546）；`event.users[]`（`ChatMemberUser`，:1432）只留在 raw
- 成员移除：同上 ← `P2ChatMemberUserDeletedV1Data`（:16572）
- 表情增 / 删：`event.message_id`、`event.reaction_type.emoji_type`（`Emoji`，:2751）、`event.operator_type`、`event.user_id.open_id`、`event.app_id`
  ← `P2MessageReactionCreatedV1Data`（:16676）/ `P2MessageReactionDeletedV1Data`（:16700）

**`sender_kind` 取舍**（给 DD3）：非 HUMAN 在 R1 就被丢、到不了 R4，所以四类都是 `HUMAN`。入群 / 成员事件 `sender_id` = 操作者 open_id。
撤回事件体里**没有操作者**（SDK 结构体只有 `message_id / chat_id / recall_time / recall_type`，`recall_type` 取值 SDK 注释没列），
撤回人可能是发送者本人或群主 / 管理员，所以一律 `HUMAN`、`sender_id` 留空。DD3 做「删根」语义时，**别拿 sender_id 判断谁撤的**；
若要区分「本人撤回 / 管理员撤回」，只能等 `recall_type` 的真机取值（raw 里有，但 events.proto 规定逻辑不得依赖 raw）。

夹具：`testdata/events/{recalled,bot_added,member_added,member_deleted}.json` + `.expected.json`（`-update` 生成）；
`testdata/events/reaction/reaction_{created,deleted}.json`（子目录，不进黄金循环，按显式路径读）。
`helpers_test.go` 的 `fixtureRoots` = 老根 `../../testdata/feishu` + 新根 `testdata/events`，`fixtureDir(name)` 决定读 / 写回哪个根；
`requiredFixtures` 与 `TestRequiredFixtureExists` 只看老根。`-update` 之后 `git status --short -- edge/testdata` 为空。
`testdata/read/README.md`（归 DD9）、`testdata/write/README.md`（归 DD10）：本轨读写测试用内联 JSON，目录只占位。

### ④ 卡片按钮开关 + 帧类型日志（`e35ce22`）

- `cards.go:87` `cardElements(…, buttons bool)`：`buttons` 且 `buildActions(card)` 非空时多一个 `{"tag":"action","actions":…}`（:163），**文字提示照留**；
  `cards.go:233` `buildChecklistCardWith(card, buttons)`，导出的 `BuildChecklistCard(card)` = 不带按钮（签名不变）。`cards.go:120` 起的注释改写（默认关、H8 定、DD10 翻）。
- `platform.go`：`Platform.cardButtons` / `platformOptions.cardButtons`；默认工厂把它传给 `larkConnection.cardButtons`；`New()` 读 `AITE_FEISHU_CARD_BUTTONS == "1"`（`platform.go:187`、:198 起）。
- `outbound.go`：`SendCard` / `UpdateCard` 用 `buildChecklistCardWith(card, p.cardButtons)`。
- 帧类型日志：(a) `events.go:207` —— `OnP2CardActionTrigger` 回调里**总是** INFO `feishu.card_frame frame_type=event event_id=…`；
  (b) `events.go:246` —— 仅开关开时 `larkws.WithLogger(sdkLogAdapter)`；`sdkLogAdapter`（`events.go:328`）摊平 :77 那条嵌套切片参数，
  认出 `receive message, message_type: card` 打 INFO `feishu.card_frame frame_type=card`，**不记 payload**；Debug / Info 一律丢，Warn / Error 转 `feishu.sdk`。
  开关关时建连参数与原来逐字相同（`WithLogLevel(Warn)`、不装 logger）。
  「不记 payload」对 Warn / Error 也成立：逐个查过 SDK `ws/*.go` 的 `logger.Warn / logger.Error` 调用点（client_lifecycle / client_message /
  client_session / client_transport），格式串里没有一个带 payload；`client_message.go:109/115` 带的是 handler 返回的 err 文本。
  `client_session.go:99` 会带 endpoint（长连接 URL，可能含 ticket 参数）—— 这条今天 SDK 的默认 logger 在 Warn 级也照样打到 stdout，不是本轨新增的泄漏面。
- core 只会发 `Stop`（`control/src/card.rs`、`worker/src/card.rs` 硬编码 `vec![Stop]`），「证据」按钮只在单测里出现 —— 没去改 core。

### ⑤ 话题历史走 thread 容器（`cd52259`）

- `reads.go:44` `ReadHistory`：带 threadID 先走 `readThreadHistory`（:138）：`getMessage`（:164，`GET PathMessage`，取 `data.items[0]`）拿 root 的 `thread_id`，
  取到就 `listMessages(ctx, "thread", omt_…, wanted)`（:84，`sort_type / page_size / with_sender_name` 同口径，预算不 ×4），
  `withRoot`（:180）把 root 按 `create_time` 插回（列表里有就不动），`recentOldestFirst` 取最近 limit 条正序。`HistoryMessage.thread_id` 仍只看 root_id。
- 回落（整群拉取 ×4 + `inThread` 筛，原逻辑照搬，只是分页循环抽成了 `listMessages`）只有两种：
  ① root 查询出错或没有 `thread_id`（Debug `feishu.thread_history_fallback reason=root_lookup_failed / root_has_no_thread_id`）；
  ② thread 列表回**权限错误**（`reason=thread_permission_denied`）。其它错误照常上抛。
- 权限判据 `reads.go:205-224`：`permissionDeniedHTTPStatus = 403` + `permissionDeniedCodes = {99991400, 99991401, 230002}`。
  **出处**：SDK `channel/types/errors.go:79-80`（业务码 → `ErrCodePermissionDenied`）与 `:96-97`（HTTP 401 / 403）。
  HTTP 401 在 `apiClient.request` 里已被「换 token 再打一次」吃掉，常量只认 403。
  **疑点**：`99991400` 在飞书通用错误码里可能是限流、不是权限，SDK 这张分类表是否可靠未核实 —— **待 H7 真机补**。

### ⑥ 发言人姓名（`564e9d1`）

- `reads.go:370` 起：`senderNameLookup`（`container/list` + map 的 LRU），`GET /open-apis/contact/v3/users/{open_id}?user_id_type=open_id`
  （`PathContactUser`，`api.go:55`；形状照 SDK `GetUserRespData.user`（contact/v3 model.go:12819）→ `User.name`（:4441）；路径照 `resource.go:1884`），取 `data.user.name`。
- 取值与理由：
  - `senderNameCacheSize = 1024`：一个群常说话的几十到几百人，edge 接的群有限；每条百来字节，不到 1MB。名字几乎不变，**不设 TTL**（改名等挤出 / 重启生效，只影响显示）。
  - `senderNameTimeout = 300ms`：事件要 1s 内交 core（`onEventBudget`），查名字是锦上添花；这个 ctx 同时截断 `rawRequest` 的退避重试。
  - `senderNameTripFor = 10min`：权限错误（同 ⑤ 的判据）熔断 10 分钟 + 每次熔断一条 WARN `feishu.sender_name_denied`；其它失败 Debug `feishu.sender_name_failed`。
  - **已知缺口（生产里最先咬人的一条）**：熔断只认 ⑤ 那张清单（HTTP 403 + 99991400 / 99991401 / 230002）。测试里的「没权限」响应是 HTTP 403 + 业务码 99991672，
    熔断靠的是 403 而不是这个码；若真机通讯录接口没权限时回的是 HTTP 200 / 400 + 某个不在清单里的业务码，`isPermissionError` 为 false → 不熔断、不打 WARN，
    H5 授权 `contact:user.base:readonly` 之前**每条人类消息都要多付一次失败的通讯录往返**（仍有 300ms 上限，不影响投递）。码值按派单不许猜，待 H7 真机补进清单。
- `fillSenderName`（`reads.go:471`）在 `dispatchRaw` 里 `Normalize` 之后、交 sink 之前调（`events.go:402`）：仅 MESSAGE / CARD_ACTION、HUMAN、`sender_id` 非空且 `sender_name` 为空。
- **只在 `New()` 里装配**（`platformOptions.senderNames=true`，`platform.go:209`）；`newPlatform` 默认 false，`dispatchPlatform` 等测试不打公网；`Normalize` 仍是纯函数，老黄金文件零变化。
  通讯录读不过出站令牌桶（`rateLimited` 未设）。要 `contact:user.base:readonly`（H5）。

### ⑦ 包内读环境变量（`6d5fc3f`）

`platform.go:185-219`：`EnvCardButtons / EnvPassiveListen / EnvAPIBase` 三个导出常量；`New()` 里：
`AITE_FEISHU_PASSIVE_LISTEN == "1"` → `SetPassiveListen(true)`（它的第一个调用方）；`AITE_FEISHU_API_BASE` 非空且 `Options.Domain` 空 → 当域名（REST 与长连接同一个，显式 `Options.Domain` 优先）；
按钮见 ④。打一行 INFO `feishu.env_flags card_buttons=… passive_listen=… api_base=…`。`newPlatform` 不读环境；`main.go` 一个字没动。

### ⑧ 每群 5 QPS + 发送 uuid（`f610c55`）

- `outbound.go:98-210`：`chatPacer`，每群一个 `TokenBucket(300/分, 容量 5, 同一套可注入时钟)`。桶表上限 `chatBucketsMax = 1024`：
  超了先淘汰闲置 ≥ `chatBucketIdle = 1s` 的（5 个令牌按 5/s 补，1s 就满，淘汰等价于重建，无损），还超就淘汰最久没用的。
- `sendMessage`（`outbound.go:55`）先过群桶（chat id 空跳过），再由 `api.request` 过全局桶；群桶等待被 ctx 取消时报 `transport_error`（retryable，与全局桶同口径）。
- **`UpdateCard` 过每群桶：做了**。`SendCard` 成功后 `rememberCard(card_id, chat_id)`（`outbound.go:234`，有界 FIFO `cardChatsMax = 4096`），
  `UpdateCard` 查得到就过该群的桶（`outbound.go:246`），查不到（进程重启前发的卡）只过全局桶。
- uuid：`newMessageUUID()` = `crypto/rand.Text()`（26 字符 base32，≤50），每次 `sendMessage` 生成一次，放进「发送」「回复」两种请求体；
  `rawRequest` 重试复用同一个 body map、同一个 uuid。给 DD10 的内部入参：`sendMessage(…, dedupeKey string)`，非空时直接当 uuid（现有调用方都传 `""`）。
  PATCH 不带 uuid（`TestUpdateCardUsesPatchNotSend` 的 `len(body) == 1` 仍绿）。
- `TestOutboundIsRateLimitedByTheCapability` 仍恰好 `[30s]`、`TestUploadsDoNotConsumeTheOutboundQuota` 仍 `[]`。

## 3. 新增测试与变异验证

新增顶层测试 12 条（115 → 127）：

| 文件 | 测试 | 钉什么 |
|---|---|---|
| events_test.go | `TestHandlerTableDrivesDispatcherAndNormalize` | 表里每一项经 `buildDispatcher().Do` 到 onRaw 且被 Normalize 认（表情项认、归一化为 nil）；临时登记假类型不改 events.go 就能投递 + 归一化 + 进 sink |
| events_test.go | `TestDuplicateEventRegistrationPanics` | 重复登记 panic |
| events_test.go | `TestReactionIsParsedAndDropped` | 表情结构体字段；`dispatchRaw` 不进 sink、返回 nil、Debug `feishu.reaction_dropped`；`Do` 返回 nil（不是 NotFound） |
| events_test.go | `TestCardActionTriggerArrivesAsEventFrame` | Platform 一层真 websocket：event 帧 → sink 收 CARD_ACTION（STOP / t-1 / om_checklist_card_0001）+ INFO `frame_type=event`；子测试（开关开）card 帧 → sink 0、`frame_type=card`、日志里无两个令牌 |
| normalize_test.go | `TestNewEventKindsNormalize` | 4 个新夹具的 kind / event_id / chat_id / anchor.message_id / sender_id / sender_kind 字面量 + 契约闸门形状；缺 event_id → nil；occurred_at 与回退 |
| cards_test.go | `TestButtonsOnlyWithFlag` | 关：无 action、有提示；开：恰好一个 action、按钮 = buildActions、提示在、schema 过；`t.Setenv`+`New`+假飞书：`"1"` 时 SendCard / UpdateCard 都带，`"true"`/`"0"`/`""` 都不带 |
| reads_test.go | `TestThreadHistoryUsesThreadContainer` | (i) thread 容器、无 chat 请求、含 root 正序、page_size 不 ×4；(ii) 权限错误（230002 / HTTP 403）→ 恰好一次 chat 请求、结果同整群筛法；(iii) 非权限错误上抛；root 查询失败 → 回落不上抛 |
| reads_test.go | `TestSenderNameFromContactAPIIsCached` | 同一发送人两条 → 通讯录 1 次、两条都有名字；`user_id_type=open_id`；机器人消息不查 |
| reads_test.go | `TestSenderNameDegradesWithin300ms` | 慢路由 → 事件照送、名字 nil、< 1s；权限错误 → nil、熔断期内不再请求、恰好一条 WARN、熔断期过后重试 |
| platform_test.go | `TestEnvFlagsAreReadInsideThePackage` | 三个开关各自生效；全清空 → FeishuP0 + DefaultDomain + 按钮关；只有恰好 `"1"` 开 |
| outbound_test.go | `TestPerChatPacing` | 同群第 6 条等 200ms、另一群不受影响；全局额度小时全局桶照卡（[30s]）；UpdateCard 过卡片所在群的桶 |
| outbound_test.go | `TestSendCarriesAStableUUIDAcrossRetries` | 500→200 两次请求同一个非空 ≤50 的 uuid；两次独立发送不同；回复接口也带 |

### 变异验证（改动撤回 → 红，输出逐字）

**② 让 `buildDispatcher` 不遍历表**（换成写死的两项）：

```
--- FAIL: TestHandlerTableDrivesDispatcherAndNormalize (0.00s)
    events_test.go:665: 登记之后 Do 该认出假事件：event type: test.fake_event_v1, not found handler
FAIL
FAIL	aite/edge/internal/feishu	0.027s
```

**③ 从表里摘掉表情登记**：

```
--- FAIL: TestReactionIsParsedAndDropped (0.00s)
    --- FAIL: TestReactionIsParsedAndDropped/reaction_created (0.00s)
        events_test.go:648: 该打一条 Debug 的 feishu.reaction_dropped，得到 []
        events_test.go:655: buildDispatcher().Do 对表情事件该返回 nil，得到 event type: im.message.reaction.created_v1, not found handler
    --- FAIL: TestReactionIsParsedAndDropped/reaction_deleted (0.00s)
        events_test.go:648: 该打一条 Debug 的 feishu.reaction_dropped，得到 []
        events_test.go:655: buildDispatcher().Do 对表情事件该返回 nil，得到 event type: im.message.reaction.deleted_v1, not found handler
FAIL
```

**④a 开关恒关**：

```
--- FAIL: TestButtonsOnlyWithFlag (0.03s)
    --- FAIL: TestButtonsOnlyWithFlag/env=1 (0.01s)
        cards_test.go:641: SendCard：开关 "1" 该带按钮，action 元素 0 个
        cards_test.go:641: UpdateCard：开关 "1" 该带按钮，action 元素 0 个
FAIL
```

**④b 开关恒开**：

```
--- FAIL: TestButtonsOnlyWithFlag (0.03s)
    --- FAIL: TestButtonsOnlyWithFlag/env=true (0.01s)
        cards_test.go:644: SendCard：开关 "true" 不该带按钮，action 元素 1 个
        cards_test.go:644: UpdateCard：开关 "true" 不该带按钮，action 元素 1 个
    --- FAIL: TestButtonsOnlyWithFlag/env=0 (0.01s)
        cards_test.go:644: SendCard：开关 "0" 不该带按钮，action 元素 1 个
        cards_test.go:644: UpdateCard：开关 "0" 不该带按钮，action 元素 1 个
    --- FAIL: TestButtonsOnlyWithFlag/env= (0.01s)
        cards_test.go:644: SendCard：开关 "" 不该带按钮，action 元素 1 个
        cards_test.go:644: UpdateCard：开关 "" 不该带按钮，action 元素 1 个
FAIL
```

**④c 删 (a)（回调里的 frame_type=event 日志）**：

```
--- FAIL: TestCardActionTriggerArrivesAsEventFrame (0.23s)
    events_test.go:360: 该打一条 INFO 的 feishu.card_frame，得到 []
FAIL
```

**④d 删 (b)（不装 WithLogger）**：

```
--- FAIL: TestCardActionTriggerArrivesAsEventFrame (2.45s)
    --- FAIL: TestCardActionTriggerArrivesAsEventFrame/buttons-on/type=card (2.22s)
        events_test.go:390: feishu.card_frame 的 frame_type = []，要 [card]
FAIL
```

**⑤a 恒走 chat 容器**（`ReadHistory` 跳过 thread 分支）：

```
--- FAIL: TestThreadHistoryUsesThreadContainer (0.02s)
    --- FAIL: TestThreadHistoryUsesThreadContainer/thread_容器 (0.01s)
        reads_test.go:284: 列表请求 = [chat:oc_chat_p0_demo_0001]，要恰好一次 thread:omt_x、没有 chat 容器请求
        reads_test.go:291: query[page_size] = "40"，要 "10"（预算不再 ×4）
        reads_test.go:295: message_id = [om_r1 om_r2]，要含 root 且正序
    --- FAIL: TestThreadHistoryUsesThreadContainer/thread_列表权限错误_→_回落整群筛法 (0.01s)
        --- FAIL: …/业务码_230002 (0.00s)
            reads_test.go:312: 权限错误该回落而不是上抛：230002: bot not in chat
        --- FAIL: …/HTTP_403 (0.00s)
            reads_test.go:312: 权限错误该回落而不是上抛：99999: forbidden
    --- FAIL: TestThreadHistoryUsesThreadContainer/非权限错误上抛；root_查询失败回落 (0.01s)
        reads_test.go:335: 非权限错误不该回落，列表请求 = [chat:oc_chat_p0_demo_0001]
FAIL
```

（这一格里 (ii) 也红，是因为 fake 路由的第一个响应是权限错误、恒走 chat 时它落到了 chat 请求上 —— 不影响「(i) 红」这条判据。）

**⑤b 删权限回落**：

```
--- FAIL: TestThreadHistoryUsesThreadContainer (0.02s)
    --- FAIL: TestThreadHistoryUsesThreadContainer/thread_列表权限错误_→_回落整群筛法 (0.01s)
        --- FAIL: TestThreadHistoryUsesThreadContainer/thread_列表权限错误_→_回落整群筛法/HTTP_403 (0.00s)
            reads_test.go:312: 权限错误该回落而不是上抛：99999: forbidden
        --- FAIL: TestThreadHistoryUsesThreadContainer/thread_列表权限错误_→_回落整群筛法/业务码_230002 (0.00s)
            reads_test.go:312: 权限错误该回落而不是上抛：230002: bot not in chat
FAIL
```

**⑥a 去缓存**：

```
--- FAIL: TestSenderNameFromContactAPIIsCached (0.01s)
    reads_test.go:621: 通讯录请求 2 次，要 1（第二条该命中缓存）
    reads_test.go:641: 机器人消息不该查通讯录，请求 2 次
FAIL
```

**⑥b 去 300ms 超时**：

```
--- FAIL: TestSenderNameDegradesWithin300ms (5.01s)
    --- FAIL: TestSenderNameDegradesWithin300ms/慢 (5.01s)
        reads_test.go:668: 耗时 5.005394945s，要 < 1s（查询该被 300ms 截断）
FAIL
```

**⑦ 删 PASSIVE_LISTEN / API_BASE 的读取**：

```
--- FAIL: TestEnvFlagsAreReadInsideThePackage (0.00s)
    --- FAIL: TestEnvFlagsAreReadInsideThePackage/AITE_FEISHU_PASSIVE_LISTEN (0.00s)
        platform_test.go:432: AITE_FEISHU_PASSIVE_LISTEN="1" 该打开 supports_passive_listen
    --- FAIL: TestEnvFlagsAreReadInsideThePackage/AITE_FEISHU_API_BASE (0.00s)
        platform_test.go:453: domain = "https://open.feishu.cn" / api "https://open.feishu.cn"，要 https://open.larksuite.test（REST 与长连接同一个域名）
FAIL
```

（按钮那一格的读取由 ④a 覆盖。）

**⑧a 去每群桶**：

```
--- FAIL: TestPerChatPacing (0.03s)
    --- FAIL: TestPerChatPacing/同一群第_6_条等_200ms，别的群不受影响 (0.01s)
        outbound_test.go:601: 同一群第 6 条 slept = []，要 [200ms]
    --- FAIL: TestPerChatPacing/UpdateCard_过它所在群的桶 (0.01s)
        outbound_test.go:641: 同一张卡第 6 次出站 slept = []，要 [200ms]
FAIL
```

**⑧b 每次重试重新生成 uuid**（body 里换成每次 Marshal 都新生成的值）：

```
--- FAIL: TestSendCarriesAStableUUIDAcrossRetries (0.01s)
    outbound_test.go:668: 重试换了 uuid："2YDLFP5QNQ3J3Y2FH3FT5FBM2Q" → "TOHWSKXHVI56WSTHHTCMNFBDDQ"（平台就去不了重）
FAIL
```

每次变异后都还原并跑全包 `-race` 绿，提交前 `grep -rn MUTATION edge/` 为空。

### 改动的钉

- `TestUnsubscribedEventIsDroppedWithoutCallingHandler`（现 `events_test.go`）：改前用 `im.chat.member.user.added_v1` 当「没订阅」的例子；
  本轨订阅了它，改后换成仍未订阅的 `im.message.message_read_v1`（与 `TestUnsubscribedEventTypeIsIgnored` 同一个），断言一字未动。
- `TestHistoryCanBeNarrowedToOneThread`（`reads_test.go`）：不改时也绿（root 查询撞假服务 404 → 回落 ①）。**改了**：注册 root 路由、返回不带 `thread_id` 的消息，
  让它**有意地**钉住回落 ①（非话题消息），期望 `[om_toplevel_0001 om_in]` 不动；注释同步。
- `TestNewWithMissingEnvDoesNotExplode`（现 `platform_test.go`）：开头加 `clearFeishuEnv(t)`（三个 `t.Setenv(…, "")`），断言不动 —— 否则导出过 `AITE_FEISHU_API_BASE` 的 shell 会让它红。
- `normalize.go` 文件头「只认 P0 订阅的两类事件」注释同步改写。
- 其余：`TestNoDeadButtonsAndAHintInstead` 与所有 cards 测试一条未改；`TestGoSDKDropsCardFramesOnTheWire` 断言一字未改、仍绿；
  `TestCardActionTriggerReachesTheHandlerOnAnEventFrame` 保留；`TestHistoryAsksForTheMostRecentWindow` 一字未改。
- 测试辅助的非断言改动：`helpers_test.go` 的 `route.responses` 改成带 `*http.Request` 的函数（新增 `onReq`，`on` 包一层，行为不变）；
  fixture 读取加第二个根（`fixtureRoots / fixtureDir / loadFixtureFile`），三处黄金循环的 expected 路径改走 `fixtureDir(name)`。

## 4. 验收（§7）

```
① (cd edge && go vet ./... && gofmt -l . | wc -l)                → 0（exit 0）
② (cd edge && go test -race ./internal/feishu/... -count=1)      → ok  	aite/edge/internal/feishu	7.599s
③ … -run '…5 个…' | grep -E '^--- '
--- PASS: TestButtonsOnlyWithFlag (0.03s)
--- PASS: TestGoSDKDropsCardFramesOnTheWire (2.21s)
--- PASS: TestCardActionTriggerArrivesAsEventFrame (2.44s)
--- PASS: TestPerChatPacing (0.03s)
--- PASS: TestThreadHistoryUsesThreadContainer (0.02s)
④ 黄金四件套：全 PASS，每件的子测试 = 老 7 个（card_action_stop / message_at_bot_toplevel / message_from_bot / message_in_thread_no_at /
   message_in_thread_with_at / message_post_with_image / message_with_file）+ bot_added / member_added / member_deleted / recalled
⑤ 9、7、0：
ok  	aite/edge/cmd/aite-edge	5.718s
?   	aite/edge/gen/aitepb	[no test files]
ok  	aite/edge/internal/aiteerr	1.033s
ok  	aite/edge/internal/config	1.028s
ok  	aite/edge/internal/feishu	7.621s
ok  	aite/edge/internal/ingress	1.315s
?   	aite/edge/internal/pin	[no test files]
ok  	aite/edge/internal/sandbox	1.033s
ok  	aite/edge/internal/server	1.037s
⑥ 127 = 115 + 12（名单见 §3；与开场名单 diff 只有这 12 行 `>`，没有 `<`）
⑦ 见下（全部通过）
⑧ git diff --name-only main...HEAD：每行都以 edge/internal/feishu/ 开头，或是 review/p1/ledger/CC8.md
   （本容器没有 origin/main，拿本地 main = 30b00e5 当合并基；用 8458435 当基会多出 D0 那个提交的 CLAUDE.md 与 review/paste-*.md，不是本轨的）
⑨ git diff --name-only main...HEAD -- edge/testdata edge/cmd → 空
⑩ git status --short → 空
```

全包 `--- PASS` 总数：172 → 233（含子测试）。导出面变化（`go doc -short` diff）：只多了 `const EnvCardButtons = "AITE_FEISHU_CARD_BUTTONS" ...`
（同组 `EnvPassiveListen / EnvAPIBase`）；`PathContactUser` 进了原有的 `PathTenantToken ...` 常量组。`New / Normalize / NormalizeMessage / NormalizeCardAction / BuildChecklistCard` 签名不变。

### ⑦ check.sh 完整输出（收尾，原样）

```

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.26s
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
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.40s
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

## 5. `cargo passed` 增量

**Δ = 0**（946 → 946），本轨不碰 Rust；B8 `passed 10/10`。Go 侧新增顶层测试 12 条（= ⑥ 的 127 − 115）：

- `events_test.go`：`TestHandlerTableDrivesDispatcherAndNormalize`、`TestDuplicateEventRegistrationPanics`、`TestReactionIsParsedAndDropped`、`TestCardActionTriggerArrivesAsEventFrame`（4）
- `normalize_test.go`：`TestNewEventKindsNormalize`（1）
- `cards_test.go`：`TestButtonsOnlyWithFlag`（1）
- `reads_test.go`：`TestThreadHistoryUsesThreadContainer`、`TestSenderNameFromContactAPIIsCached`、`TestSenderNameDegradesWithin300ms`（3）
- `platform_test.go`：`TestEnvFlagsAreReadInsideThePackage`（1）
- `outbound_test.go`：`TestPerChatPacing`、`TestSendCarriesAStableUUIDAcrossRetries`（2）

## 6. 被守卫拦过的命令

1. 开场自检第 2 步（期望被拦，原文见 §1）。
2. 两次多行 heredoc 的 `python3 - <<'EOF' …` 补丁命令（改 `helpers_test.go` / `normalize_test.go` 的 fixture 根；改 `platform.go` / `outbound.go` 的按钮开关），拦截原文（两次相同）：

```
PreToolUse:Bash hook error: [d=$(git rev-parse --show-toplevel 2>/dev/null); [ -f "$d/.claude/hooks/guard_bash.py" ] || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"]: blocked: 该操作触碰受保护面 <命令无法解析: No closing quotation>（解析失败）。停止当前工作并向人类报告。
```

   这是 CLAUDE.md「已知误拦与绕法」第一条（ASCII 引号跨行 → 判「无法解析」），命令本身没碰任何受保护路径；按那里写明的做法改成「脚本写成文件再跑」/ 用 Edit 工具，没有换写法去碰受保护面。

## 7. 记账转出去的

| 事项 | 为什么不在本轨 | 建议归哪轨 |
|---|---|---|
| `docs/acceptance-M.md` §7 登记日志关键字：`feishu.card_frame`、`feishu.reaction_dropped`、`feishu.thread_history_fallback`，以及本轨另加的 `feishu.event_malformed`、`feishu.sender_name_denied`、`feishu.sender_name_failed`、`feishu.env_flags`、`feishu.sdk` | 文档不在可写面 | 总管或 GG1 |
| core 让卡片带 `Evidence` 按钮（今天 core 只发 `Stop`） | core 不在可写面 | DD5（CC2 / CC3 之后） |
| 表情 → `EventKind::Reaction`（`reactionEvent` 结构体已就位，`dropReaction` 换成真归一化即可） | 契约还没有这个枚举 | DD9（依赖 T0） |
| 三个环境变量换成 `FeishuConfig.api_base / card_buttons`（passive_listen 同理） | Go 配置镜像不在本轨 | DD8 / DD9 / DD10 |
| 撤回的「删根」语义（撤回的是话题 root 时怎么办；撤回事件没有操作者，见 §2③） | core 语义 | DD3 |
| `card_buttons` 默认值（按 H8 结果翻） | 要真机点一次 | DD10（按 H8） |
| 权限错误码清单真机核对（`99991400` 疑为限流）；通讯录没权限时的真实 HTTP 状态与业务码（不在清单里就不熔断，见 §2⑥「已知缺口」） | 云端连不上飞书 | H7 |
| `api_test.go` / `platform_test.go` / `helpers_test.go` 在 W2 没有主人（英文原卡 DD9 只列了 `platform.go`、`api.go`；DD9 / DD10 都没列 `helpers_test.go`，而 DD9 给 `testdata/read/` 加夹具根要改它） | 本轨只定布局、不定 W2 归属 | 总管补进 DD9 可写面 |
| `sendMessage` 的 `dedupeKey` 参数接 `OutboundText.dedupe_key`（飞书 uuid 要求 ≤50 字符，接的时候要截断或哈希） | 契约字段还没有 | DD10（依赖 T0） |
| 仓库推送 / draft PR（本会话没权限，见文首） | 会话 sources 设置 | 总管 |

## 8. 没做的与原因

- **推送与 draft PR**：见文首，会话 git 代理不认 `TomwaltW/aite`、容器里也没有 `gh`。
- 姓名缓存不设 TTL（理由见 ⑥）；每群桶表的「淘汰最久没用」是 O(n) 扫描（n ≤ 1024，只在桶表满时发生）。
- ⑤ 的「含 root」按「root 进候选集、再取最近 limit 条」实现：话题回复超过 limit 条时 root 会被窗口截掉 —— 与原整群筛法同口径；
  若 CC3 / CC2 要「root 永远在」，改 `readThreadHistory` 末尾一行即可，写在这里备查。
- 没跑真容器那组（本轨不碰 sandbox）。
- **本轨新增的时序敏感测试**（CPU 紧时可能假红，先单跑再下结论）：`TestSenderNameDegradesWithin300ms/慢`（断言墙钟 < 1s）、
  `TestCardActionTriggerArrivesAsEventFrame`（真 websocket + `settleSink` 真 sleep，与 card_frames 那组同一性质）。

## 9. 契约缺口（给 T0 / T0.1）

T0 已计划的（`FeishuConfig += api_base, card_buttons, service_user_token_env`、`EventKind += Reaction, External`、`NormalizedEvent += quote, reaction, …`、
`OutboundText += mentions, dedupe_key`、`HistoryMessage += …`、`UserInfo / ChatInfo`、能力位 `supports_reactions_in / recall_event`）不重复。计划外的一条：

- **`MEMBER_CHANGED` 分不出加入 / 退出，也拿不到是谁进谁出。** 飞书的 `user.added_v1` / `user.deleted_v1` 两类事件在契约里落成同一个 `EVENT_KIND_MEMBER_CHANGED`，
  而加入 / 退出的方向、被加 / 被移的成员列表（`event.users[].user_id.open_id / name`）只在 `raw` 里 —— events.proto:92 规定任何逻辑不得依赖 raw，
  所以 core 今天连「有人进群了」和「有人退群了」都分不开，更没法做「新成员入群自动打招呼」或「退群后撤销其会话权限」。
  需要的形状：`NormalizedEvent` 加一个 `optional MemberChange member_change`，`MemberChange { enum Direction { ADDED, REMOVED } direction; repeated Member members; }`，
  `Member { string id; optional string name; }`（`BOT_ADDED` 也可复用，members 里放机器人自己）。开放通道绕不过去的原因：`raw` 被契约明令禁止用于逻辑，
  而 `text` / `card_action` 都没有语义合适的槽位，塞进去就是在发明契约。

## 10. H8 备用路径（原样抄给总管本机照做）

```bash
cd ~/Documents/Projects/Aite && git fetch origin <本 PR 的 claude/… 分支名> && git worktree add .worktrees/cc8-h8 FETCH_HEAD
cd ~/Documents/Projects/Aite/.worktrees/cc8-h8/edge && go build -o ~/Documents/Projects/Aite/edge/bin/aite-edge-cc8 ./cmd/aite-edge
cd ~/Documents/Projects/Aite && AITE_FEISHU_CARD_BUTTONS=1 edge/bin/aite-edge-cc8 --config config/aite.yaml 2>&1 | tee /tmp/h8-edge.log
cd ~/Documents/Projects/Aite && core/target/debug/aite run --config config/aite.yaml
```

（本 PR 的分支名：`claude/cc8-feishu-adapter` —— 前提是它已按文首推上去。）

- 第 3、4 行**分别在两个终端里跑**（都在仓库根，谁先起都行，见 acceptance-M §0.2.2）—— 它们都是常驻前台进程，贴进同一个终端的话 core 永远起不来。
- 为什么在 worktree 里编、回主仓库根跑：`config/aite.yaml` 不入库（`.gitignore:20`），新 worktree 里没有；socket 等相对路径按仓库根解析（acceptance-M §0.2）。
  `edge/bin/` 已被忽略（`.gitignore:27`）。本轨不碰 `server.go`，两边契约版本都是 `p0.2`，main 上编好的 core 直接能连。飞书凭证照平时的环境变量给。
- 操作：测试群里 @Aite 发一个要跑一阵的任务，卡片「进行中」时点「停止」，然后 `grep feishu.card_frame /tmp/h8-edge.log`。
  `frame_type=event` 且卡片变「已取消」→ **PASS**；`frame_type=card` → **FAIL**（平台按旧版回调发帧，SDK 丢弃）；一行都没有 → 先查 H5 的回调订阅（长连接 + `card.action.trigger`）再点一次。
- 收尾：`cd ~/Documents/Projects/Aite/.worktrees/cc8-h8 && git clean -xdff`，再 `cd ~/Documents/Projects/Aite && git worktree remove .worktrees/cc8-h8 && rm edge/bin/aite-edge-cc8`
  （先 clean：Finder 的 `.DS_Store` 会让 remove 报 Directory not empty）。
- 补一句：H8 时 edge 启动会打一行 INFO `feishu.env_flags card_buttons=true …`，先看它确认开关真的开了；
  H5 订阅通讯录权限前，`feishu.sender_name_denied` 那条 WARN 每 10 分钟最多一条，属预期。

## 提交清单（本地分支）

```
f610c55 CC8 ⑧: 每群 5 QPS 令牌桶 + 发送请求体带 uuid（重试复用）
6d5fc3f CC8 ⑦: 包内读 AITE_FEISHU_PASSIVE_LISTEN / API_BASE / CARD_BUTTONS（只在 New 里）
564e9d1 CC8 ⑥: 发言人姓名（通讯录查询 + 有界 LRU + 300ms 超时 + 权限熔断）
cd52259 CC8 ⑤: 话题历史走 thread 容器（取不到 thread_id 或权限错误时回落整群筛法）
e35ce22 CC8 ④: 卡片按钮开关（默认关）+ 帧类型日志
8983f2a CC8 ③: 订阅撤回 / 入群 / 成员 / 表情事件
7865330 CC8 ②: 事件分发表（零行为变化）
9ae4147 CC8 ①: 按关注点拆 edge/internal/feishu（零行为变化）
```

外加本回执一个提交。
