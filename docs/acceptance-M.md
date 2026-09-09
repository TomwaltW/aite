# 人工验收 M1–M6 剧本（真实飞书测试群）

对应 `docs/dev-spec-2026-09-09.md` §2.4。那张表只写了「操作 → 期望」，本文把它变成
照着做就能跑的东西：每条 M 拆成**操作 / 在哪看 / 期望 / 不对时查哪**四段。

这是 P0 的最后一关，跟 pytest 最大的不同是：**出了问题没有断言告诉你哪一行红了**，
只有群里一条没回的消息、一张卡在 working 的卡片。所以每条 M 都配了「在哪看」——
把「看起来不对」变成「哪个环节不对」，靠的是日志、证据链、`docker ps` 这三个窗口。

> **标注约定**：带 ⏳ 的命令是 **T7 / T9 并行轨的产物，本文写就时还没合入**，
> 命令行按各自派单里写死的形状写。**这些命令待 T7/T9 合入后实跑校验。**
> 没有 ⏳ 的命令（`scripts/evidence_show.py`、`docker`、`git`）都是本机实跑过的。

---

## 0. 通用前置

### 0.1 起飞前 60 秒：外部依赖自检 ⏳

```bash
.venv/bin/python scripts/preflight.py                 # 七组检查，全 OK → 退出码 0
.venv/bin/python scripts/preflight.py --offline       # 只跑不需要网络/docker 的 1、2、7
.venv/bin/python scripts/preflight.py --json          # 机器可读
```

七组分别是：① 配置可加载 ② 环境变量齐 ③ 飞书凭证有效 ④ 飞书身份对得上
⑤ 模型端点通 ⑥ 沙箱可用 ⑦ 落盘目录可写。**任一 FAIL 就别往下走** —— M1–M6 里
八成的「没反应」都是这七项里的某一项没配好，在这里花 60 秒比在群里瞎试便宜得多。

第 ④ 组尤其要过：它拿 token 查机器人自身信息、和 `FEISHU_BOT_OPEN_ID` 的取值比对。
**配错了应用时 M1 会完全静默**（收得到事件但认不出 @ 的是自己），没有任何报错。

### 0.2 起飞 ⏳

```bash
.venv/bin/python -m aite.app --config config/aite.yaml
```

起飞日志有一行「接了谁」，写着 platform / model 名 / sandbox 镜像 / sqlite 路径 /
evidence 目录。**先把这一行抄下来**，后面每一条 M 的排障都从它开始 ——
尤其是 evidence 目录，`evidence_show.py` 要用。

- `--traceback`：出错时打完整栈（默认只给人话）。
- 停：Ctrl-C（SIGINT/SIGTERM 走同一条优雅退出路径）。**再按一次是硬退**。

### 0.3 三个观察窗

跑 M 之前把这三个窗口都开好，出了问题不用现找。

| 窗口 | 命令 | 看什么 |
|---|---|---|
| 进程日志 | `python -m aite.app` 那个终端 ⏳ | 关键字见 §7 速查表 |
| 证据时间线 | `.venv/bin/python scripts/evidence_show.py --list` | 最近的任务、终态、链是否完整 |
| 沙箱 | `docker ps --filter label=aite.task` | 有没有容器、是不是该收没收 |

**每条 M 做完的固定收尾**：

```bash
# 1. 找到刚才那个任务（时间倒序，第一行就是）
.venv/bin/python scripts/evidence_show.py --list

# 2. 看它的时间线（--config 指到真配置，花费一栏才算得出来）
.venv/bin/python scripts/evidence_show.py --config config/aite.yaml <task_id>
```

时间线末尾那行 `hash 链` 是这份证据可不可信的判据。链断了 → 退出码 1，
并指出断在第几行、期望什么、实际什么。**链断了就别拿这份证据当验收依据**，
先确认目录有没有被人手改过、备份脚本有没有漏拷 `payloads/`。

`--list` 里任一任务链断了，整条命令也退出码 1，所以可以直接 `&&` 串在脚本里。

### 0.4 关于「卡片上的证据按钮」

