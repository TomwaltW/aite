# 派单 CC8：飞书适配器整理 —— 按关注点拆文件、新事件、卡片按钮开关与帧类型日志、话题历史、发言人姓名、每群限速（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC8.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC8）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

**对应 Claude Tag 的哪几条**：CT04（话题里谁回复都能引导 → 要知道是谁说的：发言人姓名）、CT06（删除 / 撤回进 transcript 的前提：先收得到撤回事件）、
CT07（上下文 = 话题窗口，而不是整群窗口）、CT13（清单卡上的「停止」按钮）。总计划 CT04/CT06/CT07/CT13 行（plan:138、140、141、147），
§1.2 第 8 行（plan:107），§0「发现 1」（plan:63-66），§2C 卡片按钮取舍（plan:202），D15（plan:259），§9 解冻项（plan:722）。
（行号按 2026-09-25 的计划核过；计划若再改，按节标题找。）

**Aite 今天的样子**（`edge/internal/feishu/`，6 个源文件 + 9 个测试文件，全部在你的可写面里）：

- **只订阅 2 类事件**：`connection.go:142-154` 的 `buildDispatcher` 只注册 `im.message.receive_v1`（:144-146）与 `card.action.trigger`（:147-152）；
  `normalize.go:621-631` 的 `Normalize` 也只认这两类（文件头 :5-6 写着「只认 P0 订阅的两类事件」）。契约槽位早就有：
  `proto/aite/v1/events.proto:21-29`（`MESSAGE_DELETED=3`、`BOT_ADDED=5`、`MEMBER_CHANGED=6`），core 的 R4 也已经接着
  （`core/crates/control/src/plane.rs:147-153` 的 `R4_KINDS`、:524-527、:607-622 只计数 / 记 system_note）—— 只是 edge 从来不发。
- **没注册的事件会被平台重推**：控制台订阅了、代码里没注册的事件类型，SDK 的 `dispatcher.Do` 返回 `NotFoundEventHandlerErr`
  （lark-oapi-go v3.12.0 `event/dispatcher/dispatcher.go:272-275`），长连接回 500 → 平台重推。H5 今天就会在控制台订阅撤回 / 表情 / 入群 / 成员事件，
  所以本轨必须把它们**全部注册**，包括暂时只解析、不上送的表情事件。
- **卡片按钮不渲染**：`cards.go:120-153` 那段注释是 BB5 的结论（SDK `ws/client_message.go:79` 把 `type=card` 帧在任何 handler 之前丢掉）；
  `:154-159` 只加一行文字提示（`actionHint`，:202-212）；`buildActions`（:164-185）留着、只在单测里被调。而飞书官方 2026-06-11《使用长连接接收回调》
  写明新版 `card.action.trigger` 走 `type=event` 帧，`OnP2CardActionTrigger` 早已注册（connection.go:147-152）。两条测试各钉一边：
  `card_frames_test.go:221-237`（`TestGoSDKDropsCardFramesOnTheWire`，旧版 `type=card` 帧被丢）与 :244-280（`type=event` 帧能到 handler）。
  **真平台发的是哪种帧只能人点一次看（H8）** —— 本轨给 H8 两样东西：按钮开关、INFO 级的帧类型日志。默认值由 DD10 按 H8 结果定。
- **话题历史是整群拉取再客户端筛**：`platform.go:480-558` 的 `ReadHistory` 永远 `container_id_type=chat`，带话题时预算 ×4（:498-502）后按
  root/parent/thread/message_id 筛（:539-547、`inThread` :657-664）；注释 :485-488 说明锚点里存的是 root 消息 id、不是 `omt_`。
- **事件上的 `sender_name` 恒为 nil**（`normalize.go:497-498`）；core 拿它当发起人姓名，缺了就退回 open_id（`plane.rs:1014-1022`）。
- **限速只有全局令牌桶**（`api.go:219-225` + `ratelimit.go`，速率取能力位 60/分，`platform.go:214`）；没有每群 5 QPS；发送体没有 `uuid`
  （`platform.go:348-377`），5xx 重试（`api.go:262-328`）可能重复发出同一条。
- `feishu.New` 由 `edge/cmd/aite-edge/main.go:86-91` 调用，不传 `Domain`；`SetPassiveListen`（platform.go:229-234）没有调用方。

**谁接你的东西**：CC3（同波）第 8 项「话题内用话题历史窗口」会把 `worker/src/agent.rs:294-300` 里的 `None` 换成 thread id —— 你的 thread 容器路径就是它的实际走法；
CC2（同波）的 `!restart` 回灌也调 `read_history(thread)`。DD9（W2）拥有 `reads.go / events.go / normalize.go / capabilities.go / platform.go / api.go`
（外加新建 users / chats / search / oauth），把你解析的表情结构体接成 T0 的 `EventKind::Reaction`；DD10（W2）拥有 `outbound.go / cards.go`
（外加 docs / dm），按 H8 翻 `card_buttons` 默认值、把 `dedupe_key` 接到你的 `uuid` 上；EE11（W3）的 `external.go` 往你的**事件分发表**里注册、
**不改 events.go**；FF3 的 `cardkit.go`、HH1 的 `oauth_user.go` 也落在这个包里；DD8（W2）做 Go 配置镜像，用 T0 的 `FeishuConfig.api_base / card_buttons`
（plan:457，§5.2「平台」）接替你今天读的环境变量。所以本轨的文件布局就是后面四个波次的分工边界 —— **文件名照本派单来，别自创**。

## 2. 必读（按顺序）

1. 仓库根 `CLAUDE.md`（云端唯一能读到的约定；与本派单冲突时以本派单为准）。
2. 总计划：§4.4 开场自检（plan:376-394）、§5.1 P0-CLOSE（plan:431-445）、§6 每波规则（plan:502-512）、§6.1 CC8 行（plan:528）、
   §7 R0 表里 `edge/internal/pin`、`go.mod`、`main.go` 两行（plan:621-622）、§8 H5 / H8（plan:652-657、676-678）、§9 解冻清单（plan:717-725）。
