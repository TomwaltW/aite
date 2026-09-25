# 派单 CC10：企微智能机器人长连接适配器包（不接线）（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC10.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC10）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

**对应 Claude Tag 的哪几条**：NEW25（Claude Tag 只有 Slack，Aite 的对等扩展是钉钉 / 企微，plan:194）、CT01（群里任何人 @ 即起任务，企微走 `aibot_msg_callback`，plan:135）、
CT02（话题 = 会话；企微没有话题 → 可见 `#A..` 锚点，plan:136）、CT13（长任务清单就地更新；企微 stream 10 分钟内必须结束，只能降级，plan:147）。

**Aite 今天的样子（`98e4460`）**：

- edge 只有飞书一个平台实现（`edge/internal/` 下只有 aiteerr / config / feishu / ingress / pin / sandbox / server）。`edge/cmd/aite-edge/main.go:86-91` 只调 `feishu.New`（本派单里 main.go 的行号都按 `98e4460`；情形 B 下 AA4 改过 main.go 注释，行号可能漂，按内容定位）；
  `edge/internal/config/config.go:94-99` 除 `feishu` / `fake` 一律拒（`config_test.go:47` 钉着「`platform: dingtalk` 必须失败」）；core 侧 `contracts/src/config.rs:149-151` 的 `PlatformChoice` 只有 Feishu / Fake。
- 端口形状：`edge/internal/server/ports.go:16-26` 的 `PlatformPort`（9 个方法）。飞书的生命周期 = `feishu.New(cfg, Options, EventSink)`（`feishu/platform.go:197-200`）
  + `Start(ctx) error` / `Connected() bool` / `ReconnectCount() int64`（:240-305）；`EventSink` 在 :75-78。`main.go:127-141` 把「没收到停机信号 `Start` 却返回了（哪怕返回 nil）」当组件故障 —— **你的 `Start` 同样只能在 ctx 取消时返回**。
- 锚点：`Anchor.task_no`（`proto/aite/v1/events.proto:55`，注释「P0 只展示不路由」）今天没人填、没人读；core 的 R6 只认「群 + 已有会话」（`control/src/plane.rs:538-543`），R7 是 @ 起新会话（:545-550）。
  任务号来自 `encode_task_no`（`contracts/src/session.rs:168-189`）：`#A` + Crockford base32，字母表 `0123456789ABCDEFGHJKMNPQRSTVWXYZ`（**没有 I L O U**；1→`#A1`、17→`#AH`、1000→`#AZ8`）；
  core 的 `normalize_task_no` 把小写归成大写（`control/src/commands.rs:23-34`）。文档里常写的「#A17」只是示意。
- 依赖：`gorilla/websocket` 与 `golang.org/x/time/rate` 早已钉进模块（`edge/internal/pin/pin.go:9`、`:14`）。企微**没有官方 Go SDK**（plan:121），所以客户端手写，
  **Go 模块文件一个字不动**（plan §7：`pin/**`、`go.mod`、`go.sum` 没人改，plan:621）。

**企微智能机器人的平台事实**（plan §1.3 plan:120-122、§2C plan:199/201/210/211、NEW04 plan:173、NEW22 plan:191）：每个 bot 只允许一条长连接，新连接踢旧连接 → 只能主备；
引用消息**不带 msgid** → 锚点只能靠可见文本 `#A..`；stream 10 分钟内必须结束；主动发送只能发给给 bot 发过消息的会话，每会话 30 条/分钟、1000 条/小时（plan:210：回复也算在内）；群里 bot 收不到文件；
下载链接 5 分钟有效、要 AES-256-CBC 解密；`enter_chat` 只用于单聊欢迎（5 秒内）；成员 `userid` 除非 bot 由超级管理员创建否则是加密的（plan:122，H9 建 bot 时处理；
`CorpIDEnv` / `AppSecretEnv` 是「另配自建应用换明文」的占位，本轨不实现换取，`SenderId` 原样填回调里的 `from.userid`）。

**谁接你的东西**：DD8（W2）在 `main.go` 做平台工厂，按生命周期接口（Start / Connected / ReconnectCount）选 feishu|dingtalk|wecom|fake，并把配置段映射成你的 `Options`（环境变量也在那里读）；
DD11（W2）在你的包上补：能力值对契约 `wecom_v1()` 的表测试、`template_card_event` 的 Approve / Reject / Submit 在 5 秒内 `aibot_respond_update_msg`、`feedback_event`（负面）→ `EventKind::Reaction{thumbs_down}`
（**用你解析出的内部结构体**）、每用户 3 条在途 / 24 小时窗口；企微的 `NormalizedEvent.quote`（T0 加的 proto 19，EE13 读它）由谁填 **DD11 卡未明写**，它在 W2 拥有 wecom 包，建议归 DD11；DD3（W2）按你填的 `Anchor.task_no` 路由；
FF3（W4）的可写面**写死了** `edge/internal/wecom/stream.go` 与 `stream_test.go` —— 所以流式逻辑必须放在这两个文件里。同波 CC9 做钉钉同形状的包（两包不共享代码）。

## 2. 必读（按顺序）

1. 仓库根 `CLAUDE.md`（云端唯一能读到的约定；与本派单冲突时以本派单为准）。
2. 总计划：§1.3（plan:111-122，企微那条在 :120-122）、§2 的 CT01 / CT02 / CT13 / NEW04 / NEW22 / NEW25 行（plan:135、136、147、173、191、194）、§2C（plan:199、201、210、211）、
   §4.4 开场自检（plan:376-394）、§6 每波规则（plan:500-512）、§6.1 CC10 行（plan:530）、§7 R0 归属（plan:604-633，`pin` / Go 模块文件那行在 :621）、§9（plan:717-727）、§10 钉钉/企微风险行（plan:739）。
