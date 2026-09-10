# 真模型实测报告 —— `--model live` 第一次真的跑起来

> 2026-09-10 · T17（T12 段 B）· 基线 `0d6939c`
> 装置：`aite/evals/protocol_probe.py`（`--protocol-report`）
> 这份报告里的每一个数都来自实跑。推测一律标成「推测」，没测到的一律写「没测到」。

## 一句话

**DeepSeek 出牌严格守协议**——133 次工具调用，0 个协议外的名字，0 条参数不合
`ToolSpec.parameters`，`finish_reason` 全是 `tool_calls`。**但它一次都没用过 checklist
工具**，而 3 分钟演示的主角正是那张会原地更新的 checklist 卡片。另有一种出牌方式
（工具成功返回但内容为空 → 原地重复同一个调用）现有兜底一条都接不住，实测连发
**33 次逐字节相同的 `run_python`**，一路烧满 `max_steps` 才停。

## 怎么跑的

| | |
|---|---|
| 端点 | `https://api.deepseek.com/v1`（OpenAI 兼容） |
| 模型 | `deepseek-chat` |
| 密钥 | 复用本机 `MAOS_LLM_API_KEY`（总管当场授权），经 `AITE_MODEL_API_KEY` 传入；`config/aite.yaml` 里只有变量名，没有取值，且不入库 |
| 平台 | `fake`（真模型 + 替身平台，不碰真实飞书） |
| 超时 | `--timeout-scale` 默认 12.0（live） |
| 跑了几遍 | **两遍**，为的是看真模型稳不稳 |

```bash
export AITE_MODEL_API_KEY=…
python -m aite.evals run evals/p0 --platform fake --model live \
    --protocol-report proto.json --json live.json
```

**`passed k/10` 不是判据**（跑1 是 5/10，跑2 是 4/10）。场景的 `expect` 照
`model_script` 的台词写，真模型复现不了那些句子。要看的是下面这些。

## 1. 走到 final 的有几个

| 场景 | 跑1 调用/final/终态 | 跑2 调用/final/终态 | 读法 |
|---|---|---|---|
| `01_simple_qa` | 1 / 1 / delivered | 1 / 1 / delivered | 第一步直接 `final`，跟 scripted 同形 |
| `02_thread_followup` | 11 / 2 / delivered×2 | 9 / 2 / delivered×2 | 两个 task 各自收敛，多绕了几步 |
| `03_checklist_progress` | 4 / 1 / delivered | 4 / 1 / delivered | 交付了，但**没建 checklist** |
| `04_csv_to_chart` | 40 / 0 / **failed** | 40 / 0 / **failed** | 撞 `max_steps`，见 §4 缺口 2 |
| `05_history_summary` | 2 / 1 / delivered | 2 / 1 / delivered | 读历史 → 交付，最干净的一条 |
| `06_read_document` | 2 / 1 / delivered | 2 / 1 / delivered | 同上 |
| `07_commands` | 1 / 0 / cancelled | 1 / 0 / cancelled | `!stop` 场景，cancelled 是对的 |
| `08_step_limit` | 3 / 0 / **failed** | 3 / **1** / delivered | 两次跑出了两种结果，见下 |
| `09_bot_ignored` | 0 / 0 / （没建任务） | 0 / 0 / （没建任务） | 模型一次都没被调用，R1 生效 |
| `10_duplicate_event` | 4 / 1 / delivered | 3 / 1 / delivered | 只建一个 task，R2 生效 |

**走到 final：跑1 有 7 次 final（6 个场景），跑2 有 8 次 final（7 个场景）。**

**跑飞的只有 `04_csv_to_chart`，两次都飞，都是撞 `max_steps` failed。**

怎么飞的（跑2，逐步实录）：

```
#0  download_attachment(file_key)      → 已下载到 /work/in/file_sales_csv（48 字节）
#1  run_python  print(open(...).read()) → exit_code=0，stdout 空
#2  run_python  查大小 + repr(读二进制) → exit_code=0，stdout 空
#3  run_python  os.walk 列整个 /work    → exit_code=0，stdout 空
#4  模型自己说：The sandbox is returning empty output for every command
    run_python  print("hello")          → 还是空
#5–#6  继续换招探测（写文件、list_files）
#7–#39 对同一段代码连发 33 次，逐字节相同 → 撞 max_steps → failed
```

