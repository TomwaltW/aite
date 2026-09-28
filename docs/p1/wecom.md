# 企微智能机器人（aibot）长连接适配器 —— `edge/internal/wecom`

> CC10 交付（第 1 波，不接线）。包实现 `server.PlatformPort` 与飞书同形状的生命周期（`Start` / `Connected` / `ReconnectCount`），
> 但**没有任何现有文件 import 它**：平台工厂、配置映射、环境变量读取、放行 `platform: wecom` 都是 DD8（W2）的。
> 协议字段除命令名 / 事件名外都**未对真机核实**（云端打不开 developer.work.weixin.qq.com，文档 101463 没核上），
> 下文凡标 `TODO(真机核对)` 的，H9 真机冒烟时逐条核，改动只落 `protocol.go`。

## 1. 协议参考表

帧外形（`TODO(真机核对)`）：客户端发 `{"cmd", "headers": {"req_id"}, "body"}`；服务端应答 `{"headers": {"req_id"}, "errcode": 0, "errmsg": "ok"[, "body"]}`
（没有 `cmd`、有 `errcode` 的就是应答）。同一 `req_id` 上多帧在途（流式刷新）按发送顺序 FIFO 对应答。`errcode != 0` →
`*aiteerr.PlatformError{Code: 十进制 errcode, Retryable: -1 / 45009 才可重试（TODO(真机核对)）, Msg: errmsg}`。

| 用途 | cmd（`protocol.go` 常量） | 方向 | 命令名来源 | body 字段（全部 `TODO(真机核对)`） |
|---|---|---|---|---|
| 订阅 / 鉴权 | `aibot_subscribe`（`cmdSubscribe`） | → | 原卡给定 | `bot_id`、`secret`；应答 `errcode == 0` 才算连上 |
| 心跳 | `ping`（`cmdPing`） | → | 原卡给定 | 无 body；每 30 秒一帧（`pingInterval`），两个周期收不到任何帧 → 读超时判断线、走重连 |
| 消息回调 | `aibot_msg_callback`（`cmdMsgCallback`） | ← | 原卡给定 | `msgid`、`aibotid`、`chatid`、`chattype`（`single` / `group`）、`from.userid`、`msgtype`、`text.content`、`mixed.msg_item[]`、`image` / `file` 的 `url` + `aeskey`（+ `filename`）、`quote`（无 msgid） |
| 事件回调 | `aibot_event_callback`（`cmdEventCallback`） | ← | 原卡给定 | `event.eventtype` ∈ `enter_chat` / `template_card_event` / `feedback_event` / `disconnected_event`（事件名原卡给定）；`event.template_card_event{card_type, event_key, task_id}`；`event.feedback_event{id, type, content, inaccurate_reason_list}`，`type` 1 = 点赞、2 = 点踩、3 = 取消 |
| 流式回复 | `aibot_respond_msg`（`cmdRespondMsg`） | → | 原卡给定 | `headers.req_id` = 所回复回调帧的 `req_id`；`msgtype: stream`，`stream{id, finish, content, feedback{id}}`；回文件用 `msgtype: file, file{media_id}` |
| 欢迎语 | `aibot_respond_welcome_msg`（`cmdRespondWelcome`） | → | 原卡给定 | 同上 `req_id`，5 秒内；`msgtype: text, text{content}` |
| 更新模板卡片 | `aibot_respond_update_msg`（`cmdRespondUpdate`） | → | 原卡给定 | 同上 `req_id`，5 秒内；`response_type: update_template_card`、`template_card{…}`（卡片形状归 DD11） |
| 主动发送 | `aibot_send_msg`（`cmdSendMsg`） | → | 原卡给定 | `chatid` + `msgtype: markdown, markdown{content}` / `msgtype: file, file{media_id}` |
| 分片上传 | `aibot_upload_media_chunk`（`cmdUploadChunk`） | → | **占位，原卡没给命令名** `TODO(真机核对)` | `upload_id`、`filename`、`chunk_index`、`total_chunks`、`total_size`、`data`（base64）；最后一片的应答 body 带 `media_id` |

