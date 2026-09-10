# 飞书 API 逐条核对（spec 附录 A × open.feishu.cn 官方文档）

> T16，2026-09-10。基线 `0d6939c`。
>
> spec 附录 A 自己写着「实施时以 open.feishu.cn 官方文档为准」，§7.6 也留了口子：
> 「飞书 API 与本文附录 A 不一致 → 以官方文档为准，在回执里写明差异；只要契约不变就不用停」。
> 这份文档就是把那个「为准」做掉的结果。
>
> **每条结论都挂 URL。查不到的写「查不到」，一条都不编。** 见文末「文档里查不到的」。
>
> 第二证据源：本机装的 **lark-oapi 1.7.3**（`pyproject.toml` 钉 `>=1.4`）。它的
> `lark_oapi/api/**` 全部是官方 OpenAPI 描述文件的生成产物，路径、HTTP 方法、
> 字段名可以拿它交叉验证 —— 文档页有几处字段表不全，靠它兜住了。

---

## 摘要：八条 + 两项待核实

| # | 附录 A 说的 | 结论 |
|---|---|---|
| 1 | 长连接 / `im.message.receive_v1` / `card.action.trigger` / `lark_oapi.ws.Client` | 名字全对，但 **SDK 有坑**：lark-oapi 1.7.3 在长连接上直接丢弃卡片回传帧 → 已修 |
| 2 | `root_id`/`parent_id`/`thread_id`、`reply_in_thread` | 一致 |
| 3 | `mentions[].id.open_id`、`@_user_1` 占位 | 文本消息一致；**富文本 post 的 `at` 段有差异** → 已修 |
| 4 | `sender.sender_type`（user → human） | 官方是**两套枚举**；映射表两套都吃得下，fixture 用错了值 → 已修 |
| 5 | PATCH 同一 `message_id`、≤30KB、14 天窗口 | 三条全部一致；卡片 schema 用 v1 是对的 |
| 6 | `file_key`/`image_key` 下载、上传再发送 | 一致；一处表情包(sticker)差异只写进报告 |
| 7 | `container_id_type=chat`、分页 | 一致；**所需权限查到了（§3.7 b）** |
| 8 | 同一应用多副本只有一个收到事件 | 一致，措辞都对得上 |
| §3.7 (a) | 只有 @ 权限时话题内不带 @ 是否投递 | **查不到**，只能真机验（M4） |
| §3.7 (b) | 群历史是否要「获取群组中所有消息」敏感权限 | **要**。`im:message.group_msg` |

---

## 1 · 应用类型与事件订阅

| 栏 | 内容 |
|---|---|
| **附录 A 怎么说** | 「企业自建应用，机器人能力，事件订阅用 **长连接**（SDK `lark_oapi.ws.Client`），事件 `im.message.receive_v1`、卡片回传 `card.action.trigger`。」 |
| **官方文档怎么说** | 事件类型固定 `im.message.receive_v1`。<br>https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN<br>回调类型固定 `card.action.trigger`，schema `2.0`。<br>https://open.feishu.cn/document/feishu-cards/card-callback-communication?lang=zh-CN<br>**回调也可以用长连接接收**：「使用长连接接收回调 方式是飞书 SDK 内提供的能力……建立一条 WebSocket 全双工通道」。<br>https://open.feishu.cn/document/event-subscription-guide/callback-subscription/callback-overview?lang=zh-CN<br>Python 侧「通过 lark.ws.Client() 初始化长连接客户端」。<br>https://open.feishu.cn/document/server-docs/event-subscription-guide/event-subscription-configure-/request-url-configuration-case?lang=zh-CN |
| **我们的代码怎么做的** | `normalize.py:29-30`（两个事件名常量）、`connection.py:129-134`（注册 + `lark_oapi.ws.Client`） |
| **结论** | 名字与类路径**一致**。但 SDK 实现层面有一条会让 P0 直接瘸腿的坑，见下。 |

### 1.1 差异：lark-oapi 1.7.3 在长连接上丢弃卡片回传帧（本轨最严重的一条）

`.venv/lib/python3.12/site-packages/lark_oapi/ws/client.py:340-344`：

```python
if message_type == MessageType.EVENT:
    result = self._event_handler._do_without_validation(pl)
elif message_type == MessageType.CARD:
    return                      # ← 收到了，然后什么都不做
else:
    return
```

`MessageType.CARD` 在整个 SDK 里**只出现这一处**（`grep -rn "MessageType.CARD"`），
没有第二条路能接到它。也就是说：卡片上的「停止」/「证据」按钮点下去，
服务端连分发都不会进，`§3.5 R3` 永远等不到事件。M2 / M6 会在真机上直接哑掉。

反过来，事件帧那一支**认得** `card.action.trigger`：
`event/dispatcher_handler.py:145-176` 的 `_do_without_validation()` 拿
`header.event_type` 拼出 `p2.card.action.trigger` 去查表，先查
`_callback_processor_map`（只有 `register_p2_card_action_trigger` 会写它），
查不到再落到 `_processorMap` —— 而 `_processorMap["p2.card.action.trigger"]`
正是我们 `connection.py:131` 的 `register_p2_customized_event` 写进去的。