**直接原因是评测替身**：`evals/p0/04_csv_to_chart.yaml` 的 `sandbox.exec_script`
只有一条规则 `match: savefig`，**任何不含 `savefig` 的代码都返回 `exit_code=0` + 空
stdout**。scripted 路径下 FakeModel 第 3 步就直接交含 `savefig` 的代码，正好命中；
真模型的第一反应是先读文件看看内容——一个完全正常的工程习惯——于是掉进空返回里。

**但底下压着一个真问题**：真沙箱里一个空 CSV、一个编码读不出来的文件，会让真模型
走进同一个死循环，而现在没有任何兜底接得住（§4 缺口 2）。

### `08_step_limit` 两次跑出两种结果

`max_steps: 3`。跑1 第 3 步回纯文本 → 走 §3.3 的 nudge 分支 → 撞上限 `failed`；
跑2 第 3 步调了 `final` → `delivered`（`steps == max_steps` 但正常交付）。
场景本来验的是「死循环撞上限」，真模型两次都没死循环，一次踩线过、一次踩线挂。
**这个场景在 live 下测不出它想测的东西**，属于预期内（判据是照 scripted 写的）。

## 2. 它调出来的牌

跑2 全量（跑1 分布几乎一致）：

```
run_python 38 · read_group_history 14 · final 8 · list_files 7
download_attachment 1 · read_document 1
checklist_add 0 · checklist_check 0 · checklist_fail 0 · checklist_note 0
```

| 检查项 | 结果 |
|---|---|
| 协议外的工具名 | **无**。两次跑 133 次调用，全部命中 `ALL_MODEL_TOOLS` |
| 参数合不合 `ToolSpec.parameters` | **全合**。`schema_violations` 两次都是空 |
| `finish_reason` | 65/65 全是 `tool_calls`（跑2）；跑1 有 1 次 `stop`（那次是纯文本兜底） |
| `checklist` 每项 ≤20 字（W9） | **没测到** —— `checklist_add` 一次都没被调用 |
| `final.artifacts` 路径合不合规 | **没测到** —— 8 次 `final` 全都只带 `reply`，一次都没带 `artifacts`；唯一会产出文件的 `04` 没走到 final |

后两行是这份报告里最需要被看见的「没测到」：**W9 的 20 字约束和 W5 的产物回传，
在真模型上一次都没被验证过。**

## 3. §3.3 兜底逐条

| §3.3 那一行 | 触发了吗 | 行为对不对 |
|---|---|---|
| 模型调用异常 / 5xx → 重试 2 次（2s / 5s），仍失败 → `failed` + 回帖 | **没触发**（DeepSeek 133 次调用零失败，`model_retry` 为空） | 没机会验。但**无 key 时被误触发过**，见缺口 3 |
| 参数不合 schema → `invalid_args`，连续 3 次 → `failed` | **没触发**（`invalid_args` 计数两次都是 0） | 没机会验 —— 模型压根没给过不合法参数 |
| 既无 tool_call 也无 final → `steps==0` 视为 final，`steps>0` 回 system 提示 | **触发了 1 次**（跑1 的 `08`，第 3 步 `steps>0`） | **对**：走 nudge 分支，没有被误兜成 final，任务以「已达步数上限」failed |
| 超 `max_steps` | **触发了**（`04` 两次、`08` 跑1 一次） | **对**：task `failed`，回帖「任务 #A1：已达步数上限，请缩小任务或 !new 重开」 |

前两条**没触发不等于没问题**，只等于这一次没测到。DeepSeek 的 tool_calling 质量把它们
全绕过去了；换一个模型（尤其是小参数量的）大概率能验到 —— 这是**推测**，不是实测。

## 4. 兜底缺口

### 缺口 1 · 真模型完全不用 checklist 工具（演示直接受影响）

**现象**：两次跑、133 次工具调用，`checklist_add` / `checklist_check` / `checklist_fail` /
`checklist_note` **一次都没出现**。`03_checklist_progress` 这个专门验进度面的场景，
真模型 4 步就 `final` 了，全程没建过一项 checklist。

**后果**：W3 会因为出现了非 `final` 的工具调用而 `send_card`，所以卡片**会**发出来，
但**卡片上一项内容都没有**，也不会因为 checklist 变化而 `update_card`。§2.4 的 M3
要求「过程中卡片至少更新 3 次」——**真模型跑演示，这条现在过不了**。

