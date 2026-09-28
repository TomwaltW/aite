# 契约 p1.0 设计（T0 起草 · 冻结规格，总管 H11 审过才冻）

> 起草：T0 云端轨（2026-09-28），代码基线 `8458435`（CC1–CC4 已合并；P0-CLOSE 尚未上 main，补丁按「B0 + AA4 + BB2 + BB4」的文本锚定）。
> 落地：总管在 W1 合并、P0-CLOSE 上 main 之后，按 `review/t0/APPLY.md` 在分支 `contract/p1.0` 上跑 `review/t0/p1-contract-patch.py`，
> `make proto-gen`，用补丁前的二进制重锁（H11）。伴随轨 T0c（W2a）让整个工作区编得过。
> **本文就是 p1.0 的规格**：补丁脚本 `review/t0/data/*.txt` 里的代码是它的逐字实现；两者不一致时以本文为准，并由 T0.1 勘误。
> 来源：`review/p1/tracks-2026-09-25.json` 的 `contract_batches[T0-p1.0]`（权威清单）+ [REVISION 2026-09-25 / 09-25b] + 派单 `review/paste-T0.md` §5 ④–⑩（W1 已合并各轨回执报的契约缺口）。

## 0. 总则

- **版本**：`CONTRACT_VERSION` `p0.2 → p1.0`（`contracts/src/lib.rs`）与 edge `server.go` 的 `ContractVersion` 同步改；两边不等 core 拒绝起飞（行为不变）。
- **兼容**：所有新字段 `#[serde(default)]`（`Map` 类的另加 `skip_serializing_if = "Map::is_empty"` 的只有 `Message.provider_extra` 与 `Turn.provider_extra`）。
  **旧 JSON / P0 的 SQLite 行照样反序列化**（`tests/roundtrip.rs::p0_json_without_p1_fields_still_loads` 钉）。
- **不动**：`ACTIVE_TASK_STATUSES`（store 按下标 `[0..2]` 绑它，孤儿恢复绝不能收待审批任务）、`Task`、`EvidenceKind`（仍 10 个）、
  `all_model_tools()`（仍 10 个；新工具走 CC4 的网关注册表）、`protocol_tools.rs` / `task_no.rs` / `evidence_vectors.rs` 三份测试、contracts `Cargo.toml`（不加依赖）。
- **锁面**：25 → **26** 个文件（唯一新文件 `core/crates/contracts/src/domain.rs`）。补丁一共动 **25 个文件、新建其中 1 个**（锁面 21 + 锁面外 4：`convert.rs`、`tests/convert.rs`、`server.go`、`aite.example.yaml`）。
- **proto 形态**：只加字段 / 枚举值 / 消息 / RPC，字段号不复用；`SandboxSpec.network`、`ExecRequest.language`、`EgressPolicy.level` 仍是 `string`（取值见 §4）。

## 1. 平台与配置（`config.rs` · `config/aite.example.yaml`）

一部署一个 IM 平台：`EdgeConfig` 仍单一，`platform` 选哪个就读哪一段。所有段 `#[serde(default, deny_unknown_fields)]`，样例 == 默认（`example_yaml_equals_defaults` 整体相等）。

| 段 / 类型 | 字段 = 默认值 | 服务 |
|---|---|---|
| `PlatformChoice` | `+= dingtalk, wecom`（`slack` 等仍拒） | CT20 / NEW16（DD8、DD11） |
| `FeishuConfig` += | `api_base = "https://open.feishu.cn"`、`card_buttons = false`、`service_user_token_env = ""` | CC8 的 `AITE_FEISHU_{API_BASE,CARD_BUTTONS}` 转正；消息搜索要 user token（§1.3 of plan） |
| `DingtalkConfig`（新） | `client_id_env = "DINGTALK_CLIENT_ID"`、`client_secret_env = "DINGTALK_CLIENT_SECRET"`、`robot_code_env = "DINGTALK_ROBOT_CODE"`、`card_template_id_env = "DINGTALK_CARD_TEMPLATE_ID"`、`api_base = "https://api.dingtalk.com"`、`bot_name = "Aite"` | CC9 包内镜像逐字同名 |
| `WecomConfig`（新） | `bot_id_env = "WECOM_BOT_ID"`、`bot_secret_env = "WECOM_BOT_SECRET"`、`ws_url = "wss://openws.work.weixin.qq.com"`、`corp_id_env = ""`、`app_secret_env = ""`、`bot_name = "Aite"` | CC10 包内镜像逐字同名 |
| `ModelVendor`（新 str_enum） | `generic, deepseek, qwen, glm, kimi, doubao, minimax, selfhost`（8 个） | CT30 / D9（DD7） |
| `ThinkingMode`（新 str_enum） | `auto, on, off` | DD7 |
| `ModelConfig` += | `name = "primary"`、`display_name = ""`（空 = 用 `model`；卡片页脚与 `!about`）、`filing_no = ""`（**这个模型服务**的备案号 / 上线编号，catalog 每项各带各的）、`vendor = generic`、`thinking = auto`、`timeout_sec = 600`、`stream = false`、`price_cached_in_per_mtok = 0.0`、`price_cache_write_per_mtok = 0.0`、`offpeak_price_multiplier = 1.0`、`allow_overseas_endpoint = false` | CT26 / CT30 / D9 |
| `ModelsConfig`（新，`AiteConfig.models`） | `fallback = ""`、`fast = ""`（都按 `catalog[].name` 引用；空 = 不用）、`catalog = []` | DD7 ModelRouter |
| `SandboxConfig` += | `network_default = none`、`proxy_url = ""`、`trusted_preset = "china"`、`extra_trusted_hosts = []`、`allow_full = false`、`linger_sec = 600` | CT10 / CT17 / NEW11（DD6、DD8） |
| `WorkerConfig` += | `max_parallel_tasks = 4`、`context_max_tokens = 96000`、`approval_timeout_sec = 1800`、**`stuck_after_sec = 900`**（缺口 ⑤） | CT01 / CT04 / CT13（T0c、DD4、EE7、CC2 ④） |
| `EdgeConfig` += | `http_listen = ""`、`webhook_secret_env = "AITE_WEBHOOK_SECRET"` | EE11 |
| `SearchConfig`（新） | `provider = none`（`SearchProvider {none, bocha, zhipu, baidu}`）、`api_key_env = "AITE_SEARCH_API_KEY"`、`endpoint = ""`、`max_results = 5`、`timeout_sec = 15` | CT21（EE4） |
| `GitConfig`（新） | `provider = none`（`GitProvider {none, gitlab, gitee, codeup, gitea, github}`）、`base_url = ""`、`token_env = "AITE_GIT_TOKEN"`、`bot_name = "aite-bot"`、`draft_prefix = "Draft: "`、`webhook_secret_env = "AITE_GIT_WEBHOOK_SECRET"` | CT22（EE5） |
| `PagesConfig`（新） | `provider = none`（`PagesProvider {none, platform_doc}`）、`folder_token_env = ""` | CT23（DD10、EE6） |
| `MemoryConfig`（新） | `enabled = true`、`max_entries_per_scope = 200`、`auto_extract = false` | CT16（EE1） |
| `RoutinesConfig`（新） | `enabled = true`、`max_per_chat = 20`、`min_interval_min = 15`、`default_tz = "Asia/Shanghai"`、`default_tz_offset_min = 480`、`tick_sec = 30` | CT12 / CT15（EE2） |
| `AmbientConfig`（新） | `respond_automatically_default = false`（**自动回复默认关**）、`daily_cap_per_chat = 20`、`stop_reading_after_msgs = 100`、`channel_session_idle_min = 60`、`channel_session_max_age_h = 24`、`stalled_followup_h = 24`、`max_consecutive_bot_turns = 3` | CT07 / CT08（FF1） |
| `AccessConfig`（新） | `dm_enabled = true`、`external_chat_mode = restrict`、`allow_chats = []`、`block_chats = []`、`member_allowlist = []`、`search_scope = joined_chats`、`require_setup = false` | CT28 / NEW06 / NEW08（EE8） |
| `BudgetConfig`（新，人民币） | `org_monthly_limit_cny = 0.0`（0 = 不限）、`channel_monthly_limit_cny = 0.0`、`task_limit_cny = 0.0`、`warn_ratio = 0.8`、`month_start_day = 1` | CT26（EE3） |
| `ComplianceConfig`（新） | `aigc_label = true`、`label_text = "内容由 AI 生成"`、`implicit_metadata = true`、`producer = "Aite"`、`service_filing_no = ""`（Aite 自己的服务备案，模型的编号在 `ModelConfig.filing_no`）、`audit_retention_days = 190`、`content_retention_days = 0`、`registration_scope_confirmed = false`、`content_safety_filter = false`（后两个给 D25：EE8 只有两者都为 true 才允许 `external_chat_mode` 离开 `restrict`） | NEW10 / NEW21 / D25（DD5、EE8、EE12） |
| `AdminConfig`（新） | `listen = ""`、`public_base_url = ""`、`owner_ids = []`、`session_secret_env = "AITE_ADMIN_SECRET"`、`bootstrap_token_env = "AITE_ADMIN_BOOTSTRAP_TOKEN"` | CT28（DD12） |
| `AiteConfig` += 13 段 | `dingtalk, wecom, models, search, git, pages, memory, routines, ambient, access, budget, compliance, admin`（字段名 = yaml 段名） | — |

