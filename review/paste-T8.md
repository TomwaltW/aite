# 任务 T8 — 集成测试：进程级接线（`tests/integration/`）

## 背景：这轨是从哪来的

T1–T6 六轨全部合进 main（最新 `34e9dee`）。全仓 **863 条测试全绿**，
§3.8 的 10 个评测场景 **passed 10/10**。

但现在绿的东西有一条共同的缝没缝上：**每一轨都是拿自己的替身测自己那一格**。
评测 runner（`aite/evals/wiring.py`）虽然把六轨串起来跑，用的却是全套替身 ——
`FakeSessionStore` 是内存 dict，`FakeEvidenceWriter` 不落盘。

于是这些问题今天没有任何测试能答：

- SQLite 真落盘之后，**换一个进程**还能不能续接同一个话题？（§2.2 B6 只验了同进程换实例）
- evidence 真写到磁盘之后，hash 链在**文件里**还对不对？manifest 的 `root_hash` 对不对得上？
- 进程收到 SIGTERM，**在跑的任务**会怎么样？沙箱还回去了吗？

`tests/integration/` 目录是空的，归 TΩ。你这轨把它填上。

## 时序：你和 T7 是并行的

T7 正在写 `aite/app.py`（组装本体），**你开工时他还没交**。
所以你要照冻结契约 C-TΩ-1 **自己也写一份最小组装**，放在你自己的
`tests/integration/` 下，让你这一轨不等任何人就能跑绿。并轨时你那份会被丢掉 ——
这是有意的，不是浪费（T5/T6 那轮就是这么并的，成了）。

另外两轨（T9 起飞前自检、T10 evidence 时间线）不碰你的路径。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t8
分支     : task-t8
基线     : 34e9dee   ← main 的 HEAD，T7–T10 钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。文档正文里的 `python` 字样照原样保留，
只有本机执行时才换 `.venv/bin/python`。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 34e9dee
git rev-parse --abbrev-ref HEAD         # 期望 task-t8
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check   # 期望 OK 11 files，退出码 0
.venv/bin/ruff check .                            # 期望 All checks passed!，退出码 0
.venv/bin/python -m pytest -q --co                # 期望 863 tests collected
.venv/bin/python -m pytest tests/contracts -q     # 期望 335 passed
.venv/bin/python -m pytest -q -m "not docker"     # 期望 827 passed, 36 deselected，退出码 0
.venv/bin/python -m pytest tests/e2e -q           # 期望 159 passed
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
                                        # 期望 末行 passed 10/10，退出码 0