**已修**：`connection.py:177` 新增 `route_card_frames_as_events()`，
在建连后把 CARD 帧的 `type` 头改写成 `event` 再交回 SDK 原处理。
只碰 CARD 帧 —— 平台若本来就用 EVENT 帧发卡片回传，或 SDK 哪天补上这一支，
这段就是一次空过路，不会多出第二种行为（严格只增不减）。

配套测试 `tests/adapters/feishu/test_feishu_card_frames.py` 三条，都不要真凭证：

* `test_sdk_alone_drops_card_frames` —— 钉住 SDK 当下的行为。**哪天升 lark-oapi
  它自己修好了，这条会红**，那就是可以把补丁删掉的信号。
* `test_card_frames_reach_the_handler` —— 加了转接后一路到 `on_raw`，内容原样。
* `test_event_frames_are_untouched_by_the_shim` —— 普通消息事件行为一个字不变。

**要总管在真机上确认的**：平台在长连接上究竟用哪种帧发卡片回传。文档只讲「可以用
长连接接收回调」，没写帧类型；SDK 有 CARD 分支说明至少存在这种可能。两种情况下
上面的补丁都是安全的，但真机能一次问清。

顺带一条真机上要小心的：官方明说**回调是同步的、不提供补推**，
「超时未响应即认为这次回调失败」，且要在 **3 秒内**回 HTTP 200。
丢掉的卡片回传不会重来，不像事件还有重推兜底。
https://open.feishu.cn/document/event-subscription-guide/callback-subscription/callback-overview?lang=zh-CN

---

## 2 · 话题

| 栏 | 内容 |
|---|---|
| **附录 A 怎么说** | 「消息体里的 `root_id` / `parent_id` / `thread_id`；回复进话题用『回复消息』接口并带 `reply_in_thread`。」 |
| **官方文档怎么说** | 字段描述逐字：`root_id`「根消息 ID，仅在回复消息场景会有返回值」；`parent_id`「父消息 ID，仅在回复消息场景会有返回值」；`thread_id`「消息所属的话题 ID（不返回说明该消息非话题消息）」。<br>https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN<br>「话题拥有在当前租户内唯一的 ID（即 `thread_id`）。`thread_id` 的格式是以 `omt_` 开头的字符串，例如：`omt_d4be107c616a`。」`container_id_type: thread` 时 `container_id` 填的就是 `thread_id`。<br>https://open.feishu.cn/document/im-v1/message/thread-introduction?lang=zh-CN<br>`POST /open-apis/im/v1/messages/:message_id/reply`，`reply_in_thread` 为 boolean、选填、**默认 false**，「取值 true 时将以话题形式回复」。<br>https://open.feishu.cn/document/server-docs/im-v1/message/reply?lang=zh-CN |
| **我们的代码怎么做的** | `normalize.py:328`（`_first(root_id, parent_id, thread_id)`）、`platform.py:271`（`reply_in_thread`）、`platform.py:365-368`（客户端按话题筛的理由） |
| **结论** | **一致**。 |

附带确认了一条原本只是推测的注释：`platform.py:365` 说「`container_id_type=thread`
收的是 `omt_` 开头的话题 id，而我们锚点里存的是话题 root **消息** id，两者不是一个
id 空间」—— 官方文档的 `omt_` 格式说明把这条坐实了，所以 `read_history` 在客户端
按话题筛是对的，不是绕路。

SDK 交叉验证：`ReplyMessageRequestBody` 字段为 `content` / `msg_type` /
`reply_in_thread` / `uuid`，uri `"/open-apis/im/v1/messages/:message_id/reply"`。

---

## 3 · @ 识别

| 栏 | 内容 |
|---|---|
| **附录 A 怎么说** | 「`mentions[].id.open_id` 与 `FEISHU_BOT_OPEN_ID` 比对；`text` 里的 `@_user_1` 占位要剥掉。」 |
| **官方文档怎么说** | 事件里 `mentions[]` = `{key: "@_user_1", id: {union_id, user_id, open_id}, mentioned_type, name, tenant_key}`，正文示例 `{"text":"@_user_1 hello"}`。<br>https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN<br>**富文本 post 的 `at` 段不一样**：`user_id` 的描述是「被 @ 的用户或机器人的**序号**。例如，第 3 个被 @ 到的成员值为 `@_user_3`」，示例 `{"tag":"at","user_id":"@_user_1","user_name":"","style":[]}`，真实身份要回 `mentions` 里按序号查。<br>https://open.feishu.cn/document/server-docs/im-v1/message-content-description/message_content?lang=zh-CN |
| **我们的代码怎么做的** | `normalize.py:143`（`mention_open_id`）、`normalize.py:171`（`apply_mentions`）、`normalize.py:364`（`_post_mentions_bot`） |
| **结论** | 文本消息**一致**；富文本 post **有差异** → 已修。 |

