# 真模型实测报告 —— 让 DeepSeek 真的去用 checklist

> 2026-09-10 · T19 · 基线 `6ee30d4`
> 装置：`aite/evals/protocol_probe.py`（T12）+ 一个只读 `deps.platform` 的观测脚本
> 这份报告里的每一个数都来自实跑，前后各跑两遍。推测一律标成「推测」，没测到的一律写「没测到」。

## 一句话

**改一句话，`checklist_*` 从 0 次变成 31 次**（after 两遍各 31 次，覆盖 6 个场景，两遍一模一样）。
卡片不再是空壳：`04_csv_to_chart` 的卡片从「42 版全是 0 项」变成「13 版 5 项、内容真正变 3 次」。
顺带一个没预料到的结果：**`04` 从两遍都撞 `max_steps` failed，变成两遍都 delivered**，
步数 40 → 13/14，整套成本还降了 ~15%。

但有三件事必须先说清楚，别把这份报告读成「M3 全过了」：

1. **M3 的「卡片至少更新 3 次」按字面判据 before 就已经过了**（`04` 的 `update_card` 是 **41 次**）。
   派单说「现在是空的」，指的是卡片**内容**空，不是更新次数不够。真正从 0 变成非 0 的是
   「checklist 内容变化次数」：0 → 3。
2. **M3 的「结果 PNG 回到线程」前后都没过**（`send_file` 前后都是 0）。根因是评测替身的沙箱，
   不是 checklist，本轨没碰。
3. **新规则不是 100% 被遵守**：该建的 7 个场景里建了 6 个，`08_step_limit` 两遍都没建。

## 怎么跑的

| | |
|---|---|
| 端点 | `https://api.deepseek.com/v1`（OpenAI 兼容） |
| 模型 | `deepseek-chat` |
| 密钥 | 复用本机 `MAOS_LLM_API_KEY`（总管当场授权），经 `AITE_MODEL_API_KEY` 传入；`config/aite.yaml` 里只有变量名，不入库（`.gitignore:26`） |
| 平台 | `fake`（真模型 + 替身平台） |
| 超时 | `--timeout-scale` 默认 12.0（live） |
| 跑了几遍 | before 2 遍 + after 2 遍（另加 `03`/`04` 各一次单场景跑，专门取卡片快照） |

`passed k/10` 依然不是判据 —— `expect` 照 `model_script` 的台词写，真模型复现不了那些句子。

观测脚本没进仓库（在 scratchpad 里），它只 import `aite.evals` 的现成 API：
`build_deps` + `_execute` 自己驱动一遍，好读到 `deps.platform` 的精确分解 ——
`run_scenario` 不把 `deps` 吐出来，而 `stats()` 只给 `platform_calls` 总数，
按 method 拆不开，也拿不到卡片每一版的内容。

## 1. 它原来为什么不用 checklist

**结论：不是不知道，是提示词把触发条件写成了一个模型在需要它的那一刻还答不出来的问题，
外加一个带贬义的出口。**

证据三条，都来自实测：

**① 它显然知道这些工具存在。** T17 两遍 133 次调用，0 个协议外工具名、0 条 schema 违规
（`live-report-2026-09-10.md` §2）。它读完了 `ALL_MODEL_TOOLS` 全部 10 个工具并正确用了 6 个。
「不知道」这个假设站不住。

**② 触发条件要它预估总步数，而它在第一步估不出来。** 旧提示词第 19 行：

> 任务需要多步时，先 `checklist_add` 列出计划（≤8 项）

「任务需要多步时」要模型在**第一步那个决策点**上判断整个任务要几步。实测它在这个决策点上
做的事恰恰相反 —— `03_checklist_progress`（"把上个月的对账跑一遍"）第一步实录：

```
#0 read_group_history(limit)
   模型自述：I'll start by looking at the recent group history to understand what "对账" refers to here.
```

它先探环境。探完发现 4 步能收，于是从头到尾没建过 checklist。**它不是判断"不需要"，
是根本没到能判断的时候。** before 10 个场景的第一步，8 个是探测类工具，没有一个是 `checklist_add`。

**③ 出口那句给 checklist 附了负面色彩。** 旧提示词第 24 行：

> 简单问题不要摆架子：能一步答完的，第一步直接 `final`，不要为了流程而建 checklist。

"摆架子""为了流程"把建 checklist 描述成走过场。配合 ② 的估不准，模型每次都从这个出口走。

**④ 契约里的工具描述本身也在劝阻（改不了，只能靠提示词压过去）。**
`aite/contracts/protocol.py:24`：

> `checklist_add` —— 添加待办项，**仅在**任务开始或发现新步骤时调用；每项 ≤20 字

"仅在"在工具目录里读起来是使用限制。契约冻结（C2），本轨一个字没动。

