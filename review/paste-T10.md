# 任务 T10 — evidence 时间线 `scripts/evidence_show.py` + 真机验收剧本

## 背景：这轨是从哪来的

T1–T6 六轨全部合进 main（最新 `34e9dee`）。全仓 **863 条测试全绿**，
§3.8 的 10 个评测场景 **passed 10/10**。P0 的最后一步是 §2.4 的人工验收
**M1–M6：在真实飞书群里跑**（并行的 T7 正在补 `aite/app.py` 的组装，让进程能起飞）。

真机验收和替身评测最大的不同：**出了问题没有 pytest 告诉你哪一行断言红了**，
只有群里一条没回的消息、一张卡在 working 的卡片。

好消息是每个任务都留了完整的证据链：`{evidence_dir}/{task_id}/events.jsonl`
一行一个 `EvidenceEvent`，前后用 sha256 串成 hash 链，收尾写 `manifest.json`。
坏消息是**它今天只有机器读得了** —— 一行几百字节的 JSON，payload 还只有 hash。

你这轨干两件事：**把证据链变成人读得懂的时间线**，
再把 **M1–M6 写成一份照着做就能跑的剧本**，把工具、观察点、期望串起来。

同时并行的还有三轨（T7 组装、T8 集成测试、T9 起飞前自检），
它们都不碰 `scripts/evidence_show.py`、`docs/acceptance-M.md` 和 `tests/tools/`。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t10
分支     : task-t10
基线     : 34e9dee   ← main 的 HEAD，T7–T10 钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。文档正文里的 `python` 字样照原样保留，
只有本机执行时才换 `.venv/bin/python`。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 34e9dee
git rev-parse --abbrev-ref HEAD         # 期望 task-t10
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check   # 期望 OK 11 files，退出码 0
.venv/bin/ruff check .                            # 期望 All checks passed!，退出码 0
.venv/bin/python -m pytest -q --co                # 期望 863 tests collected
.venv/bin/python -m pytest tests/contracts -q     # 期望 335 passed
.venv/bin/python -m pytest tests/evidence -q      # 期望 全绿（证据链的既有测试）
.venv/bin/python -m pytest -q -m "not docker"     # 期望 827 passed, 36 deselected，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
                                        # 期望 末行 passed 10/10，退出码 0