契约 R3 支持卡片的 `evidence` 按钮（点了回帖证据目录路径），但 P0 的
`aite/worker/card.py` 渲染卡片时用的是 `ChecklistCard.actions` 的默认值 `["stop"]`——
**实际卡片上只有「停止」按钮，没有「证据」按钮**。拿证据路径请走 `--list`，
别在卡片上找。（这条已列进 §8 给下一轮的输入。）

---

## M1 · 群里 @Aite 有反应

> §2.4：测试群 @Aite 你好 → 2 秒内触发消息出现 👀 类表情回应

### 操作

在测试群里发一条：`@Aite 你好`

### 在哪看

- 群里：**你发的那条消息**上的表情回应（不是机器人新发一条消息）。
- `scripts/evidence_show.py --list`：应该多出一个任务。
- 日志：没有 `ingress.slow_callback` / `ingress.handle_failed` / `control.dispatch_failed`。

### 期望

1. **2 秒内**你那条消息上出现 👀（飞书 `EYES` 表情）。这是 R7 的 `add_reaction(ack)`，
   在建会话之后、入队之前发出，所以它先于任何回复出现。
2. 随后线程里出现一条**纯文本**回复（不是卡片）。W3：第一步就 `final` 的
   Answering 路径不发卡片。
3. `evidence_show.py <task_id>` 的时间线形如：

   ```
   0  task_created     #A1 「你好」 by=ou_… chat=oc_…
   1  event_received   message msg=om_… mentioned=True event=evt_…
   2  model_call       <模型名> step=0 finish=stop token in=… out=… ¥…
   3  tool_call        final(reply=…)
   ★  4  delivered     已交付 · 产出 0 个 · 1 步
   ```

   末尾 `hash 链 OK`，退出码 0。

### 不对时查哪（按最可能排序）

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 没表情、没回复、`--list` 也没有新任务 | 事件根本没到进程 | 开放平台「事件订阅 → 推送记录」看这条有没有推出来；没有就是权限/订阅没配（§3.7 清单）；有就看进程日志有没有连上（0.2 那行起飞日志） |
| 同上，但推送记录里有 | 认不出 @ 的是自己 → R7 不命中 → R8 丢弃 | `FEISHU_BOT_OPEN_ID` 配的是不是这个应用的 open_id。跑 preflight 第 ④ 组 ⏳ |
| 有表情，没回复 | 模型这一步炸了 | 日志 `worker.model_failed`；`evidence_show.py <task_id>` 看 `model_call` 那条的 `finish_reason`；跑 preflight 第 ⑤ 组 ⏳ |
| 有表情有回复，但超过 2 秒才出现表情 | 回调里被塞了重活 | 日志 `ingress.slow_callback`（>1s 就 WARNING，带 `elapsed=`） |
| 机器人自己触发了自己 | R1 没拦住 | 不该发生（`sender_kind != human` 直接丢）。真出现了记下来，这是 bug 不是环境问题 |

---

## M2 · 断网重连，且只处理一次

> §2.4：拔网线 30 秒再插回 → 服务自动重连；断网期间群里发的 @ 在重连后被处理且只处理一次

### 操作

1. 关掉跑 `aite.app` 那台机器的网络（拔网线 / 关 Wi-Fi），**不要停进程**。
2. 断网期间在群里发**一条** `@Aite 断网期间这条`。
3. 等 30 秒，恢复网络。

### 在哪看

- 日志：`feishu.reconnecting attempt=N delay=Ns` → `feishu.reconnected after=N attempts`。
  退避是 1s→2s→…→30s 封顶、无限重试（§3.3）。
- `scripts/evidence_show.py --list`：断网期间那条消息应该**只**对应**一个**任务。
- 计数器 `events.duplicate`：平台重连后重推同一事件时 +1（R2）。

### 期望