### 3.1 差异：post 的 `at` 段拿 `user_id` 跟 open_id 比，永远不会相等

改之前 `_post_mentions_bot()` 和 `_flatten_post()` 都是 `segment["user_id"] == bot_open_id`。
按文档，`user_id` 装的是 `@_user_N` 序号，这个比较**恒为 False**。后果：

* 群里有人用富文本 @ 机器人时，`mentioned` 只能靠 `message.mentions` 那条主判据认出来；
  兜底路径实际上是死代码。
* fixture `message_post_with_image.json` 原本在 at 段里写了个 `ou_aite_bot_...`，
  是照「我们以为它是这样」造的 —— 正是它让死代码看起来是活的。

**已修**：

* `_flatten_post()` 的 at 段现在**原样吐占位符**，剥 @Aite / 换人名统一交给
  `apply_mentions()`（它本来就是按 `mentions` 的 key 做的）。顺带 post 的 `raw_text`
  从 `@Aite …` 变成 `@_user_1 …`，与 text 类消息的 `raw_text` 同口径了。
* `_post_mentions_bot()` 改成拿序号回 `mentions` 查；同时保留直接比 open_id 的老路径，
  万一哪个客户端真塞了 open_id 进来也不漏判。
* fixture `message_post_with_image.json` 的 at 段改成 `"user_id": "@_user_1"`，
  并补上文档说会同时下发的 `message.mentions`。

新增测试：`test_post_at_segment_carries_a_placeholder_not_an_open_id`、
`test_post_at_placeholder_without_mentions_cannot_identify_anyone`、
`test_post_at_with_a_raw_open_id_is_still_recognised`。

---

## 4 · 发送者类型

| 栏 | 内容 |
|---|---|
| **附录 A 怎么说** | 「`sender.sender_type`（`user` → human；其他 → app/bot）。」 |
| **官方文档怎么说** | **两套枚举，取值不一样**：<br>① 事件面 `im.message.receive_v1` 的 `event.sender.sender_type`：「消息发送者类型」，可能值 `user`（用户）/ `bot`（机器人）。<br>https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN<br>② 消息 API 面（`获取指定消息的内容` / `获取会话历史消息`）的 `items[].sender.sender_type`：「发送者类型。可能值有：」`user`（用户）/ `app`（应用）/ `anonymous`（匿名）/ `unknown`（未知）。<br>https://open.feishu.cn/document/server-docs/im-v1/message/get?lang=zh-CN |
| **我们的代码怎么做的** | `normalize.py:51`（`_SENDER_KIND` 映射表 + 兜底） |
| **结论** | 附录 A 的「其他 → app/bot」笼统但不算错。映射表两套枚举都吃得下；`anonymous` / `unknown` 契约里没有对应项，跟着兜底落到 `app` —— §3.5 R1 只看「是不是 human」，落哪个非 human 都不改路由。**fixture 用错了值** → 已修。 |

**已修**：`message_from_bot.json` 原本在一个**事件**里写 `sender_type: "app"`，
而事件面官方只列了 `user` / `bot`。改成 `bot`（`sender_kind` 随之从 `app` 变 `bot`，
§6 T1 的判据写的是「sender_kind=bot 或 app」，两个都收）。
`normalize.py:36-50` 的注释也一并改了 —— 原文写「飞书自己的机器人消息 sender_type 是
`app`」，那句对消息 API 面成立，对事件面不成立。

---

## 5 · 卡片

| 栏 | 内容 |
|---|---|
| **附录 A 怎么说** | 「用『更新应用发送的消息卡片』接口（PATCH 同一 `message_id`）做原地更新；卡片 ≤30KB；14 天内可更新。」 |
| **官方文档怎么说** | `PATCH https://open.feishu.cn/open-apis/im/v1/messages/:message_id`。`content` 必填，「最大 **30 KB**」。「仅支持更新 **14 天内**发送的消息」。「单条消息更新频控为 **5 QPS**」。「你需在更新**前后**卡片的 `config` 属性中，均显式声明 `"update_multi":true"`」。只支持 `msg_type=interactive`；批量发送的消息、仅指定人可见的卡片、已撤回的消息都不支持。<br>https://open.feishu.cn/document/server-docs/im-v1/message-card/patch?lang=zh-CN<br>「回复消息」也印证了同一个数：content「文本最大 150 KB，卡片及富文本最大 30 KB」。<br>https://open.feishu.cn/document/server-docs/im-v1/message/reply?lang=zh-CN |
| **我们的代码怎么做的** | `platform.py:302`（PATCH）、`cards.py:21`（`CARD_MAX_BYTES = 30_000`）、`cards.py:118` / `cards.py:145`（`update_multi: True`） |
| **结论** | 三条**全部一致**。`FEISHU_P0.card_edit_window_sec = 1209600` 正好是 14 天，对得上。 |

