# 真模型全场景实测 —— 十个场景 × 两档 × 两遍

> 2026-09-12 · V4 · 基线 `8d6ffd3`
> 装置：`core/crates/evals/src/protocol_probe.rs`（`--protocol-report`）
> 这份报告里的每一个数都来自实跑。推测一律标成「推测」，没测到的一律写「没测到」。
> 前四份（`live-report-2026-09-10*.md` 三份 Python 版 + `live-report-2026-09-12-romega.md`）
> 的判据与读法沿用不变；**本报告更正 RΩ §4 的一处基线错配**，见第 5 节。

## 一句话

**换语言之后，模型的出牌形状没变，而且是逐格没变。** fake 档第一遍 `passed 5/10`，
红的**正好就是** T23 那五条（`01`/`03`/`04`/`07`/`10`），一条不差；十个场景 × 两遍 = 20 格
「第一步出的牌」里，**19 格与 T19 §3 那张表逐格相同**。

**RΩ §4 那条「与 Python 版结论相反」不是翻转，是比错了基线** —— 三条硬证据在第 5 节，
其中最硬的一条：RΩ 引的 T17 基线 `0d6939c` 当时 `platform.md` 是 **2246 字节**（T19 改提示词之前），
而 Rust 用的这份与 Python 删除前最后一版**逐字节相同**（3878 字节，sha256 同为 `0d2ecfe8…`）。

**checklist 结论：稳定用。** 40 格（10 场景 × 4 遍）里 39 格在「建不建 checklist」上一致，
唯一抖动的是 `06_read_document` 在 fake 第二遍没建。

**真沙箱把叉变成了勾，而且是演示要的那种连续推进。** `04_csv_to_chart` 在 docker 档下
`checklist_check` 从 fake 档的 1 次变成 **5 次**、`checklist_fail` 从 4 次变成 **0 次**，两遍完全一致；
卡片内容在 12 步里变了 **6 次**（建 + 逐项打勾 5 次），而 fake 档只有 3 次、中间 8 步只有步数在动。
**这直接兑现了 T19 §10 建议 2 的判断**（「要连续推进的观感，该修的是真沙箱，不是再改提示词」）。

---

## 2. 怎么跑的

### 先读这一条：`--model live --sandbox fake` 是错配，但只对两个场景

RΩ §2 给的读法是「**`--model live` 之后请接着写 `--sandbox docker`**」。这话成立，
但**范围要收窄**。场景的 `sandbox.exec_script` 是照 `model_script` 那份台词写的，
真模型写的代码对不上，`FakeSandbox` 就返回无害的默认值（退出码 0、stdout 空、无产出文件）。

**十个 yaml 里只有两个有 `sandbox.exec_script`**（实测 `grep -l exec_script evals/p0/*.yaml`）：

| 场景 | `exec_script` 的匹配条件 | 对 `--sandbox fake` 是不是错配 |
|---|---|---|
| `04_csv_to_chart` | `:21` `match: savefig` | **是** |
| `07_commands` | `:38` `match: "while"` | **是**（但它 1 步就被 `!stop`，压根没走到沙箱） |
| 其余八个 | 没有 `sandbox:` 段 | **不是** —— 它们压根不碰沙箱 |

**所以两档各有各的用，都得跑**：

- **fake 档**比的是「换语言之后模型的出牌形状变没变」—— 与 T19 §3 / T23 §三**逐格可比**
  （那两张表跑的都是 `--model live --sandbox fake`，T23 §二的命令逐字写着 `--platform fake --model live`）；
- **docker 档**比的是「真沙箱 + 真 Gateway 那一段还通不通」—— Python 侧只在 `04` 上跑过。

### 装置

| | |
|---|---|
| 端点 | `https://api.deepseek.com/v1`（OpenAI 兼容） |
| 模型 | `deepseek-chat` |
| 密钥 | 只经环境变量 `AITE_MODEL_API_KEY`；`config/aite.yaml` 里只有变量名，且 `.gitignore:14` 不入库 |
| 平台 | `fake`（真模型 + 替身平台，不碰真实飞书） |
| 沙箱 | **两档**：`fake`（场景自带 `exec_script`）与 `docker`（真容器，`aite-edge` 在跑） |
| 超时 | `--timeout-scale` 默认 12.0（`cli.rs:39` 的 `LIVE_TIMEOUT_SCALE`）→ 每场景 `10.0 × 12 = 120` 秒 |
| system prompt | **契约默认值** `core/crates/worker/prompts/platform.md`（3878 字节），由 `agent.rs:305` 按相对 cwd 读 |
| 场景 | 十个全跑（`evals/p0/`，一个字没动） |
| 跑了几遍 | **每档两遍**，外加一遍不花钱的 scripted × docker 对照，外加加分项两遍 |

