# 移植清单 · 飞书 Adapter（Python → Go）— 由只读探查 agent 于 2026-09-11 生成

包根：`aite/adapters/feishu/`　测试根：`tests/adapters/`　Fixture：`tests/fixtures/feishu/`
目标：`edge/internal/feishu/`（Go，owner R1）+ `edge/testdata/feishu/`。以 Python 源码为最终依据；本清单是索引与提醒。

---

## 1. 每个 .py 文件：职责 / 公开面 / 外部依赖

### `__init__.py`（36 行）
纯 re-export 桶，`__all__` 20 个名字：`CARD_MAX_BYTES` `DEFAULT_DOMAIN` `REACTION_EMOJI` `RECONNECT_MAX_SEC` `RETRY_DELAYS` `FeishuApiClient` `FeishuPlatform` `LarkWSConnection` `PlatformError` `RawEventHandler` `TokenBucket` `WSConnection` `backoff_delay` `build_checklist_card` `build_markdown_card` `dumps_card` `normalize` `normalize_card_action` `normalize_message`。

### `errors.py`（38 行）—— 唯一的错误类型
- `PlatformError(RuntimeError)`；`__init__(code: str|int, message: str = "", *, retryable: bool = False, http_status: int|None = None)`
- 字段：`code` `message` `retryable` `http_status`；`str()` = `[code] message`。
- Go 侧对应 `edge/internal/aiteerr.PlatformError`（R0 已写，含到 gRPC status 的映射）。

### `ratelimit.py`（64 行）—— 出站令牌桶
- `TokenBucket(rate_per_min: int, *, capacity: int|None = None, clock = time.monotonic, sleep = asyncio.sleep)`；`rate_per_min <= 0` 抛 `ValueError`。
- `capacity` 默认 = `rate_per_min`；`_per_sec = rate_per_min / 60.0`。
- `tokens` property（读之前先 refill）；`async acquire(tokens=1) -> float` 返回实际等待秒数；`tokens > capacity` 抛 `ValueError`；持锁期间 sleep（等待者串行）。
- Go：`golang.org/x/time/rate` 或手写；时钟与 sleep 必须可注入（测试要断言 `slept == [30.0]`）。

### `api.py`（228 行）—— httpx REST 客户端
**为什么不用 SDK 的 API client**：SDK 底层同步、且判据要求断言 HTTP 方法/路径。**出站走 HTTP 直打，只有长连接用 SDK。** Go 同理：`net/http` + `httptest`。
- 常量：`DEFAULT_DOMAIN = "https://open.feishu.cn"`、`RETRY_DELAYS = (0.5, 1.0, 2.0)`、`_TOKEN_SAFETY_SEC = 60`
- 路径常量 11 条：`PATH_TENANT_TOKEN` `PATH_MESSAGES` `PATH_MESSAGE` `PATH_MESSAGE_REPLY` `PATH_MESSAGE_REACTIONS` `PATH_MESSAGE_RESOURCE` `PATH_FILES` `PATH_IMAGES` `PATH_DOCX_DOCUMENT` `PATH_DOCX_RAW_CONTENT` `PATH_WIKI_NODE`
- `FeishuApiClient(*, app_id, app_secret, domain, client=None, rate_per_min=60, retry_delays=RETRY_DELAYS, sleep, clock, timeout=30.0)`
- `async tenant_access_token() -> str`（锁保护的带过期缓存，提前 60s 刷新）；`invalidate_token()`
- `async request(method, path, *, params=None, json=None, files=None, data=None, rate_limited=False, binary=False) -> Any`：返回 `body.get("data") or {}`；`binary=True` 直接返回 `resp.content`（**完全跳过业务码检查**）
- `_error_from_response(resp, *, retryable=None) -> PlatformError`：`code = body.code`，缺失用 HTTP 状态码；`message = body.msg` 或 `resp.text[:200]`