### 5.1 卡片 schema 版本：v1 是对的，不动

* v2 要显式写 `"schema": "2.0"`，并把内容挪进 `body`（v1 是平铺的 `elements`）；
  **省略 `schema` 时默认按 `"1.0"` 解析**。
* v2 需要飞书客户端 **7.20 及之后**版本：「当使用 JSON 2.0 结构的卡片发送至低于 7.20
  版本的客户端时，卡片标题可正常显示，但内容将展示兜底的升级提示文案」。
  https://open.feishu.cn/document/feishu-cards/card-json-v2-structure?lang=zh-CN
* v1 的 `{"tag": "markdown", "content": ...}` 元素确实存在，且只有它支持图片 / 分割线
  这类「与文本格式无关」的 markdown 标签（`lark_md` 不支持）。
  https://open.feishu.cn/document/common-capabilities/message-card/message-cards-content/using-markdown-tags?lang=zh-CN

`cards.py` 全程 v1（`config` / `header` / `elements`），没写 `schema` 字段 —— 是默认值，
兼容面最宽。P0 不改。`build_markdown_card()` 用的 `markdown` 元素也在 v1 里。

### 5.2 新知：5 QPS 单条频控

文档新增的一条限制。W4 的「同一任务 500ms 内多次变更只调一次 `update_card`」
= 每秒最多 2 次，落在 5 QPS 之内，**不冲突**。记在这里免得日后有人把窗口调小。

---

## 6 · 文件

| 栏 | 内容 |
|---|---|
| **附录 A 怎么说** | 「群消息里的 `file_key` / `image_key` 配 `message_id` 走『获取消息中的资源文件』下载；发送文件先『上传文件』再『发送消息』。」 |
| **官方文档怎么说** | 下载：`GET /open-apis/im/v1/messages/:message_id/resources/:file_key`，`type` 必填、只有两个值：`image`（图片、富文本里的图片）/ `file`（文件、音频、视频，**不含表情包**）。<br>https://open.feishu.cn/document/server-docs/im-v1/message/get-2?lang=zh-CN<br>上传文件：`POST /open-apis/im/v1/files`，multipart，`file_type` ∈ `opus` `mp4` `pdf` `doc` `xls` `ppt` `stream`，`file_name` 必填，「文件大小不得超过 30 MB，且不允许上传空文件」。<br>https://open.feishu.cn/document/server-docs/im-v1/file/create?lang=zh-CN<br>上传图片：`POST /open-apis/im/v1/images`，`image_type` ∈ `message` `avatar`，最大 10 MB。<br>https://open.feishu.cn/document/server-docs/im-v1/image/create?lang=zh-CN |
| **我们的代码怎么做的** | `platform.py:440`（下载 `type` 判定）、`platform.py:316-333`（上传两步）、`platform.py:68`（`_FILE_TYPE_BY_EXT`） |
| **结论** | **一致**。`_FILE_TYPE_BY_EXT` 的取值集合与官方 7 个值逐字对上（docx→doc、xlsx→xls、pptx→ppt，认不出落 `stream`）；`image_type="message"` 也在合法值里。一处差异只写进报告，见下。 |

---

## 7 · 群历史

| 栏 | 内容 |
|---|---|
| **附录 A 怎么说** | 「『获取会话历史消息』（`container_id_type=chat`），按需分页。」 |
| **官方文档怎么说** | `GET /open-apis/im/v1/messages`。`container_id_type` ∈ `chat` / `thread`（必填），`container_id` 必填；`sort_type` ∈ `ByCreateTimeAsc` / `ByCreateTimeDesc`，**默认 Asc**；`page_size` 取值 1–50，**默认 20**；`page_token` 首次不填；响应里同时有 `has_more` 和 `page_token`。<br>**权限**：`im:message` / `im:message:readonly` / `im:message.history:readonly` 三者之一；**群消息「应用还必须开启 获取群组中所有消息（im:message.group_msg） 权限」**。<br>https://open.feishu.cn/document/server-docs/im-v1/message/list?lang=zh-CN |
| **我们的代码怎么做的** | `platform.py:382-386`（查询参数）、`platform.py:76`（`_HISTORY_PAGE_SIZE = 50`，正好是上限） |
| **结论** | 参数名、取值、分页字段**全部一致**。所需权限就是 §3.7 (b) 的答案，见下一节。 |

一处**文档页字段表没列、但 SDK 有**的：`platform.py:386` 传的 `with_sender_name`。
文档的参数表里没有它，但 SDK 的 `ListMessageRequest`
（`lark_oapi/api/im/v1/model/list_message_request.py:11-20`）明确带
`with_sender_name` 和 `only_thread_root_messages` 两个查询参数，
`Sender` 模型（`.../model/sender.py:20-26`）也带 `sender_name` / `open_bot_id` /
`sender_i18n_names`。SDK 是官方 OpenAPI 描述的产物，可信度高于文档页的字段表，
**保留不动**；但真机上 M5 要顺手确认 `sender_name` 真回填了 —— 若没有，
W1 的 `[message_id] 姓名: 文本` 会退化成没有姓名。