**config 只有三段进得了这一轨的结论**（三处读取点都核过）：`model:` 全段
（`wiring.rs:130-144`，`--model live` 时整段 clone 进每个场景）、`edge:` 段（`wiring.rs:102-115`，
docker 档连 edge 用）。`worker:` / `storage:` / `sandbox:` / `platform:` / `feishu:` **一律不读** ——
场景配置走 `config_of(sc)`（`deps.rs:267-275`）= 契约默认值 + 场景 yaml 自己的 `config:` 片段，
`platform` 硬写成 `fake`。**所以 config 里的 `worker.system_prompt_path` 影响不到 evals 的结论。**

```bash
# 环境：从 example 复制，只改三处（platform → fake、base_url、model）
cp config/aite.example.yaml config/aite.yaml
( cd edge && go build -o bin/aite-edge ./cmd/aite-edge )
edge/bin/aite-edge --config config/aite.yaml > data/v4/edge.log 2>&1 &

A=core/target/debug/aite
# ① 不花钱的对照：把「真 Gateway/真沙箱带来的红」与「真模型带来的红」分开
$A evals run evals/p0 --sandbox docker --json data/v4/scripted-docker.summary.json

# ② fake 档两遍（与 T19/T23 逐格可比的一档）
for i in 1 2; do
  $A evals run evals/p0 --model live \
     --protocol-report data/v4/fake-$i.json --json data/v4/fake-$i.summary.json
done

# ③ docker 档两遍（两遍之间重启 edge —— 那是目前唯一真能收容器的动作，见第 6 节）
for i in 1 2; do
  $A evals run evals/p0 --model live --sandbox docker \
     --protocol-report data/v4/docker-$i.json --json data/v4/docker-$i.summary.json
done
```

`stdout` 是「一份 JSON 摘要 + 最后一行 `passed k/10`」，**协议摘要走 stderr**（`cli.rs:413-414`）。

### 成本（估算在前，实际在后）

**开跑前的估算**（方法：拿 T23 §四的 Python 全套 × 2 折出单遍 134k input / 5.8k output，
docker 档按 RΩ 实测的 `04` 两档差额 35672 − 20213 = 15459 扣掉）：
四遍约 **506k input / 22.5k output**，全 miss 上界约 **¥1.30**，按 T17 实测 ~91% 命中约 **¥0.39**。

**四遍实测**（从 `--protocol-report` 的 `runs[].steps_detail[].usage` 汇总，`protocol_probe.rs:371-375`）：

| | 步 | input | output | 其中 cached | 命中率 | ¥（三档单价） | ¥（全 miss 上界） |
|---|---|---|---|---|---|---|---|
| fake-1 | 56 | 131,970 | 5,668 | 120,700 | 91.5% | 0.0920 | 0.3093 |
| fake-2 | 56 | 131,177 | 5,181 | 119,420 | 91.0% | 0.0888 | 0.3038 |
| docker-1 | 43 | 99,092 | 4,534 | 89,340 | 90.2% | 0.0736 | 0.2345 |
| docker-2 | 44 | 102,388 | 4,700 | 92,668 | 90.5% | 0.0756 | 0.2424 |
| **四遍合计** | **199** | **464,627** | **20,083** | **422,128** | **90.9%** | **0.3301** | **1.0899** |
| 加分项 × 2（第 6 节） | 14 | 47,224 | 3,048 | 41,984 | 88.9% | 0.0433 | 0.1188 |
| **全部合计** | **213** | **511,851** | **23,131** | **464,112** | **90.7%** | **0.3734** | **1.2087** |

> 单价假设同 T17/T19/T23：cache miss 输入 ¥2/百万、cache hit 输入 ¥0.2/百万、输出 ¥8/百万。
> **这个单价是假设，实际以账单为准**，原始 token 在表里，换单价自己乘。

**估算与实际差多少**：input 估 506k、实 465k（估高 9%）；output 估 22.5k、实 20.1k（估高 12%）。
差的方向是估高，来源是 docker 档实际比估的更省 —— 真沙箱让 `04` 从 19/21 步降到 12/11 步。

**`price_in_per_mtok` / `price_out_per_mtok` 我没填**，理由是实测出来的，不是偏好：
`cost_of`（`core/crates/models/src/lib.rs:267-271`）用**单一** `price_in_per_mtok` 乘**全部**
`input_tokens`，**不扣 `cached_tokens`**。而 DeepSeek 的 cache hit / miss 是两档单价（差 10 倍），
本轮实测命中率 90.7% —— 填 miss 价会把「已用 ¥」高估约 5.5 倍，填 hit 价则低估。
填哪个都是错的，所以按 T19/T23 的做法留 0，钱在上表里自己按三档折。
**不填不影响 `--protocol-report` 里的 token 数**（那三个字段直接来自 API 的 usage）。

> **这条要交总管**：`docs/demo-3min.md:250` 把「单价没填 → 花费全是 ¥0.0000」列为**第四幕的翻车点**。
> 演示时必须填，而上面那条意味着**填了之后卡片上的数就是错的**（按 miss 价高估约 5.5 倍）。
> 演示要的是「花费不是 0」还是「花费是对的」，这是两件事。**没测到**：填价格之后卡片实际显示什么数字。