### `cards.py`（147 行）—— ChecklistCard → 飞书卡片 JSON（见 §3.2）
### `connection.py`（217 行）—— 长连接封装（见 §4）
### `normalize.py`（471 行）—— 见 §2。不 import SDK、不 import http；全程只读原始 dict（`raw` 原样进审计）。
### `platform.py`（515 行）—— PlatformPort 实现
- 常量：`ON_EVENT_BUDGET_SEC = 1.0`、`KNOWN_EMOJI_TYPES`（OnIt/DONE/CRY/GLANCE/THUMBSUP/MUSCLE/OK）、`REACTION_EMOJI = {"ack":"OnIt","done":"DONE","fail":"CRY"}`、`_FILE_TYPE_BY_EXT`（9 条）、`_HISTORY_PAGE_SIZE = 50`、`_DOC_URL_RE = r"/(docx|docs|wiki)/([A-Za-z0-9]+)"`
- `FeishuPlatform(*, app_id, app_secret, bot_open_id, tenant_id="default", domain, history_window=50, api=None, connection_factory=None, sleep, clock, capabilities=None)`；`from_config(cfg, *, tenant_id, env)` 按**环境变量名**取值，缺变量不炸（`app_id == ""`）
- `set_passive_listen(value)` 只改实例的 capabilities 副本（**每实例一份深拷贝**）
- `start / stop / send_text / send_card / update_card / send_file / add_reaction / read_history / read_document / download_file`
- 依赖版本：`lark-oapi` 实测 1.7.3。Go：`github.com/larksuite/oapi-sdk-go/v3 v3.12.0`（已预钉）。

---

## 2. 归一化规则（`normalize.py`）全部分支

### 2.1 总入口 `normalize()`
读 `raw["header"]["event_type"]`：`im.message.receive_v1` → `normalize_message`；`card.action.trigger` → `normalize_card_action`；其他 → **`None`**。

### 2.2 `normalize_message` 逐字段映射

| 字段 | 来源与兜底 |
|---|---|
| `event_id` | `header.event_id` → `message.message_id` → `""`（取第一个非空） |
| `kind` | 固定 `message` |
| `platform` | 固定 `"feishu"` |
| `tenant_id` | 入参（默认 `"default"`） |
| `workspace_id` | `header.app_id` → 入参 → `""` |
| `chat_id` | `message.chat_id` → `""` |
| `chat_type` | `message.chat_type == "p2p"` → p2p，**其余一律 group**（含缺失） |
| `sender_id` | `event.sender.sender_id.open_id` → `""` |
| `sender_kind` | `sender_kind_of(event.sender.sender_type)` |
| `sender_name` | **恒为 None**（只有 `read_history` 会填） |
| `text` / `raw_text` | `extract_text()` |
| `mentioned` | `bool(bot_open_id) and (mentions 里有 open_id == bot_open_id 或 _post_mentions_bot())` |
| `anchor` | `Anchor(platform="feishu", chat_id, message_id, thread_id)` |
| `attachments` | `_attachments_of()` |
| `card_action` | 固定 None |
| `occurred_at` | `to_datetime(message.create_time)` → `to_datetime(header.create_time)` → now(UTC) |
| `raw` | 同一个 dict 原样（不是深拷贝） |

`thread_id`：`_first(message.root_id, message.parent_id, message.thread_id)` 取第一个非空并 `str()`；都没有 → None。飞书的 `thread_id` 是 `omt_` 开头的话题 id，与前两个不是一个 id 空间，排最后基本兜不到 —— 契约定义是「话题 root **消息** id」。

### 2.3 `extract_text`
1. `msg_type = message.message_type or message.msg_type`（事件面 / 历史 API 面两种形状）
2. 不在 `{"text","post"}` → `("", None, [])`
3. `text`：`raw_text = content["text"] or ""`；`text = apply_mentions(raw_text, …)`
4. `post`：`raw_text = _flatten_post(content, image_keys=收集)`（占位符原样保留）；`text = apply_mentions(raw_text, …)`

`load_message_content`：dict 直接用；非空 str → `json.loads`，失败或非 dict → `{}`。

### 2.4 @ 剥离（`apply_mentions`）
- @ 到机器人自己（`mention_open_id(m) == bot_open_id` 且非空）→ 正则 `re.escape(key) + r"[ \t ]*"` 全局删（**连同紧跟的空白 / U+00A0**）
- @ 到别人 → `text.replace(key, "@" + name)`；name 空则替换成 `""`
- 兜底扫 `_PLACEHOLDER_RE = r"@_(?:user|all)_\d+[ \t ]*"` 全删
- 最后 `.strip()`

`mention_open_id`：`mention["id"]` 是 dict → `["open_id"]`；是 str → 该串（空→None）；其他 → None。

