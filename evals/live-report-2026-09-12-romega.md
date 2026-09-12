# 真模型实测报告 —— Rust + Go 版第一次 `--model live`

> 2026-09-12 · RΩ（组装起飞）· 基线 `d8bf391`
> 装置：`core/crates/evals/src/protocol_probe.rs`（`--protocol-report`）
> 这份报告里的每一个数都来自实跑。推测一律标成「推测」，没测到的一律写「没测到」。
> 前三份（`live-report-2026-09-10*.md`）是 Python 版的，判据与读法沿用它们。

## 一句话

**真模型 × 真容器这条端到端跑通了**：DeepSeek 7 步走到 `final(artifacts)`，代码在真沙箱
里用 matplotlib 画出 40139 字节的 PNG，产物经 `send_file` 回到话题，任务落 `delivered`，
容器收干净（`docker ps -a --filter label=aite.task` 为 0）。
**`passed k/n` 不是判据** —— 两跑都是 `passed 0/1`，原因全在下面第 3 节，一条都不是实现的问题。

## 怎么跑的

| | |
|---|---|
| 端点 | `https://api.deepseek.com/v1`（OpenAI 兼容） |
| 模型 | `deepseek-chat` |
| 密钥 | 复用本机 `AITE_MODEL_API_KEY`（Python 版三份报告用的是同一个）；`config/aite.yaml` 里只有变量名，没有取值，且不入库 |
| 平台 | `fake`（真模型 + 替身平台，不碰真实飞书） |
| 沙箱 | **跑了两档**：`fake`（场景自带的 `exec_script`）与 `docker`（真容器，edge 在跑） |
| 超时 | `--timeout-scale` 默认 12.0（live） |
| 场景 | 只跑 `04_csv_to_chart` —— 它是唯一同时压到附件、沙箱、产物、交付四段的那个 |

```bash
core/target/debug/aite-edge --config config/aite.yaml &     # platform: fake 也行，docker 档只要它的沙箱面
core/target/debug/aite evals run evals/p0 --only 04_csv_to_chart --model live \
    --protocol-report /tmp/proto.json
core/target/debug/aite evals run evals/p0 --only 04_csv_to_chart --model live --sandbox docker \
    --protocol-report /tmp/proto-docker.json
```

## 1. 两跑的形状

| | `--sandbox fake` | `--sandbox docker` |
|---|---|---|
| 步数 | 14 | **7** |
| 到 final | 第 14 步 | 第 7 步 |
| 任务终态 | `delivered` | `delivered` |
| chat 次数 / 成功 | 14 / 14 | 7 / 7 |
| `finish_reason` | 全是 `tool_calls` | 全是 `tool_calls` |
| 协议外的工具名 | 0 | 0 |
| 参数不合 `ToolSpec.parameters` | 0 | 0 |
| 模型重试（5xx / 限流） | 0 | 0 |
| 只回文本没调工具 | 0 | 0 |
| 场景总耗时 | 17.8 s | **11.7 s** |
| 产物 | 没有（见第 2 节） | `/work/monthly_trend.png`，40139 字节，`send_file` 1 次 |

**出牌严格守协议**，与 Python 版 T17 的结论一致：21 次调用里 0 个协议外的名字、
0 条参数不合 schema。这一条在换语言之后没有退化。

### token 与延迟（实测，不是估算）

| | 步数 | input | output | 其中 cached | 单步延迟 min/中位/max | 合计 |
|---|---|---|---|---|---|---|
| fake 沙箱 | 14 | 35672 | 1284 | 32384 | 713 / 1143 / 2781 ms | 17.6 s |
| docker 沙箱 | 7 | 20213 | 994 | 17788 | 753 / 1473 / 1793 ms | 9.7 s |

**命中率很高的 prompt cache**（fake 档 32384/35672 = 91%）：agent loop 每一步都把
前面的上下文原样再发一遍，DeepSeek 的自动缓存正好吃这个形状。
**费用没有算**：`config/aite.yaml` 里 `price_in_per_mtok` / `price_out_per_mtok` 都是 0，
卡片上的「已用 ¥」因此恒为 0 —— 这是配置没填，不是计费坏了（`aite preflight` 第 ⑤ 组会说
「config 里 price_in/out_per_mtok 都是 0，没配价格」）。

## 2. `--sandbox fake` 那一跑为什么绕了 14 步

场景的 `sandbox.exec_script` 只写了一条台词：代码里出现 `savefig` 就当画好了图。
**真模型写的代码不长那样** —— 它先 `print` 探数据、再分组、再画图，前几步一条都匹配不上，
`FakeSandbox` 于是返回「无害的默认值」（退出码 0、stdout 空、没有产出文件）。