## 2. 改了哪几句

核心是**把决策点从「这个任务要几步」（事后才知道）挪到「我下一步想调什么」（当下就知道）**。

| 原文 | 新文 | 为什么 |
|---|---|---|
| `任务需要多步时，先 checklist_add 列出计划（≤8 项）` | `## 第一步只有两种出牌` + `需要先调用任何别的工具才能回答 …… → 第一步必须先 checklist_add` | 换成第一步当下可判定的判据，并与 W3 对齐 —— W3 就是「第一次非 `final` 的 tool_call → 发卡片」，判据一致后卡片和 checklist 一一对应，从结构上消灭空壳卡片 |
| （无） | `判断办法只有一句话：问自己「我下一步想调哪个工具」…… 不要去预估这个任务总共要几步 —— 你在第一步估不准，也不需要估。` | 直接堵掉 ② 那个估不准的前提 |
| `简单问题不要摆架子：能一步答完的，第一步直接 final，不要为了流程而建 checklist。` | `现在就能把答案写完 → 第一步直接调 final。不建 checklist，也不调别的工具。` | **出口保留**（`01_simple_qa` 验的就是这条），但去掉"摆架子""为了流程"的贬义，改成与另一条并列的中性规则 |
| （无） | 三个例子：天气问题 → `final`；对账 → `checklist_add([...])`；CSV 画图 → `checklist_add([...])` | T17 的建议：给正例和反例 |
| （无） | `每做完一项，立刻调 checklist_check 勾掉，不要攒到最后一起勾` + `某一项做不成，用 checklist_fail 写清原因，然后接着往下走，或者 final 说明卡在哪` | 建了不勾，卡片还是死的。后半句给了一条**退出路径** —— 见 §4，这一句是 `04` 不再撞 `max_steps` 的直接原因 |

W9 的四条铁律逐字未动（`test_context.py:110` 断言的两个子串都在）。文件 2246 → 3878 字节。

## 3. 真模型前后对比（每格都是实测，两遍都列）

格式：`跑1/跑2`。

| 场景 | checklist_* 前→后 | update_card 前→后 | 终态 前→后 | 步数 前→后 |
|---|---|---|---|---|
| `01_simple_qa` | 0/0 → **0/0** | 0/0 → 0/0 | delivered/delivered → delivered/delivered | 1/1 → 1/1 |
| `02_thread_followup` | 0/0 → **8/8** | 7/6 → 16/10 | 全 delivered → 全 delivered | 9/8 → 18/12 |
| `03_checklist_progress` | 0/0 → **5/5** | 4/4 → 4/5 | delivered/delivered → delivered/delivered | 4/4 → 4/5 |
| `04_csv_to_chart` | 0/0 → **6/6** | 41/41 → 13/14 | **failed/failed → delivered/delivered** | **40/40 → 13/14** |
| `05_history_summary` | 0/0 → **4/4** | 3/3 → 5/5 | delivered/delivered → delivered/delivered | 5/5 → 6/6 |
| `06_read_document` | 0/0 → **3/3** | 1/1 → 3/3 | delivered/delivered → delivered/delivered | 2/2 → 4/4 |
| `07_commands` | 0/0 → **0/0** | 2/2 → 2/2 | cancelled/cancelled → cancelled/cancelled | 1/1 → 1/1 |
| `08_step_limit` | 0/0 → **0/0** | 3/3 → 3/3 | failed/failed → delivered/failed | 3/3 → 3/3 |
| `09_bot_ignored` | 0/0 → **0/0** | 0/0 → 0/0 | 没建任务 → 没建任务 | 0/0 → 0/0 |
| `10_duplicate_event` | 0/0 → **5/5** | 3/3 → 4/3 | delivered/delivered → delivered/delivered | 4/4 → 5/4 |

**按工具名细分**（after 两遍完全一致）：

```
checklist_add 7 · checklist_fail 18 · checklist_check 6 · checklist_note 0
（before 两遍：全部 0）
```

**`fail` 是 `check` 的三倍**，这不是模型消极，是替身环境的真实反映 —— 沙箱和网关大量返回空，
模型干不成就照铁律第 4 条如实标失败，而不是硬说做完了。真沙箱下这个比例会变成什么样，
**没测到**。`checklist_note` 一次都没被调用，同样**没测到**。

**第一步出的牌**，这轨改的就是这一格：