**模型厂商规则（D9，DD7 实现）**：

1. 显式 `vendor`（非 `generic`）**永远优先**于 CC6 的模型名 / 端点推断；`generic` 才走推断。
2. `selfhost` = 客户自托管的 vLLM / SGLang / MindIE：**不发任何云厂商专有参数**（没有 `enable_thinking` / `thinking` / `reasoning_split` 这类），思考开关只走 `chat_template_kwargs`。
3. `allow_overseas_endpoint` 默认 `false`：`base_url` 指到海外端点（`dashscope-intl`、`api.moonshot.ai`、`api.z.ai`、`api.minimax.io` 等）是**配置错**（构造时 `ModelError::Config`）；显式打开才降为告警。
4. `ModelsConfig.fallback` / `fast` 必须能在 `catalog[].name` 里找到（DD7 构造时校验，找不到 = 配置错；契约只给形状）。

Go 侧：`edge/internal/config` 用宽松 `yaml.Unmarshal`，新段被忽略；`Validate` 仍只认 `feishu|fake`——翻它是 **DD8**。

## 2. 事件与出站（`events.rs` / `outbound.rs` · `events.proto` / `outbound.proto`）

| 类型 | 改动（Rust 字段 = proto 字段号） | 服务 |
|---|---|---|
| `EventKind` | `+= Reaction "reaction" (EVENT_KIND_REACTION = 7)`、`External "external" (= 8)` | NEW02 / CT12（T0c 把 Reaction 放进 R4 只计数） |
| `CardActionKind` | `+= Approve (3)、Reject (4)、Submit (5)` | CT13（EE7；T0c 先记日志丢弃） |
| `Quote`（新） | `message_id: Option<String> (1)`、`sender_id: Option<String> (2)`、`text: String (3)` | CT06 / DD3 锚点解析 |
| `ReactionEvent`（新） | `message_id (1)`、`emoji (2)`、`thumbs_down: bool (3)`、`added: bool (4)` | NEW02（EE9 静音） |
| `ExternalEvent`（新） | `source (1)`、`topic (2)`、`subject (3)`、`url: Option (4)`、`summary (5)`、`payload: Map ↔ Struct (6)`（**开放通道**） | CT12（EE11） |
| `NormalizedEvent` += | `quote: Option<Quote> (19)`、`reaction: Option<ReactionEvent> (20)`、`external: Option<ExternalEvent> (21)`、`sender_external: bool (22)`，全部 `serde(default)` | CT06 / NEW06 |
| `Anchor.task_no` | 形状不变；注释改为「edge 从自身文本或引用里解析 `#A`；core 按它路由」（proto 同步） | CT06（CC9/CC10 已往里写 `#A`，DD3 路由） |
| `ReactionKind` | `+= Muted "muted" (REACTION_KIND_MUTED = 4)` | NEW02 |
| `CardLink`（新） | `label (1)`、`url (2)` | NEW19（DD5） |
| `ApprovalPrompt`（新） | `approval_id (1)`、`summary (2)`、`detail: Option (3)` | CT13（EE7） |
| `ChecklistCard` += | `links: Vec<CardLink> (10)`、`approval: Option<ApprovalPrompt> (11)` | — |
| `OutboundText` += | `mentions: Vec<String> (5)`（要 @ 的平台用户 id）、`dedupe_key: Option<String> (6)`；`OutboundText::new` 同步给空值 | DD10 |
| `HistoryMessage` += | `updated_at: Option<DateTime> (8)`、`deleted: bool (9)`、`reply_to: Option<String> (10)` | EE13 拉取式编辑 |
| `UserInfo`（新） | `user_id (1)`、`name (2)`、`is_external: bool (3)`、`is_guest: bool (4)` | CT28 / EE7 |
| `ChatInfo`（新） | `chat_id (1)`、`name (2)`、`chat_type: ChatType (3)`、`is_public: Option<bool> (4)`、`is_external: bool (5)`、`member_count: Option<u32> (6)` | NEW06 / NEW08 |
| `SearchQuery`（新） | `query (1)`、`chat_ids: Vec (2)`、`limit: u32 (3)`、`since: Option<DateTime> (4)` | CT21 |
| `SearchHit`（新） | `chat_id (1)`、`message: HistoryMessage (2，必填)`、`url: Option (3)` | CT21 |
| `DocWrite`（新） | `doc_id: Option (1)`（有 = 更新同一页）、`title (2)`、`markdown (3)`、`share_chat_id: Option (4)`、`aigc_banner: bool (5)` | CT23 / NEW10 |
| `DocRef`（新） | `doc_id (1)`、`url (2)`、`title (3)` | CT23 / NEW23 |
| `OAuthStart` / `OAuthUrl` / `OAuthCode`（新） | `{redirect_uri (1), state (2)}` / `{url (1)}` / `{code (1), redirect_uri (2)}` | CT28（DD12 管理台登录） |