---

## 8 · 集群

| 栏 | 内容 |
|---|---|
| **附录 A 怎么说** | 「同一应用多副本长连接只有一个收到事件 → P0 只跑单副本。」 |
| **官方文档怎么说** | 「长连接模式的消息推送为 **集群模式**，不支持广播，即如果同一应用部署了多个客户端（client），那么只有 **其中随机一个** 客户端会收到消息。」另有一条：「每个应用最多建立 **50 个连接**（在配置长连接时，每初始化一个 client 就是一个连接）」。<br>https://open.feishu.cn/document/server-docs/event-subscription-guide/event-subscription-configure-/request-url-configuration-case?lang=zh-CN |
| **我们的代码怎么做的** | 无代码面 —— 这条是部署结论（P0 单副本） |
| **结论** | **一致**，连措辞都对得上（而且是「随机一个」，不是「固定某个」，所以多副本下事件分布不可预测，P0 单副本的结论成立）。50 连接上限是新知，P0 用不到。 |

---

## §3.7 两个待核实项

### (a) 只有 @ 权限时，话题里不带 @ 的回复是否投递 —— **查不到，只能真机验**

翻了三处都没有覆盖：

* 「接收消息」事件文档只列了可订阅的权限集合（「开启任一权限即可订阅」），
  没写各权限之间投递范围的差别，更没提话题。
  https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN
* 消息常见问题 FAQ：只说「拥有 获取用户在群组中@机器人的消息（im:message.group_at_msg）
  或 接收群聊中@机器人消息事件（im:message.group_at_msg:readonly）权限时，
  可以接收群聊中 @ 机器人的消息」——「@ 机器人的消息」这句本身就是待解释的那个词。
  https://open.feishu.cn/document/server-docs/im-v1/faq?lang=zh-CN
* 机器人相关 FAQ：不涉及话题内的投递行为。
  https://open.feishu.cn/document/faq/bot?lang=zh-CN

**为什么只能真机验**：这是一条平台**投递侧**的行为（消息压根不会推到长连接上），
不是接口的返回值。`preflight.py` 说的「光靠凭证问不出来」就是这个意思 ——
没有任何 API 能回答「这条我没收到的消息，是因为权限还是因为别的」。
唯一的观测手段是在真实群里发一条，然后看长连接上有没有帧。

**M4 该怎么设计这个实验**（建议给总管）：

1. **只勾 @ 权限**（`im:message.group_at_msg:readonly`），**先别勾**
   「获取群组中所有消息」。这是实验的前提，勾了就测不出来了。
2. 进程起来后开 `DEBUG` 日志（`connection.py` 的 `_handle` 每收一帧都会走到）。
3. 依次做三个动作，每个之间隔 ≥5 秒好对齐时间戳：
   * **A** 群里顶层 @Aite 发一句 → 期望收到事件（这条是对照组，证明订阅是通的）；
   * **B** 在 A 那条的**话题里**回一句，**不带 @** → 这就是待测项；
   * **C** 在群里另起一条**不带 @** 的普通消息 → 期望**收不到**（另一个对照组，
     证明「不带 @ 就收不到」这个基线成立）。
4. 判读：
   * A 收到、C 收不到 → 实验有效，可以看 B。
   * **B 收到** → `supports_passive_listen` 运行时置 **True**
     （调 `FeishuPlatform.set_passive_listen(True)`，**不要动 `FEISHU_P0` 常量**，
     它是冻结的保守取值）；§3.5 R6「话题内不要求 mentioned」在只有 @ 权限时也成立。
   * **B 收不到** → 维持 `False`。R6 要落地就必须补
     「获取群组中所有消息」权限，否则话题追问全部丢失（M4 会当场暴露）。
   * A 也收不到 → 不是这条待核实项的问题，先查事件订阅配置和长连接是否建上。
5. 无论结果如何，把 B 的原始帧（或「没有帧」）贴进 M4 的记录里 —— 这条结论
   将来还会被人问第二次。

### (b) 「获取会话历史消息」是否要「获取群组中所有消息」敏感权限 —— **要**

官方文档在「获取会话历史消息」的权限要求里写得很直白：
除了 `im:message` / `im:message:readonly` / `im:message.history:readonly` 三者取其一之外，
**群消息「应用还必须开启 获取群组中所有消息（`im:message.group_msg`）权限」**。

https://open.feishu.cn/document/server-docs/im-v1/message/list?lang=zh-CN

**影响**：M5（汇总本群本周开放事项）走的就是 `read_history` → 不补这个权限会 403。
请总管在申请时**一定勾上**，别只勾三个基础的。

顺带一个巧合但有用的结论：(b) 要申请的这个权限，正好就是 (a) 里
「B 收不到时的补救办法」。也就是说 **M5 需要的权限本来就要申请**，
申请之后 (a) 大概率也不再是问题 —— 但 (a) 的实验必须**在补这个权限之前**做，
否则就永远问不出「只有 @ 权限时」的答案了。**这条决定了 M4 必须排在权限补齐之前。**

