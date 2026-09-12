# 任务 T4 — 测试替身 + ModelPort + 评测 runner + 10 个场景 + CI（Aite P0）

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t4
分支     : task-t4
基线     : 0fa8348   ← main 的 HEAD，四轨钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。写 CI 时按 spec 用 `python`，
本机执行时才换 `.venv/bin/python` —— 不要为了「统一」去改文档正文里的 `python` 字样。

Docker 环境已确认可用（`docker compose` v5.3.0，aarch64），`docker compose config` 跑得起来。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 0fa8348
git rev-parse --abbrev-ref HEAD         # 期望 task-t4
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check      # 期望 OK 11 files，退出码 0
.venv/bin/ruff check .                               # 期望 All checks passed!，退出码 0
.venv/bin/python -m pytest -q --co                   # 期望 335 tests collected
.venv/bin/python -m pytest tests/contracts -q        # 期望 335 passed
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
                                        # 期望 打印 not implemented，退出码 2（T0 留的 stub）
```

上面这些是实测值，不是估计。

**第 10 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 读到内容说明 PreToolUse 守卫没生效（多半是会话不是从 worktree
根启动的），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。这种情况停下报告。

## 先读

`docs/dev-spec-2026-09-09.md` 全文。本轮唯一权威文档，**不许改**。
重点：§3.1 全部（替身要覆盖所有模型）、§3.2 全部（替身要实现所有 Port）、
§3.4 归属表 T4 那行、**§3.5 路由 R1–R8 与 §3.6 Worker W1–W9**（场景据此写）、
**§3.8 的 10 个场景表**、§6 的 T4 那一行。

## 可写路径（§3.4，只有这九条）

```
aite/models/**   aite/testing/**   aite/evals/**   evals/**   tests/e2e/**
.github/workflows/**   docker-compose.yml   Makefile   README.md   scripts/**
```

其余一律只读。`aite/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` 有守卫拦着。
别的轨的目录（`aite/adapters/`、`aite/control/`、`aite/worker/`、`aite/sandbox/`、
`aite/gateway/`）对你**不可见**。

现有的 T0 占位，**都是你的文件，随便改**：
- `aite/models/openai_compat.py` 的 `OpenAICompatModel`（签名与 §3.2 `ModelPort` 对齐）
- `aite/evals/__main__.py` 的 runner stub（`run` 打印 not implemented 退出 2、`--list` 打空表）
- `aite/testing/` 只有 `__init__.py`
- `docker-compose.yml`（`services: {}`）、`Makefile`、`README.md` 都是最小占位
- `.github/workflows/`、`evals/p0/`、`scripts/` 只有 `.gitkeep`

## 目标

五块：

1. **官方测试替身** `aite/testing/`——`FakePlatform` / `FakeModel` / `FakeSandbox`
   （按需再加假 `ToolGateway` / `SessionStore`）。
   ⚠️ **并行期间 T1/T2/T3 不会 import 你这个包**（§3.4 明令），他们各写各的私有替身。
   你这份是给 `tests/e2e/`、evals runner 和 TΩ 用的。所以：
   - `FakePlatform` 要能**记账**：`send_card` / `update_card` / `send_text` /
     `send_file` / `add_reaction` 的调用次数与参数，场景断言全靠它
   - `FakeModel` 按 `model_script` **脚本化出牌**（每个场景 yaml 里带一份）
   - 替身必须严格按 §3.2 的签名实现，不能「差不多」

2. **ModelPort（live）** `aite/models/`——OpenAI-compatible 客户端，
   指向百炼 / 智谱。base_url、model、api_key **从 `AiteConfig.model` 读**，
   密钥走 `api_key_env` 指定的环境变量名（默认 `AITE_MODEL_API_KEY`），
   **绝不把密钥写进代码或配置文件**。
   把契约的 `Message` / `ToolSpec` / `ModelTurn` / `Usage` 与 OpenAI 的
   messages / tools / choices / usage 互相翻译。`cost` 按
   `price_in_per_mtok` / `price_out_per_mtok` 算（只用于卡片上的「已用 ¥」）。
   §3.3 有一条兜底要照顾到：**模型既无 tool_call 也无 final、只返回文本**时，
   `steps==0` 视为 `final(reply=文本)`，`steps>0` 回 system 提示并计 1 步 ——
   这条逻辑归 T2 的 worker，但你的 `FakeModel` 要能造出这种出牌让 T2/TΩ 验。

3. **评测 runner** `aite/evals/`——
   ```
   python -m aite.evals run evals/p0 --platform fake --model scripted
   python -m aite.evals run evals/p0 --list
   ```
   `--list` 列出 §3.8 的 10 个场景名。`run` 跑完输出 **JSON 摘要 + 最后一行
   `passed k/10`**。**并行期间 k 可以是 0，但每个场景必须报出失败原因，
   不能是异常栈** —— 别的轨还没合进来，import 不到是预期内的，要优雅降级。
   TΩ 阶段才要求 `passed 10/10` 且退出码 0（B8）。

4. **10 个 P0 场景** `evals/p0/*.yaml`——§3.8 那张表逐个，每个含 `model_script`：
   ```
   01_simple_qa          第一步就 final → 只有 1 条 send_text，没有卡片
   02_thread_followup    话题内第二条消息（不带 @）→ 同一 session_id，新 task
   03_checklist_progress send_card×1、update_card>=3、无第二条卡片
   04_csv_to_chart       附件 → download_attachment → run_python → final(artifacts)
                         → send_file×1 且内容为 PNG
   05_history_summary    read_group_history 返回含 bot 消息的历史 → 工具结果里无 bot 消息
                         → 回复引用 >=3 个 message_id
   06_read_document      read_document → 回复含文档标题
   07_commands           !status 列出活跃任务；!stop 后 task cancelled、release 被调用
   08_step_limit         模型只会重复 checklist_note → max_steps 处 failed
   09_bot_ignored        sender_kind=bot 的 @ → 无会话、无出站
   10_duplicate_event    同一 event_id 投两次 → 只建一个 task
   ```

5. **CI / compose / Makefile / README**——
   `.github/workflows/` 至少含 **A3–A5 三步**（`lock --check` / `ruff check .` /
   `pytest -q --co`）。`docker-compose.yml` 要让 `docker compose config` 退出码 0。
   README 写清怎么跑起来（现在是 T0 留的 11 行占位）。

## 不要做的事

- 不要实现 adapter / ControlPlane / worker / 沙箱 / Gateway —— 那是别人的格子。
  你的场景在别的轨合进来之前跑不满，这是**预期内**的，按第 3 条优雅降级即可，
  不要为了让场景变绿去自己实现别人的东西。
- 不要动 `tests/conftest.py`（T0 的 worktree sys.path 引导）。你的 fixture 放
  `tests/e2e/conftest.py`。
- 不要加 §3.0 以外的依赖 → 停，报告。
- 不要做多模型路由 / 快模型 verifier —— P1。
- 契约里找不到需要的接口 → **停，报告**。

## 验收（自己跑，全绿才算完）

必须通过 **A2–A5, C1, C2**：

```bash
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # A2  p0.1
.venv/bin/python -m aite.contracts.lock --check          # A3/C2  始终 OK 11 files
.venv/bin/ruff check .                                   # A4  退出码 0
.venv/bin/python -m pytest -q --co                       # A5  退出码 0
.venv/bin/python -m pytest tests/contracts -q            # C1  >= 335 passed，不得减少
```

独有的额外验收（§6 T4 原文）：

```bash
.venv/bin/python -m pytest tests/e2e -q                  # 替身自测全绿
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
      # 跑完 10 个场景，JSON 摘要 + 最后一行 passed k/10（k 可为 0）
      # 每个场景必须报出失败原因而不是异常栈
.venv/bin/python -m aite.evals run evals/p0 --list        # 列出 §3.8 的 10 个场景名
docker compose config                                     # 退出码 0
```
外加：CI 工作流含 A3–A5 三步。

**⚠️ 关于 `aite/evals/__main__.py` 的一个坑，已经替你拆掉了**：
T0 一度在 `tests/contracts/` 里断言「`evals run` 必须退出 2 并打印 not implemented」、
「`--list` 必须打印空列表」。那两条和你上面的验收**直接互斥**——你把 runner 实现了它们
就红，而 `tests/contracts` 归 T0 独占、你只读，C1 又不许通过数减少。**T0 已经把这类
「冻结未实现状态」的断言全部撤掉了**，基线 0fa8348 里没有它们。你放心实现。

同理，`aite/app.py` 的 `main()` 现在还是 `raise NotImplementedError`，那是 TΩ 的活，
也没有测试冻着它。

**C1 说明**：非改 `tests/contracts` 不可才能变绿 → **停，报告**，那是判据错了，不是你错了。

**测试文件命名**：`tests/` 下没有 `__init__.py`，同名文件在并轨时会让 pytest 报
import file mismatch。你的都在 `tests/e2e/` 下，别起 `test_utils.py` / `test_client.py`
这种四条轨都可能撞的名字。

## 卡住了怎么办（§7）

1. 需要的接口在契约里找不到 → **停，报告**。不要自己发明。
2. 需要改的文件不在白名单里 → **停，报告**。
3. 需要新的第三方依赖 → **停，报告**（`pyproject.toml` 归 T0）。
4. 验收命令跑不起来（环境问题）→ 先自己排查，排查不动 → 报告。
5. 验收过不了但代码自认为对 → **以验收为准**；确信验收写错了 → 停，报告。

## 完成后

```bash
git add -A && git commit -m "T4: 测试替身 + ModelPort + 评测 runner + CI"
```

不要 push，不要合并到 main —— 并轨归 TΩ。

然后输出回执，最后一行严格按这个格式：

```
RECEIPT T4 status=<done|blocked> commit=<短hash> checks=<通过数>/5 files=<改动文件数>
```

（checks 的分母 5 = A2 / A3 / A4 / A5 / C1。）

回执正文包含：
- 上面每条命令的实际输出（关键几行，不是「通过了」）
- C1 的实际条数，与基线 335 对比
- `--list` 的实际输出（10 个场景名）
- `run` 的实际输出：JSON 摘要 + 最后一行 `passed k/10`，**以及 10 个场景各自的失败原因**
  （并行期跑不满是正常的，要的是「每条都说清为什么」）
- `docker compose config` 的退出码；CI 工作流的三步
- 替身覆盖了 §3.2 的哪几个 Port、哪些还没写
- `git show --stat HEAD` 的文件树
- 如果 blocked：卡在哪、试过什么、需要什么决定