3. `review/p1/tracks-2026-09-25.json`：CC10 原卡；再看 DD8 / DD11 / FF3 三张卡，**知道哪些不是你的**。
4. 照着抄形状的飞书代码（只读）：`feishu/platform.go` :75-87（`EventSink`、`Options`）、:127-200（`newPlatform` 可注入面 + `New`）、:218-227（`Capabilities` 返回副本）、:240-305（`Start` 重连循环）、
   :313-339（`dispatchRaw`：归一化为 nil 就丢、1 s 预算告警、错误往上抛）；`feishu/connection.go:31-69`（`Connection` 接口、`backoffDelay` 1,2,4,8,16,30）、:71-93（令牌不进 raw）；
   `feishu/ratelimit.go:15-34`（`clockFunc` / `sleeperFunc` 注入）；`feishu/normalize.go:440-513`（消息归一化）、:549-619（卡片动作：认不出的返回 nil）；
   `feishu/card_frames_test.go:70-157`（httptest + gorilla `Upgrader` 的假 ws 服务端）；`feishu/reconnect_test.go:176-350`（退避 / Connected / ReconnectCount 的测法）。
5. 契约类型（`proto/…` 用 **Read 工具**读）：`events.proto:50-56`（Anchor）、:74-93（NormalizedEvent）；`capabilities.proto:10-20`；
   `edge/gen/aitepb/outbound.pb.go:84-91`（CardStatus）、:327-337（ChecklistCard，`TaskNo` 在 :330）；`edge/internal/aiteerr/errors.go:16-26`、:72-73（`ErrNotImplemented`）、:97-114（错误 → gRPC 码）。
6. `edge/internal/ingress/client.go:186`（`HandleEvent` —— 它就是你本地 `EventSink` 的实现方）。

## 3. 工作区

- **分支**：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC10: 企微 aibot 长连接适配器包（不接线）」。
  PR 描述先用 **Write 工具**写成 `/tmp/cc10-pr.md`（此时回执还不存在），再 `gh pr create --draft --title "CC10: 企微 aibot 长连接适配器包（不接线）" --body-file /tmp/cc10-pr.md`；
  收尾时 `gh pr edit --body-file review/p1/ledger/CC10.md`（或先 Write 一份摘要文件再 `--body-file` 它）。**永远别**把多行正文塞进 `--body "…"` 或 heredoc（见 §6 守卫）。
- **代码基线**：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**（全部新建）：
  - `edge/internal/wecom/**`。建议布局：`platform.go`（`Config` / `Options` / `EventSink` / `Platform` / `New` / 生命周期 / 端口方法）、`protocol.go`（命令名、帧结构体，**协议假设全集中在这里**）、
    `client.go`（ws 客户端）、`normalize.go`、`stream.go`（**文件名别改，FF3 接**）、`media.go`、`ratelimit.go`；测试同前缀 `*_test.go` + `testdata/`。
  - `docs/p1/wecom.md`（`docs/p1/` 目录今天不存在；T0 / CC1 / CC9 / CC11 也各在里面新建自己的文件，互不相碰）。
  - `review/p1/ledger/CC10.md`。