---

## 权限申请清单（总管照这个勾）

后台名称是文档「权限要求」一栏里的原文。同一行里 `/` 隔开的是「任选其一」。

| 用途 | 后台里的确切名称 · scope 标识 | 哪个 API / 事件用它 | URL |
|---|---|---|---|
| 收群里 @ 机器人的消息 | 接收群聊中@机器人消息事件 · `im:message.group_at_msg:readonly`<br>或 获取用户在群组中@机器人的消息 · `im:message.group_at_msg` | 事件 `im.message.receive_v1` | https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN |
| **读群历史（必勾）** | **获取群组中所有消息 · `im:message.group_msg`** | 「获取会话历史消息」在**群**容器下的硬性附加权限；同时也是事件订阅的可选项之一 | https://open.feishu.cn/document/server-docs/im-v1/message/list?lang=zh-CN |
| 读群历史（基础） | 获取与发送单聊、群组消息 · `im:message`<br>或 获取单聊、群组消息 · `im:message:readonly`<br>或 获取单聊、群组的历史消息 · `im:message.history:readonly` | 「获取会话历史消息」`GET /open-apis/im/v1/messages` | https://open.feishu.cn/document/server-docs/im-v1/message/list?lang=zh-CN |
| 发消息 / 回复消息 | 获取与发送单聊、群组消息 · `im:message`<br>或 以应用的身份发消息 · `im:message:send_as_bot`<br>或 发送消息V2 · `im:message:send` | 「发送消息」`POST /open-apis/im/v1/messages`、「回复消息」`.../:message_id/reply` | https://open.feishu.cn/document/server-docs/im-v1/message/reply?lang=zh-CN |
| 更新卡片 | 获取与发送单聊、群组消息 · `im:message`<br>或 以应用的身份发消息 · `im:message:send_as_bot`<br>或 更新消息 · `im:message:update` | 「更新应用发送的消息卡片」`PATCH /open-apis/im/v1/messages/:message_id` | https://open.feishu.cn/document/server-docs/im-v1/message-card/patch?lang=zh-CN |
| 表情回复 | 获取与发送单聊、群组消息 · `im:message`<br>或 发送、删除消息表情回复 · `im:message.reactions:write_only` | 「添加消息表情回复」`POST /open-apis/im/v1/messages/:message_id/reactions` | https://open.feishu.cn/document/server-docs/im-v1/message-reaction/create?lang=zh-CN |
| 下载消息里的文件 | 获取与发送单聊、群组消息 · `im:message`<br>或 获取单聊、群组消息 · `im:message:readonly`<br>或 获取单聊、群组的历史消息 · `im:message.history:readonly` | 「获取消息中的资源文件」`GET .../:message_id/resources/:file_key` | https://open.feishu.cn/document/server-docs/im-v1/message/get-2?lang=zh-CN |
| 上传文件 / 图片 | 获取与上传图片或文件资源 · `im:resource`<br>或 上传文件V2 · `im:resource:upload` | 「上传文件」`POST /open-apis/im/v1/files`、「上传图片」`POST /open-apis/im/v1/images` | https://open.feishu.cn/document/server-docs/im-v1/file/create?lang=zh-CN |
| 读云文档 | 查看新版文档 · `docx:document:readonly`<br>或 创建及编辑新版文档 · `docx:document` | 「获取文档基本信息」`GET /open-apis/docx/v1/documents/:document_id`、「获取文档纯文本内容」`.../raw_content` | https://open.feishu.cn/document/server-docs/docs/docs/docx-v1/document/raw_content?lang=zh-CN |
| 读知识库节点 | 查看知识空间节点信息 · `wiki:node:read`<br>或 查看知识库 · `wiki:wiki:readonly`<br>或 查看、编辑和管理知识库 · `wiki:wiki` | 「获取知识空间节点信息」`GET /open-apis/wiki/v2/spaces/get_node`（wiki 链接换 docx token） | https://open.feishu.cn/document/server-docs/docs/wiki-v2/space-node/get_node?lang=zh-CN |

**最省事的勾法**：`im:message` 一个就覆盖了发/回/更新/表情/读历史基础/下载六格，
再单独补 **`im:message.group_msg`**（群历史必需）、`im:resource`、
`docx:document:readonly`、`wiki:node:read`，以及事件订阅那条
`im:message.group_at_msg:readonly`。

**注意 §3.7 (a) 的时序**：M4 那个实验要在**只有 @ 权限**的状态下做。
所以建议先勾 `im:message.group_at_msg:readonly` 跑 M4，记下结论，
再一次性补齐上表其余的跑 M1/M2/M3/M5/M6。

---

## 已改的代码（每条挂 URL）