1. 断网期间日志持续打 `feishu.reconnecting`，**进程不退出**。
2. 恢复网络后出现 `feishu.reconnected`。
3. 断网期间那条 @ 被处理，群里有回复。
4. **只有一个任务**：`--list` 里对应那条消息的任务只有一条，不是两条。
   平台重推同一个 `event_id` 时 R2 靠 `seen_event` 静默丢弃。

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 恢复网络后没有 `feishu.reconnected` | 重连循环挂了 | 看有没有 `feishu.reconnecting` 在持续打；完全没有就是读循环已经死了 —— 记下日志末尾，这是 bug |
| 断网期间那条 @ 完全没被处理 | 平台没重推 | 飞书的补推不保证；换成「断网 10 秒」再试一次。连续两次都不补推，就是平台行为，记进结论、不算代码问题 |
| **同一条消息出了两个任务** | R2 去重没生效 | `evidence_show.py` 看这两个任务的 `event_received` 那条，`event=` 是不是同一个 `event_id`。是 → `seen_event` 没落库（查 sqlite 路径可写、`storage.sqlite_path` 配得对不对，preflight 第 ⑦ 组 ⏳）；不是 → 平台推了两个不同 event_id，属于平台行为 |
| 进程直接退了 | 未捕获异常漏出去了 | §3.3 要求进程不退出。抓日志末尾的栈，这是 bug |

---

## M3 · CSV 画图：卡片、产物、沙箱回收

> §2.4：@Aite 把这个 CSV 画成月度趋势图（附 CSV）→ 线程里出现 checklist 卡片，
> 过程中卡片至少更新 3 次且不新增消息；结果 PNG 回到线程；5 分钟后 `docker ps` 无该任务容器

这是六条里最重的一条，它同时验 W3/W4/W5/W7 四条 worker 规则。

### 操作

1. 准备一个小 CSV（两列就行：`month,amount`，12 行），**别用大文件** ——
   这一步验的是链路，不是性能。
2. 群里发：`@Aite 把这个 CSV 画成月度趋势图`，**同一条消息带上 CSV 附件**。

### 在哪看

- 群里那条线程：卡片消息**只有一条**，内容在变。
- `docker ps --filter label=aite.task`：任务跑的时候应该看得见一个容器。
- 跑完后：`scripts/evidence_show.py --config config/aite.yaml <task_id>`。

### 期望

1. **卡片只有一条**。W3：第一次出现非 `final` 的 tool_call 时才 `send_card`；
   之后一律 `update_card`。「不新增消息」= 从你那条 @ 到最终文本回复之间，
   线程里新增的消息是 **1 张卡片 + N 个产物文件 + 1 条文本**（M3 里 N=1，共 3 条），
   卡片自始至终只有那一条、不重复出现。
2. **卡片至少更新 3 次**。怎么数：
   - 肉眼：盯着卡片，checklist 的项从 ○ 逐个变 ✓，footer 的「已用 N 步 · ¥X.XX」在涨。
   - 证据侧的必要条件：`evidence_show.py <task_id> --only checklist_op` 至少 3 行。
     每次 `checklist_check` 都写一条 evidence（W8），而卡片更新由它驱动。
   - ⚠️ **`update_card` 本身不写 evidence，也没有日志**，所以「真的调了 3 次」
     只能靠肉眼确认，证据里查不到。见 §8。
     （W4 会把 500ms 内的多次变更合并成一次 `update_card`，所以
     `checklist_op` 条数 ≥ 实际更新次数，是必要非充分条件。）
3. **PNG 回到同一线程**：文件消息的 `reply_to` 是话题 root（W5）。
4. 时间线里能看到这一串：

   ```
   tool_call        download_attachment(file_key=…, dest=/work/…)
   tool_result      download_attachment → ok …ms
   tool_call        run_python(code=…, timeout_sec=…)
   tool_result      run_python → ok …ms
   artifact         「月度趋势」 image/png …B sha=xxxxxxxx
   ★ delivered      已交付 · 产出 1 个 · N 步
   ```