默认地址 `wss://openws.work.weixin.qq.com`（`DefaultWSURL`，来自派单）；测试一律覆盖成 `httptest` 的 `ws://127.0.0.1…`。

## 2. `PlatformPort` 方法 → 企微命令

| 方法 | 企微做法 | 备注 |
|---|---|---|
| `Capabilities()` | 返回副本 | 见 §3 |
| `SendText` | `ReplyTo` 查得到 → 新开一个流，一帧 `finish=true`；否则 `aibot_send_msg` markdown | 回退受 §5 两道闸约束 |
| `SendCard` | `replyTo` 查「回复表」（键 = 入站事件的 `Anchor.MessageId`，值 = 当时回调的 `req_id`）→ `aibot_respond_msg` 开流，内容 = 清单卡 markdown（含 `TaskNo`），开流帧带 `feedback.id = stream.id`；返回 `MessageId = CardId = stream.id` | 查不到（`replyTo` 为空 / 被挤出 4096 条有界表 / 进程重启过）→ `aibot_send_msg` 一帧 markdown，返回 `"nostream-" + 本帧 req_id` |
| `UpdateCard` | 同一个 `stream.id` 推全量内容；`DELIVERED` / `FAILED` / `CANCELLED` 才 `finish=true` | `nostream-` 前缀 / 已收尾 / 不在流表里 → 零帧、返回 nil + 日志 |
| `SendFile` | 分片上传（每片恰好 512 KB、按序、每片等应答，> 100 片在第一帧之前报 `file_too_large`）→ 拿 `media_id` 走回复或主动发送 | 20 MB（`MaxUploadBytes`）**不在这里拦**，见 §5 |
| `DownloadFile` | 查下载表（入站时按 `(Anchor.MessageId, FileKey)` 存 `url + aeskey + 到期`）→ HTTP GET → AES-256-CBC 解密 | 表里没有 / 过 5 分钟 → `file_expired`（404、不可重试） |
| `ReadHistory` / `ReadDocument` / `AddReaction` | `aiteerr.ErrNotImplemented`（server 层翻成 UNIMPLEMENTED） | core 容忍：ack 失败只 warn；读历史失败用空历史 |
| 包内额外导出 | `Standby()`、`RespondTemplateCardUpdate(ctx, reqID, card)`、`DefaultConfig()`、`WecomCapabilities()`、`Feedback` 结构体 | DD8 / DD11 用 |

入站归一化口径：`platform "wecom"`、`SenderKind HUMAN`、`SenderId = from.userid`（原样，可能是加密的）、`WorkspaceId = aibotid`（空则 `Options.BotID`）、
`EventId = Anchor.MessageId = msgid`（空则回调 `req_id`）、`single → P2P` / `group → GROUP`、`OccurredAt` = 收到时刻。
群里只认 `text` / `mixed`（`quote` 挂在它们身上），`Mentioned = true`，`Text` 去掉开头的 `@<BotName>`；单聊认 `text` / `mixed` / `image` / `file`，`Mentioned = false`；其余 debug 丢。
`#A` 锚点：正则 `(?i)#A([0-9A-HJKMNP-TV-Z]+)\b`，先看正文再看 `quote` 文字，转大写后进 `Anchor.task_no`，`ThreadId` 恒 nil。
`template_card_event` 的 `event_key` 是 `stop` / `evidence` → `CARD_ACTION`（`TaskId` = 卡片的 `task_id`），其余丢；`CardId` 用本条回调的 id（回调不给卡片所在消息 id，`TODO(真机核对)`）。
`enter_chat` 不送 core（单聊欢迎，不是入群，别映射成 `BOT_ADDED`）。`feedback_event` 解析成 `Feedback`，今天不送 core（没有 `EventKind::Reaction`）。

## 3. 能力值与来源