---

## 3. 每跑的形状

格式 `a/c/f/n` = `checklist_add`/`check`/`fail`/`note` 的计数。

| 场景 | fake-1 | fake-2 | docker-1 | docker-2 |
|---|---|---|---|---|
| `01_simple_qa` | 0/0/0/0 · 1 步 · delivered | 0/0/0/0 · 1 · delivered | 0/0/0/0 · 1 · delivered | 0/0/0/0 · 1 · delivered |
| `02_thread_followup` | 2/0/6/**1** · 13 · delivered ×2 | 2/0/6/**1** · 13 · delivered ×2 | 2/0/5/0 · 9 · delivered ×2 | 2/0/9/0 · 10 · delivered ×2 |
| `03_checklist_progress` | 1/0/4/0 · 5 · delivered | 1/0/4/0 · 5 · delivered | 1/0/4/0 · 4 · delivered | 1/0/5/0 · 5 · delivered |
| `04_csv_to_chart` | 1/**1**/4/0 · 19 · delivered | 1/**1**/4/0 · 21 · delivered | 1/**5**/**0**/0 · 12 · delivered | 1/**5**/**0**/0 · 11 · delivered |
| `05_history_summary` | 1/3/0/0 · 6 · delivered | 1/3/0/0 · 6 · delivered | 1/3/0/0 · 6 · delivered | 1/3/0/0 · 6 · delivered |
| `06_read_document` | 1/3/0/0 · 4 · delivered | **0/0/0/0 · 2** · delivered | 1/3/0/0 · 4 · delivered | 1/2/0/0 · 4 · delivered |
| `07_commands` | 0/0/0/0 · 1 · cancelled | 0/0/0/0 · 1 · cancelled | 0/0/0/0 · 1 · cancelled | 0/0/0/0 · 1 · cancelled |
| `08_step_limit` | 0/0/0/0 · 3 · **failed** | 0/0/0/0 · 3 · **delivered** | 0/0/0/0 · **2** · delivered | 0/0/0/0 · **2** · delivered |
| `09_bot_ignored` | 0/0/0/0 · 0 · 没建任务 | 0/0/0/0 · 0 · 没建任务 | 0/0/0/0 · 0 · 没建任务 | 0/0/0/0 · 0 · 没建任务 |
| `10_duplicate_event` | 1/0/4/0 · 4 · delivered | 1/0/4/0 · 4 · delivered | 1/0/4/0 · 4 · delivered | 1/0/4/0 · 4 · delivered |
| **合计** | 7/7/18/1 = **33** | 6/4/18/1 = **29** | 7/11/13/0 = **31** | 7/10/18/0 = **35** |

### 协议纪律（四遍都干净）

| | fake-1 | fake-2 | docker-1 | docker-2 |
|---|---|---|---|---|
| chat 次数 / 成功 | 56/56 | 56/56 | 43/43 | 44/44 |
| `finish_reason` | 全 `tool_calls` | 全 `tool_calls` | 全 `tool_calls` | 全 `tool_calls` |
| 协议外的工具名 | **0** | **0** | **0** | **0** |
| 参数不合 `ToolSpec.parameters` | **0** | **0** | **0** | **0** |
| `repeat_loops` 原地打转 | 0 | **1**（`04`） | 0 | 0 |
| 模型重试（5xx / 限流） | 0 | 0 | 0 | 0 |
| 只回文本没调工具 | 0 | 0 | 0 | 0 |
| `hit_max_steps` | 无 | 无 | 无 | 无 |
| 场景总耗时（**记录，不是判据**） | 75 s | 74 s | 62 s | 66 s |

**出牌严格守协议**，与 T17（133 次调用 0 个协议外名字）和 RΩ（21 次 0 个）一致。
本轮 199 次调用、0 个协议外的名字、0 条参数不合 schema。**换语言之后这一条没有退化。**

> 报告里 `arguments` 显示成字符串（例如 `"items": "[\"下载 sales.csv\",…]"`）是
> `clipped_arguments()`（`protocol_probe.rs:120-126`）落盘时 `stringify` 的结果，
> **不是模型返回的形态** —— schema 校验（`:349`）吃的是原始 `arguments`，所以 `schema_ok: true` 是对的。
> 核过一次，**不是缺陷**。

### 第一步出的牌

| 场景 | fake-1 | fake-2 | docker-1 | docker-2 |
|---|---|---|---|---|
| `01_simple_qa` | `final` | `final` | `final` | `final` |
| `02_thread_followup` | `checklist_add`+`read_group_history` | 同 | 同 | 同 |
| `03_checklist_progress` | `checklist_add`+`read_group_history` | 同 | 同 | 同 |
| `04_csv_to_chart` | `checklist_add` | `checklist_add` | `checklist_add` | `checklist_add` |
| `05_history_summary` | `checklist_add` | `checklist_add` | `checklist_add` | `checklist_add` |
| `06_read_document` | `checklist_add` | **`read_document`** | `checklist_add` | `checklist_add` |
| `07_commands` | `read_group_history` | 同 | 同 | 同 |
| `08_step_limit` | `read_group_history` | 同 | 同 | 同 |
| `09_bot_ignored` | （模型没被调用） | 同 | 同 | 同 |
| `10_duplicate_event` | `checklist_add`+`read_group_history` | 同 | 同 | 同 |