**该改哪个文件**：`aite/worker/prompts/platform.md`（系统提示词）。
**这是 T14 的地盘（`aite/worker/**`），本轨没动。**

**建议修法**（按代价从低到高）：
1. 提示词里把 checklist 从「可用工具」提到**硬要求**：凡是需要 2 步以上的任务，
   第一步必须 `checklist_add`；给一个正例和一个反例。当前 `platform.md` 只在 W9
   那几条铁律里提了「每项 ≤20 字」，没说「必须先建」。
2. worker 侧加一条软约束：第一次出现非 `final` 工具调用、且 `task.checklist` 为空时，
   回一条 system 提示「先用 checklist_add 列出你的步骤」，计 1 步。
3. 如果演示必须稳，退回脚本化替身（见 §5）。

### 缺口 2 · 工具「成功但空返回」→ 原地打转，没有任何兜底

**现象**：`04` 里模型对逐字节相同的 `run_python` **连发 33 次**（跑2，步 7–39），
每次工具都 `ok=True`，只是 stdout 是空的。烧掉 33 次模型调用 + 33 次沙箱执行。

**为什么现有兜底接不住**：§3.3 的两条计数兜底都要求工具**失败**——`invalid_args`
要参数不合 schema，`sandbox` 要沙箱报错。这里工具**成功**了。唯一接住它的是
`max_steps`，代价是烧满整整 40 步。

**该改哪个文件**：`aite/worker/loop.py`。**这是 T14 的地盘，本轨没动。**

**建议修法**：在 `_RunContext` 里记一个 `(tool_name, canonical_json(arguments))` 的
连续重复计数（跟现有的 `invalid_args` / `sandbox_errors` 并排）：

- 连续第 3 次完全相同 → 回一条 system 提示，例如「这个调用你已经原样重复 3 次、
  结果相同，换个思路，或调 `final` 说明你卡在哪」，计 1 步；
- 连续第 5 次 → `_fail`，回帖「模型在同一个调用上原地打转，已终止」。

阈值取 3 有实测依据：正常探测里连着两次相同调用是常态（`03` 里 `list_files()`
就连发过 2 次，无害），3 次起才是真卡住。

### 缺口 3 · 配置缺失被当成模型故障重试（本轨已修一半）

**现象**：`base_url` 或密钥环境变量没配时，`ModelConfigError` 一路穿到 worker 的
`_chat`，被 §3.3 的 `except Exception` 当成模型 5xx，**白重试 2 次（2s + 5s）**，
最后给用户一句「模型服务暂不可用，任务 #A1 已终止」。真正的原因（yaml 没填 /
环境变量没设）一个字都看不到，10 个场景就是 10 次 7 秒空等。

**本轨修了 evals 入口这一半**（`aite/evals/__main__.py`，在可写路径内）：
`_live_model_factory` 起飞前先建一次客户端验配置，缺什么当场说什么并退 2——
也就是派单里描述的、但**此前实际做不到**的那个行为：

```
$ python -m aite.evals run evals/p0 --model live      # base_url 空
--model live 起不来：ModelConfigError: ModelConfig.base_url 是空的：填百炼 / 智谱的 OpenAI 兼容端点
$ echo $?
2
```

**另一半在 `aite/worker/loop.py`，是 T14 的地盘，本轨没动。**
**建议修法**：`_chat` 的重试不该对所有 `Exception` 一视同仁。`ModelConfigError`
这类配置错误重试多少次都不可能成功，应该直接穿透到 `_fail`，并且回帖里说清是配置
问题而不是「模型服务暂不可用」。可重试的是超时、连接错误、5xx。

### 缺口 4 · `cost_of()` 忽略 `cached_tokens`，卡片上的「已用 ¥」高估约 3.8 倍

**现象**：实测 prompt cache 命中率 **92%**（跑2：170678 输入 token 里 157824 是
cached）。`aite/models/openai_compat.py` 的 `cost_of()` 对全部 `input_tokens` 按
`price_in_per_mtok` 计价，不区分 cached——按 DeepSeek 公示价折算，卡片会显示
¥0.385 而实际约 ¥0.10。

**为什么本轨没改**：修它要给 `ModelConfig` 加一个 `price_cached_per_mtok` 字段，
而 `ModelConfig` 在 `aite/contracts/config.py`——**冻结契约，只读，改了任务失败**
（§3.4 / C2）。契约变更要走 T0。