模型的反应是**对的**：它连试了六次 `run_python`，发现连 `print("hello")` 都没有输出，
于是判定「沙箱执行环境异常」，把卡点写清楚交付了出去：

> 抱歉，这个任务没能完成，原因如下：**卡点：沙箱执行环境异常** —— 附件 `sales.csv`
> 已成功下载到 `/work/in/file_sales_csv`（48 字节）。但 `run_python` 完全无法返回任何结果……

这不是缺陷，是 `--model live --sandbox fake` 这个组合本身的错配：
**场景的 `exec_script` 是照 `model_script` 那份台词写的**，换成真模型就对不上。
真模型要跑有意义的实测，得配 `--sandbox docker`。这条值得写进读法：
**`--model live` 之后请接着写 `--sandbox docker`**。

## 3. `passed 0/1` 的两条失败分别是什么

### 3.1 `--sandbox docker`：1 条，是场景与替身排版的耦合

```
run_python 的结果里没有 "exit_code=0"；实际："执行成功（715 ms）
stdout:
saved

/work 下本次新增/修改的文件：
  /work/monthly_trend.png（40139 字节）"
```

`04_csv_to_chart.yaml` 第 69 行写的是 `{check: gateway_result, name: run_python, ok: true, contains: "exit_code=0"}`。
`exit_code=0` 是 **`FakeToolGateway` 的排版**（`fake_gateway.rs:296`：`exit_code={}\n--- stdout ---\n{}`）；
**真** `P0ToolGateway` 的排版是 `执行成功（N ms）…`，而它与 Python 版**逐字一致**
（对拍过 `aite/tools/python_exec.py:78` 与 `core/crates/gateway/src/tools/python_exec.rs:84`）。

所以这一条在 Python 时代也一样红 —— 它是 `evals/p0/*.yaml`（**冻结**，一个字不动）里
继承下来的耦合，不是本轮的回归。`review/inventory-gateway-evals.md` §11 第 26 条
（「场景断言耦合 FakeGateway 排版」）记的就是它。

**其余 7 条断言全过**，包括两条最硬的：

- `{check: file, index: 0, magic: "89504e470d0a1a0a"}` —— 发回去的那份字节**真的是 PNG**；
- `{check: task, which: last, status: delivered}` —— spec §6.1 给 docker 档定的判据就是这一条。

### 3.2 `--sandbox fake`：2 条，是第 2 节那个错配的下游

```
platform.send_file 期望 == 1，实际 0
期望第 0 个 send_file，实际只发了 0 个文件
```

模型没画出图（沙箱替身不配合），自然没有产物可发。同上，不是实现的问题。

## 4. 与 Python 版三份报告的对照

| Python 版 T17 记的 | Rust 版这次 |
|---|---|
| 出牌严格守协议：133 次调用 0 个协议外的名字 | 21 次调用 0 个协议外的名字、0 条参数不合 schema ✅ 一致 |
| **一次都没用过 checklist 工具** | **用了**：两跑都以 `checklist_add` 开局（同样的 5 项：下载 / 读取检查 / 按月聚合 / 画图 / 保存 PNG），中途 `checklist_check` 逐项打勾，fake 档还用了 `checklist_fail` + `checklist_note` ⚠️ **与 Python 版结论相反** |
| 「工具成功但内容为空 → 原地重复同一个调用」，实测连发 33 次逐字节相同的 `run_python` | fake 档遇到了同一形态（沙箱替身返回空），但**只连了 6 次就自己收敛**去 `list_files` 找原因，然后交付了一份说清卡点的失败说明。`repeat_loops` 计数为 **0**（现有的 T20 重复检测一次都没触发 —— 它比的是逐字节相同的参数，而模型每次的代码都不一样） |

**checklist 那条翻转值得单独说**：3 分钟演示的主角正是那张会原地更新的 checklist 卡片，
Python 版实测时模型一次都不用它。这次两跑都用了，而且用满了四个 checklist 工具。
**没测到的是为什么** —— 中间隔着换语言、换提示词渲染、模型本身的版本漂移，
本轮没有做对照实验，不敢归因。真要演示之前，**建议再跑一次确认它稳定**（一次不算数）。

## 5. 没测到的

- **真实飞书**：两跑的 platform 都是 `fake`。M1–M6 才碰真机。
- **费用**：价格没配，`¥` 恒为 0。要看真花了多少去 DeepSeek 控制台对。
- **稳定性**：每档只跑了**一遍**。Python 版那三份是跑两遍对比的，这次没有 ——
  上面 checklist 那条翻转尤其需要第二遍。
- **别的九个场景**：只跑了 04。其余九个在 `--model live` 下的形状没测。
