# 派单 CC9：钉钉 Stream 适配器包（不接线）（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC9.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC9）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

（下文引总计划一律用节号 / 表行名，不写行号 —— D0 之前总计划还在改，行号会漂。）

**对应 Claude Tag 的哪几条**：NEW25（Claude Tag 只有 Slack、Teams「即将支持」；我们的对等扩展是钉钉，总计划 §2 B 表 NEW25 行）、
CT01（频道内任何人 @ 即起任务，钉钉走 Stream `/v1.0/im/bot/messages/get` + `isInAtList`，§2 A 表 CT01 行）、
CT02（话题 = 会话；钉钉没有话题 → 可见 `#A17` 锚点 + 引用内容续接，§2 A 表 CT02 行、§2 C「钉钉和企微没有话题」一条）、
CT13（就地编辑的清单卡；钉钉 AI 卡片流式每次 ≤1KB、回调 2 秒，§2 A 表 CT13 行、§2 C「CT13 流式进度卡有平台上限」一条）。
平台事实（§1.3 钉钉一条）：卡片回调时限 **2 秒**；群里 @ 机器人**收不到文件**（只有单聊能收，§2 C「CT11/NEW04 文件」一条）；
引用回复字段 `text.repliedMsg` **没有文档**、官方 Go SDK 会丢掉它。D8：钉钉只用机器人身份（§3 D8）；D10：一个部署只接一个 IM（§3 D10）。

**Aite 今天的样子**：只有飞书。

- `edge/internal/server/ports.go:14-26` 的 `PlatformPort` 只有 `internal/feishu` 一个实现（:4）；`:15` 规定只返回 `*aiteerr.PlatformError` 或 nil。
- 配置只认 `feishu | fake`：`edge/internal/config/config.go:94-99`；`edge/cmd/aite-edge/main.go:86-91` 只会 `feishu.New(...)`，
  `:128-131` 只在 `platform == "feishu"` 时 `Start`，`:59-61` 读 `Connected()` / `ReconnectCount()` 给 EdgeStatus。
  这两个文件分别是 R0（config.go:5「owner: R0」）与 P0-CLOSE 文件，**所以本轨只交一个独立包，不接线**（接线是 DD8 的）。
- `Anchor.task_no` 的注释还是「P0 只展示不路由」（`edge/gen/aitepb/events.pb.go:300`）。T0 契约只改它的文档串（改成「edge 从本文或引用内容解析 `#A`，core 按它路由」），
  字段不变 —— **你是第一个往 `task_no` 里填东西的 adapter**。
- 任务号格式：`#A` + **Crockford base32**（`core/crates/contracts/src/session.rs:168-189`，向量 `1→#A1`、`17→#AH`、`32→#A10`、`1000→#AZ8`，
  字母表去掉了 I L O U）；core 用 `normalize_task_no` 转大写（`core/crates/control/src/commands.rs:24-34`）。**只认 `#A\d+` 是错的**。
- core 按 `anchor.thread_id` 找话题会话（`core/crates/control/src/plane.rs:1289-1301`），R7 建会话时把 `thread_id` 设成触发消息的 `message_id`（plane.rs:545-551），
  R6 只在群里续接（:538-543）。所以把 `repliedMsg.msgId` 放进 `thread_id`：引用的是**用户自己那条根消息**时会命中正确的会话；
  引用的是 **Aite 的回复**时大概率对不上（它存的 id 是什么正是 H9 要探的，总计划 §8 H9）→ 走 R7，和不填一样。这就是「只作提示」的含义。