ls tests/integration                    # 期望 空目录 ← 你的起点
```

上面这些是我在同基线的 worktree 里实跑出来的值，不是估计。

**第 13 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 读到内容说明 PreToolUse 守卫没生效（多半是会话不是从
worktree 根启动的），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。
这种情况停下报告，别接着做。

## 先读

- `docs/dev-spec-2026-09-09.md` 的 §2.2（B6 / B7）、§3.5 R6、§3.6 W5/W7。**不许改**。
- `aite/evals/wiring.py` 的 `build_deps` —— 替身版组装长什么样，你的最小组装照它。
- `aite/control/store.py`（`SqliteSessionStore`）、`aite/evidence/writer.py`
  （`FileEvidenceWriter`）—— 你要真用的两个落盘实现。
- `aite/testing/`（`FakePlatform` / `FakeModel` / `FakeSandbox`）—— 平台和模型用替身，
  **T4 的这套是公开可用的**，不用自己再造。

## 可写路径

```
tests/integration/**
```

**只有这个目录。** 其余全仓只读 —— 特别是 **`aite/app.py` 是 T7 的，绝对别碰**，
你在那儿改一行并轨时必撞。`aite/` 下任何实现文件也都不许动：集成测试红了是发现问题，
**报告它**，不是就地把被测代码改绿（§7）。

`scripts/**` 是 T9/T10 的，`tests/scripts/**` 是 T9 的，`tests/tools/**` 是 T10 的。

## 冻结契约 C-TΩ-1：app 组装面

⚠️ **这一节在 T7 与 T8 的派单里逐字一致。由 T7 实现，两轨共用。**

```python
@dataclass
class AiteApp:
    config:   AiteConfig
    platform: PlatformPort
    store:    SessionStore
    evidence: EvidenceWriter
    sandbox:  SandboxPort | None
    gateway:  ToolGateway | None
    model:    ModelPort
    worker:   AgentWorker
    plane:    Any            # InProcessControlPlane
    ingress:  Ingress


def build_app(
    config: AiteConfig,
    *,
    platform: PlatformPort | None = None,
    model:    ModelPort | None = None,
    sandbox:  SandboxPort | None = None,
) -> AiteApp: ...


async def run_app(app: AiteApp, *, stop: asyncio.Event | None = None) -> None: ...


def main() -> None: ...
```

三条硬约束：

1. **`build_app` 只组装，不产生副作用。** 不连网、不起容器、不发消息；SQLite 与
   evidence 目录允许按配置创建（那是落盘路径的必要准备）。给了 `platform` /
   `model` / `sandbox` 就用给的，不给才按 `config` 造真的 —— 你靠这三个口子把
   替身塞进来测真实接线。
2. **`run_app` 的退出序列写死成这个顺序**，每一步都不许跳：

   ```
   platform.start(ingress.on_event)  →  plane.run_forever() 挂后台
   等 stop（SIGINT / SIGTERM，或调用方传进来的 Event）
   platform.stop()                   ← 先闭嘴，不再收新事件
   plane.join() 限时 shutdown_grace_sec 秒   ← 在跑/排队的任务收尾
   超时 → 取消 run_forever 那条 task（worker 的 cancel 路径会还沙箱、把卡片置 cancelled）
   sandbox.aclose()（有沙箱才调）
   store.close()
   ```

   宽限期 `shutdown_grace_sec` 默认 **20.0**（对齐 `docker-compose.yml` 的
   `stop_grace_period: 20s`），做成 `run_app` 的关键字参数。
3. **`main()` 失败要给人话，不许抛裸栈。** 配置读不到、环境变量没设、
   `base_url` / `model` 是空串这类起飞前就能看出来的问题，一律打一行中文说清
   缺什么、怎么补，然后**非零退出码**。裸 traceback 只在 `--traceback` 时打 stderr。

### 你要怎么用它

照上面的签名，在 `tests/integration/` 下自己写一份最小 `build_app` / `run_app`
（名字随你，别叫 `aite.app` 就行），让你的测试现在就能跑。并轨时**以 T7 的
`aite/app.py` 为准**，你那份丢掉。所以有两件事必须做到，否则并轨后你的测试全红：

- **测试只依赖 C-TΩ-1 里写死的那些名字**：`AiteApp` 的字段名、`build_app` 的三个
  关键字口子、`run_app(app, stop=...)`、退出顺序、`shutdown_grace_sec=20.0`。
- **别依赖你自己那份组装的内部细节**（私有函数名、你加的辅助类、异常类型）。
  测试里想断言的东西，一律从 `AiteApp` 的公开字段和落盘结果上看。

并轨那天我会把你的 `tests/integration/` 原样接到 T7 的 `aite/app.py` 上，
只改 import 那一行。改完必须一条不红 —— 你写测试时就按这个前提写。

## 目标

四组，每组至少一条真跑起来的测试（全部用 `tmp_path`，**不许往仓库里的 `data/` 落盘**）：

1. **SQLite 真跨进程**（B6 的进程级版本）：
   一套组装跑完一个任务 → `store.close()` → **用同一个 .db 文件新建第二套组装** →
   同一 `thread_id` 的追问命中同一 `session_id`，`list_turns` 含此前的轮次。
   ⚠️ 「新建第二套」指真的重新 `build_app`，不是复用同一个 store 实例。
2. **evidence 真落盘**（B7 的文件版）：
   任务跑完后 `{evidence_dir}/{task_id}/events.jsonl` 一行一个事件、
   `manifest.json` 的 `root_hash` 等于最后一条的 hash、`event_count` 对得上；
   拿 `aite/evidence` 现成的 verify 路径校验为 True；**改坏文件里任一行的 payload
   后校验为 False**。
3. **优雅退出**：`run_app` 起来后置 `stop` →
   `platform.stop()` 被调过（替身上看得见）、在跑的任务收了尾、`store.close()` 调过。
   收尾期间**新到的事件不再被处理**。
4. **宽限期超时**：造一个收不完的任务（`FakeModel` 的 `hold_ticks` 或 `repeat: inf`
   都行，看哪个更稳），`shutdown_grace_sec` 设成很小的值 →
   任务被取消、沙箱 `release` 被调过、`run_app` 仍在有限时间内返回（不许挂死）。

写的时候记住：**这几条是拿来在真机验收前兜底的**，别为了绿把断言写松。
测出来的红如果是被测代码的问题 —— 那正是这轨的价值，**写进回执报告**，别自己去改
`aite/` 下的实现。

## 验收

```bash
.venv/bin/ruff check .                          # All checks passed!
.venv/bin/python -m aite.contracts.lock --check # OK 11 files
.venv/bin/python -m pytest tests/integration -q # 你新增的，全绿
.venv/bin/python -m pytest -q -m "not docker"   # 827 + 你新增的条数，一条不许红
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # passed 10/10
```

最后一条特别要跑：你的测试要是往仓库 `data/` 里落了脏文件，评测和别的用例会被带红。

跑完再连跑 3 遍 `pytest tests/integration -q` 确认不是时序 flaky —— 这一轨天生
跟 asyncio 的调度打交道，一遍绿不算数。

## 回执格式

做完在最后贴一段，格式照这个：

```
RECEIPT T8 status=done commit=<短 sha> checks=<过了几项>/<共几项> files=<改了几个文件>
```

另外用人话写清楚：

- 四组各写了几条、分别钉住了什么
- **有没有测出被测代码的真问题**（这是这轨最值钱的产出）。有就逐条写：现象、
  哪个文件哪一行、你判断的根因、以及为什么你没有自己改
- 你那份最小组装与 C-TΩ-1 有没有偏离 —— 有就逐条列出来，这直接决定并轨后你的测试还绿不绿
- 你的测试里有没有哪条依赖了你自己那份组装的内部细节（并轨后会被丢掉）
- 连跑 3 遍的结果

**不要 push，不要合 main。** 我这边统一并轨。
