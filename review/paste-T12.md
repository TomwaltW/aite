# 任务 T12 — 真模型实测：`--model live` 第一次真的跑起来

## 背景：这轨是从哪来的

T1–T10 十轨全部合进 main（最新 `f9d45ab`）。全仓 **936 passed, 1 xfailed**，
§3.8 的 10 个评测场景 **passed 10/10**，`scripts/check.sh` 全部通过。

但这 936 条测试和那个 10/10，**出牌的全是 `FakeModel` 按 `model_script` 演的**。
真实 LLM 从来没有驱动过 `aite/contracts/protocol.py` 里那套工具协议 —— 一次都没有。

`--model live` 的代码是落地的（`aite/models/openai_compat.py` +
`aite/evals/__main__.py:41` 的 `_live_model_factory`），spec §6 T4 那格也点了它
（"评测 runner 与 live 模型客户端…正好也是 §14.2「模型实测」要用的工具（`--model live`）"）。
**但它从没被真跑过一次。**

这件事的风险是实打实的：P0 的目标是"能录一段 3 分钟演示"。演示要靠真模型
调 `checklist_add` / `checklist_check` / `run_python` / `final` 把进度面驱动起来。
国内模型（百炼 / 智谱）的 tool_calling 行为跟脚本化替身差多远，
今天**没有任何人知道**。到真机 M3 上才发现真模型不按协议出牌，排查成本高得多。

## 先说清楚一件事，免得你走错方向

**live 模式下，`evals/p0/*.yaml` 里的 `expect` 判据必然大面积红。这是预期的，不是 bug。**

看一眼 `evals/p0/01_simple_qa.yaml` 就明白：

```yaml
expect:
  - {check: platform_calls, method: send_text, equals: 1}
  - {check: text, where: last, contains: 北京今天晴}
```

`contains: 北京今天晴` 是照 `model_script` 里那句台词写的。真模型不可能复现这句话。
`aite/evals/__main__.py:42` 的注释也写明了：`--model live` **"不属于 P0 验收路径"**。

所以这一轨的产出**不是**"让 live 跑出 passed 10/10"。谁也别为了凑绿去改场景判据 ——
那些判据钉的是 scripted 路径的 P0 验收（B8），改了就是把 P0 的验收面拆了。

**这一轨要回答的是另一个问题：真模型能不能按协议出牌，不能的地方 §3.3 的兜底够不够。**

---

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t12
分支     : task-t12
基线     : f9d45ab   ← main 的 HEAD（完整 sha f9d45ab1f2899a7f297911d60acafc1fa0c31b87）
Python   : python3.12（3.12.1）—— venv 要你自己建，见下
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，
**必须用 `python3.12`**，否则 `pip install -e` 直接失败：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