## 3. 能力位（`capabilities.rs` · `capabilities.proto` 字段 10–23）

新增 14 个（9 bool + 4 u32 + 1 u64），Rust 侧全部 `serde(default)`。**数值 0 = 「未声明」**，调用方按保守值处理。

| # | 字段 | `feishu_p0()` | `dingtalk_v1()` | `wecom_v1()` |
|---|---|---|---|---|
| 10 | `supports_reactions_in` | true | false | true（只有 `feedback_event` 的赞 / 踩） |
| 11 | `supports_recall_event` | true | false | false |
| 12 | `supports_edit_event` | false（飞书没有编辑事件，EE13 拉取比对） | false | false |
| 13 | `supports_pins` | true | false | false |
| 14 | `supports_message_search` | false（要 service-user token） | false | false |
| 15 | `supports_docs` | true | false | false |
| 16 | `supports_dm` | true | true | true |
| 17 | `supports_quote_id` | true | false | false（引用不带 message id） |
| 18 | `requires_visible_anchor` | false | true | true |
| 19 | `stream_max_sec` (u32) | 600（CardKit 流式 10 分钟自动关） | 0 | 600 |
| 20 | `card_action_deadline_ms` (u32) | 3000 | 2000 | 5000 |
| 21 | `max_card_bytes` (u32) | 30720 | 0 | 0 |
| 22 | `max_text_chars` (u32) | 0 | 0 | 0 |
| 23 | `max_upload_bytes` (u64) | 31457280（30 MB） | 0 | 20971520（20 MB） |

旧 9 个：`feishu_p0()` **一个不改**；`dingtalk_v1()` = `thread/history/passive false、card_edit true、card_edit_window_sec 0、inbound_file_in_group false、proactive_requires_prior_message false、outbound_rate_per_min 20`（与 CC9 包内写死的保守值逐字一致）；
`wecom_v1()` = `thread/history/passive false、card_edit false、card_edit_window_sec 0、inbound_file_in_group false、proactive_requires_prior_message true、outbound_rate_per_min 30`（与 CC10 一致）。
取值依据：原卡 + 总计划 §1.3；标 0 的都是「云端查不到官方数字」，DD11 / H9 实测后由 T0.1 更新。

## 4. 沙箱与出网（`sandbox.rs` · `sandbox.proto` · `edge.proto`）

| 类型 | 改动 | 服务 |
|---|---|---|
| `SandboxNetwork` | `none | trusted | custom | full`（proto 仍是 string；空串 = none；其余拒） | CT17 / CT18 / NEW11 / NEW12 |
| `ExecLanguage` | `python | bash`（`ExecRequest::bash(code, timeout)` 便捷构造） | DD6 `run_shell` |
| `CredentialBinding`（新） | `host (1)`、`header (2)`、`scheme (3)`、`secret_ref (4)`——**只是名字**，取值由 edge 持有，永不过 gRPC | EE10（CC11 `CredentialBinding` 逐字同形） |
| `EgressPolicy`（新） | `level: SandboxNetwork (1, string)`、`allow_hosts: Vec (2)`、`credentials: Vec<CredentialBinding> (3)`、`audit_tag (4)`；`Default` = `level none`、其余空 | CC11 `Policy{Level, AllowHosts, AuditTag, Credentials}` |
| `AcquireRequest` += | `EgressPolicy egress = 3`（缺省 / 空 = none） | DD6 / DD8 |
| `EdgeStatus` += | `bool egress_ok = 7`、`bool http_ingress_ok = 8`；`platform` 注释补 `dingtalk / wecom` | DD8 / EE11 |

`SandboxPort::acquire_scoped(scope_id, &SandboxSpec, &EgressPolicy)`：默认实现 `level == None` 时转调 `acquire(scope_id, spec)`，否则 `Err(SandboxError{Unavailable, "…不支持出网档位…"})`。
`scope_id` 在 DD6 之后是 `session_id`（按线程沙箱，CT10）；容器标签仍是 `aite.task=<scope_id>`。

## 5. 模型（`protocol.rs` / `errors.rs`）

- `Message += provider_extra: Map`（`default` + 空则不序列化；原样携带 `reasoning_content` / `encrypted_content` / `reasoning_details`；`Message::text` 同步给空 Map）。
- `Usage += cache_write_tokens: u64`（default 0）。成本 = 输入 + 缓存命中 + 缓存写入 + 输出（DD7 / EE3）。
- `ModelError += RateLimited { retry_after_ms: Option<u64>, message: String }`、`Auth(String)`、`BadRequest(String)`。
  取代 CC3 / CC6 过渡期的 `retry-after-ms=N` 文本约定（DD4 / DD7 切过去后删文本约定）。
- 三个工具目录函数**不动**：`all_model_tools()` 仍 10 个。

## 6. 会话（`session.rs`）

- `Turn += message_id: Option<String>`（CT06 编辑前后对照）、`provider_extra: Map`（default + 空不序列化；DD4 在助手轮回放）、**`sender_name: Option<String>`**（缺口 ⑨）。
- `Session += meta: Map`（**开放通道**：`muted`、`page_id`、`forked_from`、频道会话计数……键名由用到的轨登记在本文 §11）。
- `OPEN_TASK_STATUSES: [TaskStatus; 5] = [Created, Planning, Answering, Working, AwaitingApproval]` + `TaskStatus::is_open()`。
  用于 `!status`、`list_open_tasks`、`find_task_by_no` 之外的「这个群有哪些没结束的任务」。
- `ACTIVE_TASK_STATUSES`、`TaskStatus::is_active`、`Task`：**一个字不动**。

## 7. 网关（`gateway.rs` / `ports.rs::ToolGateway`）

- `ToolContext += initiator_id: Option<String>`、`chat_type: Option<ChatType>`、`initiator_external: bool`（全部 default）。CT29 混淆代理防护与 EE7 审批人校验要用。
- `ToolGateway::call` 的调用顺序注释补上策略钩子（缺口 ⑩，CC4 已实现的真实顺序）：
  `校验 session_token → 查工具（含注册表的 enabled 谓词）→ before_call 策略钩子 → 按 parameters 校验 arguments → 执行（带超时）→ 截断 content → 返回`。

## 8. 调用面（`ports.rs`）

### 8.1 `PlatformPort` +12 个默认方法

默认实现一律 `Err(PlatformError { code: "unimplemented", message: "<方法名>: 本平台 adapter 未实现", retryable: false })`。