5. **沙箱回收**：任务结束后容器**不会立刻消失**（W7 交给 reaper）。
   - reaper 每 **60 秒**跑一次，回收空闲超过 `config.sandbox.idle_sec`（默认 **300 秒**）的容器。
   - 所以最坏情况是 **300 + 60 = 360 秒**。§2.4 写的「5 分钟后」踩在边界上：
     **建议等满 6 分钟再判**，5 分整还在的不算失败。
   - 回收时日志打 `control.reaped`，计数器 `sandbox.reaped` +N。

   ```bash
   docker ps --filter label=aite.task          # 6 分钟后：空
   docker ps -a --filter label=aite.task       # 连停掉的也算：空
   ```

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 没出卡片，直接回了一段文字 | 模型一步就 `final` 了，没调工具 | 这是 W3 的 Answering 路径，**不算错**，但说明模型没去读附件。`--only tool_call` 看有没有 `download_attachment`；没有就是提示词/模型的问题 |
| 卡片出来了，一直停在 working | 某个工具卡住 | `--only tool_call,tool_result` 看最后一条：只有 `tool_call` 没有 `tool_result` = 正卡在那一步；有 `tool_result` 且 `FAIL[timeout]` = 超时（`run_python` 用请求里的 `timeout_sec`，其余默认 60s） |
| `tool_result → FAIL[sandbox]` | Docker 不可用 / 镜像不在 | `docker images aite-sandbox`；不在就 `docker build -t aite-sandbox:p0 docker/sandbox`。连续 2 次 sandbox 失败 → 任务直接 failed（§3.3） |
| `tool_result → FAIL[upstream]`（下载附件） | adapter 下载失败 | 附件是不是过期了/太大；换个小文件重试 |
| 回帖里有「产物 x 未找到」 | 模型给的 path 不在 /work 下或不存在 | §3.3 规定跳过该产物、任务仍 delivered。`--only artifact` 看实际写出去几个；`delivered` 那行的 `产物缺失 N 个` |
| 卡片更新次数不够 3 次 | 模型没建足够的 checklist 项 | `--only checklist_op` 数条数。少于 3 条是模型行为，不是链路故障 —— 换个更需要分步的任务重试 |
| 6 分钟后容器还在 | reaper 没跑 / 释放失败 | 日志 `control.reap_failed`、`control.gateway_release_failed`、`worker.gateway_release_failed`；都没有就看 reaper 那条协程是不是根本没起（回到 0.2 的起飞日志） |
| 群里有两条卡片 | W3/W4 的合并没生效 | 这是 bug，把两条卡片的消息 id 和 `evidence_show.py` 输出一起记下来 |

---

## M4 · 线程里追问，命中同一会话

> §2.4：线程里追问「再按季度画一张」（不带 @ 或带 @，取决于 3.7 的权限核实结果）
> → 命中同一 `session_id`（日志可查），沙箱重建，第二张图回到同一线程

### ⚠️ §3.7 的两条待核实项，直接决定这条怎么操作

规范 §3.7 留了两条起飞前要核实的权限，**本文两种情况都写**，不赌一种：

**(a) 只有「接收 @ 消息」权限时，话题里不带 @ 的回复会不会投递给应用？**
决定 `PlatformCapabilities.supports_passive_listen` 的运行时取值
（契约默认 `FEISHU_P0.supports_passive_listen = False`，拿到「获取群组中所有消息」
后由 adapter 在运行时改成 `True`）。

**(b)「获取会话历史消息」API 是否要求「获取群组中所有消息」这个敏感权限？**
这条主要影响 M5（群历史），但和 (a) 是同一个权限包，往往一起批下来 ——
所以 (b) 批了通常 (a) 也就成立了。

**这两条都不影响路由逻辑**：R6 在 R7 之前求值，且**不要求 `mentioned`** ——
只要事件到得了进程、`anchor.thread_id` 命中 `find_session_by_thread`，
带不带 @ 都续接同一个会话。差别**只在投递**。

### 操作

分两步做，顺序别换 —— 这个顺序本身就是对 §3.7(a) 的核实：

1. **先试不带 @**：在 M3 那条话题（卡片所在的线程）里回复 `再按季度画一张`，**不 @**。
   等 30 秒。
2. **30 秒内没反应，再补一条带 @ 的**：`@Aite 再按季度画一张`，还是发在同一条话题里。

### 在哪看

- `scripts/evidence_show.py --list`：应该多出**一个新任务**（不是新会话）。
- 新任务时间线第 0 条 `task_created` 里的 `session=` 字段，要和 M3 那个任务的一致。

  ```bash
  .venv/bin/python scripts/evidence_show.py <M3的task_id> --only task_created
  .venv/bin/python scripts/evidence_show.py <M4的task_id> --only task_created
  # 两行的 session= 必须相同
  ```

  用 `--json` 更好比：`.…--json | python -c "import json,sys;print(json.load(sys.stdin)['events'][0]['fields']['session_id'])"`
