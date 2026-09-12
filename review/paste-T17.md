# 任务 T17 — 真模型实测：把 `--model live` 真的跑一遍（T12 段 B）

## 背景：这轨是从哪来的

T1–T12 十二轨全部合进 main（最新 `0d6939c`）。全仓 **969 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过。P0 的代码面齐了，剩 §2.4 的 M1–M6 卡在飞书凭证。

**这轨是 T12 那一轨没做完的后半段。** T12 分两段派的：

- **段 A（做完了，已合入）**：`aite/evals/protocol_probe.py` 协议出牌观测装置
  （`--protocol-report`），外加修掉 `--model live` 两处让它一次都没跑起来过的硬伤 ——
  `Deps.activity()`/`stats()` 读的 `model.calls`/`call_count` 是替身的记账面真模型没有；
  `settle()` 的 150ms 静默判据看不见长时间 await，真模型第一次回包前任务就被判"不干活"取消了。
  现在 scripted / live 统一套一层 `ModelProbe`，`model_busy()` 参与静默判据，
  `--timeout-scale`（live 默认 12.0）在内存里放大场景等待上限。
- **段 B（就是你）**：**拿真模型跑一遍，看它到底怎么出牌。** T12 没 key，如实停在了这儿。

为什么这件事值得单独一轨：**那 969 条测试和 `passed 10/10`，出牌的全是 `FakeModel`
按 `model_script` 演的。** 真实 LLM 从没驱动过 `aite/contracts/protocol.py` 那套工具协议。
3 分钟演示（spec §1）要靠真模型调 `checklist_add` / `run_python` / `final` 把进度面驱动起来，
国内模型（百炼 / 智谱）的 tool_calling 跟脚本化替身差多远，**今天没有任何人知道**。

到真机 M3 才发现真模型不按协议出牌，排查成本比现在高得多。

## 这轨的前置：没有 key 就停下报告

`--model live` 要三样东西：

```
model.base_url         百炼 / 智谱的 OpenAI 兼容端点     ← config/aite.yaml，总管填
model.model            模型名                            ← config/aite.yaml，总管填
AITE_MODEL_API_KEY     环境变量（§3.1：密钥只放环境变量）  ← 总管给
```

**这三样缺任何一样，这轨就没法做。** 那就：

1. 在回执里写明"缺什么、等总管给"
2. **不要拿 `--model scripted` 的结果冒充 live 的实测**
3. **不要编造模型行为**（"真模型大概会……"这种推测一句都不要写进报告）
4. 也不要为了"有产出"去改别的东西凑数 —— 停下就是正确的交付