| 字段 | 值 | 来源 |
|---|---|---|
| `platform` | `"wecom"` | 本轨取值 |
| `supports_thread` | false | 本轨取值（企微无话题，plan §1.3） |
| `supports_history` | false | 本轨取值（bot 读不到历史） |
| `supports_passive_listen` | false | 本轨取值（群里只收 @） |
| `supports_card_edit` | false | 与 T0 `wecom_v1()` 一致 |
| `card_edit_window_sec` | 0 | 本轨取值 |
| `inbound_file_in_group` | false | 本轨取值（群里 bot 收不到文件，plan §2C） |
| `proactive_requires_prior_message` | true | 与 T0 `wecom_v1()` 一致 |
| `outbound_rate_per_min` | 30 | 原卡「每会话 30 条/分钟」（T0 原卡没给，契约缺口，给 T0 对齐） |

T0 新字段今天 pb 里没有，先写成包内常量（名字贴契约字段，DD11 逐个对到 `wecom_v1()`）：`StreamMaxSec = 600`、`CardActionDeadlineMs = 5000`、
`MaxUploadBytes = 20 MB（20·1024·1024）`、`ReactionsIn = true`、`RequiresVisibleAnchor = true`。

## 4. 主备 / 备用语义

一个 bot 只许一条长连接，新连接踢旧连接。收到 `disconnected_event`：`Standby()` 置 true、`Connected()` 置 false、Warn 日志 `wecom.standby`、
断开本连接，**不再重连**（重连会把新主机踢下线，两台互踢）；`Start` 继续阻塞到 ctx 取消再返回 nil（否则 main.go 会把「Start 提前返回」当组件故障）。
普通断线（读错误 / 两个心跳周期无帧 / 服务端关连接）照旧按 1,2,4,8,16,30,30… 秒退避无限重连，重连成功 `ReconnectCount` +1。

**备机怎么接管**（本轨不做，记账转 DD8）：备用是终态，只有重启进程能让它再去抢连接（或者以后加一个管理动作清掉 `standby` 位）。
`EdgeStatus` 今天没有「备用」位，备机在 `!status` 里只能显示离线（见回执「契约缺口」）。

## 5. 限额：哪些在本包、哪些不在

| 限额 | 在哪里 | 说明 |
|---|---|---|
| 主动发送只给来过消息的会话 | 本包 `ratelimit.go` | 入站过的 `chatid` 集合（有界 10000）；否则 `proactive_not_allowed`（不可重试），一帧不发 |
| 每会话任意 60 秒内主动发送 ≤ 30 条 | 本包 `ratelimit.go` | 滑动窗口（最近 30 次发送时刻的环形），第 31 次若最早那次距今 < 60 s 就拒 → `rate_limited`（可重试 → UNAVAILABLE）。**不用 `x/time/rate`**：令牌桶 0.5/s + 突发 30 头一分钟能放过约 59 条 |
| 回复（流式）是否计入同一 30 条/分钟预算 | **不在本包** → DD11 / FF3 | plan:210 说回复也算；本轨只对 `aibot_send_msg` 计数 |
| 每小时 1000 条 | **不在本包** → FF3 | |
| stream 10 分钟必须收尾 | 本包 `stream.go` | `streamAutoFinishAfter = 9 min`：后台 sweep 发 `finish=true`，内容 = 最后一次内容 + `\n\n进度见 #A..`；≥ 9 分钟的 `UpdateCard` 也直接收尾；之后的里程碑消息是 FF3 的 |
| 模板卡片 / 欢迎语 5 秒内回 | 本包 | `RespondTemplateCardUpdate` 距回调 > 5 s → `deadline_exceeded` 一帧不发；Approve / Reject / Submit 与「谁在 5 秒内调它」是 DD11 的 |
| 每用户 3 条在途 / 24 小时回复窗口 | **不在本包** → DD11 | |
| 上传 ≤ 100 片 × 512 KB | 本包 `media.go` | 超了在第一帧之前报错 |
| 上传 ≤ 20 MB（`max_upload_bytes`） | **不在本包** → DD11 | 20 MB = 40 片，若在 `SendFile` 里拦，100 片那道检查就永远走不到；由 DD11 对到契约后决定在哪里拦 |
| 下载链接 5 分钟有效 | 本包 `media.go` | 到期（含恰好 5 分钟）→ `file_expired` |

## 6. 密钥处理