- `docker ps --filter label=aite.task`：M3 的容器多半已经被 reaper 收了，
  这一轮会**重建**一个新的（容器 id 不同）。

### 期望

**情况 A —— 不带 @ 就有反应**（`supports_passive_listen` 实际为 True）：

1. 第 1 步就触发任务，走 R6 续接。
2. 新任务的 `session_id` 与 M3 相同，`task_no` 比 M3 的**大**
   （租户内原子递增，中间有别的任务就会跳号，不一定正好 +1）。
3. 第二张图回到**同一条话题**里。
4. 结论：§3.7(a) = **不带 @ 也投递**，`supports_passive_listen` 该置 True。

**情况 B —— 不带 @ 没反应、带 @ 才有**（`supports_passive_listen` 为 False）：

1. 第 1 步 30 秒无任何动静：没表情、没回复、`--list` 没有新任务。
2. 第 2 步（带 @）触发任务，**同样走 R6**（thread_id 命中优先于 mentioned），
   所以 `session_id` 仍与 M3 相同、`task_no` 比 M3 的大。
3. 第二张图回到同一条话题里。
4. 结论：§3.7(a) = **不带 @ 不投递**，`supports_passive_listen` 保持 False，
   并且要在飞书群的使用说明里写清「话题里追问也要 @」。

**两种情况共同的判据**（这条 M 真正验的是它，与 (a) 的结论无关）：
`session_id` 相同 + 第二张图回到同一线程 + 沙箱重建。

### 怎么判「情况 B」是没投递、还是投递了被丢弃

这两者对操作的结论一样（都得 @），但对代码的结论完全不同，别混：

1. 去飞书开放平台 →「事件订阅 → 推送记录 / 调试」，查第 1 步那条消息。
   - **推送记录里没有** = 平台压根没投递 → §3.7(a) 结论为「不投递」，权限问题，代码没错。
   - **推送记录里有** = 投递了但进程丢了它 → R6 没命中，**这是 bug**。
2. ⚠️ 进程侧目前**帮不上忙**：被 R8 丢弃的事件在 INFO 级别不留任何日志，
   `events.ignored` 计数器也没有对外的查看入口。所以只能靠开放平台的推送记录判。见 §8。

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 带 @ 也没反应 | 不是话题里的回复，是新消息 | 飞书里「回复」和「在群里另发一条」不是一回事。确认发的是**对卡片那条消息的回复**（线程内） |
| 有反应，但 `session_id` 变了 | `anchor.thread_id` 没命中 | `--only task_created,event_received` 看新任务的 `msg=`；R7 会把这条消息自己变成新话题 root。多半是回复挂错了父消息 |
| 有反应，但图回到了群里而不是线程 | `reply_to` 没带话题 root | W5 要求 `reply_to = 话题 root`。这是 bug |
| 第一步就有反应，但回了「未知命令」 | 文本以 `!` 开头了 | R5 先于 R6 判定。别用 `!` 开头（可用命令：`!status` `!stop <任务号>` `!restart` `!new`） |
| 追问被当成了 steer（合并进当前任务）而不是新任务 | M3 的任务还在跑 | R6：会话有活跃 task → 排队为 steer 消息。**等 M3 彻底 delivered 再做 M4** |

---

## M5 · 汇总群历史，引用真实消息

> §2.4：@Aite 汇总本群本周开放事项 → 回复引用到 ≥3 条真实群消息

这条依赖 §3.7(b) 的权限结论。**先在群里制造素材**：至少 5–6 条不同人发的、
带明确待办口吻的消息（「X 那个还没弄完」「Y 下周之前给我」之类），否则模型没得引。

### 操作

群里发：`@Aite 汇总本群本周开放事项`

### 在哪看

- 群里的回复正文：里面应该带得出**具体的消息内容**，能和群里真实存在的消息对上。
- `scripts/evidence_show.py <task_id> --only tool_call,tool_result`：
  应该有一条 `read_group_history`。

### 期望

