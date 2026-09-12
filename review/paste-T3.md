# 任务 T3 — Docker 沙箱 + Tool Gateway（Aite P0）

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t3
分支     : task-t3
基线     : 0fa8348   ← main 的 HEAD，四轨钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。

**Docker 环境已确认可用**（起飞前实测）：Docker Desktop 29.6.1，`docker compose` v5.3.0，
架构 **aarch64（Apple Silicon）**，本地已有 `python:3.11-slim` 镜像。
注意 arch：镜像里装 pandas / matplotlib 要用 arm64 wheel，别指定 amd64 基底。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 0fa8348
git rev-parse --abbrev-ref HEAD         # 期望 task-t3
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check      # 期望 OK 11 files，退出码 0
.venv/bin/ruff check .                               # 期望 All checks passed!，退出码 0
.venv/bin/python -m pytest -q --co                   # 期望 335 tests collected
.venv/bin/python -m pytest tests/contracts -q        # 期望 335 passed
docker info --format '{{.ServerVersion}}'            # 期望 29.6.1（daemon 活着）
```

上面这些是实测值，不是估计。

**第 10 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 读到内容说明 PreToolUse 守卫没生效（多半是会话不是从 worktree
根启动的），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。这种情况停下报告。

## 先读

`docs/dev-spec-2026-09-09.md` 全文。本轮唯一权威文档，**不许改**。
重点：§3.1 的 `sandbox.py` / `gateway.py` / `protocol.py`（`GATEWAY_TOOLS` 那 5 个工具的
名字与 schema **已冻结**）、§3.2 的 `SandboxPort` / `ToolGateway`、§3.3 失败面里
工具与沙箱那几行、§3.4 归属表 T3 那行、§6 的 T3 那一行。

## 可写路径（§3.4，只有这六条）

```
aite/sandbox/**    aite/gateway/**    aite/tools/**
docker/sandbox/**  tests/sandbox/**   tests/gateway/**
```

其余一律只读。`aite/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` 有守卫拦着。
别的轨的目录（`aite/adapters/`、`aite/control/`、`aite/worker/`、`aite/models/`）对你**不可见**。

你目录下的两个 T0 空实现（`aite/sandbox/docker_sandbox.py` 的 `DockerSandbox`、
`aite/gateway/tool_gateway.py` 的 `P0ToolGateway`）签名与 §3.2 逐字对齐过，
**是你的文件，随便改**。`aite/tools/` 现在只有 `__init__.py`，5 个 Gateway 工具的实现放这儿。

## 目标

三块：

1. **沙箱镜像** `docker/sandbox/` → `docker build -t aite-sandbox:p0 docker/sandbox`。
   镜像内必须：`import pandas, matplotlib, openpyxl, docx` 全部成功；
   **含中文字体**（`fc-list :lang=zh` 非空 —— 否则 matplotlib 画中文出方块，
   这是 M3 演示的成败点）；**无网络**。

2. **SandboxPort（Docker）**——`aite/sandbox/`。
   - `acquire(task_id, spec)` 返回 `sandbox_id`；容器**必须打标签 `aite.task=<task_id>`**
     （B4 靠 `docker ps -a --filter label=aite.task` 验收）
   - `SandboxSpec.network` 是 `Literal["none"]`，容器**无网络**：凭证与平台调用
     都不在沙箱里发生
   - 工作目录 `/work`；`put_file` / `get_file` 的 path **必须在 `/work` 下**
   - `exec` 返回 `ExecResult`，stdout/stderr 超 `MAX_EXEC_OUTPUT_CHARS`（20000）
     要截断并置 `truncated=True`；`files_out` 是本次新增/修改的 `/work` 下文件
   - `release` **幂等**；`touch` 刷新最近活动时间；
     `reap_idle(idle_sec)` 释放空闲超时的并返回被释放的 id 列表

3. **ToolGateway + 5 个工具**——`aite/gateway/` + `aite/tools/`。
   `catalog(ctx)` 在 P0 就是 `GATEWAY_TOOLS` **原样**返回。
   `call(ctx, req)` 的顺序**照 §3.2 的注释逐字执行**：
   ```
   校验 session_token → 查工具存在 → 按 ToolSpec.parameters 校验 arguments
   → 执行（带超时）→ 截断 content → 返回
   ```
   **永远不抛异常给调用方**，所有失败一律 `ToolResult(ok=False, error=ToolError(...))`。
   错误码对应关系（B5 逐条验）：
   ```
   未知工具              → not_found
   参数不合 schema        → invalid_args
   session_token 不匹配   → denied
   工具超时              → timeout
   平台/外部系统错误      → upstream
   沙箱创建/执行失败      → sandbox
   ```
   超时：默认 `DEFAULT_TOOL_TIMEOUT_SEC`（60）；`run_python` 用请求里的 `timeout_sec`。
   `content` 截断到 `MAX_TOOL_CONTENT_CHARS`（12000）。

   5 个工具（名字与 schema **冻结**，见 §3.1 `GATEWAY_TOOLS`）：
   - `read_group_history` —— **必须过滤掉 `sender_kind != human`**。
     注意 `PlatformPort.read_history` 按契约**不做**这个过滤，过滤是你这一层的责任。
   - `read_document` —— 返回 markdown 文本
   - `download_attachment` —— 下到沙箱 `/work/in/` 下，返回路径；
     adapter 下载失败时返回 `upstream`
   - `run_python` —— 在沙箱执行，工作目录 `/work`
   - `list_files` —— 列 `/work` 下的文件

   平台侧的事（读历史、读文档、下载附件）要通过 `PlatformPort` 做，但 **adapter 是 T1 的、
   对你不可见**。所以在 `tests/gateway/` 下写你自己的**私有假 PlatformPort**，
   按 §3.2 的签名实现。

## 不要做的事

- 不要实现 adapter / ControlPlane / worker / 模型客户端。
- **不要 `import aite.testing`**（T4 的官方替身，并行期间还是空包）。
  FakeSandbox / 假 PlatformPort 在 `tests/sandbox/`、`tests/gateway/` 下写私有版本。
- pytest fixture 放自己目录的 `conftest.py`，**不要动 `tests/conftest.py`**（T0 的）。
- 不要加 §3.0 以外的依赖 → 停，报告。**镜像内的 pip 包不受这条约束**
  （那是 `docker/sandbox/` 里的事，不进 `pyproject.toml`）。
- 不要做 gVisor / K8s / 域名白名单 / 凭证注入 —— 全是 P1。
- 契约里找不到需要的接口 → **停，报告**。

## 验收（自己跑，全绿才算完）

必须通过 **A2–A5, C1, C2, B4, B5**：

```bash
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # A2  p0.1
.venv/bin/python -m aite.contracts.lock --check          # A3/C2  始终 OK 11 files
.venv/bin/ruff check .                                   # A4  退出码 0
.venv/bin/python -m pytest -q --co                       # A5  退出码 0
.venv/bin/python -m pytest tests/contracts -q            # C1  >= 335 passed，不得减少
.venv/bin/python -m pytest tests/sandbox -q -m docker     # B4
.venv/bin/python -m pytest tests/gateway -q               # B5
```

各条的期望（§2.2 原文）：

- **B4** `run_python` 跑 matplotlib 生成 `/work/out.png` → `get_file` 返回的
  **前 8 字节为 PNG 魔数**；`reap_idle(1)` 后 `docker ps -a --filter label=aite.task` 为空
- **B5** 未知工具 → `not_found`；参数不合 schema → `invalid_args`；
  工具超时 → `timeout`；`session_token` 不匹配 → `denied`

独有的额外验收（§6 T3 原文）：

```bash
docker build -t aite-sandbox:p0 docker/sandbox                       # 成功
docker run --rm aite-sandbox:p0 python -c "import pandas, matplotlib, openpyxl, docx"   # 成功
docker run --rm aite-sandbox:p0 fc-list :lang=zh                     # 非空
docker run --rm --network none aite-sandbox:p0 \
  python -c "import urllib.request;urllib.request.urlopen('https://example.com',timeout=3)"   # 必须失败
```
外加：`read_group_history` 过滤掉 `sender_kind != human`（在 B5 或单独用例里断言）。

`pyproject.toml` 里 pytest 已经声明了 `markers = ["docker: 需要本机 Docker daemon 的测试"]`，
`-m docker` 直接可用，不用自己注册。

**C1 说明**：`tests/contracts` 是 T0 独占、你只读。T0 已经把「stub 必须还是空实现」那类
断言全部撤掉了，所以你把两个 Port 写成真的**不会**打红 C1。非改 `tests/contracts` 不可
才能变绿 → **停，报告**，那是判据错了。

**测试文件命名**：`tests/` 下没有 `__init__.py`，同名文件在并轨时会让 pytest 报
import file mismatch。别起 `test_client.py` / `test_utils.py` 这种四条轨都可能撞的名字。

## 卡住了怎么办（§7）

1. 需要的接口在契约里找不到 → **停，报告**。不要自己发明。
2. 需要改的文件不在白名单里 → **停，报告**。
3. 需要新的第三方依赖（指 `pyproject.toml`，不含镜像内的 pip 包）→ **停，报告**。
4. 验收命令跑不起来（如 Docker 抽风）→ 先自己排查环境，排查不动 → 报告。
5. 验收过不了但代码自认为对 → **以验收为准**；确信验收写错了 → 停，报告。

## 完成后

```bash
git add -A && git commit -m "T3: Docker 沙箱 + Tool Gateway"
```

不要 push，不要合并到 main —— 并轨归 TΩ。

然后输出回执，最后一行严格按这个格式：

```
RECEIPT T3 status=<done|blocked> commit=<短hash> checks=<通过数>/7 files=<改动文件数>
```

（checks 的分母 7 = A2 / A3 / A4 / A5 / C1 / B4 / B5。）

回执正文包含：
- 上面每条命令的实际输出（关键几行，不是「通过了」）
- C1 的实际条数，与基线 335 对比
- `docker build` 的最后几行 + 镜像大小
- 三条镜像内验证（import 四个包 / `fc-list :lang=zh` / 无网络必须失败）的实际输出
- B4 里 PNG 魔数那 8 字节的实际值、`reap_idle` 前后 `docker ps -a` 的对比
- B5 四个错误码各自的用例与实际 `ToolResult`
- `git show --stat HEAD` 的文件树
- 如果 blocked：卡在哪、试过什么、需要什么决定
