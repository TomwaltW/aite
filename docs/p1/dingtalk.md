# 钉钉适配器（`edge/internal/dingtalk`）

> W1 归 CC9 建，W2 起归 DD11 维护（总计划 §8 H9）。总管不直接改这份文档：H9 探针结果写进
> `review/p1/ledger/H9-dingtalk-probe.md`，DD11 再并进本文末尾的表。

## 0. 状态

- **未接线**。本包只交一个独立的 `PlatformPort` 实现 + `Start / Connected / ReconnectCount`（签名与 `*feishu.Platform` 逐字相同）。
  平台工厂、`config` 放行 `platform: dingtalk`、`DingtalkConfig` → 本包 `Config` / `Options` 的映射、EdgeStatus 读连接状态，都归 **DD8**。
- 全部行为只在假件上验过（httptest 假 REST + 假 Stream 网关 + gorilla 假 ws 服务端），**没连过真钉钉**。
  凡标「推断」「调研记忆」的，都在 §8「待 H9 真机核实」里。
- 依赖：标准库 + 已钉的 `github.com/gorilla/websocket`（`edge/internal/pin/pin.go`），没动 Go 模块文件，没 import `internal/feishu`。

## 1. Stream 协议

| 环节 | 形状 | 来源 |
|---|---|---|
| 开连接 | `POST {api_base}/v1.0/gateway/connections/open`，体 `{"clientId","clientSecret","subscriptions":[…],"ua","localIp"}` → `{"endpoint","ticket"}` | SDK 读码 |
| 订阅 | `EVENT *`、`CALLBACK /v1.0/im/bot/messages/get`、`CALLBACK /v1.0/card/instances/callback`（逐字，测试钉住） | 卡片 + SDK 读码 |
| 建 ws | 拨 `endpoint?ticket=<ticket>`；ticket 一次性，每次重连都重新 `open` | SDK 读码 |
| 下行帧 | `{"specVersion","type","time","headers":{"messageId","topic","contentType",…},"data":"<JSON 字符串>"}`，`type` ∈ SYSTEM / EVENT / CALLBACK | SDK 读码 |
| ACK | 每一帧都回 `{"code":200,"headers":{"contentType":"application/json","messageId":<原样回显>},"message":"OK","data":"…"}`；失败回 `code 500` | SDK 读码 |
| SYSTEM `ping` | 读循环里当场 ACK，data 原样回显 | SDK 读码 |
| SYSTEM `disconnect` | ACK 后关本连接，退避 1 秒后重新 `open`（新 ticket） | SDK 读码（SDK 不 ACK 直接关；本包先 ACK 再关，无害） |
| EVENT | ACK data `{"status":"SUCCESS","message":"success"}`，Debug 日志，不上送 | SDK 读码；`LATER` 语义为推断，本包不用 |
| 回调类 ACK data | `{"response":{}}` | **推断** |
| 机器人消息 | 串行 dispatcher：归一化 → sink → sink 返回后 ACK；sink 出错 → `code 500` | 本包设计（对齐 feishu「失败 → 让平台重推」的意图） |
| 卡片回调 | 读循环里解析完**立刻** ACK，再另起 goroutine 投递 sink | 卡片（2 秒时限） |

生命周期：`Start` 只有 ctx 取消才返回；断线（服务端断开或 `disconnect`）后按 1, 2, 4, 8, 16, 30, 30… 秒退避、无限重试；
成功重连才 `ReconnectCount+1`；`Connected()` 在连接期间为 true。

ACK 时序一览：

| 帧 | 何时 ACK | 在哪个 goroutine |
|---|---|---|
| SYSTEM ping / disconnect / 其它 | 收到即 ACK | 读循环 |
| EVENT | 收到即 ACK | 读循环 |
| CALLBACK 卡片回调 | 解析完即 ACK，之后异步投递 | 读循环（投递另起 goroutine） |
| CALLBACK 机器人消息 | sink 返回后 | 串行 dispatcher（保序，慢 sink 不堵读循环） |

gorilla 的 `Conn` 同一时刻只许一个写者：所有 ACK 都过一把写锁。连接关掉时 dispatcher 里没处理的帧不再 ACK（平台没收到 ACK 会重推，core 靠 `event_id` 去重）。