（并行的 T11 那个 worktree 我已经装好了，这个没装。文档正文里的 `python` 字样照原样保留，
只有本机执行时才换 `.venv/bin/python`。）

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 f9d45ab
git rev-parse --abbrev-ref HEAD         # 期望 task-t12
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check    # 期望 OK 11 files，退出码 0
.venv/bin/python -m pytest -q                      # 期望 936 passed, 1 xfailed
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python scripts/preflight.py --offline    # 期望退出码 0
```

**还有一条自检是验守卫的**（这条要"被拦"才算过）：

```
Read 工具读 .claude/hooks/guard_bash.py
```

期望**被 hook 拦下**，报 `blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）`。
被拦 = 守卫挂上了。**没被拦就说明 hook 失效了，停下报告** —— hook 失败是非阻塞放行且不报警，
守卫会静默失效，后面你改到契约都不会有人拦。

## 守卫会拦你的几种写法（我这一轮实撞出来的，照着避开省时间）

| 写法 | 结果 |
|---|---|
| Bash 命令里出现 `aite/contracts`、`.contracts.lock`、`docs/dev-spec-*.md` 字面量 | 拦停，**哪怕只是 `[ -e ]` 只读探测**。想看目录就 `ls aite/`，别写全路径 |
| `find ... -delete` / `find ... -exec` | 拦停（覆盖面判不出来）。逐个写明确路径 |
| `git commit -m "多行\n带引号的 message"` | 拦停（`No closing quotation`，shlex 解析失败）。**用 `git commit -F <文件>`** |

被拦是守卫在干活 —— 但**不要想办法绕过它**。真需要碰受保护面 = 停下报告。

## 这一轨分两段，第二段要总管给 key

`--model live` 需要三样东西，前两样在 `config/aite.yaml`（现在只有 `.example.yaml`，
`model.base_url` / `model.model` 都是空字符串），第三样是环境变量：

```
model.base_url         百炼 / 智谱的 OpenAI 兼容端点     ← 总管填
model.model            模型名                            ← 总管填
AITE_MODEL_API_KEY     环境变量，只放环境变量不进 yaml    ← 总管给
```

**段 A 不需要 key，先做。段 B 需要 key。** 拿不到 key 就把段 A 做完、
在回执里写明"段 B 待 key"，**不要伪造实测输出，不要拿 scripted 的结果冒充 live 的**。
没 key 时 `--model live` 会打 `--model live 起不来：<异常>` 并退 2 —— 这是它现在的正常行为。

---

## 段 A（不需要 key）：把观测装置搭起来

现在 live 跑完只会得到一堆红判据，看不出真模型到底怎么出牌的。段 A 就是补上这个眼睛。

需要能回答这些问题（形式你定，建议 `--json` 摘要里多一段，或者单独一个
`--protocol-report` 开关；**不要动现有 `--json` 已有字段的含义**，T4 的 CI 在读它）：

1. **每步调了什么**：步序 → tool_calls 的 name / arguments，有没有 `final`
2. **参数合不合 schema**：按 `ToolSpec.parameters` 校验每个 arguments，
   不合的记下来（这对应 §3.3"tool_call 参数不合 schema"那条，连续 3 次会 failed）
3. **调了协议外的名字吗**：不在 `ALL_MODEL_TOOLS` 里的工具名
4. **几步收敛**：到 `final` 用了几步，有没有撞 `max_steps`
5. **§3.3 的兜底有没被触发**，逐条点名：
   - "模型既无 tool_call 也无 `final`，只返回文本" → `steps==0` 时视为 `final`，`steps>0` 时回 system 提示
   - "参数不合 schema" → 回 `invalid_args`，连续 3 次 failed
   - 模型 5xx / 异常 → 重试 2 次（2s / 5s）

段 A 的自测**用 scripted 模型跑**（`--model scripted` 照样能产出这份报告），
这样没 key 也能验装置本身是对的。给它配测试放 `tests/e2e/`。

段 A 做完，`--platform fake --model scripted` 的 **passed 10/10 必须还是 10/10**。

## 段 B（需要 key）：真模型实跑

```bash
cp config/aite.example.yaml config/aite.yaml      # 填 model.base_url / model.model
export AITE_MODEL_API_KEY=…                        # 只放环境变量
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model live --json live-report.json
```

建议先 `--only 01_simple_qa` 单场景过一遍再跑全套，省 token。

要交的是一份**真模型出牌报告**，回执里写清：

- 10 个场景里，真模型有几个走到了 `final`，几个跑飞（撞 max_steps / 异常）
- 它调出来的工具名和参数，哪些不合 `ToolSpec.parameters`
- §3.3 那几条兜底，哪几条真的被触发了，触发后行为对不对
- **发现的兜底缺口**：真模型的某种出牌方式，现有兜底接不住的

缺口的修法**只能落在你的可写路径里**。如果结论是要改 `aite/worker/loop.py` 的 agent loop
（§3.3 的兜底大多在那里）—— **停下报告，别改**：并行的 T11 正在改
`aite/control/plane.py` 和 `aite/app.py`，`aite/worker/**` 也在它的影响半径里，
两轨同时改会撞。把缺口写进回执，由总管定序。

`config/aite.yaml` 里**绝不能写 api key 的值**（§3.1 config 那节：密钥只放环境变量名）。
它也不在你的可写路径里 —— 它是起飞配置，`.gitignore` 该挡住它，
**回执前 `git status --porcelain` 确认它没被 commit 进去**。

## 可写路径

```
aite/models/**
aite/evals/**            （不含 evals/p0/*.yaml 里现有 expect 判据，见下）
evals/**                 （可新增文件；现有 10 个场景的 expect 只读）
tests/e2e/**
```

**只读，改了就是任务失败**：
- `aite/contracts/**`、`.contracts.lock`（守卫也会拦）
- `docs/dev-spec-2026-09-09.md`
- `evals/p0/*.yaml` 里**现有的 `expect` 判据**（那是 B8 的 P0 验收面，scripted 的 10/10 靠它）
- `aite/worker/**`、`aite/control/**`、`aite/app.py`、`tests/integration/**`（**T11 正在动，别碰**）

按 spec §7：接口契约里没有 → **停，报告**；要改的文件不在白名单 → **停，报告**；
需要新的第三方依赖 → **停，报告**（`pyproject.toml` 归 T0）。

## 验收

```bash
.venv/bin/python -m pytest -q                     # 期望 936 passed, 1 xfailed（不许回归）
scripts/check.sh                                  # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python -m pytest tests/e2e -q           # 期望 ≥159 passed（你加了测试就更多）
.venv/bin/python -m aite.evals run evals/p0 --list   # 期望还是那 10 个场景名
```

`check.sh` 里的数字：A5 全仓可收集 **937 tests**、C1 契约测试 **335 passed**、
T4 替身自测 **159 passed**、B8 评测 **passed 10/10**。
`.contracts.lock --check` 必须始终 `OK 11 files`（C2）。变红 = 这一轨失败。

## 回执格式

```
## T12 回执

基线 f9d45ab → 提交 <短 sha>

### 段 A：观测装置
- <文件:行> <一句话>
- 怎么用：<命令>

### 段 B：真模型实测
拿到 key 了吗：<是，模型 xxx / 没有，待总管>
（拿到了才填下面）
- 走到 final 的场景：k/10
- 跑飞的：<场景名 + 原因>
- 不合 schema 的 arguments：<列出来>
- §3.3 兜底触发情况：<逐条>
- 发现的缺口：<有就点名，说清要改哪个文件（哪怕不在你的白名单里）>

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

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t12` 分支上，回执贴出来，
合并由总管在主仓做。`config/aite.yaml` 不要提交。