| 方法 | 签名 | RPC | Go 可选接口（T0c `ports_p1.go`） |
|---|---|---|---|
| `edit_text` | `(&self, message_id: &str, msg: &OutboundText) -> Result<(), PlatformError>` | `EditText(EditTextRequest{message_id=1, OutboundText msg=2}) → EditTextResponse{}` | `TextEditor` |
| `send_dm` | `(&self, user_id: &str, msg: &OutboundText) -> Result<SendResult, PlatformError>`（`msg.chat_id` 忽略） | `SendDirect(SendDirectRequest{user_id=1, OutboundText msg=2}) → SendResult` | `DirectSender` |
| `get_user` | `(&self, user_id: &str) -> Result<UserInfo, PlatformError>` | `GetUser(GetUserRequest{user_id=1}) → UserInfo` | `UserLookup` |
| `get_chat` | `(&self, chat_id: &str) -> Result<ChatInfo, PlatformError>` | `GetChat(GetChatRequest{chat_id=1}) → ChatInfo` | `ChatLookup` |
| `is_member` | `(&self, chat_id: &str, user_id: &str) -> Result<bool, PlatformError>` | `CheckMember(CheckMemberRequest{chat_id=1, user_id=2}) → CheckMemberResponse{is_member=1}` | `MemberChecker` |
| `list_chats` | `(&self) -> Result<Vec<ChatInfo>, PlatformError>`（bot 所在的群） | `ListChats(ListChatsRequest{}) → ListChatsResponse{repeated ChatInfo chats=1}` | `ChatLister` |
| `list_pins` | `(&self, chat_id: &str) -> Result<Vec<HistoryMessage>, PlatformError>` | `ListPins(ListPinsRequest{chat_id=1}) → ListPinsResponse{repeated HistoryMessage messages=1}` | `PinReader` |
| `search_messages` | `(&self, q: &SearchQuery) -> Result<Vec<SearchHit>, PlatformError>` | `SearchMessages(SearchQuery) → SearchMessagesResponse{repeated SearchHit hits=1}` | `MessageSearcher` |
| `write_doc` | `(&self, doc: &DocWrite) -> Result<DocRef, PlatformError>` | `WriteDoc(DocWrite) → DocRef` | `DocWriter` |
| `delete_doc` | `(&self, doc: &DocRef) -> Result<(), PlatformError>`：删除应用建的云文档，**做不到删除时至少撤销 openchat 共享** | `DeleteDoc(DeleteDocRequest{DocRef doc=1}) → DeleteDocResponse{}` | **`DocDeleter`**（单独一个接口，不并进 `DocWriter`） |
| `oauth_authorize_url` | `(&self, s: &OAuthStart) -> Result<OAuthUrl, PlatformError>` | `OAuthAuthorizeUrl(OAuthStart) → OAuthUrl` | `OAuthProvider` |
| `oauth_exchange` | `(&self, c: &OAuthCode) -> Result<UserInfo, PlatformError>` | `OAuthExchange(OAuthCode) → UserInfo` | `OAuthProvider`（同一接口两个方法） |

`DeleteDoc` 选最小形状 `DeleteDocResponse {}`（照 `UpdateCardResponse {}`）：「删掉」和「只撤共享」对调用方（FF6 清除）都算成功，
区别只进 edge 日志；FF6 的测试 `purge_deletes_or_unshares_hosted_docs` 只需要「调到了、没报错」。要区分时由 T0.1 加 `bool deleted = 1; bool unshared = 2;`（字段号空着，向后兼容）。

**Go 可选接口签名（T0c 在新文件 `edge/internal/server/ports_p1.go` 里照抄；参数与返回全用 pb 类型）**：

```go
package server

import (
	"context"

	pb "aite/edge/gen/aitepb"
)

type TextEditor interface {
	EditText(ctx context.Context, req *pb.EditTextRequest) error
}
type DirectSender interface {
	SendDirect(ctx context.Context, req *pb.SendDirectRequest) (*pb.SendResult, error)
}
type UserLookup interface {
	GetUser(ctx context.Context, userID string) (*pb.UserInfo, error)
}
type ChatLookup interface {
	GetChat(ctx context.Context, chatID string) (*pb.ChatInfo, error)
}
type MemberChecker interface {
	CheckMember(ctx context.Context, chatID, userID string) (bool, error)
}
type ChatLister interface {
	ListChats(ctx context.Context) ([]*pb.ChatInfo, error)
}
type PinReader interface {
	ListPins(ctx context.Context, chatID string) ([]*pb.HistoryMessage, error)
}
type MessageSearcher interface {
	SearchMessages(ctx context.Context, q *pb.SearchQuery) ([]*pb.SearchHit, error)
}
type DocWriter interface {
	WriteDoc(ctx context.Context, doc *pb.DocWrite) (*pb.DocRef, error)
}
type DocDeleter interface {
	DeleteDoc(ctx context.Context, doc *pb.DocRef) error
}
type OAuthProvider interface {
	OAuthAuthorizeURL(ctx context.Context, s *pb.OAuthStart) (*pb.OAuthUrl, error)
	OAuthExchange(ctx context.Context, c *pb.OAuthCode) (*pb.UserInfo, error)
}
```

`services_p1.go` 里每个新 RPC 的 handler 是现有 `PlatformService` 结构体上的方法：对 adapter 做类型断言，断言不上回 `status.Error(codes.Unimplemented, "unimplemented: <RPC 名>")`
（edge-client 把 `UNIMPLEMENTED` 映射成 `PlatformError{code:"unimplemented", retryable:false}`，与 §8.1 的 Rust 默认实现同形）；其余错误照 `edge.proto` 头注释的冻结映射。
原卡 `companion_edits` 列的是 10 个接口名（没随 REVISION 更新）；**以本表 11 个为准**（多出的是 `DocDeleter`）。

### 8.2 `SandboxPort += acquire_scoped`（见 §4）

### 8.3 `SessionStore` +14 个默认方法

默认实现一律 `Err(StoreError::Other("unimplemented: <方法名>"))`。原卡 12 个 + 缺口 ④ ⑦ 两个。DD1 实现（SQLite）。