## 2. 归一化（机器人消息）

data 一律按 `map[string]any` 解析（`text.isReplyMsg` / `text.repliedMsg` 没有文档，官方 Go SDK 的结构体会丢掉它们）。

| `NormalizedEvent` 字段 | 取值 |
|---|---|
| `event_id` | `msgId`（平台重推不变；adapter 不去重） |
| `kind` / `platform` | `MESSAGE` / `"dingtalk"` |
| `tenant_id` | `Options.TenantID`，空 → `default` |
| `workspace_id` | `chatbotCorpId`，空退 `senderCorpId` |
| `chat_id` | `conversationId` |
| `chat_type` | `"1"` → `P2P`，`"2"` → `GROUP` |
| `sender_id` | `senderStaffId`，空退 `senderId` |
| `sender_name` / `sender_kind` | `senderNick` / `HUMAN` |
| `mentioned` | `isInAtList`（单聊不强行置 true，DM 路由是 DD3 的） |
| `text` | `text.content` 去首尾空白，开头若是 `@<BotName>` 再剥掉（否则 `@Aite !stop` 过不了 R5） |
| `raw_text` | 原文（richText 为各文本段拼接；picture 为空） |
| `attachments` | richText 的图片段 / picture：`IMAGE`，`file_key = downloadCode`（空退 `pictureDownloadCode`），`message_id = msgId` |
| `occurred_at` | `createAt`（ms），缺失用注入时钟 |
| `raw` | 整份 data，**剥掉 `sessionWebhook`**（它能直接往会话里发消息） |
| `anchor` | 见 §3 |

不支持的 msgtype（file / audio / video …）返回 nil、Debug 日志，不上送。

同时把 `conversationId → {sessionWebhook, sessionWebhookExpiredTime, conversationType, senderStaffId, chatbotCorpId}` 记进会话缓存（出站用）。

`ParseQuote(data) *Quote{MessageID, SenderID, Text}`：只在 `text.isReplyMsg == true` 或 `text.repliedMsg` 存在时非 nil；
`repliedMsg.content` 是对象取 `.text`、是字符串直接用，其它形状留空。T0 之后由它填 `NormalizedEvent.quote`（建议归 DD11）。

## 3. 锚点规则

- 钉钉没有话题 → 可见 `#A` 锚点 + 引用续接。
- `TaskNoOf(text)`：不分大小写匹配 `#A` + **Crockford base32** `[0-9A-HJKMNP-TV-Z]+`（去掉 I L O U），结果转大写；
  号码后紧跟 ASCII 字母或数字就不算（`#Awesome` 不是锚点），中文可以紧跟（`#AH进展如何` → `#AH`）。只认 `#A\d+` 是错的（17 → `#AH`）。
- **先看本条正文，找不到再看引用内容**，都没有 → `anchor.task_no = nil`。
- `anchor.thread_id = repliedMsg.msgId`，**只作提示**：引用的是用户自己那条根消息时能命中 R7 建的会话；
  引用的是 Aite 的回复时大概率对不上 → 走 R7，与不填一样。H9 确认前 `#A` 文本是主锚点。
- `anchor.message_id = msgId`。

## 4. 出站

| 方法 | 路由 |
|---|---|
| `SendText` | 缓存里有未过期的 `sessionWebhook`（到期前留 60 秒余量）→ POST 它 `{"msgtype":"markdown","markdown":{"title","text"}}`，**不返回消息 id**（`SendResult.message_id` 为空；core 今天不读 `send_text` 的返回值）；否则群 → `POST /v1.0/robot/groupMessages/send`（`openConversationId`），单聊 → `POST /v1.0/robot/oToMessages/batchSend`（`userIds` = 缓存的 `senderStaffId`，缺 → 不可重试错误），`msgKey "sampleMarkdown"`，`message_id = processQueryKey`。缓存里没有的会话按群处理 |
| `SendText` 的 `reply_to` / `in_thread` | 钉钉没有对应物，忽略 |
| `DownloadFile` | `POST /v1.0/robot/messageFiles/download {downloadCode, robotCode}` → `downloadUrl` → GET（不带 token 头）。单聊才收得到文件，群里只有图片走这条 |
| 鉴权 | `POST /v1.0/oauth2/accessToken {appKey, appSecret}` → `accessToken` / `expireIn`，提前 60 秒换新；请求头 `x-acs-dingtalk-access-token`；401 作废重取一次 |
| 错误 | HTTP ≥ 400 → `PlatformError{Code: 响应体 code 或 HTTP 状态, HTTPStatus, Retryable: 429/5xx}`；传输失败 → `timeout` / `transport_error`，可重试。传输错误里的 URL 一律去掉 query（`sessionWebhook` 的 query 就是会话令牌）；`clientSecret` / token / webhook 永不进日志与错误文本 |