**建议**：P0 演示阶段这个高估无害（数量级都对），**建议不改**；真要做预算功能
（P1 的三级预算）时再连同契约一起动。

## 5. 成本与时长

跑一遍完整 10 个场景（跑2 实测）：

| | |
|---|---|
| 时长 | **63 秒**（跑1 是 77 秒） |
| 模型调用 | 65 次 |
| 输入 token | 170,678（其中 **cached 157,824**，cache miss 12,854） |
| 输出 token | 5,412 |
| 单次调用耗时 | 0.5–2.8 秒（实测 `elapsed_ms` 区间） |

**钱**：按 DeepSeek 公示价折算约 **¥0.10 / 全套**。

> 单价假设写在这里，方便自己核账：cache miss 输入 ¥2/百万、cache hit 输入 ¥0.2/百万、
> 输出 ¥8/百万。**这个单价是假设，实际以账单为准**——原始 token 数在上表里，
> 换个单价自己乘就行。两次跑合计约 ¥0.20。

### 演示该用真模型还是脚本化替身

**建议：3 分钟演示（§1 / M1–M6）用真模型，但先补缺口 1。**

理由：

- **成本和速度都不是问题**：一毛钱、一分钟，比预期便宜得多，cache 命中率还高达 92%。
- **协议面已经稳了**：0 协议外工具名、0 schema 违规、`finish_reason` 全是 `tool_calls`。
  国内模型的 tool_calling 跟脚本化替身的差距，比开跑前担心的小得多。
- **但 checklist 卡片现在是空壳**——而那正是演示要给人看的东西。缺口 1 不补，
  真模型演示看不到进度面原地更新，M3 的「卡片至少更新 3 次」过不了。
- **04 那条路（附件 → 画图 → 回传文件）在真模型下还没走通过一次**。M3 恰好就是这条。
  演示前必须在真沙箱上单独验一遍——评测替身的空返回问题在真沙箱上不存在，
  但缺口 2 的死循环风险还在。

**取舍**：如果补缺口 1 来不及，就用脚本化替身录演示，**但要在旁白里说明这是脚本化
回放**，别让它看起来像真模型跑的。

## 6. 本轨改了什么

全部在可写路径内。**没有动 `aite/contracts/**`、`.contracts.lock`、
`docs/dev-spec-2026-09-09.md`，也没有动 `evals/p0/*.yaml` 里任何一条 `expect`。**

| 文件 | 改动 | 为什么 |
|---|---|---|
| `aite/evals/__main__.py` | `--model live` 起飞前验配置，缺就退 2 | 缺口 3 的一半 |
| `aite/evals/protocol_probe.py` | 修 `_fallback_text_only` 误判 | 撞 `max_steps` 时没有下一次调用，旧判据（看下一步 delta 有没有 system）把 nudge 误报成「兜成 final」。实测 `08` 撞到 |
| `aite/evals/protocol_probe.py` | 新增 `repeat_loops` 观测项 | 缺口 2 全靠肉眼在 40 行 `run_python(code)` 里数。现在直接打一行「⚠ 原地打转：连着 33 次一模一样的 run_python」 |
| `aite/evals/protocol_probe.py` | digest 里 `hit_max_steps` 带上终态 | `steps == max_steps` 跟「撞上限失败」不是一回事，`08` 跑2 就是步数用满却正常 delivered |
| `tests/e2e/test_t12_protocol_probe.py` | +9 条 | 上面每条改动都配了测试；`test_cli_live_defaults_to_a_bigger_timeout_scale` 因为 fail-fast 改了配置前提 |

`--model scripted` 的 `passed 10/10` 不变（B8 没被动过）。

## 附 · 这次没测到的

诚实清单，别把「没测到」读成「测过了没问题」：

1. **W9 的 checklist 每项 ≤20 字** —— 模型没调过 checklist 工具
2. **W5 的 `final.artifacts` → `get_file` → `send_file`** —— 8 次 final 没有一次带 artifacts
3. **§3.3 模型 5xx 重试** —— DeepSeek 133 次调用零失败
4. **§3.3 参数不合 schema 连续 3 次 failed** —— 模型没给过不合法参数
5. **真沙箱**（这轮全程 `--platform fake` + 替身沙箱）
6. **别的模型** —— 只测了 `deepseek-chat` 一个。百炼 / 智谱 / 更小的模型会不会守协议，
   这份报告一个字都没法回答