1. 时间线里有 `tool_call read_group_history(...)` 且对应的 `tool_result → ok`。
2. 回复正文里能对上 **≥3 条**真实群消息。
   - W1 给模型的群历史格式是 `[message_id] 姓名: 文本`，且**只保留 `sender_kind == human`**。
   - 逐条核：回复里提到的每件事，都能在群里找到那条原始消息。
     **模型编出来的算不通过** —— 这条 M 验的就是「它真的读到了群历史」。
3. 群历史窗口是 `config.feishu.history_window`（默认 50 条）。

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| `tool_result → FAIL[denied]` 或 4xx | §3.7(b)：缺「获取群组中所有消息」敏感权限 | 开放平台看这个权限的审批状态。**这就是 §3.7(b) 的答案：需要敏感权限** |
| `read_group_history` 返回了，但内容是空的 | 群里没有符合条件的消息 | 拉历史只留 human；机器人自己发的不算。先在群里补几条真人消息 |
| 日志 `worker.read_history_failed` | 拉历史抛异常了 | 看栈。W1 里拉历史失败是软失败（继续跑，只是上下文里没有群历史），所以任务仍会 delivered —— **别被「有回复」骗了**，一定要核 `tool_result` |
| 回复里的事项在群里找不到 | 模型编的 | 不是链路故障。`--only tool_call,tool_result` 确认历史真的拉到了；拉到了还编，是提示词/模型的问题（W9 明确要求「不得声称做了没做的事」） |
| 压根没调 `read_group_history` | 模型没想到要用 | 换个更明确的说法重试（「读一下最近的群消息，汇总…」）。仍不调 = 工具目录/提示词问题 |

---

## M6 · 重启后旧线程还能续接

> §2.4：`systemctl restart` / 杀进程重启后在旧线程追问 → 仍能续接

### 操作

1. 记下 M3/M4 那条话题。
2. **Ctrl-C 停掉进程**（或 `kill <pid>`，SIGTERM 走同一条优雅退出路径）。
3. **确认进程真的没了**：`ps aux | grep aite.app`。
4. 重新起飞：`.venv/bin/python -m aite.app --config config/aite.yaml` ⏳
5. 在**同一条旧话题**里追问：`@Aite 刚才那张图换成柱状的`（按 M4 的结论决定带不带 @）。

### 在哪看

- 重启后的起飞日志：`sqlite 路径`那一段要和重启前**是同一个文件**。
- `scripts/evidence_show.py --list`：新任务的 `session_id` 要和旧任务相同。

### 期望

1. 新任务的 `session_id` 与重启前那条话题的会话相同 —— 会话在 SQLite 里，
   进程重启不丢（`find_session_by_thread` 按 `anchor.thread_id` 查）。
2. `task_no` 在旧编号上继续递增，不从 `#A1` 重来。
3. 证据链接得上：新任务是**新的** `{evidence_dir}/{task_id}/` 目录
   （证据按 task 分目录，不按 session），链从 GENESIS 重新起，这是对的。

### 顺手多验一条（成本很低，值得做）

杀进程时如果**有任务正在跑**，那个任务的证据目录会停在「没写 manifest.json」的状态。
渲染它，确认工具认得这种半截证据：

```bash
.venv/bin/python scripts/evidence_show.py <被杀掉的task_id>
# 期望：manifest 那行显示「未 finalize」，事件照常渲染，hash 链 OK，退出码 0
```

**「未 finalize」不等于「证据损坏」** —— 这是排障时最容易误判的一处。

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 重启后追问变成了新会话（`session_id` 不同） | SQLite 换文件了 | 比对重启前后起飞日志里的 sqlite 路径；相对路径 + 换了工作目录最常见 |
| 重启后追问完全没反应 | 进程没真起来 / 没连上 | 起飞日志那一行；`preflight.py` 第 ③ 组 ⏳ |
| `task_no` 从 `#A1` 重来 | `next_task_no` 的计数表没落库 | 编号存在 SQLite 的 `task_counters` 表里，重启该接得上。回退到 `#A1` = sqlite 换文件了（同上一行），或计数没提交 —— 贴 `--list` 输出 |
| 重启前那个跑到一半的任务，重启后自己接着跑了 | 不该发生 | P0 没有任务恢复。它应该停在没有终态事件的状态（`--list` 里终态列显示「无终态」）。真自己跑起来了，记下来 |
| Ctrl-C 按不动 | 优雅退出卡住了 | **再按一次是硬退**（T7 的实现里第二次信号立即硬退）。要记下第一次为什么没退 |