## 5. AI 卡片

- `SendCard`：`outTrackId = "aite-" + 16 位随机 hex`；`POST /v1.0/card/instances/createAndDeliver`，
  `cardTemplateId`（`Options.CardTemplateID`，空 → 不可重试错误）、`callbackType "STREAM"`、`cardData.cardParamMap`；
  群 `openSpaceId = dtv1.card//IM_GROUP.<chat_id>` + `imGroupOpenSpaceModel{supportForward}` + `imGroupOpenDeliverModel{robotCode}`，
  单聊 `dtv1.card//IM_ROBOT.<staffId>` + `imRobotOpenSpaceModel{supportForward}` + `imRobotOpenDeliverModel{spaceType:"IM_ROBOT"}`
  （两个 `*OpenSpaceModel` 是调研记忆里的官方示例形状，派单没点名，待 H9 核实是否必需）。
  返回 `SendResult{message_id: outTrackId, card_id: outTrackId}`，并记下 `outTrackId → {chat_id, chat_type, corp_id}` 给卡片回调用。
- `UpdateCard`：`PUT /v1.0/card/instances`（同一 `outTrackId`、同一套 `cardParamMap`、`cardUpdateOptions.updateCardDataByKey = true`），**绝不新发一张**。
- `StreamCard(ctx, cardID, markdown, finalize)`（不在 `PlatformPort` 上，FF3 接）：`PUT /v1.0/card/streaming`
  `{outTrackId, guid, key:"content", content, isFull, isFinalize, isError:false}`。口径：**每次都把完整 markdown 重发**（不做增量 diff，那是 FF3 的），
  按 rune 边界切成 ≤1024 字节的块，第一块 `isFull=true`、其余 `isFull=false`（追加）；`finalize` 时只有最后一块 `isFinalize=true`；每个请求一个新 `guid`。

### H9 建模板须知

| 模板变量 | 内容 |
|---|---|
| `content` | 清单渲染成的 markdown（标题行 `**#AH 标题**`、`发起人 · 开始时间 · 状态`、`- ✅/⏳/⬜/❌ 条目（备注）`、页脚）；`StreamCard` 也写这个 key |
| `task_id` | 任务 UUID |
| `task_no` | `#AH` 形式的任务号 |
| `status` | `working` / `delivered` / `failed` / `cancelled` |

Stop 按钮回传参数：`{"action":"stop","task_id":"${task_id}"}`（放在 `cardPrivateData.params` 里）。

## 6. 卡片回调 → `CARD_ACTION`

- data 形状（**推断**）：`{"outTrackId","userId","content":"<JSON 字符串>"}`，`content.cardPrivateData.params = {"action","task_id"}`。
- 只认 `action == "stop"` → `kind CARD_ACTION`、`card_action{card_id: outTrackId, action: STOP, task_id（空 → nil）, value: params}`；
  `chat_id / chat_type / workspace_id` 取 `outTrackId` 映射（取不到留零值，core R3 按 task_id 查）；
  `anchor{platform:"dingtalk", chat_id, message_id: outTrackId, thread_id: nil}` 必填；`sender_id = userId`、`HUMAN`、`mentioned = true`；
  `event_id = 帧 headers.messageId`；`occurred_at` 用注入时钟。
- 别的动作 → 不产生事件，照样 ACK。
- ACK 在读循环里当场回（2 秒时限），投递另起 goroutine；投递失败只打 Error 日志（已 ACK，平台不会重推）。

## 7. 能力值（今天 pb 的 9 个字段）