**40 格里 39 格一致**，唯一抖动的是 `06` 在 fake 第二遍退回到了 T19 **before** 的形态。

### `04_csv_to_chart` 两档的逐步实录（演示第二幕主角）

`--sandbox docker`（docker-1，12 步，delivered）—— **这就是演示要的形态**：

```
#0  checklist_add(下载 sales.csv / 读取并检查数据 / 按月份聚合 / 画月度趋势图 / 保存 PNG)
#1  download_attachment
#2  checklist_check(c1)      ← 卡片内容变化 2
#3  run_python(45 字)
#4  checklist_check(c2)      ← 变化 3
#5  run_python(82 字)
#6  checklist_check(c3)      ← 变化 4
#7  run_python(200 字)
#8  run_python(200 字)
#9  checklist_check(c4)      ← 变化 5
#10 checklist_check(c5)      ← 变化 6
#11 final
```

`--sandbox fake`（fake-1，19 步，delivered）—— 同一个场景，替身沙箱返回空：

```
#0  checklist_add(同样 5 项)
#1  download_attachment
#2  checklist_check(c1) + run_python
#3–#16  run_python × 12、list_files × 3   ← 撞上替身的空返回，反复探测
#17 checklist_fail ×4                      ← 干不成，诚实标 4 项失败
#18 final（如实说明卡在哪）
```

**卡片内容变化次数：docker 档 6 次（连续推进），fake 档 3 次（建 + 勾 1 项 + 一次性 fail 4 项）。**
T19 §4 当时说 fake 档「字面上够了，但观感上只有 3 个瞬间在动，不是连续推进」，
并在 §10 建议 2 里判断「根子在替身沙箱返回空，该修的是真沙箱」。**本轮实测证实了这个判断。**

docker 档两遍的产物：`/work/monthly_trend.png`，**41091 / 40139 字节**，PNG 魔数校验过
（`:72` `magic: "89504e470d0a1a0a"`），`send_file` 1 次，任务 delivered。

---

## 4. 失败逐条是什么

### 五跑的 `passed`

| 跑 | `passed` | 红的是哪几个 |
|---|---|---|
| **scripted × docker**（不花钱的对照） | **9/10** | 只有 `04` |
| fake-1 | **5/10** | `01` `03` `04` `07` `10` ← **与 T23 §三那五条一字不差** |
| fake-2 | **4/10** | 上面五条 + `08` |
| docker-1 | **4/10** | `01` `03` `04` `07` `08` `10` |
| docker-2 | **4/10** | 同 docker-1 |

**`passed k/n` 不是判据**，逐条归类如下。三类：**(a) 耦合台词 / (b) 结构性不可测 / (c) 真缺陷**。

| 场景 | 哪条断言红了 | 类 | 依据 |
|---|---|---|---|
| `01_simple_qa` | `:27 contains 北京今天晴` | **(a)** | 模型拒绝编天气：「沙箱没有网络，也没有接天气数据源」——正是 W9 铁律 4。与 T23 同一句话、同一个理由。**另两条免费的仪表 `:24 send_card equals 0` / `:26 cards distinct_equals 0` 四遍全过** —— 第一步出 `final`，一张卡都没发，出牌形状没变 |
| `03_checklist_progress` | `:48 checklist_check min 3`，实际 **0**（四遍） | **(a)** | **比 T23 更精确的归因**：T23 说「替身沙箱空返回 → 模型用 fail 而不是 check」。本轮 docker 档实测**仍然是 0**，所以根因不是沙箱 —— `03` 的 yaml **压根没有 `platform:` 段**（无附件、无群历史），模型的实录是「群历史为空，无对账上下文 / 无附件可下载 / 无账单数据可核对」。**它缺的是输入数据，换真沙箱救不了。** 而 `:45 cards distinct_equals 1, updates_min 3, final_status delivered` **四遍全过** —— 卡片确实在动，只是勾变成了叉 |
| `04_csv_to_chart` | fake 档：`:71 send_file equals 1` 实际 0；docker 档：`:69 contains "exit_code=0"` | **(a)** | 见第 5 节。docker 档那条**连脚本模型都红**（scripted × docker 实测），与真模型无关 |
| `07_commands` | `:83 release min 1`、`:90 acquire min 1`，实际 0（四遍） | **(a)** | 与 T23 同因：真模型第 1 步 `read_group_history` 之后就被 `!stop` 掐掉，还没用到沙箱。**`:80 contains "#A1"`、`:82 status cancelled`、`:84 cards final_status cancelled`、`:89 cards distinct_equals 1` 四遍全过**。T19 说这个场景「对提示词改动完全不敏感，是个好基准」——本轮四遍步数都是 1、终态都是 cancelled，**基准依然成立** |
| `08_step_limit` | `:30 status failed`、`:32 contains 上限`、`:34 cards final_status failed`；docker 档还多一条 `:29 model_calls min 3` 实际 2 | **(b)** | **四遍出了三种形态**：failed(3步) / delivered(3步) / delivered(2步) / delivered(2步)。yaml `:21-26` 用 `repeat: inf` 的退化脚本人为造死循环，真模型不会这么打 —— 它读不到群历史就直接请用户补充材料，一步到位。`inventory-gateway-evals.md:85` 记的「`08_step_limit` 在 live 下不稳定」**本轮四遍复现**。**live 下这个场景验不了它想验的东西** |
| `10_duplicate_event` | `:30 model_calls equals 1`，实际 **4**（四遍） | **(a)** | 与 T23 同因：脚本一步答完，真模型走 4 步。**它真正要验的那条 `:27 store tasks_equals 1, sessions_equals 1`（去重）四遍全过** —— 去重本身在 live 下照样有效 |

