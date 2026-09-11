# 任务 RΩ — 组装起飞 + 起飞前自检 + compose + 删掉 Python 树 + 全量验收

## 背景：这轨是从哪来的

Aite 用 **Rust + Go** 重写（权威文档 `docs/dev-spec-2026-09-11-rustgo.md`，先通读 §1–§3、§5–§7）。
R0 落骨架，R1–R7 七轨并行把各面做完，**已经全部合入 main**，并经过一轮逐轨代码审核（七个独立 agent
对照 Python 源码读代码）+ 一次总管修复。你是最后一轨：**把它们接成一个能起飞的东西**，然后删掉 Python 树。

当下的状态一句话：**所有零件都有了，没有人把它们插在一起。** 具体地说 —— `aite run` 和
`aite preflight` 还是 `not implemented`；评测的 `PlaneFactory` 没人注入所以 `passed 0/10`；
`--model live` 和 `--sandbox docker` 两档的工厂都空着。

必读（按顺序）：

1. `docs/dev-spec-2026-09-11-rustgo.md` §2（进程模型与错误约定）、§3.2（构造入口表）、§4（验收）
2. `review/review-findings-2026-09-11.md` —— **这一份对你最要紧**：七轨审核的全部结论，
   第三节「记账给 RΩ」列的就是留给你的坑，下面 §要做什么 里点名的几条都出自那里
3. `review/inventory-core.md` §7（Python `app.py` 的组装顺序、收尾序列、孤儿收场）
4. `review/inventory-gateway-evals.md` §8（`preflight.py` 的七组检查）、§9（compose）

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-romega
分支     : task-romega
基线     : c300e37   ← main 的 HEAD（完整 sha c300e37fcbee4b8d340616074182eeb3c2e2af93）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、go 1.27.1、protoc 36.1、Docker 29.6.1
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-romega
git log --oneline -1                                   # 期望 c300e37 ...
git status --short                                     # 期望空
scripts/check.sh                                       # 期望最后一行 "全部通过"，退出码 0（首次全量编译 5–10 分钟）
core/target/debug/aite contracts lock --check          # 期望 OK 36 files
```

`scripts/check.sh` 的关键行期望（**这就是你的起跑线，任何一条变小都是回归**）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 36 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=633 failed=0` |
| B 全量 go test（-race） | 八个包全 `ok`（`-race` 是硬门禁，别去掉） |
| B8 评测 | `passed 0/10` —— **这正是你要变成 10/10 的那个数** |

另外单独跑一次，它不在 check.sh 里：

