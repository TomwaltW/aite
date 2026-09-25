# 派单 T0：契约 p1.0 补丁起草（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-T0.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=T0）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

P1 要对齐 Claude Tag 的 26 条机制（CT02/06/07/08/09/12/13/14/15/16/17/18/20/21/22/23/26/27/28/30、NEW02/08/10/11/16/21），
全都卡在同一件事上：**契约还是 p0.2**。另外原卡的 [REVISION 2026-09-25] 为 NEW23（断开工作区 / PIPL 清除，FF6 做）加了 `delete_doc` / `DeleteDoc`，
为 D9（只用大陆端点、私有化自托管）加了 `ModelVendor` 的 `selfhost` 与 `ModelConfig.allow_overseas_endpoint`。实证（行号基于 `98e4460`）：

- 版本与平台：`contracts/src/lib.rs:26` `CONTRACT_VERSION = "p0.2"`、`edge/internal/server/server.go:18` 同值；`config.rs:149-151`
  `PlatformChoice { feishu, fake }`，`contracts/tests/config.rs:63` 钉着「`platform: dingtalk` 被拒」。
- 沙箱只有零出网：`sandbox.rs:6-13` `SandboxNetwork { None }`、`ExecLanguage { Python }`；`proto/src/convert.rs:500-508` 与 `:535-543` 其余一律拒。
- 模型：`protocol.rs:155-169` `Message` 没有地方放 `reasoning_content`；`:183-191` `Usage` 无缓存写入；`errors.rs:71-80` `ModelError` 只有三种，
  所以 CC3/CC6 只能用 `retry-after-ms=N` 文本约定过渡。
- 会话：`session.rs:56-68` `Turn` 无 `message_id`（CT06 前后对照做不了）、`:87-106` `Session` 无 `meta`；`events.rs:76-78` `task_no`「P0 只展示不路由」；
  `ports.rs:127-159` `SessionStore` 没有 `find_task_by_no`（钉钉/企微 `#A17` 锚点路由做不了）。
- 事件与能力：`events.rs:42-51` `EventKind` 无 Reaction/External；`capabilities.rs:4-23` 只有 9 个能力位；`edge.proto:34-44` `PlatformService` 只有 9 个 RPC、
  `:99-102` `AcquireRequest` 不带出网策略。

**机制（D11）**：本轨只**起草**补丁，不落地。补丁只写「没有任何云端轨能改」的文件（锁定面 + 守卫前缀面 + 无主 R0），
所以 W1 怎么合都不会让锚点漂；总管在 W1 合并、P0-CLOSE 上 main 之后，在分支 `contract/p1.0` 上打补丁 + `make proto-gen` + 用补丁前的二进制重锁（H11）；
W2a 的 **T0c** 让整个工作区编得过。**你交付的设计文档就是 p1.0 的冻结规格**（总管审过才冻）。

**谁吃你的产出**：T0c（伴随清单、5 处版本钉、Go 可选接口签名）；DD1（`SessionStore` 12 个默认方法 + 五个领域存储 trait）；DD2（12 个新 RPC 的客户端与假件）；
DD3（`find_task_by_no`、`submit_internal`、`RunHooks.take_decision`、`OPEN_TASK_STATUSES`）；DD4（`provider_extra`）；DD6（`acquire_scoped` + `EgressPolicy`）；
DD7（`ModelVendor`/`ModelsConfig`，含 `selfhost` 档与 `allow_overseas_endpoint` 海外端点默认拒绝）；DD8（Go 侧 `dingtalk` 平台与配置镜像）；EE1–EE12（各配置段、op 登记表）；
FF6（`delete_doc` / `DeleteDoc`：NEW23 清除时逐个删除或撤销共享托管页，测试 `purge_deletes_or_unshares_hosted_docs`）。
W1 的接缝要对上：CC2 预埋的 `routing/{resolver,card_actions,external,edits}.rs`、`internal.rs`、`approvals.rs`；CC3 的 `local_tools/{post_update,request_approval}`、
`snapshot.rs`、`label.rs` 与 `retry-after-ms` 文本约定；CC4 的 `GatewayTool` 注册表、`policy.rs`、`SandboxKey`、`FeatureCtx`；CC5 的消息索引表与显式状态列表；
CC6 的厂商识别与 `ModelTurn.raw["provider_extra"]`；CC8 解析后丢弃的表情事件与 `AITE_FEISHU_{API_BASE,CARD_BUTTONS}`；CC9/CC10 写进 `Anchor.task_no` 的 `#A`；
CC11 的 `Policy{Level, AllowHosts, AuditTag}` 与每请求一行的 `NetworkEvent` JSONL。

## 2. 必读（按顺序）

1. `CLAUDE.md` 全文（云端唯一能读到的仓库约定）。
2. 总计划 §0、§3（D9 / D11 / D13 / D17）、§4.4、§4.5、§5.1–§5.4、§6 开头的规则、§7、§8 的 H11、§9、§10 第一行。
3. `review/p1/tracks-2026-09-25.json`：`contract_batches` 里 `T0-p1.0` 的 `contents` 与 `companion_edits`（**本轨的权威清单**）、`P0-CLOSE`；
   `waves[0].tracks` 里 T0 与 CC1–CC12 的卡（接缝）；`waves[1]` 是 T0c 的卡；`waves[2].tracks` 里 DD1–DD12 的 goal（trait 方法集从这里推，尤其 DD1 / DD3）。
4. 补丁脚本的范本：`review/aa4-proto-patch.py`（全文；`Edit` 恰好命中一次 `:121-135`、protoc 闸门 `:188-220`、授权闸门 `:236-244`）；
   `review/bb2-created-at-chain-patch.py`（lib.rs 导出锚点 `:170-179`、`EDITS` `:627-766`、`check_rust` `:779-807`、`main` `:811-901`）；
   `review/bb4-guard-patch.py`（`--root` 主流程 `:641-733`）。