| 方法 | 签名 | 语义 | 谁用 |
|---|---|---|---|
| `find_task_by_no` | `(&self, chat_id: &str, task_no: &str) -> Result<Option<Task>, StoreError>` | 本群里编号为 `task_no` 的任务，**含 delivered / failed / cancelled**；按群限定（防跨群串号） | DD3 锚点路由、EE12 `!evidence #A17` |
| `list_open_tasks` | `(&self, chat_id: &str) -> Result<Vec<Task>, StoreError>` | `status ∈ OPEN_TASK_STATUSES`，按 `(created_at, id)` 正序 | DD3 `!status`、EE7 |
| `index_message` | `(&self, e: &MessageIndexEntry) -> Result<(), StoreError>` | 写 CC5 的 `message_index`；同 `(chat_id, message_id)` 重复写幂等（语义照 CC5 回执） | DD3 / DD5 |
| `find_session_by_message` | `(&self, chat_id: &str, message_id: &str) -> Result<Option<MessageIndexEntry>, StoreError>` | 按消息反查会话 / 任务 | DD3 引用锚点 |
| `recent_sessions_by_sender` | `(&self, chat_id: &str, sender_id: &str, since: DateTime<Utc>) -> Result<Vec<Session>, StoreError>` | 本群该人 `created_by` 且 `last_active_at >= since` 的非归档会话，新的在前 | DD3「同一发起人 30 分钟内」 |
| `find_open_session` | `(&self, chat_id: &str, kind: SessionKind) -> Result<Option<Session>, StoreError>` | 本群该类的非归档会话，多条取 `created_at` 最新 | DD3 DM 会话、FF1 频道会话 |
| `list_sessions_inactive_since` | `(&self, before: DateTime<Utc>) -> Result<Vec<Session>, StoreError>` | 非归档且 `last_active_at < before` | FF1 / EE2 空闲归档 |
| `get_turn_by_message` | `(&self, session_id: &str, message_id: &str) -> Result<Option<Turn>, StoreError>` | 按 `Turn.message_id` 找轮次 | DD3 编辑前后对照（CT06） |
| `update_turn` | `(&self, t: &Turn) -> Result<(), StoreError>` | 按 `(session_id, seq)` 覆盖；不存在报错 | DD3 / EE13 |
| `list_tasks` | `(&self, q: &TaskQuery) -> Result<Vec<Task>, StoreError>` | 见 `TaskQuery` | DD12 管理台、EE12 |
| `purge_chat_content` | `(&self, chat_id: &str) -> Result<u64, StoreError>` | 清掉本群 transcript 正文（墓碑：保留审计元数据），返回动了的行数 | FF6（NEW23 / PIPL） |
| `prune_seen_events` | `(&self, before: DateTime<Utc>) -> Result<u64, StoreError>` | 删 `seen_at < before` 的去重记录 | EE12 留存 |
| **`unsee_event`** | `(&self, event_id: &str) -> Result<(), StoreError>` | 撤销一次 `seen_event` 登记（入口处理失败、平台将重推时调用；不存在静默 Ok） | **缺口 ④**：CC2 ③ 让入口错误回平台重推，但重推回来被 R2 当重复丢掉 |
| **`update_task_if`** | `(&self, t: &Task, expected: &[TaskStatus]) -> Result<bool, StoreError>` | 比较并交换：库里当前状态 ∈ `expected` 才写入并返回 true，否则不写返回 false | **缺口 ⑦**：取消 / 卡死替换与被丢 worker 之间「最后一笔写靠时序」 |

`recover_orphan_tasks` 的文档串改为「created / planning / answering / working 四态改 failed……**不含 awaiting_approval**（CC5 口径）」。

### 8.4 `RunHooks`

```rust
pub struct RunHooks {
    pub drain_steer: Arc<dyn Fn() -> Vec<SteerMessage> + Send + Sync>,     // 缺口 ⑧：原 Vec<String>
    pub is_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    pub cancelled_by: Arc<dyn Fn() -> Option<String> + Send + Sync>,      // 缺口 ⑥：停止发起人（真人才有）
    pub take_decision: Arc<dyn Fn() -> Option<ApprovalDecision> + Send + Sync>, // DD3 信箱，EE7 用
}
```

`none()`：`Vec::new` / `false` / `None` / `None`。`SteerMessage { text, platform_user_id, sender_name: Option<String>, message_id: Option<String> }`（domain.rs）。
⑧ 是本批唯一**改已有字段类型**的一处：T0c 在 control（入队处本来就有事件的 `sender_id` / `sender_name` / `message_id`）与 worker（取 `.text`，并按 `platform_user_id` 认领发言人）各改一处，
属机械活；换来的是「同一句话多人说过时认不准发言人」这个真 bug 能修（CC3 回执 §9）。

### 8.5 `ControlPlane += submit_internal`

`async fn submit_internal(&self, req: &InternalRequest) -> Result<Option<String>, IngressError>`，默认 `Err(IngressError::Other("unimplemented: submit_internal"))`。
`Ok(Some(task_id))` = 建了任务；`Ok(None)` = 按规则不建（静音、群被停用……原因进日志）。DD3 实现，走与 @ 同一条派发。

### 8.6 五个新 trait（async_trait，**无默认实现**，W2 起由 DD1 实现、DD2 出假件）

错误类型一律 `StoreError`。时间一律 `DateTime<Utc>`。

**`ScopeStore`**（CT09 / CT28；继承 org → workspace → channel / dm）

| 方法 | 语义 |
|---|---|
| `get_settings(&self, scope: &ScopeRef) -> Result<Option<ScopeSettings>, StoreError>` | 读这一层自己的设置（不继承） |
| `put_settings(&self, s: &ScopeSettings) -> Result<(), StoreError>` | 按 `s.scope` 覆盖写这一层 |
| `delete_settings(&self, scope: &ScopeRef) -> Result<bool, StoreError>` | 删这一层（FF6 清除），返回是否删了 |
| `list_settings(&self, kind: Option<ScopeKind>) -> Result<Vec<ScopeSettings>, StoreError>` | 管理台列表 |
| `resolve(&self, chain: &[ScopeRef]) -> Result<EffectiveSettings, StoreError>` | `chain` 从 org 到叶子（channel 或 dm）；逐层用非 `None` 的值覆盖，`bundles` 逐层并集——**叶子的 `external_chat_mode` 为 `channel_only` 时只取叶子自己的 bundles**；最后填 `snapshot_hash = EffectiveSettings::compute_snapshot_hash()` |

**`MemoryStore`**（CT16；墓碑 + 清除）

| 方法 | 语义 |
|---|---|
| `list(&self, scope: MemoryScope, scope_id: &str, include_deleted: bool) -> Result<Vec<MemoryEntry>, StoreError>` | 按 `updated_at` 正序 |
| `get(&self, id: &str) -> Result<Option<MemoryEntry>, StoreError>` | 墓碑也返回（`deleted_at` 有值） |
| `upsert(&self, e: &MemoryEntry) -> Result<(), StoreError>` | 按 `id` 插入或覆盖 |
| `delete(&self, id: &str, by: &str) -> Result<bool, StoreError>` | 打墓碑（`deleted_at` / `deleted_by`），正文清空；返回是否存在 |
| `purge_scope(&self, scope: MemoryScope, scope_id: &str) -> Result<u64, StoreError>` | 硬删整个 scope（FF6），返回行数 |

**`RoutineStore`**（CT12 / CT15；`due()` 按 `next_run_at` 取）

| 方法 | 语义 |
|---|---|
| `create(&self, r: &Routine)` / `update(&self, r: &Routine)` → `Result<(), StoreError>` | — |
| `get(&self, id: &str) -> Result<Option<Routine>, StoreError>` | — |
| `list_by_chat(&self, chat_id: &str) -> Result<Vec<Routine>, StoreError>` | 按 `created_at` 正序 |
| `delete(&self, id: &str) -> Result<bool, StoreError>` | — |
| `due(&self, now: DateTime<Utc>, limit: u32) -> Result<Vec<Routine>, StoreError>` | `enabled && next_run_at <= now`，按 `(next_run_at, id)` 正序，最多 `limit` 条 |
| `mark_fired(&self, id: &str, fired_at: DateTime<Utc>, next_run_at: Option<DateTime<Utc>>, task_id: Option<&str>) -> Result<(), StoreError>` | 写 `last_run_at` / `last_task_id` / `next_run_at`（`None` = 不再触发） |
| `find_by_external_topic(&self, topic: &str) -> Result<Vec<Routine>, StoreError>` | EE11 外部事件匹配 |
| `purge_chat(&self, chat_id: &str) -> Result<u64, StoreError>` | FF6 |

