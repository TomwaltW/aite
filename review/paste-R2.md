# 任务 R2 — Go Docker 沙箱 + ingress 客户端 + aite-edge 守护进程

## 背景：这轨是从哪来的

Aite 决定整体用 **Rust + Go** 重写（权威文档 `docs/dev-spec-2026-09-11-rustgo.md`，先通读 §1–§3、§5–§7）。
Python 版 P0 已经跑通（T0–T24），**它是规格与参考，只读**。R0 已合入 main（`c9d96d2`）：proto 契约、Rust 契约 crate、
Go 模块骨架、守卫、check.sh、CI、spec、移植清单。

**你这轨是 edge 进程里「对外连接」的另一半**：把 `aite/sandbox/**`（Docker 沙箱，约 540 行）搬成 `edge/internal/sandbox/**`，
实现 R0 定好的 `server.SandboxPort`；把 `edge/internal/ingress`（edge → core 的 IngressService 客户端，R0 只放了最小版本）补齐退避与计数；
把 `edge/cmd/aite-edge/main.go`（R0 骨架）做成真正能起飞、能优雅退出、能报状态的守护进程。

移植清单：`review/inventory-gateway-evals.md` §3（DockerSandbox 全部参数与语义）、§11 第 2–9 条；进程收尾语义见 `review/inventory-core.md` §7（app.py 的 `_shutdown`）。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-r2
分支     : task-r2
基线     : c9d96d2   ← main 的 HEAD（完整 sha c9d96d29d6d7ad835deef9fc80ec365c786fe876）
工具链   : go 1.27.1、cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、Docker 29.6.1（daemon 在跑）
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r2
git log --oneline -1                                   # 期望 c9d96d2 ...
git status --short                                     # 期望空
scripts/check.sh --quick                               # 期望最后一行 "全部通过"，退出码 0（首次 cargo 全量编译 2–5 分钟）
core/target/debug/aite contracts lock --check          # 期望 OK 36 files
cd edge && go test ./... -count=1                      # 期望 aiteerr / config / server 三包 ok
docker version --format '{{.Server.Version}}'          # 期望 29.6.1（daemon 可达）
docker image inspect aite-sandbox:p0 --format '{{.Id}}' # 有就打 id；没有就先 docker build -t aite-sandbox:p0 docker/sandbox
```

`scripts/check.sh --quick` 关键行期望：`contracts passed=25 failed=0`、`cargo passed=35 failed=0`。

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明 hook 没挂上，停下报告。

**关于 Docker 测试的串扰**（先看这条能省十分钟）：`docker ps -a --filter label=aite.task` 是**全机器**命名空间；
别的轨（RΩ 之前只有你）或旧 Python 树的测试同时跑会互相看见。你的 `reap_idle` 测试用自己独有的 task_id 前缀，
断言时按 task_id 过滤，别断言"全局为空"，除了 B4 那一条（跑它之前先 `docker ps -a --filter label=aite.task -q` 确认没别人的）。

## 可写路径（白名单，之外一律只读）

```
edge/internal/sandbox/**       ← docker.go / workdir.go / errors 映射 / *_test.go
edge/internal/ingress/**       ← client.go 补齐 + 测试
edge/cmd/aite-edge/**          ← main.go 守护进程 + 测试
docker/sandbox/**              ← 镜像内容不变；只有真需要时才动（动了要在回执写明）
```

**邻居（只读，接口在这里）**：
- `edge/internal/server/ports.go`：`SandboxPort`、`StatusSource` 接口；`server.New(...)`、`server.ListenUnix`、`server.ContractVersion`。
- `edge/internal/aiteerr/errors.go`：`SandboxError{Kind, Msg}`（Unavailable / NotFound / InvalidPath / Timeout / Internal）与 gRPC 映射；**沙箱方法只返回它或 nil**。文件不存在（`GetFile`）目前没有单独 Kind：用 `SandboxNotFound` 且 `Msg` 以 `file_not_found:` 开头 —— core 侧 `aite-proto::status` 就是按这个前缀区分 `FileNotFound` 的（见 `core/crates/proto/src/status.rs`）。
- `edge/internal/feishu/platform.go`（**R1 的**，并行期间是占位）：main.go 调 `feishu.New(cfg.Feishu, feishu.Options{...}, sink)`、`Start(ctx)`、`Connected()`、`ReconnectCount()`，签名冻结；你按这个接线，R1 落地后不用改。
- `edge/internal/config/config.go`：`config.Sandbox`（image/cpu/mem_mb/idle_sec/exec_timeout_sec）、`config.Edge`（两个 socket、deadline、max_message_mb）。
- `edge/internal/pin/pin.go`：已预钉 `github.com/docker/docker v28.5.2`（`client`、`api/types/container`；同模块其他子包如 `api/types/filters`、`api/types/image`、`pkg/stdcopy` 直接 import）。要模块外新依赖 → 停下报告。
- `proto/aite/v1/{sandbox,edge}.proto`：`ExecResult` 的 `truncated` / `files_out`、错误约定在文件头注释。

## 要做什么

### ① `sandbox/workdir.go`（对应 `aite/sandbox/workdir.go`）
`NormalizeWorkPath(path, workdir)` / `RequireFilePath`：必须绝对路径、纯字符串归一（`.` 跳过、`..` 弹栈、越过根报错）、必须在 `/work` 下、文件路径不能恰好是 `/work`、不解析符号链接；错误是 `SandboxError{Kind: InvalidPath}`。`ExecTimeoutExitCode = 124`。

### ② `sandbox/docker.go`（对应 `aite/sandbox/docker_sandbox.py`，清单 §3 逐条）
- `NewDocker(cfg)` 只装配；`Ping(ctx)` 探 daemon；client 懒建（`client.NewClientWithOpts(client.FromEnv, client.WithAPIVersionNegotiation())`）。
- `Acquire`：先 `ImageInspect`（缺 → `Unavailable`，消息含 `先跑 docker build -t {image} docker/sandbox`）；`ContainerCreate` 参数**逐条**：`Cmd ["sleep","infinity"]`、`Labels {"aite.task": task_id, "aite.managed": "p0"}`、`WorkingDir`、`NetworkMode none`、`Memory mem_mb*MiB`、`NanoCPUs cpu*1e9`、`PidsLimit 256`、`CapDrop ["ALL"]`、`SecurityOpt ["no-new-privileges:true"]`、`Env AITE_WORKDIR=`、无 tty；`ContainerStart`；记账 `last_active`；`_check_ready`（容器内跑 python 检查 `timeout` 在、workdir 可写，输出 `AITE_SANDBOX_OK`），**不过就 Release 再返回错误**。
- `Exec`：前快照（容器内 python `os.walk` 出 `{path: [size, mtime_ns]}` JSON）→ 代码写 `/tmp/aite_exec_<uuid>.py`（0600，`CopyToContainer` tar）→ `ContainerExecCreate/Attach`：`["timeout","-k","2",N,"python","-u",script]`，`WorkingDir=/work`，stdout/stderr 分流（`stdcopy.StdCopy`）→ 后快照 + `rm -f` + touch → 超时判定 `124 || (137 && elapsed >= timeout)` 改写 124 并 stderr 追加 `[aite] 执行超过 {N}s 上限，已在沙箱内终止` → stdout/stderr 各自截到 20000 字符（marker `"\n…[输出已截断]"`，按 rune 数）→ `files_out` = after 里 `before[path] != meta` 的（size/mtime_ns 二元组比较；排序）→ `duration_ms` 从入口算。
- `PutFile`：`RequireFilePath` → 容器内 `mkdir -p parent` → 内存 tar（name=basename、mode 0644、**uid/gid 1000**）→ `CopyToContainer`。`GetFile`：`CopyFromContainer` → 取第一个普通文件；不存在 → `SandboxError{NotFound, "file_not_found: <path>"}`。`ListFiles`：复用快照，排序返回，不 touch。
- `Touch(unknown)` 静默；`Release` 幂等（先从记账表删，再 `ContainerRemove{Force:true}`，NotFound 当成功）；`ReapIdle`：本进程 idle 超时的 + **孤儿**（`ContainerList{All:true, Filters: label=aite.task}`，跳过已记账，取 Created / StartedAt / FinishedAt 最大值判 idle，三个都解析不出也收，list 出错返回空不报错）→ 去重逐个 Release。时钟可注入。
- `CloseAll()`（非接口方法，main 收尾调）：释放全部记账容器。
- 日志用 slog，事件名沿用 Python（`sandbox.acquired` / `sandbox.released` / `sandbox.reaped` 这类若 Python 有就照抄，没有就按同风格起名并在回执列出）。

### ③ `sandbox` 测试（对应 `tests/sandbox/**` 23 条）
- `workdir_test.go`：5 条（参数化拆子测试）。
- `docker_test.go`：18 条，**打 build tag `//go:build docker`**，daemon 不可达或镜像不在时 `t.Skip` 并说明；覆盖：标签；network none；镜像不存在被拒；exec 返回；files_out 只报本次；用户代码报错不是沙箱失败；超 20000 截断；超时在容器内强制（`while True: pass` + timeout 2 → 124 且 duration ≈ 2s）；put/get 往返；put 拒 /work 外；get 缺失是 file_not_found；list_files；未知 id 被拒；release 幂等；touch 推后 reaper；reap 不动新鲜的；捡另一实例的孤儿；**B4**：matplotlib 画 `/work/out.png` → GetFile 前 8 字节 `89 50 4E 47 0D 0A 1A 0A` → `ReapIdle(1)`（注入时钟推进）后 `docker ps -a --filter label=aite.task=<你的 task_id>` 为空。

### ④ `ingress/client.go`（补齐）
- 懒连接 + 连不上时指数退避重连（1→2→…→30s 封顶，无限），日志 `ingress.reconnecting` / `ingress.reconnected`；
- `HandleEvent`：1s deadline（配置项）；`INVALID_ARGUMENT` → 计 `ingress.invalid`（**不重推**，返回 nil 让 SDK 确认，这是 core 说事件非法）；其余错误 → 计 `ingress.errors`，返回 error 让平台重推；
- `Connected()` 给 EdgeStatus；`Counters()` 快照。
- 测试：起一个假的 `IngressServiceServer`（gRPC，UDS）验 deadline、INVALID_ARGUMENT 不重推、INTERNAL 重推、断线重连。

### ⑤ `cmd/aite-edge/main.go`（做实）
- `statusSource.Status`：`contract_version = server.ContractVersion`、`platform_connected`、`reconnect_count`、`sandbox_ok = docker.Ping == nil`、`platform` 字段。
- `platform: fake`：不起飞书，只提供 gRPC 面（评测 / 集成测试用）。
- 信号：SIGINT/SIGTERM 第一次优雅（`GracefulStop` 上限 10s → `Stop`；再关长连接；`docker.CloseAll()`），第二次硬退 130（直写 stderr `aite-edge: 又收到 <sig>，硬退出。`）。
- 退出码：正常 0；起飞失败 1（stderr 一行人话：配置错 / socket 占用 / …）。
- 测试：`go test ./cmd/...` 里用 `exec.Command` 起进程（`platform: fake`、临时目录里的 socket），`GetStatus` 可达且 `contract_version == "p0.2"`，发 SIGTERM 退出码 0，socket 文件被清掉。

### ⑥ 镜像
`docker build -t aite-sandbox:p0 docker/sandbox` 成功；镜像内 `python -c "import pandas, matplotlib, openpyxl, docx"`、`fc-list :lang=zh` 非空、无网络（`urllib.request.urlopen('https://example.com', timeout=3)` 必须失败）—— 这三条在 docker_test.go 里各一条。

## 纪律

1. 契约与锁：`OK 36 files` 全程不变；`proto/**`、`core/crates/contracts/**` 一个字不动。
2. 白名单之外只读；要改 `server.SandboxPort` / `aiteerr` / 依赖表 → **停下报告**。
3. Python 行为原样搬，想改的写回执（例如 `files_out` 比 `[size, mtime_ns]` 检测不到同长度改写这条）。
4. `go vet ./...`、`gofmt` 干净；Docker 测试全部打 tag，`go test ./...` 不带 tag 时不碰 daemon。
5. 每条结论挂实测。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-r2
cd edge && go test ./... -count=1                                            # 期望全部 ok（不带 tag）
cd edge && go test -tags docker ./internal/sandbox/... -count=1 -v 2>&1 | tail -30   # 期望 ok，B4 那条 PASS
cd edge && go vet ./... && go vet -tags docker ./... && test -z "$(gofmt -l .)" && echo lint-ok
cd .. && scripts/check.sh --quick                                            # 期望 全部通过
core/target/debug/aite contracts lock --check                                # 期望 OK 36 files
docker ps -a --filter label=aite.task -q | wc -l                             # 跑完测试后期望 0
```

## 回执格式

```
## R2 回执

基线 c9d96d2 → 提交 <短 sha>

### 移植对照表
| Python | Go | 说明 |
（tests/sandbox 23 条 → Go N 条；ingress / main 新增测试 M 条）

### Docker Go client 的取舍
<create/exec/copy 三处 API 与 Python SDK 的对应；超时判定；孤儿判定的时间戳来源>

### 与 Python 行为的差异（逐条；没有就写"没有"）

### aite-edge 起飞实录
$ ./aite-edge --config config/aite.example.yaml   （platform: fake，临时 socket）
<前几行日志 + GetStatus 输出 + SIGTERM 后退出码>

### 改了什么
- <文件> <一句话>

### 实测输出（粘实际的）
$ cd edge && go test -tags docker ./internal/sandbox/... -count=1 -v | tail -30
<粘>
$ scripts/check.sh --quick
<最后 3 行>
$ docker ps -a --filter label=aite.task -q | wc -l
<>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-r2` 分支上，回执贴出来。
