# 真模型 × 真沙箱实测（T23 段 B + T20 段 ③）

> 跑的人：总管（原 T23 / T20 两个会话已关闭，凭证到位后在主仓补跑）
> 提交：`0960272`（T19–T24 六轨全部合入之后的 main）
> 模型：`deepseek-chat` @ `https://api.deepseek.com/v1`，密钥只经 `AITE_MODEL_API_KEY`
> 日期：2026-09-10

这份报告补上两轨各自没跑完的那一半：T23 的「真模型 × 真沙箱」和 T20 的
「兜底在真模型下开不开火」。两件事共用同一批实测，所以写在一起。

---

## 一、M3 主干第一次真的走通了

`04_csv_to_chart` 走的是 §2.4 M3 / 演示第二幕那条路：
附件 → `download_attachment` → `run_python` 画图 → `final(artifacts)` → `send_file`。

```bash
python -m aite.evals run evals/p0 --only 04_csv_to_chart \
    --platform fake --model live --sandbox docker
```

**三遍，全部 delivered，每遍 7 步。**

| 遍 | 终态 | 步数 | run_python 耗时 | 产出 PNG | 全程 |
|---|---|---|---|---|---|
| 1 | delivered | 7 | 723 ms | `/work/monthly_trend.png` 40139 字节 | ~12 s |
| 2 | delivered | 7 | 717 ms | `/work/monthly_trend.png` 41006 字节 | ~12 s |
| 3 | delivered | 7 | 701 ms | `/work/monthly_trend.png` 40139 字节 | ~12 s |

那张 PNG 是 `aite-sandbox:p0` 容器里的 matplotlib 真画出来的，魔数检查
（`89504e470d0a1a0a`）在评测里过了。模型自己给文件取名 `monthly_trend.png`
（场景脚本里写的是 `out.png`），`final.artifacts` 的路径跟着自己的命名走，W5 那条链没断。

回帖内容也对得上真数据 —— CSV 里就是 2026-01/02/03 的 120/180/90：

> 已把 sales.csv 画成月度趋势图 📈　数据只有 3 个月：…
> 趋势：2 月冲到最高 180，3 月回落到 90，比 1 月还低。图见附件。

第 2 遍模型还顺手把解析结果打了出来：
`stdout: saved ['2026-01', '2026-02', '2026-03'] [120.0, 180.0, 90.0]`。

跑完 `docker ps -a --filter label=aite.task` 是空的，容器没有残留。

### 唯一那条红的不是沙箱的问题

```
run_python 的结果里没有 'exit_code=0'；
实际：'执行成功（723 ms）\n\nstdout:\nsaved\n\n\n/work 下本次新增/修改的文件：
       /work/monthly_trend.png（40139 字节）'
```

`{check: gateway_result, name: run_python, ok: true, contains: "exit_code=0"}` 断的是
`FakeToolGateway` **自己的 content 排版**；真 `P0ToolGateway` 排的是「执行成功（N ms）」。
两边说的是同一件事（工具成功、退出码 0），只是措辞不同。

按纪律没动 `evals/p0/04_csv_to_chart.yaml` —— 那是 §3.8 的验收面。处理办法见文末。

### 对比：这条路此前从没走通过

| 谁验的 | 模型 | 沙箱 | 结果 |
|---|---|---|---|
| `passed 10/10` 里的 04 | `FakeModel` 按脚本 | `FakeSandbox` 按 `exec_script` | 两头都是演的 |
| T17（`live-report-2026-09-10.md`） | **真 DeepSeek** | `FakeSandbox` | **两遍都 failed**，对逐字节相同的 `run_python` 连发 33 次烧满 `max_steps` |
| 本次 | **真 DeepSeek** | **真 Docker** | **3/3 delivered，7 步** |

---

## 二、T20 的兜底：真模型下一次都没开火

```bash
python -m aite.evals run evals/p0 --platform fake --model live      # 全套 10 个场景，两遍
```

**两遍全套，`repeat_loops` 合计 0 处，兜底的提示和 `_fail` 一次都没触发**
（回帖里搜「原样重复调用」= 0 次）。连「同一张牌连着出 3 次」这个更低的门槛都没到过。

原因是 T19 那一轨：提示词把 checklist 的触发条件改成第一步可判定的判据之后，模型不再
陷在「空返回 → 再试一次一模一样的代码」里 —— 它会 `checklist_fail` 标掉这一项，然后
换招或者 `final` 说明卡在哪。T19 自己也观察到了这一点（`04` 的原样连发 30/32 → 0）。