**(c) 真缺陷：0 条。**

### 金丝雀

**`09_bot_ignored` 四遍全绿**（`outbound_total equals 0`、`model_calls equals 0`、
`store sessions_equals 0, tasks_equals 0`、`add_reaction equals 0`）。
它是唯一一个 live 与 scripted 走同一条路的场景，四遍零偏差。

### 那条不花钱的对照，是「这条红跟真模型无关」的直接证据

```
$ core/target/debug/aite evals run evals/p0 --sandbox docker
passed 9/10
红: 04_csv_to_chart
   run_python 的结果里没有 "exit_code=0"；实际："执行成功（839 ms）\n\nstdout: （空）\n\n
   /work 下本次新增/修改的文件：\n  /work/out.png（22183 字节）"
```

**Rust 侧这一格此前没人跑过**，实测是 **9/10**，与 Python 侧（T23 §五第 1 条）相同，
**红的就是那唯一一条**。这条对照一分钱没花，却证明了：`04` 在 docker 档那条红
**与真模型完全无关**，脚本模型照样红。

### `04` 那条耦合的处置建议（⑤）

病在 `evals/p0/04_csv_to_chart.yaml:69`：`contains: "exit_code=0"` 断的是
`FakeToolGateway` **自己的排版**（`fake_gateway.rs:295-298`），而真 `P0ToolGateway`
排的是「执行成功（N ms）」（`python_exec.rs:84`）。我复核了两件事，都成立：

1. 真 Gateway 的排版与 Python 版**逐字一致** —— `python_exec.rs:84` 的
   `format!("执行成功（{} ms）", …)` 对 `d8bf391:aite/tools/python_exec.py:78` 的
   `f"执行成功（{result.duration_ms} ms）"`，连失败分支的「代码以退出码 N 结束」也逐字同形；
2. `core/crates/testing/tests/fake_gateway.rs:314-315` 确实有一条逐字断言钉着替身的排版，
   注释里就写着「04 的断言耦合它」。

**我建议走 D（不动，记成已知项），并且跑完之后这个倾向更强了，不是更弱。**
派单预设「如果发现别的场景也有同类耦合，路 A 的性价比可能反而上来」——**实测下来没有上来**，理由：

- (a) 类耦合本轮实测到 **5 条**：`01`（台词）、`03`（缺输入数据）、`04`（排版 + send_file）、
  `07`（沙箱时序）、`10`（步数）。但**只有 `04` 那一条在 scripted 档下也会红**，
  其余四条是 **live 专有**的。改 `04` 那一条**一条 live 脚注都省不掉**，
  只把 scripted × docker 从 9/10 变成 10/10；
- docker 档的真判据（spec §6.1 定的 `:74` delivered + `:72` PNG 魔数）**四遍全过**；
- `evals/p0/**` 是 §3.8 的验收面，而 B8 的 `passed 10/10` 现在是 `check.sh:43-45` 的硬门禁
  （退出码 0 **且**最后一行逐字 `passed 10/10`，两条都判），动它要重证；
- 这条耦合在 Python 时代就红（T23 §五第 1 条），不是本轮回归。

**我一个字都没改 `evals/p0/`**（`git diff --stat -- evals/p0/` 为空）。

---

## 5. 与 Python 版三份报告的对照

### 5.1 更正 RΩ §4 那一行：checklist 那条不是「翻转」，是比错了基线

RΩ §4 写的是「**与 Python 版结论相反**」，并说「中间隔着换语言、换提示词渲染、模型本身的
版本漂移，本轮没有做对照实验，不敢归因」。**三个混淆项里，前两个已经可以排掉。**

**证据 1 —— 提示词逐字节相同，连 sha256 都一样。**