### 2.5 `sender_kind` 映射
`user→human`、`bot→bot`、`app→app`、`system→system`；其他（anonymous/unknown/非字符串/未来新增）→ **app**（兜底，一律不猜成 system）。先 `.lower()`。飞书两套枚举（事件面 user/bot；消息 API 面 user/app/anonymous/unknown）都吃。

### 2.6 附件
```
image   → (kind=image, key 字段 image_key)
sticker → (kind=image, key 字段 file_key)
file    → (kind=file,  key 字段 file_key)
audio   → (kind=file,  key 字段 file_key)
media   → (kind=file,  key 字段 file_key)
```
主附件：key 非空才建；`name = content.file_name or None`；`size = _to_int(content.file_size)`（**平台给字符串 "20480"，转 int**）；`mime = content.mime_type or None`；`message_id` 填当前消息 id。随后把 post 内嵌 `image_keys` 逐个追加为 `Attachment(kind=image, file_key, message_id)`（name/size/mime 全 None）。顺序：主附件在前。

### 2.7 post 拍平（`_flatten_post`）
- `title` 非空 → `.strip()` 作第一行
- 逐段落逐 segment：`text/md/code_block` → `segment.text` → `segment.content` → `""`；`a` → `[text](href)`（href 空则只 text）；`at` → `user_id` 以 `@_` 开头原样吐占位符，否则 `@{user_name}`（无 name 则 `""`）；`img` → 不进文本，`image_key` 收进 `image_keys`；`emotion` → `emoji_type`；其他 tag 忽略
- 每段 `""` 拼接后 `.strip()`；空行丢弃；行间 `"\n"`

`_post_mentions_bot`：只对 post 生效；先从 mentions 挑出 open_id == bot 的 `key` 集合（`@_user_N`），再扫 `at` 段，`user_id in bot_keys` 或 `user_id == bot_open_id`（老形状兜底）即命中。

### 2.8 `normalize_card_action`
- `value = event.action.value`；`name = value["action"]`；**不在 {stop, evidence} → None**
- `card_id = event.context.open_message_id`；`chat_id = event.context.open_chat_id`
- `event_id = header.event_id → card_id → ""`；`workspace_id = header.app_id → 入参`
- `chat_type` 固定 group；`sender_id = event.operator.open_id`；`sender_kind` 固定 human
- `text=""`、`raw_text=None`、`mentioned=True`
- `anchor.message_id = card_id`，`anchor.thread_id` 固定 None
- `attachments = []`；`card_action = CardAction(card_id, action=name, task_id=value.get("task_id") or None, value=整个 value)`
- `occurred_at = to_datetime(header.create_time) or now`

### 2.9 时间戳（`to_datetime`）—— T16 修的第二处
- None/"" → None；`int()` 失败 → None
- **按量级判单位**：`abs(ticks) >= 10**14` → 微秒（/1e6），否则毫秒（/1e3）
- `fromtimestamp(seconds, tz=UTC)`；溢出/值错 → None（让调用方回退 now，不为一个时间戳丢整条事件）
- 原因：官方文档自相矛盾（事件订阅概述与 card.action.trigger 例子是 16 位微秒，im.message.receive_v1 例子是 13 位毫秒）

---

## 3. 出站：端点 / 请求体 / 卡片 JSON

### 3.1 端点表