---

## 7. 日志关键字速查

进程日志里出现这些就有故事，按 logger 名分组：

| 关键字 | 级别 | 说明 |
|---|---|---|
| `feishu.reconnecting attempt=N delay=Ns` | WARNING | 长连接在退避重连（1s→2s→…→30s 封顶） |
| `feishu.reconnected after=N attempts` | INFO | 重连成功。M2 要看的就是它 |
| `ingress.slow_callback event=… elapsed=…s` | WARNING | 回调超过 1s，违反 §3.3 第一行 |
| `ingress.handle_failed event=… kind=…` | ERROR | 路由里抛异常，已吞掉不让长连接挂掉 |
| `control.dispatch_failed` | ERROR | 派发任务失败 |
| `control.no_worker` | — | 没接 worker（组装漏了，回到 0.2） |
| `control.reaped` | INFO | reaper 收了容器。M3 第 5 条要看的 |
| `control.reap_failed` | ERROR | 回收失败，容器会留着 |
| `control.gateway_release_failed` / `worker.gateway_release_failed` | ERROR | 沙箱没还回去 |
| `worker.model_failed` | ERROR | 模型调用炸了（已重试 2 次，2s/5s） |
| `worker.read_history_failed` | ERROR | 拉群历史失败（软失败，任务继续）。M5 要看的 |
| `worker.artifact_failed` | ERROR | 取产物失败 |
| `worker.fail_notice_failed` | ERROR | 连「任务失败」的回帖都没发出去 |
| `worker.unhandled` | ERROR | 未捕获异常，任务 failed 但进程活着 |

计数器（`ControlPlane.counters` / `Ingress.counters`，**目前没有对外查看入口**，见 §8）：
`events.handled` / `events.duplicate` / `events.nonhuman` / `events.ignored` /
`events.steer` / `events.edited` / `events.deleted` / `sandbox.reaped` / `ingress.errors` / `ingress.slow`。

---

## 8. 跑这份剧本时发现的观测缺口

写剧本时发现有几件真机排障需要的事，现在**查不到**。这里只记录，
`aite/` 的修改不属于本轨（T10 只读消费证据目录），留给下一轮：

1. **`created_at` 不进 hash 链。** `hash = chain_hash(prev_hash, payload_hash)`，
   而 `payload_hash` 只覆盖 payload —— 改掉 `events.jsonl` 里所有时间戳，
   链校验照样全绿。M2/M3 的时序判断建立在这些时间戳上，值得知道它没被保护。
2. **卡片的发送与更新不写 evidence，也没有日志。** M3 明确要求「卡片至少更新 3 次
   且不新增消息」，但 `send_card` / `update_card` 在证据里没有任何痕迹，
   只能靠肉眼数。建议加 `card_sent` / `card_updated` 两类事件（或复用 `checklist_op`）。
3. **被丢弃的事件不留痕。** R1/R2/R8 丢弃事件时只加内存计数器，INFO 级别没有日志，
   计数器也没有查看入口。M4 判「没投递 vs 投递了被丢」因此只能去开放平台看推送记录。
4. **卡片上没有「证据」按钮。** 契约 R3 支持 `evidence` 动作，但 `render_card`
   用的是 `ChecklistCard.actions` 的默认值 `["stop"]`，那条分支实际走不到。
5. **`checklist_op` 的 check/fail 只记 `id` 和 `state`，不带那一项的文本。**
   `evidence_show.py` 已经通过回放前面的 `add` 事件把文本补了回来，
   但这意味着**单看一条 `checklist_op` 是读不懂的**，任何别的消费方都得自己回放。
6. **`tool_result` 只有 `content_hash`，没有摘要。** 工具失败时证据里只有
   `error` 的错误码（`timeout` / `sandbox` / `invalid_args` …），
   拿不到那一行具体的报错文本。排 M3 的沙箱问题时这一点最疼。
7. **沙箱 id 不进证据。** 任务和容器对不上号，M3 查「哪个容器该收没收」
   只能靠时间先后猜。