ls scripts/                             # 期望 只有 check.sh ← 你的起点
```

上面这些是我在同基线的 worktree 里实跑出来的值，不是估计。

**第 13 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 读到内容说明 PreToolUse 守卫没生效（多半是会话不是从
worktree 根启动的），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。
这种情况停下报告，别接着做。

## 先读

- `docs/dev-spec-2026-09-09.md` 的 **§2.4（M1–M6 逐条）**、§3.6 W1–W9（一个任务
  在证据里会留下什么痕迹）、§3.3（失败面）。**不许改这个文件。**
- `aite/contracts/evidence.py`：`EvidenceEvent` 的字段、`EvidenceKind` 的 11 个取值
  （`task_created` / `event_received` / `model_call` / `checklist_op` / `tool_call` /
  `tool_result` / `artifact` / `delivered` / `failed` / `cancelled`）、
  `canonical_json` / `chain_hash` / `GENESIS`。
- `aite/evidence/writer.py` 的 `FileEvidenceWriter`：`task_dir` / `events_path` /
  `manifest_path` / `verify(task_id)`，以及 payload > 64KB 时外置成
  `payloads/{seq}.json` 的规矩 —— 你的渲染要认得这种外置 payload。
- `scripts/check.sh` —— 现有脚本的风格（输出格式、退出码），你的脚本照这个调子。

## 可写路径

```
scripts/evidence_show.py
docs/acceptance-M.md          ← 新建，只有这一个 docs 文件
tests/tools/**
```

其余全仓只读。**`aite/` 下一个文件都别改** —— 你是只读地消费证据目录。
`docs/dev-spec-2026-09-09.md` 有守卫拦着，**碰它即任务失败**。

`aite/app.py` 是 T7 的，`tests/integration/**` 是 T8 的，
`scripts/preflight.py` 与 `tests/scripts/**` 是 T9 的。都别碰。
`scripts/check.sh` 是既有文件，**不许改**。

## 目标一：`scripts/evidence_show.py`

```bash
python scripts/evidence_show.py <task_id>              # 从默认 evidence_dir 找
python scripts/evidence_show.py --dir data/evidence/<task_id>
python scripts/evidence_show.py --list                 # 列出目录下所有任务（时间倒序）
```

渲染一条人读的时间线，每个事件一行，至少给到：序号、相对起点的耗时、kind、
以及**按 kind 挑出来的那几个真正有用的字段**：

- `model_call` → 模型名、finish_reason、usage（prompt/completion/total）、按 config
  里的单价算出的这一步花费
- `tool_call` → 工具名 + 参数摘要（长的截断）
- `tool_result` → ok / error_code、duration_ms
- `checklist_op` → 操作与那一项的文本
- `artifact` → title、mime、size、sha256 前 8 位
- `delivered` / `failed` / `cancelled` → 终态那一行要显眼

末尾一段汇总：事件总数、模型调用次数、token 合计与花费合计、工具调用次数与失败数、
产出文件数、**hash 链校验结果**（逐条重算 `chain_hash(prev, payload_hash)`，
并把最后一条的 `hash` 与 `manifest.json` 的 `root_hash` 比对）。

硬要求：

1. **hash 链校验是这个工具的骨头，不是装饰。** 校验不过时要指出**断在第几条**、
   期望什么、实际什么，然后**非零退出码**。人在真机排障时最需要知道的就是
   「这份证据到底还可不可信」。
2. **没有 manifest.json 也要能渲染**（任务还在跑、或者进程被杀了没 finalize）。
   这种情况明确打印「未 finalize」，别当成损坏。
3. **payload 外置**（`payload_ref` 指向 `payloads/{seq}.json`）时要跟过去读；
   文件不在就标注「payload 缺失」，继续渲染剩下的，别整个崩掉。
4. `--json` 输出机器可读结构；`--only model_call,tool_call` 按 kind 过滤；
   `--tail N` 只看最后 N 条。
5. **不打印任何密钥、token、消息全文**。证据里本来就只存 hash 不存模型全文
   （W8），你别把这个口子开了。

`tests/tools/` 下配测试：拿 `FileEvidenceWriter` 在 `tmp_path` 里真写一份证据，
再跑渲染。至少钉住：正常任务渲染出的行数与汇总数字对得上、
**篡改一条 payload 后校验失败且指出了断点位置**、未 finalize 的目录能渲染、
外置 payload 读得到、缺失时不崩。

## 目标二：`docs/acceptance-M.md` —— M1–M6 真机验收剧本

把 §2.4 那张表变成照着做就能跑的东西。每条 M 写成四段：

- **操作**：在群里具体做什么（消息原文、附件、点哪个按钮）
- **在哪看**：进程日志的哪个关键字、`scripts/evidence_show.py` 看哪个 task、
  `docker ps --filter label=aite.task` 看什么
- **期望**：具体到能判定真假（"卡片至少更新 3 次且不新增消息" 要说清在哪数）
- **不对时查哪**：按最可能的原因排序，每条指向一个具体动作

开头补一段**通用前置**：先跑 `scripts/preflight.py`（T9 正在做这个脚本，
你按它派单里的命令行写，跑不了就先按约定写）、再 `python -m aite.app` 起飞
（T7 正在做组装）、日志看什么。

⚠️ **T7/T9 的东西你现在跑不了**（他们和你并行）。剧本里引用它们的命令时，
按各自派单里写死的命令行写，**并在文档里标一行「本节命令待 T7/T9 合入后实跑校验」**。
别为了能跑而自己去实现它们的功能。

⚠️ §3.7 有两条**待核实**的飞书权限（话题里不带 @ 的回复是否投递、群历史是否需要
敏感权限），它们直接决定 **M4 怎么操作**。剧本里把两种情况都写出来，别赌一种。

## 验收

```bash
.venv/bin/ruff check .                          # All checks passed!
.venv/bin/python -m aite.contracts.lock --check # OK 11 files
.venv/bin/python -m pytest tests/tools -q       # 你新增的，全绿
.venv/bin/python -m pytest -q -m "not docker"   # 827 + 你新增的条数，一条不许红
.venv/bin/python scripts/evidence_show.py --help    # 退出码 0
```

再自己造一份证据实跑一遍渲染（`tmp_path` 或 `/tmp` 下，**别往仓库 `data/` 落**），
把输出贴进回执 —— 我要看这个工具打出来到底长什么样。

## 回执格式

做完在最后贴一段，格式照这个：

```
RECEIPT T10 status=done commit=<短 sha> checks=<过了几项>/<共几项> files=<改了几个文件>
```

另外用人话写清楚：

- `evidence_show.py` 对一个**完整任务**的实际输出（原样贴，长就截中间）
- hash 链断掉时的输出长什么样（造一个篡改的例子，原样贴）
- 渲染时你发现证据链里**缺了什么真机排障需要的信息**吗 —— 有就逐条列出来，
  这是给下一轮改 `aite/evidence/` 的输入（**但你不许自己改**）
- `docs/acceptance-M.md` 里哪些命令是"待 T7/T9 合入后实跑校验"的
- §3.7 那两条待核实项，你在 M4 那节是怎么写的

**不要 push，不要合 main。** 我这边统一并轨。