**结论：兜底现在是保险，不是常用路径。**

**它仍然该留着**，理由是这次实测本身给出的：兜底针对的失败面（工具 `ok=True` 但没进展）
今天靠**提示词**压住了，而提示词对模型是软约束 —— 换个模型（百炼 / 智谱）、换个题面、
或者哪天供应商悄悄改了模型版本，这条路随时可能回来，而 §3.3 里没有任何一条硬兜底接得住它。
留着的代价是零（不触发就完全不参与），去掉的代价是回到 T17 那次「烧满 40 步 + 一笔钱 +
几分钟一动不动」。

一起验掉的：本地工具不开火那个取舍没有副作用 —— `08_step_limit` 在 live 下照旧
`failed` 3 步（它 `max_steps` 压到 3），走的仍是 `max_steps` 那条路。

---

## 三、live 全套 5/10，五条失败逐条看

`passed k/10` 在 live 下**不是验收数**：那 10 份 `expect` 是照 `model_script` 的确切
出牌写的，真模型只要换个说法就对不上。scripted 那条路仍然是 **10/10**（验收面没动）。

五条失败，没有一条是缺陷：

| 场景 | 失败判据 | 实际发生了什么 |
|---|---|---|
| `01_simple_qa` | 文本要含「北京今天晴」 | 模型**拒绝编天气**：「沙箱没有网络，也没有接天气数据源，所以给不出今天北京的真实天气」——正是 W9 铁律 4「不得声称做了没做的事」。脚本里那句「北京今天晴」是替身的台词 |
| `03_checklist_progress` | `checklist_check ≥ 3`，实际 0 | 替身沙箱空返回 → 模型按铁律把这几项 `checklist_fail` 而不是 `check`。卡片确实在动，只是勾变成了叉 |
| `04_csv_to_chart` | `send_file == 1`，实际 0 | **替身沙箱**下没有真 PNG 可发。同一场景换 `--sandbox docker` 就是 3/3 delivered（见第一节） |
| `07_commands` | `sandbox.acquire/release ≥ 1`，实际 0 | 真模型被 `!stop` 掐掉之前还没用到沙箱 |
| `10_duplicate_event` | `ModelPort.chat == 1`，实际 4 | 脚本一步答完，真模型走了 4 步。去重本身是对的（只建了一个 task） |

`01` 那条尤其值得记一笔：**替身的验收判据在惩罚模型的正确行为**。

---

## 四、成本

| | input token | output token |
|---|---|---|
| `04` × 3（live × docker） | 60,741 | 2,994 |
| 全套 × 2（live × fake） | 268,657 | 11,544 |
| 合计 | **329,398** | **14,538** |

按 DeepSeek 公示价（miss ¥2/M、hit ¥0.2/M、out ¥8/M）折算：全部按 miss 计的**上界**约
**¥0.78**；实际缓存命中很高（T17 那轮实测 92%），真实开销约 ¥0.2 上下。**以账单为准**，
原始 token 在表里，换单价自己乘。

---

## 五、要总管定的

1. **`04_csv_to_chart.yaml` 那条 `contains: "exit_code=0"`。** 它把 `FakeToolGateway`
   的排版写进了验收面，导致 docker 档天然差一条（scripted × docker 是 9/10，
   live × docker 那条也是同一条红）。三个选择：
   - **放着不动**，docker 档就是 9/10，在文档里写明那一条为什么红（成本最低，但每次看到
     9/10 都要重新解释一遍）；
   - 把判据放宽成两边都成立的（比如断 `ok: true` 加上产物文件名），
     **改的是 §3.8 的验收面，要你点头**；
   - 让真 Gateway 的 content 里也带上 `exit_code=0`（改实现去迁就测试，不建议）。

   我倾向第二个：这条判据本意是「工具成功了」，而「成功」的证据不该是替身的排版字符串。

2. **演示第二幕用哪条路。** 现在有实测支撑了：真模型 × 真沙箱 3/3 通、每遍约 12 秒、
   7 步、PNG 40 KB。§2.4 M3 那条链在离线环境下已经端到端成立，剩下只有飞书那一段没验。

3. **`01_simple_qa` 的台词判据**（表里那条）。同样是替身台词进了验收面。P0 不改也行
   —— scripted 路径本来就该按脚本断言 —— 但 live 报告里每次都会多一条红，值得在
   README 或场景注释里写一句「live 下 k/10 不是验收数」。