**`UsageLedger`**（CT26；DM 记到个人）

| 方法 | 语义 |
|---|---|
| `record(&self, r: &UsageRecord) -> Result<(), StoreError>` | 只追加 |
| `spent(&self, scope: BudgetScope, key: &str, since: DateTime<Utc>) -> Result<f64, StoreError>` | `org`：key = tenant_id；`channel`：key = chat_id（**不含 DM**）；`user`：key = user_id（含 DM）；`task`：key = task_id；`ts >= since` 的 `cost_cny` 之和 |
| `summary(&self, group_by: UsageGroupBy, since: DateTime<Utc>, until: DateTime<Utc>) -> Result<Vec<UsageSummary>, StoreError>` | 按群 / 人 / 模型汇总，`cost_cny` 降序；DM 的记录按群汇总时归到 key `""`、按人汇总时归到发起人 |

**`AuditLog`**（CT27；只追加）

| 方法 | 语义 |
|---|---|
| `append(&self, e: &AuditEvent) -> Result<(), StoreError>` | 只追加，无 update / delete |
| `query(&self, q: &AuditQuery) -> Result<Vec<AuditEvent>, StoreError>` | 按 `ts` 正序 |
| `purge_before(&self, before: DateTime<Utc>) -> Result<u64, StoreError>` | 删 `ts < before` 的行；**实现必须把 `before` 钳到 `now - 190 天` 以前**（即使调用方给了更晚的时间也只删 190 天以前的），返回行数 |

## 9. `domain.rs`（新文件，唯一新锁定文件）

原卡 30 个类型 + 本文加的 2 个（`MessageIndexEntry`、`SteerMessage`，理由见表）。枚举全部 `str_enum!`（snake_case 字符串，`ALL` / `as_str` / `FromStr`）；结构体 `Serialize + Deserialize + PartialEq`，可选字段 `serde(default)`。

| 类型 | 形状 | 服务 |
|---|---|---|
| `ScopeKind` | `org, workspace, channel, dm` | CT09 |
| `ScopeRef` | `{kind: ScopeKind, id: String}`（org = tenant_id、workspace = workspace_id、channel = chat_id、dm = 用户 id） | CT09 |
| `ExternalChatMode` | `restrict, channel_only, allow`（默认 restrict：回固定提示、不执行） | NEW06 / NEW08 / D25 |
| `SearchScope` | `current_chat, joined_chats, workspace` | CT21 |
| `ConnectionRef` | `{id, kind, host, secret_ref, read_only: bool}`（`secret_ref` 只是 edge 持有的密钥名） | CT17 / EE10 |
| `RepoGrant` | `{repo, can_push: bool, default_branch: Option<String>}` | CT22（EE5） |
| `Skill` | `{name, description, body}`（markdown） | CT09（DD12 / FF8） |
| `AccessBundle` | `{name, connections: Vec<ConnectionRef>, repos: Vec<RepoGrant>, domains: Vec<String>, skills: Vec<Skill>, instructions: String, tools: Vec<String>, approval_required_tools: Vec<String>}` | CT09 / CT13 / CT17 |
| `ScopeSettings` | `{scope: ScopeRef, enabled: Option<bool>, respond_automatically: Option<bool>, network: Option<SandboxNetwork>, model: Option<String>, external_chat_mode: Option<ExternalChatMode>, search_scope: Option<SearchScope>, spend_limit_cny_month: Option<f64>, instructions: Option<String>, bundles: Vec<AccessBundle>, extra: Map, updated_by: String, updated_at: Option<DateTime>}`（`None` = 继承上层） | CT09 / CT26 / CT28 |
| `EffectiveSettings` | `{chain: Vec<ScopeRef>, enabled, respond_automatically, network: SandboxNetwork, model: String（空 = primary）, external_chat_mode, search_scope, spend_limit_cny_month: Option<f64>, instructions: String, bundle: AccessBundle（合并后）, snapshot_hash: String}` + `compute_snapshot_hash()` = `sha256(canonical_json(把 snapshot_hash 置空后的整体))` 的 hex | CT09 锁定（DD4 写进 `config_snapshot`） |
| `MemoryScope` | `workspace, channel, user` | CT16 / FF4 |
| `MemoryEntry` | `{id, scope: MemoryScope, scope_id, title, content, created_by, created_at, updated_at, deleted_at: Option, deleted_by: Option}` | CT16 |
| `ScheduleKind` | `once, interval, cron` | CT15 |
| `Schedule` | `{kind, at: Option<DateTime>, every_min: Option<u32>, cron: Option<String>, tz_name（默认 "Asia/Shanghai"）, tz_offset_min: i32（默认 480）}` | CT15 |
| `RoutineTrigger` | 内部标签枚举 `{"kind": "schedule", schedule}` / `{"kind": "channel_watch", chat_id, keyword}` / `{"kind": "external", topic}` | CT12 / CT15 |
| `RoutineOutput` | 内部标签枚举 `{"kind": "chat", chat_id, thread_id?}` / `{"kind": "dm", user_id}` / `{"kind": "page", doc_id?}` | CT15 / CT23 |
| `Routine` | `{id, name, chat_id, prompt, trigger, output, enabled, created_by, created_at, updated_at, next_run_at: Option, last_run_at: Option, last_task_id: Option}` | CT12 / CT15 / CT27 |
| `BudgetScope` | `org, channel, user, task` | CT26 |
| `UsageRecord` | `{ts, tenant_id, chat_id: Option（DM 为 None）, user_id, task_id: Option, kind（"model" / "search"）, model, input_tokens, output_tokens, cached_tokens, cache_write_tokens, cost_cny: f64}` | CT26 |
| `UsageGroupBy` | `chat, user, model` | CT26 |
| `UsageSummary` | `{group_by, key, calls: u64, input_tokens, output_tokens, cost_cny}` | CT26 |
| `AuditCategory` | `access, config, memory, routine, approval, feedback, purge, admin, network` | CT27 |
| `AuditEvent` | `{id, ts, tenant_id, category, action, actor_id, chat_id: Option, target: Option, detail: Map}`（`detail` 是**开放通道**） | CT27 |
| `AuditQuery` | `{since: Option, until: Option, category: Option, chat_id: Option, actor_id: Option, limit: u32（0 = 实现默认）}` | CT27 |
| `NetworkEvent` | **逐字照录 CC11 §5 第 7 项的 13 个键**：`ts: DateTime<Utc>`（CC11 写 UTC 固定 6 位小数 + `Z`，chrono 照读）、`audit_tag`、`level`（`String`：令牌没认出来时是 `""`）、`method`、`host`（规范化后、不含端口）、`port: u32`、`decision`（`allow` / `deny`）、`reason`（放行为 `""`）、`status: u32`、`bytes_up: u64`、`bytes_down: u64`、`duration_ms: u64`、`injected: bool`；小时文件 `<AuditDir>/YYYYMMDDHH.jsonl`（UTC） | CT27 / NEW11（EE12 导出） |
| `ApprovalDecision` | `{approval_id, approved: bool, decided_by, comment: Option, decided_at}` | CT13（EE7） |
| `Services` | `{scope, memory, routines, usage, audit: Option<Arc<dyn …>>}`，`Clone + Default` + 手写 `Debug`（只打印哪几个接上了） | 贯通 control / worker / gateway（T0c） |
| `TaskOrigin` | `routine, followup, ambient, fork, external, admin` | CT07 / CT12 / CT15 / NEW16 |
| `InternalRequest` | `{origin, tenant_id, workspace_id, chat_id, thread_id: Option（None = 新开顶层）, text, initiator_id, initiator_name: Option, routine_id: Option, meta: Map}` | DD3 `submit_internal` |
| `TaskQuery` | `{chat_id: Option, statuses: Vec<TaskStatus>（空 = 不限）, created_by: Option, since: Option, until: Option, limit: u32（0 = 50）}` | DD12 / EE12 |
| `MessageIndexEntry`（**本文加**） | `{chat_id, message_id, session_id, task_id: Option, outbound: bool}` = CC5 `message_index` 的五列（`created_at` 由 store 内部盖戳，不进契约） | `index_message` / `find_session_by_message` 要一个参数 / 返回类型；原卡只写了方法名 |
| `SteerMessage`（**本文加**） | `{text, platform_user_id, sender_name: Option, message_id: Option}` | 缺口 ⑧ |