| 方法 | HTTP | 路径 | 请求体/参数要点 | 过令牌桶 |
|---|---|---|---|---|
| 鉴权 | POST | `/open-apis/auth/v3/tenant_access_token/internal` | `{app_id, app_secret}`，不带 Authorization | 否 |
| `send_text` | POST | 有 reply_to：`/open-apis/im/v1/messages/{reply_to}/reply`；无：`/open-apis/im/v1/messages?receive_id_type=chat_id` | reply：`{content, msg_type:"interactive", reply_in_thread: msg.in_thread}`；create：`{receive_id: chat_id, msg_type:"interactive", content}` | 是 |
| `send_card` | 同上 | 同上 | 同上，但 **`in_thread` 恒为 True** | 是 |
| `update_card` | **PATCH** | `/open-apis/im/v1/messages/{card_id}` | **body 只有 `{"content": "<卡片 JSON 字符串>"}`** | 是 |
| `send_file`（图） | POST | `/open-apis/im/v1/images` | multipart `image`，form `image_type=message` | 否 |
| `send_file`（文件） | POST | `/open-apis/im/v1/files` | multipart `file`，form `file_type=_file_type_of(name)`、`file_name` | 否 |
| `send_file` 第二步 | POST | 同 send_text | `content = {"image_key": …}` / `{"file_key": …}`，`msg_type = image / file` | 是 |
| `add_reaction` | POST | `/open-apis/im/v1/messages/{message_id}/reactions` | `{"reaction_type": {"emoji_type": REACTION_EMOJI[kind]}}` | 是 |
| `read_history` | GET | `/open-apis/im/v1/messages` | `container_id_type=chat`、`container_id`、`sort_type=ByCreateTimeDesc`、`page_size=min(50, budget-已收)`、`with_sender_name="true"`、可选 `page_token` | 否 |
| `read_document`（wiki） | GET | `/open-apis/wiki/v2/spaces/get_node` | `?token=<wiki token>&obj_type=wiki` → `data.node.obj_token` | 否 |
| `read_document`（元信息） | GET | `/open-apis/docx/v1/documents/{token}` | → `data.document.title` | 否 |
| `read_document`（正文） | GET | `/open-apis/docx/v1/documents/{token}/raw_content` | `?lang=0` → `data.content`（纯文本） | 否 |
| `download_file` | GET | `/open-apis/im/v1/messages/{message_id}/resources/{file_key}` | `?type=image`（key 以 `img_` 开头）或 `?type=file`；binary | 否 |

`_file_type_of`：`.opus/.mp4/.pdf → 同名`；`.doc/.docx → doc`；`.xls/.xlsx → xls`；`.ppt/.pptx → ppt`；其余 → **`stream`**。
返回：`_send_message` 返回 `str(data.get("message_id") or "")`；`send_card` 的 `SendResult.card_id == message_id`。

### 3.2 ChecklistCard → 飞书卡片 JSON（v1 schema，不写 `schema` 字段）
```
config:  {wide_screen_mode: true, update_multi: true}     ← update_multi 必须 true（更新前后都要）
header:  {template: STATUS_TEMPLATE[status], title: {tag:"plain_text", content:"<task_no> <title>"}}
elements: [...]
```
`STATUS_TEMPLATE`：working→blue、delivered→green、failed→red、cancelled→grey，未知→blue。
`elements` 顺序：① `div` 三个 `is_short` 的 lark_md 字段（`**发起人**\n{initiator}`、`**开始于**\n{started_at}`、`**状态**\n{STATUS_LABEL}`：进行中/已交付/失败/已取消）② `hr` ③ `div` lark_md 待办正文：每行 `f"{STATE_ICON[state]} {text}"`，有 note 接 `"　—— {note}"`（U+3000 + 双破折号）；items 为空时 `_（还没有待办项）_`；裁掉时追加 `"\n…… 另有 N 项未显示"` ④ 有 footer → `note` 元素 ⑤ 有按钮 → `action` 元素。
`STATE_ICON`：todo→⬜、doing→🔄、done→✅、failed→❌。
按钮按 `card.actions` 原样顺序；不在 `ACTION_BUTTON` 里的静默丢弃；`{tag:"button", text:{tag:"plain_text", content:"停止"|"证据"}, type:"danger"|"default", value:{"action": <name>, "task_id": card.task_id}}` —— 与 `normalize_card_action` 读的口径一致（round-trip 测试钉住）。
**30KB 裁剪**：循环重建 + `dumps_card` 按 UTF-8 字节量，`<= 30_000` 或 lines 空则返回，否则从尾部丢一项、`dropped += 1`。
`dumps_card`：`json.dumps(payload, ensure_ascii=False, separators=(",",":"))`（飞书 `content` 收字符串）。
`build_markdown_card(text)`：`{config:{wide_screen_mode:true, update_multi:true}, elements:[{tag:"markdown", content: text}]}`，无 header。**所有 `send_text` 都走它**（`msg_type=interactive`）；`msg_type=text` 会把 `**粗体**` 原样吐出来。

---

## 4. 连接与重连