```
$ diff <(git show d8bf391:aite/worker/prompts/platform.md) core/crates/worker/prompts/platform.md
（无输出）
$ git show d8bf391:aite/worker/prompts/platform.md | shasum -a 256
0d2ecfe8283d5984a39a6540e526587f89e2adefaa1839736b3a77efa2f19e09  -
$ shasum -a 256 core/crates/worker/prompts/platform.md
0d2ecfe8283d5984a39a6540e526587f89e2adefaa1839736b3a77efa2f19e09  core/crates/worker/prompts/platform.md
```

`d8bf391` 是删 Python 树前的最后一个提交，两边都是 **3878 字节**。
`live-report-2026-09-10-t19.md:92` 原话「文件 **2246 → 3878 字节**」——
**Rust 用的就是 T19 改完之后那份提示词，一个字都没动。**

**证据 2 —— checklist 四个工具的描述与 schema 也逐字相同。**
`core/crates/contracts/src/protocol.rs:44-67` 的 `CHECKLIST_TOOLS` 与
`d8bf391:aite/contracts/protocol.py:24-31` 逐字对得上，包括 T19 §1④ 专门点名的那句劝阻性描述
「添加待办项，**仅在**任务开始或发现新步骤时调用；每项 ≤20 字」，四条的 `parameters` 也一样。

**证据 3 —— RΩ 引的是 T17 的数，而 T17 的基线在 T19 改提示词之前。** 这条我用字节数钉死了：

| 提交 | 是谁的基线 | 当时 `platform.md` 的字节数 |
|---|---|---|
| `0d6939c` | **T17**（RΩ §4 引的那份） | **2246** ← 改之前 |
| `6ee30d4` | T19（改之前的基线） | 2246 |
| `a3dcd04` | T19 改提示词那一笔 | → 3878 |
| `0960272` | T23（Python 侧最终态） | **3878** ← 改之后 |
| `8d6ffd3` | **本轨** | **3878** |

`git merge-base --is-ancestor 0d6939c 6ee30d4` 成立。Python 侧的**最终态**是 T19 之后：
`checklist_*` 0 → **31 次**，覆盖 6 个场景，after 两遍一模一样。
`review/inventory-gateway-evals.md:85` 那条一行总账写得更直白：
「曾 **0 次**用 checklist → **T19 改提示词后 31 次**」。

**所以这不是翻转，是拿 T17 的数去比 T19 之后的实现。**
（更正写在这里，RΩ 那份已合入的报告一个字没动。）

### 5.2 T19 §3 那张表的 Rust 列

| 场景 | T19 after（跑1/跑2） | V4 fake（跑1/跑2） | 差在哪 |
|---|---|---|---|
| `01_simple_qa` | 0/0 | 0/0 | 一致 |
| `02_thread_followup` | 8/8 | **9/9** | 多 1，多出来的正是 `checklist_note` |
| `03_checklist_progress` | 5/5 | 5/5 | 一致 |
| `04_csv_to_chart` | 6/6 | 6/6 | 一致 |
| `05_history_summary` | 4/4 | 4/4 | 一致 |
| `06_read_document` | 3/3 | **4/0** | **跑2 没建** —— 退回 T19 before 的形态 |
| `07_commands` | 0/0 | 0/0 | 一致 |
| `08_step_limit` | 0/0 | 0/0 | 一致 |
| `09_bot_ignored` | 0/0 | 0/0 | 一致 |
| `10_duplicate_event` | 5/5 | 5/5 | 一致 |
| **合计** | **31/31** | **33/29** | |

**按工具名细分**：

| | `add` | `check` | `fail` | `note` | 合计 |
|---|---|---|---|---|---|
| T19 after（两遍都是） | 7 | 6 | **18** | **0** | 31 |
| V4 fake-1 | 7 | 7 | **18** | **1** | 33 |
| V4 fake-2 | 6 | 4 | **18** | **1** | 29 |
| V4 docker-1 | 7 | 11 | 13 | 0 | 31 |
| V4 docker-2 | 7 | 10 | 18 | 0 | 35 |

**`checklist_fail` 那一格：T19 是 18，V4 fake 两遍也都是 18 —— 逐字相同。**

**`checklist_note` 那一格**（派单点名「真正没对上的地方」）：Python 侧 T19 两遍都是 **0**；
V4 fake 档两遍都是 **1**，两遍都落在同一个场景同一个位置 —— `02_thread_followup` run0 第 #4 步：

```
#3  checklist_fail(c1, 群历史为空，未找到任何消息或数据来源), checklist_fail(c2, 没有可下载的附件)
#4  checklist_note(无数据源，后续步骤无法进行)      ← 这一格
#5  final
```

用法完全合规（「在卡片上写一条 ≤40 字的备注，不新增消息」，本例 14 字）：
标完两项 fail 之后写一条总的卡点备注，然后 final。**docker 档两遍都是 0。**
**为什么 Python 侧一次没用过、Rust 侧稳定用 1 次，没测到**（只能说明它不是「从不用」）。

**`fail : check` 的比例**（T19 §3 自己说「真沙箱下这个比例会变成什么样，没测到」——本轮回答它）：