- **只读面**：其余一切。特别点名：
  - P0-CLOSE 文件（总管本波期间本机打；14 个路径逐个列在 §4 第 1 步）：`proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`、
    `core/crates/contracts/src/{evidence,lib}.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、`core/crates/evidence/**`、
    `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`、`.contracts.lock`、`.claude/**`、`core/crates/app/tests/guard.rs`。
  - T0 补丁文件：`config/aite.example.yaml`、`edge/internal/server/server.go`、`core/crates/proto/**`（外加锁定面 `proto/aite/v1/*.proto`、`core/crates/contracts/**`）。
  - 离你最近的别轨面：CC9 的 `edge/internal/dingtalk/**` 与 `docs/p1/dingtalk.md`；CC8 的 `edge/internal/feishu/**`；CC11 的 `edge/internal/egress/**`；
    本波没人拥有、DD8 在 W2 才动的 `edge/internal/{server,config,aiteerr,ingress}/**`、`edge/cmd/aite-edge/**`；`edge/internal/pin/pin.go` 与 Go 模块文件（永远没人改）；`edge/gen/**`（只有总管本机生成）。
- **本轨解冻的冻结项**：无（§9 没给 CC10 列解冻项）。`config_test.go:47` 的「dingtalk 被拒」由 DD8 翻，不是你的。

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
   - 别的 → 停下，写回执。（D0 的改动全是删除或落在排除的路径里，所以情形 A 输出为空；`--no-renames` 让「挪文件」也现形。）回执里写明是 A 还是 B。
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（期望形如「blocked: 该操作触碰受保护面 …」；拦截原文逐字贴进回执）。
   **这一步被拦就是通过**：拦截原文末尾的「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步。
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**（冷编译 10–15 分钟，耐心等）。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认 → `ok`。
   若 CC1 已合并、check.sh 多打一行 `go packages ok=N fail=M`，以那行为准。
5. **本轨附加**：
   - 「Go 9 包」的口径 = 7 行 `ok` + 2 行 `? … [no test files]`（`gen/aitepb`、`internal/pin`）：`(cd edge && go test -race ./... -count=1 2>&1) | grep -cE '^(ok|\?)'` → `9`，
     `… | grep -c '^ok'` → `7`（按源码里的 `*_test.go` 数的，本机没实跑 —— 对不上先贴原输出再判断）。
   - 两个依赖已钉，不另跑任何读模块元数据的命令：`edge/internal/pin/pin.go:9`、`:14` 的空导入 + 第 4 步 check.sh 里 Go 那格编得过，已足以证明；你只 import、不改模块文件。

## 5. 工作项

**总纪律**：只用标准库 + 已钉的 `github.com/gorilla/websocket`（`golang.org/x/time/rate` 也已钉、可以 import，但第 7 项的限速不用它）+ 仓库内 `aite/edge/gen/aitepb`、`aite/edge/internal/aiteerr`、`aite/edge/internal/server`（只为编译期断言）、
已在用的 `google.golang.org/protobuf`。**永远不跑 `go get` / `go mod tidy`**；`go build` 若报缺 go.sum 条目 → 停下报告。非测试代码不 import `internal/feishu`（要的小工具抄过来）。
包**不接线**：没有任何现有文件 import 它。所有测试封闭：只连 `httptest` 起的 `ws://127.0.0.1…` / `http://127.0.0.1…`，**永不拨** `openws.work.weixin.qq.com`（每个测试都显式覆盖 `WSURL`）。
所有时间相关的断言走注入时钟 / 注入 sleep（照 `ratelimit.go:15-34`），不靠真睡。

**协议参考**（命令名、事件名来自原卡，权威；**其余字段名是派单作者按公开资料整理的参考，未对真机核实**。云端环境的网络是 Trusted + 4 个自定义域名（plan:279-281），
`developer.work.weixin.qq.com` 大概率打不开 —— 能打开就先核对文档 101463；打不开就照下表实现，全部集中在 `protocol.go`，在 `docs/p1/wecom.md` 逐条标 `TODO(真机核对)`）：

| 用途 | cmd | 方向 | body 要点（参考） |
|---|---|---|---|
| 订阅 / 鉴权 | `aibot_subscribe` | → | `bot_id`、`secret`；应答 `errcode == 0` 才算连上 |
| 心跳 | `ping` | → | 每 30 秒一帧 |
| 消息回调 | `aibot_msg_callback` | ← | `msgid`、`aibotid`、`chatid`、`chattype`（`single` / `group`）、`from.userid`、`msgtype`、`text.content`、`mixed.msg_item[]`、`image` / `file` 的 `url` + `aeskey`、`quote`（**无 msgid**） |
| 事件回调 | `aibot_event_callback` | ← | `event.eventtype` ∈ `enter_chat` / `template_card_event` / `feedback_event` / `disconnected_event` |
| 流式回复 | `aibot_respond_msg` | → | `headers.req_id` = 所回复的那条回调帧的 `req_id`；`msgtype: stream`，`stream{id, finish, content, feedback{id}}` |
| 欢迎语 | `aibot_respond_welcome_msg` | → | 同上 `req_id`，5 秒内 |
| 更新模板卡片 | `aibot_respond_update_msg` | → | 同上 `req_id`，5 秒内 |
| 主动发送 | `aibot_send_msg` | → | `chatid` + 消息体 |
| 分片上传 | **卡片没给命令名，别编** | → | 512 KB / 片、最多 100 片；命令名用带 `TODO(真机核对)` 注释的常量占位，测试不钉命令名 |

帧外形（参考）：`{"cmd": …, "headers": {"req_id": …}, "body": {…}}`；服务端应答 `{"headers": {"req_id": …}, "errcode": 0, "errmsg": "ok"}`。同一 `req_id` 上的多帧（流式刷新）按发送顺序 FIFO 对应答。

1. **包骨架与生命周期**（`platform.go`）。
   - 本地 `Config`（**字段名照 T0 的 `WecomConfig`**，DD8 好机械映射）：`BotIDEnv "WECOM_BOT_ID"`、`BotSecretEnv "WECOM_BOT_SECRET"`、`WSURL "wss://openws.work.weixin.qq.com"`、`CorpIDEnv ""`、`AppSecretEnv ""`、`BotName "Aite"`，
     yaml tag 用 snake_case（`bot_id_env` …），`DefaultConfig()` 返回这组值。`edge/internal/config` 是 DD8 的 R0 文件，**别去那里加 `Wecom` 段**。包内不读环境变量（DD8 在 main.go 读，照 main.go:86-91）。
   - `Options{BotID, BotSecret, TenantID, WSURL, WelcomeText}`（`TenantID` 空 → `"default"`，`WSURL` 空 → 用 cfg 的）；`EventSink` 照 platform.go:76-78 在本包重声明一份（`ingress.Client` 结构性满足）。
   - `func New(cfg Config, opts Options, sink EventSink) (*Platform, error)`：只装配、不建连（照 platform.go:197-200）；内部 `platformOptions` 放可注入面（dialer / clock / sleep / pingEvery / logger）。
   - `Start(ctx) error`：建连 → 订阅 → 读循环；断线按 1,2,4,8,16,30 秒退避无限重连（从 connection.go:56-69 抄一份），**只有 ctx 取消能让它返回**；`Connected() bool`、`ReconnectCount() int64` 语义同飞书。
   - `Capabilities()` 返回**副本**（照 :218-227），值只用今天 pb 的 9 个字段，来源分三类（回执能力值表照此标注）：
     - 与 T0 `wecom_v1()` 一致：`supports_card_edit false`、`proactive_requires_prior_message true`；
     - **本轨取值**（T0 原卡没给 wecom 的这几项，依据 plan §1.3 / §2C：无话题、读不到历史、只收 @、群里收不到文件）：`platform "wecom"`、`supports_thread` / `supports_history` / `supports_passive_listen` 均 false、
       `card_edit_window_sec 0`、`inbound_file_in_group false`；
     - `outbound_rate_per_min 30`（T0 原卡也没给，取卡片的「每会话 30 条/分钟」，回执「契约缺口」里写给 T0 对齐）。
     T0 的新字段今天 pb 里没有 → 先写成包内常量，名字贴着契约字段，DD11 逐个对到 `wecom_v1()`：`stream_max_sec 600`、`card_action_deadline_ms 5000`、`max_upload_bytes 20 MB`、
     `reactions_in true`、`requires_visible_anchor true`。
   - 编译期断言 `var _ server.PlatformPort = (*Platform)(nil)`。新测试 `TestPlatformShapeMatchesFeishu`：测试里声明 `interface{ Start(context.Context) error; Connected() bool; ReconnectCount() int64 }`，
     `(*feishu.Platform)(nil)` 与 `(*Platform)(nil)` 都赋得进去（测试文件 import feishu 可以），再断言 `Capabilities()` 的 9 个值与改返回值后调用方拿不到本体。变异：把 `ReconnectCount` 返回类型改成 `int` → 编译红。
2. **ws 客户端**（`client.go` + `protocol.go`，这两份合计约 500 行）。gorilla 的 `Conn` 只允许一个并发写者 → 写操作一把锁；读循环按 `cmd` 分发；出站命令等同 `req_id` 的应答（带 ctx 超时），`errcode != 0` →
   `*aiteerr.PlatformError{Code: 十进制 errcode, Retryable: 按码判, Msg: errmsg}`。心跳常量 `pingInterval = 30 * time.Second`（注入面可改短），两个周期收不到任何帧 → 判断线、走重连。
   `secret` / `aeskey` 永不进日志（帧日志只打 cmd + req_id）。
   新测试：`TestSubscribePingAndCallback`（假 ws：第一帧必须是 `aibot_subscribe` 且带 `bot_id` / `secret`；应答后收到 ≥2 帧 `ping`；服务端推一条 `aibot_msg_callback` → sink 收到 1 个事件；另断言默认间隔常量 == 30 s）；
   `TestSubscribeRejectedBacksOff`（订阅应答 `errcode != 0` → 不算连上、`Connected()==false`，注入 sleep 记到 1 s 退避后再拨）；`TestReconnectAfterDropCounts`（服务端断开 → 重连成功 → `ReconnectCount()==1`）。
   变异：删掉 ping 定时器 / 订阅帧顺序对调 / 重连成功不加计数 → 各自红。
3. **`disconnected_event` → 备用**（一个 bot 只许一条连接：新连接会踢旧连接）。收到它：`Connected()` 置 false、`Standby()` 返回 true、Warn 日志 `wecom.standby`，**不再重连**（重连会把主机踢下线，
   两台互踢）；`Start` 继续阻塞到 ctx 取消再返回 nil（否则 main.go:137-140 会把它当组件故障）。备机怎么接管（重启进程 / 以后的管理动作）→ 记账转给 DD8，写进 `wecom.md`。
   新测试：`TestDisconnectedEventEntersStandby`（服务端推 `disconnected_event` 后断开；注入的 sleep / 时钟推进远超 30 s 封顶退避，假服务端**一次重拨都没收到**；ctx 取消后 `Start` 返回 nil）。
   变异：让 `disconnected_event` 落回普通断线分支 → 假服务端收到重拨 → 红。
4. **入站归一化**（`normalize.go`，照 feishu/normalize.go:440-513 的字段口径）：`aibot_msg_callback` → `EVENT_KIND_MESSAGE`、`platform "wecom"`、`SenderKind HUMAN`、`SenderId = from.userid`、
   `WorkspaceId = aibotid`（空则 `Options.BotID`）、`EventId = msgid`（空则回调 `req_id`）、`chattype single → P2P / group → GROUP`、`OccurredAt` = 收到时刻（注入时钟）。
   - **`Anchor` 必填**（`events.proto:88`：缺了 core 拒收）：`Anchor{Platform "wecom", ChatId = chatid, MessageId = msgid（空则回调 req_id，与 EventId 同值）, ThreadId nil, TaskNo 见下}`。
     core 的 R7 拿 `ev.anchor.message_id` 当新会话的 thread 键（`control/src/plane.rs:547`），之后又当 `SendCard` 的 `replyTo` 传回来 —— 所以第 6 项「replyTo → req_id」表的键**就是这个 `MessageId` 值**。
   - **群消息只认 @ 的 text / mixed / quote**：群里其它 `msgtype` → 返回 nil + debug 日志（照 platform.go:319-323 的丢法）。群消息 `Mentioned = true`、`Text` 去掉开头的 `@<BotName>` 再 trim，`RawText` 存原文；
     单聊 `Mentioned = false`（与飞书 p2p 同口径），单聊认 text / mixed / image / file，其余 debug 丢。mixed 里的文字按顺序拼接、图片进 `Attachments`。
   - **`#A` 锚点**：正则 `(?i)#A([0-9A-HJKMNP-TV-Z]+)\b`，取到后转大写；**先看本条正文，再看 `quote` 的文字内容**（quote 的 text / mixed 文字），命中写 `Anchor.task_no`；`Anchor.ThreadId` 恒 nil（没有 msgid 可填）。
     quote 本身解析进包内结构体，原样留在 `Raw` 里；T0 之后填 `NormalizedEvent.quote` 不是本轨的（DD11 卡未明写，它在 W2 拥有 wecom 包，建议归 DD11，回执「记账转出去的」照此写）。
   - **`Raw` 脱敏**：进 `NormalizedEvent.Raw` 之前删掉所有 `aeskey`（以及下载 `url`）—— raw 会落进审计 / 证据（照 connection.go:71-75 不放 token 的理由）。
   新测试：`TestGroupOnlyTextMixedQuote`（群 text / mixed / 带 quote 的 text 各出 1 个事件；群 image / file / voice 出 nil；单聊 image 出带附件的事件）；
   `TestQuoteTaskNoGoesToAnchor`（quote 里 `…进度见 #AH…` → `task_no "#AH"`、`ThreadId == nil`；正文 `#A1` + quote `#AH` → `"#A1"`；`#ah` → `"#AH"`；`#AI` → nil）；
   `TestRawRedactsAESKey`（`Raw` 序列化后不含夹具里的 aeskey 串）。变异：quote 分支删掉 / 先看 quote 后看正文 / 去掉脱敏 → 各自红。
5. **事件回调**：
   - `enter_chat`：不送 core（它是单聊欢迎，不是入群，别映射成 `BOT_ADDED`，plan:191）；`Options.WelcomeText` 非空时 5 秒内用 `aibot_respond_welcome_msg` 回；空就只打 debug。
   - `template_card_event`：按钮 key 是 `stop` / `evidence` → `EVENT_KIND_CARD_ACTION`（`CardActionKind` 今天只有这两值），其余返回 nil（照 normalize.go:561-569）；`CardAction.TaskId` 取按钮负载里的 `task_id`（有才填，照 :578-582，DD11 渲染模板卡片时会放进去）；记下回调 `req_id` 与收到时刻。
     提供 `RespondTemplateCardUpdate(ctx, reqID string, card map[string]any) error`：距回调收到 ≤ 5 s 才发 `aibot_respond_update_msg`，超时返回 `PlatformError{Code: "deadline_exceeded", Retryable: false}` 且一帧不发。
     Approve / Reject / Submit 与「谁来在 5 秒内调它」是 DD11 的。
   - `feedback_event`：解析成包内结构体 `Feedback{FeedbackID, Kind(like / dislike / cancel), Text, ReasonCodes, ChatID, UserID, At}`，**不送 core**（今天没有 `EventKind::Reaction`），debug 日志后丢。
     `FeedbackID` 能对回 Aite 的消息，靠第 6 项开流时把 `stream.feedback.id` 设成你返回给 core 的 `MessageId`。
   新测试：`TestEnterChatWelcomeWithin5s`（有 WelcomeText → 假服务端收到一帧 welcome，`req_id` 对得上；无 → 一帧不发；sink 零事件）；
   `TestTemplateCardEventAndUpdateDeadline5s`（stop 按钮 → CARD_ACTION；注入时钟 4.9 s 发得出、5.1 s 返回错误且无帧）；`TestFeedbackEventParsed`（点踩 / 点赞 / 取消三份夹具 → 结构体逐字段对上，sink 零事件）。
6. **流式回复**（`stream.go`，FF3 接手）：一次回复 = 一个 `stream.id`。`SendCard(chatID, replyTo, card)` 以 `replyTo` 查包内有界表（键 = 第 4 项的 `Anchor.MessageId`，值 = 当时回调的 `req_id`）开流，内容 = 清单卡的 markdown 渲染（含 `TaskNo`），
   返回 `MessageId = CardId = stream.id`；`UpdateCard(cardID, card)` 在**同一个** `stream.id` 上推全量内容，`CardStatus` 为 DELIVERED / FAILED / CANCELLED 时 `finish=true`；
   `SendText` 带可回复的 `ReplyTo` → 新开一个流、一帧 `finish=true`。
   - **没有可用的 `req_id`**（`replyTo == nil`，或表里查不到：被挤出有界表 / 进程重启过）：退回 `aibot_send_msg` 发**一帧** markdown，受第 7 项「来过消息」与 30 条/分钟两道检查约束（不满足就原样返回那两个错误，一帧不发）；
     返回的 `MessageId = CardId` = `"nostream-" + 这一帧的 req_id`（本地生成，不依赖应答里有没有 msgid），前缀让 `UpdateCard` 认出它是「非流式」：对它零帧、返回 nil + Info 日志（与收尾后同口径）。
   - **9 分钟自动收尾**：常量 `streamAutoFinishAfter = 9 * time.Minute`（注释：平台 10 分钟上限留 1 分钟余量，plan:201），
     到点由**后台 sweep**（注入定时器 / 时钟）判、发 `finish=true`（不能只靠下一次 `UpdateCard`：任务第 4 分钟起不再更新，流就会冲过 10 分钟），内容 = 最后一次内容 + `"\n\n进度见 " + TaskNo`（`TaskNo` 空则不加指针）；
     收尾后的 `UpdateCard` 不再发帧、返回 nil + Info 日志。收尾后的里程碑消息是 FF3 的。

   新测试：`TestStreamAutoFinishesAt9MinWithPointer`（注入时钟：8m59s 的更新 `finish=false`；9m 时 `finish=true` 且内容以 `进度见 #AH` 结尾；之后更新零帧）；
   `TestUpdateCardReusesStreamID`（多次更新同一 `stream.id`、终态才 `finish`、开流帧带 `feedback.id`；另加一段：该会话先入站过一条、`replyTo` 查不到 → 假服务端收到恰好一帧 `aibot_send_msg`、之后对该 `CardId` 的更新零帧）。
   变异：9 → 10 分钟 / 去掉指针 / 每次更新新开 id / 查不到 `req_id` 时照样发 `aibot_respond_msg` → 红。
7. **主动发送 + 限速**（`ratelimit.go`）：`SendText` 没有可回复的上下文 → `aibot_send_msg`，**只发给来过消息的会话**（入站过的 `chatid` 集合，有界），否则 `PlatformError{Code: "proactive_not_allowed", Retryable: false}`；
   每会话**滑动窗口**：记最近 30 次发送时刻（环形，注入时钟），第 31 次时若其中最早那次距今 < 60 s 就拒 —— 任意 60 秒内最多 30 条；超了立即返回 `PlatformError{Code: "rate_limited", Retryable: true}`（映射成 UNAVAILABLE，core 会重试）。
   **别用 `x/time/rate`**：它是令牌桶（按 0.5/s、突发 30 配，头一分钟能放过约 59 条），守不住平台的「每分钟 30 条」；`capabilities.proto:19` 里 `outbound_rate_per_min` 注释写的「令牌桶」只是泛指，以本条为准。
   本轨只对主动发送（`aibot_send_msg`）计数；plan:210 说回复也算在同一预算里 —— 流式回复要不要计入、怎么计，写进 `wecom.md` 与回执「记账转出去的」（DD11 / FF3），本轨不做。
   新测试：`TestProactiveSendRequiresPriorMessage`、`TestProactiveSend30PerMinPerConversation`（第 31 条被拒、换一个会话不受影响、时钟推 60 s 后恢复）。变异：去掉「来过消息」检查 / 限额改 31 → 红。
8. **媒体**（`media.go`）：
   - 下载：入站时把 `(url, aeskey, 到期 = 收到 + 5 分钟)` 按 `(msgid, file_key)` 存进包内有界表，`Attachment.FileKey` 只放不含密钥的短 id；`DownloadFile(messageID, fileKey)` 查表 → HTTP GET → 解密。
     解密（参考，未核实）：key = base64 解码 `aeskey` —— 先 `base64.StdEncoding`，失败再 `base64.RawStdEncoding`（企微传统的 EncodingAESKey 是 43 个字符、不带 `=` 填充，严格解码会拒真 key），
     解出来必须**恰好 32 字节**，否则返回错误；IV = key 前 16 字节，AES-256-CBC（`crypto/aes` + `crypto/cipher`），去 PKCS#7 填充时接受 1..32（企微惯例填充到 32 的倍数）；
     填充非法、密文长度不是 16 的倍数 → 返回错误，**绝不 panic**。表里没有或过期 → `PlatformError{Code: "file_expired", HTTPStatus: 404, Retryable: false}`。
   - 上传：`SendFile` 分片上传，每片恰好 512 KB（最后一片可小），按序发、每片等应答，超过 100 片在**发出第一帧之前**就报错；拿到的媒体 id 再走回复或主动发送。
     **本轨 `SendFile` 只执行协议上限（≤ 100 片 × 512 KB）**；T0 的 `max_upload_bytes 20 MB` 只作第 1 项的包内常量，**不在 `SendFile` 里拦** —— 否则 20 MB（= 40 片）先拦住，100 片那道检查永远走不到，
     下面「100×512 KB+1 → 报错」钉的就不是片数上限，「片数上限」的变异也红不了。20 MB 由 DD11 对到契约后再决定在哪里拦（写进 `wecom.md` 与回执「记账转出去的」）。
   新测试：`TestAES256CBCDecryptVector`（固定向量，测试运行时不调任何外部命令；向量来源二选一，回执写明用的哪个、命令与输出原样贴：
   ① 云端有 `openssl`（先跑 `openssl version`）→ 用 `openssl enc -aes-256-cbc -nopad -K <hex> -iv <hex>` 对手工填充到 32 倍数的明文生成一次，结果写死进测试；
   ② `openssl version` 失败 → CBC 核心那步用 NIST SP 800-38A F.2.6（CBC-AES256.Decrypt）向量：Go 自带源码里就有，`go env GOROOT` 拿到根目录后读 `src/crypto/cipher/cbc_aes_test.go` 的 `CBC-AES256` 那项
   与 `src/crypto/cipher/common_test.go` 的 `commonKey256` / `commonIV`（注意它用自己的 IV，不是 key 前 16 字节），然后**另外单测**「IV = key[:16]」的推导与「按 32 块去 PKCS#7」各一条；
   另测错 key / 坏填充 / 43 字符无填充 aeskey 能解出 32 字节）；`TestChunkedUpload512KB`（1.2 MB → 3 片，前两片各 524288 字节、序号连续；100×512 KB+1 字节 → 报错且假服务端零帧）；
   `TestDownloadFileDecryptsAndExpires`（httptest 回密文 → 拿到明文；时钟推 5 分钟后 → `file_expired`）。变异：IV 改全零 / 片长改 500 KB / 不判过期 → 红。
9. **没有企微对应物的端口方法**：`ReadHistory`、`ReadDocument`、`AddReaction` → `aiteerr.ErrNotImplemented`（server 层翻成 UNIMPLEMENTED，errors.go:83-85）。core 容忍这两类失败：ack 失败只 warn（plane.rs:1044-1052），
   读历史失败记 error 后用空历史（worker/src/agent.rs:294-307）。新测试：`TestUnsupportedPortMethodsAreUnimplemented`（`errors.Is(err, aiteerr.ErrNotImplemented)`）。
10. **`docs/p1/wecom.md`**：协议参考表（每行标「原卡给定」或「`TODO(真机核对)`」）、`PlatformPort` 方法 → 企微命令映射、能力值与来源、主备 / 备用语义与接管方式、限额（哪些在本包、哪些是 DD11 / FF3）、
    密钥处理（secret / aeskey 只在 edge 内存、不进日志与 raw）、给 H9 真机冒烟的核对清单（发 @、引用 `#A..` 追问、流式更新、停止、点踩；顺带看 `from.userid` 是否明文，plan:122）。

## 6. 规则

- **可写面 / 只读面**见 §3；需要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。
- **B8 不变量**（`evals/p0` 钉死，弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；
  `!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 一个字都不许改。本包不接线，B8 走 fake 平台，理应纹丝不动。
- **守卫**：被拦就停、拦截原文进回执、不许换写法绕。云端命令里永不出现：`AITE_RELOCK=1`、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`；check.sh 不接 `| tail`。
  多行脚本、PR 描述、回执、多行提交信息**一律先用 Write 工具落文件再用**（`python3 <文件>` / `gh pr create --draft --title "CC10: 企微 aibot 长连接适配器包（不接线）" --body-file /tmp/cc10-pr.md` /
  `gh pr edit --body-file <文件>` / `git commit -F <文件>`；命令行 `-m` 只写单行）；不走 Bash heredoc / `echo >`、不传多行 `--body "…"`（跨行引号会被判「无法解析」，
  守卫也扫 heredoc 正文 —— 回执里逐字贴的拦截原文本身就带受保护路径名；一旦被拦，按本条开头就得停）。
- **每个行为改动**：回归测试 + 变异验证（撤回改动 → 测试红 → 贴输出）。本轨文件全是新建的：**变异前先提交当前实现**（提交后再改、跑红、`git checkout -- <文件>` 还原、再跑绿）——
  没提交就 checkout，未跟踪文件会报错、已跟踪文件会退回上一次提交，悄悄丢掉没提交的活。还原**别用 `cp -p` / `shutil.copy2`**。
- **格式化**：`gofmt -w <改过的 .go 文件>`（本轨没有 Rust 改动；万一有，`rustfmt --edition 2024 <文件>`，别用 `cargo fmt --all`）。
- **新第三方依赖、R0 文件（不在你可写面里的）、锁定面** → 停下报告。T0 已计划的契约字段（`NormalizedEvent.quote`、`EventKind::Reaction`、`wecom_v1()` 能力位、`WecomConfig`）不算缺口，别在本轨绕着造。
- **密钥**：云端环境没有也不许放企微的 bot_id / secret；测试一律假值。secret、aeskey 永不进日志、`Raw`、回执。
- Docker 测试封闭（本轨用不到 Docker）；所有网络测试只连 127.0.0.1 上的 httptest。云端 protoc 生成的 `edge/gen` 永不提交（本轨不跑 `make proto-gen`）。
- **已知时序抖动**（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、`aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、
  Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。你自己的新测试不许靠真睡计时，`-race` 下连跑 5 遍（`-count=5`）都得绿。

## 7. 验收（命令 + 期望输出）

卡片验收括号里的七项 ↔ 测试名（卡片没给名字，下列名字由本派单定，照抄）：subscribe/ping/callback ↔ `TestSubscribePingAndCallback`；disconnected_event -> standby ↔ `TestDisconnectedEventEntersStandby`；
stream auto-finish at 9 min with an injected clock ↔ `TestStreamAutoFinishesAt9MinWithPointer`；AES-256-CBC vector ↔ `TestAES256CBCDecryptVector`；chunked upload 512 KB ↔ `TestChunkedUpload512KB`；
quote -> #A anchor ↔ `TestQuoteTaskNoGoesToAnchor`；feedback_event parsed ↔ `TestFeedbackEventParsed`。

| # | 命令 | 期望 |
|---|---|---|
| 1 | `cd edge && go test -race ./internal/wecom/... -count=1` | `ok  aite/edge/internal/wecom` |
| 2 | 下面代码块 A（卡片点名的 7 条） | 两个数：`7`、`0` |
| 3 | 下面代码块 B（其余 12 条） | 两个数：`12`、`0` |
| 4 | `cd edge && go test -race ./internal/wecom/... -count=5` | `ok`（时序稳定性） |
| 5 | 下面代码块 C（vet + gofmt） | exit 0，输出 `0` |
| 6 | 下面代码块 D（全 edge 包计数，同一份输出数三遍） | 三个数：`10`、`8`、`0` |
| 7 | `scripts/check.sh` | 末行「全部通过」、exit 0；`cargo passed=<897 或 901> failed=0`（Δ = 0：本轨不加 Rust 测试，Go 测试不计入 cargo）、`contracts passed=<25 或 27> failed=0`、`OK 25 files`、`passed 10/10`；Go 那格 8 行 = `internal/aiteerr … internal/wecom`（7 行 `ok` + `? internal/pin`），`cmd/aite-edge` 与 `gen/aitepb` 被截掉，单跑 `cd edge && go test -race ./cmd/... -count=1` → `ok`；若 CC1 已合并则看 `go packages ok=10 fail=0` |
| 8 | `git diff --name-only origin/main...HEAD` | 每一行都以 `edge/internal/wecom/` 开头，或正好是 `docs/p1/wecom.md`、`review/p1/ledger/CC10.md` |
| 9 | `git status --short` | 空（探针、临时脚本、openssl 中间文件都清掉） |

第 2、3、5、6 行的原样命令（每条一行，逐字照敲；输出先落到仓库外的 `/tmp`，一份输出数几遍，`git status` 也保持干净）：

```bash
# A：卡片点名的 7 条 → 期望输出两行：7、0
(cd edge && go test -race ./internal/wecom/... -count=1 -v -run '^(TestSubscribePingAndCallback|TestDisconnectedEventEntersStandby|TestStreamAutoFinishesAt9MinWithPointer|TestAES256CBCDecryptVector|TestChunkedUpload512KB|TestQuoteTaskNoGoesToAnchor|TestFeedbackEventParsed)$' > /tmp/cc10-a.out 2>&1); grep -c '^--- PASS' /tmp/cc10-a.out; grep -c '^--- FAIL' /tmp/cc10-a.out
# B：其余 12 条 → 期望输出两行：12、0
(cd edge && go test -race ./internal/wecom/... -count=1 -v -run '^(TestPlatformShapeMatchesFeishu|TestSubscribeRejectedBacksOff|TestReconnectAfterDropCounts|TestGroupOnlyTextMixedQuote|TestRawRedactsAESKey|TestEnterChatWelcomeWithin5s|TestTemplateCardEventAndUpdateDeadline5s|TestUpdateCardReusesStreamID|TestProactiveSendRequiresPriorMessage|TestProactiveSend30PerMinPerConversation|TestDownloadFileDecryptsAndExpires|TestUnsupportedPortMethodsAreUnimplemented)$' > /tmp/cc10-b.out 2>&1); grep -c '^--- PASS' /tmp/cc10-b.out; grep -c '^--- FAIL' /tmp/cc10-b.out
# C：vet + gofmt → exit 0，输出 0
(cd edge && go vet ./... && gofmt -l . | wc -l)
# D：全 edge 包 → 期望输出三行：10、8、0
(cd edge && go test -race ./... -count=1 > /tmp/cc10-go.out 2>&1); grep -cE '^(ok|\?)' /tmp/cc10-go.out; grep -c '^ok' /tmp/cc10-go.out; grep -cE '^(FAIL|panic:)' /tmp/cc10-go.out
```

- 第 3 行（代码块 B）的 12 个：`TestPlatformShapeMatchesFeishu`、`TestSubscribeRejectedBacksOff`、`TestReconnectAfterDropCounts`、`TestGroupOnlyTextMixedQuote`、`TestRawRedactsAESKey`、`TestEnterChatWelcomeWithin5s`、
  `TestTemplateCardEventAndUpdateDeadline5s`、`TestUpdateCardReusesStreamID`、`TestProactiveSendRequiresPriorMessage`、`TestProactiveSend30PerMinPerConversation`、`TestDownloadFileDecryptsAndExpires`、`TestUnsupportedPortMethodsAreUnimplemented`。
  名字逐字照抄；想多加测试可以，另列、计入回执，但这 19 个一个不能少。
- 第 7 行的基线数按开场自检第 1 步判定的情形取（A：897 / 25；B：901 / 27）。**别接 `| tail`**。第 8 行的三点 diff 要合并基，靠开场自检第 1 步的 `--unshallow` 补历史。
- Go 模块文件有没有被改，由总管审 PR 时看；你别在命令里 grep 或 diff 它们。
- 人工步骤（写进回执，给总管）：DD8 + DD11 合并后用企微 API 模式智能机器人真机冒烟（H9 的账号），逐条核对 `wecom.md` 里的 `TODO(真机核对)`。

## 8. 回执（写 `review/p1/ledger/CC10.md`，PR 描述贴摘要；两者都先用 Write 工具落文件，PR 用 `--body-file`，见 §3、§6）

1. **开场自检原文**（4 + 1 项）：第 1 步 `cat-file` / `fetch --unshallow` 是否触发、diff 输出与判定（A / B）；守卫拦截原文（逐字）；三条工具链版本（`protoc-gen-go` / `protoc-gen-go-grpc` 对不上的记一笔）；check.sh 各行原样；Go 包计数（9 / 7）。
2. **工作项逐条**：1–10 每项改了哪些 `文件:行`；能力值表（每项来源：T0 `wecom_v1` / 原卡 / 本轨取值）；`Config` 字段与 T0 `WecomConfig` 的对照；备用语义的最终决定。
3. **新增测试逐条 + 变异验证输出**：每条测试钉什么；变异怎么做的；红的那段输出逐字贴。AES 向量：写明用的是 ① openssl 还是 ② NIST SP 800-38A F.2.6（Go 源码里的哪一项），命令与输出原样贴。
4. **check.sh 完整输出**（原样，不截）。
5. **`cargo passed` 增量逐条**：本轨预期 Δ = 0；不是 0 就逐条解释。另列 Go 新增测试名清单（19 条 + 你另加的）。
6. **协议核实表**：`protocol.go` 里每个命令名 / 字段名，标「原卡给定」「文档已核」或「未核实」；云端能否打开 developer.work.weixin.qq.com 写明。
7. **被守卫拦过的命令与拦截原文**（没有就写「无」）。
8. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）。至少考虑：平台工厂、环境变量读取、Go 配置镜像、放行 `platform: wecom`（DD8）；备机接管机制（DD8）；
   每用户 3 条在途、24 小时回复窗口、Approve / Reject / Submit 模板卡片、`feedback_event` → Reaction、能力值对契约表测试（DD11）；
   `NormalizedEvent.quote`（EE13 读它；DD11 卡未明写，它在 W2 拥有 wecom 包，建议归 DD11，表里写明这层不确定）；`max_upload_bytes 20 MB` 在哪里拦（DD11）；
   回复是否计入每会话 30 条/分钟的同一预算（plan:210，DD11 / FF3）；9 分钟后的里程碑消息、每小时 1000 条上限（FF3）；钉钉 / 企微两份 `#A` 正则的去重（DD11 或以后）；真机字段核对（H9 / DD11 冒烟）；
   明文 `userid`（超管建 bot，或用 `CorpIDEnv` / `AppSecretEnv` 另配自建应用换明文：H9 定做法，换取实现没有卡片认领，建议 DD11，表里写明这层不确定）。
9. **没做的与原因**（包括所有 `TODO(真机核对)`、上传命令名占位）。
10. **契约缺口**（给 T0 / T0.1）：只报 T0 已计划字段之外的；每条写清需要什么形状、为什么开放通道（`NormalizedEvent.raw`、`Session.meta` 等）绕不过去。至少判断这两条再写：
    ① `wecom_v1()` 的 `outbound_rate_per_min` 该取多少（原卡没给，你取 30 的理由）；② `EdgeStatus` 没有「备用」位，主备里的备机在 `!status` 里只能显示离线 —— 是不是缺口、值不值得进 T0。没有就写「无」。