| 场景 | 前 | 后（跑1/跑2） |
|---|---|---|
| `01_simple_qa` | `final` | `final` / `final` ← **没被误伤** |
| `02_thread_followup` | `read_group_history` | `checklist_add`+`read_group_history` ×2 |
| `03_checklist_progress` | `read_group_history` | `checklist_add`+`read_group_history` ×2 |
| `04_csv_to_chart` | `download_attachment` | `checklist_add` / `checklist_add` |
| `05_history_summary` | `read_group_history` | `checklist_add` / `checklist_add` |
| `06_read_document` | `read_document` | `checklist_add` / `checklist_add` |
| `07_commands` | `read_group_history` | `read_group_history` ×2 ← 没建 |
| `08_step_limit` | `read_group_history` | `read_group_history` ×2 ← 没建 |
| `10_duplicate_event` | `read_group_history` | `checklist_add`+`read_group_history` ×2 |

## 4. M3 逐条判据（`04_csv_to_chart`）

> M3 | @Aite 把这个 CSV 画成月度趋势图（附 CSV） | 线程里出现 checklist 卡片，
> 过程中卡片至少更新 3 次且不新增消息；结果 PNG 回到线程

| 判据 | before（两遍） | after（两遍） | 过了吗 |
|---|---|---|---|
| 线程里出现 checklist 卡片 | 卡片在，**42 版全是 0 项** | 卡片在，13/14 版**各 5 项** | **before 不过 → after 过** |
| 卡片至少更新 3 次 | `update_card` **41 次** | `update_card` 13 / 14 次 | 按字面**两边都过** |
| ↑ 其中 checklist 内容真的变了几次 | **0 次** | **3 次 / 3 次** | before 不过 → after **刚好踩线过** |
| 不新增消息 | 卡片张数 1，`send_text` 1 | 卡片张数 1，`send_text` 1 | 两边都过 |
| 结果 PNG 回到线程 | `send_file` **0** | `send_file` **0** | **两边都不过** |

**卡片前后长什么样**（实测快照，`04` 单场景跑）：

```
before：42 版，每版都是      0 项，唯一在动的是 footer「已用 N 步」
after ：14 版
  版本0  working   0项  已用 1 步
  版本1  working   5项  todo:下载 sales.csv / todo:读取并检查数据 / todo:按月份聚合 / todo:画月度趋势图 / todo:保存 PNG
  版本3  working   5项  done:下载 sales.csv  ← 第 1 项勾掉
  版本4–11         5项  内容不变，只有步数在动
  版本12 working   5项  done:下载 sales.csv + 后 4 项 failed
  版本13 delivered 5项  同上
```

**「刚好踩线过」要当心**：3 次内容变化里有 1 次是建 checklist、1 次是勾掉第 1 项、
1 次是把剩下 4 项一起标 fail。中间版本 4–11 那 8 次更新，卡片上只有步数在动。
演示第二幕想看到「一张卡原地变 ≥3 次」，字面上够了，但**观感上只有 3 个瞬间在动**，
不是连续推进。原因见下一条。

**PNG 为什么还是没回线程**：`04` 的逐步实录（after 跑1）：

```
#0  checklist_add(items)                     ← 新规则生效
#1  download_attachment(file_key)
#2  checklist_check(id) + run_python(code)   ← 下载完立刻勾掉第 1 项
#3–#10  run_python × 8                        ← 撞上评测替身的空返回
#11 checklist_fail ×4                         ← 干不成，诚实标 4 项失败
#12 final(reply)                              ← 如实说明卡在哪
```

沙箱一直返回空，是 `evals/p0/04_csv_to_chart.yaml` 的 `sandbox.exec_script` 只有
`match: savefig` 一条规则（T17 §4 缺口 2 已经定位）。模型读不到数据 → 画不出图 →
没有 PNG。**这条归 T23 的真沙箱开关，本轨没碰。** 真沙箱上能不能过，**没测到**。

## 5. 一个没预料到的结果：`04` 不再撞 `max_steps`

| | before | after |
|---|---|---|
| 步数 | 40 / 40（撞上限） | 13 / 14 |
| 终态 | failed / failed | delivered / delivered |
| `repeat_loops` 原地打转 | `run_python` **30 次 / 32 次** | **0 / 0** |

直接原因是提示词新加的那句退出路径：「某一项做不成，用 `checklist_fail` 写清原因，
然后接着往下走，或者 `final` 说明卡在哪」。before 的提示词里没有任何「可以放弃」的说法，
模型只能一直试到被 `max_steps` 强杀；after 它在第 11 步主动收手。

**但别把这读成「T20 的兜底不需要了」**：`#3–#10` 仍然是连着 8 次 `run_python` 探测，
只是没到 `repeat_loops` 的阈值。空返回导致原地打转的根因一点没变，本轨只是把打转
从 30+ 次缩短到 8 次。**T20 的兜底照做。** 这是本轨最容易被误读的一条。

## 6. 新规则的遵守率（诚实清单）

按新规则「第一步要调非 `final` 工具 → 必须先 `checklist_add`」，10 个场景里：