没 key 时 `--model live` 会打 `--model live 起不来：<异常>` 并退 2，这是它现在的正常行为，
不是 bug。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t17
分支     : task-t17
基线     : 0d6939c   ← main 的 HEAD（完整 sha 0d6939ca119ed40bb3320746d357aae85ebb4c41）
Python   : python3.12（3.12.1）—— venv 要你自己建
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 0d6939c
git rev-parse --abbrev-ref HEAD         # 期望 task-t17
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check    # 期望 OK 11 files，退出码 0
.venv/bin/python -m pytest -q                      # 期望 969 passed（没有 xfailed）
.venv/bin/python -m pytest tests/e2e -q            # 期望 190 passed
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
scripts/check.sh                                   # 期望最后一行 全部通过，退出码 0
```

**还有一条自检是验守卫的**（这条要"被拦"才算过）：

```
Read 工具读 .claude/hooks/guard_bash.py
```

期望**被 hook 拦下**（`blocked: … .claude/hooks/guard_bash.py（读取位置）`）。
被拦 = 守卫挂上了。**没被拦就说明 hook 失效了，停下报告**。

## 守卫会拦你的几种写法（前几轨实撞出来的）

| 写法 | 结果 |
|---|---|
| Bash 命令里出现 `aite/contracts`、`.contracts.lock`、`docs/dev-spec-*.md` 字面量 | 拦停，哪怕只是 `[ -e ]` 只读探测 |
| `find ... -delete` / `find ... -exec` | 拦停（覆盖面判不出来） |
| `git commit -m "多行\n带引号的 message"` | 拦停（shlex 解析失败）。**用 `git commit -F <文件>`** |

## 先读

1. `evals/README.md` 第 2 节「live：真模型实测」—— T12 把怎么跑、为什么 `passed k/10`
   不是判据、该看什么都写清了，**先读它，别重新发明**
2. `aite/evals/protocol_probe.py` —— 观测装置，`analyze()` 出什么、`render_digest()` 打什么
3. `aite/evals/__main__.py` 的 `--protocol-report` / `--timeout-scale` / `LIVE_TIMEOUT_SCALE`
4. `aite/models/openai_compat.py` —— live 模型客户端
5. spec §3.3 里跟模型有关的四行（模型 5xx 重试 2 次、参数不合 schema 连续 3 次、
   既无 tool_call 也无 final 的兜底、超 max_steps）—— 你要对照的就是这几条
6. `aite/contracts/protocol.py` 的 `ALL_MODEL_TOOLS`（**只读**）—— 协议本身

## 先说清楚：live 下 `expect` 判据必然大面积红，那是预期的

`evals/p0/*.yaml` 里的判据是照 `model_script` 的台词写的，比如
`01_simple_qa.yaml` 里有 `{check: text, where: last, contains: 北京今天晴}` ——
真模型不可能复现这句话。`aite/evals/__main__.py:42` 的注释也写明
`--model live`「不属于 P0 验收路径」。

**所以：不许改场景判据来让 live 变绿。** 那些判据钉的是 scripted 路径的 B8 验收面，
改了就是把 P0 的验收拆了。要看的是 `--protocol-report`。

## 可写路径

```
aite/models/**
aite/evals/**            （不含 evals/p0/*.yaml 里现有的 expect 判据）
evals/**                 （可新增文件，如实测报告；现有 10 个场景的 expect 只读）
tests/e2e/**
```

**只读，改了就是任务失败**：`aite/contracts/**`、`.contracts.lock`、
`docs/dev-spec-2026-09-09.md`、`evals/p0/*.yaml` 里现有的 `expect`。

并行的另外五轨在动这些地方，**别碰**：`tests/integration/**` `aite/app.py`（T13）、
`aite/control/plane.py` `aite/worker/**`（T14）、`docs/demo-3min.md` `scripts/demo_*`（T15）、
`aite/adapters/feishu/**` `docs/feishu-api-diff.md`（T16）、`aite/control/store.py`（T18）。

**特别注意**：§3.3 那几条模型兜底大多实现在 `aite/worker/loop.py` —— **那是 T14 的地盘。**
你发现兜底缺口时**只写进回执，不要改 loop.py**，由总管定序。

## 目标一：真模型实跑

先单场景过一遍再跑全套，省 token：

```bash
cp config/aite.example.yaml config/aite.yaml      # 填 model.base_url / model.model
export AITE_MODEL_API_KEY=…                       # 只放环境变量，不写进 yaml

.venv/bin/python -m aite.evals run evals/p0 --only 01_simple_qa \
    --platform fake --model live --protocol-report /tmp/proto-01.json

.venv/bin/python -m aite.evals run evals/p0 \
    --platform fake --model live --protocol-report /tmp/proto-all.json --json /tmp/live.json
```

`config/aite.yaml` **绝不能写 key 的值**，而且它不该被提交 —— 交回执前
`git status --porcelain` 确认它没进去（`.gitignore` 应该挡住了，自己核一遍）。

## 目标二：实测报告（这轨的主产出）

落到 `evals/live-report-<日期>.md`（新建文件，在你的可写路径里）。要回答：

1. **10 个场景里，真模型走到 `final` 的有几个**，跑飞的是哪些、怎么飞的
   （撞 `max_steps`？超时？异常？）
2. **它调出来的工具名和参数**：有没有协议外的名字、参数合不合 `ToolSpec.parameters`、
   `checklist_add` 的每项有没有超 20 字（W9 要求）、`final` 的 `artifacts` 路径合不合规
3. **§3.3 那几条兜底哪几条真的被触发了**，触发后行为对不对：
   - 模型 5xx / 异常 → 重试 2 次（2s / 5s），仍失败 → task `failed` + 回帖
   - 参数不合 schema → 回 `invalid_args`，同一任务连续 3 次 → `failed`
   - 既无 tool_call 也无 `final` 只返回文本 → `steps==0` 时视为 `final`，
     `steps>0` 时回 system 提示
4. **兜底缺口**：真模型的某种出牌方式，现有兜底接不住的。每条写清：
   现象、该在哪个文件修（哪怕是 `loop.py`）、建议怎么修 —— **但不要动 T14 的文件**
5. **成本与时长**：跑完 10 个场景花了多少 token / 多少钱 / 多少分钟
   （演示要不要用真模型，总管得知道这个数）

## 目标三（如果段 A 的装置不够用，补它）

跑的过程中如果发现 `--protocol-report` 少了你需要的观测项（比如没记 `finish_reason`、
没记每步耗时、没标出哪一步触发了兜底），补 `protocol_probe.py` —— 那是你的可写路径。
补了要配测试（`tests/e2e/`），并且**`--model scripted` 的 `passed 10/10` 不许变**。

## 验收

```bash
.venv/bin/python -m pytest -q                     # 期望 ≥969 passed，一条不许红
.venv/bin/python -m pytest tests/e2e -q           # 期望 ≥190 passed
scripts/check.sh                                  # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python -m aite.evals run evals/p0 --list # 期望还是那 10 个场景名
git status --porcelain                            # 期望里面没有 config/aite.yaml
```

`.contracts.lock --check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T17 回执

基线 0d6939c → 提交 <短 sha>

### 拿到 key 了吗
<是，端点 xxx / 模型 xxx  |  没有，缺 <哪几样>，本轨停在这里>

（没拿到 key 的话，下面几栏写"待 key"，不要填推测）

### 真模型实测
- 走到 final 的场景：k/10
- 跑飞的：<场景名 + 怎么飞的>
- 协议外的工具名：<有/无，列出来>
- 不合 schema 的 arguments：<列出来>
- checklist 每项 ≤20 字：<守没守>

### §3.3 兜底逐条
模型 5xx 重试：<触发了吗 / 行为对不对>
参数不合 schema 连续 3 次：<>
无 tool_call 无 final 的兜底：<>
超 max_steps：<>

### 兜底缺口（这轨最值钱的一栏）
<每条：现象 / 该改哪个文件 / 建议修法。属于 T14 地盘的只写建议，别动手>

### 成本
token：<in/out>  钱：<¥>  时长：<分钟>
演示该用真模型还是脚本化替身：<你的建议 + 理由>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
<最后一行>

$ scripts/check.sh
<最后 3 行>

### 卡住的地方 / 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t17` 分支上，回执贴出来。
`config/aite.yaml` 不要提交。