```bash
cd edge && go test -tags docker ./internal/sandbox/... -count=1   # 期望 ok（22 条，含 B4 真容器）
docker ps -a --filter label=aite.task -q | wc -l                  # 期望 0
```

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
`CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，停下报告。

## 可写路径

你是最后一轨，**全仓可写**，但下面这些仍然要走授权：

- `proto/**`、`core/crates/contracts/**`、`.contracts.lock` —— 契约冻结。**删 Python 树那一步会让锁变红**（`aite/contracts/**` 在锁定面里），那一次重锁是允许的，用 `AITE_RELOCK=1 core/target/debug/aite contracts lock --write`，并在回执里单独说明。
- `evals/p0/*.yaml` —— 十个场景是验收面，一个字不动。
- `docs/dev-spec-2026-09-11-rustgo.md` —— 冻结。

## 要做什么

### ① `aite run`：组装 + 起飞（主戏）

在 `core/crates/app/` 里实现，对应 Python 的 `aite/app.py`（`review/inventory-core.md` §7 有逐条拆解）。

**组装顺序**（照 Python 的 `build_app`，顺序有讲究）：

1. 建三个落盘目录（sqlite 父目录、`evidence_dir`、`artifacts_dir`）—— 唯一允许的副作用
2. 读 system prompt（读不到就 `StartupError`，消息要点名 `worker.system_prompt_path` 的当前值和「多半是没在仓库根起进程」）
3. `EdgeClient::connect(&cfg.edge, repo_root)` → `platform()` / `sandbox()`
4. **比对 `contract_version`**：`edge.status()?.contract_version != CONTRACT_VERSION` → 拒绝起飞，两边版本都印出来
5. `OpenAiCompatModel::from_config(&cfg.model, &env)`
6. `SqliteSessionStore::open(&cfg.storage.sqlite_path)`
7. `FileEvidenceWriter::new(&cfg.storage.evidence_dir)`
8. `P0ToolGateway::new(platform, sandbox, sandbox_spec)` + **`token_resolver` 接到 store 上**
9. `AgentWorker::new(WorkerDeps{..})`
10. `InProcessControlPlane::new(ControlDeps{.., worker, gateway, sandbox, model_name})`
11. `Ingress::new(plane)` → `handler()` 交给 `EdgePlatform::start`

**起飞与收尾**（`run_app`，冻结序列，照 `inventory-core.md` §7 抄）：

- `store.init()` 必须在 try 里 —— 它一成功就有连接挂着，此后任何异常都要走到 `store.close()`
- `_recover_orphans` 在 `platform.start()` **之前**（出站链不依赖长连接，而 start 是跑到 stop 才返回的）
- 信号：第一次优雅（`platform.stop()` → `plane.join()` 宽限 20s → 超时前**先抄** `worker.in_flight` → `runner.cancel()` → 给硬取消的任务走 `cancel_task(notify=false)` → `sandbox.close_all()` → `store.close()`），第二次硬退 130
- 退出码：正常 0 / 起飞失败 2 / 硬退 130

**`platform: fake` 时**必须由调用方注入替身，不注入就 `StartupError`（fake 不是「内建替身」，是「必须注入」的标记）。

### ② `aite preflight`：七组自检

对应 `scripts/preflight.py`（`inventory-gateway-evals.md` §8）。七组：配置可加载 / 环境变量齐（**只报在不在，不打取值**）/ 飞书凭证有效 / 机器人身份对上 / 模型端点通 / 沙箱可用（跑完必须收掉容器）/ 落盘目录可写。

`--offline` 只跑 1、2、7；`--json`；`--chat-id` 顺带实测群历史。状态 `ok/fail/warn/skip`，**任一 FAIL → 退出 1，一项失败不阻断后面的检查**。

**红线：任何输出不得出现密钥取值**，除了「不主动打」还要有 `Redactor` 兜底（短于 4 个字符的值不替换，否则正常输出会被打成马赛克）。

### ③ 把评测接起来 —— B8 `passed 10/10`

`aite evals` 现在走 `aite_evals::cli::run(args)`，你要改成 `run_with_factory(args, wiring)`，填三个工厂：

```rust
Wiring {
    plane:     Some(/* 真 InProcessControlPlane + AgentWorker */),
    model:     Some(/* --model live 的 OpenAiCompatModel */),
    sandbox:   Some(/* --sandbox docker 的 EdgeSandbox + P0ToolGateway */),
    preflight: Some(/* docker daemon + 镜像体检 */),
}
```

四条已经替你堵好的坑（都出自审核，别再踩）：

- **`SandboxFactory` 的第四个参数是现成的 `TokenResolver`**，直接塞进 `P0ToolGateway`。不接的后果是每个工具调用都 `denied`，而报出来的是「denied」不是「你没接 token_resolver」。
- **`preflight` 必须与 `sandbox` 成对注入**，只给工厂不给体检会被直接拒（退出码 2）。体检说不行时十个场景一个都不跑。
- **`ModelFactory` 要在起飞前就试造一次**（Python 那边 `_live_model_factory` 就是这么做的）：否则配置缺一样会变成每个场景跑到第一次 chat 才抛，被 worker 当成模型 5xx 白重试 2 次（2s + 5s），十个场景就是十次 7 秒空等，而真正的原因一个字看不到。
- **plane 工厂里别 panic**：runner 现在有 `catch_unwind` 兜底会收成 `phase="error"`，但那是安全网不是许可证。

`PlaneFactory` 的签名是 `Arc<dyn Fn(&Deps) -> Result<Arc<dyn ControlPlane>, String>>`，`Deps` 七个字段够你造全套。

### ④ compose 与 CI

- `docker-compose.yml` 改成 **core + edge 两个 service**，共享 `data/run` 卷（两个 unix socket 在那里）与 `/var/run/docker.sock`（沙箱是兄弟容器）。单副本（同一飞书应用多副本长连接只有一个收得到事件）。
- `.github/workflows/ci.yml`：把 B8 那步的 `continue-on-error: true` **删掉**，它从此是硬门禁。

### ⑤ 删掉 Python 树

`aite/**`、`tests/**`、`pyproject.toml`、`scripts/*.py`、`.venv/`（不入库但要清）。删之前：

- 确认 `review/inventory-*.md` 三份清单还在（它们是删掉之后唯一的规格记录）
- 删完 `AITE_RELOCK=1 core/target/debug/aite contracts lock --write` 重锁，回执里贴新的 `OK N files`
- `scripts/check.sh` 全绿

### ⑥ 把审核记账清掉（`review/review-findings-2026-09-11.md` 第三节）

七轨审核留给你的账，**按性价比排序**，做不完的在回执里逐条说明为什么：

| 优先 | 项 |
|---|---|
| 高 | R2 `ReapIdle` 单个 Release 失败会把**已经真删掉**的 id 随 error 丢成 nil → core 手上的 `task → sandbox_id` 不会被清，之后拿死 id 去 exec |
| 高 | R4 `run_one` 的 `queue.task_done()` 不是 drop-safe（Python 是 `finally`）；`FinishGuard::drop` 两次 remove 不在一个临界区 |
| 高 | R5 `final.artifacts` 是「假值但非 null」（`""` / `{}` / `0`）时 Python 放行、Rust 判 invalid_args 退回重来 —— 交付路径上的行为翻转，弱模型给 `artifacts: ""` 不罕见 |
| 中 | R2 `ingress` 的 `Connected()` 落后于 RPC 成功；首次连上被计成一次 reconnect（`!status` 会说「重连 1 次」但从没连上过）；`Close()` 后再调会把连接和 goroutine 复活 |
| 中 | R6 `safe_name` 与 Python 在 `file_key` 以 `/.` 结尾时分歧；那四步（折 `_`、`strip("._")`、空→attachment、`[:120]`）一条用例都没有 |
| 中 | R6 `DEFAULT_RUN_PYTHON_GRACE_SEC = 5.0` 没有测试钉住，改成 0 一条都不会红 |
| 中 | R2 `clip`/`diffFiles`/`parseDockerTime`/`orphans` 这些纯函数只在 `docker` tag 下被跑到，没 daemon 的 CI 上等于裸奔 —— 拆一个不带 tag 的 `docker_pure_test.go` |
| 低 | R3 `find_session_by_thread` 排除 archived / 多条取最新两条语义零断言；`busy_timeout` 没测试钉 |
| 低 | R6 未使用依赖（`gateway` 的 sha2/hex/tempfile、`edge-client` 的 tonic-prost/prost/thiserror）；`constant_time_eq` 缺 `black_box` 屏障（**不要**为此加 `subtle` 依赖） |
| 低 | R7 `serde_json` 没开 `preserve_order`（JSON 输出按字母序而非插入序）；`--help` 走 stderr + exit 2（Python argparse 是 stdout + exit 0） |

### ⑦ 一条要你拍板的能力缺口

**卡片上「停止」「证据」两个按钮在 Go 版点了没反应。** lark-oapi-go v3.12.0 在长连接上把非 event 帧
整条丢弃（`ws/client_message.go:79`），唯一的 `WithCardHandler` 钩子是注释掉的，
Python 靠 monkeypatch 私有方法绕过的那条路 Go 没有。

影响边界已核：`!stop` / `!status` 走普通消息事件不受影响，评测 `07_commands` 用的正是文本命令，
M1–M6 没有一条依赖按钮。**但 `cards.go` 照常渲染这两个按钮，用户会看到点不动的按钮，比不显示更糟。**

四个选项：①自定义 WebSocket dialer 在字节流上改帧头 ②fork/vendor 一份 ws 包打补丁
③卡片回传改走 HTTP webhook ④先不渲染按钮。**总管的倾向是短期取 ④**（一行改动，不骗用户），
但这条归你实施前再确认一次。

## 纪律

1. 契约与锁：除了「删 Python 树之后重锁」那一次，全程 `OK 36 files`。
2. `evals/p0/*.yaml` 一个字不动。
3. 不 panic；不在 async 里阻塞；测试不靠真实 sleep。
4. `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check`、`go vet`、`gofmt`、
   `go test -race` 全干净。**`-race` 是硬门禁**（合流时它抓出过一处测试辅助的竞态）。
5. 密钥只从配置点名的环境变量读，任何日志 / 错误 / Debug 输出不得出现取值。
6. 每条结论挂实测。「应该会」「大概」一句不要。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-romega
scripts/check.sh                                                  # 期望 全部通过，退出码 0
core/target/debug/aite evals run evals/p0 --platform fake --model scripted
                                                                  # 期望 passed 10/10，退出码 0  ← B8
core/target/debug/aite contracts lock --check                     # 期望 OK <重锁后的数> files
cd edge && go test -tags docker ./internal/sandbox/... -count=1    # 期望 ok
docker ps -a --filter label=aite.task -q | wc -l                   # 期望 0
docker compose config                                              # 期望退出码 0，两个 service
```

真机那一档（要凭证，在你手上的话就跑）：

```bash
core/target/debug/aite preflight --offline                        # 期望七行里 1/2/7 有结论，其余 skip
core/target/debug/aite preflight                                  # 有凭证时期望全 ok
core/target/debug/aite evals run evals/p0 --only 04_csv_to_chart --sandbox docker
core/target/debug/aite evals run evals/p0 --only 04_csv_to_chart --model live
```

`--model live` 跑通之后补一份 `evals/live-report-2026-09-XX-romega.md`（照 Python 版那三份的样子）。

## 回执格式

```
## RΩ 回执

基线 c300e37 → 提交 <短 sha>

### B8
$ core/target/debug/aite evals run evals/p0 --platform fake --model scripted
<最后一行；期望 passed 10/10>
接线用的四个工厂分别是怎么造的：<各一句>

### 组装与收尾
contract_version 比对：<不匹配时的实际输出>
收尾序列实测：<SIGTERM 后的日志 + 退出码 + 残留容器数>
孤儿收场实测：<造一个残局再起飞的输出>

### 删 Python 树
删了什么：<>
重锁：$ AITE_RELOCK=1 ... lock --write → <OK N files>

### 审核记账清掉了哪些（⑥ 那张表逐条）
| 项 | 做了没 | 没做的理由 |

### 卡片按钮那条（⑦）
你的处置：<>

### 实测输出（粘实际的）
$ scripts/check.sh
<最后 3 行>
$ cd edge && go test -tags docker ./internal/sandbox/... -count=1
<>
$ docker compose config | head -20
<>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-romega` 分支上，回执贴出来。