## 10. op 登记表（替代新 `EvidenceKind`）

不加 `EvidenceKind`（仍 10 个，`frozen_values` 钉 `ALL.len() == 10`）。新事件一律挂在已有 kind 上，用一个键区分：
`checklist_op` 用 payload 键 **`op`**（现有取值 `add` / `check` / `fail` / `note` 与卡片的两条，`worker/src/local_tools/checklist.rs` 的 `checklist_evidence`、`worker/src/card.rs` 的 `write_evidence`）；
`event_received` 用 payload 键 **`route`**（现有 `new_task` / `steer` / `command`，常量在 `control/src/evidence_log.rs`；payload 键 `event_id/kind/chat_id/sender_id/message_id/mentioned/route[/text]`，`event_payload()`）。

| op | kind | 区分键 = 值 | payload 必有的键（除区分键） | 谁发 |
|---|---|---|---|---|
| `command` | `event_received` | `route = "command"` | `event_payload` 的 7 键 + `command`（命令名，如 `stop` / `restart`） | CC2（W1 已发，`ROUTE_COMMAND`） |
| `approval_requested` | `checklist_op` | `op = "approval_requested"` | `approval_id`、`tool`（要审批的工具名，可空串）、`summary`、`requested_by`（发起人 id） | EE7（`local_tools/request_approval`） |
| `approval_decided` | `checklist_op` | `op = "approval_decided"` | `approval_id`、`approved`（bool）、`decided_by`（人 id；超时为 `""`）、`via`（`card` / `text` / `timeout`） | EE7 |
| `reply_sent` | `checklist_op` | `op = "reply_sent"` | `message_id`、`source`（`post_update` / `routine` / `ambient`）、`chars` | DD4（`post_update`）；EE2 / FF1 复用 |
| `budget_stop` | `checklist_op` | `op = "budget_stop"` | `scope`（`BudgetScope` 字符串）、`key`、`limit_cny`、`spent_cny` | EE3（`budget.rs` 的 `before_step`） |
| `memory_op` | `checklist_op` | `op = "memory_op"` | `action`（`read` / `write` / `delete`）、`memory_id`、`scope`（`MemoryScope` 字符串）、`scope_id` | EE1 |
| `routine_fired` | `event_received` | `route = "internal"` 且 `origin = "routine"` | `origin`、`chat_id`、`initiator_id`、`routine_id`、`fired_at`（其余 `event_payload` 键此路没有平台事件，不写） | DD3（`submit_internal` 写 `route=internal` + `origin`）；`routine_id` 由 EE2 经 `InternalRequest.routine_id` 带进来 |
| `policy_denied` | `tool_result` | payload 键 `policy_denied` 存在 | 现有 `ok=false`、`error`（`code = "denied"`）、`content_hash`、`duration_ms` + `policy_denied`（原因码：`approval_required` / `not_in_bundle` / `cross_chat` / `egress`） | DD6 在 `ToolResult.data["policy_denied"]` 里标；worker 写 `tool_result` 时照抄该键——**跨轨接缝**：worker 的 `tool_result` 写端在 W2 归 DD4，请总管在 H11 审稿时确认由 DD4 顺手接（一行） |
| `cancelled.stopped_by` | `cancelled` | payload 键 `stopped_by` | 真人发起时才有：`!stop` 与卡片 Stop 按钮；收尾那条路**不写该键**（不写 `null`）。`by = "stop"` 与 `steps` 不动 | CC2（W1，control 那一支）；worker 那一支经 `RunHooks.cancelled_by`（缺口 ⑥）由 T0c 接线 |

EE12 的 `aite evidence show` / `!evidence` 按本表渲染（`renders_approval_ops`）。新 op 进表 = 改本文，不改契约。

## 11. 开放通道（不改契约就能扩的地方；T0.1 的准入前提是这些都绕不过去）

| 通道 | 登记的键 / 用法 |
|---|---|
| `Session.meta` | `muted: bool`（EE9）、`page_id: String`（EE6）、`forked_from: String`（EE9 `!fork`）、`channel_turns: u64` / `bot_turns: u64`（FF1 / EE8 bot 连轮上限）。新键在本表登记 |
| `ExternalEvent.payload` | 原始外部事件（GitLab webhook 体、飞书文档 / 审批事件），`source` + `topic` 决定解释方式（EE11） |
| `AuditEvent.detail` | 每个 `category/action` 自带的细节（EE1 记忆编辑前后、EE9 反馈原文、FF6 清除范围） |
| `Message.provider_extra` / `Turn.provider_extra` | 厂商思考字段原样携带（DD4 / DD7）；换模型 / 换厂商时丢弃 |
| `ModelTurn.raw["provider_extra"]` | CC6 已用的过渡位；DD4 把它搬进 `Turn.provider_extra` |
| `ScopeSettings.extra` | 管理台上还没进类型的开关 |
| `InternalRequest.meta` | 内部来源的附加键（例程输出目标、fork 来源） |
| trait 默认方法 | 新增方法先带默认实现（本批的 `PlatformPort` / `SessionStore` / `SandboxPort` / `ControlPlane` 都是这样） |
| 网关注册表（CC4 `GatewayTool`） | 新工具不进 `all_model_tools()`，经注册表 + `enabled` 谓词 |