| | `fail : check` | 比值 |
|---|---|---|
| T19 after（fake 档） | 18 : 6 | 3.0 : 1 |
| **V4 fake 档**（两遍合计） | 36 : 11 | **3.3 : 1** |
| **V4 docker 档**（两遍合计） | 31 : 21 | **1.5 : 1** |
| ↑ 其中只看 `04_csv_to_chart` | fake 8:2 → **docker 0:10** | **完全翻转** |

**真沙箱把比例从 ~3:1 拉到 ~1.5:1**，而变化几乎全部来自 `04` 一个场景 ——
它是十个场景里唯一真正用得上沙箱的那个（`07` 一步就被 `!stop`）。

### 5.3 `03_checklist_progress` 在 docker 档下是真勾还是叉

**仍然是叉。** 四遍 `checklist_check` 都是 0，`:48 min 3` 四遍都红。

但**根因不是沙箱** —— `03` 的 yaml 没有 `platform:` 段，模型拿不到任何输入。实录（docker-1）：

```
#0  checklist_add(读群历史找对账上下文 / 下载对账相关附件 / 核对账单数据 / 生成对账报告)
    + read_group_history
#1  list_files
#2  checklist_fail(c1, 群历史为空，无对账上下文), checklist_fail(c2, 无附件可下载),
    checklist_fail(c3, 无账单数据可核对), checklist_fail(c4, 无数据，无法生成报告)
#3  final
```

**这是对 T23 §三那条归因的一处精化**：T23 把它归给「替身沙箱空返回」，
本轮真沙箱实测证明换沙箱救不了它。**演示第二幕该看的不是 `03`，是 `04`** ——
`04` 在真沙箱下才给出连续推进的卡片（见第 3 节）。

### 5.4 与 T23 的成本对照

| | input | output |
|---|---|---|
| Python 侧全套 × 2（live × fake，T23 §四） | 268,657 | 11,544 |
| **Rust 侧全套 × 2（live × fake，本轮）** | **263,147** | **10,849** |
| 差 | −2.1% | −6.0% |

### 5.5 checklist 稳定性的明确结论

**稳定用。** 判据分两层，两层都给数：

- **「用不用」这一层：稳定。** 10 场景 × 4 遍 = 40 格里 **39 格一致**。
  四遍都不建的是 `01`（一步 `final`）、`07`（被 `!stop`）、`08`（题面模糊，模型在弄清任务是什么）、
  `09`（模型没被调用）—— 全是 T19 §6 已经解释过的正确行为。
  四遍都建的是 `02`/`03`/`04`/`05`/`10`。唯一抖动：`06_read_document` 在 fake 第二遍没建
  （直接 `read_document` 两步收工，答案正确、delivered）。
  按 T19 §6 的口径，严格遵守率四遍分别是 **6/8、5/8、6/8、6/8**。
- **「用几次」这一层：不稳定，且这是预期内的。** 同一场景两遍之间 `fail` / `check` 的具体计数会变
  （`02` docker 两遍 5 vs 9、`03` 4 vs 5、`06` 3 vs 2），因为模型每次写的代码不同、探测步数不同。
  **计数抖动不影响卡片会不会动**，只影响动几次。

---

### 5.6 顺带留下的一份现场：`cargo passed=717 failed=1` 那条假红

`review-findings-2026-09-12-romega.md` 第五节末尾记着一条未复现的现象：合并后在 main 上复验时
出过一次 `717 passed / 1 failed`，紧接着连跑六遍都是 `718 / 0`，**但失败的测试名没留下**。

**本轨收尾复验时撞到了同一条，这次现场留住了**（`check.sh` 在 `0bc8d55` 改成「失败测试名在前、
计数在后」之后的第一次真正兑现）：

```
$ scripts/check.sh          # 收尾复验，第二次
error: test failed, to rerun pass `-p aite --test startup_recovery`
cargo passed=717 failed=1
以下没过：B 全量 cargo test

$ cd core && cargo test -p aite --test startup_recovery    # 单独连跑三遍
test result: ok. 9 passed; 0 failed; 0 ignored    （三遍都是这个数）

$ scripts/check.sh          # 再整轮跑一次
cargo passed=718 failed=0 · contracts passed=25 failed=0 · OK 25 files · passed 10/10
全部通过
```

**是并行挤的，不是回归**：开场自检那一轮是 `718 / 0`，本轨从头到尾一行代码没改
（交付物只有一个 `.md`）。`startup_recovery` 那 9 条测试全是起飞/孤儿回收的时序面，
六轨抢 CPU 时抖得动。**失败的那一条具体是 9 条里的哪一条仍然没抓到** ——
`check.sh` 的 grep 只留了 target 名，没留 `test xxx ... FAILED` 那行。要再往前一步的话，
得让它把 `FAILED` 行也打出来，**本轨没改 `check.sh`**（不是这一轨的文件）。

## 6. 没测到的

诚实清单。别把「没测到」读成「测过了没问题」。