| 字段 | 值 | 来源 |
|---|---|---|
| `platform` | `"dingtalk"` | — |
| `supports_thread` | false | 平台事实（没有话题） |
| `supports_history` | false | 平台事实（机器人读不了群历史） |
| `supports_passive_listen` | false | 平台事实（群里只收 @） |
| `supports_card_edit` | true | AI 卡片实例可按 `outTrackId` 更新 |
| `card_edit_window_sec` | 0 | **保守占位，待核实**（能更新多久待 H9；core 今天没有消费者）；DD11 按 `dingtalk_v1()` 对齐 |
| `inbound_file_in_group` | false | 平台事实（群里 @ 机器人收不到文件） |
| `proactive_requires_prior_message` | false | **保守取值，待核实**（`groupMessages/send` 能主动发）；DD11 按 `dingtalk_v1()` 对齐 |
| `outbound_rate_per_min` | 20 | **保守取值，待核实**；DD11 按 `dingtalk_v1()` 对齐 |

## 8. 未实现

| 方法 | 返回 | 后续 |
|---|---|---|
| `ReadHistory` | `aiteerr.ErrNotImplemented` → `UNIMPLEMENTED` | 平台不支持 |
| `ReadDocument` | 同上 | 不在 W1 |
| `SendFile` | 同上 | DD11 |
| `AddReaction` | 同上（core 对 ack 表情失败只打 warn） | DD11 |

另：file / audio / video 消息归一化、Approve / Reject 回调、@ 人（`atUserIds`）、单聊主动发、客户端保活 ping、OAuth 都不在本包（见回执「记账转出去的」）。

## 9. 待 H9 真机核实

1. `text.repliedMsg` 的真实形状（`msgId` / `senderId` / `content` 是对象还是字符串、`isReplyMsg` 是否总在）。
2. `repliedMsg.msgId` 能否对上：Aite 回复的 `processQueryKey`、卡片的 `outTrackId`、原消息的 `msgId`（决定 `thread_id` 能否从「提示」升级为路由键）。
3. 群里 `text.content` 带不带 `@名`、带的话是不是 `@<BotName>` 开头（决定 `stripBotMention` 是否够用）。
4. 机器人消息回非 200 ACK 时钉钉会不会重推；回调类 ACK 的 data 是否必须是 `{"response":…}`。
5. 卡片回调实测时延（2 秒时限的余量）与回调 data 的真实形状（`content` / `cardPrivateData.params`）。
6. 卡片实例 `PUT /v1.0/card/instances` 能更新多久（决定 `card_edit_window_sec`）。
7. 下列 REST 路径、请求头、请求体键名来自总管 09-25 的调研记忆，未对官方文档（集中在 `api.go` 的 `Path*` 常量）：
   `POST /v1.0/oauth2/accessToken`（`appKey` / `appSecret` → `accessToken` / `expireIn`）、请求头 `x-acs-dingtalk-access-token`、
   `POST /v1.0/robot/groupMessages/send`、`POST /v1.0/robot/oToMessages/batchSend`（`robotCode` / `msgKey` / `msgParam` / `openConversationId` / `userIds` → `processQueryKey`）、
   `POST /v1.0/robot/messageFiles/download`（`downloadCode` / `robotCode` → `downloadUrl`）、
   `POST /v1.0/card/instances/createAndDeliver`（`openSpaceId` 的 `dtv1.card//IM_GROUP.` / `dtv1.card//IM_ROBOT.` 前缀、`*OpenSpaceModel` / `*OpenDeliverModel`）、
   `PUT /v1.0/card/instances`、`PUT /v1.0/card/streaming`、sessionWebhook 的 markdown 体与 `errcode` 响应、错误体 `{"code","message"}`。
8. `card/streaming` 分块追加（`isFull=false`）的语义是否如本包假设；1 KB 上限按字节还是字符计。
9. 三个保守能力值（§7）。

## 10. H9 探针结果（DD11 按 `review/p1/ledger/H9-dingtalk-probe.md` 填）

| # | 核实项 | 实测结果 | 本包需要的改动 | 由谁改 |
|---|---|---|---|---|
|  |  |  |  |  |