- **不需要建**：`01`（一步 `final`）、`09`（模型没被调用）—— 行为正确
- **该建且建了**：`02`、`03`、`04`、`05`、`06`、`10` —— **6 个，两遍都建**
- **该建但没建**：`07`、`08` —— **2 个，两遍都没建**

**严格遵守率 6/8。** 两个例外都看得懂：

- `07_commands`（"把全量数据重算一遍"）第 1 步就被 `!stop` 取消，只出了 1 步，**没机会建**。
- `08_step_limit`（"帮我把这事想清楚"）题面模糊到模型不知道"这事"是什么。它第一步的自述是
  「我先看看群里最近在聊什么，才能知道"这事"指的是哪件事」—— 它在**弄清楚任务是什么**，
  而不是在执行任务。这时候确实无从建起计划。

把 `07` 排除（被取消，无机会），**遵守率 6/7**。`08` 是新规则的一个真实盲区：
提示词没说「任务本身还没搞清楚时该怎么办」。要不要补，见 §8。

## 7. W9 第 2 条：checklist 每项 ≤20 字

T17 那轮写的是「**没测到**」（一次都没调用）。这轮测到了。

**after 两遍共 56 项，全部 ≤12 字，一项都没超。** 最长的一项是 `下载 sales.csv`（12 字）。

抽样：

```
03  读群历史找对账口径(9) / 下载对账附件(6) / 逐笔比对(4) / 生成对账报告(6)
04  下载 sales.csv(12) / 读取并检查数据(7) / 按月份聚合(5) / 画月度趋势图(6) / 保存 PNG(6)
05  读取本群消息记录(8) / 筛选本周开放事项(8) / 汇总输出(4)
```

模型不但守住了 20 字，而且是**大幅守住**（中位数 6 字）—— 铁律第 2 条里那三个例子
（"读取 CSV""按月份聚合""画趋势图"）被照抄了句式。**W9 第 2 条从「没测到」转为「实测通过」。**

## 8. 成本

| | 调用次数 | 输入 token（cached） | 输出 token | 约 ¥ |
|---|---|---|---|---|
| before 跑1 | 69 | 168,018（154,750） | 5,448 | 0.101 |
| before 跑2 | 68 | 172,665（159,358） | 5,916 | 0.106 |
| after 跑1 | 55 | 125,677（113,024） | 5,744 | 0.094 |
| after 跑2 | 50 | 111,712（101,632） | 5,322 | 0.083 |

**成本反而降了约 15%。** 建 checklist 多花的步数，被 `04` 从 40 步降到 13/14 步省下的
盖过去了。cache 命中率仍然 ~90%。

> 单价假设同 T17：cache miss 输入 ¥2/百万、cache hit 输入 ¥0.2/百万、输出 ¥8/百万。
> **这个单价是假设，实际以账单为准**，原始 token 数在表里。
> 本轨全部实跑（含两次单场景取快照）合计约 **¥0.42**。

## 9. 这轮没测到的

诚实清单，别把「没测到」读成「测过了没问题」：

1. **真沙箱** —— 全程 `--platform fake` + 替身沙箱。M3 的「PNG 回到线程」在真沙箱上能不能过，
   这份报告一个字都回答不了（归 T23）。
2. **`final.artifacts` → `get_file` → `send_file`** —— `send_file` 前后都是 0，W5 这条路
   在真模型上仍然一次都没走通（跟 T17 同样的「没测到」）。
3. **别的模型** —— 只测了 `deepseek-chat`。新规则对更小的模型是否同样有效，**推测**它会
   比旧写法好（判据更机械），但没有实测支撑。
4. **`08` 那个盲区补上之后会怎样** —— 没改，所以没测。
5. **真人观感** —— 「卡片内容变 3 次、中间 8 次只有步数在动」够不够演示第二幕用，
   这是人的判断，不是实测能回答的。

## 10. 建议（要总管定的）

1. **`08` 的盲区要不要补一句**：比如「任务本身没说清楚时，先把它问清楚或探明白，
   探完再建 checklist」。代价是提示词又长一点，而且这句本身也可能被当成新出口。
   **倾向不补** —— 6/7 的遵守率对演示够用，补它的收益不确定，而每加一条出口都是风险。
2. **演示第二幕的观感**：卡片内容只在 3 个瞬间变。如果要「连续推进」的观感，
   根子在 `04` 的替身沙箱返回空（模型勾不动第 2 项之后的项），**该修的是 T23 的真沙箱**，
   不是再改提示词。
3. **`05`/`06` 现在会多出一张卡片**（原来 2 步收敛，现在 4–6 步）—— 总管已确认按
   「跟 W3 对齐」的口径走，此处只做备案：这是预期内的行为改变，不是回归。