- `backoff_delay(attempt) = min(1 * 2**(attempt-1), 30)`，`attempt < 1` → 0；序列 `[1,2,4,8,16,30,30]`，之后恒 30，无限重试；返回 int。
- `start()` 循环：第一次连接不延迟；连上就 `attempt = 0`；「连接失败」与「连上又断」共用一个 attempt；断开后下一轮从 1s 起；每轮新建连接对象；SDK 自带 `auto_reconnect` 一律关掉（它用服务端固定间隔，不是指数退避）。
- `feishu.reconnected` INFO 只在 `attempt != 0` 时打，格式 `feishu.reconnected after=%s attempts`；`feishu.reconnecting attempt=.. delay=..s` / `feishu.connect_failed` / `feishu.connection_lost` 为 WARNING。
- `stop()`：置 `_stopping`、关连接、关 api client。
- **on_event 1s 约束不是超时打断，是事后告警**：`_dispatch_raw` 夹 `time.monotonic()`，超 `ON_EVENT_BUDGET_SEC` 打 `feishu.on_event_slow event_id=.. elapsed=%.3fs budget=%.1fs`；handler 抛异常 → `feishu.on_event_failed`，不外抛。Go 版对应：`ingress.Client.HandleEvent` 带 1s deadline，超时/失败向平台返回错误让其重推（见 spec §2.1）。
- **ack 时机**：Python SDK 在 handler 同步返回后自己回响应帧；`_handle` 只把协程扔进 loop 就返回。Go 版：SDK handler 同步等 `HandleEvent` 结果再返回（1s 内），失败返回 error。
- **去重不在 adapter**：无任何 event_id 缓存；两条测试钉住「同一 raw 投两次 handler 被调两次」。
- `_envelope(ctx)`：`{"schema": ctx.schema or "2.0", "header": {event_id, create_time, event_type, tenant_key, app_id}, "event": ctx.event or {}}` —— **故意不带 `header.token`**。
- Python SDK 私有面三处（Go 里应消失但行为要等价）：对齐 event loop、轮询 `_conn` 判断断线、`route_card_frames_as_events`（见 §8 第 1 条）。

---

## 5. 限速 / 错误

- 令牌桶 `rate_per_min` 来自 `capabilities.outbound_rate_per_min`（60）；`capacity` 默认 = rate → 空闲一分钟后允许一次 60 个突发；上传接口（images/files）与读接口不过桶，只有发消息/更新卡片/表情过。
- `PlatformError`：429/5xx/传输层错误 → 退避 3 次仍失败 → `retryable=True`；4xx 非 429 → 不重试 `retryable=False`；HTTP 200 但业务 `code != 0` → `retryable=False, http_status=200`；传输层 → `code="transport_error"`；token 接口 code != 0 → `retryable=False`；响应无 token → `PlatformError("no_token")`。
- `RETRY_DELAYS = (0.5, 1.0, 2.0)` → 总共 4 次请求；只对 429 / >=500 / 传输错误生效；重试前打 `feishu.retry method=.. path=.. status=.. attempt=..`。
- **401 例外**：`request()` 层 `invalidate_token()` 后重打一次，不吃退避额度、不 sleep。

---

## 6. 测试清单（100 个 def test_，参数化后 146 用例）

