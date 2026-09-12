# 任务 T20 — 原地打转：§3.3 没有一条兜底接得住的那种失败

## 背景：这轨是从哪来的

T13–T18 六轨全部合进 main（最新 `6ee30d4`）。全仓 **1061 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过，契约锁 `OK 11 files`。P0 的代码面齐了，剩 §2.4 的 M1–M6
卡在飞书凭证。

T17 拿真模型（DeepSeek）跑了两遍 10 个场景，报告在 `evals/live-report-2026-09-10.md`。
`04_csv_to_chart` **两次都飞**，而且飞法一模一样：

模型下载附件后想先读文件看看内容 → 替身沙箱对不含 `savefig` 的代码一律回
`exit_code=0` + 空 stdout → 它自己诊断了几步（原话：`The sandbox is returning empty
output for every command`）→ 然后对**逐字节相同**的 `run_python` **连发 33 次**，
一路烧到 `max_steps=40` 才停。

§3.3 那张失败面表里没有一条接得住它：

| §3.3 的兜底 | 为什么接不住 |
|---|---|
| 参数不合 schema 连续 3 次 | 参数完全合法 |
| 沙箱创建/执行失败连续 2 次 | 工具返回的是 **`ok=True`**，只是内容为空 |
| 工具执行超时 | 没超时，每次都秒回 |
| 无 tool_call 无 final | 每次都有 tool_call |

**唯一接住它的是 `max_steps`，代价是烧满整整 40 次模型调用。** 真机上这是几分钟的
一动不动加一笔钱；演示时这是第二幕当场死在镜头前。

## 这轨的形状

给 worker 加一条 §3.3 现在没有的兜底：**同一张牌连着出好几次 = 卡住了**。
然后**拿真模型证明它真的接住了 04 那条路**。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t20
分支     : task-t20
基线     : 6ee30d4   ← main 的 HEAD（完整 sha 6ee30d4e6013782ff7cdd4e69ec8efa101f4b2db）
Python   : python3.12（3.12.1）—— venv 要你自己建
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

## 真模型凭证

跑 live 要 `model.base_url` / `model.model`（`config/aite.yaml`，总管填）+ 环境变量
`AITE_MODEL_API_KEY`。上一轮总管**当场授权**借用 DeepSeek 那份（端点
`https://api.deepseek.com/v1`，模型 `deepseek-chat`），全套 10 个场景约 ¥0.10。

