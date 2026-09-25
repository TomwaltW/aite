# Aite 对齐 Claude Tag 总计划（中国化 · Claude Code 云端执行版）

> 2026-09-25 · 总管：沈思锴 · 起草：本机 Claude 会话（调研 + 设计 + 三方评审合并）
>
> - **代码基线** `98e4460`（main，CI 绿）。本机实测 `scripts/check.sh` 全绿：`cargo passed=897 failed=0`、
>   `contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 个包全 ok（check.sh 那格只显示 8 行，
>   `edge/cmd/aite-edge` 被 tail 截掉，单跑是 ok 的）。
> - **本文地位**：总管计划，不是冻结规格。契约批次的冻结规格由 T0 轨起草成 `docs/p1/contract-p1.md`，
>   总管审过、本机打补丁后才算冻结。`docs/dev-spec-*` 两份 P0 规格不动。
> - **依据**：Drive《Claude Tag 架构详解》（09-09）、《Aite 立项方案 v0.1》；claude.com/docs/claude-tag 官方文档
>   （09-25 逐条复核，多出 25 条原文档没写的机制，记作 NEW01–NEW25）；本仓库逐文件盘点（6 个只读阅读轨）；
>   飞书 / 钉钉 / 企微开放平台与六家国产模型厂商官方文档（09-25 抓取）；Claude Code 云端文档；
>   三种拆法（契约先行 / 分主题两车道 / 平台先行）× 三个评审视角打分后合并。
> - **第 1 波派单已写好**：`review/paste-T0.md`、`review/paste-CC1.md` … `review/paste-CC12.md`。
>   第 2 波起的派单**不预先写**——它们要钉的基线 sha 和条数要到同步点才知道（派单放久了会过期），
>   到时按 §6 的轨道卡片 + `review/p1/tracks-2026-09-25.json` 里的英文原卡生成。

---

## 0. 一页结论

**「和 Claude Tag 做一模一样的事」在这里的定义**：Claude Tag 官方文档里的 30 条核心机制（CT01–CT30）加
复核时新找到的 25 条（NEW01–NEW25），逐条对齐；做不到的地方写明降级方式，让你拍板接受，而不是悄悄缩水。

**能做到什么程度**：

- **飞书**：几乎完全对齐。平台能力上做不到一样的有 4 处：编辑消息只能「拉取比对」（飞书没有编辑事件）；
  工作区搜索要一个服务用户的 user token；托管网页 v1 用「应用建的飞书云文档 + 按群共享」代替任意 HTML 页
  （自有域名要 ICP 备案）；「非 @ 也能跟着看」要管理员批敏感权限 `im:message.group_msg`。
  另有产品 / 身份 / 合规类差异同样适用于飞书：顶层 @ 一律开话题（CT03，p0 评测冻结）、v1 私聊以应用身份跑且计入组织余额
  （CT20，W6 的 HH1 之前）、Git 用 GitLab bot 而不是 GitHub App、断开只删内容留审计元数据、AIGC 标识、单副本无高可用——全部见 §2.C。
- **钉钉 / 企微**：机器人只收得到 @ 消息、没有话题、没有群历史，所以会话靠**可见的任务号 `#A17` + 引用内容**
  续接（锚点协议），频道顶层会话 / Ambient / 群历史只能降级；企微流式进度 10 分钟必须结束。
- **中国化的部分**（Claude Tag 没有、我们必须有）：国产模型多厂商（各家思考字段回传规则不同）、
  国内镜像源的沙箱「可信网络」档位、GitLab/极狐代替 GitHub App、国内联网搜索、AIGC 显式 + 隐式标识、
  审计日志 ≥190 天且与内容分存、单租户私有化离线包（amd64 + arm64）。

**路线**：6 个波次、2 次人工契约落地、1 个可选的勘误批次。

| 波次 | 什么时候能派 | 干什么 | 轨数 |
|---|---|---|---|
| **W1** | **今天**（你推完 D0 之后） | 不碰契约的地基：拆大文件并预埋桩文件、助手回复进 transcript、并发派发（默认仍串行）、迁移机制、国产模型兼容、钉钉/企微/出网代理三个独立包、沙箱镜像中国版；**同时 T0 起草整批契约补丁** | 13（T0 + CC1–CC12） |
| 人工 | W1 期间 | **P0-CLOSE**：AA4 + BB2 一次重锁，BB4 守卫补丁；M1–M6 真机验收宣布 P0 完成 | — |
| 人工 | W1 合并后 | **T0 落地**：在分支 `contract/p1.0` 上打补丁 + `make proto-gen` + 用补丁前的二进制重锁 | — |
| **W2a** | T0 分支推上去后 | **T0c**：让整个工作区在 p1.0 契约上编得过（字面量、5 处版本钉、服务句柄贯通），并把并发派发默认翻成 4 | 1 |
| **W2** | T0c 合并后 | 契约上的地基：存储 P1、edge-client 新 RPC、锚点路由、执行循环 P1（思考字段回放、配置快照）、输出 P1（AIGC 标识机制默认关、卡片链接）、模型厂商档案、按线程沙箱、edge 接线、飞书读写、钉钉/企微补全、管理台 v0 | 12 |
| **W3** | W2 合并后 | 功能纵切：记忆、例程、预算、搜索、Git、托管页、审批、访问闸门、配对、凭证注入、外部触发、审计合规、飞书上下文深度、私有化离线包 | 14 |
| **W4** | W3 合并后（FF1 权限未批前只 @ 降级交付） | Ambient + 频道顶层会话、流式进度卡、DM 与换模型、自检扩组、PIPL 删除、管理台 v1、连接器包、多模型 live 矩阵、提示词与注入场景 | 9 |
| **W5** | W4 合并后 | P1 验收剧本 + 演示，你在三端真机跑 | 1 |
| **W6** | 你决定后（可选） | 个人连接器（NEW09）、自有域名托管页、钉钉 DWS 代理用户 | 3 |

**今天你要做的**（§8 有完整的人工队列）：

1. **H1 拍板**（§3，15 分钟）——尤其是依赖审批，第 1 波里不许有等后续审批的东西。
2. **H2 推 D0**：本计划 + `CLAUDE.md` + 13 份派单 + 7 份以前没入库的 review 文件（`backlog-2026-09-15.md` + `paste-BB1..6.md`）+ 仓库卫生（把误入库的 `.venv/` 等移出索引）。
3. **H3 配云端环境**（安装脚本全文在 §4.1）。
4. **H4 先只派 CC1**，看它开场自检里 `check.sh` 全绿、守卫被拦，再一口气派其余 12 轨。
5. **H5 飞书后台**：申请 `im:message.group_msg` 等权限（外部等待时间最长，今天就提）；把回调订阅设成长连接。
6. **H8 点一次卡片按钮**（10 分钟）——决定卡片按钮能不能用（见下面第 1 条发现）。

**改变计划的五个发现**：

1. **卡片按钮很可能本来就能用。** BB5 的结论是「lark-oapi-go 在长连接上丢弃卡片帧」，但飞书官方 2026-06-11 的
   《使用长连接接收回调》明确支持新版 `card.action.trigger`，它走的是 `type=event` 帧；被丢的 `type=card` 是**旧版**
   回调。仓库里 `OnP2CardActionTrigger` 早就注册了。点一次按钮看帧类型就能定：通过 → 停止 / 看证据 / 批准 / 驳回全走按钮；
   不通过 → 全走 `!stop` / `!approve` / `!reject` 文本命令。
2. **Aite 从来没把自己的回答写进 transcript。** 生产代码里 `TurnRole::Assistant` 零写入方（只有 `plane.rs` 写 User/SystemNote），
   同一话题里追问时模型看不到自己上一轮说了什么。W1 的 CC3 修。
3. **所有群的任务是全局串行的。** 一个长任务会卡住所有群（`plane.rs` 单循环）。团队产品不能这样。
   W1 的 CC2 做成「同会话串行、跨会话并行 N」，默认先保持 1，T0c 翻成 4。
4. **模型这边现在没坏，但也只测过一家。** 今天用你的 key 实测：`GET /models` 只剩 `deepseek-flash` 和 `deepseek-v4-pro`；
   配置里的 `deepseek-chat` 仍被接受、静默路由到 `deepseek-flash`（非思考模式），带工具能用；`deepseek-flash` 思考模式下
   第二轮不回传 `reasoning_content` 也返回了 200（调研里「会 400」没复现）。但 Kimi 不能传 temperature、Qwen 要显式开
   `parallel_tool_calls`、GLM-5.3 关不掉思考、豆包要回传 `encrypted_content`、MiniMax 把 `<think>` 混在正文里——
   一个 OpenAI 兼容客户端不够，要按厂商出档案（CC6 先做不改契约的部分，DD7 做完整的）。
5. **云端和本机不一样的地方**：云端会话看不到你的 `~/.claude`（全局 CLAUDE.md、记忆、peer-sessions hook 都没有），
   看不到未跟踪文件，只能推自己那条 `claude/*` 分支；有 Docker，没有 protoc；项目级守卫 hook 会照常跑
   （前提是单仓会话）。所以仓库里新增了 `CLAUDE.md`，派单写成自足的，受保护面的改动全部走「补丁脚本 → 你本机跑 → 重锁」。

---

## 1. 现状（实测 + 盘点）

### 1.1 P0 做到了哪

- **代码层面完成**：飞书单平台闭环（@ → 话题会话 → 清单卡片原地更新 → Docker 沙箱 → 结果回话题），
  Rust core 897 条测试、评测 `passed 10/10`、CI 两个 job（checks + compose-smoke）全绿。
- **没正式收口**：M1–M6 真机验收从没留下结果（`docs/acceptance-M.md` 是剧本、没有结果栏）；
  三份人工补丁（AA4 proto 注释、BB2 `created_at` 入哈希链、BB4 守卫）没跑；BB2 一落地，之前跑了一半的 M 系列证据全部作废，得从零重跑。
- **仓库卫生**：AA4 合并（`987a2cc`）误把 `.venv/`（14,439 个文件）、`aite.egg-info/`、仓库根的 `dev-spec-2026-09-09.md`
  （与 `docs/` 那份逐字节相同、但不在守卫保护面里）提交了进去；`review/backlog-2026-09-15.md` 与 `paste-BB1..6.md` 还是未跟踪文件。
- **本机仓库搬过家**：从 `~/Documents/Aite` 搬到 `~/Documents/Projects/Aite`，`core/target` 里 build script 缓存了旧绝对路径，
  会让 A1 / A5 / C1 假红（protoc「Could not make proto path relative」）。`cargo clean -p aite-proto` 之后全绿。
  台账里 AA4 / BB2 / BB4 的人跑命令链还指着旧路径和旧条数（853 / 873→874），§8 的 H6 已按新路径与 897 刷新。

### 1.2 离 Claude Tag 最远的十件事（按影响排）

| # | 缺口 | 证据 | 谁修 |
|---|---|---|---|
| 1 | 全局串行派发，一个长任务卡住所有群 | `control/src/plane.rs:1120-1125` | CC2 → T0c |
| 2 | 助手回复不进 transcript | `plane.rs:1322-1344` 只写 User/SystemNote | CC3 |
| 3 | 沙箱按任务、交付即释放；同话题追问从空 `/work` 开始 | `gateway/src/gateway.rs:56`、`worker/src/agent.rs:831-835` | DD6 |
| 4 | 能力位取了但 core 一处都没用；R6 写死只认群 | `edge-client/src/platform.rs:48-75`、`plane.rs:539` | CC2 → DD3 |
| 5 | 没有 Scope / Access Bundle / 凭证代理，沙箱安全只靠「完全不出网」 | `sandbox.rs` 只有 `None` | T0 → DD6 → EE10 |
| 6 | 没有记忆、例程、预算、审批、搜索、Git、托管页 | 目录里只有 5 个网关工具 | W3 |
| 7 | 模型客户端不回传思考字段、不按厂商调参、不流式、成本不算缓存命中 | `models/src/lib.rs:192-196, 267-271` | CC6 → DD7 |
| 8 | 边缘只订阅 2 类飞书事件（撤回 / 入群 / 成员 / 表情都没有） | `edge/internal/feishu/connection.go:142-154` | CC8 → DD9 |
| 9 | 没有钉钉、企微 | `PlatformChoice{feishu,fake}` | CC9/CC10 → DD8/DD11 |
| 10 | 没有管理台 / 配对 / 审计页 | 配置就是 `aite.yaml` | DD12 → EE9 → FF7 |

### 1.3 三端平台的真实能力（修正立项方案 §3.5 的地方）

- 飞书**没有**「消息被编辑」事件，只有 `im.message.recalled_v1`；编辑只能拉 history 的 `updated` / `update_time` 比对。
- 飞书消息搜索（`POST /open-apis/im/v1/messages/search`）**只收 user_access_token**；文档搜索 tenant token 即可。
- 飞书群历史除了 `im:message` 还要 `im:message.group_msg`；延时更新卡片的 token 30 分钟内**最多用 2 次**；
  CardKit 流式模式**开启 10 分钟后自动关**，流式中回调不能改卡；发消息每群 5 QPS（所有 bot 共享）。
- 钉钉卡片回调时限是 **2 秒**不是 3 秒；群里 @ 机器人**收不到文件**（只有单聊能收）；`groupMessages/send` **不能 @ 人**；
  引用回复字段 `text.repliedMsg` **没有文档**、官方 Go SDK 会丢掉它；AI 卡片流式每次 ≤1KB。
  钉钉 2026 年开源的 DWS CLI 能用「代理用户」拿到群历史 / 搜索 / 非 @ 消息，但它是 beta、要管理员开、占席位（W6 可选）。
- 企微智能机器人：每个 bot 只允许一条长连接（新踢旧，只能主备）；引用消息**不带 message id**（锚点只能靠可见文本 `#A17`）；
  流式 10 分钟（回调模式 6 分钟）；每用户同时最多 3 条在途回复；**没有官方 Go SDK**（自己写 ~500 行客户端）；
  成员 userid **除非 bot 由超级管理员创建，否则是加密的**——成员白名单与审计发起人要明文，所以企微 bot 须由超管创建，或另配自建应用换明文。

---

## 2. 对齐矩阵：Claude Tag → Aite

**读法**：一行一个机制。「三端可达」按 飞书·钉钉·企微 排；「需改契约?」写「是→T0」的，要等 T0 落地后才能做完；
「落地轨」是本计划里交付这一条的轨（括号里是波次）。矩阵里的行号基于 `98e4460`。

### A. CT01–CT30