5. 锁：`core/crates/app/src/lock.rs`（`LOCK_DIRS` `:17`、输出格式 `:118-157`）、`core/crates/app/src/main.rs:77-111`（`--repo`、退出码）。
6. 补丁目标（**只用 Read 工具读**）：`core/crates/contracts/src/*.rs`（12 个）与 `tests/*.rs`（7 个）；`proto/aite/v1/*.proto`（5 个）；
   `core/crates/proto/src/convert.rs`（596 行）与 `tests/convert.rs`（224 行）；`edge/internal/server/server.go:16-18`；`config/aite.example.yaml`（49 行）；
   `edge/internal/config/config.go:74-110`（宽松 `yaml.Unmarshal`；`Validate` 只认 feishu|fake —— 翻它是 DD8 的）。
7. 被钉住的行为：`core/crates/store/src/lib.rs:419-424` 与 `:480-484`（按下标 `[0..2]` 绑 `ACTIVE_TASK_STATUSES`）；`contracts/tests/frozen_values.rs:4-18, :45-46`；
   `contracts/tests/protocol_tools.rs:5-40`（`catalog_is_frozen` = 10 个）。

## 3. 工作区

- 分支：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「T0: 契约 p1.0 补丁起草」（正文与多行提交信息怎么写见 §6 守卫）。
- 代码基线：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**（全部新建）：`review/t0/**`（`p1-contract-patch.py`、`data/*.txt`、`APPLY.md`、`companion-todo.md`）、`docs/p1/contract-p1.md`、`review/p1/ledger/T0.md`。
- **只读面**：其余一切。点名：
  - P0-CLOSE 文件（完整 14 个路径见 §4 第 1 步；其余几个也落在下一条的补丁目标面里）：`proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`core/crates/contracts/src/evidence.rs`、`core/crates/evidence/**`、
    `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`、`.claude/**`、`core/crates/app/tests/guard.rs`。
  - T0 补丁的目标文件**在真树里本轨也只读**：`proto/aite/v1/*.proto`、`core/crates/contracts/**`、`core/crates/proto/**`、`edge/internal/server/server.go`、
    `config/aite.example.yaml`、`edge/gen/**`、`.contracts.lock`。它们只在临时副本里被脚本改。
  - 邻轨（原卡可写面原文）：CC1 的 `scripts/{cloud-setup,check}.sh`、`Makefile`、`core/Cargo.{toml,lock}`、`CLAUDE.md`、`docs/p1/cloud-runbook.md`；
    CC4 的 `core/crates/app/src/features/**`；CC9/10/11 的 `docs/p1/{dingtalk,wecom,egress}.md`；CC2 的 `core/crates/control/{src,tests}/**`；
    CC3 的 `core/crates/worker/{src,tests}/**`；CC5 的 `core/crates/store/**`；CC6 的 `core/crates/models/**`；
    CC7 的 `core/crates/{evals,testing}/**`、`evals/p1/**`、`evals/README.md`。
- **本轨解冻的冻结项**（§9）：`CONTRACT_VERSION p0.2`（`frozen_values.rs:5`）与「`platform: dingtalk` 被拒」（`tests/config.rs:63`；Go 侧 `config.go:94-99` 仍拒，归 DD8）；
  `SandboxNetwork` 只有 none（契约侧；edge 侧归 DD8）。回归测试都在补丁里；变异验证见工作项 3 的 h 步。

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
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（形如「blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）…」，原文贴进回执）。
   **这一步被拦就是通过**：拦截原文里「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步（§6「被拦就停」指的是其它任何一次被拦）。
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27。
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（§4.1 的环境脚本把两者软链进 `/usr/local/bin`，
   直接在 PATH 上找；输出原样贴进回执）。缺了或版本不对 → 记进回执；之后脚本可以在副本里 `GOBIN=<副本>/bin go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.12` / `google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2` 补装，
   回执写明「插件是自测补装的」。
   本轨另加：`cd core && rustfmt --version`（云端 rustup 没有默认工具链，**rustfmt / cargo 只在 `core/` 下才解析得到**）；
   `ls review/aa4-proto-patch.py review/bb2-created-at-chain-patch.py review/bb4-guard-patch.py` 三个都在。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**。
   冷跑可能超过 Bash 单条 600 秒上限：用 Bash 工具的 `run_in_background` 跑 `scripts/check.sh > /tmp/check-open.log 2>&1; echo "exit=$?" >> /tmp/check-open.log`，
   完成通知到了再用 Read 工具读整份日志（见 §6 的「串行跑」）。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认。
   CC1 合并之后 check.sh 会多打一行 `go packages ok=N fail=M`，从那以后以这行为准。

## 5. 工作项

### ① 设计文档 `docs/p1/contract-p1.md` 首稿 → 第一次提交 → 开 draft PR

它是 p1.0 的冻结规格，必须覆盖：每个类型 / 字段 / 默认值 / RPC 与它服务的 CT/NEW 编号；下表的完整清单；五个新 trait 的方法集
（按 DD1：ScopeStore = org→workspace→channel/dm 继承 + `snapshot_hash`；MemoryStore = 墓碑 + 清除；RoutineStore = 按 `next_run_at` 取 `due()`；
UsageLedger = 按 scope/key/since 求和、按群/人/模型汇总、DM 记到个人；AuditLog = 只追加、`purge_before` 只删 190 天以前的）；
T0c 的 Go 可选接口签名（新文件 `edge/internal/server/ports_p1.go` 里每个新 RPC 一个接口：`TextEditor`、`DirectSender`、`UserLookup`、`ChatLookup`、
`MemberChecker`、`ChatLister`、`PinReader`、`MessageSearcher`、`DocWriter`、`DocDeleter`、`OAuthProvider`，参数与返回全用 pb 类型，handler 做类型断言、断言不上回 UNIMPLEMENTED；
`DocDeleter` 对应 `DeleteDoc`——原卡 `companion_edits` 的 10 个接口名没随 REVISION 更新，以设计文档为准，也可以写明由 `DocWriter` 一并覆盖 `DeleteDoc`，二选一写定）；
**模型厂商规则**（D9）：显式 `vendor` 永远优先于 CC6 的模型名推断；`selfhost` = 客户自托管 vLLM / SGLang / MindIE，不发任何云厂商专有参数；
`allow_overseas_endpoint` 默认 false，配到海外端点是配置错（DD7 实现，显式打开才降为告警）；
**op 登记表**（替代新 `EvidenceKind`）：`approval_requested`、`approval_decided`、`reply_sent`、`budget_stop`、`memory_op`、`routine_fired`、`command`、`policy_denied`
—— 每个写清挂在哪个 kind、用哪个键、payload 必有哪些键、由哪轨发。现状依据：`checklist_op` 走 payload 键 `op`（`worker/src/agent.rs:534`、`card.rs:269`），
`event_received` 的 payload 键是 `event_id/kind/chat_id/sender_id/message_id/mentioned/route[/text]`（`control/src/plane.rs:226-247` 的 `event_payload`），
`route ∈ {new_task, steer}`（常量 `:217` / `:219`），CC2 会加 `route=command`；
最后是交给 T0c 的伴随清单（见 ③ 的 companion 段）与「开放通道」一节（`Session.meta`、`ExternalEvent.payload`、`AuditEvent.detail`、`provider_extra`、trait 默认方法、网关注册表）。

**W1 已在 D0 派单里定死的形状，设计文档逐字照录、不另起**（以下三份派单为准，本段只是指路；照录时若与原卡冲突，写进回执「契约缺口」，不自行调和）：
① `domain.rs` 的 `NetworkEvent` = 逐字照录 `review/paste-CC11.md` §5 第 7 项的 13 个键（`ts`=UTC、固定 6 位小数 + `Z`，`audit_tag`、`level`、`method`、`host`、`port`（int）、`decision`、`reason`、`status`、`bytes_up`、`bytes_down`、`duration_ms`、`injected`），
小时文件名按 UTC（`YYYYMMDDHH.jsonl`）；键名、个数与 CC11 写出的 JSON 行一致，能反序列化它；
② `SessionStore::index_message` / `find_session_by_message` 的参数与返回按 `review/paste-CC5.md` §5② 的 `message_index` 列（`chat_id`、`message_id`、`session_id`、`task_id`（可空）、`outbound`；`created_at` 是 store 内部盖的戳）；
③ op 登记表的 `command` 条目 = `event_received` + `route="command"` + `command` 键，W1 由 CC2 发（`review/paste-CC2.md` §5⑨）；另登记 `cancelled` 载荷的 `stopped_by`（仅真人发起时出现：`!stop` 与卡片 Stop 按钮；收尾那条路不写该键）。

**p1.0 完整清单**（译自 `contract_batches[T0-p1.0]`；所有新字段 `#[serde(default)]`，**旧 JSON / P0 的 SQLite 行必须照样能反序列化**；精确默认值抄原卡，设计文档逐条写出）：

| 文件 | 改动 | 钉它的测试 |
|---|---|---|
| contracts `src/lib.rs` | `CONTRACT_VERSION` `p0.2→p1.0`（`:26`）；`pub mod domain;`；导出全部新公开项。**锚点避开 `:41-43` 的 `pub use evidence::{…}`**（BB2 改写它） | `frozen_values::constants` |
| `src/config.rs` | `PlatformChoice += Dingtalk "dingtalk", Wecom "wecom"`（一部署一平台，`EdgeConfig` 仍单一）；`FeishuConfig += api_base "https://open.feishu.cn", card_buttons false, service_user_token_env ""`；新 `DingtalkConfig` / `WecomConfig`（字段与 env 名照原卡）；`str_enum ModelVendor {generic,deepseek,qwen,glm,kimi,doubao,minimax,selfhost}`（`selfhost` 来自 REVISION）、`ThinkingMode {auto,on,off}`；`ModelConfig += name "primary", display_name, filing_no, vendor, thinking, timeout_sec 600, stream false, price_cached_in_per_mtok, price_cache_write_per_mtok, offpeak_price_multiplier 1.0, allow_overseas_endpoint false`（最后一项来自 REVISION）；`ModelsConfig {fallback, fast, catalog}`；`SandboxConfig += network_default none, proxy_url, trusted_preset "china", extra_trusted_hosts, allow_full false, linger_sec 600`；`WorkerConfig += max_parallel_tasks 4, context_max_tokens 96000, approval_timeout_sec 1800`；`EdgeConfig += http_listen, webhook_secret_env`；新段 `Search / Git / Pages / Memory / Routines(Asia/Shanghai, +480) / Ambient(自动回复默认关) / Access / Budget(人民币, 0=不限) / Compliance(aigc_label true, label_text "内容由 AI 生成", audit_retention_days 190, registration_scope_confirmed false, content_safety_filter false —— 后两个是总管 2026-09-25 为计划 D25 加的：EE8 只有两者都为 true 才允许把 access.external_chat_mode 从 restrict 切走) / Admin`；`AiteConfig` += 13 段。全部 `deny_unknown_fields` + 默认值 | `tests/config.rs` |
| `src/events.rs` | `EventKind += Reaction, External`；`CardActionKind += Approve, Reject, Submit`；新 `Quote`、`ReactionEvent`、`ExternalEvent{…, payload: Map}`；`NormalizedEvent += quote, reaction, external, sender_external`；`Anchor.task_no` 注释（`:76`）改为「edge 从自身文本或引用里解析 #A；core 按它路由」 | `frozen_values`、`roundtrip` |
| `src/outbound.rs` | `ReactionKind += Muted`；新 `CardLink`、`ApprovalPrompt`；`ChecklistCard += links, approval`；`OutboundText += mentions, dedupe_key`（`OutboundText::new` `:26-33` 同步）；`HistoryMessage += updated_at, deleted, reply_to`；新 `UserInfo / ChatInfo / SearchQuery / SearchHit / DocWrite(aigc_banner) / DocRef / OAuthStart / OAuthUrl / OAuthCode` | `roundtrip` |
| `src/capabilities.rs` | 新能力位 14 个（proto 字段 10–23：9 个 bool + 4 个 u32 + `max_upload_bytes` u64）；`feishu_p0()` 的 9 个旧值一个不改；新 `dingtalk_v1()` / `wecom_v1()`（取值照原卡） | `frozen_values` |
| `src/sandbox.rs` | `SandboxNetwork += Trusted, Custom, Full`；`ExecLanguage += Bash`；新 `CredentialBinding {host, header, scheme, secret_ref}`（只是名字，取值永不过 gRPC）、`EgressPolicy {level, allow_hosts, credentials, audit_tag}`（对齐 CC11 的 Policy） | `roundtrip` |
| `src/protocol.rs` | `Message += provider_extra: Map`（default + 空则不序列化；`Message::text` `:171-181` 同步）；`Usage += cache_write_tokens`；三个工具函数**不动**，`all_model_tools()` 仍 10 个 | `protocol_tools`（原样绿）、`roundtrip` |
| `src/session.rs` | `Turn += message_id, provider_extra`；`Session += meta: Map`；新 `OPEN_TASK_STATUSES = [Created, Planning, Answering, Working, AwaitingApproval]`。**`ACTIVE_TASK_STATUSES`（`:32-37`）与 `Task` 一个字都不动**（store 按下标绑它，孤儿恢复绝不能收待审批任务） | `frozen_values` |
| `src/gateway.rs`、`src/errors.rs` | `ToolContext += initiator_id, chat_type, initiator_external`；`ModelError += RateLimited{retry_after_ms, message}, Auth, BadRequest` | `roundtrip`、新测试 |
| `src/ports.rs` | `PlatformPort` +12 个默认方法（`edit_text, send_dm, get_user, get_chat, is_member, list_chats, list_pins, search_messages, write_doc, delete_doc, oauth_authorize_url, oauth_exchange`，默认 `Err(PlatformError "unimplemented", retryable false)`；`delete_doc(&DocRef) -> Result<(), PlatformError>`：删除应用建的云文档，或至少撤销 openchat 共享，来自 REVISION）；`SandboxPort += acquire_scoped`（level=None 时走 `acquire`，否则 `Unavailable`）；`SessionStore` +12 个默认方法（照原卡，默认 `StoreError::Other("unimplemented: <名>")`）；`RunHooks += take_decision`（`none()` 返回 None）；`ControlPlane += submit_internal`（默认 `IngressError::Other`）；新 trait `ScopeStore / MemoryStore / RoutineStore / UsageLedger / AuditLog`（async_trait、无默认）；`recover_orphan_tasks` 文档串（`:156-157`，现写「所有活跃态任务」）改为「created/planning/answering/working，不含 awaiting_approval（CC5 口径）」 | 新测试：只实现旧方法的替身能编过、新方法回 unimplemented |
| `src/domain.rs`（**新**，唯一新锁定文件） | 原卡列的 30 个类型（含 `Services`）；`Services {scope, memory, routines, usage, audit: Option<Arc<dyn …>>}`：`Clone + Default` + 手写 `Debug` | `roundtrip`、`layout` |
| contracts `tests/` | `frozen_values.rs`：`p1.0`、新枚举字符串与全部 `ALL.len()`（含 `ModelVendor` 8 个、`"selfhost"`）、`OPEN_TASK_STATUSES`、三份能力档；`:45` `EvidenceKind::ALL.len()==10` 与 `:46` 不动。`config.rs`：`:63` 翻成 dingtalk/wecom 可用、slack 仍拒、新默认值（点名钉 `allow_overseas_endpoint` 默认 false）、样例==默认。`roundtrip.rs`：`:24-63 / :120-135 / :162-170 / :181-197 / :248-256 / :265-285 / :309-318` 的字面量补新字段，新形状往返，**缺新字段的旧 JSON 照样能读**。`layout.rs`：`must += domain.rs`。`protocol_tools.rs`、`task_no.rs`、`evidence_vectors.rs` 不动 | — |
| `proto/aite/v1/events.proto` | `EVENT_KIND_REACTION = 7`、`_EXTERNAL = 8`；`CARD_ACTION_KIND_APPROVE = 3 / REJECT = 4 / SUBMIT = 5`；`Quote / ReactionEvent / ExternalEvent(payload = Struct)`；`NormalizedEvent` 19–22；`task_no` 注释（`:55`） | proto `convert` |
| `outbound.proto`、`capabilities.proto`、`sandbox.proto` | `REACTION_KIND_MUTED = 4`、上面 outbound 的全部新消息与字段（`ChecklistCard` 10/11、`OutboundText` 5/6、`HistoryMessage` 8/9/10）；能力位 10–23；`CredentialBinding` / `EgressPolicy`，network / language 注释更新（仍是 string） | proto `convert` |
| `edge.proto` | `PlatformService` +12 RPC（`EditText, SendDirect, GetUser, GetChat, CheckMember, ListChats, ListPins, SearchMessages, WriteDoc, DeleteDoc, OAuthAuthorizeUrl, OAuthExchange`）及其请求/响应；`DeleteDoc` 形状建议 `DeleteDocRequest { DocRef doc = 1; }` → `DeleteDocResponse {}`（照 `UpdateCardResponse {}` `:58` 的写法；若要区分删掉 / 只撤共享，改成 `bool deleted, bool unshared`，设计文档写定）；`AcquireRequest += EgressPolicy egress = 3`；`EdgeStatus += egress_ok = 7, http_ingress_ok = 8`；`IngressService` 不动。**锚点避开 `:151`**（AA4 改写那句注释）：用 `:43-44` 与 `:164-165` | proto `convert` |
| `core/crates/proto/src/convert.rs` + `tests/convert.rs` | 全部新字段 / 枚举 / 消息双向互转；network 认 `none|trusted|custom|full`、language 认 `python|bash`；UNSPECIFIED 照旧拒；测试字面量 `:9-46`、`:79-122` 补字段，`:150-155` 的 `"bridge"` 仍拒 | proto `convert` |
| `edge/internal/server/server.go` | 只改 `:18` 一行 → `"p1.0"`（`services.go:12` 已嵌 `pb.UnimplementedPlatformServiceServer`，新 RPC 不写 handler 也编得过） | `go build` |
| `config/aite.example.yaml` | 每个新段按默认值写全（contracts `example_yaml_equals_defaults` 要求样例==默认；Go 的 `config_test.go` 只比 Go 认识的字段，宽松解析不受影响）；不许出现 `sk-` | `tests/config.rs` |

**明确不做**：不加新 `EvidenceKind`；不碰字面量构造点、Services 贯通、Go 接口（全是 T0c）；不加任何依赖（contracts `Cargo.toml` 不动）。

### ② 补丁脚本 `review/t0/p1-contract-patch.py` + `review/t0/data/*.txt`

- 形状照 AA4/BB2：每处改动一个 `Edit(rel, label, old, new)`，**锚点恰好命中 1 次**（0 次、2 次都整份拒写）；同文件多处按顺序叠加；
  新文件（`domain.rs`）只在目标不存在时创建；**全有或全无**；写完读回验证。大段内容放 `review/t0/data/*.txt`（扁平文件名，
  名字里不含 `proto`、`contracts/`、`claude`、`go.mod`、`go.sum`、`.lock`，例 `ctr_src_domain.rs.txt`）。
- **幂等**：每条 `Edit.old` 在 `apply` 之后必须不复存在（纯追加型改动要把锚点扩到会被改写的相邻文本，照 BB2 `:84-87` 的做法；
  否则 `pub mod session;` → `pub mod session;\npub mod domain;` 这类锚点第二次还命中 1 次、会被打两遍，③ 的 f 步就报不出「已经打过」）。
- 数据目录按脚本自身定位：`DATA_DIR = Path(__file__).resolve().parent / "data"`，**绝不用 `root / "review/t0/data"`**（副本是 clone 出来的，没有你未提交的数据文件）。
- 锚点按 **B0 + AA4 + BB2 + BB4** 的文本量（总管真打时 main 上已有 P0-CLOSE；自测也先套 P0-CLOSE 再套 T0，两边形状一致）。
- 受保护路径在脚本里**分段拼**（照 BB2 `:65-71`：`"core/crates/" + "contracts/src"`），只用 Python 标准库（不许 `import yaml`）；
  `REPO_ROOT = Path(__file__).resolve().parent.parent.parent`（本脚本比 AA4/BB2 深一层）。
- 命令行：`--check`（干跑）、`--root DIR`（改副本）、`--self-test [--scratch DIR]`、`--codegen --root DIR`、`--estimate`（可选）。
  退出码：**0** 通过；**1** 锚点对不上 / 闸门不过 / 已经打过；**2** 未授权 —— 不带 `--root`（或 `--root` 规范化后等于真仓库根）且环境里没有总管设的重锁变量；
  `--codegen` 不带 `--root` 一律 2。
- 有效性闸门（任一不过就一个字节不写）：5 份 `.proto` 一起摆进临时 `aite/v1/` 让 `protoc` 解析（照 AA4 `:188-220`）；每个 `.rs` 走
  `rustfmt --edition 2024 --emit stdout`（stdin，**cwd 设在 `<树>/core`**）且输出==输入；`server.go` 走 `gofmt` 且输出==输入。

### ③ `--self-test`：所有受保护操作都在脚本里做（命令文本里只出现脚本名）

按顺序，每步打一行 PASS / FAIL / SKIP，最后一行 `[T0 self-test] 全部通过` 或列出没过的步：

- a. `git clone --local` 当前 HEAD 到副本（默认 `/tmp/aite-t0-scratch`，先删后建；有 `.git`，guard.rs 与 hook 的 `git rev-parse` 要它）。
- b. 判情形：按内容探测副本里 P0-CLOSE 是否已在（`evidence.rs` 里有 `chain_hash_at`、守卫里有 `_bb4_precheck`、`edge.proto` 里有 AA4 的新注释）。
  没有 → 依次跑 `aa4-proto-patch.py`、`bb2-created-at-chain-patch.py`、`bb4-guard-patch.py` 的 `--root <副本>`（BB2 的 subprocess **cwd = `<副本>/core`**，它的 rustfmt 要工具链）；
  三个标记都在 → SKIP 并在报告里写「情形 B」；只探到 1–2 个 → 本步 FAIL「P0-CLOSE 部分落地」，写进回执、停（既不能 SKIP 也不能重套）。
- c. 在副本 `core/` 下 `cargo build -p aite`（副本自己的 target，**绝不与真树共用 `CARGO_TARGET_DIR`**：build.rs 缓存绝对路径会让 protoc 假红），
  把二进制拷到 `/tmp/aite-t0-prelock/aite`（= 补丁前的二进制）。
- d. 套 T0 前跑一次 `cargo test -p aite-contracts`（期望 `contracts passed=27 failed=0`，`constants` 钉 p0.2、`rejects_bad_shapes` 拒 dingtalk）。
- e. 以子进程跑本脚本 `--root <副本>`（cwd = `<副本>/core`）→ exit 0，N 处锚点全部命中 1 次、0 miss。
- f. 再跑一次 `--root <副本>` → exit 1「已经打过」，补丁面文件 sha256 前后一致（没写任何字节）。
- g. `cargo test -p aite-contracts` 与 `cargo test -p aite-proto` 分开跑、分开计数 → `contracts passed=27+K failed=0`、`proto passed=M failed=0`。
- h. 变异（≥3 处，都要能编译）：如 `CONTRACT_VERSION` 改回 `p0.2`、`OPEN_TASK_STATUSES` 里的 `AwaitingApproval` 换成 `Delivered`（换不删：它若照
  `ACTIVE_TASK_STATUSES`（`session.rs:33` 的 `[TaskStatus; 3]`）写成定长数组，删一项就编不过）、convert 拒 `trusted` →
  各自点名的测试变红 → 用 `write_text` 写回原文（新 mtime；**别用 `cp -p` / `copy2`**）→ 重跑变绿。红与绿的输出都进日志。
- i. `--codegen --root <副本>`：`protoc -I proto --go_out=edge --go_opt=module=aite/edge --go-grpc_out=edge --go-grpc_opt=module=aite/edge` + 5 份 proto（= Makefile `proto-gen`，`:47-48`），
  插件先用 `shutil.which` 在 PATH 上找（§4.1 软链在 `/usr/local/bin`），找不到再试脚本里子进程取的 `go env GOBIN` / `go env GOPATH` + `/bin`（不假设 `~/go/bin`），
  再不行才用开场自检第 3 步说的副本内补装；定位到后显式 `--plugin=protoc-gen-go=<路径>` 并核版本（`v1.36.12` / `1.6.2`）；生成物只留在副本，**永不提交**。
- j. 副本 `edge/` 下 `go build ./...` 与 `go vet ./...` → exit 0（**不跑 go test**：`main_test.go:202` 的 p0.2 钉归 T0c）。
- k. `/tmp/aite-t0-prelock/aite contracts lock --check --repo <副本>` → exit 1，`MISMATCH n file(s):`（stderr），逐条 `  changed  <rel>` / `  added    <rel>（新文件未入锁）`（`lock.rs:140-149`；
  每条 changed 后面还跟 `    locked <hash>` / `    actual <hash>` 两行，`:143`）；**按路径集合比对，不比哈希行**：期望 = 本脚本 EDITS 里落在锁面内的文件 ∪（情形 A 时）P0-CLOSE 的 4 个锁面文件。按本清单推算：T0 锁面集 21 个
  （5 proto + contracts src 改 11 个 + 新增 `domain.rs` + tests 4 个），与 P0-CLOSE 的 `edge.proto`、`lib.rs` 重叠，所以情形 A = **23**（22 changed + 1 added），情形 B = **21**。以实测为准，差异写回执。
- l. 报告里核一遍 `review/t0/data/` 的文件名不含上面那些子串。
- `--estimate`（可选，限时 30 分钟）：副本上 `cargo check --workspace --all-targets --message-format=short`，按 crate/文件归并 E0063 / E0004 / E0046，
  写 `review/t0/companion-todo.md` 给 T0c 当起点（只是估计；已知一处：`edge-client/tests/common/mod.rs:203` 的 fake `PlatformService` 要补 12 个 RPC）。

T0c 伴随清单（设计文档要原样交接）：结构体字面量与穷举 match（`Message, Turn, Session, NormalizedEvent, ChecklistCard, OutboundText, HistoryMessage, PlatformCapabilities,
ToolContext, Usage, ModelConfig, RunHooks` 与各新枚举，范围 = 分支上 `cargo check --workspace --all-targets` 报的）；**5 处 p0.2 字面量钉**：`edge/cmd/aite-edge/main_test.go:202`、
`core/crates/evidence/tests/manifest.rs:65`、`core/crates/evals/tests/evals_runner.rs:111`、`core/crates/evals/tests/protocol_probe.rs:523`、`core/crates/app/tests/cli_smoke.rs:481`；
`EventKind::Reaction` 进 R4 只计数（排在 R5/R6 之前）；新 `CardActionKind` 记日志后丢；`Services` 贯通（`WorkerDeps` / `ControlDeps` / gateway `with_services()` / `FeatureCtx.services`）
并同一遍给每个 `WorkerDeps` 字面量加 `aigc_label: Option<LabelConfig>`；`ports_p1.go` + `services_p1.go`；evals `EventSpec` 新字段；`max_parallel_tasks` 接 CC2 的 `with_max_parallel`（默认翻 4），
`context_max_tokens` 经 CC3 的 `AgentWorker::with_context_max_tokens` 接进预算（T0c 原卡第 8 项；`review/paste-CC3.md` §5⑦）；`edge/gen/**` 永不由 T0c 重生成。
另列 **W1 各轨记账转给 T0c 的**（不在 T0c 原卡 `companion_edits` 里，来源见括号，以各轨回执为准）：`run.rs` 的 `takeoff` 调 `features::start_all`（EE7 / EE2 / DD12 依赖；`review/paste-CC4.md` §5⑧）；
`wiring.rs` 的 docker 档（`docker_sandbox_factory`）挂登记与 `gateway_options`（`review/paste-CC4.md` §5⑦）；`plane_factory` 把 CC7 的 `worker_options.aigc_label` 接进 `WorkerDeps`，
并撤掉 CC7 的「无消费方」闸门及其测试 `worker_options_label_without_consumer_is_a_wiring_error`（`review/paste-CC7.md` §5⑥；CC7 标为计划缺口、请总管确认归属）。

### ④ `review/t0/APPLY.md`：总管的 H11 人跑链，每步带期望输出

在 `~/Documents/Projects/Aite`、main（W1 已合、P0-CLOSE 已落）上，每条命令 `cd` 与命令同一行：编 `aite` 并拷到 `/tmp/aite-prelock` → `git checkout -b contract/p1.0` →
授权 `--check`（逐条「命中 1 条」+ 将动 25 个文件、其中新建 1 个）→ 真跑 → `make proto-gen`（`edge/gen/aitepb/` 6 个文件变；文件头仍是 `protoc v7.36.1`——
这条期望**抄自已提交的 `edge/gen` 文件头与 `CLAUDE.md` 工具链一节**，不来自自测：云端 protoc 31.x 生成的文件头本来就不同）→
契约测试**分两条跑**（与自测 g 步同口径，每条按 check.sh C1 的 awk 对本 crate 的 `test result` 行求和，不跨 crate 相加）：
`cargo test -p aite-contracts` → `contracts passed=27+K failed=0`（K 逐条点名）、`cargo test -p aite-proto` → `proto passed=M failed=0` →
`/tmp/aite-prelock contracts lock --check` → **`MISMATCH 21 file(s):`，21 条 = 20 条 `  changed  <rel>` + 1 条 `  added    core/crates/contracts/src/domain.rs（新文件未入锁）`**，
逐条列出路径、按路径集合比对（每条 changed 后的 `locked` / `actual` 哈希行不要求逐字：情形 A 的自测副本没经 P0-CLOSE 重锁，`lib.rs`、`edge.proto` 的 locked 哈希必然不同）→
授权 `lock --write` → `wrote 26 files -> .contracts.lock` → `--check` → `OK 26 files` → `(cd edge && go build ./... && go vet ./...)` → exit 0 →
提交照总计划 §8 H11 那一行写全：`cd ~/Documents/Projects/Aite && git add -A proto core/crates/contracts core/crates/proto edge config .contracts.lock && git status --short && git commit -m "contract(p1.0): T0" && git push origin contract/p1.0 && git checkout main`；
期望 `git status --short` 里有 `A  core/crates/contracts/src/domain.rs`、24 行补丁改动文件（`M `）、`edge/gen/aitepb/` 的 6 个文件、`.contracts.lock`，共 32 行（未跟踪的 `??` 行不算）；
**没有 `A  …/domain.rs` 就停**（`commit -a` 带不上新文件，漏了它 `contract/p1.0` 上 contracts crate 就编不过，T0c 和之后各波全从坏分支起步）。
写明：这几行在**普通终端**里敲（总计划 §8 开头：带重锁变量的命令别用 Claude 会话的 `!` 前缀）；**工作区其余部分按预期编不过、check.sh 按预期红，直到 T0c 合并**；
分支上 CI 不跑；`--check` 报锚点 0 命中 = P0-CLOSE 实际落盘与脚本不同 → 停，另开会话按设计文档在新 sha 上重锚。**除上面注明来源的文件头与 `git status` 两条，每个「期望」都抄自自测日志的实测，不许凭推算写。**

### ⑤ 回执 `review/p1/ledger/T0.md`（格式见 §8）

## 6. 规则

- 可写面 / 只读面见 §3；需要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。本轨对真树**零代码改动**。
- **B8 不变量**：p0 场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；`!stop` 仍立即释放沙箱（07 `release min:1`）；
  对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 不许改。B8 红了就停下报告。
- **守卫**：被拦就停、原文进回执、不许绕（唯一例外：开场自检第 2 步那次 Read 被拦是期望结果）。云端命令里永不出现：重锁变量赋值、守卫点名路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、`.claude/settings.json`、
  `guard_bash.py`、`proto/…` 路径、`core/crates/proto`，副本里的同名路径也一样）、包着它们的 `$(…)`。多行脚本写成文件再跑。check.sh 不接 `| tail`。
  守卫对「引号跨行」一律拒（`命令无法解析: No closing quotation`），所以 **PR 描述、回执、多行提交信息一律先用 Write 工具写成文件**（回执本身就是
  `review/p1/ledger/T0.md`；PR 正文写到 scratch 文件，如 `/tmp/t0-pr-body.md`），再 `gh pr create --draft --title "T0: 契约 p1.0 补丁起草" --body-file <文件>` /
  `gh pr edit --body-file <文件>` / `git commit -F <文件>`；命令行上的 `-m` 只写单行。摘要里的重锁命令只写「见 `review/t0/APPLY.md`」，不抄原文。
  本轨专项：APPLY.md、设计文档、数据文件、回执 `review/p1/ledger/T0.md`、PR 正文文件**只用 Write 工具写**（heredoc 正文会被扫，APPLY.md 必然含重锁赋值）；**别 grep 重锁变量名或 `proto/`**（命令文本本身就会被拦）；
  要看副本里的文件用 Read 工具；AA4/BB2/BB4 只经自测的子进程跑。若连 `python3 review/t0/p1-contract-patch.py --self-test` 都被拦（例如守卫扫到脚本内容），
  拦截原文进回执，把 b 步及其后标「总管本机实测」，照样交付脚本与文档。
- 每个行为改动：回归测试 + 变异验证（本轨的等价物是 ③ 的 d/g 前后对照 + h 步变异；还原别用 `cp -p` / `copy2`）。
- 格式化：`rustfmt --edition 2024 <文件>`（不要 `cargo fmt --all`）/ `gofmt -w <文件>` —— 本轨只在副本里、经脚本做。
- 新第三方依赖、R0 文件（不在可写面里的）、锁定面 → 停下报告。Docker 测试封闭（本轨不跑 docker）。云端 protoc 生成的 `edge/gen` 永不提交。
- 自测冷编译约 20–40 分钟，超过 Bash 单条 600 秒上限：用 **Bash 工具的 `run_in_background` 参数**跑
  `python3 review/t0/p1-contract-patch.py --self-test > /tmp/t0-selftest.log 2>&1; echo "exit=$?" >> /tmp/t0-selftest.log`（或把这行写进 `/tmp/t0-run.sh` 再后台跑），
  完成通知到了再用 Read 工具读日志；日志原文进回执。check.sh 同理：`scripts/check.sh > /tmp/check-accept.log 2>&1; echo "exit=$?" >> /tmp/check-accept.log` 后台跑（开场自检那次写 `/tmp/check-open.log`），
  读整份日志（不 `| tail`）。**串行跑**：开场自检的 check.sh → 自测 → 验收的 check.sh，绝不同时跑两个（4 vCPU 上并发冷编译会把已知时序抖动测试挤红）。
- 已知时序抖动（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、`aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。

## 7. 验收（命令 + 期望输出）

1. `scripts/check.sh`（后台跑、自测结束之后，写法见 §6）→ 末行「全部通过」，各行与开场自检**逐字相同**（Δ = 0）：情形 A `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 全 ok；
   情形 B `901 / 27 / OK 25 / 10/10`。任何一行变了 = 越界，不是「增量」。
2. 自测（后台）→ 日志末行 `exit=0`、倒数第二行 `[T0 self-test] 全部通过`；报告里：情形 A 为 AA4 + BB2 + BB4 + T0 都套上、情形 B 为 P0-CLOSE 已在 + T0 套上，锚点 0 miss。
3. 同一报告：`contracts passed=27+K failed=0`（K 在回执里逐条点名）、`proto passed=M failed=0`；变异 ≥3 处红→绿。
4. 同一报告：codegen ok；副本 `go build ./...`、`go vet ./...` exit 0（不声称 go test 过）。
5. 同一报告：`MISMATCH 23 file(s):`（情形 A）/ `MISMATCH 21 file(s):`（情形 B），按路径集合与期望相等（不比哈希行）；APPLY.md 写明总管真跑（P0-CLOSE 已重锁）只见 T0 那 21 个。
6. `python3 review/t0/p1-contract-patch.py --root /tmp/aite-t0-scratch; echo "exit=$?"` → 「已经打过」，`exit=1`；
   `python3 review/t0/p1-contract-patch.py --check; echo "exit=$?"` → 拒绝，`exit=2`；`python3 review/t0/p1-contract-patch.py --codegen; echo "exit=$?"` → `exit=2`。
7. 不对整个工作区下任何结论：它在 T0c 之前本来就编不过。
8. `git diff --name-only origin/main...HEAD` → 全部落在 `review/t0/**`、`docs/p1/contract-p1.md`、`review/p1/ledger/T0.md` 内（Go 模块文件有没有被动，由总管审 PR 时看，你不用查）。

## 8. 回执（写 `review/p1/ledger/T0.md`，PR 描述贴摘要；两者都先 Write 成文件，PR 用 `--body-file`）

1. **开场自检原文**（4 项 + 本轨附加项）：diff 输出与判定的情形、守卫拦截原文、工具链与插件版本、check.sh 完整输出。
2. **工作项逐条**：每个产出文件 + 行数；补丁的 EDITS 总数、动到的 25 个文件（新建 1）逐个列出。
3. **新增测试逐条**：K 条契约测试 + M' 条 proto 测试，各钉什么；d/g 前后对照与 h 步变异的红/绿原文。
4. **自测日志全文**（或按步骤截取、不删行）+ `MISMATCH` 清单原文。
5. **check.sh 完整输出**；**cargo passed 增量**：真树 Δ = 0（写明）；副本里的 `27 → 27+K` 逐条来源。
6. **被守卫拦过的命令与拦截原文**（没有就写「无」）。
7. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）—— 至少：`edge-client/tests/common/mod.rs:203` 的 fake 要补 12 个 RPC（或 tonic 默认桩，但 `core/crates/proto/build.rs` 不在补丁面）→ T0c；
   `lock.rs:19` 注释里的「= 25」→ T0c；Go `config.go:94-99` 拒 dingtalk/wecom → DD8。
8. **没做的与原因**（含 `--estimate` 没跑完之类）。
9. **契约缺口**（给 T0 审稿 / T0.1）：写清需要什么形状、为什么开放通道绕不过去；与 W1 各轨接缝对不上的地方逐条列出（§5① 照录的 CC11 / CC5 / CC2 三项不算缺口，除非照录时与原卡冲突）。
   另写明：原卡说五个新 trait 的方法集「同契约先行草稿」，而那份草稿是过程材料、**没入库**（总计划附录 D）——本轨是按 DD1–DD3 的 goal 推出来的，请总管在 H11 审稿时重点看。