1. **真实飞书** —— 四遍的 platform 都是 `fake`。M1–M6 才碰真机。
2. **模型版本漂移** —— `deepseek-chat` 是滚动别名，9-10 到 9-12 之间可能已经换过权重。
   **这个查不了，没测到，不敢归因。** 它是 §5.1 那三个混淆项里唯一没排掉的一个。
3. **提示词渲染这个混淆项 —— 查清了，不是「没测到」。**
   `core/crates/worker/src/context.rs` 与 `d8bf391:aite/worker/context.py` 逐项对齐：
   顺序写死为 `system(platform.md) → transcript → 群历史 → 附件清单`（`context.rs:128-140` 对 `context.py:80-85`，
   system prompt 都摆在**第一条**）、角色映射一致、两个 header 常量逐字相同、
   截断参数同为 40/2/30、history 行格式同为 `[message_id] 姓名: 文本`、附件行格式一致。
   **但实录层面没法逐格对**：T17/T19 报告里没有记 `n_messages`（grep 过，一处都没有），
   所以「同一份提示词在两边摆出来的消息条数是否相同」**没测到**，只有源码对拍这一层证据。
4. **`checklist_note` 为什么 Python 侧 0 次、Rust 侧稳定 1 次** —— 本轮只观察到它在 `02` 被用、
   两遍位置相同，**为什么没测到**。
5. **`cost_of` 填价格之后卡片上显示什么** —— 见第 2 节末尾那条给总管的。**没测到。**
6. **`--sandbox docker` 档的容器清理** —— `edge-client/src/sandbox.rs:138-140` 的
   `close_all` 是 `Ok(())` 空操作，`SandboxProbe::close()`（`real_stack.rs:179-181`）什么也没收；
   真正的 `CloseAll`（`edge/internal/sandbox/docker.go:468`）只在 `main.go:202` 被调。
   **本轮两遍之间重启了 edge，所以没撞上残留**（每次停机 `edge.down` 之后 `docker ps -a --filter
   label=aite.task` 都是 0）。**不重启会残留多少，没测到。**（这是 `review-findings…romega.md`
   §4.1 `wiring.rs:153` 那条已知项，不是新发现。）
7. **只跑了一遍的东西** —— scripted × docker 那条对照**只跑了一遍，没测到第二遍**。
8. **`08_step_limit` 在 live 下验不了它想验的东西** —— 四遍出了三种形态。
   要不要给它换一个 live 也成立的判据，交总管。

### 加分项：演示第二幕那句台词（⑧）—— 做了，两遍都是能用的形态

演示要发的那句（`docs/demo-3min.md:334`）比场景文案多两个动作，附件也不是同一份
（场景用 `samples.rs:105` 的 48 字节 3 行 CSV，演示用 `aite evals demo-fixture all` 生成的
24 个月那份，364 字节）。**把 `04` 复制到 `evals/` 之外**（`load_suite` 收任意目录，
`scenario.rs:509-526`），只改三处：`events[0].text` 换成演示那句、`platform.files[0].content`
换成真 fixture、`attachments[0].size` 48 → 364（比派单多改的一处：附件清单是渲染给模型看的，
写错大小会误导），然后 `--model live --sandbox docker` 跑两遍。

**判据只有一个，不看 `passed`：第一步是不是 `final`。**

| | demo-1 | demo-2 |
|---|---|---|
| **第一步出的牌** | **`checklist_add`（6 项）** | **`checklist_add`（6 项）** |
| `checklist_*` 计数 | 1 add + 6 check = **7** | 1 add + 6 check = **7** |
| 步数 / 终态 | 6 / delivered | 8 / delivered |
| 卡片内容变化次数 | 4 | 5 |
| 产物 | `sales_monthly_trend.png` 120,542 字节 | `monthly_trend.png` 124,597 字节 |
| 极值读对了吗 | 最高 2025-11 **140,494**、最低 2025-07 **60,700** ✅ | 同 ✅ |

两遍建的 checklist 逐字相同：`下载 sales.csv / 读取并检查数据 / 按月份聚合 / 画月度趋势图 /
标注最高/最低月 / 保存 PNG 并给结论` —— **台词里那三个动作确实变成了 checklist 项**，
demo 文档 `:336` 那句「三个动作是故意的」得到实测支撑。

**结论：演示那句台词是能用的形态，两遍都是。** `docs/demo-3min.md:249` 列为全片最致命的翻车
（「模型一步就 `final`、卡片压根没出现」）**两遍都没发生**。

**边界**：这条只做观测，没给改法，也没改任何东西。两遍的 `passed 0/1` 红的仍是
`exit_code=0` 那条耦合（对照套件继承了 `04` 的 expect），与本条结论无关。

> **只跑了两遍。** 演示前的彩排（`docs/demo-3min.md:130-140`）照做，这两遍不能替代它 ——
> 彩排跑的是真飞书 + `aite run`，本轮跑的是 evals + `platform: fake`。