| 编号 | Claude Tag 行为（一句） | Aite 现状（done/partial/absent + 关键文件:行） | 中国化设计（飞书/钉钉/企微） | 三端可达（全/降级/不可） | 差距工作项 | 需改契约? | 落地轨（波次） |
|---|---|---|---|---|---|---|---|
| CT01 | 频道内任何人 @ 即起任务；可按组织/角色限制谁能用 | done（仅飞书）：R7 @→新会话+ack（control/src/plane.rs:545-551）；R1 滤非人；无发起人 ACL | 飞 im.message.receive_v1 mentions；钉 Stream /v1.0/im/bot/messages/get（isInAtList）；企 aibot_msg_callback | 飞全·钉全·企全（钉/企群内仅文本类消息） | 发起人/角色白名单（env/SQLite）；core 真正消费能力位（edge-client/src/platform.rs:48-75 只缓存；plane.rs:539 硬编码 Group）；钉/企适配器手写 WS（gorilla 已钉 edge/internal/pin/pin.go:9，免 go.mod 补丁） | 适配器否；PlatformChoice/配置段 是→T0 | CC2(W1)、CC9(W1)、CC10(W1)、T0c(W2a)、DD8(W2)、DD11(W2)、EE8(W3)、GG1(W5) |
| CT02 | 话题=会话；transcript 存服务端，沙箱释放后仍在，下条回复重建；编辑前后与归档会话都保留 | partial：SQLite sessions/turns（store/src/lib.rs:36-53）；但从不写 Assistant 轮（只有 plane.rs:613/956/1055 的 User/SystemNote） | 飞 thread_id/root_id 作键；钉/企无话题→可见「#A17」锚点；transcript 同存厂商思考字段 | 飞全·钉降级·企降级 | ① deliver() 后 append_turn(Assistant)（worker/src/agent.rs:672-779）② provider_extra 入 transcript ③ 在 R7 之前加锚点解析：#A17→任务→会话 ④ 按租户留存 | ①④否 ②是→T0（Message.provider_extra）③是→T0（find_task_by_no） | T0(W1)、CC3(W1)、CC5(W1)、CC9(W1)、CC10(W1)、T0c(W2a)、DD1(W2)、DD3(W2)、DD4(W2)、GG1(W5) |
| CT03 | 频道顶层会话：简单问题顶层直答，复杂的开话题；1h 闲置/1 天/配置变更时替换；约 100 条后停读 | absent：SessionKind::Channel 从未创建（contracts session.rs:10-12）；R8 丢非 @ 顶层（plane.rs:553-556） | 飞书顶层 @ 仍建 Task 会话、在话题里回（B8：evals/p0/01 `sessions_equals 1` 冻结），频道会话只接非 @ 的顶层流量（需 im:message.group_msg）；钉/企只到 @ 层 | 飞降级·钉降级·企降级 | 频道会话用合成 thread 键；惰性替换（看 created_at/last_active_at）；开关默认关，保 evals/p0/01·02；约 100 条计数随 FF1（W4） | 否；正式按 last_active 查询→T0 | FF1(W4)、GG1(W5)、HH3(W6) |
| CT04 | 话题里任何人回复即可引导，无需再 @；运行中持续读新回复；!status 不打断 | partial：R6 免 @ 续接（plane.rs:538-543）；每步 drain steer（agent.rs:144-147）；但全局单循环串行（plane.rs:1120-1125） | 飞话题回复带 thread_id（非 @ 能否送达待 M4 §3.7a 实测）；钉/企须 @+引用或 #A17 | 飞全·钉降级·企降级 | 改为会话内串行、跨群并行派发（queue.rs）；steer/transcript 带发言人（worker/src/context.rs:65、plane.rs:987）；R6 补 ack | 否 | CC2(W1)、CC3(W1)、CC8(W1)、T0c(W2a)、DD3(W2)、DD4(W2)、FF1(W4)、FF10(W4) |
| CT05 | !restart 归档会话、新建并重读话题；卡死会话在下条消息时自动替换 | partial：!restart 归档后同 thread 重开（plane.rs:798-853），不重读话题；进程内卡死不替换 | 三端「@Aite !restart [#A17]」；飞重读话题历史，钉/企用 Aite 存的 transcript | 飞全（读历史需 group_msg，否则回落 transcript）·钉全·企全 | 新会话用 read_history(thread) 回灌；卡死判据（N 分钟无步进）→下条消息归档重开；Answering 孤儿纳入恢复（app/src/run.rs:364-379） | 否 | CC2(W1)、CC5(W1)、DD1(W2)、DD3(W2) |
| CT06 | 编辑→前后对照记入 transcript，不起新任务；删回复不通知；删根：有回复续、无回复归档 | partial：R4 编辑只记新文本（plane.rs:607-622）；edge 只订阅 2 类事件（edge/internal/feishu/connection.go:142-154），编辑/删除从未上线 | 飞无编辑事件：拉 history 的 updated/update_time 比对；删根走 im.message.recalled_v1；钉/企 bot 无编辑/撤回事件 | 飞降级·钉不可·企不可 | 订阅 recalled_v1→MessageDeleted（槽位已有）（CC8/W1）+删根语义（DD3/W2）；前后对照需 Turn.message_id 与 HistoryMessage 的 updated 字段 | 撤回 否；编辑对照 是→T0（Turn/HistoryMessage） | T0(W1)、CC8(W1)、DD3(W2)、DD9(W2)、EE13(W3) |
| CT07 | 上下文=话题+频道历史+置顶；中途 @ 取消息窗口；滤掉其它 bot；bot 发的不触发 | partial：R1 滤非人、历史只留人（context.rs:75）；历史是群级窗口 thread=None（agent.rs:294-300）；无置顶 | 飞 GET /im/v1/messages（container thread）+ /im/v1/pins；钉/企只有见过的 @ 消息+引用快照 | 飞全（群历史需 group_msg）·钉降级·企降级 | 中途 @ 取话题窗口（read_history 传 thread_id，CC3/W1；EE13/W3 收窗口）；置顶要新 RPC；bot 连环对话熔断 | 置顶 是→T0（edge.proto PlatformService） | T0(W1)、CC3(W1)、CC7(W1)、CC8(W1)、DD2(W2)、DD9(W2)、EE13(W3)、HH3(W6) |
| CT08 | 像普通成员一样关键词搜索公开频道；访客频道默认不可；可限定只搜已加入的频道 | absent：edge.proto 无 search RPC，工具目录无搜索 | 飞消息搜索只收 user_access_token（服务用户或请求者 OAuth），文档搜索 tenant token 即可；钉只能靠 DWS 代理用户；企无消息搜索 | 飞降级·钉不可（D8 只用机器人身份；HH3 开 DWS 后降级可用）·企不可（消息）/ 降级（文档） | search_messages/search_docs 工具（走 catalog）；Aite 服务用户 OAuth；跨群结果按请求者成员资格过滤 | 是→T0（edge.proto 搜索 RPC） | T0(W1)、DD2(W2)、DD9(W2)、EE4(W3)、HH3(W6) |
| CT09 | skills/插件/自定义指令在话题开始时锁定；连接与域名规则每次请求评估 | partial：config_snapshot 只有 model+initiator（plane.rs:1009-1020）；worker 每次重读 platform.md、用实时模型（agent.rs:127,320） | 与平台无关：快照 instructions/skills 哈希与 ProviderProfile；连接/域名每次请求评估 | 三端全 | 快照写入开放 Map config_snapshot，worker 执行时读快照；task_created 证据记快照哈希 | 否 | T0(W1)、DD4(W2)、DD6(W2)、DD12(W2)、EE8(W3) |
| CT10 | 每话题一个临时沙箱，闲置数分钟后释放、下次回复重建；沙箱内文件不保留 | partial：每任务一容器（gateway/src/gateway.rs:56 task_id→id），finish() 立即释放（agent.rs:831-835） | 与平台无关；镜像走离线 tarball、amd64+arm64；国内源烘进镜像 | 三端全 | 沙箱键改会话 id；delivered 不立即释放，按 `sandbox.linger_sec`（默认 600s）空闲后由 reaper 回收；cancel/failed 仍立即释放（保 evals/p0/07 的 release 断言）；解冻 spec W7 与 test_sandbox_handoff | 否（AcquireRequest.task_id 是裸串） | CC1(W1)、CC12(W1)、DD6(W2)、EE14(W3)、FF5(W4) |
| CT11 | 零连接基线：读话题/历史/置顶、搜工作区、沙箱跑代码（CSV→图）、联网搜索 | partial：5 个工具（contracts protocol.rs:82-114）+ CSV→图（evals/p0/04）；无置顶/搜索/联网 | 飞全量；钉/企群里 bot 收不到文件（只在单聊）→CSV 走单聊或云文档链接 | 飞全（读群历史需 group_msg）·钉降级·企降级 | 联网见 CT19（EE4/W3）；置顶/搜索见 CT07/CT08（DD9/W2 → EE4、EE13/W3）；生成文件加 AIGC 隐式标识 | 部分→T0（见 CT07/CT08） | CC12(W1)、EE4(W3)、FF10(W4) |
| CT12 | clone 授权仓库、推分支、开 draft PR（作者=Claude GitHub App），链接回帖、PR 反链话题 | absent：无 git 工具；沙箱只有 network none（contracts sandbox.rs:6-9、proto/src/convert.rs:497-508）；镜像无 git | GitHost 端口：GitLab/极狐项目访问令牌=bot 用户+「Draft:」MR；其次 Gitee（P1 只做桩）；Codeup / GitHub 不在 P1 | 三端全（钉/企反链到 #A17 任务页） | GitHost trait+GitLab 适配；git 工具走 catalog；沙箱装 git；代理注入令牌；过渡期可用 run_python 子进程调 git | 是→T0（SandboxNetwork 档位、ExecLanguage::Shell） | T0(W1)、EE5(W3)、EE11(W3)、FF8(W4)、GG1(W5) |
| CT13 | 长任务首条回复是就地编辑的清单（2000 字上限，忙话题 ≤15 分钟重贴）；有「thinking」确认；受阻会明说 | done（飞书）：卡片 PATCH 同一 message_id（edge/internal/feishu/platform.go:404-416），500ms 合并，ack 表情；按钮未渲染（cards.go:120-153） | 飞 CardKit 流式（10 分钟自关，流式中回调不能改卡）/PATCH 兜底；钉 AI 卡片 ≤1KB/次；企 stream 10 分钟内须结束 | 飞全·钉全·企降级 | 人工点一次→复用已注册的 OnP2CardActionTrigger（connection.go:147）恢复按钮；启用 Doing 态；R6 补 ack；页脚加「内容由 AI 生成」 | 按钮 否；受阻态（AwaitingApproval 入活跃集）是→T0 | T0(W1)、CC2(W1)、CC8(W1)、CC9(W1)、CC10(W1)、DD3(W2)、DD4(W2)、DD5(W2)、DD10(W2)、DD11(W2)、EE7(W3)、FF3(W4) |
| CT14 | 输出：回复、附件/图表、持续更新页、托管网页（按频道授权，会后仍在，话题里可更新） | partial：final→send_file+send_text（agent.rs:672-779）；卡片就地更新；ToolResult.artifacts 被丢弃（agent.rs:589-600） | 托管页 v1=应用建飞书 docx/多维表格，按 openchat 共享给群（免 ICP）；钉 AI 表格开放 API 只给内部应用；企智能文档（授权 7 天） | 飞降级（v1 用云文档代替任意 HTML 页，HH2 后全）·钉降级·企降级 | 自动交付 run_python 产物；文件加 AIGC 显式+隐式标识（标识器 CC12/W1；调用与显式标识 DD5/W2 默认关；EE12/W3 翻开）；docx 创建/共享 RPC | 是→T0（edge.proto 文档写 RPC） | T0(W1)、DD2(W2)、DD5(W2)、DD10(W2)、EE6(W3)、EE12(W3)、FF3(W4)、GG1(W5)、HH2(W6) |
| CT15 | 独立代理身份：IM 应用、Git App、每个工具一个服务账号；动作都记在代理名下；DM 例外 | partial：只有一个飞书自建应用身份（contracts config.rs:9-28）；无 Git/连接器身份 | 飞自建应用 bot；钉企业机器人 robotCode；企 aibot BotID（要明文 userid 需配自建应用）；Git=GitLab bot 用户 | 三端全 | 连接器服务账号表+凭证库；证据记录执行身份 | 是→T0（config 各平台身份段） | T0(W1)、DD8(W2)、DD9(W2)、DD11(W2)、EE5(W3)、EE10(W3)、FF8(W4) |
| CT16 | Scope（组织/工作区/频道）+ Access Bundle；频道内人人同权；可问「能访问什么」；回复页脚有 Configure 链接 | absent：只有裸 tenant/workspace/chat 字符串；catalog 忽略 ctx（gateway.rs:329-333） | org=tenant_key/corpId；channel=chat_id/openConversationId/chatid；bundle 存在 core | 三端全 | Scope/Bundle 表（SQLite）；gateway 按 ctx 解析并过滤目录；!access 命令；页脚链接到管理台 | 否（bundle 存 SQLite；若进 aite.yaml→T0） | T0(W1)、CC4(W1)、DD1(W2)、DD2(W2)、DD6(W2)、DD12(W2)、EE8(W3) |
| CT17 | 凭证库+Agent Proxy：凭证永不进沙箱/模型；三层放行，默认拒绝，只放 HTTP(S)；被拦请求可审计 | absent：安全只靠沙箱零出网，无代理/凭证库/白名单（edge/internal/sandbox/docker.go:189,524） | edge 用 Go 写 HTTP CONNECT 代理：默认拒绝+请求日志；带凭证的主机做 TLS 终结（Aite CA）；厂商 CLI 用假凭证 | 三端全 | 新建 edge/internal/egress（纯标准库）；Docker 内网；SandboxSpec 没有 env/挂载字段，CA 证书与 HTTPS_PROXY 由 edge 在 network≠none 时注入或烘进镜像（免改契约）；凭证库（SQLite 加密或客户 Vault）；网络事件日志留存 ≥190 天 | 是→T0（SandboxNetwork/sandbox.proto） | T0(W1)、CC11(W1)、DD6(W2)、DD8(W2)、EE10(W3)、FF8(W4) |
| CT18 | 环境网络级别：默认 Trusted（包源/开发主机），另有 No access、Full（默认关，按组织开） | absent：SandboxNetwork 只有 None；edge 不校验 network 串、直接传给 Docker（docker.go:537-539） | 档位 none / china-trusted（清华/阿里 PyPI、npmmirror、goproxy.cn、ustc crates（rsproxy.cn 大陆实测通过后再列）、apt 镜像、modelscope.cn 为主 / hf-mirror.com 为辅）/ custom / full（默认关） | 三端全 | edge 端校验 network 值（DD8/W2）；镜像内置 pip.conf/.npmrc/GOPROXY（CC12/W1）；各档预设（CC11/W1）；代理与 CA 就绪后 EE10 把生效默认翻成 trusted（离线私有化模板写回 none） | 是→T0（SandboxNetwork 枚举） | T0(W1)、CC1(W1)、CC11(W1)、CC12(W1)、DD6(W2)、DD8(W2)、EE10(W3)、EE14(W3)、FF5(W4) |
| CT19 | 联网搜索在服务端跑，不经代理；抓不到的页面也能引用；不受域名规则约束 | absent：工具目录无 web_search（contracts tests/protocol_tools.rs 锁死 10 个） | SearchPort：默认博查或智谱 web search，百度千帆兜底；结果包 <external> 进上下文；离线部署默认关 | 三端全 | worker 从 all_model_tools() 改用 gateway.catalog(ctx)（agent.rs:334；trait 已有 ports.rs:114）；web_search 工具；key 走 env | 否（走 catalog；契约里 `all_model_tools()` 仍是 10 个，解冻的是 §9 那条「worker 固定喂 all_model_tools()」） | CC3(W1)、EE4(W3) |
| CT20 | 只支持 1:1 DM；用用户个人账号/连接器运行，计入其席位；组织可禁 DM | absent：R6 要求 Group（plane.rs:539），p2p 落到 R8；SessionKind::Dm 未用 | 飞 im:message.p2p_msg+用户 OAuth；钉 1:1 机器人+用户 OAuth/DWS；企 aibot 单聊（个人授权 7 天） | 飞降级（HH1 后全）·钉降级·企降级 | v1：DM 用应用身份+共享只读连接+个人记忆，计入组织余额；DM 开关；个人连接器留到 W6（HH1，可选，见 NEW09） | 否 | T0(W1)、DD2(W2)、DD3(W2)、DD9(W2)、DD10(W2)、DD11(W2)、FF4(W4)、HH1(W6) |
| CT21 | 记忆按频道累积：公开频道的全局可读，私有频道自存、对工作区记忆只读；人人可问可改可删；管理员可审 | absent：无记忆表/工具（store/src/lib.rs:36-73） | 每群一份记忆文件（SQLite）；飞书群信息疑似有公开/私有字段（未实测，H9 探；T0 的 ChatInfo.is_public 是 Option，实测前一律按私有群）；钉/企无此信号→一律按私有群 | 飞全·钉降级·企降级 | memory_read/write 工具（走 catalog）；注入上下文（context.rs:123-138）；「你记得什么」命令；PIPL：只存工作事实，带来源 task_id | 否；公开属性需群信息 RPC→T0 | T0(W1)、DD1(W2)、EE1(W3)、FF7(W4)、GG1(W5) |
| CT22 | Routine：定时（带时区）/频道观察/PR 订阅，按频道权限运行；可自己排后续跟进 | absent：唯一的后台循环是 reaper（plane.rs:1206-1227） | core 调度（Asia/Shanghai）；飞主动发 5 QPS/群；钉 groupMessages/send（不能 @）；企只能发给来过消息的会话；MR 订阅需 edge HTTP 入口 | 飞全·钉降级·企降级 | routines 表；control 内部入口（绕开 ControlPlane trait）；schedule_followup 工具；!routines；GitLab webhook | 可选→T0（EventKind::Schedule 更干净） | T0(W1)、DD1(W2)、DD8(W2)、EE2(W3)、EE11(W3)、FF7(W4)、GG1(W5) |
| CT23 | 按频道开关「自动回复」（默认开）；主动提醒、跟进停滞话题、任务完成播报；成本随流量涨 | absent：supports_passive_listen=false，SetPassiveListen 无调用方（platform.go:209,230） | 飞需 group_msg 权限+快模型判定+每群每日上限，默认关；钉/企退化为 Routine+外部事件+#A 任务跟进 | 飞全(权限获批后)·钉降级(可升 DWS)·企降级 | 被动监听开关；快模型分类器；每群日上限与成本计量 | 是→T0（ModelConfig 多模型） | T0(W1)、DD10(W2)、DD11(W2)、FF1(W4)、GG1(W5) |
| CT24 | 仅 owner 5 步配对（一次性 15 分钟配对码/选首批工具/接 GitHub/建服务账号/设月度限额后上线），初始零权限 | absent：配置就是 aite.yaml + aite preflight（app/src/preflight.rs） | 飞每客户一个自建应用（长连接免公网），管理员按权限清单装，私聊发「!connect <码>」或群里「@Aite !connect <码>」；钉内部应用或 ISV 都能走 Stream；企由管理员建 API 模式 aibot | 飞全·钉全·企降级 | 配对码状态表；管理台（axum SSR+飞书 OAuth；axum 是新依赖需批准）；上线页展示模型与备案号 | 否 | DD9(W2)、DD11(W2)、DD12(W2)、EE9(W3)、EE14(W3)、FF5(W4)、GG1(W5) |
| CT25 | 扩展：连接（约 17 个预置+自定义）、插件、skills、自定义工具与 MCP，按 scope 配置 | absent：工具目录冻结在 10 个，无注册表 | 预置连接面向国内：飞书云文档/多维表格、钉钉、GitLab、数据库；厂商 CLI（lark-cli/dws/wecom-cli）在沙箱里经代理 | 飞全·钉降级(DWS beta)·企降级 | 连接注册表；skills（按 scope 的提示文件）；MCP 客户端（新依赖需批准）；5 个首发连接 | 是→T0（ExecLanguage::Shell 跑厂商 CLI）；另依赖 CT17 代理 | CC1(W1)、CC4(W1)、EE10(W3)、FF8(W4) |
| CT26 | 按量从组织余额扣费；组织级+频道级月度限额；频道用量明细；限流和限额分开；DM 计入个人 | partial：Task.cost 累计，卡片页脚显示 ¥（worker/src/card.rs:67）；无上限；cost_of 忽略缓存命中（models/src/lib.rs:267-271） | 人民币；按厂商分档计价（缓存命中/写入、上下文分档、DeepSeek 峰谷）；按 chat 汇总 | 三端全 | agent 循环加成本上限（agent.rs:135-142，env 配置）；tasks 表加 chat/cost 列并引入迁移机制；精确分档计价 | 是→T0（ModelConfig 价目、Usage.cache_write） | T0(W1)、CC5(W1)、CC6(W1)、T0c(W2a)、DD1(W2)、DD7(W2)、EE3(W3)、FF7(W4)、GG1(W5) |
| CT27 | 审计：服务账号的一切动作；审计页含定时任务、记忆文件、每小时网络事件导出；routine 记创建人 | partial：每任务哈希链证据（contracts evidence.rs）+ aite evidence show；created_at 未入链（BB2 待人跑）；命令/丢弃/send_text 不留证 | 审计与内容分开存（PIPL 下内容可删）；留存 ≥190 天且防删；等保二级审计项 | 三端全 | 补证：命令、!stop 发起人、丢弃、最终回帖（用 checklist_op/event_received 的新 op 值）；写入侧脱敏；审计页；网络事件导出 | 否（新 EvidenceKind 可选→T0；BB2 是人工补丁） | T0(W1)、CC7(W1)、CC11(W1)、DD1(W2)、EE2(W3)、EE12(W3)、FF6(W4)、FF7(W4) |
| CT28 | 限制项：禁 DM、访客频道模式、按 scope 限运行位置、收紧网络、按成员/角色、限搜索、按频道名拦截/自动加入 | absent：AiteConfig 只有模型/沙箱/worker 限额（contracts config.rs:153-164） | 飞 DMMode/可用范围，外部群默认关；钉忽略 conversationType=1；企忽略 chattype=single；群白名单 | 三端全 | 禁 DM、群白名单、成员白名单（SQLite，EE8/W3）；按 scope 和群名规则的放进管理台 | 否 | T0(W1)、DD9(W2)、EE8(W3) |
| CT29 | 只认人手打的触发；个人连接器防注入（别人的消息只当信息）；公开频道成员=访问权（混淆代理风险） | partial：R1 只认真人（plane.rs:505-510）；外部内容标「数据不是指令」（context.rs:28-29）；任何人都能 !stop/!restart 任意任务 | 飞 sender_type=user + bot 连环对话熔断；跨群读取前校验请求者是否在群；面向公众时加内容安全过滤（《暂行办法》第十四条；P1 无轨实现——是 SaaS 公开注册、外部群 allow / channel_only、HH2 公网页面的前置条件） | 三端全 | 工具输出也包 <external>；注入评测进 evals/p1；外部成员/访客不能 !approve/!restart/!mute（见 NEW08；群内成员仍同权，同 CT16） | 否 | CC3(W1)、CC7(W1)、DD6(W2)、EE4(W3)、EE7(W3)、EE8(W3)、FF9(W4)、FF10(W4) |
| CT30 | 多模型可选（按组织策略过滤）；管理员按 scope 设默认；用户说「用 X 模型」即切换；页脚显示当前模型 | partial：单一 openai_compat 模型（models/src/lib.rs）；只 live 测过 deepseek-chat；无 reasoning_content 回传/流式/路由 | 主力 qwen3.7-plus（百炼工作区域名），兜底 deepseek-v4-pro/flash，快模型 deepseek-flash；只用大陆端点；!about 显示模型名+备案号 | 三端全 | W1（CC3/CC6）：配置错 / 4xx 不再重试（agent.rs:324-345）、429 按 retry-after、厂商怪癖；W2（DD4/DD5/DD7）：页脚模型名、厂商档案（含自托管 selfhost 档）、provider_extra、多模型路由、海外端点默认拒绝；W4（FF4/FF9）：按 scope / 话题选模型、六家 + 自托管 live 矩阵 | 是→T0（Message/ModelConfig/Usage/ModelError） | T0(W1)、CC3(W1)、CC6(W1)、DD4(W2)、DD5(W2)、DD7(W2)、DD12(W2)、FF4(W4)、FF9(W4) |

### B. NEW01–NEW25

| 编号 | Claude Tag 行为（一句） | Aite 现状（done/partial/absent + 关键文件:行） | 中国化设计（飞书/钉钉/企微） | 三端可达（全/降级/不可） | 差距工作项 | 需改契约? | 落地轨（波次） |
|---|---|---|---|---|---|---|---|
| NEW01 | 命令 !help !configure !restart !status !mute !unmute !feedback !routines !fork；须单独成句（后三个可带参数） | partial：只有 !status !stop !restart !new；UNKNOWN_COMMAND_TEXT 逐字钉死（control/tests/wording.rs）；只认 ASCII「!」+U+0020（commands.rs:15-21） | 三端都用「@Aite !命令」；认全角「！」、U+3000、中文别名；另加 !approve !reject !about !access !evidence | 三端全 | 解冻 wording.rs 与 spec:742 的未知命令文本；命令逐个补（!evidence 需 find_task_by_no：T0 → DD1/W2，命令本身 EE12/W3） | 否（!evidence 除外→T0） | CC2(W1)、EE2(W3)、EE8(W3)、EE9(W3)、EE12(W3) |
| NEW02 | 给回复点 👎 即静音该话题：放弃未完成的回复并发通知；再回复或 @ 解除 | absent：无表情入站事件，无静音态 | 飞 im.message.reaction.created_v1；钉 bot 无表情事件（DWS 可）；企 feedback_event 近似 | 飞全·钉不可(可升 DWS)·企降级 | 会话静音态+取消在跑任务+通知；!mute/!unmute 文本版与表情入站都在 EE9（W3）做（CC2/W1 只放禁用桩，DD9·DD11/W2 送来 Reaction 事件） | 是→T0（EventKind 加 Reaction） | T0(W1)、DD9(W2)、DD11(W2)、EE9(W3) |
| NEW03 | !fork [#频道] <提示>：在新话题（本频道或另一公开频道）继续并互相链接；只限公开频道 | absent | 飞顶层新发一条作新 root，带摘要并互链；跨群只允许 is_public=true 的群、且请求者和 bot 都在群（与 Tag「只限公开频道」一致）；钉/企无话题→开新 #A 任务号 | 飞全·钉降级·企降级 | 同群 fork（OutboundText.reply_to=None 已支持）；跨群成员校验 | 跨群 是→T0（成员查询 RPC） | EE9(W3) |
| NEW04 | 附件上限：图约 3.75MB、PDF 5MB、其它 100MB，每条最多 5 个，多出的忽略 | partial：download_attachment 只清洗文件名（gateway/src/tools/attachments.rs:18-50），不限大小/数量 | 飞上传 30MB、图 10MB；钉群内只有 text/richText/picture，文件只在单聊；企群内无文件，下载链接 5 分钟有效且需 AES 解密 | 飞全·钉降级·企降级 | 大小/数量上限+超限提示；企微 AES-256-CBC 解密（标准库）；PDF 在沙箱转文本 | 否 | DD4(W2) |
| NEW05 | 读不了 Slack canvas；claude.ai 上的托管 artifact 按频道授权（非成员看到申请访问页） | partial：read_document 已能读飞书云文档（platform.go:564-602）；无托管页 | 飞 docx/wiki 可读（比原版强）；托管页=应用建 docx 并按 openchat 共享；钉/企见 CT14 | 飞全·钉降级·企降级 | 同 CT14 | 同 CT14（是→T0） | DD10(W2)、EE6(W3)、HH2(W6) |
| NEW06 | Slack Connect（跨公司共享频道）里不工作，@ 时给固定提示，无开关 | absent（平台默认已兜住）：飞自建应用默认进不了外部群 | 飞外部群默认不可用（需开对外共享）；钉企业机器人只进内部群；企外部群行为待查 | 飞全·钉全·企待查 | 与 NEW08 共用 `access.external_chat_mode`（默认 restrict：回固定提示、不执行）；EE8 的 channel_only / allow 比 Slack Connect「无开关」宽，属刻意差异，且只在 compliance 标明已确认登记范围、内容安全过滤就绪时才允许打开 | 否 | DD9(W2)、EE8(W3) |
| NEW07 | 不支持群 DM，只支持 1:1 | 不适用：国内 IM 没有独立的「群 DM」 | 多人会话一律按群处理 | — | 无 | 否 | 不做（国内无群 DM；DD3 保证只有 1:1） |
| NEW08 | 访客频道三种模式（Restrict 默认/Channel-only/Allow）；访客不能审批/重启/静音 | absent | 访客≈外部群成员/外部联系人；飞外部群需开启；钉企业机器人没有外部群；企待查 | 飞降级·钉不可·企待查 | 按 scope 设外部成员模式；外部成员禁用 !approve/!restart/!mute | 否 | T0(W1)、DD9(W2)、EE7(W3)、EE8(W3) |
| NEW09 | 个人连接器（限量开放）：用户授权后，本人的任务用个人权限跑并记在本人名下；他人请求仍用频道权限 | absent：只有 tenant_access_token（edge/internal/feishu/api.go:43） | 飞 user_access_token（约 2h）+refresh（需 offline_access）；钉用户 OAuth/DWS；企成员授权 7 天 | 飞全·钉降级·企降级 | 用户 OAuth 流程；个人凭证入凭证库；按发起人选身份；审计归属 | 否（依赖 CT17 凭证库） | HH1(W6) |
| NEW10 | 组织级默认 scope 是根，挂上的 bundle 全局生效，工作区/频道可覆盖 | absent | 根=租户（tenant_key/corpId）；私有化单租户就是一个根 | 三端全 | Scope 继承解析（同 CT16） | 否 | T0(W1)、DD1(W2)、DD12(W2)、EE8(W3) |
| NEW11 | Bundle 的 Domains 页：放行主机但不带凭证（公开 API/文档站） | absent | 代理第二层：国内文档站/公开 API 白名单 | 三端全 | 同 CT17 代理第二层 | 是→T0（同 CT17） | T0(W1)、CC11(W1)、DD6(W2)、DD12(W2)、EE10(W3) |
| NEW12 | 「*」放行任意主机（仍不带凭证），默认关，由 Anthropic 按组织开启 | absent | full 档默认关；owner 在管理台自己开，开时提示数据出境风险，全量记日志 | 三端全 | 同 CT18 的 full 档 | 是→T0（同 CT18） | CC11(W1)、DD12(W2)、EE10(W3) |
| NEW13 | 启用 ZDR/CMEK 的组织用不了（transcript/记忆需要服务端留存） | 不适用：私有化时数据本来就在客户侧 | 改为按租户设留存策略+内容删除（墓碑）+审计保留 | — | 不照抄该条款；等价物是留存/清除策略（见 CT27、NEW23，W3） | 否 | EE12(W3)（改为内容/审计分存） |
| NEW14 | 组织必须开启 Routines，否则所有 @ 都回「Claude is unavailable」 | 不适用 | 这是和 Anthropic 基础设施的耦合，无对应物 | — | 无 | 否 | 不做 |
| NEW15 | 配置完成前 @ 会收到「Claude is disabled in this channel」，不跑任务 | absent：一拉进群就开始工作 | 未配对/未启用的群→回固定中文提示，不起任务 | 三端全 | 启用标志+群白名单（env/SQLite）+固定文案 | 否 | EE8(W3)、EE9(W3) |
| NEW16 | 月度限额预设 $500/$1k/$2.5k/$5k/不限/自定义（≤$1M），每月重置 | absent | 人民币档位（D24）+自定义；按北京时间自然月重置 | 三端全 | 月度账本+档位 UI；精确计价见 CT26 | 否（计价→T0） | T0(W1)、EE3(W3)、EE9(W3) |
| NEW17 | 启动赠金：用量页已抵扣部分显示 $0，分析页「原价」列含抵扣额 | 不适用 | 无赠金 | — | 无 | 否 | 不做 |
| NEW18 | 上线时给全员发介绍 DM（可关），不计用量 | absent | 飞可按可用范围私聊；钉 oToMessages/batchSend；企 aibot 不能主动私聊没对话过的成员；默认关 | 飞全·钉全·企不可 | 可选：上线时给成员私聊介绍（EE9，默认关；飞书 / 钉钉可做，企微做不到），不做群公告 | 否 | EE9(W3) |
| NEW19 | 频道回复页脚带 Configure 链接；DM 和组织共享频道里没有 | absent：卡片页脚是纯文本（edge/internal/feishu/cards.go:113-118） | 飞卡片 URL/markdown 链接；钉卡片链接；企 markdown 链接；私有化指向内网地址 | 三端全 | 页脚加管理台的群设置链接（DM 里不显示） | 否 | DD5(W2)、EE8(W3) |
| NEW20 | Routine 可指定时区，但按 UTC 跑，夏令时会漂 1 小时，需手动重排 | absent | 中国无夏令时：按 Asia/Shanghai 固定 +08:00 计算，不会漂 | 三端全 | 调度记录时区和下次运行时间；不引入时区库 | 否 | EE2(W3) |
| NEW21 | Routine 可发到：所在频道、另一公开频道（需已加入且授权）、创建者 DM；不能发私有/共享/访客频道或别人的 DM | absent | 飞主动发（每群 5 QPS，所有 bot 共享）；钉 groupMessages/send 不能 @；企只能发给来过消息的会话，每会话 30 条/分钟 | 飞全·钉降级·企降级 | 校验输出目标（飞书只允许所在群、另一个 is_public=true 且已加入并授权的群、创建者私聊；钉/企一律视为私有群，只允许所在群 + 创建者） | 否（公开属性→T0） | T0(W1)、EE2(W3) |
| NEW22 | 第一次被加进频道时贴自我介绍+建议任务（被 bot 拉入、共享频道、已有记忆时不贴；再邀请不贴） | absent：EventKind::BotAdded 有槽位但 edge 从不发出（normalize.go:621-631） | 飞 `im.chat.member.bot.added_v1`（CC8 已订阅成 BOT_ADDED，H5 已列）；企 enter_chat 只用于单聊欢迎（5s 内）；钉待查 | 飞全·钉待查·企降级 | 订阅入群事件→BotAdded；判断是否首次（无记忆且没贴过）；介绍文案带 AIGC 标识 | 否 | CC8(W1)、EE1(W3)、EE9(W3) |
| NEW23 | 断开工作区即永久删除会话/transcript/记忆/routine/artifact/scope/账号绑定；应用保留，可重新配对 | absent：无清除路径 | 注销时删内容（网络数据安全管理条例第 22 条），审计元数据留存 ≥190 天（网络安全法）；先墓碑再清除 | 三端全 | 按租户清除命令；证据链校验支持「删内容、留哈希」 | 是→T0（PlatformPort / edge.proto 增 DeleteDoc：删除应用建的云文档或撤销 openchat 共享）；证据「删内容留哈希」在 FF6（evidence crate） | DD1(W2)、EE12(W3)、FF6(W4) |
| NEW24 | Claude Tag（共享身份、频道权限、PR 作者=App）和 Claude Code in Slack（以请求者身份）是两个产品 | 不适用 | Aite 只有共享代理身份一种 | — | 无 | 否 | EE5(W3)（MR 作者=bot 用户） |
| NEW25 | Microsoft Teams 标为「即将支持」，目前只有 Slack | 不适用 | 对等的扩展是钉钉、企微（见 CT01） | — | 不做 Teams；等价扩展是钉钉、企微：W1 出适配器包（CC9 / CC10），W2 接线与补全（DD8 / DD11） | 否（钉/企的 PlatformChoice 算在 CT01） | CC9(W1)、CC10(W1)、DD8(W2)、DD11(W2)、HH3(W6) |

### C. 做不到完全一样的地方（需要你接受的取舍）
- CT06 编辑语义：飞书没有「消息被编辑」事件，只能拉 history 的 updated/update_time 比对，所以编辑到下一轮才被发现，还耗配额；钉钉和企微的 bot 收不到用户编辑或撤回事件。用户要接受：飞书是延迟感知，钉钉/企微完全感知不到。
- CT03/CT04/CT23 非 @ 消息：飞书要申请敏感权限 im:message.group_msg，需租户管理员审批，批下来后 Aite 能看到群里全部消息，是隐私和主动性的取舍；钉钉和企微的 bot 只收 @ 消息，续接只能靠 @ 加 #A17。钉钉可以升级到 DWS 代理用户，代价是付费席位、管理员开启、beta 协议，且账号必须标明是 Aite，由用户拍板。
- 钉钉和企微没有话题：会话锚点只能靠可见文本「#A17」加引用内容解析（企微 quote 不带 msgid，钉钉 repliedMsg 没有文档）。每条 Aite 消息都必须带任务号；同一发起人 30 分钟内有不止一个未结任务时不猜：按新任务处理并明说「未带 #A 号，已按新任务处理」（DD3 的解析顺序），锚错了由用户引用带 #A 的消息纠正，体验比 Slack 话题差。
- CT08 工作区搜索：飞书消息搜索只接受 user_access_token，要么建一个 Aite 服务用户（占席位，refresh 需 offline_access，约 7 天轮换），要么用请求者本人授权，bot 做不到像普通成员那样搜；钉钉只能走 DWS 代理用户；企微不能搜消息，只能搜文档。
- CT13 流式进度卡有平台上限：飞书 CardKit 的 streaming_mode 开启 10 分钟后自动关，流式中回调不能更新卡片；钉钉 AI 卡片每次 ≤1KB、总量约 3KB；企微同一个 stream 必须在 10 分钟内结束。但 10 分钟上限只卡「流式」，不卡清单卡本身：飞书可以在 card/settings 里把 streaming_mode 重新打开，也可以退回 im PATCH（同一 message_id 14 天内可改，单条 5 QPS、30KB，P0 今天就这么做），所以清单卡能一直就地编辑到底；钉钉卡片实例用 PUT /v1.0/card/instances 更新，能更新多久未核实（H9 探）；真正只能改成「9 分钟收尾 + 阶段消息 + #A17 进展页」的只有企微 stream（长连接 10 分钟、回调模式 6 分钟）。
- 卡片按钮：BB5 的结论（SDK 丢帧）与飞书官方文档冲突，要人工点一次按钮实测才能定。如果确实收不到，停止、审批、静音只能退回 !stop / !approve / !reject / !mute 文本命令。各平台回调时限不同：钉钉 2 秒，企微 5 秒，飞书 3 秒且不重试。
- CT21 记忆的公开/私有语义：钉钉和企微的 bot 拿不到群的公开属性，只能一律按私有群处理，各群记忆不互通。这和 Claude Tag「公开频道记忆全工作区可读」不等价。
- CT12/CT15 Git 身份：国内没有 GitHub App 的等价物。GitLab/极狐的项目访问令牌会生成 bot 用户，但默认 365 天过期，需要轮换；Gitee 和 Codeup 只能用专用成员账号加个人令牌，占席位，提交归属是「人」而不是「应用」；GitHub 从大陆访问不稳，P1 不做。
- CT20/NEW09 DM 与个人连接器：企微的个人授权只有 7 天，国内个人连接器生态也弱。v1 的 DM 只能用应用身份 + 共享只读连接 + 个人记忆来跑，费用计入组织余额，做不到「以个人账号运行、计入个人席位」。
- CT30 模型：没有单一的前沿模型，各厂商的思考与回传契约都不同：GLM-5.3 关不掉思考；Kimi 不能传 temperature；豆包要回传 encrypted_content；MiniMax 把 <think> 混在 content 里；Qwen 要显式开 parallel_tool_calls。今天实测 deepseek-flash 在第二轮工具调用时不回传 reasoning_content 仍返回 200（调研里「会报 400」没复现），但对质量的影响还没评估。质量和延迟与 Claude 不等价，必须逐个厂商跑 live eval；私有化时能力取决于客户 GPU 能跑的模型（Qwen3.8-27B、DeepSeek-V4-Flash 级别）。
- CT19 联网搜索：离线或私有化部署只能默认关闭；国内搜索供应商对结果缓存和存储的条款没有公开，需要书面确认；Kimi 的 $web_search 在 2026-10-20 下线，不能选它。
- CT14/NEW05 托管网页：自有域名要做 ICP 备案和公安备案，付费 SaaS 可能还需要 ICP 经营许可证；OSS/COS 的默认域名会强制下载 HTML。v1 只能用飞书云文档加 openchat 共享来代替任意 HTML 页面。钉钉 AI 表格开放 API 只给内部应用；企微文档授权 7 天，bot 只能改自己建的文档。
- CT27/NEW23 删除和留存冲突：网络安全法要求网络和审计日志留存 ≥6 个月（取 190 天下限），《网络数据安全管理条例》第 22 条要求注销时删除。只能「删内容、留审计元数据和哈希」，做不到 Claude Tag 那样断开即全删。
- 企微 aibot 的连接和主动发送：每个 bot 只允许一条 WS 连接，新连接会踢掉旧的，只能做主备、不能多副本；主动发送只能发给已经给 bot 发过消息的会话，每会话 30 条/分钟、1000 条/小时，回复也算在内。所以企微上的 Routine、Ambient、NEW18 全员介绍都会降级或做不了。
- CT11/NEW04 文件：钉钉和企微的 bot 在群里收不到文件，只在单聊里能收；企微的下载链接 5 分钟有效，还要 AES 解密。「群里丢个 CSV 让它画图」在钉钉/企微上只能走单聊或云文档链接。
- CT07 上下文：钉钉和企微的 bot 读不到历史和置顶，上下文只有 Aite 自己见过的 @ 消息和引用快照。
- 单副本、无高可用：飞书长连接是集群模式投递，steer/owned/running 等状态又存在进程里，v1 只能跑单副本，与 Anthropic 托管服务的可用性不等价。
- CT17 凭证注入要做 TLS 终结：沙箱要信任 Aite 自签 CA，做了证书固定的 CLI 或 SDK 会失败，需要逐个工具验证。
- 合规上的差异：面向公众时，《人工智能生成合成内容标识办法》+ GB 45438-2025 要求回复、卡片、文件带显式标识「内容由 AI 生成」、文件写隐式元数据，还要内容安全过滤并展示模型名和备案号；单租户私有化大概率不在范围内，但标识成本低，D14 仍默认开。Claude Tag 没有这些，属于做不到完全一样。
- CT03 顶层 @：Claude Tag 由频道会话接、简单问题在顶层直接答；Aite 因 p0 评测冻结（01 `sessions_equals 1`），顶层 @ 一律开话题，只有非 @ 流量（FF1、需 group_msg）才可能在顶层直答。

### D. 刻意不照抄的
- Slack AI 侧边栏和 Home 页模型选择器：飞书、钉钉、企微都没有给第三方的对等入口（飞书 aily 是独立的一方产品）。改为自然语言「用 X 模型」加管理台按 scope 设置。
- Slack Connect 相关行为（NEW06）：国内对应外部群，自建应用默认就进不了。外部群与 NEW08 共用 `access.external_chat_mode`（默认 restrict：回固定提示、不执行）；EE8 实现的 channel_only / allow 比 Slack Connect「无开关」宽，属刻意差异——又因进外部群可能构成「向境内公众提供」，EE8 只在 compliance 标明已确认登记范围、且内容安全过滤就绪时才允许切过去，管理台开启时显示这一风险。
- 群 DM（NEW07）：国内 IM 没有这种独立形态，多人会话一律按群处理。
- 启动赠金和「原价」列（NEW17）：国内没有这个商业安排。
- 美元限额档位（NEW16）：不照抄 $500/$1k/$2.5k/$5k，改成人民币档位加自定义（数值见 D24：默认按 1:7 折算 ¥3,500 / ¥7,000 / ¥17,500 / ¥35,000 / 不限 / 自定义 ≤ ¥7,000,000），按北京时间自然月重置。
- ZDR/CMEK 组织不可用条款（NEW13）：私有化时数据本来就在客户侧，改为按租户的留存和内容删除策略。
- 「组织必须开启 Routines 才能用」（NEW14）：这是 Anthropic 基础设施的耦合，照抄没有意义。
- Claude Tag 与 Claude Code in Slack 两个产品的区分（NEW24）：Aite 只保留共享代理身份一种。
- Microsoft Teams 等待名单（NEW25）：不做 Teams，对等的扩展就是钉钉和企微（W1 适配器包，W2 接线与补全）。
- 托管在 claude.ai 的 artifact 页面：v1 不自建公网页面（要 ICP），改用应用创建的飞书云文档或多维表格，按 openchat 共享给群。
- 默认用 Claude GitHub App：大陆访问不可靠，默认改为 GitLab/极狐的 bot 用户；GitHub 不在 P1 任何轨里（EE5 只做 GitLab + Gitee 桩，GitConfig 的 github / codeup / gitea 在 P1 报「未实现」），需要时 W6 之后另开轨。
- DM 计入个人席位（CT20/CT26）：国内企业按组织采购，DM 用量也计入组织余额，按人统计明细。
- Routine 按 UTC 跑、夏令时漂 1 小时（NEW20）：Asia/Shanghai 没有夏令时，不会漂；其他时区 EE2 按固定偏移计算，有夏令时的时区照样漂 1 小时（与 Claude Tag 相同，文档写明）。
- 「自动回复」默认开（CT23）：国内默认关。原因有三：要审批敏感权限 im:message.group_msg、隐私顾虑、成本随流量涨。由 owner 按群开启。
- 上线时默认给全员发介绍 DM（NEW18）：默认关；可选项是上线时给成员私聊介绍（EE9；飞书 / 钉钉可做，企微做不到），不做群公告。
- Trusted access 用海外包源作默认网络级别（CT18）：不照抄。代理上线前默认仍是 none。T0 的 `sandbox.network_default` 保持 none（锁定面不动），EE10（W3，代理与 CA 就绪后）在 `features/egress.rs` 把生效默认翻成 `trusted`（china 预设），配测试 `default_network_is_china_trusted_after_ee10`；EE14 的离线私有化配置模板显式写回 `none`（客户内网没有镜像源时）；full 档由 owner 自己开，开时提示数据出境风险。
- Anthropic 服务端的 web search：不复制实现，替换成国内的博查或智谱 web search 接口；语义保持一致，都在服务端跑，不经代理。

---

## 3. 需要你拍板的事（H1，派第 1 波之前）

每条给了推荐项；不回就按推荐走。**第 1 波里没有任何一轨会卡在后续审批上**——所以依赖审批必须今天定。

| # | 要定的事 | 推荐 | 备选 / 代价 |
|---|---|---|---|
| D1 | 部署形态先后 | **单租户私有化 / 混合先行**（一客户一套，tenant_id 仍保留在数据里），SaaS 多租户以后再说 | SaaS 先行 → 要多租户隔离、等保测评、大概率要 AI 应用登记，全都压上关键路径 |
| D2 | 存储 | **SQLite + 迁移机制**（CC5 建） | Postgres + pgvector（立项方案原计划）留到 SaaS 阶段 |
| D3 | 记忆形态 | **按群 / 工作区的记忆文件，不上向量库**（Claude Tag 自己就是「记忆文件」，管理员可查可改） | 向量检索 → 多一个依赖、多一类失败面 |
| D4 | 管理台技术 | **Rust axum 服务端渲染，无前端构建链**（守「只用 Rust 和 Go」），DD12 做 | 批 axum 0.8 的同时要接受 `Cargo.lock` 新增传递依赖（`serde_urlencoded`、`serde_path_to_error` 等），DD12 回执逐个列；备选纯 hyper 手写（约 +1 天） |
| D5 | Git 托管 | **GitLab / 极狐优先**（项目访问令牌 = bot 用户，MR 标题前缀 `Draft: `），Gitee 先做桩 | GitHub / Codeup / Gitea 不在 P1 任何轨里（配置里选了报「未实现」）；GitHub 大陆访问不稳，要做就 W6 之后另开轨 |
| D6 | 托管网页 | **v1 = 应用建飞书云文档，按 openchat 共享给群**（按群授权、会后仍在、可更新、免 ICP） | 自有域名 HTML 页要 ICP 备案（W6 的 HH2） |
| D7 | 联网搜索 | **博查默认，智谱备选，百度千帆兜底**；离线私有化默认关 | Kimi 的 `$web_search` 2026-10-20 下线，不选 |
| D8 | 钉钉身份 | **只用机器人身份** | DWS 代理用户（beta、要管理员开、占席位）放 W6 的 HH3 |
| D9 | 模型 | **主力 `qwen3.7-plus`（百炼工作区域名）；兜底 `deepseek-v4-pro` / `deepseek-flash`；快模型 `deepseek-flash`；只用大陆端点**（海外端点默认拒绝，T0 加 `allow_overseas_endpoint` 默认 false） | 私有化时换成客户 vLLM / SGLang（或昇腾 MindIE）上的 Qwen3.8-27B / DeepSeek-V4-Flash 级别：T0 的 `ModelVendor` 加 `selfhost`，显式 vendor 永远优先于 CC6 的模型名推断，DD7 给 selfhost 单独一档（不发任何云厂商专有参数，思考开关走 `chat_template_kwargs`），FF9 的 live 矩阵加第 7 行「自托管 vLLM + Qwen3.8-27B」 |
| D10 | 平台数 | **v1 一个部署只接一个 IM 平台**（一 core 配一 edge） | 同一部署同时接多平台 → 再加一次契约（RPC 带平台标签） |
| D11 | 契约落地机制 | **T0 补丁只写锁定文件；你在分支 `contract/p1.0` 上打补丁、`make proto-gen`、用补丁前编好的二进制重锁；然后云端 T0c 让整个工作区编得过**（锚点不会漂，不依赖某个云会话活好几天） | 备选：一个自带全部伴随改动的大补丁 + W1 合并后「冻结 / 重锚」——W1 会改同一批文件，锚点几乎必然对不上 |
| D12 | 依赖审批（今天） | **批**：沙箱镜像加 apt `git curl jq unzip` 与 Python `pypdf`（CC12）；axum 0.8 带明确 feature 列表，外加测试用 dev-dependency `tower`（features = ["util"]）与 `http-body-util`（DD12） | 不批 pypdf → PDF 不打隐式 AIGC 标识；不批 axum → 选 hyper 手写；不批那两个 dev 依赖 → DD12 的路由测试改用现有 reqwest 打 127.0.0.1 |
| D13 | 并发派发默认值 | CC2 先建成「默认 1」（与今天逐字节一致），**T0c 翻成 4** | 保持 1 到更后面 |
| D14 | AIGC 显式标识 | **W3 的 EE12 起默认开**，文案「内容由 AI 生成」放在**同一条**回复 / 卡片里（p0 评测钉死了回帖条数）；云文档页顶部横幅 | 措辞你来定 |
| D15 | 卡片按钮 | **只有 H8 点击实测通过才默认开**；不通过就全走 `!stop` / `!approve #A..` / `!reject #A..` 文本命令 | — |
| D16 | 备案号展示 | **每个模型服务单独配 `filing_no`**，`!about` 与管理台「模型与备案」面板逐个列出 | Aite 自己要不要登记取决于是否面向公众：单租户私有化大概率不在范围内，公开注册的 SaaS 大概率要 |
| D17 | 审批跨重启 | **v1 不跨重启**：进程重启时把待审批任务判失败并通知「服务重启，审批已失效，请重新发起」 | 要跨重启得做任务恢复机制（新范围） |
| D18 | 私有化离线包时间 | **W3（EE14）**，因为私有化先行 | W4 |
| D19 | 模型 key 放哪 | **只在本机**（live 测试你本机跑） | 单独建云端环境 `aite-live`——环境变量对所有使用者可见 |
| D20 | 派单与启动 | 派单放 `review/paste-<轨号>.md`，`claude --cloud "读 review/paste-<轨号>.md 并照做"` 启动 | `claude-fleet` 目前不管云端会话；要不要给它加 `--cloud` 模式，你定 |
| D21 | 回执位置 | **每轨一个文件 `review/p1/ledger/<轨号>.md`**（云端 PR 没法按老办法拼接共享台账） | 老台账 `review/review-findings-2026-09-12-vmerge.md` 保持只读追加 |
| D22 | 解冻清单 | 签 §9 列出的解冻项 | — |
| D23 | W6 做哪些 | 等 W5 验收后按设计伙伴需求定：HH1 个人连接器 / HH2 自有域名托管页 / HH3 钉钉 DWS | — |
| D24 | 月度限额预设（人民币） | **¥3,500 / ¥7,000 / ¥17,500 / ¥35,000 / 不限 / 自定义（≤ ¥7,000,000）**（Claude Tag 的美元档按 1:7 折算；EE3 / EE9 照此实现） | 改数值就同步改 EE3 / EE9 卡片与 `rmb_presets_exact` 的期望 |
| D25 | 外部群 / 面向公众 | **v1 外部群一律 restrict**（回固定提示、不执行）；切到 channel_only / allow、开放 SaaS 注册、公网托管页之前，必须先确认登记范围并补内容安全过滤（《暂行办法》第十四条；P1 没有轨做这个过滤） | 真要做公众面 → 单独立项 |

---

## 4. 云端执行方式（Claude Code cloud）

### 4.1 环境（H3，一次性）

在 claude.ai/code 给 `TomwaltW/aite` 建一个环境（下称 `aite`），**并把它设为默认环境**（`claude --cloud` 不带环境参数；在网页建会话时手动选 `aite`）：

- **网络**：访问级别选 **Custom**，勾上「同时包含默认 Trusted 列表」，再加自定义域名：`deb.debian.org`、`security.debian.org`、
  `auth.docker.io`、`production.cloudflare.docker.com`（沙箱镜像 `python:3.11-slim` + apt 要用；crates.io / rustup /
  proxy.golang.org / GitHub release / PyPI / Docker Hub 本来就在 Trusted 里）。
- **环境变量**：只放 `GOTOOLCHAIN=auto`。**不放任何密钥**——环境变量对所有使用这个环境的人可见；模型 / 平台 key 一律留在本机。
- **GitHub**：装好 Claude GitHub App（云会话开 draft PR 要它）。
- **Setup script**（缓存约 7 天；改了脚本或网络设置会重建缓存）。CC1 会把它原样提交成 `scripts/cloud-setup.sh`，
  之后你要改就改仓库里那份再贴回来：

```bash
#!/usr/bin/env bash
# Aite 云端环境安装脚本 —— 贴进 claude.ai/code 的环境设置（Setup script）。结果缓存约 7 天。
# 必需的三件（protoc / Rust / Go）失败就退非 0；可选的（codegen 插件、仓库预热、沙箱镜像）失败只告警，
# 免得一个镜像站抖一下就把整个环境的缓存搞坏。
# 官方约束（code.claude.com/docs/en/cloud-environments「Script requirements」「GitHub proxy」）：非 0 退出 = 会话起不来；
# 总时长压在约 5 分钟内环境缓存才建得成；setup 阶段下载「没附加到会话的仓库」的 GitHub release 会 403。
set -uo pipefail
SUDO=""; [ "$(id -u)" = 0 ] || SUDO="sudo"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
export GOTOOLCHAIN=auto
export PATH="${HOME:-/root}/.cargo/bin:/usr/local/go/bin:/usr/local/bin:$PATH"   # setup 阶段未必继承会话的 PATH

# ① protoc v31.1（与 CI 的 arduino/setup-protoc "31.x"、docker/core/Dockerfile 同源；core/crates/proto/build.rs 每次重编都要它）
#    先走 GitHub release；setup 阶段它可能 403（protocolbuffers/protobuf 没附加到会话），就退到 Maven Central 的同版本二进制
#    （protoc 4.31.1 = libprotoc 31.1，按官方 .sha1 校验）+ raw.githubusercontent.com 上 v31.1 的 WKT（events.proto 要 struct / timestamp）。
if ! protoc --version 2>/dev/null | grep -q 'libprotoc 31\.'; then
  command -v unzip >/dev/null || { $SUDO apt-get update -qq && $SUDO apt-get install -y -qq unzip; } || exit 1
  if curl -fsSL -o /tmp/protoc.zip \
      https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip; then
    $SUDO unzip -o -q /tmp/protoc.zip -d /usr/local bin/protoc 'include/*' || exit 1
  else
    echo "WARN: GitHub release 没下来，改走 Maven Central + raw.githubusercontent.com"
    curl -fsSL -o /tmp/protoc.bin \
      https://repo1.maven.org/maven2/com/google/protobuf/protoc/4.31.1/protoc-4.31.1-linux-x86_64.exe || exit 1
    echo "b419c80e305bc1ee74d2a303e0a3e90d2549b203  /tmp/protoc.bin" | sha1sum -c --quiet || exit 1
    $SUDO install -m 755 /tmp/protoc.bin /usr/local/bin/protoc || exit 1
    $SUDO mkdir -p /usr/local/include/google/protobuf/compiler || exit 1
    for f in any api descriptor duration empty field_mask source_context struct timestamp type wrappers compiler/plugin; do
      $SUDO curl -fsSL -o "/usr/local/include/google/protobuf/$f.proto" \
        "https://raw.githubusercontent.com/protocolbuffers/protobuf/v31.1/src/google/protobuf/$f.proto" || exit 1
    done
  fi
  $SUDO chmod 755 /usr/local/bin/protoc
fi
hash -r
protoc --version | grep -q 'libprotoc 31\.' || { echo "protoc 不是 31.x（PATH 上先命中的是别的 protoc）"; exit 1; }

# ② Rust 1.98.1 + clippy + rustfmt（core/rust-toolchain.toml 钉的版本）
if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none || exit 1
  . "$HOME/.cargo/env"
fi
rustup toolchain install 1.98.1 --profile minimal --component clippy,rustfmt || exit 1

# ③ Go ≥ 1.27：go.mod 的 go 指令 + GOTOOLCHAIN=auto 会自动拉 1.27.x（要求镜像自带的 go ≥ 1.21）
go version || exit 1
# T0 的 --codegen 自测要这两个插件（版本与 edge/gen 文件头一致）；装不上只告警
timeout 120 go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.12 || echo "WARN: protoc-gen-go 没装上"
timeout 120 go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2 || echo "WARN: protoc-gen-go-grpc 没装上"
GB="$(go env GOBIN)"; GB="${GB:-$(go env GOPATH)/bin}"
for p in protoc-gen-go protoc-gen-go-grpc; do [ -x "$GB/$p" ] && $SUDO ln -sf "$GB/$p" "/usr/local/bin/$p"; done
protoc-gen-go --version && protoc-gen-go-grpc --version || echo "WARN: codegen 插件不在 PATH 上，T0 的 --codegen 自测会报 program not found"

# ④ 仓库内预热（setup 若不在仓库里跑就跳过）。注意：只有 setup 与会话共用同一份 checkout 时 core/target 才留得住；
#    否则这里只暖了 ~/.cargo/registry 与 Go 模块缓存。CC1 实测「会话里首编是否仍是冷编译」并写进 docs/p1/cloud-runbook.md。
#    这里不跑 cargo test --no-run 预编译：冷编 10–15 分钟，远超 setup 约 5 分钟的上限（官方说会话从全新 clone 起跑，target 本来也未必留得住）。
if [ -d "$ROOT/core" ] && [ -d "$ROOT/edge" ]; then
  ( cd "$ROOT/edge" && timeout 120 go version && timeout 120 go mod download ) || echo "WARN: go mod download 没过"
  ( cd "$ROOT/core" && timeout 120 cargo fetch ) || echo "WARN: cargo fetch 没过"
  # ⑤ 可选：沙箱镜像（跑 -tags docker 那组才要）
  if docker info >/dev/null 2>&1; then
    timeout 150 docker build -q -t aite-sandbox:p0 "$ROOT/docker/sandbox" \
      || echo "WARN: 沙箱镜像没建成（多半是 deb.debian.org / pypi 不通），会话里要跑 docker 那组时再建"
  else
    echo "WARN: 此刻 docker daemon 不在，沙箱镜像留到会话里建"
  fi
else
  echo "WARN: 不在仓库目录里（ROOT=$ROOT），跳过预热"
fi
echo "cloud-setup 完成"
```

> 这份脚本里的 `$(…)` 与多行引号**只在环境安装阶段跑**，不经过守卫；云会话里别把它当 Bash 命令整段敲（守卫会误拦），
> 要跑就 `bash scripts/cloud-setup.sh`。

### 4.2 一轨 = 一个云会话 = 一个 draft PR

- 云会话从 GitHub 的 `main` clone，自动起一条 `claude/<形容词>-<名字>-<hash>` 分支，**只能推这一条**。
  所以不再有 `task-cc3` 这种分支名；**PR 标题以轨号开头**（`CC3: …`）就是追踪手段。
- 每轨尽早开 **draft PR**：CI（`.github/workflows/ci.yml`）只在 PR、推 main、手动触发时跑，推分支本身不触发。
- 回执写 `review/p1/ledger/<轨号>.md`（每轨一个文件，避免并行 PR 在同一个台账上冲突），PR 描述里贴同样的摘要。
- 云会话会一直挂着（关掉浏览器也在）；你可以在 claude.ai/code 里看 diff、留行内评论，或 `claude --teleport <会话>` 拉到本机。
  但**不要让任何步骤依赖「某个云会话几天后还活着」**——空闲过期时间文档没写（约 24–48 小时）。

### 4.3 怎么启动（H4）

先只派 CC1（它验证环境、提交安装脚本、量云端基线）：

```bash
cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC1.md 并照做"
```

CC1 的开场自检贴出「`check.sh` 全部通过」且「Read `.claude/hooks/guard_bash.py` 被拦」之后，派其余 12 轨：

```bash
cd ~/Documents/Projects/Aite && for t in T0 CC2 CC3 CC4 CC5 CC6 CC7 CC8 CC9 CC10 CC11 CC12; do claude --cloud "读 review/paste-$t.md 并照做"; done
```

> **这 12 轨要在任何一个 W1 PR 合并之前全部启动**：开场自检第 1 步要求代码与 `98e4460` 一致（只允许 D0 与 P0-CLOSE），
> 晚启动的会话会 clone 到已合并别轨代码的 main，自检会（正确地）停下。真要晚派，先按 §4.7 刷新那份派单。
>
> 如果 `claude --cloud` 会进入交互、接管终端（循环会卡在第一轨），就每轨在 Ghostty 新标签里各敲一行
> `cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-<轨号>.md 并照做"`，或者在 claude.ai/code 网页里新建会话、粘同一句话。
> 用短提示词指向已提交的派单文件，而不是 `$(cat …)` 把几千字塞进参数——派单在 clone 里本来就有。

### 4.4 开场自检（每份派单第一件事，全部对上才开工）

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
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（把拦截原文贴进回执）。
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认。
   CC1 合并之后 check.sh 会多打一行 `go packages ok=N fail=M`，从那以后以这行为准。

### 4.5 受保护面的人工回路

```
云端轨写补丁脚本（review/…/…-patch.py：锚点恰好一次、全有或全无、--check、--root 自测、--self-test / --codegen）
      → 你本机：AITE_RELOCK=1 python3 <脚本> --check → 真跑 → make proto-gen（只在本机）→ 用补丁前的二进制重锁 → 推
      → 云端伴随轨（T0c）让工作区编得过、开 PR → 你合并
```

- `make proto-gen` **只在本机跑**：`edge/gen` 的文件头钉的是 `protoc v7.36.1`（= libprotoc 36.1，你本机 Homebrew 那个）、
  `protoc-gen-go v1.36.12`、`protoc-gen-go-grpc v1.6.2`；云端装的是 protoc 31.x，它生成的文件**永远不提交**。
- 云端命令里**永远不出现**：`AITE_RELOCK=1`、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、`.claude/settings.json`、
  `guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着这些路径的 `$(…)`。Go 模块文件有没有被改，由你审 PR 时看。

### 4.6 和本机多轨惯例的差异

| 本机惯例（`~/.claude/CLAUDE.md`） | 云端这一轮 |
|---|---|
| 每轨一个 worktree，钉具体 sha | 云会话自己 clone `main`；基线靠开场自检第 1 步的 diff 判定 |
| 末尾一行 `claude-fleet t7..t12` | `claude --cloud "读 review/paste-<轨号>.md 并照做"`，逐轨或循环 |
| 派单 `$(cat …)` 整份塞进参数 | 派单已提交进仓库，提示词只指个路径 |
| peer-sessions hook 提醒撞车 | 云端没有；撞车靠「每波可写面两两不交」保证 |
| 回执追加到一个台账 | `review/p1/ledger/<轨号>.md` 每轨一个文件 |
| 分支 `task-<轨号>` | `claude/*` 自动分支 + PR 标题带轨号 |

### 4.7 基线刷新规则

每到一个同步点（P0-CLOSE 之后、T0c 合并后的 B1、W2 合并后的 B2、W3 后的 B3（T0.1 若启用，B3 取 T01c 合并之后，
`CONTRACT_VERSION p1.1` 与 `contracts passed` 一起刷）、W4 后的 B4）：你（或一个短云会话）在 main 上重跑一次 `scripts/check.sh`，
把新的 sha 与各行条数刷进下一波派单，**`git add review/paste-<轨号>.md && git commit && git push origin main` 之后**再派
（云端只看得到已推送的派单；粘之前 grep 一遍旧 sha / 旧条数）。**不刷的话子会话开场自检第一条就对不上，会被当成回归报假警。**

---

## 5. 契约与人工落地

### 5.1 P0-CLOSE（W1 期间你本机做，约 1 小时）

- **AA4**：改 `edge.proto` 里 `EdgeStatusService` 的注释与 `edge/cmd/aite-edge/main.go` 的一句注释，然后 `make proto-gen`。
- **BB2**：`created_at` 进哈希链（contracts `evidence.rs` 的 `chain_hash_at` + evidence crate 写端 / 校验端 + 测试；
  同时改 `app/tests/evidence_on_disk.rs` 与 `cold_start_to_delivery.rs`）。第一步先 `mv data/evidence data/evidence-p0.1-legacy`（旧证据按新口径必红）。
- **一次重锁盖住 AA4 + BB2**（BB2 的文档串写明两者锚点不交、顺序无关）。重锁前 `lock --check` 会报 **MISMATCH 4 个文件**
  （`edge.proto` + BB2 的 3 个 contracts 文件；BB2 脚本自己打印的「3」只算了它自己）；此时跑全量测试，
  `cli_smoke contracts_lock_check_is_ok_on_a_clean_tree` **按构造就是红的**，所以重锁之后才跑 check.sh。
- 重锁后期望：`cargo passed=901 failed=0`（897 + BB2 的 2 条契约测试 + 2 条 evidence 测试；别的差异按测试名解释）、
  `contracts passed=27 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包 ok。`CONTRACT_VERSION` 仍是 `p0.2`。
- **BB4**：守卫补丁（`cd` 等价写法的漏拦），不用重锁；`cd core && cargo test -p aite --test guard` → 21 passed。
- **W1 没有任何一轨碰 P0-CLOSE 的文件**——就是 §4.4 第 1 步列的那 14 个路径（`edge.proto`、`edge/cmd/aite-edge/main.go`、
  `edge/gen/aitepb/edge_grpc.pb.go`、contracts 的 `evidence.rs` / `lib.rs` / `tests/evidence_vectors.rs`、`core/crates/evidence/**`、
  `app/tests/evidence_on_disk.rs`、`app/tests/cold_start_to_delivery.rs`、`.contracts.lock`、`.claude/**`、`app/tests/guard.rs`），
  所以你可以在 W1 跑着的时候做。

### 5.2 T0：契约 p1.0（W1 的 T0 轨起草，你在 W1 合并后落地）

**补丁的文件范围（穷举）**：`proto/aite/v1/*.proto`、`core/crates/contracts/**`（含新文件 `src/domain.rs`，锁面变成 26 个文件）、
`core/crates/proto/src/convert.rs` 与 `tests/convert.rs`（守卫按 `proto/` 前缀也拦它）、`edge/internal/server/server.go` 里
`ContractVersion` 那一行、`config/aite.example.yaml`（contracts 的 `tests/config.rs` 要求样例 == 默认值）。**别的一个都不碰**——
所以 W1 怎么改都不会让它的锚点漂。

**内容要点**（完整设计在 T0 产出的 `docs/p1/contract-p1.md`，你审过再打）：

- **版本**：`CONTRACT_VERSION` `p0.2 → p1.0`（core 与 edge 两边）。
- **平台**：`PlatformChoice += dingtalk, wecom`；`DingtalkConfig` / `WecomConfig`；`FeishuConfig += api_base, card_buttons, service_user_token_env`；
  能力位新增 14 个（proto 字段 10–23：`supports_reactions_in / recall_event / edit_event / pins / message_search / docs / dm / quote_id / requires_visible_anchor`、
  `stream_max_sec`、`card_action_deadline_ms`、`max_card_bytes`、`max_text_chars`、`max_upload_bytes`），新增 `dingtalk_v1()` / `wecom_v1()` 工厂，
  `feishu_p0()` 的旧值一个不改。
- **事件 / 出站**：`EventKind += Reaction, External`；`CardActionKind += Approve, Reject, Submit`；`NormalizedEvent += quote, reaction, external, sender_external`；
  `ChecklistCard += links, approval`；`OutboundText += mentions, dedupe_key`；`HistoryMessage += updated_at, deleted, reply_to`；
  新消息 `UserInfo / ChatInfo / SearchQuery / SearchHit / DocWrite(aigc_banner) / DocRef / OAuth*`；
  PlatformPort / edge.proto 除 `write_doc` / `WriteDoc` 外再加 **`delete_doc` / `DeleteDoc`**（删除应用建的云文档，或至少撤销 openchat 共享——
  FF6 断开 / PIPL 清除时要用，否则群里还能看到 AI 生成的页面）。
- **模型**：`Message += provider_extra`（原样携带 `reasoning_content` / `encrypted_content` / `reasoning_details`）；`Usage += cache_write_tokens`；
  `ModelConfig += name, display_name, filing_no, vendor, thinking, timeout_sec, stream, price_cached_in_per_mtok, price_cache_write_per_mtok, offpeak_price_multiplier`；
  `ModelsConfig { fallback, fast, catalog }`；`ModelError += RateLimited, Auth, BadRequest`；
  `ModelVendor` 取值 `{generic, deepseek, qwen, glm, kimi, doubao, minimax, selfhost}`（`selfhost` = 客户自托管的 vLLM / SGLang / MindIE，
  不发任何云厂商专有参数）；`ModelConfig += allow_overseas_endpoint`（默认 false：配到海外端点直接报配置错）。
- **会话**：`Turn += message_id, provider_extra`；`Session += meta`；新常量 `OPEN_TASK_STATUSES`（含 `awaiting_approval`）。
  **`ACTIVE_TASK_STATUSES` 不动**——store 在 `lib.rs:419-421` 与 `:481-483` 按下标绑定它，孤儿恢复绝不能把待审批任务当孤儿。`Task` 不动。
- **沙箱**：`SandboxNetwork += Trusted, Custom, Full`；`ExecLanguage += Bash`；`EgressPolicy`、`CredentialBinding`（`secret_ref` 只是名字，取值永不过 gRPC）。
- **Port**：`PlatformPort` / `SessionStore` / `SandboxPort` / `ControlPlane` 的新方法**全部带默认实现**（返回 unimplemented），
  所以旧实现不用改就能编；新 trait `ScopeStore / MemoryStore / RoutineStore / UsageLedger / AuditLog`；`Services` 句柄包。
- **配置段**（全部 `deny_unknown_fields` + 默认值）：`sandbox`（网络默认 none、linger 600s）、`worker`（`max_parallel_tasks 4`、`context_max_tokens 96000`、
  `approval_timeout_sec 1800`）、`search`、`git`、`pages`、`memory`、`routines`（默认 `Asia/Shanghai` +480）、`ambient`（自动回复默认关、每群每日上限、
  100 条停读、连续 bot 轮次上限 3）、`access`、`budget`（人民币）、`compliance`（`aigc_label true`、`label_text "内容由 AI 生成"`、`audit_retention_days 190`、`registration_scope_confirmed false`、
  `content_safety_filter false`——后两个给 D25 用：EE8 只有两者都为 true 才允许外部群离开 restrict）、`admin`。
- **明确不做**：不加新 `EvidenceKind`（改用 `checklist_op` / `event_received` 的新 op 值，op 登记表写进设计文档）；
  `all_model_tools()` 仍是 10 个（新工具走 CC4 的网关注册表）。

**你本机落地（H11，一行，约 45 分钟）**：见 §8。落地后分支上 `lock --check` → `OK 26 files`、contracts 与 proto crate 测试全绿、`go build` 过；
**工作区其余部分按预期编不过，直到 T0c 合并**。

### 5.3 T0c：伴随轨（W2a，单独跑）

从 `origin/contract/p1.0` 起，只做机械活：各 crate 的结构体字面量与穷举 match；**5 处写死 `p0.2` 的测试钉**
（`edge/cmd/aite-edge/main_test.go:202`、`core/crates/evidence/tests/manifest.rs:65`、`core/crates/evals/tests/evals_runner.rs:111`、
`core/crates/evals/tests/protocol_probe.rs:523`、`core/crates/app/tests/cli_smoke.rs:481`）；`EventKind::Reaction` 进 R4 只计数（排在 R5/R6 之前，
否则进行中话题里的一个表情会被当成 steer 塞进任务）；`Services` 贯通 control / worker / gateway / app；Go 可选接口新文件；
**并发派发默认翻成 4**（唯一的行为变化，撞上真竞态就停下报告）。PR 里带着你的契约提交，合并后删 `contract/p1.0`。
生成 T0c 派单时，把 W1 各轨交给 T0c 的接线一并写进去（已知的：CC3 的 `with_context_max_tokens` 接 `worker.context_max_tokens`；
CC4 的 `features::start_all` 接进 `run.rs` 起飞、`wiring.rs` 的 docker 档挂工具注册表与 `gateway_options`；CC7 的 `worker_options.aigc_label`
经 `plane_factory` 接进 `WorkerDeps` 并去掉 CC7 那道「无消费方」闸门与它的测试；CC2 的 `with_max_parallel`），再加上 H10 清单里的其它「转给 T0c」条目。

### 5.4 T0.1 勘误批（备用，默认不启用）

启用条件：W2/W3 某份回执用**失败的测试或编译错误**证明某个 p1.0 形状挡住了某条 CT，且开放通道（`Session.meta`、`ExternalEvent.payload`、
`AuditEvent.detail`、`provider_extra`、trait 默认方法、网关注册表）都绕不过去。多个缺口攒成一批，只在 W3→W4 边界落一次（重锁 #3）。

---

## 6. 波次与轨道

**每一波都遵守的规则**（每份派单的「规则」一节都会写进去）：

- 可写面两两不交；每一波里 `control/**`、`worker/**`、`gateway/**`、`app/**` 的每个文件只有一个主人。
- **B8 不变量**：p0 场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条（所以 AIGC 标识必须在同一条里）；
  顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；`!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）；
  `evals/p0/*.yaml` 不许改。哪一轨把 B8 弄红了就停下报告。
- 改 P0 默认行为的（助手回复入 transcript、并发派发、按线程沙箱、AIGC 标识）要么**默认关着交付**，要么**同一轨接管钉住旧行为的 app 测试**——
  下表每一项都写了谁接管。
- 新的集成测试放 `core/crates/app/tests/<轨号>_*.rs`；新评测场景只放 `evals/p1/<轨号>_*.yaml`。
- 每个行为改动配回归测试 + 变异验证（还原时别用保留旧 mtime 的拷贝）；`cargo passed` 增量按测试名逐条解释。
- 需要锁定面 / R0 文件 / 新依赖 → 停下、写进回执，不动手。Docker 测试必须封闭（只连本地测试服务器，不连公网）。

### 6.1 W1：不碰契约的地基（今天可派；派单已写好）

基线 B0 = `98e4460` + 你的 D0 提交。先 CC1，验过再派其余。

| 轨 | 标题 | 做什么（摘要） | 可写面（摘要） | 规模 |
|---|---|---|---|---|
| **T0** | 契约 p1.0 补丁起草 | 写 `review/t0/p1-contract-patch.py`（只写 §5.2 那几类文件；`--self-test` 在 `git clone --local` 的临时副本上先套 AA4/BB2/BB4 再套 T0，跑 contracts / proto 测试、codegen、go build，用补丁前的二进制报 MISMATCH 清单），外加 `review/t0/APPLY.md`（你的人跑命令链，每步带期望输出）与设计文档 `docs/p1/contract-p1.md` | `review/t0/**`、`docs/p1/contract-p1.md` | XL |
| **CC1** | 云端基建 · R0 主人 | 提交 `scripts/cloud-setup.sh`；check.sh 加 `go packages ok=N fail=M` 行、`evals/p1` 的 B9 门、`--docker`；Makefile / CI 加 docker 测试 job；docker/core、docker/edge 加国内镜像构建参数；建 5 个空骨架 crate（admin / search / githost / routines / memory，依赖预先声明、只用现有 workspace 依赖；memory / routines / search / githost 连 `aite-gateway` 一起预声明——CC4 的 `GatewayTool` trait 在它里面——并把 `core/Cargo.toml` 里「各轨只许依赖 contracts / proto / testing」那句改成允许这四个 crate 依赖 `aite-gateway`）；Dockerfile 加 `ARG BASE_REGISTRY=docker.io`，所有 `FROM` 走它（大陆构建期也不直连 Docker Hub）；量云端基线写进 `docs/p1/cloud-runbook.md` 与 `CLAUDE.md` | `scripts/**`、`Makefile`、`.github/**`、`docker/core|edge/**`、`docker-compose.yml`、`core/Cargo.toml`、`Cargo.lock`、新骨架 crate、`CLAUDE.md` | M |
| **CC2** | 控制面拆分 + 并发 + 命令 | 零行为变化地拆 `plane.rs`（1710 行）并预埋后续各波要的桩文件；按会话串行、跨会话并行 N（**默认 1**）；入口错误传播（平台重推）；`!restart` 回灌话题历史；卡死任务下条消息替换；全角 `！`、U+3000、中文别名、`!help`；R6 按 `supports_thread` 路由；R6 追问补 ack；`!stop` 记发起人 | `control/**`、`app/src/run.rs` | L |
| **CC3** | 执行面拆分 + 助手回复 | 零行为变化地拆 `agent.rs`（1107 行）并预埋桩；**助手回复写入 transcript（默认开，接管 `reconnect_replay.rs` / `sqlite_cross_process.rs`）**；发言人署名；重试分类（配置错 / 4xx 不重试，429 按 retry-after）；每个 tool_call 都有回复；按 token 预算裁上下文；话题内用话题历史窗口；证据写入侧脱敏；工具结果按外部数据包裹 | `worker/src/**`、`worker/tests/**`、那两个 app 测试（`prompts/platform.md` 不在内，W2 归 DD4） | L |
| **CC4** | 网关注册表 + 策略钩子 + 功能接缝 | `GatewayTool` 注册表（外部 crate 挂工具）；`policy.rs` 前置钩子；`SandboxKey` 抽象；`app/src/features/` 两段式接线（各功能以后只改自己那一个文件） | `gateway/**`、`app/src/{app,wiring,lib}.rs`、`app/src/features/**` | M |
| **CC5** | 存储迁移 | `schema_version` + 有序事务迁移；`seen_events` 可剪枝；tasks 加 chat / cost 查询列；消息索引表；Answering 任务纳入孤儿恢复（**待审批不算孤儿**） | `store/**`、`app/tests/{startup,crash}_recovery.rs` | M |
| **CC6** | 国产模型加固 | 按模型名前缀 / 域名识别厂商；Kimi 不传 temperature、Qwen 开 `parallel_tool_calls`、GLM 只用 `tool_choice:auto`、MiniMax 剥 `<think>`；任务内按 tool_call_id 回挂思考字段（T0 前的过渡）；三种缓存命中字段；超时 600s；六家录制夹具；把今天的探测结果写成测试 | `models/**` | M |
| **CC7** | P1 评测底座 | 新建 `evals/p1/`（先 3 个场景）；新检查项（证据 op、模型看到了什么、回帖正则、成本上限、提供了哪些工具）；场景级平台能力覆盖；探针按实际提供的工具判未知工具 | `evals/**`（crate）、`testing/**`、`evals/p1/**` | M |
| **CC8** | 飞书适配器整理 | 按关注点拆文件（零行为变化）；订阅撤回 / 入群 / 成员事件；卡片按钮只在 `AITE_FEISHU_CARD_BUTTONS=1` 时渲染并记录帧类型（给 H8 用）；话题历史走 `container_id_type=thread`；发言人姓名；每群 5 QPS | `edge/internal/feishu/**` | L |
| **CC9** | 钉钉 Stream 适配器（不接线） | 手写 Stream 客户端（已钉的 gorilla/websocket，不动 go.mod）；归一化 + 未文档化的引用字段；`#A` 锚点；AI 卡片流式 ≤1KB；2 秒回调 | `edge/internal/dingtalk/**` | L |
| **CC10** | 企微智能机器人适配器（不接线） | 手写 aibot WS 客户端；主备；流式 9 分钟自动收尾；模板卡片 5 秒；分片上传；AES-256-CBC 解密；引用只有内容 → `#A` 锚点；`feedback_event` 解析 | `edge/internal/wecom/**` | L |
| **CC11** | 出网代理包（不接线） | 纯标准库 CONNECT 代理：默认拒绝、按沙箱令牌识别、china-trusted 预设（IM / API 主机**永不**进匿名预设）、每小时 JSONL 审计 | `edge/internal/egress/**` | M |
| **CC12** | 沙箱镜像中国版 + 隐式标识器 | apt / pip 镜像构建参数与 `BASE_REGISTRY`（`FROM python:3.11-slim` 也走它）；加 git / curl / jq / unzip；运行时镜像配置（none 下不起作用）；`/opt/aite/aite_label.py` 写 GB 45438 隐式元数据（PNG / JPEG / DOCX / XLSX / PPTX / Markdown / PDF） | `docker/sandbox/**`、`edge/internal/sandbox/docker_test.go` | M |

### 6.2 W2a：T0c（单独跑，见 §5.3）

### 6.3 W2：契约上的地基（基线 B1 = T0c 合并后的 main）

| 轨 | 标题 | 做什么（摘要） | 依赖 | 规模 |
|---|---|---|---|---|
| DD1 | 存储 P1 | 实现 `SessionStore` 全部新方法 + 五个领域存储（Scope 继承、记忆墓碑、例程到期、用量按群 / 人 / 模型汇总、审计只追加且 190 天内不许删） | T0c、CC5 | XL |
| DD2 | edge-client 新 RPC + 全部新假件 | 每个新 RPC 的 Rust 客户端；五个新存储的 Fake；FakePlatform 带钉钉 / 企微能力档；评测夹具（置顶、搜索命中、用户、群、文档） | T0c | L |
| DD3 | 控制面 P1 路由 | **锚点解析**（本文 `#A` → 引用里的 `#A` → 消息索引命中 → 同发起人 30 分钟内恰好一个未结任务 → 否则新开并说明）；无话题平台所有回复带可见 `#A..`；p2p → DM 会话；编辑前后对照 / 删根语义；`submit_internal`（例程 / 外部事件入口）；审批信箱 | T0c、CC2 | L |
| DD4 | 执行循环 P1 | 思考字段端到端回放（换模型时丢弃）；按类型重试；`post_update` 本地工具（「进展如何」「卡住了」）；CT09 话题开始时锁定配置快照；附件 5 个上限 | T0c、CC3、CC6 | L |
| DD5 | 输出 P1 | AIGC 显式标识机制（**默认关**）+ 隐式标识器调用；卡片链接（Configure、MR、页面）；页脚写模型名；忙话题（卡片下方超过 20 条）把清单卡重贴到底部，最多每 15 分钟一次 | T0c、CC3、CC12 | M |
| DD6 | 网关策略 + 按线程沙箱 | 按 bundle 过滤目录；出网策略 → `acquire_scoped`；**按线程沙箱 + 空闲回收（!stop 仍立即释放）**，接管 `cold_start_to_delivery.rs:661-665` 与 `graceful_shutdown.rs:279`；`run_shell`；`describe_access`；跨群读取要求成员身份 | T0c、CC4 | L |
| DD7 | 模型 P1 | 六家厂商档案（思考参数形状、回传规则）；SSE 流式；分档计价（缓存命中 / 写入、DeepSeek 峰谷）；`ModelRouter`（主力 / 兜底 / 快模型 / 按名选）；`selfhost` 档（自托管端点，不发云厂商专有参数）；海外端点默认拒绝（`allow_overseas_endpoint` 显式打开才降为告警） | T0c、CC6 | L |
| DD8 | edge 接线（Go 侧 R0 主人） | `main.go` 平台工厂（feishu / dingtalk / wecom / fake）；起出网代理 + 沙箱内网；HTTP 入口监听（给 webhook；`listener.go` 预埋路由登记表 `Register(path, func(Deps) http.Handler)`，`main.go` 把 ingress sink 与配置作为 Deps 传进去，EE11 只加 `gitlab.go`、不改 listener.go / main.go）；Go 配置镜像；docker 测试封闭 | T0c、CC9–CC11 | L |
| DD9 | 飞书读 P1 | 置顶、消息搜索（服务用户 token）、用户 / 群 / 成员、表情 → Reaction（只认 Aite 自己的消息）、引用内容、历史编辑字段、OAuth、访客 / 外部判定 | T0c、CC8 | L |
| DD10 | 飞书写 P1 | 云文档托管页（openchat 共享 + AIGC 横幅）、编辑自己的消息、私聊、卡片链接 / 审批按钮（按 H8 结果）、@ 人、发送去重 | T0c、CC8 | L |
| DD11 | 钉钉 + 企微补全 | 能力值对齐契约；审批回调（钉 2 秒 / 企 5 秒）；企微 👎 反馈 → Reaction；企微 3 条在途 / 24 小时 / 30 条每分钟；钉钉 @ 人与 OAuth | T0c、CC9、CC10 | L |
| DD12 | 管理台 v0 | axum 服务端渲染（`aite admin serve`：在 `app/src/main.rs` 的 clap 顶层加一个转发分支）；平台 OAuth + 一次性引导令牌登录；等保二级基础（登录失败锁定、会话超时、CSRF、转义）；Scope / Bundle / Skills 编辑；任务证据只读页；「模型与备案」面板 | T0c、CC1、CC4 | L |

W2 合并顺序：DD1 → DD2 → DD8 → DD3…DD7 → DD9…DD12。DD8 + DD9 + DD10 + DD11 合并后，你在三端真机各冒烟一次（@ 提问、引用 `#A` 追问、卡片更新、停止、👎）。

### 6.4 W3：功能纵切（基线 B2）

每个功能只拥有自己那一个 `app/src/features/<x>.rs`、自己的预埋桩文件、自己的骨架 crate；**谁都不改注册表、`Cargo.toml`、`Cargo.lock`**
（CC1 已预先声明依赖；要新依赖就停下报告）。

| 轨 | 标题 | 覆盖 | 依赖 |
|---|---|---|---|
| EE1 | 记忆（按群 / 工作区的记忆文件；`!memory`；公开群进工作区、私有群自存；钉 / 企一律按私有；只存工作事实、拒收身份证 / 手机号类） | CT21、NEW22 | DD1 DD2 DD4 DD6 |
| EE2 | 例程（定时 / 模型自己排跟进 / 频道观察 / 输出目标规则；`!routines`；Asia/Shanghai 无夏令时） | CT22、NEW20、NEW21 | DD1 DD3 |
| EE3 | 预算与用量（组织 / 群 / 任务三级人民币上限；80% 告警一次、100% 停；DM 记到个人；`!usage`；预设见 D24，`worker/src/budget.rs` 钉 `rmb_presets_exact`；EE9 在 `admin/src/pages/setup.rs` 自己放一份同样的常量，不引用 EE3 的代码——同波不许互相依赖） | CT26、NEW16 | DD1 DD4 DD7 |
| EE4 | 联网搜索（服务端，不经代理，引用出处）+ 工作区搜索（按请求者成员身份过滤；外部群默认不开） | CT08、CT11、CT19 | DD2 DD6 |
| EE5 | Git（GitLab / 极狐：服务端拉归档进 `/work/repo`、提交 API 建分支、`Draft: ` MR 并反链话题；令牌不进沙箱） | CT12、CT15、NEW24 | DD1 DD6 |
| EE6 | 托管页（`publish_page` → 飞书云文档，同一话题再调用就更新同一页；无文档能力的平台退成附件） | CT14、NEW05 | DD2 DD6 DD10 |
| EE7 | 审批（`request_approval` → AwaitingApproval；按钮或 `!approve` / `!reject`；访客 / 外部用户不能批；超时取消；重启判失败并通知） | CT13、CT29、NEW08 | DD3–DD6 DD10 |
| EE8 | 访问闸门（未启用提示——但未启用的群 / 私聊里 `!connect <码>`、`!help`、`!about` 必须先放行到命令路由，否则 `access.require_setup=true` 时永远配不上对，测试 `connect_passes_gate_before_enabled`；外部群模式（切出 restrict 要满足 D25）、DM 开关、群名单、成员白名单；bot 连环对话熔断；`!access` `!configure` `!about`（`!help` CC2 已建，EE8 只给 help 补新命令条目）；skills 块） | CT01 CT09 CT16 CT28 CT29、NEW01 NEW06 NEW08 NEW10 NEW15 NEW19 | DD1 DD3 DD4 |
| EE9 | 配对与上线（一次性 15 分钟配对码 `@Aite !connect`；首批工具 → Git → 服务账号 → 限额 → 上线）；首次入群自我介绍；`!fork`；👎 / `!mute` / `!unmute`；`!feedback` | CT24、NEW01 NEW02 NEW03 NEW15 NEW16 NEW18 NEW22 | DD1 DD3 DD9 DD11 DD12 |
| EE10 | 出网 v2：凭证注入（只对带凭证的主机做 TLS 终结；Aite CA；IM / API 主机只能这样到达）；Full 档要显式开；**代理与 CA 就绪后在 `features/egress.rs` 把生效默认网络档翻成 `trusted`（china 预设）**，测试 `default_network_is_china_trusted_after_ee10` | CT15 CT17 CT18 CT25、NEW11 NEW12 | DD8 CC12 |
| EE11 | 外部触发（GitLab webhook；飞书文档编辑 / 评论 / 审批事件 → 例程；自己 MR 上的评论回到原话题） | CT22、CT12 | DD1 DD3 DD8 |
| EE12 | 审计与合规（新 op 渲染；`aite audit export` 按小时导出（`app/src/main.rs` 加 `audit` 转发分支）；审计 / 网络日志 ≥190 天、内容按租户留存；`!evidence #A..`；**AIGC 标识默认翻开**并接管 `cold_start_to_delivery.rs`） | CT14 CT27、NEW13 NEW23 | DD1 DD5 DD6 CC11 CC12 |
| EE13 | 飞书上下文深度（拉取比对编辑 → 前后对照；置顶块；引用块；话题中途 @ 的窗口） | CT06、CT07 | DD2 DD3 DD4 DD9 |
| EE14 | 私有化离线包 v1（amd64 + arm64；`BASE_REGISTRY` 取值写进 deploy.md；离线配置模板把网络档写回 `none`；`make bundle`；install / upgrade / uninstall 脚本；sha256 清单；Kylin V10 / UOS V20 说明；运行时绝不拉 Docker Hub） | CT10 CT18 CT24 | DD8 CC1 |

### 6.5 W4：P2 对齐与加固（基线 B3；FF1 在 `im:message.group_msg` 获批前只 @ 降级交付）

| 轨 | 标题 | 覆盖 |
|---|---|---|
| FF1 | Ambient + 频道顶层会话（按群开关、默认关；简单问题顶层直答、复杂的开话题；1 小时 / 1 天 / 配置变更替换；100 条停读；快模型判定 + 每日上限；停滞话题 @ 人；**顶层 @ 仍恰好 1 个 Task 会话**） | CT03 CT04 CT23 |
| FF3 | 流式进度面（飞书 CardKit：10 分钟自关要重开、回调前先关流式；钉钉 AI 卡片 ≤1KB；企微 9 分钟收尾后发里程碑消息） | CT13 CT14 |
| FF4 | DM 与换模型（DM = 共享只读连接 + 个人记忆、用量计入组织余额并按人记明细（不设个人余额）；「用 X 模型」/ `!model` 按话题或群切换） | CT20 CT30 |
| FF5 | 起飞自检扩组 + `aite doctor`（`app/src/main.rs` 加 `doctor` 转发分支；搜索 / Git / 出网 / 管理台 / 合规 / 钉企凭证 / 大陆端点；宿主机容量） | CT24 CT18 CT10 |
| FF6 | PIPL 删除与断开工作区（删内容留审计墓碑；证据文件里的内容字段换成墓碑、`payload_hash` 保留，证据链删内容后仍能校验——可写面加 `core/crates/evidence/**`，W4 没别的轨碰它；托管页用 T0 的 `delete_doc` 逐个删除或撤销共享，测试 `purge_deletes_or_unshares_hosted_docs`）+ `docs/p1/compliance.md` | NEW23 CT27 |
| FF7 | 管理台 v1（用量明细、审计与网络事件导出、例程、记忆查改、网络事件检索） | CT21 CT22 CT26 CT27 |
| FF8 | 连接器包与 skills 库（飞书文档 / 多维表格、GitLab 真 clone / push、通用 HTTP；厂商 CLI 用假凭证 + 代理注入；MCP 视审批） | CT25 CT15 CT17 CT12 |
| FF9 | 多模型 live 矩阵（6 家 + 自托管 vLLM / Qwen3.8-27B 共 7 行；云端只跑脚本化与 dry-run，live 你本机跑） | CT30 CT29 |
| FF10 | P1 提示词与跨功能场景（含 prompt injection：历史 / 文档 / 搜索结果 / 引用里藏的指令都不执行） | CT29 CT11 CT04 |

（原 FF2 已并入 W3 的 EE13。）

### 6.6 W5：P1 验收（GG1 写剧本与演示分镜；你在飞书全量、钉钉 / 企微按降级矩阵各跑一遍）

### 6.7 W6：可选的延后对齐（W5 之后按设计伙伴需求定）

HH1 个人连接器（NEW09，飞书用户 OAuth + offline_access）· HH2 自有域名托管 HTML 页（OAuth + 群成员校验，内网或 ICP 备案）·
HH3 钉钉 DWS 代理用户（群历史 / 搜索 / 非 @，默认关）。

> 各轨英文原卡（目标、完整可写面、验收命令、依赖）在 `review/p1/tracks-2026-09-25.json`，生成后续波次派单时以它为准。

---

## 7. R0 文件归属（「约定只读」的文件每波只有一个主人）

| 文件 | 归属（按波次） |
|---|---|
| `scripts/check.sh`、`scripts/cloud-setup.sh`（新） | CC1；之后没人计划改 |
| `Makefile`、`.github/**` | CC1（W1）→ EE14（W3） |
| `docker-compose.yml` | CC1（W1：镜像构建参数）→ DD8（W2：沙箱内网）→ EE14（W3：打包 profile） |
| `docker/core/**`、`docker/edge/**` | CC1（W1）→ EE14（W3：多架构） |
| `docker/sandbox/**` | CC12（W1）→ EE10（W3：CA 信任）→ FF8（W4：厂商 CLI） |
| `core/Cargo.toml` | CC1（W1：骨架 crate）→ DD12（W2：axum）；别人不改 |
| `core/Cargo.lock` | CC1（W1）→ T0c（仅在伴随改动逼出时）→ DD12（W2）；W3/W4 不改 |
| `core/crates/app/Cargo.toml` | CC1（W1）→ T0c（W2a，仅在伴随改动逼出时）；别人不改 |
| `core/crates/proto/**`、`config/aite.example.yaml` | 只有 T0 补丁（T0.1 若启用），你来打 |
| `core/crates/evidence/**` | BB2（你，P0-CLOSE）→ EE12（W3）→ FF6（W4） |
| `edge/gen/**` | 只有你本机的 `make proto-gen`（AA4、T0、T0.1） |
| `edge/internal/server/**` | T0 补丁（`ContractVersion` 一行）→ T0c（新文件 `ports_p1.go` / `services_p1.go`）→ DD8 |
| `edge/internal/config/**`、`edge/internal/aiteerr/**` | DD8（W2） |
| `edge/internal/pin/**`、`edge/go.mod`、`edge/go.sum` | 没人（不加 Go 依赖；钉钉 / 企微客户端基于已钉的 gorilla/websocket 手写） |
| `edge/cmd/aite-edge/main.go` | AA4（你，只改注释）→ DD8（W2：平台工厂、出网、HTTP 入口）；`main_test.go`：T0c → DD8 |
| `core/crates/app/src/{app,wiring,lib}.rs`、`features/mod.rs` | CC4（W1）→ T0c（W2a）；之后每个功能只改自己的 `features/<x>.rs` |
| `core/crates/app/src/run.rs` | CC2（W1）→ T0c（W2a） |
| `core/crates/app/src/cli.rs`（只解析 `aite run` 的参数） | T0c（W2a，只改字面量）→ DD12（W2）→ FF5（W4）；`preflight.rs`：T0c（只改字面量）→ FF5 |
| `core/crates/app/src/main.rs`（clap 顶层子命令） | DD12（W2：`admin`）→ EE12（W3：`audit`）→ FF5（W4：`doctor`）；每轨只加一个转发分支 |
| `core/crates/app/tests/cold_start_to_delivery.rs` | BB2（你）→ T0c（只改字面量）→ DD6（W2）→ EE12（W3） |
| `core/crates/worker/prompts/platform.md` | DD4（W2）→ FF10（W4）（每次改都要记新的 live 基线） |
| `CLAUDE.md` | 你（D0）→ CC1（W1 云端基线）→ T0c（W2a 的 B1 行） |
| `.gitignore` | 你（D0）→ CC1（W1）；之后没人计划改 |
| `.claude/**`、`docs/dev-spec-*.md`、`evals/p0/*.yaml`、`.contracts.lock` | 只有你 / 永不（重锁只有 3 次：P0-CLOSE、T0、T0.1 若启用） |

---

## 8. 人工队列（按顺序；每条命令 `cd` 与命令写在同一行）

> 带 `AITE_RELOCK=1` 的命令（H6、H11）和 H2 的 `git rm` 都在**普通终端**里敲——别用 Claude 会话里的 `!` 前缀，
> 那会经过守卫 hook，重锁变量赋值与 dev-spec 文件名都会被拦。

- **H1（第 0 天，派单前，15 分钟）拍板** §3 的 D1–D23，尤其 D12 依赖审批。
- **H2（第 0 天）推 D0**（在普通终端里敲，不要经本机 Claude 会话——守卫会扫到 dev-spec 那个文件名）：

  ```bash
  cd ~/Documents/Projects/Aite && ls review/paste-T0.md review/paste-CC{1,2,3,4,5,6,7,8,9,10,11,12}.md >/dev/null && git rm -r -q --cached --ignore-unmatch .venv aite.egg-info dev-spec-2026-09-09.md && { grep -qxF '.venv/' .gitignore || printf '\n# Python 残留（AA4 合并时误入库，2026-09-25 移出索引）\n.venv/\n*.egg-info/\n' >> .gitignore; } && git add .gitignore CLAUDE.md review/plan-2026-09-25-claude-tag-parity.md review/p1 review/paste-T0.md review/paste-CC*.md review/backlog-2026-09-15.md review/paste-BB*.md && git commit -m "docs(p1): 对齐 Claude Tag 总计划 + 第 1 波派单 + 仓库卫生" && git push origin main
  cd ~/Documents/Projects/Aite && cmp dev-spec-2026-09-09.md docs/dev-spec-2026-09-09.md && rm dev-spec-2026-09-09.md
  cd ~/Documents/Projects/Aite && rm -f .claude/workflows/aite-w1-pastes.js && rmdir .claude/workflows 2>/dev/null; git status --short
  ```

  第三行删掉生成派单时 Workflow 工具顺手存进 `.claude/workflows/` 的脚本（不是有意创建的；它在守卫面里，Claude 会话删不了）。
  不删的话它会以 `??` 一直挂着，H6 / H11 里「`git status --short` 恰好 N 行」的判据就数不对了。最后那个 `git status --short` 应该是空的。

  第一行先 `ls` 校验 13 份派单都在，缺一份整条命令不动任何东西；`--ignore-unmatch` 与 `grep -qxF` 让它能安全重跑。
  第二行删掉仓库根那份与 `docs/` 逐字节相同的规格副本（否则它以 `??` 一直挂在 `git status` 里，哪次 `git add -A` 又会带回去）。
- **H3（第 0 天）配云端环境**，见 §4.1。
- **H4（第 0 天）派 W1**，见 §4.3：先 CC1，验过再其余 12 轨。守卫在云端没生效就全部停下，先修 hook。
- **H5（第 0 天，外部等待最长）飞书开发者后台**：
  - 回调订阅方式 = **长连接**，加 `card.action.trigger`；
  - 事件：`im.message.recalled_v1`、`im.message.reaction.created_v1` / `deleted_v1`、`im.chat.member.bot.added_v1`、`im.chat.member.user.added_v1` / `deleted_v1`；
  - 权限：`im:message.group_msg`（敏感，要管理员审批；决定 CT03 / 免 @ 引导 / 频道观察 / CT23）、`im:message.p2p_msg`、表情读、群信息读、置顶读、
    `contact:user.base:readonly`、`docx:document`、文档协作者（openchat 共享）、文档 / 知识库搜索；
  - 另建一个带 `offline_access` 与消息搜索权限的服务用户 OAuth（CT08）；发布应用版本。
- **H6（第 0–1 天，本机，约 1 小时，W1 跑着时做）P0-CLOSE**：

  ```bash
  cd ~/Documents/Projects/Aite && git checkout main && git pull --ff-only && protoc --version && protoc-gen-go --version && protoc-gen-go-grpc --version   # 期望 libprotoc 36.1 / v1.36.12 / 1.6.2，对不上就停（版本一变，make proto-gen 会改写全部生成文件头）
  cd ~/Documents/Projects/Aite && { [ -e data/evidence-p0.1-legacy ] || mv data/evidence data/evidence-p0.1-legacy; }
  cd ~/Documents/Projects/Aite && AITE_RELOCK=1 python3 review/aa4-proto-patch.py --check && AITE_RELOCK=1 python3 review/aa4-proto-patch.py && make proto-gen
  cd ~/Documents/Projects/Aite && AITE_RELOCK=1 python3 review/bb2-created-at-chain-patch.py --check && AITE_RELOCK=1 python3 review/bb2-created-at-chain-patch.py && git status --short   # 期望恰好 11 行 ` M`（AA4 3 个含 edge_grpc.pb.go + BB2 8 个）
  cd ~/Documents/Projects/Aite/core && cargo test -p aite-contracts -p aite-evidence && cargo build && ./target/debug/aite contracts lock --check   # 前半全绿；后半期望 MISMATCH 4 file(s)（edge.proto + BB2 的 3 个 contracts 文件；BB2 脚本自己打印「3」只算了它自己）
  cd ~/Documents/Projects/Aite && AITE_RELOCK=1 core/target/debug/aite contracts lock --write
  cd ~/Documents/Projects/Aite && AITE_RELOCK=1 python3 review/bb4-guard-patch.py --check && AITE_RELOCK=1 python3 review/bb4-guard-patch.py && (cd core && cargo test -p aite --test guard)   # 期望 9 条锚点全部「命中 1」、21 passed
  cd ~/Documents/Projects/Aite && scripts/check.sh   # 期望 cargo passed=901 failed=0、contracts passed=27 failed=0、OK 25 files、passed 10/10 —— 这就是情形 B 基线（别的差异按测试名解释）
  cd ~/Documents/Projects/Aite && git status --short && git commit -am "fix(p0-close): AA4 + BB2 重锁 + BB4 守卫" && git push origin main   # status 期望恰好 14 行 ` M`（§4.4 列的 14 个路径）
  ```

  **正在跑的 W1 会话不受影响**（它们的可写面避开了所有 P0-CLOSE 文件）。
  台账里旧的 BB4 命令链第一行 `cd /Users/shensikai/Documents/Aite/.worktrees/task-bb4` 指着已经不存在的目录，别照抄；在仓库根跑就行。
- **H7（第 1 天）P0 真机验收**：P0-CLOSE 之后的 main，在真实飞书测试群跑 M1–M6（M6 三个变体）+ §3.7(a) 免 @ 话题回复探针 + §3.7(b) 群历史权限探针，
  结果写进 `docs/acceptance-M.md`。**这一步宣布 P0 完成。**
- **H8（第 0–1 天，10 分钟）卡片点击实测**：本机按 main 起 edge 连测试租户，用 API Explorer 发一张带回调按钮的 schema 2.0 卡片，点一下，
  看 edge / core 日志里 `card.action.trigger` 有没有到 `OnP2CardActionTrigger`。不确定就在 CC8 的分支上本机编 edge、`AITE_FEISHU_CARD_BUTTONS=1`、
  点一次「停止」读记录下来的帧类型。结果（PASS = 帧类型 event / FAIL）写进 `review/p1/ledger/CC8-clicktest.md`，DD10 按它定默认值。
- **H9（第 1 天）账号与探针（全部留在本机）**：百炼工作区 key（qwen3.7-plus）、DeepSeek、智谱（可选 Kimi / 豆包）、博查或智谱搜索 key；
  钉钉测试组织（企业内部应用 + 机器人，Stream 模式，卡片回调 STREAM，AI 卡片模板）；企微 API 模式智能机器人；GitLab 测试项目 + 项目访问令牌；
  每个模型服务的备案号 / 上线编号。**钉钉探针**：引用回复一条 Aite 的消息，看 `text.repliedMsg.msgId` 能不能对上 Aite 存下的任何 id
  （processQueryKey / outTrackId），结果写 `review/p1/ledger/H9-dingtalk-probe.md`（`docs/p1/dingtalk.md` 在 W1 归 CC9、W2 归 DD11，
  你不直接改，由 DD11 把探针结论并进去）；没确认前 `#A` 文本是主锚点。同一次顺手探两件：飞书群信息接口到底有没有公开 / 私有字段
  （CT21 / NEW21 依赖它）、钉钉卡片实例 `PUT /v1.0/card/instances` 能更新多久（CT13）。企微 bot 要由**超级管理员创建**
  （否则成员 userid 是加密的，成员白名单与审计发起人拿不到明文），或另配一个自建应用换明文（自建应用要配企业可信 IP / 域名）。
- **H10（第 1 天）审 + 合 W1**：**其余 12 轨全部启动之前，CC1 的 draft PR 先别合**（晚启动的会话会 clone 到含 CC1 的 main，开场自检会停）。
  合并顺序：CC1 → CC2 → CC3（**CC2 必须在 CC3 之前**：CC2 的第 ⑩ 项修了「worker 写助手回复 × plane 写用户轮」撞 `DuplicateTurn` 的竞态，
  CC3 单独合进去会让追问丢失、`reconnect_replay` 变抖；两个都合完在 main 上连跑 5 次 `(cd core && cargo test -p aite --test reconnect_replay)`）
  → CC4–CC7 → CC8–CC12 与 T0 的文档 PR。每个 PR 看：CI 绿、回执在、没越可写面（含你自己看一眼 Go 模块文件没被改）、条数增量逐条解释。
  W1 里只有 CC1 改 `Cargo.lock`。**顺手把各轨回执里的「契约缺口」「转给 T0 / T0c」两节抄进一张清单**——同波会话互相看不到回执，H11 与 H12 要用。
- **H11（第 1–2 天，本机，约 45 分钟）落地 T0**（照 `review/t0/APPLY.md`；它会给出每步期望输出，下面是骨架）：

  前提：H6（P0-CLOSE）已推到 main、W1 已合完，且你已经拿 H10 攒的「契约缺口」清单审过 `docs/p1/contract-p1.md`
  ——缺口若必须进 p1.0，就请一个短云会话按设计文档改补丁脚本再落地（比事后开 T0.1 便宜）。

  ```bash
  cd ~/Documents/Projects/Aite && git checkout main && git pull --ff-only && (cd core && cargo build -p aite) && cp core/target/debug/aite /tmp/aite-prelock && git checkout -b contract/p1.0 && AITE_RELOCK=1 python3 review/t0/p1-contract-patch.py --check && AITE_RELOCK=1 python3 review/t0/p1-contract-patch.py && make proto-gen && (cd core && cargo test -p aite-contracts -p aite-proto)
  cd ~/Documents/Projects/Aite && /tmp/aite-prelock contracts lock --check   # 退 1 是对的；MISMATCH 清单必须恰好 = APPLY.md 列的 T0 文件集（P0-CLOSE 的 4 个已在 H6 重锁，出现就停）
  cd ~/Documents/Projects/Aite && AITE_RELOCK=1 /tmp/aite-prelock contracts lock --write && /tmp/aite-prelock contracts lock --check && (cd edge && go build ./... && go vet ./...)
  cd ~/Documents/Projects/Aite && git add -A proto core/crates/contracts core/crates/proto edge config .contracts.lock && git status --short && git commit -m "contract(p1.0): T0" && git push origin contract/p1.0 && git checkout main
  ```

  期望 `cargo test -p aite-contracts -p aite-proto` 全绿（contracts 合计 27+K 条，APPLY.md 逐条点名）、`OK 26 files`；工作区其余部分按预期编不过。
  `git status --short` 里必须看到 `A  core/crates/contracts/src/domain.rs`（新文件，`commit -a` 不会带上它）；没有就停。
  （`/tmp/aite-prelock` 是补丁前编好的二进制：`lock.rs` 只按磁盘上 `proto/aite/v1` 与 `core/crates/contracts` 的字节算哈希，所以它重锁的结果是对的。）
- **H12（第 2 天）派 T0c**：T0c 的派单在 H11 之后按 §5.3 生成，**提交到 `contract/p1.0` 并推送**（云端只 clone GitHub 上已有的内容）：
  `cd ~/Documents/Projects/Aite && git checkout contract/p1.0 && git add review/paste-T0c.md && git commit -m "docs(p1): T0c 派单" && git push origin contract/p1.0 && git checkout main`；
  然后在 claude.ai/code 新建会话、**分支选 `contract/p1.0`**、提示词「读 review/paste-T0c.md 并照做」（`claude --cloud` 能不能指定分支以 CC1 的实测为准，
  不能就只用网页）。T0c 派单的开场自检第 1 步改成「HEAD 就是 `origin/contract/p1.0` 的那个 sha」，不沿用 §4.4 对 `98e4460` 的 diff。
  它的 PR 绿了就合（里面带着你的契约提交），删 `contract/p1.0`，把 B1 的 sha 与条数刷进 W2 的派单并推送。
- **H13（第 2–3 天）派 W2** DD1–DD12，按 §6.3 的顺序合并；DD8–DD11 合并后三端真机冒烟；DD7 合并后本机用真 key 跑一次 live。
- **H14（第 3–4 天）派 W3** EE1–EE14 并合并；读每份回执的「契约缺口」一节，满足 T0.1 启用条件就起 T01 → 你打补丁（重锁 #3）→ T01c，全部在 W4 之前。
- **H15（第 5 天起）派 W4**（FF1 与其余一起派：`im:message.group_msg` 没批下来就把 passive-listen 能力位关着、只 @ 降级交付，
  批下来后只补 FF1 卡里的真群 ambient 实测，不让外部审批卡住 GG1）并合并；本机跑 `scripts/live-matrix.sh` 并提交报告。
- **H16（第 7 天起）W5**：照 `docs/p1/acceptance-P1.md` 在飞书（全量）、钉钉 / 企微（降级矩阵）跑 M-P1 + 3 分钟演示，发现的问题开 S/M 修补轨。
  合规：确认登记范围（单租户私有化大概率不在范围内）、填每个模型的备案号、审计日志保留 ≥190 天。把 EE14 的离线包装到设计伙伴那里。
- **H17（W5 之后）** 决定 W6 做哪些（HH1 / HH2 / HH3）。

---

## 9. 本计划解冻的 P0 冻结项 / 仍然冻结的

**解冻**（每项由指定轨解冻，配回归测试与变异验证）：全局串行派发（CC2 建、T0c 翻）· `UNKNOWN_COMMAND_TEXT` / `KNOWN_COMMANDS` 文案钉
（CC2；`docs/dev-spec` 第 742 行那句只是被取代，不改原文）· worker 固定喂 `all_model_tools()` 与完整目录顺序钉（CC3）· 助手回复不入 transcript（CC3）·
所有模型错误都重试（CC3）· 证据里工具参数不脱敏（CC3）· 证据 payload 追加新键（CC2、CC3）· `CONTRACT_VERSION p0.2` 与「`platform: dingtalk` 被拒」
（T0；Go 侧钉由 DD8 翻）· `SandboxNetwork` 只有 none（T0、DD8）· 沙箱按任务、交付即释放（DD6）· 卡片用文字提示代替按钮（CC8 加开关、DD10 按 H8 定默认）·
回帖文案加 AIGC 标识（DD5 建、EE12 翻）· `platform.md` live 基线（DD4、FF10）· preflight「七组」的组数（FF5）。

**仍冻结**：`evals/p0/*.yaml` · 停机顺序 · BB2 之后的证据哈希公式 · `FakeToolGateway` 的输出格式 · `journal_mode=delete` · `ACTIVE_TASK_STATUSES`。

---

## 10. 风险与对策

| 风险 | 对策 |
|---|---|
| T0 是一次性的 XL 设计，W2/W3 才发现设计漏了要重锁第三次 | 落地前你审 `docs/p1/contract-p1.md`；开放通道（`Session.meta`、`ExternalEvent.payload`、`AuditEvent.detail`、`provider_extra`、trait 默认方法）吸收小缺口；T0.1 有启用门槛且只批量落一次 |
| T0c 独占关键路径、改动横跨所有 crate | 它只做机械活，唯一行为变化是并发默认翻 4；撞上真竞态就停下报告 |
| 守卫在云端静默失效（只在单仓会话加载、失败即放行） | 每份派单开场自检都要求 Read 守卫脚本被拦；T0 的受保护操作全在脚本内部做；数据文件名不含受保护子串 |
| 云端与本机的差异（Linux / 4 vCPU / 冷编译 10–15 分钟 / Go 版本未知 / Debian 与 Docker Hub 可达性） | CC1 实测并记录；安装脚本的可选部分失败只告警 |
| 你是瓶颈：每波约 12 个 PR 评审、两次本机落地、所有真机检查 | 合并顺序写死；回执格式统一；W1 先合 CC1–CC7 这些「没有外部依赖」的 |
| 外部闸门：`im:message.group_msg` 审批时长未知；消息搜索要服务用户 token；卡片按钮待点击实测 | H5 今天就申请；FF1 在权限下来前以「只 @」降级交付；按钮有文本命令兜底 |
| 钉钉 / 企微只在假件上验过，引用字段未文档化 | H9 钉钉探针；`#A` 文本为主锚点；DD11 合并后真机冒烟 |
| 模型行为与调研不符（DeepSeek「不回传就 400」没复现；`deepseek-chat` 被静默改路由） | 云端只跑录制夹具；live 你本机跑；FF9 做六家矩阵 |
| W3 之前 main 上回复还没有 AIGC 标识 | EE12 合并前不做公开试点 |
| 审批不跨重启 | 已作为 D17 让你拍板，并写进给设计伙伴的说明 |
| 凭证注入（MITM）是安全敏感面：CA 私钥保管、edge 本来就握着 docker.sock | 只对带凭证的主机做 TLS 终结；密钥只在 edge；注入记 `injected=true`；docker 测试封闭 |
| 合规不确定：公开 SaaS 的登记范围；GB 45438 元数据字段布局来自第三方解读 | FF6 签收前买标准原文；单租户私有化先行 |
| 按线程沙箱 linger × 4 并发 → 容器数上升 | FF5 的自检加宿主机容量检查 |

---

## 附录 A · 国产模型接入要点（09-25 官方文档）

| 厂商 | 旗舰 / 快模型 | 大陆端点 | 思考与回传 | 工具调用坑 | 缓存 |
|---|---|---|---|---|---|
| 阿里云百炼（Qwen） | `qwen3.8-max`（¥12/¥36 每百万）、`qwen3.7-plus`（¥2/¥8，百炼推荐的 agent 默认）、`qwen3.8-flash` | `https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1`（`dashscope.aliyuncs.com` 2026-09-30 起不再加新功能） | `enable_thinking`、`preserve_thinking` 把历史 `reasoning_content` 折回去 | `parallel_tool_calls` **默认关**要显式开；`tool_choice` 从不支持 `required`，思考模式下指定具体工具也报错，只用 `auto` / `none` | 隐式命中按输入 20% 计；显式 `cache_control` 写 125% / 命中 10% |
| 智谱（GLM） | `glm-5.3`（只思考、纯文本）、`glm-5.3-flash`（多模态） | `https://open.bigmodel.cn/api/paas/v4` | `thinking.type`（5.3 关不掉）；`clear_thinking:false` 时必须原样回传 `reasoning_content` | `tool_choice` **只能 auto** | 智能缓存（隐式） |
| DeepSeek | `deepseek-v4-pro`、`deepseek-flash`（V4.1-Flash，带视觉） | `https://api.deepseek.com` | 思考默认开；文档说带 tools 时必须回传历史 `reasoning_content`，**09-25 实测单轮未回传仍 200** | `strict` 只在 `/beta` | 自动磁盘缓存；工作日 9–12、14–18 为峰时 |
| 月之暗面（Kimi） | `kimi-k3`、`kimi-k2.6` | `https://api.moonshot.cn/v1` | 完整 assistant 消息要回传 | **别传 temperature**（0 会 400）；`max_tokens ≥ 16000` | 缓存写按 TTL 计价 |
| 火山方舟（豆包） | `doubao-seed-2-1-pro-260915`、`-lite-260915` | `https://ark.cn-beijing.volces.com/api/v3` | 思考默认开，返回摘要 + `encrypted_content`；**不回传不报错但效果变差** | 默认回答上限 4k | 缓存按小时收存储费 |
| MiniMax | `MiniMax-M3`、`MiniMax-M2.7` | `https://api.minimax.cn/v1` | `<think>` 混在 `content` 里，除非 `reasoning_split=true` | — | — |

**私有化现实尺寸**：Qwen3.8-27B（单机）、DeepSeek-V4-Flash（284B MoE，一台 8 卡）、GLM-5.3-Flash、MiniMax-M2.7，客户用 vLLM / SGLang（或昇腾 MindIE）挂 OpenAI 兼容地址。

## 附录 B · 合规落点

- **《人工智能生成合成内容标识办法》（2025-09-01 施行）+ GB 45438-2025**：文本 / 卡片显式标识「内容由 AI 生成」（DD5 建、EE12 翻）；图片可见水印（CC12 可选）；
  文件隐式元数据 `AIGC{Label, ContentProducer, ProduceID, ContentPropagator, PropagateID…}`（CC12 标识器、DD5 调用）；云文档页顶部横幅（DD10）；
  提供未标识内容时保留接收方日志 ≥6 个月。
- **网络安全法（2026-01-01 修订施行）**：网络 / 审计日志 ≥6 个月 → 取 **190 天**下限，存在 agent 删不掉的地方（DD1 审计表、EE12 留存任务）。
- **《网络数据安全管理条例》**：注销即删 → 「删内容、留审计元数据与哈希」（FF6）；模型厂商作为受托处理方写进委托处理合同；只用大陆端点（DD7 告警海外端点）。
- **PIPL / 数据安全法**：记忆只存工作事实、拒收明显个人敏感信息（EE1）；内容与审计分存（EE12）；按租户留存。
- **生成式 AI 备案 / 登记**：只调用已备案模型的应用走「登记」而不是「备案」；单租户私有化（不向公众提供）大概率不在《暂行办法》第二条范围内，
  公开注册的 SaaS 大概率要登记；立项方案里「2026-03-17 细则、6-30 / 9-30 截止」在网信办 03-17 / 05-13 / 07-10 公告里都**没找到**，要向浙江 / 杭州网信办或备案代理确认。
  `!about` 与管理台面板展示每个模型的名称与备案号 / 上线编号（EE8、DD12）。
- **等保 2.0 二级**（只有 SaaS 要测）：审计全开、登录失败锁定、会话超时、管理员角色分离、全程 TLS（DD12 起）。

## 附录 C · 对《立项方案 v0.1》的修正

- §8.1 技术栈（Python / FastAPI / Postgres / NATS）作废：仓库规则是 Rust + Go；v1 用 SQLite + 迁移。
- §3.5 平台矩阵的修正见本文 §1.3；「飞书是唯一能完整复刻的平台」仍成立，但飞书也有编辑（只能拉取）与搜索（要 user token）两处降级。
- §5.2 模型型号：`qwen3.8-max` 存在但百炼推荐 agent 用 `qwen3.7-plus`；GLM 当前是 `glm-5.3`（GLM-5 系列是纯文本，多模态是 `glm-5.3-flash`）；
  `deepseek-v4-flash` 已被 `deepseek-flash` 取代；Kimi K3 已在官方与百炼上架；MiniMax 当前是 M3；豆包的实际 ID 见附录 A。
- §5.1「一个 OpenAI 兼容适配层」不够：六家的思考 / 回传契约各不相同（附录 A）。
- §4.8「P1 上 gVisor」在鲲鹏上不成立（gVisor ARM64 要 4KB 页内核，Kylin V10 时代 openEuler 默认 64KB 页）→ gVisor 只在 x86 上可选。
- §4.12 证据保留默认 180 天不够 → 190 天下限；内容与审计分存。
- Git 默认 GitHub → GitLab / 极狐；托管页「境蓝域名」→ 飞书云文档 openchat 共享（自有域名要 ICP 备案，OSS / COS 默认域名会强制下载 HTML）。

## 附录 D · 来源

- Claude Tag 官方：claude.com/docs/claude-tag（users / admins / concepts 各页，09-25 复核）、anthropic.com/news/introducing-claude-tag、claude.com/product/tag。
- 飞书：open.feishu.cn「使用长连接接收回调」（2026-06-11）、消息接收事件（2026-08-05）、消息列表 / 搜索、卡片 PATCH、CardKit 流式、延时更新、撤回事件。
- 钉钉：open.dingtalk.com Stream 模式、机器人接收消息、AI 卡片流式更新、卡片回调、群消息发送；github.com/DingTalk-Real-AI/dingtalk-workspace-cli。
- 企微：developer.work.weixin.qq.com 文档 101463（智能机器人长连接，2026-05-25）、100719（引用消息）、101031、101468。
- 模型：help.aliyun.com/zh/model-studio、docs.bigmodel.cn、api-docs.deepseek.com、platform.kimi.com、docs.volcengine.com/docs/82379、platform.minimax.cn。
- 合规：cac.gov.cn（标识办法 2025-03-14；备案 / 登记公告 2026-03-17、05-13、07-10）。
- 过程材料（本机 scratchpad，不入库）：六份子系统盘点、三份外部调研、三种拆法与三份评审、合并稿。英文原卡已入库为 `review/p1/tracks-2026-09-25.json`。