**起来时这三样缺任何一样 —— 停下问总管。不要去 `~/.bash_profile` 或别的地方自己翻密钥。**
拿不到 key 的话：①②④ 照做（离线能做完），③ 那栏写「待 key」，**不要拿 scripted 的结果
冒充 live**，也不要写「真模型大概会……」这种推测。`config/aite.yaml` 不许进 git。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-t20
git log --oneline -1                        # 期望 6ee30d4 ...
git status --short                          # 期望空（.venv 已在 .gitignore 里）
.venv/bin/python -m pytest -q               # 期望 1061 passed
.venv/bin/python -m pytest tests/worker -q  # 期望 48 passed
scripts/check.sh                            # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
```

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
被拦才说明 hook 挂上了；没被拦说明 `CLAUDE_PROJECT_DIR` 不对，所有守卫都在静默失效，
停下报告。

**关于 `tests/sandbox` 假红（先看这条，能省你十分钟）**：六轨同时跑测试时
`tests/sandbox/test_docker_sandbox.py` 会红 1–3 条，每次红的用例还不一样。根因是
`docker ps -a --filter label=aite.task` 是**全机器**的命名空间，别轨的容器会被你的
`reap_idle` 收走、你的也会被别人收走。**判据**：红的只在 `tests/sandbox/`、失败断言形如
`labelled_ids() == []` 或 `reap_idle(...) == []`、单跑 `pytest tests/sandbox -q` 是
**36 passed** → 串扰不是回归，等别轨安静了重跑。那个目录不在任何一轨的可写面。

## 可写路径（白名单，之外一律只读）

```
aite/worker/loop.py                        ← 主战场
tests/worker/test_loop_fallbacks.py        ← 新建
tests/worker/test_limits.py                ← 可改（§3.3 的上限类兜底本来就归它）
```

**邻居**：`aite/worker/prompts/platform.md` 归 **T19**（它在改 checklist 的提示词）；
`aite/evals/**` 和 `evals/p0/*.yaml` 归 **T23**；`aite/models/**` 只读。一个字都别碰。

## 要做什么

### ① 加原地打转的兜底

现成的形状就在同一个文件里，照抄它：`loop.py:45` 起有
`MAX_CONSECUTIVE_INVALID_ARGS` / `MAX_CONSECUTIVE_SANDBOX_ERRORS`，
`_RunContext` 上挂 `ctx.invalid_args` / `ctx.sandbox_errors`，命中错误 +1、成功清零，
到阈值走 `_fail`（`loop.py:197–224`）。你要加的是第三个同族计数器。

T17 给的建议：记 `(tool_name, canonical_json(args))` 连续重复数，第 3 次回一条 system
提示、第 5 次 `_fail`。阈值 3 有实测依据 —— `03` 里 `list_files()` 正常连发过 2 次。

**但这个设计你要自己判一遍，几个必须想清楚的点：**

- **「连续」怎么定义**：中间插进一张别的牌要不要清零？（我倾向要 —— 换招了说明模型还在
  推进，不算卡住。但你去看 T17 报告里 `04` 的实际序列再定。）
- **一步出多张牌时怎么算**：`turn.message.tool_calls` 是个列表，同一步里出两张一样的牌算 2 还是 1？
- **算不算本地工具**：`checklist_note` 这类 `LOCAL_TOOL_NAMES` 里的牌，连发算不算打转？
  **这一条直接决定你会不会踩到下面那颗雷，先读完再定。**
- **system 提示该说什么**：目标是让模型**换招**，不是让它道歉后再发一次同样的牌。
  T17 报告里模型自己都诊断出「沙箱每条命令都返回空输出」了，说明它需要的不是
  「你重复了」而是「这条路走不通，换一条或者 final 说明情况」。
- **`_fail` 时回帖说什么**：§3.3 的口径是「task failed + 回帖 + evidence failed」，
  回帖要让群里的人看懂发生了什么（参照现有那两条的措辞）。

### ⚠️ 一颗雷：`08_step_limit`

`evals/p0/08_step_limit.yaml` 的 `model_script` 是：

```yaml
config:
  worker:
    max_steps: 3
model_script:
  - tool_calls:
      - name: checklist_note
        arguments: {text: 再想想}
    repeat: inf
expect:
  - {check: model_calls, min: 3}
  - {check: task, which: last, status: failed}
  - {check: text, where: any, contains: 上限}          # ← 最脆的一条
  - {check: platform_calls, method: send_card, equals: 1}
  - {check: cards, distinct_equals: 1, final_status: failed}
```

**这就是一个「连发同一张牌」的场景，`max_steps` 压到了 3。** 如果你的计数器把本地工具
也算进去、阈值又取 3，它会在 `max_steps` 之前（或同时）开火，回帖里那句「上限」就没了 ——
`08` 当场变红，而 `evals/p0/*.yaml` 是 **T23 的面，你不能改**。

出路有几条（自己选，理由写进回执）：只算 Gateway 工具不算本地工具；把「回 system 提示」
放在阈值 3、把 `_fail` 放在更后面（提示不改变终态，`max_steps` 仍然先到）；或者别的。
**动手前先把 `08` 跑通一遍确认你的选择成立**，别做完才发现。

### ② 口径要和观测装置对得上

`aite/evals/protocol_probe.py` 里已经有一个 `_repeat_loops()`（T17 加的），按
`f"{name}:{canonical_json(sorted(args))}"` 摊平算连续重复，`MIN_REPEAT_RUN = 3`。
那是**观测**侧的口径，你加的是**兜底**侧的口径 —— 两边不一致的话，报告里说「打转 33 次」
而系统说「没打转」，排查时会打架。对不上就在回执里写清为什么该不一样。
（那个文件归 T23，**只读**。）

### ③ 拿真模型证明它接住了（这轨的靶心）

```bash
# 改之前
.venv/bin/python -m aite.evals run evals/p0 --only 04_csv_to_chart --platform fake --model live --protocol-report /tmp/t20-before.json
# 改之后
.venv/bin/python -m aite.evals run evals/p0 --only 04_csv_to_chart --platform fake --model live --protocol-report /tmp/t20-after.json
```

要答出来的：改之前烧了几步？改之后在第几步被接住？接住之后模型**换招了吗**（这是提示
写得好不好的证据），还是照样重复到 `_fail`？省下多少次模型调用？

真模型有随机性，**同一个配置至少跑两遍**再下结论。

### ④ 顺手证伪一条（别做成任务，只要一个结论）

T17 的报告里有一条建议：「`ModelConfigError` 被 `_chat` 的 `except Exception` 吞掉，
白重试 2 次」。`_chat` 在 `loop.py:246–260`，确实是 `except Exception` 一视同仁。

**但生产路径上这条可能已经不成立了**：`aite/app.py:290–303` 在 `build_app` 阶段就把
`base_url` / `model` 空串和 `resolve_api_key` 失败都转成了 `StartupError`，压根到不了
worker。去核实一遍，**如果确认拦住了就在回执里写「不用改」并给出行号依据**；
如果发现还有别的路径能让 `ModelConfigError` 走到 `_chat`（说清是哪条），那再说要不要修。
**不要为了「有产出」硬加一个改动。**

## 纪律

1. 契约（`aite/contracts/**`）一个字都不许动，锁必须全程 `OK 11 files`。
   `max_steps` / `max_wall_sec` 冻在 `WorkerConfig` 里（`tests/contracts/test_frozen_values.py:299`
   逐字段钉着），别顺手调。
2. 白名单之外的文件只读。要改别人的面 → **停下报告**，写清建议。
3. 每条结论挂实测。推测一句不要。

## 验收

```bash
.venv/bin/python -m pytest tests/worker -q   # 期望 ≥48 passed，一条不许红
.venv/bin/python -m pytest -q                # 期望 ≥1061 passed
scripts/check.sh                             # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted            # 期望 passed 10/10
.venv/bin/python -m aite.evals run evals/p0 --only 08_step_limit --platform fake --model scripted   # 单跑，必须绿
.venv/bin/python -m aite.evals run evals/p0 --only 03_checklist_progress --platform fake --model scripted
```

最后两条单独跑：`08` 是上面那颗雷；`03` 是「正常的连发」的对照（`checklist_check` 逐项
调用，参数不同所以不该被你的计数器算成打转）。契约锁 `--check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T20 回执

基线 6ee30d4 → 提交 <短 sha>

### 兜底的设计（每条给出你的选择 + 理由）
连续怎么定义：<中间插别的牌清不清零>
一步多张牌：<怎么算>
算不算本地工具：<算/不算，为什么>
阈值：<提示在第几次 / failed 在第几次，实测依据>
system 提示原文：<贴出来>
_fail 的回帖原文：<贴出来>

### 08_step_limit 那颗雷
<你的设计为什么不会让它变红，附单跑输出>

### 与 protocol_probe 的口径
<一致 / 不一致，不一致的理由>

### 真模型实测（04_csv_to_chart，跑两遍）
改前烧了几步：<>　改后第几步被接住：<>
接住之后模型换招了吗：<换了/照样重复到 failed>
省下多少次模型调用：<>　省下多少钱：<>

### ④ ModelConfigError 那条
<已被 app.py 拦住，不用改 / 还有这条路径能走到 _chat：…>

### 改了什么
- <文件:行> <一句话>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m pytest tests/worker -q
<最后一行>

$ scripts/check.sh
<最后 3 行>

$ .venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
<最后一行>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t20` 分支上，回执贴出来。