| 文件:行 | 改了什么 | 依据 |
|---|---|---|
| `aite/adapters/feishu/connection.py:177` | 新增 `route_card_frames_as_events()`：把长连接上的 CARD 帧 type 头改写成 event 再交回 SDK。不接的话 lark-oapi 1.7.3 直接 `return` 丢掉，卡片按钮全哑 | SDK `ws/client.py:340-344`（本机可查）＋ 回调可用长连接接收：https://open.feishu.cn/document/event-subscription-guide/callback-subscription/callback-overview?lang=zh-CN |
| `aite/adapters/feishu/connection.py:151` | `connect()` 里建完 client 就装上上面那层 | 同上 |
| `aite/adapters/feishu/normalize.py:77,100` | `_to_datetime` → 公开的 `to_datetime`，并**按量级判毫秒/微秒**（≥1e14 当微秒）。原来一律除 1000，遇到微秒时间戳会算出五万年后的秒数，`fromtimestamp` 当场抛，而 `_dispatch_raw` 没兜底 → 整条卡片回传事件被丢 | 文档自相矛盾，两种都得吃：信封示例 `1603977298000000`（16 位微秒）https://open.feishu.cn/document/server-docs/event-subscription-guide/overview?lang=zh-CN ；卡片回调示例同为 16 位 https://open.feishu.cn/document/feishu-cards/card-callback-communication?lang=zh-CN ；而 `im.message.receive_v1` 示例是 `1608725989000`（13 位毫秒）且 `message.create_time` 明写「毫秒」 https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN |
| `aite/adapters/feishu/normalize.py:222`（`_flatten_post` 的 at 分支） | at 段原样吐 `@_user_N` 占位符，剥离/改名统一交给 `apply_mentions` | https://open.feishu.cn/document/server-docs/im-v1/message-content-description/message_content?lang=zh-CN |
| `aite/adapters/feishu/normalize.py:364`（`_post_mentions_bot`） | 拿占位序号回 `mentions` 查身份（原来直接跟 open_id 比，恒为 False）；保留比 open_id 的老路径 | 同上 |
| `aite/adapters/feishu/normalize.py:35-50` | `_SENDER_KIND` 注释改成「两套枚举」的准确说法 | https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN ＋ https://open.feishu.cn/document/server-docs/im-v1/message/get?lang=zh-CN |
| `aite/adapters/feishu/platform.py:65` | `REACTION_EMOJI["ack"]`：`EYES` → `OnIt`。`EYES` **不在**消息表情回复的合法清单里（清单里有的是云文档高亮块那套小写 `eyes`，两套枚举不通用），发上去平台回 `231001 表情类型不合法` → R7 的每一次 ack 都会失败 | 清单：https://open.feishu.cn/document/server-docs/im-v1/message-reaction/emojis-introduce?lang=zh-CN ；错误码：https://open.feishu.cn/document/server-docs/im-v1/message-reaction/create?lang=zh-CN |
| `aite/adapters/feishu/platform.py:54` | 新增 `KNOWN_EMOJI_TYPES`，把清单里确实存在的几个 key 固化下来供测试兜底 | 同上 |
| `aite/adapters/feishu/platform.py:509`（`_history_created_at`） | 改用 `to_datetime`，跟事件面共用一套单位判定 | 同上（时间戳那条） |
| `tests/fixtures/feishu/card_action_stop.json` | `header.create_time` 13 位 → 16 位微秒，照官方示例的量级 | https://open.feishu.cn/document/feishu-cards/card-callback-communication?lang=zh-CN |
| `tests/fixtures/feishu/message_from_bot.json` | 事件里的 `sender_type` `app` → `bot` | https://open.feishu.cn/document/server-docs/im-v1/message/events/receive?lang=zh-CN |
| `tests/fixtures/feishu/message_post_with_image.json` | at 段 `user_id` 改成 `@_user_1`，并补上 `message.mentions` | https://open.feishu.cn/document/server-docs/im-v1/message-content-description/message_content?lang=zh-CN |
| 三个 `.expected.json` | 随上面的 fixture 重生成（`text` 未变；post 的 `raw_text` 改为占位符口径） | — |
| `tests/adapters/feishu/test_feishu_card_frames.py`（新） | 3 条：SDK 现状、转接后能到 `on_raw`、事件帧不受影响 | — |
| `tests/adapters/feishu/test_normalize.py` | 新增 6 条：时间戳单位（4 个参数化）、卡片回传微秒不炸、post at 三条 | — |
| `tests/adapters/feishu/test_feishu_outbound.py` | 表情断言从「非空」升级为「必须在官方清单里」，另加一条禁止 `EYES` 回潮 | — |

契约面一个字没动：`aite/contracts/**`、`.contracts.lock` 保持
`OK 11 files`；`FEISHU_P0.supports_passive_listen` 仍是冻结的 `False`，
真机结论走 `FeishuPlatform.set_passive_listen()` 的运行时路径（`platform.py:148`）。

---

## 只写进报告、没动代码的差异

