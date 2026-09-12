# 任务 T7 — TΩ 组装：让 `python -m aite.app` 真的起飞

## 背景：这轨是从哪来的

T1–T6 六轨全部合进 main（最新 `34e9dee`）。现在全仓 **863 条测试全绿**
（827 非 docker + 36 docker），§3.8 的 10 个评测场景 **passed 10/10**。

但 `aite/app.py` 还是 T0 留的 stub：

```
$ .venv/bin/python -m aite.app
NotImplementedError: aite.app.main 由 TΩ 实现（dev-spec §3.4 归属表、§5 任务表）
退出码 1
```

**所有零件都造好了，没人把它们装起来。** §2.4 的人工验收 M1–M6（真实飞书群）
因此一条都跑不了 —— 这是 P0 收尾**唯一**的关键路径，你这轨就是它。

同时并行的还有三轨（T8 集成测试、T9 起飞前自检、T10 evidence 时间线），
它们都不碰 `aite/app.py`。**这个文件只有你能写。**

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t7
分支     : task-t7
基线     : 34e9dee   ← main 的 HEAD，T7–T10 钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。文档正文里的 `python` 字样照原样保留，
只有本机执行时才换 `.venv/bin/python`。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 34e9dee
git rev-parse --abbrev-ref HEAD         # 期望 task-t7
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
.venv/bin/python -m aite.app            # 期望 NotImplementedError，退出码 1 ← 你的起点
```

上面这些是我在这个 worktree 里实跑出来的值，不是估计。

**第 13 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 读到内容说明 PreToolUse 守卫没生效（多半是会话不是从
worktree 根启动的），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。
这种情况停下报告，别接着做。

## 先读

- `docs/dev-spec-2026-09-09.md` 的 §2.1/§2.4（DoD 与 M1–M6）、§3.4 归属表、§5 任务表。**不许改**。
- `aite/evals/wiring.py` 的 `build_deps` —— 已经有一份「组装」，只不过接的是替身。
  真实组装的形状照它，别另起一套风格。
- 你要接的九个真实实现，构造面都已冻结，**一个都不要改**：

  | 角色 | 类 | 模块 | 关键构造参数 |
  |---|---|---|---|
  | 平台 | `FeishuPlatform` | `aite.adapters.feishu` | `app_id / app_secret / bot_open_id / tenant_id / history_window` |
  | 会话库 | `SqliteSessionStore` | `aite.control.store` | `path`（位置参数） |
  | 证据 | `FileEvidenceWriter` | `aite.evidence.writer` | `evidence_dir`（位置参数） |
  | 沙箱 | `DockerSandbox` | `aite.sandbox.docker_sandbox` | 全默认即可 |
  | 工具网关 | `ToolGateway` | `aite.gateway.tool_gateway` | `platform / sandbox / sandbox_spec` |
  | 模型 | `OpenAICompatModel` | `aite.models.openai_compat` | `cfg: ModelConfig`（位置参数） |
  | Worker | `AgentWorker` | `aite.worker.loop` | `store / platform / model / evidence / config / gateway / sandbox` |
  | 控制面 | `InProcessControlPlane` | `aite.control.plane` | 上面那套 + `worker` |
  | 入口 | `Ingress` | `aite.ingress.handler` | `plane`（位置参数） |

- 收尾面（优雅退出要用，已存在）：`platform.stop()`、`plane.join()`、`plane.pending`、
  `store.close()`、`sandbox.aclose()`。**沙箱 reaper（W7）已经在 `plane.run_forever()`
  里了**，别再起一条。

## 可写路径

```
aite/app.py
```

**只有这一个文件。** 其余全仓只读。`aite/contracts/**`、`.contracts.lock`、
`docs/dev-spec-*.md` 有守卫拦着。缺什么、接不上 → 按 §7 **停下报告**，
不要「就改一行」别人的文件，也不要自己发明契约里没有的接口。

`tests/integration/**` 是 T8 的，`scripts/**` 是 T9/T10 的，别碰。

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
   `model` / `sandbox` 就用给的，不给才按 `config` 造真的 —— T8 靠这三个口子把
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

### 并轨规程（合并时怎么缝）

T8 也会照这份契约写一份最小组装（放在他自己的 `tests/integration/` 下）。
合并时**以你的 `aite/app.py` 为准**，T8 那份丢弃、只保留他的测试。
所以**签名、字段名、默认值、退出顺序照上面写死，别自己改名** ——
改了名 T8 的集成测试并轨后全红。

## 目标

1. 按 C-TΩ-1 实现 `build_app` / `run_app` / `main`。
2. `main()` 的命令行：`--config`（默认 `config/aite.yaml`）、`--traceback`。
   `--help` 要能跑通且退出码 0（没有凭证也能跑，这是 T8/CI 唯一能自动验的入口行为）。
3. 信号处理：SIGINT / SIGTERM 都进同一条优雅退出路径。第二次收到信号 → 立即硬退
   （别让人 Ctrl-C 按不动）。
4. 起飞日志打一行「接了谁」：platform / model 名 / sandbox 镜像 / sqlite 路径 /
   evidence 目录。真机排障时这一行是第一现场，别省。
5. `config.platform == "fake"` 时**不要**去造 `FeishuPlatform` —— 按 §3.1 config 的
   口径，那是留给评测/回放的取值。这种情况下没给 `platform=` 就报人话错误
   （「platform=fake 时必须由调用方注入平台实现」），别静默起一个连不上的东西。

## 验收

```bash
.venv/bin/ruff check .                          # All checks passed!
.venv/bin/python -m aite.contracts.lock --check # OK 11 files
.venv/bin/python -m pytest -q -m "not docker"   # 827 passed，一条不许红
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # passed 10/10
.venv/bin/python -m aite.app --help             # 退出码 0
.venv/bin/python -m aite.app --config /tmp/nope.yaml   # 人话报错 + 非零退出码，无裸栈
```

**没有飞书凭证也不要试图真起飞**，那是总管在 M1–M6 里跑的。你能自证的到
`--help` 和「缺配置时的报错姿态」为止；再往前的接线正确性归 T8 的集成测试。

⚠️ 你会很想给 `aite/app.py` 配一个自己的测试文件。**别建** ——
`tests/integration/**` 是 T8 的可写路径，你在那儿建文件并轨时必撞。

## 回执格式

做完在最后贴一段，格式照这个：

```
RECEIPT T7 status=done commit=<短 sha> checks=<过了几项>/<共几项> files=<改了几个文件>
```

另外用人话写清楚：

- 优雅退出的实际顺序，以及宽限期超时那一支你怎么让在跑的任务收场
- 契约 C-TΩ-1 有没有原样落地；有任何偏离逐条列出来 —— T8 的集成测试依赖它
- 组装过程中有没有发现某个零件的构造面对不上（比如某个 Port 少了 app 需要的东西）。
  **有就写清楚是哪个、缺什么**，别自己改别人的文件绕过去
- `config.platform == "fake"` 那条你怎么处理的

**不要 push，不要合 main。** 我这边统一并轨。