- `tests/adapters/conftest.py`：`FakeClock`（手动推进，`sleep` 只记账进 `slept`）、fixtures。
- `test_normalize.py`（21 defs/53）：6 个必需 fixture 存在；每个有 expected；逐字段相等；expected 可反序列化；`raw` 逐字节一致；toplevel（剥 @、raw_text 保留占位符、thread_id None、workspace_id==app_id）；in_thread_no_at（mentioned False 但 thread_id 有值）；in_thread_with_at（root/parent/thread 三者互不相同，取 root_id；别人的 @ 换成 `@李四`；无 `@_user_`）；from_bot（sender_kind ∈ {bot, app} 且 mentioned True）；with_file（text ""、raw_text None、1 附件、size 20480、attachment.message_id == anchor.message_id）；card_action_stop；post 拍平 + at 段算 @ + 内嵌图片；未订阅事件 None；未知按钮 None；不去重；只 @ 别人 → mentioned False；T16 三条（时间戳四种参数都落到 `2026-09-09T01:07:00+00:00`；card.action.trigger 微秒不炸；post at 段三种形状）。
- `test_feishu_outbound.py`（12/14）：update_card PATCH 且两条 POST 路由零命中、body 只有 content、`update_multi` true；连更 3 次 → PATCH 3 次；send_card 有/无 reply_to 两条路；send_text content 是 markdown 元素；`in_thread=False` → `reply_in_thread False`；send_file PNG → images、CSV → files；add_reaction 三种 emoji 合法；REACTION_EMOJI 键恰好 {ack,done,fail} 且禁止 `EYES`；限速 `rate_per_min=2` 连发 3 次 → `slept == [30.0]`；读接口不占额度。
- `test_feishu_read.py`（13）：历史正序；查询参数；不过滤 sender_kind；sender_name/thread_id 拿得到；thread 过滤客户端做（root_id 或 message_id 命中）；分页 limit=55 → 2 次、第二次带 `page_token=pt2`；`limit=0` → 不发请求返回 []；非文本 text ""；docx 打 meta + raw_content；wiki 先换 obj_token；裸 token；download `file_v2_` → type=file、`img_v3_` → type=image。
- `test_feishu_reconnect.py`（13）：退避序列；真跑 `start()` 量出 `[1,2,4,8,16,30,30]`；连上归零；连挂 50 次不抛；reconnected 是 INFO；首连不延迟；重连前旧连接 close；stop 后 start 正常返回；归一化交给 on_event；不去重；未订阅不调 handler；handler 抛异常不带走长连接；慢 handler 打 on_event_slow。
- `test_feishu_errors.py`（10/16）：`RETRY_DELAYS`；429/500/502/503 → 4 次请求、`slept == [0.5,1.0,2.0]`、retryable、http_status；第二次成功只 2 次；ConnectError → transport_error；400/403/404/422 → 1 次、retryable False；200 + 业务码 → 不重试；token code=10003 → retryable False；401 换 token 再打一次（token 2 次、发送 2 次、第二次 `Bearer t-new`、无 sleep）；token 跨调用缓存。
- `test_feishu_cards.py`（16/22）：本地最小 schema 校验；四状态、四 item state；空 items；note/header/footer；按钮 round-trip；actions 原样；30KB 裁剪；普通卡片 < 2000 字节；markdown 逐字保留；SDK 认更新 ≠ 发送（PATCH `/open-apis/im/v1/messages/:message_id`）。
- `test_feishu_capabilities.py`（12）：默认等于 FEISHU_P0 且 passive_listen False；每实例深拷贝；`set_passive_listen` 只改实例；注入的也拷；桶速率 60；from_config 按变量名、缺变量不炸；桶：满桶 60 不等、第 61 次等 1.0s、10 秒补 10 个、空闲不超 capacity、`TokenBucket(0)` 抛。
- `test_feishu_card_frames.py`（3，T16 专项）：手工构造 protobuf Frame 喂 SDK：不打补丁时 CARD 帧被 SDK 丢（`received == []`）；装上转接后到 `on_raw` 且 `"token" not in envelope["header"]`；普通 EVENT 帧不受影响。**Go SDK 是否投递 card.action.trigger 帧要实测，结论写回执。**

## 7. `docs/feishu-api-diff.md` 结论（T16）
1 长连接与两个事件名对，但 Python SDK 1.7.3 丢卡片回传帧（已修）；2 root/parent/thread 一致，`thread_id` 是 `omt_` 另一空间；3 文本消息一致，post `at` 段 `user_id` 是序号不是 open_id（已修）；4 sender_type 两套枚举，fixture `app`→`bot`（已修）；5 PATCH 同 message_id / ≤30KB / 14 天全部一致，单条消息更新 5 QPS，更新前后都要 `update_multi:true`，卡片 v1 不动；6 file_key/image_key 下载与上传发送一致，sticker 可能下不下来；7 `container_id_type=chat` 分页一致（page_size 上限 50）；8 单副本一致，每应用最多 50 连接；§3.7(a) 文档查不到只能真机验；§3.7(b) 群历史要 `im:message.group_msg`。