## 12. 契约测试（锁面内，K 条新增；与补丁一起落）

锁面保持 26 个文件：**不新建测试文件**，新测试都加进现有的 `frozen_values.rs` / `config.rs` / `roundtrip.rs`；`layout.rs` 的 `must` 加 `domain.rs`。清单与每条钉什么见 `review/t0/APPLY.md` 与回执 `review/p1/ledger/T0.md` §3。

## 13. 交给 T0c 的伴随清单（原样交接；T0c 不改锁定面、不重生成 `edge/gen`）

1. **结构体字面量与穷举 match**：`Message, Turn, Session, NormalizedEvent, ChecklistCard, OutboundText, HistoryMessage, PlatformCapabilities, ToolContext, Usage, ModelConfig, RunHooks`
   与各新枚举值（`EventKind, CardActionKind, ModelError, SandboxNetwork, ExecLanguage, ReactionKind`）；范围 = `contract/p1.0` 上 `cargo check --workspace --all-targets` 报的
   （T0 的估计见 `review/t0/companion-todo.md`）。另：`WorkerConfig` / `SandboxConfig` / `FeishuConfig` / `EdgeConfig` 若有字面量构造点同理；`RunHooks.drain_steer` 的类型变了（§8.4），control 入队与 worker 消费各一处。
2. **5 处 p0.2 字面量钉**：`edge/cmd/aite-edge/main_test.go:202`、`core/crates/evidence/tests/manifest.rs:65`、`core/crates/evals/tests/evals_runner.rs:111`、
   `core/crates/evals/tests/protocol_probe.rs:523`、`core/crates/app/tests/cli_smoke.rs:481`（行号生于 `98e4460`，按内容找）。
3. `EventKind::Reaction` 进 R4（`on_non_message`）**只计数**，排在 R5 / R6 之前（否则进行中话题里的一个表情会被当 steer 塞进任务）；`External` 同样先只计数（EE11 接）。
4. 新 `CardActionKind`（Approve / Reject / Submit）记日志后丢（EE7 接）。
5. `Services` 贯通：`WorkerDeps` / `ControlDeps` 加 `services: Services`、gateway `with_services()`、`FeatureCtx.services`（T0c 加字段、DD1 填）；
   同一遍给每个 `WorkerDeps` 字面量加 `aigc_label: Option<LabelConfig>`（`LabelConfig` 在 `worker/src/label.rs`，默认 `None`）。
6. `edge/internal/server/ports_p1.go` + `services_p1.go`（+ 测试）：§8.1 的 11 个接口与 UNIMPLEMENTED 兜底。
7. evals `EventSpec += quote, reaction, external, sender_external`。
8. `WorkerConfig.max_parallel_tasks`（默认 4）接 CC2 的 `with_max_parallel`；`context_max_tokens` 经 CC3 的 `AgentWorker::with_context_max_tokens` 接进预算（`review/paste-CC3.md` §5⑦）；
   **`stuck_after_sec` 接 CC2 的 `with_stuck_after_sec`**（缺口 ⑤ 进了 p1.0）；`RunHooks.cancelled_by` 由 control 填、worker 写进 `cancelled` 载荷的 `stopped_by`（缺口 ⑥）。
9. **`edge/gen/**` 永不由 T0c 重生成**（总管在 `contract/p1.0` 上的提交带着它）。
10. W1 各轨记账转给 T0c 的（不在原卡 `companion_edits` 里；来源见括号）：`run.rs` 的 `takeoff` 调 `features::start_all`（EE7 / EE2 / DD12 依赖；`review/paste-CC4.md` §5⑧）；
    `wiring.rs` 的 docker 档（`docker_sandbox_factory`）挂登记与 `gateway_options`（`review/paste-CC4.md` §5⑦）；`plane_factory` 把 CC7 的 `worker_options.aigc_label` 接进 `WorkerDeps`，
    并撤掉 CC7 的「无消费方」闸门及其测试 `worker_options_label_without_consumer_is_a_wiring_error`（`review/paste-CC7.md` §5⑥；**总管已确认归 T0c 撤**）；
    `FeatureCtx.services` 由 T0c 加字段、DD1 填（`review/p1/ledger/CC4.md` §7）。
11. 锁面外但 T0 补丁点不到的：`core/crates/app/src/lock.rs:19` 注释「= 25」→ 26；`edge-client/tests/common/mod.rs` 的 fake `PlatformService` 补 12 个 RPC（或靠 tonic 生成的默认桩，视 `build.rs` 是否生成 `Unimplemented*`）。
12. `ModelError::RateLimited` 落地后，CC3 / CC6 的 `retry-after-ms=N` 文本约定由 DD4 / DD7 删掉（不是 T0c 的活，登记在此备查）。

## 14. W1 缺口 ④–⑩ 的处置

| # | 缺口 | 处置 | 形状 |
|---|---|---|---|
| ④ | 重推回来被 R2 当重复丢掉 | **进 p1.0** | `SessionStore::unsee_event(event_id)` 默认方法（两段式「处理完才提交」要改 `seen_event` 的语义，影响面大，不选） |
| ⑤ | 卡死阈值写死在 control 常量 | **进 p1.0** | `WorkerConfig.stuck_after_sec = 900` |
| ⑥ | worker 写的 `cancelled` 没有 `stopped_by` | **进 p1.0** | `RunHooks.cancelled_by: Arc<dyn Fn() -> Option<String> + Send + Sync>`（新字段，不改 `is_cancelled` 的类型） |
| ⑦ | 取消 / 卡死替换与被丢 worker 的最后一笔写靠时序 | **进 p1.0** | `SessionStore::update_task_if(&Task, expected: &[TaskStatus]) -> bool` 默认方法 |
| ⑧ | steer 认不准发言人 | **进 p1.0** | `RunHooks.drain_steer` → `Vec<SteerMessage>`（本批唯一改类型的一处，§8.4） |
| ⑨ | transcript 没有发言人显示名 | **进 p1.0** | `Turn.sender_name: Option<String>` |
| ⑩ | `ToolGateway::call` 顺序注释没有策略钩子 | **进 p1.0** | 只改文档串（§7）；`ToolContext += initiator_id / chat_type / initiator_external` 照原卡 |

## 15. 已知的审稿重点

- 五个新 trait 的方法集：原卡说「同契约先行草稿」，那份草稿是过程材料、**没入库**（总计划附录 D）。本文 §8.6 是按 DD1–DD3、EE1–EE12、FF4 / FF6 的 goal **推出来的**，请总管在 H11 重点看。
- `find_task_by_no` 按 `chat_id` 限定（原卡只写「incl. delivered」）：任务编号租户内唯一，但路由只该命中本群的任务。
- `policy_denied` 的证据写端跨 DD6 / DD4 两轨（§10）。
- 能力位里标 0 的数值（§3）是占位，等 DD11 / H9 实测。