3. `edge/internal/feishu/` 全部源文件：`platform.go`（709 行）、`normalize.go`（634）、`api.go`（488）、`cards.go`（268）、`connection.go`（245）、`ratelimit.go`（120）。
4. 同目录全部测试：`helpers_test.go`（假时钟 / 日志捕获 / 假飞书 / 装配，:29-37 测试 id、:36 fixture 目录、:302-348 `mustPlatform`、:366-394 fixture 读取）、
   `normalize_test.go`（:5-10 两层判据、:30 `-update`、:83-157 黄金循环）、`card_frames_test.go`（真 websocket 假服务端 :70-187）、`reconnect_test.go`（:337-346 `dispatchPlatform`）、
   `read_test.go`、`outbound_test.go`、`cards_test.go`、`capabilities_test.go`、`errors_test.go`。
5. 契约与 core 侧（只读，**用 Read 工具**读，守卫按路径前缀拦 Bash）：`proto/aite/v1/events.proto:21-29、74-93`；`core/crates/proto/src/convert.rs:240-274`
   （`kind / chat_type / sender_kind` 不许 UNSPECIFIED、`anchor` 必须有、`occurred_at` 必须有；`message_id` 可以空）；`core/crates/edge-client/src/ingress.rs:5`；
   `edge/internal/ingress/client.go:9、186-200`（INVALID_ARGUMENT → 计 `ingress.invalid`、不重推 —— 事件形状错了是**静默丢**）；`core/crates/control/src/plane.rs:504-557`（R1–R8）。
6. SDK 源码（只读）：`go env GOMODCACHE` 下的 `github.com/larksuite/oapi-sdk-go/v3@v3.12.0/`：`ws/client_message.go:61-95`（:77 的 Debug 行、:79 的 type 闸门）、
   `ws/client.go:50-72`（`WithEventHandler / WithLogLevel / WithLogger`）、`event/dispatcher/dispatcher.go:244-296`。
   **字段形状的离线权威**（云端环境连不上 open.feishu.cn 的文档；照 `api.go:9`「逐条对过 SDK 里生成的请求类」的先例，以 SDK 生成的结构体 json tag 为准）：
   `service/im/v1/model.go` 的 `P2ChatMemberBotAddedV1Data`（:16498）、`P2ChatMemberUserAddedV1Data`（:16546）、`P2ChatMemberUserDeletedV1Data`（:16572）、
   `P2MessageRecalledV1Data`（:16640）、`P2MessageReactionCreatedV1Data`（:16676）、`P2MessageReactionDeletedV1Data`（:16700）、`Message.ThreadId`（:5682、:5689）、
   `GetMessageRespData`（:14134-14136，`items` 是**数组**）；`service/contact/v3/model.go` 的 `User.Name`（:4434、:4441）、`GetUserRespData`（:12819-12821）；
   路径见 `service/im/v1/resource.go:1274-1277`（`message.Get`）、`service/contact/v3/resource.go:1884-1887`（`user.Get`）。
   权限错误的离线出处：`channel/types/errors.go:79-80`（业务码 → `ErrCodePermissionDenied`）、`:96-97`（HTTP 401 / 403）。只读、不 import（我们照旧用 net/http 直打）。
7. `docs/dev-spec-2026-09-11-rustgo.md`（Read 工具）:106、:236（B1：`edge/testdata/feishu/` 至少 7 对 fixture 逐字节比）、:267；
   `docs/acceptance-M.md` §0.2（:262-300，本机起飞必须在仓库根）、§0.4（:559 起，卡片提示）、§7（:1058 起，日志关键字的命名风格）。

## 3. 工作区

- **分支**：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC8: 飞书适配器整理 + 新事件 + 卡片按钮开关」。
  PR 描述先用 **Write 工具**写成 `/tmp/cc8-pr.md`，再 `gh pr create --draft --title "CC8: 飞书适配器整理 + 新事件 + 卡片按钮开关" --body-file /tmp/cc8-pr.md`；
  收尾时 `gh pr edit --body-file review/p1/ledger/CC8.md`（或它的摘要文件）。**永远别**把多行正文塞进 `--body "…"`（跨行引号会被守卫判「无法解析」）。
- **代码基线**：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**（逐条核过）：
  - `edge/internal/feishu/**` —— 现有 15 个文件；目标布局（§5 第 1 项）：`api.go platform.go reads.go outbound.go cards.go events.go normalize.go capabilities.go`
    + 同前缀的 `*_test.go` + 共享的 `helpers_test.go`；新建 `testdata/events/`（新事件夹具）、`testdata/read/`（话题历史 / 通讯录的响应夹具）、
    `testdata/write/`（出站夹具）。三个都建；本轨某个目录若没有夹具（测试用内联 JSON），放一个 `README.md` 写明归属（read → DD9、write → DD10）—— git 不收空目录。
  - `review/p1/ledger/CC8.md`（新建）。