## 8. 非显然的行为（读了代码才知道的坑）
1. **卡片回传帧被 SDK 丢掉**（`route_card_frames_as_events`）：把 CARD 帧 `type` 头改成 `event` 交回 SDK；不修 `!stop`/证据按钮永远没反应。
2. **微秒时间戳整条丢事件**：原一律 /1000，微秒会溢出；`_dispatch_raw` 对 `normalize()` 没兜底。
3. **`REACTION_EMOJI["ack"]` 曾是 `EYES`**（非法，231001）→ 改 `OnIt`。
4. `mentioned` 需要 `bot_open_id` 非空；`FEISHU_BOT_OPEN_ID` 缺失时 R7 静默失效，而 `from_config` 缺变量不报错。
5. `chat_type` 只认 `"p2p"`，其余一律 group。
6. `sender_name` 事件路径恒 None。
7. `raw` 是同一 dict 引用。
8. 未知 card action 返回 None，只打 `feishu.event_ignored type=card.action.trigger` debug。
9. `CardAction.task_id` 用 `value.get("task_id") or None`（空串→None）。
10. **`event.token` 会进 `raw` 进审计**（只剥了 `header.token`）—— Go 版一并剥掉并写明。
11. `send_card` / `send_file` 的 `in_thread` 写死 True，只有 `send_text` 透传。
12. 上传不过令牌桶；读接口全不过。
13. 401 重打一次在 `request()` 层；每次重试循环重新取 token（走缓存）。
14. `binary=True` 跳过业务码检查（200 + JSON 错误体会被当文件字节）。
15. body 无 `code` 时用 HTTP 状态码当 code；message 兜底 `resp.text[:200]`。
16. `request()` 返回 `body.get("data") or {}`。
17. `update_card` 不检查响应内容。
18. `read_history` 带 `thread_id` 时 budget = `limit * 4`，过滤在收完之后；不够会静默少返回。
19. `page_size` 随已收数量收缩；翻页条件 `has_more && page_token && items 非空`。
20. `HistoryMessage.sender_id` 取 `item.sender.id`（历史 API 形状）。
21. `_to_history_message` 传 `bot_open_id=None`：历史里别人 @ 机器人不剥。
22. `HistoryMessage.thread_id` 只看 `root_id`；`_in_thread` 过滤是 root/parent/thread/message_id 四选一。
23. `with_sender_name` 传字符串 `"true"`（文档没有、SDK 有）。
24. `_parse_doc_ref` 把 `/docs/<token>` 也当 docx token（真机可能 404）。
25. 裸 token：`strip()` → 砍 `?` → `rstrip("/")` → 取最后一段。
26. `DocumentContent.url` 非 http 输入时合成 `/docx/{token}` 相对路径。
27. `download_file` 只按 `img_` 前缀判 type。
28. 所有文本出站都变 interactive 卡片，没有 `msg_type=text`。
29. 30KB 裁剪 O(n²)。
30. `card.actions` 未知名字静默丢弃。
31. capabilities 即使注入也深拷贝（`FEISHU_P0` 是共享可变单例；Rust 版 `feishu_p0()` 每次返回新值已消除此坑）。
32. `FeishuPlatform` 不保存 `app_secret`；注入 `api=` 时 `app_secret`/`domain` 被忽略。
33. `ON_EVENT_BUDGET_SEC` 按模块全局名查找（测试 monkeypatch）—— Go 里做成可注入。
34. `TokenBucket` 持锁 sleep。
35. `backoff_delay` 返回 int。

## 汇总
`tests/adapters` def test_ 总数 **100**（normalize 21、cards 16、read 13、reconnect 13、outbound 12、capabilities 12、errors 10、card_frames 3），参数化展开 **146**。Fixtures **7 对 = 14 个 JSON**：

| fixture | 一句话 |
|---|---|
| `message_at_bot_toplevel` | 顶层 @Aite 的 text 消息，无 root/parent/thread → thread_id None、mentioned true |
| `message_in_thread_no_at` | 话题里不带 @ 的追问，root_id==parent_id，无 mentions → mentioned false 但 thread_id 有值 |
| `message_in_thread_with_at` | 话题里带 @，root/parent/thread 三者各不相同（thread 是 `omt_`），两条 mentions（Aite + 李四） |
| `message_from_bot` | 另一个机器人发的、@ 了 Aite，`sender_type: "bot"`，带 root_id |
| `message_with_file` | CSV 附件 file 消息，`file_size` 是字符串 "20480"，无 mentions |
| `message_post_with_image` | 富文本 post：title「本周复盘」+ at/text/a + img，带 mentions |
| `card_action_stop` | 卡片「停止」回传，`header.create_time` 16 位微秒，`event.token` 存在 |

`.expected.json` 是 `NormalizedEvent.model_dump(mode="json")` 全字段快照（`occurred_at` 形如 `2026-09-09T01:0X:00Z`，`raw` 内嵌整条原始事件）。Go 版改为 protojson（`UseProtoNames:true, EmitUnpopulated:false`）重生成 expected，枚举形如 `SENDER_KIND_HUMAN`，时间为 RFC3339；真判据仍是手写字面量断言。