1. **表情包（sticker）附件下不下得来** ——
   `normalize.py:63` 把 `msg_type=sticker` 归成 `kind="image"` 的附件，
   但「获取消息中的资源文件」的 `type` 说明里，`file` 一档明确写着**不含表情包**，
   `image` 一档说的是「图片、富文本里的图片」。也就是说表情包很可能压根下不下来。
   https://open.feishu.cn/document/server-docs/im-v1/message/get-2?lang=zh-CN
   **为什么不动**：改法有两种（附件表里去掉 sticker／下载时特判），
   哪种对取决于平台实际返回什么错误码，而这个只有真机能答。P0 的六个场景
   都不涉及表情包，现在改属于凭猜测动归一化输出。

2. **`with_sender_name` 与 `sender_name`** ——
   文档页的参数表 / 响应字段表都没列，SDK 里有（见第 7 节）。
   **为什么不动**：两个证据源打架时，SDK 是 OpenAPI 描述的产物、可信度更高，
   而且删掉它只会让 `HistoryMessage.sender_name` 铁定为空，比现在更差。
   真机 M5 顺手确认即可。

3. **`GLANCE` 到底是不是 👀** ——
   spec 里 M1 要的是「👀 类表情」。合法清单里 `GLANCE` 和 `OnIt` 都在，
   但那份文档只给表情图片不给中文文案，光看文本判不出哪个是 👀。
   现在选了 `OnIt`（语义 = 收到/正在处理，正对 R7 的 ack）。
   **为什么不动更多**：换成 `GLANCE` 也是合法的，纯属观感取舍 ——
   真机上点一次两个都试出来，再定；改的话只动 `REACTION_EMOJI` 一行。

4. **卡片 schema v2** —— v2 有流式更新等新能力，但要客户端 ≥7.20，
   低版本只显示兜底升级提示。P0 继续用 v1（默认值，兼容面最宽）。
   **为什么不动**：这是产品取舍不是 bug，且 v2 会把 `elements` 挪进 `body`，
   `cards.py` 要整体重写，没有任何 P0 判据要求它。

5. **回调 3 秒同步窗口 / 无补推** —— §3.2 已经要求 `on_event` 1s 内返回，
   比 3 秒更严，代码不用改。记进报告是因为「没有补推」这条影响排障心态：
   卡片回传丢了就是永久丢了，不会像事件那样重推。

---

## 文档里查不到的（老实列出来）

1. **§3.7 (a)**：只有 @ 权限时话题内不带 @ 的消息是否投递。翻了接收消息事件文档、
   消息 FAQ、机器人 FAQ 三处，都只说「可以接收群聊中 @ 机器人的消息」，
   没有定义话题场景下这句话的边界。**只能真机验，M4 实验设计见上。**
2. **卡片回传在长连接上用的帧类型**：文档只讲「可以用长连接接收回调」，
   没写帧的 `type` 头是 `card` 还是 `event`。SDK 里有 `MessageType.CARD` 分支
   说明至少存在这种可能，我们的补丁两种情况都安全，但**真机能一次问清**。
3. **`header.create_time` 的权威单位**：官方文档自相矛盾（16 位微秒 vs 13 位毫秒，
   见第 5 节旁边那条改动的依据）。没找到任何一处说明「哪种事件用哪种单位」，
   所以代码按量级判，没有押注任何一边。
4. **`with_sender_name` / `sender.sender_name`**：SDK 有，文档页字段表没有。
   没找到说明它是新参数、废弃参数，还是文档漏写。
5. **`GLANCE` / `OnIt` 的中文文案**：「表情文案说明」是一张图片 + key 的表，
   没有文字描述，无法从文本判断哪个渲染成 👀。
6. **`PATCH /messages/:message_id` 是否接受卡片 JSON v2**：文档只说
   「支持卡片 JSON 或搭建工具生成的卡片」，没有按版本区分。我们用 v1，不受影响。

---

## 复核方式

这份文档里的每条「一致」都能离线复跑：

```bash
.venv/bin/python -m pytest tests/adapters -q          # 146 passed
.venv/bin/python -m pytest tests/adapters/feishu/test_normalize.py -q   # B1
.venv/bin/python -m pytest -q                         # 981 passed
scripts/check.sh                                      # 全部通过
```

SDK 交叉验证用到的文件（都在 `.venv/lib/python3.12/site-packages/lark_oapi/` 下）：

* `ws/client.py:340-344` —— CARD 帧被丢弃的那三行
* `ws/enum.py:9-13` —— `MessageType` 四个值
* `event/dispatcher_handler.py:145-176` —— `event_key` 怎么拼、查哪张表
* `api/im/v1/model/list_message_request.py` —— 群历史的 10 个查询参数
* `api/im/v1/model/sender.py` / `event_sender.py` —— 两套 sender 形状
* `api/im/v1/model/{patch,reply,create}_message_request.py` —— 方法与路径
* `api/im/v1/model/{create_file,create_image}_request_body.py` —— 上传的表单字段