**谁接你的东西**：DD8（W2）按 `Start / Connected / ReconnectCount` 做平台工厂，把 T0 的 `DingtalkConfig` 映射成你的 `Config` / `Options`；
DD11（W2，可写面同样是 `edge/internal/dingtalk/**`）把能力值对齐契约的 `dingtalk_v1()`、加审批回调、@ 人（`atUserIds`）、DirectSender、OAuth，锚点仍以 `#A` 文本为主
（DD11 原卡原话「Anchors stay content-first (#A); repliedMsg.msgId is used only once the H9 probe confirms」）；
T0 之后 `NormalizedEvent.quote` 由谁用你的 `ParseQuote` 填，原卡没写 —— 建议归 DD11（写进回执「记账转出去的」）；DD3（W2）按你填的 `Anchor.task_no` 路由；
FF3（W4）要新建 `edge/internal/dingtalk/stream_card.go` + `stream_card_test.go`（增量流式），HH3（W6）要新建 `dws.go` + `dws_test.go` —— **这四个文件名留给它们，你别占**；
总管在 H9 把钉钉探针结果写进 `review/p1/ledger/H9-dingtalk-probe.md`，DD11 再并进你建的 `docs/p1/dingtalk.md`（总计划 §8 H9：这份 docs W1 归你、W2 归 DD11）。

**协议要点**（卡片原文 + 09-25 读官方 SDK `open-dingtalk/dingtalk-stream-sdk-go` 源码；云端打不开 open.dingtalk.com。
标「推断」的写进 `docs/p1/dingtalk.md` 的「待 H9 真机核实」，假网关按同样的假设写）：

| 环节 | 形状 |
|---|---|
| 开连接 | `POST {api_base}/v1.0/gateway/connections/open`，体 `{"clientId","clientSecret","subscriptions":[{"type","topic"}],"ua","localIp"}`；订阅三条：`EVENT *`、`CALLBACK /v1.0/im/bot/messages/get`、`CALLBACK /v1.0/card/instances/callback` → 回 `{"endpoint","ticket"}` |
| 建 ws | 拨 `endpoint + "?ticket=" + ticket`（SDK 就这么拼）；**ticket 一次性**：每次重连都重新 `open` 拿新 ticket（SDK 的 reconnect 也是整套重走） |
| 下行帧 | JSON 文本帧 `{"specVersion","type","time","headers":{"messageId","topic","contentType",…},"data":"<JSON 字符串>"}`，`type` 取 SYSTEM / EVENT / CALLBACK |
| ACK | 每一帧都回 `{"code":200,"headers":{"contentType":"application/json","messageId":<原样回显>},"message":"OK","data":"<JSON 字符串>"}`；失败用非 200 code（SDK 的 `NewErrorDataFrameResponse`） |
| SYSTEM | `topic=ping` → 立即 ACK，data 原样回显；`topic=disconnect` → 关掉本连接、走重连（新 ticket） |
| EVENT | ACK data `{"status":"SUCCESS","message":"success"}`（`LATER` = 要平台重推，推断） |
| 机器人消息 data | `conversationId`、`conversationType`（`"1"` 单聊 / `"2"` 群）、`msgId`、`msgtype`（`text`/`richText`/`picture`/…）、`text.content`、`content`（richText / picture）、`isInAtList`、`senderStaffId`、`senderId`、`senderNick`、`chatbotCorpId`、`robotCode`、`createAt`（ms）、`sessionWebhook`、`sessionWebhookExpiredTime`（ms）；**未文档化**：`text.isReplyMsg`、`text.repliedMsg{msgId, senderId, msgType, content…}` |
| 卡片回调 data | `outTrackId`、`userId`、`content`（JSON 字符串，内含 `cardPrivateData.params`，推断）；**2 秒内必须 ACK** |

## 2. 必读（按顺序）

1. 仓库根 `CLAUDE.md`（云端唯一能读到的约定；与本派单冲突时以本派单为准）。
2. 总计划（按节号找，别按行号）：§4.4 开场自检、§4.5 云端命令禁区（「云端命令里永远不出现」那条）、§5.1 P0-CLOSE 文件（「W1 没有任何一轨碰」那条）、§5.2「平台」一条、
   §6 开头的每波规则、§6.1 CC9 行、§7 R0 表 `edge/internal/pin/**` / Go 模块文件那行、§8 H9、§9 解冻清单、§10「钉钉 / 企微只在假件上验过」那行。
3. 英文原卡 `review/p1/tracks-2026-09-25.json`：`waves[0].tracks` 里 `id == "CC9"`；`contract_batches` 里 T0-p1.0 的 `DingtalkConfig`、`dingtalk_v1()`、`Quote`、`Anchor` 那几条
   （多行脚本写成文件再 `python3` 跑，别用跨行引号）。
4. 照着写的飞书实现（**只读、只抄形状，不许 import**：CC8 本波正在拆这个包的文件）：
   `edge/internal/feishu/platform.go` —— `EventSink`（:75-78）、`Options`（:80-87）、`Platform` 与可注入面（:89-125）、`New`（:197-200）、
   `Capabilities` 返回副本（:218-227）、`Start` 重连循环（:240-290）、`Connected` / `ReconnectCount`（:301-305）、`dispatchRaw`（:307-339，:311-312 是「失败 → 返回错误让平台重推」）、
   `UpdateCard` 绝不新发（:404-407）；`connection.go` 的 `Connection` / 退避 1,2,4,8,16,30（:31-69）；
   `normalize.go` 的 `rawStruct` 剥校验令牌（:391-425）、`NormalizeMessage`（:439-513）、`NormalizeCardAction`（:549-619，未知动作返回 nil 在 :561-569，按钮人 / Mentioned 在 :595-601）；
   `api.go` 的 token 缓存（:56-57、:157-208）、`transportError`（:390-400）、`errorFromResponse`（:454-475）。
5. 测试范式：`feishu/helpers_test.go:27`（`var _ server.PlatformPort = (*Platform)(nil)`）、:43-80（fakeClock）、:85-120（logCapture）、:167-200（fakeFeishu）；
   `feishu/card_frames_test.go:70-157`（gorilla `websocket.Upgrader` 起假 ws 服务端）；`feishu/reconnect_test.go:143-168`（recordingSink）。
6. `edge/internal/aiteerr/errors.go`：`PlatformError`（:17-22）、`ErrNotImplemented`（:72-73）→ `ToStatus` 翻成 `codes.Unimplemented`（:83-84）、重试映射（:97-114）。
7. pb 类型：`events.pb.go`（`ChatType` :146-152、`AttachmentKind` :195-201、`CardActionKind` :245-251、`Anchor` :294-300、`NormalizedEvent` :522-541）、
   `capabilities.pb.go:28-38`（今天只有 9 个字段）、`outbound.pb.go`（`OutboundText` :191-196、`ChecklistCard` :327-337、`SendResult` :511-514）。
8. `edge/internal/pin/pin.go:9`：`github.com/gorilla/websocket` 已钉，直接 import，**不碰 Go 模块文件**。

## 3. 工作区

- **分支**：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC9: 钉钉 Stream 适配器包（不接线）」。
  PR 描述先用 **Write 工具**写成 `/tmp/cc9-pr.md`（此时回执还不存在），再 `gh pr create --draft --title "CC9: 钉钉 Stream 适配器包（不接线）" --body-file /tmp/cc9-pr.md`；
  收尾时 `gh pr edit --body-file review/p1/ledger/CC9.md`（或先 Write 一份摘要文件再 `--body-file` 它）。**永远别**把多行正文塞进 `--body "…"` 或 heredoc（见 §6 守卫）。
- **代码基线**：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**（原卡逐字；三处今天都不存在，全部新建）：`edge/internal/dingtalk/**`、`docs/p1/dingtalk.md`、`review/p1/ledger/CC9.md`。
  建议文件：`platform.go`、`stream.go`、`normalize.go`、`anchor.go`、`api.go`、`outbound.go`、`card.go`、`callback.go` 与各自 `_test.go`、`helpers_test.go`，
  夹具放 `edge/internal/dingtalk/testdata/*.json`（**不是** `edge/testdata/`，那是飞书的、不在你面里）。**不建** `stream_card.go`、`stream_card_test.go`、`dws.go`、`dws_test.go`。
- **只读面**：其余一切。特别点名：
  - P0-CLOSE 文件（总管在本波期间本机打）= §4 第 1 步列的那 14 个路径：`proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`、
    `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
    `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
    `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`、`.contracts.lock`、
    `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；外加总计划 §5.1 点名的整目录 `core/crates/evidence/**`、`.claude/**`。
    （这只是一张对照清单，用来读 diff 输出；这些路径永远不写进你的命令。）
  - T0 补丁文件：`config/aite.example.yaml`、`edge/internal/server/server.go`、`core/crates/proto/**`（外加锁定面 `proto/aite/v1/*.proto`、`core/crates/contracts/**`）。
  - 离你最近的别轨 / R0 面：CC8 的 `edge/internal/feishu/**`（只读参考，不 import）；CC10 的 `edge/internal/wecom/**`（同波同形状，**不共享代码、不建公共包**）；
    CC11 的 `edge/internal/egress/**`；R0 面（按总计划 §7）：`edge/internal/server/**`（T0 补丁 → T0c → DD8）、`edge/internal/config/**` 与 `edge/internal/aiteerr/**`（DD8）、
    `edge/cmd/aite-edge/**`（AA4 即总管 P0-CLOSE → DD8；`main_test.go` T0c → DD8）；
    没人拥有的 `edge/internal/pin/**` 与 Go 模块文件；`edge/gen/**`（只有总管本机 `make proto-gen`）；CC1 的 `scripts/**`、`Makefile`、`.github/**`。
- **本轨解冻的冻结项**：无（§9 没给 CC9 列解冻项）。「`platform: dingtalk` 被拒」（config.go:94-99）由 T0 / DD8 翻，**你别动**。

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
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（期望形如「blocked: 该操作触碰受保护面 …」；拦截原文逐字贴进回执）。
   **这一步被拦就是通过；拦截原文里「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步。**
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可 —— 本轨就是「其它轨」，照跑、照记，不因此停）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**（冷编译 10–15 分钟，耐心等）。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认 → `ok`。
   若 CC1 已合并、check.sh 多打一行 `go packages ok=N fail=M`，以那行为准。
5. **本轨附加**：
   - 一条命令、只跑一遍全量 Go：
     `cd edge && go test -race ./... -count=1 > /tmp/cc9-go.txt 2>&1; grep -c '^ok' /tmp/cc9-go.txt; grep -cE '^(ok|\?)' /tmp/cc9-go.txt; grep -c '^FAIL' /tmp/cc9-go.txt`
     → 三个数依次是 **N0**、`9`、`0`（最后那个 grep 打印 0 时退出码是 1，属正常）。
     按源码静态数 N0 应是 **7**（有 `_test.go` 的包：`cmd/aite-edge`、`aiteerr`、`config`、`feishu`、`ingress`、`sandbox`、`server`；
     `gen/aitepb` 与 `internal/pin` 打 `?  … [no test files]`，9 行合计就是「9 包」）。原卡把它写成 9 —— 两个数都进回执，以实测为准。
   - `grep -n 'gorilla/websocket' edge/internal/pin/pin.go` → 第 9 行；`ls edge/internal/dingtalk docs/p1/dingtalk.md` → 两个都不存在。

## 5. 工作项

原卡没点测试名（验收只列了 5 个场景）；以下 13 个 `Test*` 名字由本派单定，回执与 PR 逐字沿用。

**总纪律**：只用标准库 + 已钉的 `gorilla/websocket`（随机 id 用 `crypto/rand`，别找 uuid 库）；错误只有 `*aiteerr.PlatformError` 与 `aiteerr.ErrNotImplemented` 两种；
`clientSecret`、access token、`sessionWebhook` 永不进日志、错误文本、`NormalizedEvent.raw`；所有测试只连 `httptest` 起的本地假服务，永不拨真实钉钉域名（真实域名只许作为默认值常量被断言）。
**REST 路径与请求体的来源**：只有 Stream 的 open / 帧 / ACK 形状核对过官方 SDK 源码；下面第 5–6 项的 REST 路径、请求头、请求体键名（`/v1.0/oauth2/accessToken`、
`x-acs-dingtalk-access-token`、`groupMessages/send`、`oToMessages/batchSend`、`messageFiles/download`、`card/instances/createAndDeliver`、`card/streaming`、`dtv1.card//…`）
来自总管 09-25 的调研记忆，没对过官方文档，云端也打不开。全部集中成 `api.go` 顶部的 `Path*` 常量（照 feishu api.go:41-54），并逐条列进 docs 的「待 H9 真机核实」。

1. **包骨架与生命周期**（`platform.go`）。
   - `Config`：包内镜像 T0 的 `DingtalkConfig`（`ClientIDEnv` 默认 `DINGTALK_CLIENT_ID`、`ClientSecretEnv` `DINGTALK_CLIENT_SECRET`、`RobotCodeEnv` `DINGTALK_ROBOT_CODE`、
     `CardTemplateIDEnv` `DINGTALK_CARD_TEMPLATE_ID`、`APIBase` `https://api.dingtalk.com`、`BotName` `Aite`，yaml 标签用 snake_case），外加 `DefaultConfig()`；
     注释写明「`edge/internal/config` 是 R0，DD8 接线时把 `config.Dingtalk` 映射过来」。
   - `Options{ClientID, ClientSecret, RobotCode, CardTemplateID, TenantID, APIBase}`（已从环境变量解析好，照 feishu :80-87）；`EventSink` 自己定义一份同形接口（feishu :76-78）。
   - `New(cfg Config, opts Options, sink EventSink) (*Platform, error)` 只装配不建连；内部 `newPlatform(platformOptions{…})` 暴露给测试的注入面：`httpClient`、`dialer`、`sleep`、`clock`、`logger`。
   - `Start(ctx) error` / `Connected() bool` / `ReconnectCount() int64` 签名与 `*feishu.Platform` 逐字相同；循环照 feishu :240-290（只有 ctx 取消才返回；退避 1,2,4,8,16,30 封顶、无限重试；成功重连才 `ReconnectCount+1`）。
   - `Capabilities()` 返回 `proto.Clone` 的副本（照 :218-227）。只用今天的 9 个字段：`Platform:"dingtalk"`、`SupportsThread/History/PassiveListen:false`、
     `SupportsCardEdit:true`（AI 卡片实例可更新）、`InboundFileInGroup:false`；另外三个云端查不到官方文档，**照下面的保守值写死**：
     `CardEditWindowSec: 0`（能更新多久待 H9 探；core 今天没有这个值的消费者 —— `card_edit_window_sec` 只出现在 contracts / proto 转换 / testing 的假平台里，所以 0 只是被钉住的占位）、
     `ProactiveRequiresPriorMessage: false`（`groupMessages/send` 能主动发）、`OutboundRatePerMin: 20`。
     三个值在 docs 能力值表里都标「待核实，DD11 按 `dingtalk_v1()` 对齐」。
   - 测试：`TestPlatformImplementsPort`（`var _ server.PlatformPort = (*Platform)(nil)` + `DefaultConfig` 的 6 个默认值逐字 + `Capabilities()` 的 9 个字段值逐个断言）、
     `TestCapabilitiesReturnsACopy`（改返回值不影响下一次读；`-race` 下并发读）。
2. **Stream 客户端**（`stream.go`）：按 §1 表。`open` 请求体三条订阅逐字；每轮连接先 `open` 拿新 ticket 再拨 `endpoint?ticket=`；读循环逐帧解码；**每一帧都 ACK 并原样回显 `headers.messageId`**；
   SYSTEM `ping` 当场 ACK（data 回显），`disconnect` → 关本连接、下一轮重新 `open`；服务端断开 → `Connected()` 变 false、退避后重连。
   gorilla 的 `Conn` 同一时刻只许一个写者：所有 ACK 走一把写锁；收尾时等在途 goroutine 退出（`Close` 幂等）。
   **读循环永不在本 goroutine 里调 sink**：它只解码、分发。SYSTEM ping 与卡片回调在读循环里当场 ACK；机器人消息帧投给**一个串行 dispatcher goroutine**
   （保序：`@Aite 起任务` 紧跟 `@Aite !stop` 不许乱序），dispatcher 调 sink、sink 返回后经写锁 ACK。这样慢 sink（core ingress 截止默认 1000 ms，
   `edge/internal/config/config.go:68`，加上重试可能更久）拖不住后面的 ping / 卡片回调帧的读取与 ACK。
   - 测试：`TestStreamOpenConnectCallbackAckEchoesMessageID`（钉：open 体里的三条订阅；ws 请求带 `ticket`；推一帧机器人消息 CALLBACK → sink 收到归一化事件、假服务端收到 `code 200` + 同一 `messageId` 的 ACK；
     再推 SYSTEM ping → 同样回显）；`TestReconnectReopensWithFreshTicket`（假网关每个 ticket 只放行一次、复用即 401；服务端断开或发 `disconnect` 后，客户端再 `open` 一次、用的是新 ticket；
     `Connected` / `ReconnectCount` 跟着变；退避用注入的 `sleep`，不吃墙钟）。变异：ACK 写死空 `messageId` → 前者红；缓存首个 ticket 复用 → 后者红。
3. **归一化**（`normalize.go`）：机器人消息 data **按 `map[string]any` 解析**（未文档化字段要从原始 JSON 里取，官方 SDK 的结构体会丢掉它们）。
   - `Kind MESSAGE`、`Platform "dingtalk"`、`EventId = msgId`（平台重推同一条消息 msgId 不变，core 靠它去重；adapter 自己不去重）、`TenantId = opts.TenantID`（空 → `default`）、
     `WorkspaceId = chatbotCorpId`（空退 `senderCorpId`）、`ChatId = conversationId`、`ChatType`：`"1"` → `P2P`、`"2"` → `GROUP`；
     `SenderId = senderStaffId`（空退 `senderId`）、`SenderName = senderNick`、`SenderKind HUMAN`；`Mentioned = isInAtList`（单聊不强行置 true，DM 路由是 DD3 的）；
     `OccurredAt = createAt`（缺失用注入时钟）。
   - 正文：`text` → `text.content`，去首尾空白，开头若是 `@<BotName>` 再剥掉（否则 `@Aite !stop` 过不了 R5 的 `starts_with('!')`，plane.rs:534）；`RawText` 放原文。
     `richText` → 按顺序拼文本段，图片段进附件；`picture` → 一个图片附件。附件 `Kind IMAGE`、`FileKey = downloadCode`（空退 `pictureDownloadCode`）、`MessageId = msgId`。
     其它 msgtype（file / audio / video …）→ 返回 nil + Debug 日志（照 feishu :320-322），记进「记账转出去的」给 DD11。
   - `Raw`：整份 data 进 `structpb`，**剥掉 `sessionWebhook`**（它能直接往会话里发消息，同 feishu :391-397 剥校验令牌的理由）。
   - 解析 `sessionWebhook` / `sessionWebhookExpiredTime` / `conversationType` / `senderStaffId` / `chatbotCorpId` 进 `conversationId → 会话缓存`（给第 5、6 项用）。
   - 导出 `ParseQuote(data map[string]any) *Quote`（`Quote{MessageID, SenderID, Text}`；只在 `text.isReplyMsg == true` 或 `text.repliedMsg` 存在时非 nil；
     `repliedMsg.content` 是对象取 `.text`、是字符串直接用，其它形状留空、不 panic）。T0 之后 `NormalizedEvent.quote` 才存在，由谁用这个函数填它建议归 DD11（记账转出去）。
   - 测试：`TestNormalizeFixtures`（表驱动读 `testdata/` 下 `text_group.json`、`text_p2p.json`、`rich_text.json`、`picture.json`、`quote_reply.json`、`quote_reply_string_content.json`，
     逐字段比期望；并断言 `Raw` 里没有 `sessionWebhook`）。变异：把 `ChatType` 判反或删掉剥 webhook 那行 → 红。
4. **`#A` 锚点**（`anchor.go`）：`TaskNoOf(text string) (string, bool)` —— 不分大小写匹配 `#A` + Crockford 字符 `[0-9A-HJKMNP-TV-Z]+`，后一个字符若是 ASCII 字母或数字就不算（`#Awesome` 不是锚点），
   结果转大写。**先看本条正文，找不到再看引用内容**，都没有 → `Anchor.TaskNo = nil`。`Anchor.ThreadId = repliedMsg.msgId`（只作提示，§1 最后一段；没有引用 → nil）；`Anchor.MessageId = msgId`。
   代码注释里写明「H9 探针（总计划 §8 H9）确认前 `#A` 文本是主锚点；`thread_id` 只是提示」（注释里不写行号）。
   - 测试：`TestAnchorTaskNoFromOwnTextThenQuote`（向量 `#A1`、`#AH`、`#A10`、`#AZ8`、小写 `#ah` → `#AH`、`#AH进展如何` → `#AH`、`#Awesome` → 无；本文与引用都有时取本文；只在引用里 → 取引用；
     `thread_id` = `repliedMsg.msgId`）。变异：正则改成 `#A\d+` → `#AH`、`#AZ8`、`#ah`、`#AH进展如何` 四格红（本文 / 引用组合若也用了字母号，一并红）。
5. **出站文本与文件下载**（`outbound.go` + `api.go`）。
   - `api.go`：`POST /v1.0/oauth2/accessToken`（`{"appKey","appSecret"}` → `accessToken`、`expireIn`），带提前 60 秒过期的缓存（照 feishu api.go:56-57、:157-201），401 时作废重取一次；
     请求头 `x-acs-dingtalk-access-token`；HTTP 失败 → `PlatformError{Code: 响应体 code 或 HTTP 状态, HTTPStatus, Retryable: 429/5xx, Msg}`，传输失败 → `timeout` / `transport_error` 且可重试。
     **别照抄 feishu 的 `transportError`**（`edge/internal/feishu/api.go:394-400` 把 `err.Error()` 整个放进 `Msg`，而 net/http 的 `*url.Error` 文本带完整 URL，
     `sessionWebhook` 的 query 里就是会话令牌）：webhook 请求的传输错误先 `errors.As` 出 `*url.Error`，把它的 `URL` 换成去掉 query 的形式，再交给 `transportError`；日志同理。
   - `SendText`：`msg == nil` → `bad_request`（照 feishu :382-384）。会话缓存里有 `sessionWebhook` 且没过期（到期前留 60 秒余量）→ POST 它，`{"msgtype":"markdown","markdown":{"title","text"}}`
     （这条路不给消息 id，`SendResult.MessageId` 留空；core 今天不读 `send_text` 的返回值：plane.rs:718、worker/src/agent.rs:750-757、:791）；
     否则走机器人 API：群 → `POST /v1.0/robot/groupMessages/send`（`openConversationId` = chat_id），单聊 → `POST /v1.0/robot/oToMessages/batchSend`（`userIds` = 缓存的 `senderStaffId`；
     缓存里没有 → 不可重试的 `PlatformError`），`msgKey "sampleMarkdown"`，`MessageId = processQueryKey`。`ReplyTo` / `InThread` 钉钉没有对应物，忽略并在 docs 写明。
   - `DownloadFile(ctx, messageID, fileKey)`：`POST /v1.0/robot/messageFiles/download`（`{"downloadCode": fileKey, "robotCode"}`）→ `downloadUrl` → GET 取字节（不带 token 头）。
     单聊才收得到文件（总计划 §2 C「CT11/NEW04 文件」一条），群里只有图片走这条。
   - 测试：`TestAccessTokenIsCachedUntilExpiry`（两次调用只打一次 token 端点；时钟推过期后再打一次；日志里没有 secret / token）；
     `TestSendTextUsesSessionWebhookThenRobotAPI`（有效 webhook → 只打 webhook；过期 → 群走 groupMessages、单聊走 oToMessages 且 `userIds` 对；返回的 `MessageId` 分别是空与 `processQueryKey`；
     再把 webhook 假服务关掉后发一次 → 返回的 `PlatformError.Msg` 与捕获的日志里都**不含**会话令牌 `session=` 的值）；
     `TestDownloadFileViaDownloadCode`（两步请求体与返回字节逐字）。变异：忽略 webhook 过期 → 第二条红；去掉 token 缓存 → 第一条红。
6. **AI 卡片**（`card.go`；**不叫** `stream_card.go`）。
   - `SendCard(ctx, chatID, replyTo, card)`：`outTrackId = "aite-" + 16 位随机 hex`；`POST /v1.0/card/instances/createAndDeliver`（`cardTemplateId` 来自 `Options.CardTemplateID`，空 → 不可重试错误；
     `callbackType "STREAM"`；群 `openSpaceId = "dtv1.card//IM_GROUP." + chatID` 配 `imGroupOpenDeliverModel{robotCode}`，单聊 `dtv1.card//IM_ROBOT.<staffId>` 配 `imRobotOpenDeliverModel{spaceType:"IM_ROBOT"}`）；
     `cardData.cardParamMap` 至少放 `content`（清单渲染成的 markdown）、`task_id`、`task_no`、`status`；返回 `SendResult{MessageId: outTrackId, CardId: &outTrackId}`，并记下 `outTrackId → {chatID, chatType, corpId}`（`chatType` / `corpId` 取第 3 项会话缓存里的 `conversationType` / `chatbotCorpId`，取不到留零值；给第 7 项填 `ChatId` / `ChatType` / `WorkspaceId`）。
   - `UpdateCard(ctx, cardID, card)` = 卡片实例更新：`PUT /v1.0/card/instances`（`outTrackId = cardID`、同一套 `cardParamMap`、`cardUpdateOptions.updateCardDataByKey = true`），**绝不新发一张**（照 feishu :404-407）。
   - 导出 `StreamCard(ctx, cardID, markdown string, finalize bool) error`（不在 `PlatformPort` 上，FF3 接）：`PUT /v1.0/card/streaming`，`{outTrackId, guid(每次调用新生成), key:"content", content, isFull, isFinalize, isError:false}`。
     原卡原话「≤ 1 KB per call, full markdown each time」的实现口径：**每次都把完整 markdown 重发**（不做增量 diff，那是 FF3 的），按 rune 边界切成 ≤1024 字节的块，
     第一块 `isFull=true`、其余 `isFull=false`（追加），`finalize` 时只有最后一块 `isFinalize=true`。拿不准就照此实现，回执写明。
   - 模板变量名（`content` / `task_id` / `task_no` / `status`）与 Stop 按钮回传的参数（`{"action":"stop","task_id":"${task_id}"}`）写进 docs 的「H9 建模板须知」。
   - 测试：`TestSendCardCreateAndDeliverThenUpdateInPlace`（群 / 单聊两种 `openSpaceId`；`UpdateCard` 只打 `PUT /v1.0/card/instances`、从不打 createAndDeliver）；
     `TestCardStreamingChunksAtMost1KB`（3 KB 含中文的 markdown → 每个 PUT 的 `content` ≤1024 字节且是合法 UTF-8、拼起来逐字等于原文、只有第一块 `isFull=true`、每块 `guid` 不同）。
     变异：去掉切块整段发 → 后者红。
7. **卡片回调 Stop → CARD_ACTION，2 秒内 ACK**（`callback.go`）：`topic = /v1.0/card/instances/callback` 的帧**解析完立刻 ACK**，再另起 goroutine 把事件交给 sink（慢 sink 不许拖住 ACK；
   交付失败打 Error 日志）。`params.action == "stop"` → 事件字段：`Kind CARD_ACTION`、`Platform "dingtalk"`、`TenantId = opts.TenantID`（空 → `default`）、
   `CardAction{CardId: outTrackId, Action: STOP, TaskId: params.task_id（空 → nil）}`；`ChatId` / `ChatType` / `WorkspaceId` 从第 6 项的 `outTrackId → {chatID, chatType, corpId}` 映射取
   （取不到留零值，core R3 按 task_id 查，同 feishu normalize.go:606-607）；**`Anchor{Platform:"dingtalk", ChatId, MessageId: outTrackId, ThreadId: nil}` 必填**
   （照 feishu normalize.go:602-608；`events.pb.go:537` 写明 Anchor「必填；缺失视为非法事件（core 拒收，INVALID_ARGUMENT）」）；
   `SenderId = userId`、`SenderKind HUMAN`、`Mentioned true`（照 feishu :595-601）、`EventId = 帧 headers.messageId`、`OccurredAt` 用注入时钟；
   别的动作 → 不产生事件但照样 ACK（照 feishu :561-569）。
   机器人消息帧则是「sink 返回后再 ACK」，但走第 2 项的串行 dispatcher、**不在读循环里**：sink 出错 → 非 200 ACK（对齐 feishu :311-312 让平台重推的意图；钉钉会不会真重推写进「待 H9 核实」）。
   EVENT 帧（`topic *`）→ ACK SUCCESS，Debug 日志，不上送。
   - 测试：`TestCardCallbackAckedWithin2sWhileSinkBlocks`（sink 阻塞在一个 channel 上；断言假服务端**在放开 sink 之前**就收到 ACK、且耗时 < 2 s；放开后 sink 收到的事件字段逐字，
     含 `Anchor` 非 nil 且 `Anchor.MessageId == outTrackId`；未知动作 → 有 ACK、无事件；
     子用例：先推一帧机器人消息、让它的 sink 阻塞住，再推一帧卡片回调 → 卡片回调照样 < 2 s 收到 ACK）。
     变异：把 ACK 挪到 `HandleEvent` 之后 → 红；把机器人消息改成在读循环里同步调 sink → 子用例红。这条在 `-race -count=3` 下也要稳。
8. **未实现方法**：`ReadHistory`、`ReadDocument`（原卡明写）→ `aiteerr.ErrNotImplemented`；`SendFile`、`AddReaction` 原卡没列 → 同样 `ErrNotImplemented`，
   记账转给 DD11（core 对 ack 表情失败只打 warn：plane.rs:1044-1052）。测试：`TestUnimplementedMethodsMapToUnimplemented`（四个方法经 `aiteerr.ToStatus` 都是 `codes.Unimplemented`）。
9. **`docs/p1/dingtalk.md`**（新建）：状态（未接线，DD8 接）；§1 协议表 + 每条的来源（卡片 / SDK 读码 / 推断）；归一化字段表；锚点规则（Crockford、先本文后引用、`thread_id` 只是提示）；
   出站路由与「webhook 无消息 id」；AI 卡片模板须知；能力值表（标出待核实的）；未实现清单；「待 H9 真机核实」清单（`repliedMsg` 真实形状、`msgId` 能否对上 `processQueryKey` / `outTrackId` / 原消息 `msgId`、
   群里 `text.content` 带不带 `@名`、失败 ACK 会不会重推、卡片回调实测时延、卡片实例 `PUT /v1.0/card/instances` 能更新多久（决定 `CardEditWindowSec`））；
   末尾留一张空的「H9 探针结果（DD11 按 `review/p1/ledger/H9-dingtalk-probe.md` 填）」表 —— 总管不直接改这份 docs。

## 6. 规则

- **可写面 / 只读面**见 §3；需要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。
- **B8 不变量**（`evals/p0` 钉死，弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；
  `!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 一个字都不许改。本包不接线，`passed 10/10` 必须逐字不变。
- **守卫**：被拦就停（开场自检第 2 步那次除外：那次被拦就是通过）、拦截原文进回执、不许换写法绕。云端命令里永不出现：`AITE_RELOCK=1`、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`；多行脚本写成文件再跑（跨行引号会被判「无法解析」）；check.sh 不接 `| tail`。
  守卫也扫 heredoc 正文和命令参数，而回执要逐字贴的拦截原文里就有 `guard_bash.py` 路径：多行脚本、PR 描述、回执、多行提交信息**一律先用 Write 工具落文件再用**
  （`python3 <文件>` / `gh pr create --draft --title "CC9: 钉钉 Stream 适配器包（不接线）" --body-file /tmp/cc9-pr.md` / `gh pr edit --body-file <文件>` / `git commit -F <文件>`；
  命令行 `-m` 只写单行、且不带上面那些路径）；不走 Bash heredoc / `echo >`、不传多行 `--body "…"` —— 被拦了按上条就得停。
- **Go 依赖**：只 import 标准库、`aite/edge/gen/aitepb`、`aite/edge/internal/aiteerr`、`aite/edge/internal/server`（仅测试里做接口断言）、`google.golang.org/grpc/codes` + `google.golang.org/grpc/status`（仅测试里判 `codes.Unimplemented`）、
  `google.golang.org/protobuf/...`、`github.com/gorilla/websocket`（这些都已在依赖表里）。
  **永不跑 `go get` / `go mod tidy`**；缺依赖 → 停下报告。不 import `edge/internal/feishu`。
- **每个行为改动**：回归测试 + 变异验证（撤回改动 → 测试红 → 贴输出）。本轨文件全是新建的：**变异前先把被变异文件提交（或至少 `git add`）**，
  再改、跑红、`git checkout -- <文件>` 还原（没进索引的新文件 `git checkout` 还原不了，`git stash` 默认也不带未跟踪文件）；**别用 `cp -p` / `shutil.copy2`**。
- **格式化**：`gofmt -w <改过的文件>`；本轨不碰 Rust（真要碰也只许 `rustfmt --edition 2024 <文件>`，别用 `cargo fmt --all`）。
- **新第三方依赖、R0 文件（不在你可写面里的）、锁定面** → 停下报告。
- Docker 测试封闭（只连本地测试服务器；本轨用不到 Docker）。云端 protoc 生成的 `edge/gen` 永不提交（本轨不跑 `make proto-gen`）。
- **密钥**：云端没有钉钉凭证，也不许放；测试里的 clientId / secret / token 一律假值。
- **已知时序抖动**（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、
  `aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。你自己的 2 秒测试不在豁免名单里：抖就改测试的等待方式，别放宽断言。

## 7. 验收（命令 + 期望输出）

（写成列表而不是表格：命令里有 `|`，放进 Markdown 表格会被转义。）

1. `cd edge && go test -race ./internal/dingtalk/... -count=1`
   → `ok  aite/edge/internal/dingtalk`（假网关 + 假 ws：open → connect → CALLBACK → ACK 回显；ticket 失效重连；夹具 text / richText / picture / quote；卡片流式块 ≤1KB；回调 2 秒内 ACK）。
2. `cd edge && go test -race ./internal/dingtalk/... -count=3 -v -run 'TestStreamOpenConnectCallbackAckEchoesMessageID|TestReconnectReopensWithFreshTicket|TestNormalizeFixtures|TestAnchorTaskNoFromOwnTextThenQuote|TestCardStreamingChunksAtMost1KB|TestCardCallbackAckedWithin2sWhileSinkBlocks'`
   → 6 个名字各 3 次 `--- PASS`，0 个 `FAIL`（原卡五个场景 + 锚点，连跑 3 遍查抖动）。
3. `cd edge && go test -race ./internal/dingtalk/... -count=1 -v 2>&1 | grep -c '^--- PASS'`
   → ≥ 13（§5 点名的 13 个 `Test*`；子测试的 `--- PASS` 是缩进开头，不计入）。
4. `cd edge && go vet ./... && gofmt -l . | wc -l` → exit 0，输出 `0`。
5. `cd edge && go test -race ./... -count=1 > /tmp/cc9-go.txt 2>&1; grep -c '^ok' /tmp/cc9-go.txt; grep -cE '^(ok|\?)' /tmp/cc9-go.txt; grep -c '^FAIL' /tmp/cc9-go.txt`
   → 三个数依次是 **N0 + 1**、`10`、`0`（全量 Go 只跑这一遍；最后那个 grep 打印 0 时退出码是 1，属正常）。
   原卡写「10 = 9 + 本包」指的是 `^ok` 行数；按源码静态数 `^ok` 是 7 → 8，两个数都进回执。
6. `scripts/check.sh` → 末行「全部通过」、exit 0；`cargo passed=<897 或 901>+0 failed=0`（本轨 Δ = 0，不碰 Rust）、`contracts passed=<25 或 27> failed=0`、`OK 25 files`、`passed 10/10`；
   Go 那格 8 行无 `FAIL`，另单跑 `cd edge && go test -race ./cmd/... -count=1` → `ok`。基线数按开场自检第 1 步的情形取（A：897 / 25；B：901 / 27）。**别接 `| tail`**。
7. `git diff --name-only origin/main...HEAD` → 每一行都以 `edge/internal/dingtalk/` 开头，或正好是 `docs/p1/dingtalk.md` / `review/p1/ledger/CC9.md`。
   原卡这条里「reviewer 另确认 Go 模块文件没改」是**总管审 PR 时看**的事，你别在命令里 grep / diff 那两个文件。
8. `grep -rn '"aite/edge/internal/feishu"' edge/internal/dingtalk` → 无输出（注释里写「照 feishu/platform.go:…」没问题，只禁 import）；`ls edge/internal/dingtalk` → 没有 `stream_card.go`、`stream_card_test.go`、`dws.go`、`dws_test.go`。
9. `git status --short` → 空（临时脚本、探针都清掉）。

## 8. 回执（写 `review/p1/ledger/CC9.md`，PR 描述贴摘要；两者都先 Write 成文件，PR 用 `--body-file`）

回执**只用 Write 工具**写（它要逐字贴守卫拦截原文，里面有受保护路径，Bash heredoc / `echo` 会被拦）；PR 描述用 `gh pr edit --body-file review/p1/ledger/CC9.md`
（或先 Write 一份摘要文件再 `--body-file` 它）同步，见 §3、§6。


1. **开场自检原文**（4 + 1 项）：第 1 步 diff 输出与判定（A / B）；守卫拦截原文（逐字）；三条工具链版本；check.sh 各行原样；N0、`^(ok|\?)` 行数、`^FAIL` 行数。
2. **工作项逐条**：1–9 每项新建 / 改了哪些 `文件:行`；能力值表（哪些是查到的、哪些是保守取值）；流式切块口径；ACK 时序（哪类帧先 ACK、哪类帧等 sink）。
3. **新增测试逐条 + 变异验证输出**：13 个 `Test*` 各钉什么；变异怎么做的；红的那段输出逐字贴。
4. **check.sh 完整输出**（原样，不截）+ 单跑 `cmd/aite-edge` 的输出。
5. **`cargo passed` 增量逐条**：Δ = 0（本轨只动 Go 与文档）；Go 侧写 `^ok` 从 N0 → N0+1 的那一行（`ok  aite/edge/internal/dingtalk`）。
6. **被守卫拦过的命令与拦截原文**（开场自检第 2 步那条除外，已在第 1 条；没有就写「无」）。
7. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）。至少考虑：接线 + `config` 放行 `dingtalk`（DD8）；能力值对齐 `dingtalk_v1()`、`NormalizedEvent.quote` 用 `ParseQuote` 填、
   Approve / Reject 回调、@ 人、`SendFile`、`AddReaction`、file / audio / video 消息归一化、单聊主动发（DD11）；增量流式（FF3）；客户端保活 ping（DD11）；可见 `#A` 前缀（DD3 / DD5）。
8. **没做的与原因**（包括所有只在假件上验过、等 H9 真机核实的点）。
9. **契约缺口**（给 T0 / T0.1）：只报 T0 已计划内容（`DingtalkConfig`、`PlatformChoice::Dingtalk`、`dingtalk_v1()` 与 14 个新能力位（proto 字段 10–23）、`Quote` / `NormalizedEvent.quote`、
   `CardActionKind` 的 Approve / Reject / Submit、`Anchor.task_no` 的新文档串、`OutboundText.mentions` / `dedupe_key`、`OAuth*`、
   PlatformPort / edge.proto 的 `write_doc` / `WriteDoc` 与 09-25 修订新增的 `delete_doc` / `DeleteDoc`；全表以总计划 §5.2 为准）**之外**的缺口；每条写清需要什么形状、为什么 `NormalizedEvent.raw` / `CardAction.value` 这类开放通道绕不过去。没有就写「无」。