- **只读面**：其余一切。特别点名：
  - P0-CLOSE 的 14 个路径（与开场自检第 1 步情形 B 同一份）：`proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
    `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
    `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
    `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`、`.contracts.lock`、
    `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`（外加整个 `.claude/**`、`core/crates/evidence/**`）。
  - T0 补丁文件：`config/aite.example.yaml`、`edge/internal/server/server.go`、`core/crates/proto/**`（外加锁定面 `proto/aite/v1/*.proto`、`core/crates/contracts/**`）。
  - **`edge/testdata/feishu/**`**（老的 7 对 fixture）：在你的可写面**外面**，且被 dev-spec B1 钉着 —— 拆文件时**不许搬、不许改**，`-update` 之后也必须零 diff。
  - 离你最近的别轨面：CC9 `edge/internal/dingtalk/**`、CC10 `edge/internal/wecom/**`、CC11 `edge/internal/egress/**`、CC12 `edge/internal/sandbox/docker_test.go`；
    没人拥有的 `edge/internal/{server,config,ingress,aiteerr,pin}/**`、`edge/cmd/aite-edge/**`、`edge/gen/**`、`edge/go.mod`、`edge/go.sum`；
    `review/p1/ledger/CC8-clicktest.md`（H8 结果，总管本人写，你别建）。
- **本轨解冻的冻结项**：「卡片用文字提示代替按钮」（plan:722，§9 解冻）→ 本轨只加开关、**默认关**；回归 `TestButtonsOnlyWithFlag` + 变异验证。
  另有测试钉受本轨新行为影响（不是意外发现，照做并在回执「改动的钉」里列）：`TestUnsubscribedEventIsDroppedWithoutCallingHandler` 必须改（§5 第 3 项）；
  `TestHistoryCanBeNarrowedToOneThread` 预期不改也绿、建议加 root 路由让回落显式（§5 第 5 项）。

## 4. 开场自检（全部对上才开工；对不上就写回执停下）

本节与 §7 的命令都从仓库根起跑；进子目录一律写成子 shell `(cd edge && …)` / `(cd core && …)`（Bash 的工作目录在两次调用之间是保留的，
裸 `cd edge && …` 之后，下一条 `cd edge` 就进不去了，`scripts/check.sh`、`git diff` 也会落在 `edge/` 里跑）。

1. **代码基线**：先 `git cat-file -e 98e4460 || git fetch -q --unshallow origin`（浅 clone 时补历史；§7 ⑧⑨ 的三点 diff 也要靠它拿到合并基），再
   `git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'`
   - 输出为空 → 情形 A：基线行 = `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok；
   - 恰好列出下面 14 个 P0-CLOSE 路径、一个不多 → 情形 B：基线行 = `cargo passed=901 failed=0`、`contracts passed=27 failed=0`、`OK 25 files`、`passed 10/10`（以实测为准，差异逐条解释）。
     14 个路径：AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
     BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
     `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
     `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
     BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；
   - 别的 → 停下，写回执。（D0 的改动全是删除或落在排除的路径里，所以情形 A 输出为空；`--no-renames` 让「挪文件」也现形。）回执里写明是 A 还是 B。
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（期望形如「blocked: 该操作触碰受保护面 …（读取位置）。停止当前工作并向人类报告。」；拦截原文逐字贴进回执）。
   **这一步被拦就是通过**：拦截原文里「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步。
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`(cd core && rustc --version)` → 1.98.1；`(cd edge && go version)` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可；本轨不跑 codegen，对不上只记进回执、不停）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**（冷编译 10–15 分钟，耐心等）。
   Go 那一格只显示 8 行是正常的：6 行 `ok` + 2 行 `? … [no test files]`（`gen/aitepb`、`internal/pin`），排第一的 `cmd/aite-edge` 被截掉；
   单跑 `(cd edge && go test -race ./cmd/... -count=1)` 确认 → `ok`，8 + 1 = 9 包。
   若 CC1 已合并、check.sh 多打一行 `go packages ok=N fail=M`，以那行为准。
5. **本轨附加（给第 1 项的零行为对照留底，文件放 `/tmp`，不入库）**：
   - `(cd edge && go test -race ./... -count=1) > /tmp/cc8-go-before.txt 2>&1; grep -cE '^(ok|\?)' /tmp/cc8-go-before.txt; grep -c '^ok' /tmp/cc8-go-before.txt; grep -c FAIL /tmp/cc8-go-before.txt`
     → 期望 `9`、`7`、`0`。「Go 9 包」的口径 = 7 行 `ok`（`cmd/aite-edge`、`aiteerr`、`config`、`feishu`、`ingress`、`sandbox`、`server`）
     + 2 行 `? … [no test files]`（`gen/aitepb`、`internal/pin` 没有 `*_test.go`）。按源码数的、本机没实跑 —— 对不上先贴原输出再判断。
     原卡写的「`grep -c '^ok'` → 9」是错的（`?` 那两行不以 `ok` 开头，`^ok` 只数得到 7），回执里注明「按 7 ok + 2 ? 验」。
   - `(cd edge && go test -list '.*' ./internal/feishu/) | grep '^Test' | sort > /tmp/cc8-tests-before.txt; wc -l < /tmp/cc8-tests-before.txt` → 期望 `115`
     （2026-09-25 按源码 `grep -c '^func Test'` 数的，未实跑；对不上先贴实际值再判断）。
   - `(cd edge && go test -race -v -count=1 ./internal/feishu/ 2>&1) | grep -c -- '--- PASS'` → 记为 P0（含子测试，本机没数过，只记不比）。
   - `(cd edge/internal/feishu && grep -c '^func Test' *_test.go)` → capabilities 14 / card_frames 5 / cards 18 / errors 14 / helpers 0 / normalize 22 / outbound 14 / read 14 / reconnect 14。
   - `(cd edge && go doc -short ./internal/feishu) | sort > /tmp/cc8-api-before.txt`（导出面快照）。

## 5. 工作项

**总纪律**：`New(cfg, opts, sink)` 签名不许改（platform.go:16、main.go:86-91）；`main.go` 一个字不动 —— 环境变量在包里读。
`Normalize / NormalizeMessage / NormalizeCardAction / BuildChecklistCard` 等导出签名不变。**所有环境变量只在 `New()` 里读**；`newPlatform`（测试都走它）
不读环境、不装配任何真网络查询 —— `dispatchPlatform`（reconnect_test.go:337-346）造的 Platform 指向真的 `open.feishu.cn`，任何默认开启的网络查询都会让测试在云端打公网。
**edge 的改动碰不到 B8**：`evals/p0` 在 core 里跑 `--platform fake`；本轨 `cargo passed` 的 Δ 应当是 **0**。
下文所有 `文件:行` 都指 `98e4460` 上的原位置；第 1 项拆完之后按符号名找。

1. **零行为变化拆文件（单独第 1 个提交：「CC8 ①: 按关注点拆 edge/internal/feishu（零行为变化）」）**。只搬、不改逻辑、不改名：

   | 目标文件 | 内容来源 |
   |---|---|
   | `platform.go` | 包文档（:1-16）、`onEventBudget`（platform.go:39-40）、`EventSink / Options / Platform / platformOptions / newPlatform / New`、`Start` 重连循环、`safeClose / Connected / ReconnectCount`；`ReconnectBaseSec/MaxSec`（原 connection.go:31-35）与 `backoffDelay`（原 connection.go:53-69） |
   | `capabilities.go` | `FeishuP0 / Capabilities / SetPassiveListen`（platform.go:202-234） |
   | `events.go` | 原 `connection.go` 其余部分：`RawEventHandler / Connection` 接口 `/ ConnectionFactory`（:37-51）+ :71-245（`envelopeHeaderKeys`、`larkConnection`、`buildDispatcher`、`envelopeOf`、`deliver`）+ `dispatchRaw`（platform.go:307-339）+ `Normalize` 总入口与两个事件名常量（normalize.go:28-31、621-631） |
   | `normalize.go` | 其余归一化与小工具 |
   | `reads.go` | `ReadHistory / ReadDocument / DownloadFile / parseDocRef / inThread / toHistoryMessage / senderKindToken`、`historyPageSize / docURLRe`（platform.go:69-73、476-623，以及 :625-709 里除 `fileTypeOf` 以外的小工具） |
   | `outbound.go` | `sendMessage / SendText / SendCard / UpdateCard / SendFile / AddReaction / fileTypeOf`、`KnownEmojiTypes / ReactionEmoji / fileTypeByExt` |
   | `api.go` | 原样 + 原 `ratelimit.go` 全部（`clockFunc / sleeperFunc / realSleep / TokenBucket`，桶是 apiClient 用的） |
   | `cards.go` | 原样 |

   删掉 `connection.go`、`ratelimit.go`。测试同前缀：`errors_test.go`→`api_test.go`（+ `capabilities_test.go` 里 5 条令牌桶测试）；`read_test.go`→`reads_test.go`；
   `reconnect_test.go` 的 9 条退避 / 重连测试 + `capabilities_test.go` 的 3 条装配测试（:126-204）→`platform_test.go`；`reconnect_test.go` 的 5 条投递测试（:350-507）
   + `card_frames_test.go` 全部 →`events_test.go`；`capabilities_test.go` 剩 6 条；`cards / normalize / outbound_test.go` 不动；`helpers_test.go` 保留（共享夹具，无对应源文件）。
   `reconnect_test.go:1-175` 的测试辅助按用途分：`recordingSink`（:142-168）两边都用 → 搬进 `helpers_test.go`；`dispatchPlatform`（:337-346）→ `helpers_test.go`
   （events 的投递测试要它，第 3、6 项的新测试多半也会用）；`expectedBackoff / recordingSleep / fakeConnection / connectionScript / reconnectHarness`（:21-140）只服务退避 / 重连 → `platform_test.go`。
   期望分布：api 19 / platform 12 / reads 14 / events 10 / capabilities 6 / cards 18 / normalize 22 / outbound 14 = 115。能 `git mv` 的用 `git mv`。
   **这个提交的判据**（输出全部贴回执）：`git show --stat HEAD` 只有 `edge/internal/feishu/*.go`；`go test -list` 排序后与 `/tmp/cc8-tests-before.txt` `diff` 为空；
   `--- PASS` 总数 = P0；`go doc -short` 排序后与 `/tmp/cc8-api-before.txt` `diff` 为空；新旧逐文件条数对照表；`go vet ./...`、`gofmt -l . | wc -l` → `0`。
2. **事件分发表（第 2 个提交，仍是零行为变化）**。`events.go` 里放一张 `事件类型 → handler` 表 + 一个注册函数，各文件在自己的 `init()` 里往里登记
   （重复登记 panic）；`buildDispatcher` 遍历这张表注册（`card.action.trigger` 仍走 `OnP2CardActionTrigger`，其余走 `OnCustomizedEvent`）；`Normalize` 查表。
   这一步之后表里恰好两项、行为与今天一致。新测试（建议名 `TestHandlerTableDrivesDispatcherAndNormalize`）钉：表里每一项都能经 `buildDispatcher().Do` 到 `onRaw`
   且被 `Normalize` 认；测试里临时登记一个假事件类型（撤销写在 `t.Cleanup` 里，别污染其它测试），不改 events.go 就能被投递和归一化 —— 这正是 EE11 要的接口。变异：让 `buildDispatcher` 不遍历表 → 红。
   **并发**：这张表是包级可变状态，测试运行期登记 / 撤销时，别的测试的 SDK websocket goroutine 正在经 `buildDispatcher` / `Normalize` 读它 ——
   表必须由 `sync.RWMutex` 守着（或者读侧先取快照再用），否则 `-race` 会抓现行。测试专用的登记 / 撤销函数放 `helpers_test.go` 或 events_test.go，不导出。
3. **订阅新事件**（在分发表里登记，各自一个归一化函数）：
   - `im.message.recalled_v1` → `EVENT_KIND_MESSAGE_DELETED`，`anchor.message_id` = 被撤回的消息 id；
   - `im.chat.member.bot.added_v1` → `EVENT_KIND_BOT_ADDED`；`im.chat.member.user.added_v1` / `deleted_v1` → `EVENT_KIND_MEMBER_CHANGED`；
   - `im.message.reaction.created_v1` / `deleted_v1` → 解析成包内结构体（消息 id、emoji_type、操作者、增 / 删），**不产出 NormalizedEvent**，Debug 日志 `feishu.reaction_dropped`
     后返回 nil（不报错、不重推），等 T0 的 `EventKind::Reaction` 与 DD9。
   - **形状规则**（违反就在 core 被判 INVALID_ARGUMENT、edge 静默丢）：`event_id` 取 `header.event_id`，缺了就不产出、Debug 日志（别拿 message_id 顶替）；
     `anchor` 必须非 nil（`platform=feishu`、`chat_id`；入群 / 成员事件没有触发消息，`message_id` **留空** —— 别拿 event_id 冒充，下游会把 anchor.message_id 当 reply_to）；
     `chat_type` 事件体里没有就填 GROUP；`occurred_at` 走 `toTime(header.create_time)`、解析不了回退 `timeNow()`；`mentioned=false`、`text=""`、`raw=rawStruct(raw)`。
   - **`sender_kind` 决定 core 走哪条规则**：非 HUMAN 在 R1 就被丢（plane.rs:505-510），到不了 R4（:524-527）。入群 / 成员事件：`sender_id` = 操作者 open_id、HUMAN；
     撤回事件体里没有操作者（`P2MessageRecalledV1Data` 只有 `message_id / chat_id / recall_time / recall_type`，SDK 注释没列 `recall_type` 的取值）：
     一律 HUMAN、`sender_id` 留空（撤回的操作者是发送者本人或群主 / 管理员）。取舍与理由写进回执（DD3 做删根语义时要读）。
   - **字段路径以 SDK 结构体的 json tag 为准**（§2 第 6 条列的 `file:line`），回执里逐个写「字段路径 ← SDK 文件:行」；能打开官方文档就顺带贴 URL，打不开不算缺。
     夹具按这些 json tag 手写、信封照 card_frames_test.go:28-64 的形状、id 换成 helpers_test.go:30-37 那套。
   - **夹具**放 `testdata/events/`：`recalled*.json`、`bot_added*.json`、`member*.json`（各配 `.expected.json`，用 `-update` 生成）。表情夹具放**子目录**
     `testdata/events/reaction/`（`fixtureNames` 的 `*.json` glob 不递归，放在 events/ 顶层会被 `TestEveryFixtureHasExpected` 要 expected），由 `TestReactionIsParsedAndDropped` 按显式路径读。
     给 fixture 读取与黄金循环加第二个根目录（老的 `../../testdata/feishu` 一个不动）。`-update` 之后 `git status --short -- edge/testdata`（仓库根跑）必须为空。
   - 黄金文件只是兜底（normalize_test.go:5-10）：新测试（建议名 `TestNewEventKindsNormalize`）对每个新夹具用字面量钉 `kind / event_id / chat_id / anchor.message_id / sender_id / sender_kind`；
     新测试（建议名 `TestReactionIsParsedAndDropped`）钉：结构体字段解析对、`dispatchRaw` 不进 sink 且返回 nil、`buildDispatcher().Do` 对表情事件返回 nil（不是 `NotFoundEventHandlerErr`）。
     变异：从表里摘掉表情登记 → `Do` 报 NotFound → 红。
   - **改动的钉**：`TestUnsubscribedEventIsDroppedWithoutCallingHandler`（reconnect_test.go:400-416，拆分后在 events_test.go）在 :405 用 `im.chat.member.user.added_v1` 当「没订阅」的例子 ——
     换成仍未订阅的 `im.message.message_read_v1`（与 normalize_test.go:374 同一个），断言不动。`normalize.go:5-6` 的文件头注释同步改。
4. **卡片按钮开关 + 帧类型日志**：
   - `AITE_FEISHU_CARD_BUTTONS` **恰好等于 `"1"`** 才开（别的值一律关）。开时 `SendCard / UpdateCard` 渲染的卡片在 actions 非空时多一个
     `{"tag":"action","actions": buildActions(card)}` 元素，**文字提示照留**；≤30KB 裁剪照旧。实现上加一个带选项的构造（如 `BuildChecklistCardWith`），
     导出的 `BuildChecklistCard(card)` 保持「不带按钮」—— `TestNoDeadButtonsAndAHintInstead`（cards_test.go:360-404）与其余 cards 测试一条不改。
     同步改写 cards.go:120-153 的注释（默认关、H8 定、DD10 翻）。core 只会发 `Stop`（`control/src/card.rs:83`、`worker/src/card.rs:99`），「证据」按钮只在单测里出现 —— 写进回执，别去改 core。
   - **帧类型日志**。SDK 在 `ws/client_message.go:79` 把 `type=card` 帧丢掉，发生在任何 handler **之前**，所以 handler 只可能看到 `event` 帧。两处打 INFO `feishu.card_frame`：
     (a) `OnP2CardActionTrigger` 的回调里，**总是**打 `frame_type=event`（带 `event_id`）；(b) **只在开关开时**，建连加 `larkws.WithLogger(适配器)`（`ws/client.go:68-72`）：
     适配器认出 :77 那条 Debug 行 `receive message, message_type: <X>, …`（注意 :77 传的是 `c.fmtLog(...)` 整个切片、没有 `...` 展开，参数是嵌套切片），
     X 为 `card` 时打 INFO `feishu.card_frame frame_type=card`；**绝不记 payload**（里面有校验令牌与消息正文）。
     注意：装了 `WithLogger` 后 SDK 的 `WithLogLevel` 不再生效（`ws/client.go:216-218`：只有 `cli.logger == nil` 才用 `NewDefaultLogger(cli.logLevel)`），
     所以级别过滤得适配器自己做：Debug 与 Info 一律丢（只把 :77 那行里 `message_type` 为 `card` 的转成上面那条 INFO），Warn / Error 照级别转进 slog。
     开关关时建连参数与今天逐字相同（connection.go:172-179，`WithLogLevel(Warn)`、不装 logger）。
   - 新测试 `TestButtonsOnlyWithFlag`：关 → 无 `action` 元素、有提示；开 → 恰好一个 `action` 元素、按钮与 `buildActions` 一致、提示仍在、`checkCardSchema` 过；
     经 `t.Setenv` + `New` + 假飞书：开时 `SendCard` 与 `UpdateCard` 的请求体都带按钮，关时都不带；`"true"`、`"0"`、空串都算关。变异：忽略开关（恒关 / 恒开）各红一次。
   - 新测试 `TestCardActionTriggerArrivesAsEventFrame`：用 card_frames_test.go:70-187 那套真 websocket 假服务端，但走到 **Platform 一层**：
     `type=event` 帧载 `cardActionPayload` → `OnP2CardActionTrigger` → `dispatchRaw` → sink 收到 `EVENT_KIND_CARD_ACTION`（`action=STOP`、`task_id=t-1`、`card_id=om_checklist_card_0001`），
     且日志有 `feishu.card_frame frame_type=event`。子测试（开关开）：`type=card` 帧 → sink 0 条、日志有 `frame_type=card`、任何日志属性里都不出现 payload 里的两个令牌串（card_frames_test.go:37、:49）。
     `TestGoSDKDropsCardFramesOnTheWire` 断言**一字不改**、必须仍然绿；旧的 `TestCardActionTriggerReachesTheHandlerOnAnEventFrame` 保留（条数只增不减）。变异：删 (a) 或 (b) 各红一次。
   - **装配方式（第 4、6、7 项共用）**：`platformOptions` 加显式字段 —— 如 `cardButtons bool`、发言人姓名查询（接口或函数，nil = 不查）；`New()` 从环境变量填它们，
     `newPlatform` 只认字段、不读环境。`helpers_test.go` 的 `platformBuild`（:287）/ `mustPlatform`（:302-348）加同名字段，测试直接设（开关、假 websocket 域名、
     `newLogCapture` 的 logger 都经它传）。**别用 `slog.SetDefault`，别在 `newPlatform` 里读环境** —— `TestCardActionTriggerArrivesAsEventFrame` 的「开关开」子测试、
     第 6 项的两条测试都走 `mustPlatform`；只有「环境变量确实在 `New()` 里被读」这件事（第 4 项 `TestButtonsOnlyWithFlag` 的 `New` 那半、第 7 项）才走 `t.Setenv` + `New`。
5. **话题历史走 thread 容器**（`ReadHistory` 带 threadID 时）：先 `GET` `PathMessage`（api.go:45，同一个路径的 GET）取 root 的 `thread_id`（`omt_…`；
   响应是 `data.items[]` 数组，取 `items[0].thread_id`，形状照 SDK `GetMessageRespData`）；取到了就按
   `container_id_type=thread`、`container_id=omt_…` 分页拉（`sort_type`、`page_size`、`with_sender_name` 与今天同口径，预算不再 ×4）。返回口径不变：正序、最近 limit 条、
   **含 root**（列表里没有 root 就用第一步取到的补上、按时间归位）、不做 sender_kind 过滤、`HistoryMessage.thread_id` 仍只看 root_id（platform.go:690-693）。
   **回落今天的筛法**（platform.go:498-547 原样保留）的条件只有两个：① root 取不到 `thread_id`（非话题消息或查询失败）；② thread 列表返回**权限错误**。
   权限错误的判据写成具名常量：HTTP 403，外加一张权限类业务码清单 —— 码值不许猜，离线出处是 SDK 的
   `$(go env GOMODCACHE)/github.com/larksuite/oapi-sdk-go/v3@v3.12.0/channel/types/errors.go:79-80`（`99991400 / 99991401 / 230002` → `ErrCodePermissionDenied`）
   与 `:96-97`（HTTP 401 / 403 → 同一类）。用它给常量打底，注释写明出处；并注明疑点：`99991400` 在飞书的通用错误码里可能是限流、不是权限，
   SDK 这张分类表是否可靠未核实。清单在回执里仍标「待 H7 真机补」。其它错误照常上抛。回落打 Debug `feishu.thread_history_fallback reason=…`。
   - 新测试 `TestThreadHistoryUsesThreadContainer`，子测试：(i) 解析出 `omt_x` 后列表请求带 `container_id_type=thread&container_id=omt_x`、没有 chat 容器请求、结果含 root 且正序；
     (ii) thread 列表回权限错误 → 恰好一次 chat 容器请求、结果与今天的筛法一致；(iii) thread 列表回非权限错误 → 错误上抛。变异：恒走 chat 容器 → (i) 红；删回落 → (ii) 红。
   - **相关的钉**：`TestHistoryCanBeNarrowedToOneThread`（read_test.go:192-217）只注册了 `GET PathMessages`，root 查询会撞假服务的 404 → 按回落 ①「查询失败」走今天的筛法，
     **预期不改也绿**。建议再给它注册 root 路由、返回不带 `thread_id` 的消息，让它**有意地**钉住回落 ①，期望 `[root om_in]` 不动；改没改都在回执写明。
     子测试 (iii) 另加一格：root 查询失败 → 回落、不上抛（生产里 root 被撤回就是这种情形）。`TestHistoryAsksForTheMostRecentWindow`（:108-134，无 thread）一字不改。
6. **发言人姓名**（`reads.go`）：`GET /open-apis/contact/v3/users/{open_id}?user_id_type=open_id` 取 `data.user.name`（形状照 SDK `GetUserRespData` / `User.Name`）；
   有界 LRU（`container/list` + map，容量常量写理由），每次查询 `context.WithTimeout(300ms)`（这样也截断 api.go:262-328 的退避重试）；超时 / 出错 → `sender_name` 留 nil、Debug；
   权限错误 → 本进程内熔断一段时间（常量）+ 一条 WARN，别每条事件都打。在 `dispatchRaw` 里、`Normalize` 之后、交 sink 之前补：仅 MESSAGE / CARD_ACTION、`sender_kind=HUMAN`、
   `sender_id` 非空且 `sender_name` 为空时查。**只在 `New()` 里装配**（`newPlatform` 收到 nil 就不查），`Normalize` 保持纯函数 → 老黄金文件不变。要 `contact:user.base:readonly`（H5，plan:656）。
   新测试（建议名）`TestSenderNameFromContactAPIIsCached`（同一发送人两条事件 → 通讯录请求 1 次、两条都有名字）、`TestSenderNameDegradesWithin300ms`（慢路由 → 事件照送、名字 nil、耗时 < 1s；
   权限错误 → nil 且熔断期内第二条不再请求）。慢路由要 `select` 在 `r.Context().Done()` 上返回，否则 `httptest.Server.Close`（t.Cleanup）会一直等 handler，测试尾巴拖几秒。
   现有假飞书的响应函数是 `func(w http.ResponseWriter)`（helpers_test.go:163、:186，`serve` 在 :250-252 调它），拿不到 `*http.Request` ——
   在 `helpers_test.go` 加一个带请求的路由变体（如 `onReq(method, path, func(w http.ResponseWriter, r *http.Request))`），或给这条测试单开一个 `httptest.Server`。
   变异：去缓存 → 请求 2 次 → 红；去超时 → 耗时断言红。
7. **包内读环境变量**（只在 `New()`；打一行 INFO `feishu.env_flags`，只列三个开关的取值）：`AITE_FEISHU_PASSIVE_LISTEN="1"` → `SetPassiveListen(true)`（它的第一个调用方）；
   `AITE_FEISHU_API_BASE` 非空且 `Options.Domain` 为空 → 用它（REST 与长连接同一个域名；显式 `Options.Domain` 优先）；`AITE_FEISHU_CARD_BUTTONS` 见第 4 项。
   新测试（建议名 `TestEnvFlagsAreReadInsideThePackage`，`t.Setenv`）：三个开关各自生效；「全不设」那格用 `t.Setenv(…, "")` 把三个变量**显式清空**
   （别依赖外面的 shell 没导出）→ `FeishuP0()` 与 `DefaultDomain`（`TestCapabilitiesDefaultToTheFrozenFeishuP0` 照旧绿）。变异：删读取 → 红。
   `New()` 读了 `AITE_FEISHU_API_BASE` 之后，`TestNewWithMissingEnvDoesNotExplode`（capabilities_test.go:186-207，拆分后在 platform_test.go，断言 `DefaultDomain`）
   就依赖环境了：导出过这个变量的 shell（包括总管做 H8 时的终端）会让它红。建议给它开头加同样三行 `t.Setenv(…, "")`，断言不动；动了就列进回执「改动的钉」。
8. **每群 5 QPS + 发送 uuid**（`outbound.go`）：每个 chat 一个 `TokenBucket`（300/分、容量 5、同一套可注入时钟），在 `sendMessage` 里先过每群桶、再由 `api.request` 过全局桶（api.go:219-225）；
   桶表有界（常量 + 淘汰闲置）。chat id 取 `SendText / SendFile` 的 `msg.ChatId`、`SendCard` 的 `chatID`，空则跳过每群桶。`UpdateCard`（platform.go:408）签名里没有 chat id：
   建议 `SendCard` 成功后记 `card_id → chat_id`（有界），`UpdateCard` 查得到就过该群的桶，查不到只过全局桶 —— 做或不做都写进回执。
   `TestOutboundIsRateLimitedByTheCapability`（outbound_test.go:476-499）必须仍是恰好 `[30s]`，`TestUploadsDoNotConsumeTheOutboundQuota` 仍是 `[]`。
   uuid：每次 `sendMessage` 生成一次（`crypto/rand`，≤50 字符），放进「发送」与「回复」两种请求体（platform.go:354-371），`rawRequest` 的 3 次重试复用同一个请求体、
   也就复用同一个 uuid；给 DD10 留一个内部入参（有 `dedupe_key` 时由它提供 uuid）。
   **PATCH（`UpdateCard`）请求体不许带 uuid** —— outbound_test.go:109 钉着 `len(body) == 1`（只有 `content`）；uuid 只在 `sendMessage` 里加，别塞进 `api.request` 的通用路径。
   新测试 `TestPerChatPacing`（假时钟、全局额度调大：同一群连发 6 条 → 第 6 条等 200ms；穿插另一群不受影响；全局额度调小时全局桶照样卡）；
   建议名 `TestSendCarriesAStableUUIDAcrossRetries`（先 500 后 200 → 两次请求同一个非空 uuid；两次独立发送 uuid 不同）。变异：去每群桶 / 每次重试重新生成 uuid → 各红一次。

## 6. 规则

- **可写面 / 只读面**见 §3；需要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。
- **B8 不变量**（`evals/p0` 钉死，弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；
  `!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 一个字都不许改。
- **守卫**：被拦就停、拦截原文进回执、不许换写法绕（开场自检第 2 步除外：那一步被拦正是期望）。云端命令里永不出现：`AITE_RELOCK=1`、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`；多行脚本写成文件再跑（跨行引号会被判「无法解析」）；check.sh 不接 `| tail`。
  守卫也扫 heredoc 正文与命令参数：回执（要逐字贴拦截原文，里面有 `guard_bash.py` 与 `proto/…` 路径）和 PR 描述**一律用 Write 工具落文件**，
  再用 `gh pr create --draft --title "…" --body-file <文件>` / `gh pr edit --body-file <文件>` 引用；不走 Bash heredoc / `echo >`，不传多行 `--body "…"`。
  提交信息一行，`git commit -m "…"`。
- **不加依赖**：别跑 `go get` / `go mod tidy`；只用标准库和已经 import 过的模块（lark-oapi-go v3 的子包、`golang.org/x/time/rate`、gorilla/websocket，见 `edge/internal/pin/pin.go`）。
  需要新模块 → 停下报告。
- **每个行为改动**：回归测试 + 变异验证（撤回改动 → 测试红 → 贴输出）。还原用 `git checkout -- <文件>` 或 `git stash pop`，别用 `cp -p` / `shutil.copy2`。
- **格式化**：`gofmt -w <改过的文件>`；本轨不碰 Rust，不跑 `rustfmt`，更别跑 `cargo fmt --all`。
- **测试封闭**：只连 `httptest` 假服务与本地假 websocket，永不连 `open.feishu.cn`；需要环境变量的测试用 `t.Setenv`（因此这些测试不能 `t.Parallel`）。
- 新第三方依赖、R0 文件（不在你可写面里的）、锁定面 → 停下报告。云端 protoc 生成的 `edge/gen` 永不提交（本轨不跑 `make proto-gen`）；Docker 测试封闭（本轨用不到）。
- **已知时序抖动**（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、`aite-testing` 的
  `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。本包 card_frames 那组靠真 sleep 等帧，红了也先单跑。

## 7. 验收（命令 + 期望输出）

命令逐行照抄、每行都从仓库根起跑（`#` 后是期望；命令里的 `|` 是真管道，别写成 `\|`；`(cd edge && …)` 是子 shell，别拆成裸 `cd`）：

```bash
(cd edge && go vet ./... && gofmt -l . | wc -l)                                 # ① exit 0，输出 0
(cd edge && go test -race ./internal/feishu/... -count=1)                       # ② ok
(cd edge && go test -race -count=1 -v ./internal/feishu/ -run 'TestCardActionTriggerArrivesAsEventFrame|TestGoSDKDropsCardFramesOnTheWire|TestButtonsOnlyWithFlag|TestThreadHistoryUsesThreadContainer|TestPerChatPacing' 2>&1) | grep -E '^--- '
#                                                                                 ③ 恰好 5 行 --- PASS，名字逐字
(cd edge && go test -race -count=1 -v ./internal/feishu/ -run 'TestFixtureMatchesExpectedByteForByte|TestEveryFixtureHasExpected|TestExpectedRoundTripsBackIntoTheContract|TestRawIsKeptVerbatim' 2>&1) | grep -E -- '--- (PASS|FAIL)'
#                                                                                 ④ 全 PASS；子测试含 recalled* / bot_added* / member* 与老的 7 个名字
(cd edge && go test -race ./... -count=1) > /tmp/cc8-go-after.txt 2>&1; grep -cE '^(ok|\?)' /tmp/cc8-go-after.txt; grep -c '^ok' /tmp/cc8-go-after.txt; grep -c FAIL /tmp/cc8-go-after.txt
#                                                                                 ⑤ 9、7、0（与开场自检第 5 步一致）
(cd edge && go test -list '.*' ./internal/feishu/) | grep -c '^Test'            # ⑥ 115 + 本轨新增顶层测试条数（逐条列名）
scripts/check.sh                                                                # ⑦ 见下
git diff --name-only origin/main...HEAD                                         # ⑧ 每行都以 edge/internal/feishu/ 开头，或正好是 review/p1/ledger/CC8.md
git diff --name-only origin/main...HEAD -- edge/testdata edge/cmd               # ⑨ 空
git status --short                                                              # ⑩ 空（/tmp 下的对照文件不在仓库里）
```

- ⑦ 期望：末行「全部通过」、exit 0；`cargo passed=<897 或 901> failed=0`（Δ = 0：本轨不碰 Rust）、`contracts passed=<25 或 27> failed=0`、`OK 25 files`、
  `passed 10/10`；Go 那格 8 行（6 行 `ok` + 2 行 `? … [no test files]`，排第一的 `cmd/aite-edge` 被截掉），单跑 `cmd/aite-edge` → `ok`，合计 9 包。

- ⑦ 的基线数按开场自检第 1 步判定的情形取（A：897 / 25；B：901 / 27）。**别接 `| tail`**。
- Go 模块文件有没有被改，由总管审 PR 时看；你别在命令里 grep 它们。
- 第 1 项的零行为判据（test 名单 diff、`--- PASS` 总数、`go doc -short` diff、逐文件对照表）另在回执里单列。

## 8. 回执（写 `review/p1/ledger/CC8.md`，PR 描述贴摘要）

回执**只用 Write 工具**写（它要逐字贴守卫拦截原文，里面有受保护路径，Bash heredoc / `echo` 会被拦）；PR 描述用 `gh pr edit --body-file review/p1/ledger/CC8.md`
（或先 Write 一份摘要文件再 `--body-file` 它）同步，见 §3、§6。

1. **开场自检原文**（4 + 1 项）：第 1 步 diff 输出与判定（A / B）；守卫拦截原文（逐字）；三条工具链版本；check.sh 各行原样；
   第 5 步的 9 / 7 / 0（注明原卡「`^ok` → 9」有误，按 7 ok + 2 ? 验）、115、P0、逐文件条数。
2. **工作项逐条**：1–8 每项改了哪些 `文件:行`；第 1 项的四样零行为判据原样贴出；新事件的字段来源（字段路径 ← SDK 文件:行）与 `sender_kind` 取舍；
   权限错误码常量的出处；LRU 容量 / 熔断时长 / 每群桶表上限的取值与理由；`UpdateCard` 过不过每群桶的决定。
3. **新增测试逐条 + 变异验证输出**：每条钉什么；变异怎么做的；红的那段输出逐字贴。另列「改动的钉」：`TestUnsubscribedEventIsDroppedWithoutCallingHandler`、`TestHistoryCanBeNarrowedToOneThread`（改前改后各一句话；没改就写「未改、仍绿、走的是回落 ①」）。
4. **check.sh 完整输出**（原样，不截）+ §7 ⑤ 的输出。
5. **`cargo passed` 增量**：应为 0；不是 0 就逐条解释。Go 侧新增测试按文件列名，合计 = §7 ⑥ 的增量。
6. **被守卫拦过的命令与拦截原文**（没有就写「无」）。
7. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）。至少考虑：`docs/acceptance-M.md` §7 登记 `feishu.card_frame` / `feishu.reaction_dropped` / `feishu.thread_history_fallback`
   （文档不在可写面，总管或 GG1）；core 让卡片带 `Evidence`（CC2 / CC3 之后的 DD5）；表情 → `EventKind::Reaction`（DD9）；环境变量换成 `FeishuConfig.api_base / card_buttons`（DD8 / DD9 / DD10）；
   撤回删根语义（DD3）；`card_buttons` 默认值（DD10，按 H8）；
   本轨新建的 `api_test.go` / `platform_test.go` / `helpers_test.go` 在 W2 没有主人（英文原卡 DD9 只列了 `platform.go`、`api.go`，没列这两个的 `_test.go`；
   DD9 / DD10 都没列 `helpers_test.go`，而 DD9 给 `testdata/read/` 加夹具根很可能要改它）—— 为什么不在本轨：本轨只定布局、不定 W2 归属；建议：总管补进 DD9 可写面。
8. **没做的与原因**。
9. **契约缺口**（给 T0 / T0.1）：只报 T0 已计划字段之外的 —— 已计划的见 plan §5.2「平台」「事件 / 出站」两条（plan:457-465），与本轨相关的有
   `FeishuConfig += api_base, card_buttons, service_user_token_env`、`EventKind += Reaction, External`、`NormalizedEvent += quote, reaction, external, sender_external`、
   `OutboundText += mentions, dedupe_key`、`HistoryMessage += updated_at, deleted, reply_to`、新消息 `UserInfo / ChatInfo`、能力位 `supports_reactions_in / recall_event`。
   至少判断一条：`MEMBER_CHANGED` 分不出加入 / 退出、拿不到成员列表（只在 `raw` 里，而 events.proto:92 规定任何逻辑不得依赖 raw）—— 写清需要什么形状、为什么开放通道绕不过去；没有就写「无」。
10. **H8 备用路径**（把下面这段原样抄进回执末尾，给总管本机照做；`review/p1/ledger/CC8-clicktest.md` 由总管写）：

```bash
cd ~/Documents/Projects/Aite && git fetch origin <本 PR 的 claude/… 分支名> && git worktree add .worktrees/cc8-h8 FETCH_HEAD
cd ~/Documents/Projects/Aite/.worktrees/cc8-h8/edge && go build -o ~/Documents/Projects/Aite/edge/bin/aite-edge-cc8 ./cmd/aite-edge
cd ~/Documents/Projects/Aite && AITE_FEISHU_CARD_BUTTONS=1 edge/bin/aite-edge-cc8 --config config/aite.yaml 2>&1 | tee /tmp/h8-edge.log
cd ~/Documents/Projects/Aite && core/target/debug/aite run --config config/aite.yaml
```

   - 第 3、4 行**分别在两个终端里跑**（都在仓库根，谁先起都行，见 acceptance-M §0.2.2）—— 它们都是常驻前台进程，贴进同一个终端的话 core 永远起不来。
   - 为什么在 worktree 里编、回主仓库根跑：`config/aite.yaml` 不入库（`.gitignore:20`），新 worktree 里没有；socket 等相对路径按仓库根解析（acceptance-M §0.2）。
     `edge/bin/` 已被忽略（`.gitignore:27`）。本轨不碰 `server.go`，两边契约版本都是 `p0.2`，main 上编好的 core 直接能连。飞书凭证照平时的环境变量给。
   - 操作：测试群里 @Aite 发一个要跑一阵的任务，卡片「进行中」时点「停止」，然后 `grep feishu.card_frame /tmp/h8-edge.log`。
     `frame_type=event` 且卡片变「已取消」→ **PASS**；`frame_type=card` → **FAIL**（平台按旧版回调发帧，SDK 丢弃）；一行都没有 → 先查 H5 的回调订阅（长连接 + `card.action.trigger`）再点一次。
   - 收尾：`cd ~/Documents/Projects/Aite/.worktrees/cc8-h8 && git clean -xdff`，再 `cd ~/Documents/Projects/Aite && git worktree remove .worktrees/cc8-h8 && rm edge/bin/aite-edge-cc8`
     （先 clean：Finder 的 `.DS_Store` 会让 remove 报 Directory not empty）。