- `secret` 只出现在订阅帧里；`aeskey` 与下载 `url` 只进包内有界表（edge 内存）。
- 帧日志只打 `cmd` + `req_id`；`Attachment.FileKey` 只放 `img-0` / `file-0` 这类短 id。
- `NormalizedEvent.Raw` 进之前递归删掉所有 `aeskey` 与 `url`（raw 会落进审计 / 证据）。
- 下载失败的错误信息不带原始 URL。
- 解密（参考，`TODO(真机核对)`）：key = base64 解码 `aeskey`（先 `StdEncoding`，失败再 `RawStdEncoding` —— 43 字符无填充的 EncodingAESKey），
  必须恰好 32 字节；IV = key 前 16 字节；AES-256-CBC；PKCS#7 按 32 块去填充（接受 1..32）；非法输入一律返回错误，不 panic。
- 云端环境没有、也不许放真 bot_id / secret；测试全用假值。
- 成员 `userid` 除非 bot 由超级管理员创建否则是加密的（plan:122）。`Config.CorpIDEnv` / `AppSecretEnv` 是「另配自建应用换明文」的占位，本包不实现换取。

## 7. `Config` 与 DD8 的对接

`Config` 字段名照 T0 的 `WecomConfig`，yaml tag snake_case：`bot_id_env`（默认 `WECOM_BOT_ID`）、`bot_secret_env`（`WECOM_BOT_SECRET`）、
`ws_url`（`wss://openws.work.weixin.qq.com`）、`corp_id_env`（空）、`app_secret_env`（空）、`bot_name`（`Aite`）。`DefaultConfig()` 返回这组值。
包内不读环境变量：DD8 在 main.go 里读 `BotIDEnv` / `BotSecretEnv` 指向的变量，填 `Options{BotID, BotSecret, TenantID, WSURL, WelcomeText}`，
调 `wecom.New(cfg, opts, ingressClient)`，按生命周期接口起 `Start(ctx)`。`edge/internal/config` 里的 `Wecom` 段也是 DD8 的。

## 8. H9 真机冒烟核对清单（DD8 + DD11 合并后，企微 API 模式智能机器人）

1. 订阅：`aibot_subscribe` 的 body 字段名（`bot_id` / `secret`）、应答形状（`errcode` / `errmsg` 在顶层还是 body 里）。
2. 心跳：`ping` 的 cmd 名与是否要 body；服务端是否应答 ping；30 秒周期是否合适。
3. 群里 @ bot 发一句：`aibot_msg_callback` 的 `msgid` / `aibotid` / `chatid` / `chattype` / `from.userid` / `msgtype` / `text.content`；
   **`from.userid` 是明文还是加密的**（plan:122）；`text.content` 里 `@Aite` 的写法（前缀是否带空格 / 是否就是 bot 名）。
4. 引用 bot 的回复（里面带 `#A..`）再追问：`quote` 的字段名与结构（`quote.msgtype` / `quote.text.content` / mixed）；确认无 msgid。
5. 流式：`aibot_respond_msg` 的 `stream{id, finish, content, feedback{id}}`；同一 id 多次刷新是否全量替换；10 分钟上限的实际表现。
6. 停止：模板卡片按钮回调 `template_card_event` 的 `event_key` / `task_id` / `card_type`；`aibot_respond_update_msg` 的 body 形状与 5 秒窗口。
7. 点踩：`feedback_event` 的 `id` / `type`（1/2/3 的含义）/ `content` / `inaccurate_reason_list`。
8. 单聊首次进入：`enter_chat` 回调与 `aibot_respond_welcome_msg` 的 body。
9. 主动发送：`aibot_send_msg` 的 `chatid` + markdown 体；「没来过消息」时平台返回的 errcode。
10. 图片 / 文件（单聊）：下载 `url` + `aeskey` 字段名、AES 解密参数（key 长度、IV 取法、填充块长）、`filename` 字段。
11. 分片上传的**真实命令名**与字段（`cmdUploadChunk` 是占位），`media_id` 在哪个应答里。
12. 可重试的 errcode 集合（现假设 `-1`、`45009`）。
13. 两台 edge 同一个 bot：后连的那台上线时，先连的那台是否收到 `disconnected_event`、之后连接是否被服务端关掉。
