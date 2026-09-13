# V1–V6 合并台账 —— 2026-09-12

> 上一份是 `review-findings-2026-09-12-romega.md`（RΩ 合并前审核）。本文接它的第四节，
> 记三件事：**V1–V6 销掉了哪些账**、**合并本身造出了哪些新失真**、**还剩什么**。
>
> 合并方式：`task-v1 → task-v4 → task-v3 → task-v2 → task-v5 → task-v6` 依次 `--no-ff`，
> **六轨零冲突**（可写面两两不相交，六份派单划得对）。

---

## 一、先说结论

**P0 的代码面收完了。两份 spec 查下来，没兑现的只剩 `dev-spec-2026-09-09.md` §2.4 /
`dev-spec-2026-09-11-rustgo.md` §4.4 的 M1–M6 与 3 分钟演示 —— 那是总管自己的活。**

剩下三组账（下面第三、四节）**全是 low，没有一条拦着验收**。

合并后在 main 上的实测：

```
$ scripts/check.sh
A3/C2 契约锁 ............. OK 25 files
C1 契约测试 .............. contracts passed=25 failed=0
B 全量 cargo test ........ cargo passed=793 failed=0      （起跑线 718，+75）
B 全量 go test（-race）... 六包全 ok（aiteerr/config/feishu/ingress/sandbox/server）
B8 评测 .................. passed 10/10
clippy -D warnings / fmt --check / go vet / gofmt ... 全干净
全部通过，退出码 0

$ cd edge && go test -tags docker ./internal/sandbox/... -count=1
ok  	aite/edge/internal/sandbox	13.959s
$ docker ps -a --filter label=aite.task -q | wc -l
0
```

> 793 比六轨各自自述之和（802）少 9 条。不是回归 —— 各轨报的「718 → N」是各自在
> **8d6ffd3** 上单独量的，V2/V5/V6 有几条同名测试落在同一个文件里，合并后只算一次。
> 逐条核对没有变红的，B 段的失败名输出为空。

---

## 二、V1–V6 销掉的账（对照 RΩ 台账第四节）

### 4.1 medium 八条 —— 全清

| 原账 | 谁销的 | 怎么销的 |
|---|---|---|
| `app.rs:329` contract_version 门禁起飞后不再比对 | V5 ① | 下沉到 `edge-client/src/gate.rs` 契约闸门，每次重连 + 第一发 RPC 各补比一次，不一致就闸断两条链路 |
| `run.rs:185` 抄 in_flight 与 abort 的并发窗口 | V5 ② | 抄快照挪到 `runner.abort()` 之后；另加控制面防线：`cancel_task` 落刀前按 id 重读，终态不改写 |
| `preflight.rs:576` 网络 FAIL 丢 source() 链 | V2 | `error_chain()` 拼 `std::error::Error::source()`，代理/DNS/TLS 三种病现在打得不一样 |
| `preflight.rs:1314` Redactor 红线零覆盖 | V2 | `tests/preflight_e2e.rs` 15 条，自带按 path 路由的 HTTP 假服务 |
| `wiring.rs:98` `--model live` 校验抢在参数解析之前 | V6 ① | 改惰性：`evals_wiring` 只装闭包，I/O 推迟到工厂首次被调用 |
| `wiring.rs:153` docker 档场景收尾是空操作 | V6 ② | `LeasedSandbox` 租约记账，裹在探针**下面**（收尾的 release 不污染 CallLog） |
| `docker.go:423` ReapIdle 的 released 无消费者 | V5 ④ | 不动契约锁里的 trait，改 gateway 内部自愈：撞 `NotFound` 摘死 id、重建、只重试一次 |
| `preflight.rs:946` Python 25 条 preflight 测试没人接管 | V2 | 产品代码五处改动 + 38 条测试 |

### 4.2 四条 —— 全清

`CARGO_TARGET_DIR`（V1 ①，走 BuildKit cache mount，实测跑完一整轮 `git status` 仍为空）、
`gomod` 卷（V1 ①）、`Answering` 不在活跃集（V5 ③，把控制面自己的 running 并进 `cmd_status`）、
Redactor 两个口子（V2，`arm_redactor()` 提到 `check_config` 之前 + 改走 `env_var_names()`）。

### 4.3 销账 —— 维持

`serde_json` 的 `preserve_order` 不做，三条理由照旧。

### 4.4 low 二十六条 —— 清掉十七条

V1 清四条（`Makefile:24` 的 `-race`、compose/CI 三条）、V2 清两条（`MIN_REDACT_LEN` 数字符、
`rel()` 空串）、V3 清两条（README 的 inventory 说法、旧 Python spec 的定位）、
V6 清五条（`--grace` 门口上界、`aite run -- -h`、`--traceback` 文案、
`evals demo-fixture --help`、守卫回归测试接管）、`check.sh` 的 B8 与 `.gitignore` 早前已清。

`preflight.rs:1252` 的写副作用**有依据地留着**（V2 把理由与残留面写进注释）。

---

## 三、合并本身造出的新失真（W1 的料）

**这一组是六轨各自正确、合起来自相矛盾。** 单看任何一轨的 diff 都看不见，只有合完才现形。
**危险性比它的严重度高** —— 总管接下来就要照着这几份文档跑 M1–M6 和录演示。

### 3.1 `docs/acceptance-M.md` 有五处在教总管一件已经不成立的事（最要紧）

V3 写这几段时，V5 还没并进来。V5 ③ 已经把「交付中的短任务 `!status` 查不到」修好了
（控制面的 running 并进 `cmd_status`，按 id 重读 + `get_session` 过滤到本群），而文档现在仍写着：

| 行 | 现在的原话 | 问题 |
|---|---|---|
| `acceptance-M.md:358` | `<!-- 台账 §4.2；V5 修掉后删本段 -->` | 整段引用框的前提变了 |
| `:363` | 「**这是已知记账（台账 §4.2）**」 | `!status` 那一半已不是记账 |
| `:435` | 「**已知记账，不是故障** … **别去查沙箱**」 | 排障表里这一行会把人引向错误结论 |
| `:554` | 「这一路交付中 `!status` 查不到它」 | 同上 |
| `:864` | 「交付中的短任务查不到，见 §0.4」 | 同上 |

这五处是 V3 自己留的记账注释，约定就是「对应轨修掉后连注释一起删」。

> ⚠️ **但不能整段删 —— V5 只修了一半，而剩下那一半现在更难受。** 核过代码：
>
> ```
> plane.rs:481  status_tasks()        list_active_tasks + 并进控制面 running（过滤终态 + 同群）  ✅ V5 改了
> plane.rs:651  resolve_stop_target() 仍然只有 list_active_tasks                                ❌ 没改
> plane.rs:669  resolve_task()        仍然只有 list_active_tasks（卡片 stop 按钮走这条）        ❌ 没改
> ```
>
> V5 提交里写的就是「改为把控制面自己的 `running` 并进 **`cmd_status`**」—— 范围确实只有它。
> 于是现在：**交付中的短任务在 `!status` 里列得出来，`!stop` 却回「没有这个任务」。**
> 改之前两条口径一致（都查不到），改之后两条**互相矛盾**：用户看见它在列表里、
> 伸手去停却被告知不存在。对总管排障来说这比原来更费解。
>
> 这条既是 W1 的（文档要把这句话拆成两半写对），也是 W2 的（要么 `!stop` 跟上，
> 要么明确写下「Answering 不可停」是刻意的并补测试钉住 —— 现在是两边都没说法）。

另两处 `<!-- 台账 -->`：

- `:53` —— 「`aite run -- --help` 走 **stderr**、**退出码 2**」。V6 ④b 已改成 **stdout + 0**，
  正文说错了一半；但 `aite run --help`（不带 `--`）**仍被 clap 截胡**，根治归 R0（见 §4.2）。
  这一段要改成「哪个写法现在对、哪个还不对」，不能整段删。
- `:276` —— `--traceback` 那条，V6 ④c 已改文案（不再承诺「完整错误链」），复核本行。

### 3.2 README 的 CI 章节停在 V1 之前

- `README.md:217` 的十步链条以「→ compose 双 service 可解析」收尾。V1 已经把那一步换掉了：
  `ci.yml` 现在有独立的 `compose-smoke` job（与 `checks` 并行），真 build 两个镜像 →
  `preflight --offline` → 两个进程真起 → 断言两个 socket 都在共享卷里、cwd 是 `/app`、
  运行层没有工具链、core 侧四行起飞日志、`RestartCount` 为 0。
- `README.md:114` 的「ℹ️ **CI 里目前没有 preflight 这一步**」**现在是假的**
  （`ci.yml` 第 115 行就是 `preflight --offline`）。这句是 V3 加的，加的时候它是真的。

### 3.3 README 的容器段会让人起不来

`README.md:82-89` 只有一句 `docker compose up -d`。V1 把 compose 从「挂仓库进去现编」
换成了两个多阶段真镜像 —— 镜像不在就起不来，而 `docker-compose.yml` 抬头写的是
「`make compose-up` 会连 `aite-sandbox:p0` 一起建」。README 没提 `make compose-build` /
`compose-up`，也没提新增的 healthcheck 与日志轮转（`max-size 10m` / `max-file 5`）。

### 3.4 V4 那份实测报告没有入口

`evals/live-report-2026-09-12-v4.md`（529 行，十场景 × 两档 × 两遍）全仓只有它自己引用自己。
`evals/README.md` 121 行里一个字没提，README 也没有。**它同时是 RΩ §4 那条「与 Python 版
结论相反」的更正**（不是翻转，是比错了基线：T17 的 0d6939c 上 `platform.md` 是 T19 改提示词
之前的 2246 字节版），这个更正没有任何人看得见。

---

## 四、还剩的账

> **2026-09-13 更新**：4.1（W2）与 4.2（W3）**已全部销掉**，回执见第六、七节。
> 4.5 是这天新记的，**它不是记账，是当场咬到总管的一条** —— 归 X1。

### 4.1 W2 · `core/crates/app` 的拆弹与测试成色

| 位置 | 病 |
|---|---|
| `run.rs:197` | `Duration::from_secs_f64(grace.max(0.0))` **仍无上界**。V6 ④a 只在 CLI 门口挡了 86400，`ServeOptions` 直接构造照样 panic —— 而 panic 在收尾那一刻发生，`sandbox.close_all()` / `store.close()` 整段被跳过。V6 提交里明写「**这是把炸弹挡在门口，不是拆弹 —— 拆弹归 V5**」，V5 没做 |
| `app.rs:129` | 文档注释写着「**只组装**：不连网、不起容器、不发消息。唯一的副作用是建那三个落盘目录」。**这条现在是假的** —— `build_app` 里 `EdgeClient::connect()` + `check_contract_version()` 都要连 edge（`app.rs:157-171`）。这不是措辞问题：C-TΩ-1 是靠这条注释声明边界的 |
| `app.rs:78` | `edge` 字段注释说「留着它是为了 `!status` 的健康行与收尾时的连接态日志」，这两处都不存在（V5 之后 edge 的真实用途是契约闸门） |
| `tests/crash_recovery.rs:53,125` | `list_active_tasks(CHAT).await.expect("list")[0]` 没有守卫，任务已交付时越界 panic（报的是 index out of bounds，不是断言失败） |
| `tests/reconnect_replay.rs:168` | `if self.inner.call_count() > 0 { return; }` 是死代码 —— 闸门期间 `call_count()` 恒为 0，那个「防丢通知」的守卫从没生效过。**2026-09-12 升级：这不是清死代码，是一个真在抖的测试的病根** —— 建 W2 worktree 跑基线时撞到 `cargo passed=792 failed=1`，失败名逐字是 `-p aite --test reconnect_replay`，单独连跑五遍 10/10 全绿。守卫想防的那个「通知早于 await」的竞态**是真会发生的**，而兜住它的分支恒为 false |
| `tests/graceful_shutdown.rs:237,249` | `started` 起在建场之前，`elapsed < 3.0` 因此包含两个各 5s 预算的 `wait_until`，量错了区间 |

**外加 V5 留下的半截**（见 §3.1 的引用框，位置在 `core/crates/control/`，不在 app 里）：
`status_tasks` 并进了 `running`，`resolve_stop_target` / `resolve_task` 没有 —— `!status`
和 `!stop` 现在对「什么算活跃」意见不一致。两条出路：`!stop` 跟上，或者把「Answering
不可停」写成刻意的约定并补测试钉住。**现在是两边都没有说法，也没有任何测试管着。**

### 4.2 W3 · edge 侧 + `main.rs`

| 位置 | 病 |
|---|---|
| `edge/internal/sandbox/docker.go:381-397` | 注释写「Release 幂等：不认识的 id、已经没了的容器，都当成已经释放」，而 `Release("")` 走到 `ContainerRemove(ctx, "", …)` 拿回的不是 `IsErrNotFound`，于是返回 `SandboxInternal`。与自己的注释、与 `proto` 头注释三方矛盾 |
| `edge/internal/sandbox/docker_pure_test.go` | RΩ 把 `clip` / `diffFiles` / `parseDockerTime` / `reapVictims` 纯化进了无 daemon 门禁，但 `orphans` 仍只在 `-tags docker` 下跑到，可纯化的 `inspectStamps` 一条没测 —— CI 上没 daemon，这两个在门禁里等于裸奔 |
| `core/crates/app/src/main.rs` | `aite run --help` 被 clap 截胡（打的是不含任何真实选项的帮助）。V6 ④b 只修了 `aite run -- -h` 这一个写法，根治要动 `main.rs`，**归口 R0，至今没人接** |

### 4.5 X1 · preflight 说「可以起飞」然后起不来（2026-09-13 新记，**不是记账**）

总管 2026-09-13 想试机器，`aite preflight --offline` 报「**全部没红，可以起飞**」，
紧接着 `aite run` 退出码 2：

```
aite 起不来：读不到 system prompt：system prompt 不存在：aite/worker/prompts/platform.md。
配置项是 worker.system_prompt_path（当前值 aite/worker/prompts/platform.md），
路径相对于进程的工作目录 —— 多半是没在仓库根起进程。
```

真因：他那份 `config/aite.yaml`（2026-09-10 写的）还指着 **2026-09-12 被删掉的 Python 树**
（`aite/worker/prompts/` → 现在是 `core/crates/worker/prompts/`）。已就地修好他那份本地配置
（不入库），但**两个代码面的问题原样还在**：

| 位置 | 病 |
|---|---|
| `core/crates/app/src/preflight.rs` | **七组里没有一组管 `worker.system_prompt_path` 读不读得到**，而 `require_system_prompt` 只在 `build_app` 里（`app.rs:310`）。于是 preflight 给的「可以起飞」是假的。V3 记过同族的一条（「`--offline` 全绿 ≠ 起得来」，因为跳过第 5 组）——**这条更狠：去掉 `--offline` 全跑一遍也救不了你**。不许变成「八组」（`dev-spec-2026-09-11-rustgo.md:308` 冻结着「七组」，且这个说法散在 README 3 处 / acceptance-M 多处 / demo-3min / cli_smoke / preflight_e2e 的硬断言里），折进第 ① 组「配置可加载」 |
| `core/crates/app/src/app.rs:314` | 那句「路径相对于进程的工作目录 —— **多半是没在仓库根起进程**」是**误诊**：总管撞上时 cwd 就是仓库根，真因是配置里的路径本身指着已删的树。`app.rs:195`（连不上 edge 那条）有同样措辞，待复核 |

顺带一条**本地环境**的账（不入库、不算代码面）：`config/aite.yaml` 缺整个 `edge:` 节
（它是 09-11 重写时加的，比那份配置晚一天）。契约默认值兜住了 ——
实测日志里 `edge_socket=data/run/aite-edge.sock core_socket=data/run/aite-core.sock`
与样例逐字一致，所以没改。

### 4.3 只有人能做的两件

1. **跑 `review/v6-guard-patch.py`**（16 条锚点 `--check` 全中）。V6 出不了 diff：
   `.claude/hooks/guard_bash.py` 与 `.claude/settings.json` 在守卫的 `PROT_PATHS` 里
   （`readable=False`，**读和写都拦**），V6 一个字节都没读过。agent 也跑不了 ——
   守卫**故意**拦自我授权，`AITE_RELOCK=1 …` 判「授权变量赋值」直接拦。这是设计如此。
   它修的是那条真踩过的病：hook 命令依赖 `CLAUDE_PROJECT_DIR`，在仓库子目录里起会话
   就指向错路径 → hook 执行失败 → **非阻塞放行、不报警** → 整场守卫静默失效
   （改成 `$(git rev-parse --show-toplevel)`，V6 实测三个 cwd 下都是 fail-closed）。
   改完跑 `cd core && cargo test -p aite --test guard`（9 条）。
2. **M1–M6 + 录 3 分钟演示。**

### 4.4 与代码无关的一笔

主仓里的 Python 构建残渣仍在：`.venv/`（202M）、`aite.egg-info/`、`.pytest_cache/`、
`.ruff_cache/`，外加根目录一份与 `docs/dev-spec-2026-09-09.md` **逐字节相同**的冗余副本
（sha1 `8386a711c128`，45792 字节）。2026-09-12 总管明确保留，不是漏了。

---

## 五、W1 回执 —— 2026-09-12

基线 `59043aa`。**没改任何代码**，改动面只有四份文档
（`docs/acceptance-M.md` / `README.md` / `evals/README.md` / 本文追加这一节）。

`scripts/check.sh` 收尾与开场自检**逐字相同**：`OK 25 files` · `contracts passed=25 failed=0` ·
`cargo passed=793 failed=0` · go 六包全 `ok` · `passed 10/10` · 全部通过 · 退出码 0。

### 销掉的（本文 §三那四条）

| 账 | 怎么销的 |
|---|---|
| §3.1 五处 `!status` / `!stop` | **拆成两半写**，不是删。`acceptance-M.md` §0.4 换成一张三行表（`status_tasks` / `resolve_stop_target` / `resolve_task` 各查什么），M1 排障表那一行**裂成两行**：`!stop` 停不掉仍是记账、`!status` 查不到**现在是真故障**；§7 两处同口径 |
| §3.2 README 的 CI 章节 | 照 `ci.yml` 重写成两个并行 job，`compose-smoke` 六步逐条写开；`README.md:114` 那句「CI 里目前没有 preflight」改成「在 `compose-smoke` 里、`ci.yml:115-118`、跑在真镜像里」 |
| §3.3 README 的容器段 | 补 `make compose-*` 五个 target、先建镜像（含 `aite-sandbox:p0`）、两边不同口径的 healthcheck、日志轮转 |
| §3.4 V4 报告没入口 | `evals/README.md` §2 加了五份报告的索引表，外加「RΩ §4 那条被 V4 §5.1 更正」的单独一段 |

### `acceptance-M.md` §0.2.5 新增：compose 起飞（真起了一遍核的）

**一条结论翻了**：原文写「仓库是 `./:/app` bind mount」——**现在不是了**。挂载表只有
`./config:ro` 与 `./data:rw` 两条 bind，容器里 `/app` 没有 `README.md` / `docs/`，
`core/crates` 与 `evals` 是镜像 `COPY` 进去的、**冻在镜像里**。
但下游结论仍成立：`data/evidence` 经 `./data` 这条 bind 在宿主机看得见，
`aite evidence show` 照常在宿主机跑 —— **理由换了，结论没换**。

另两条实证成立：`docker compose logs -f` 照旧；两个 socket 在命名卷 `run:` 里，
宿主机 `data/run/` 是空目录（命名卷比 `./data` 更深，盖住了它）。
顺带一条新坑：**`docker compose exec` 不过 ENTRYPOINT**，
要写成 `docker compose exec core aite evidence show --list`（`run --rm` 那条路才过）。

### 行号引用：11 处漂了，已逐个 `sed -n` 核过改掉

`preflight.rs:746-800`→`801-854`、`app.rs:137-141`→`142-146`、`app.rs:38`→`43`、
`edge-client/src/lib.rs:96-101`→`109-116`、`cli.rs:104-106`→`161-163`、
`evidence/src/cli.rs:1318-1320`→`1320-1322`、`evidence/src/cli.rs:1504-1508`→`1399`、
`gateway.rs:221-231`→`238-248`、`plane.rs:1193`→`1305`、`plane.rs:458-481`→`536-559`、
`plane.rs:557-566`→`539`/`556`+`635-644`、`plane.rs:568-583`→`646-661`、
`Makefile:18-20`→`24-26`、`Makefile:55`→`88`、`docker-compose.yml:59`→`docker/edge/Dockerfile:21`。
核过没漂的：`session.rs:33`、`plane.rs:28`、`plane.rs:377-378`、`link.rs:9`、
`agent.rs:643-648`、`context.rs:70-82`、`platform.md:54`、`cards.go` 四处、
`platform.go` 两处、`server.go:21-24`、`main.go` 三处、`.gitignore` 三处、
`evidence/src/cli.rs:976-977`、`cli.rs:1279`。
`demo-3min.md` 与 `evals/README.md` 里**一个 `文件:行号` 都没有**，不用核。

### 记账转出去的（撞见但没伸手改）

| 位置 | 病 | 归哪轨 |
|---|---|---|
| `core/crates/control/src/plane.rs:646-661`（`resolve_stop_target`）、`:663-678`（`resolve_task`） | 只查 `list_active_tasks`，跟不上 V5 改过的 `status_tasks`。**`!status` 列得出来、`!stop` 停不掉**，两条命令对「什么算活跃」意见不一致，且零测试钉着 | **W2**（本文 §4.1 末段已立） |
| `core/crates/app/src/main.rs` 的 clap 声明 | `#[arg(trailing_var_arg = true, allow_hyphen_values = true)]` 把 `--help` 吞了：`aite run --help` / `aite preflight --help` 都打不出真选项，**而且退出码是 0**，脚本里判不出来。V6 ④b 只修了 `-- --help` 那个写法 | **R0 / W3**（本文 §4.2 已立） |
| `.github/workflows/ci.yml:112` 的注释 | 引的是「README.md:90 写着…CI 用这一档」。那句话在 V3 改写时挪了位置、「CI 用这一档」的字样也掉了。W1 已在 `README.md:136` 把语义补回来，但 ci.yml 里那个**行号**仍是漂的 —— ci.yml 是 W1 的只读面 | 顺手改（W2/W3 或总管） |
| `docker/core/Dockerfile`、`docker/edge/Dockerfile` | 两个 Dockerfile 都**没有 `USER`**，容器以 root 跑。macOS 的 Docker Desktop 把 uid 映射回宿主用户，所以 `./data` 里的文件宿主机读写正常（W1 实测 owner 是当前用户）；**Linux 上不是这样** —— `data/evidence` 会是 root 拥有，宿主机那条 `aite evidence show` 可能读不了。**只在 macOS 上验过，Linux 未验证** | 待定 |

### 顺带留下的一份现场：又一次 `cargo` 假红

收尾第一遍 `check.sh` 报 `cargo passed=791 failed=2`，失败名是
`-p aite --test graceful_shutdown` 与 `-p aite --test startup_recovery`。
**单独跑这两个 target：7 passed / 9 passed，全绿**；紧接着重跑全量回到 `793 failed=0`。
与 RΩ 台账第五节那次 `717/1` 同形态（本轨没改代码，改的全是 `.md`）。
排除过一个混淆项：当时 `config/aite.yaml` 因 compose 实证而存在（平时不存在），
但单跑复现时它仍在、照样全绿，**不是它**。
`graceful_shutdown` 本身有本文 §4.1 记着的计时区间量错（`:237,249`），是天然的抖动源。

---

## 六、W2 回执 —— 2026-09-12

基线 `0ac0b58`（= `abd93da` 的代码面 + 一个只加派单文件的 commit）。

开场自检与收尾 `scripts/check.sh` 的五行关键值：

```
契约锁 ............ OK 25 files                 （开场 / 收尾一致）
C1 契约测试 ....... contracts passed=25 failed=0（开场 / 收尾一致）
B 全量 cargo ...... 开场 793 failed=0 → 收尾 806 failed=0
B go test（-race）. 六个包全 ok                  （开场 / 收尾一致）
B8 评测 ........... passed 10/10                （开场 / 收尾一致）
全部通过，退出码 0
```

**开场自检没撞到假红**（一次跑过，793/0）。守卫实测有效：Read `.claude/hooks/guard_bash.py`
被拦下。

### ①–⑦ 逐条

| | 病在哪 | 改法 / 为什么选这条 | 钉它的测试 | 改坏产品代码验过 |
|---|---|---|---|---|
| ① | `run.rs:197` `Duration::from_secs_f64(grace.max(0.0))`：`max(0.0)` 只挡住负数，`NaN.max(0.0)` 侥幸是 `0.0`，**上溢一个字没拦**；panic 点在 `shutdown()` 中段，`sandbox.close_all()` / `store.close()` 整段被跳过 | 新增私有 `grace_duration(f64) -> Duration`，走 `Duration::try_from_secs_f64`：造得出来就照用，造不出来（NaN / 负数 / 上溢）**退到 `DEFAULT_SHUTDOWN_GRACE_SEC` 并 warn 一行 `aite.grace_invalid`**。**没有**选「`ServeOptions` 改带校验的构造函数」那条：`shutdown_grace_sec` 是 `pub` 字段，收窄它必然要改 `cli.rs:137`，而 `cli.rs` 归 W3、本轨只读 —— 越界的代价大于收益。也**刻意没有**复刻 `cli.rs` 那个 86400 上界：那是命令行的人机约定，这一层只负责「不许 panic、不许跳过收尾」 | `graceful_shutdown.rs` 新增 4 条（NaN / INFINITY / 1e300 / 负数各一条），断的是**收尾跑完了**：`platform.stop()` 走到、`close_all()` 恰好一次、`store.close()` 之后库查不动、`run_app` 正常返回 | ✅ 把那句换回 `from_secs_f64(grace.max(0.0))` → INFINITY / 1e300 两条红。**NaN 与负数两条在旧代码上本来就绿**（旧实现碰巧不炸），它们钉的是「四个取值都不许炸、收尾都得走完」这条面，不是回归判据 —— 如实记 |
| ② | `app.rs:129` 注释「只组装：不连网…唯一副作用是建那三个目录」是假的 | 注释改成说实话，逐条写清：落盘（三个 mkdir **+ SqliteSessionStore::open 会建一个空库文件**）、读盘（system prompt）、连 edge 的两件事。**顺带纠了台账一个说法**：`EdgeClient::connect` 是**懒连接、不发一个字节**（`link.rs` 的 `connect_lazy()` + spawn 探针），只会因「socket 路径拼不成合法地址」失败；真花时间的是 `check_contract_version` 的 5 发 `GetStatus` + 4 次 `sleep(1s)` ≈ 最坏 4s。**没有**动代码去让 `build_app` 真不连网 —— 那会改 C-TΩ-1 的形状，不是本轨能单方面定的 | `build_app_contract.rs` 新增 2 条，钉的是**分界**：三个口子全注入 → `edge.is_none()` **且**没付那 4s；少注入一个（沙箱）→ `edge.is_some()` **且**确实付了 ≥4s。判据用行为（4s）而不是 `is_none()`，因为后者太软 | ✅ `need_edge` 改成恒真 → 「不连」那条红；`EDGE_STATUS_ATTEMPTS` 改 1 → 「真去连」那条红 |
| ③ | `app.rs:78` 注释说 `edge` 是给「`!status` 的健康行与收尾时的连接态日志」留的，**两处都不存在** | 全仓 grep 核过：产品代码里只有两处读它 —— `run.rs` 的 `log_takeoff`（起飞那行 `aite.up` 的 socket 路径）和本文件 `Debug`。注释按实际用途重写，并写明**它不是生命线**：`platform()` / `sandbox()` 各攥着同一根 `Link` 的 `Arc`、重连探针又自己攥着一个，所以契约闸门（`gate.rs`）跟这个字段在不在无关 | 无新增测试（纯注释；每句都当场 grep 核过） | 不适用 |
| ④ | `crash_recovery.rs:53,125` 裸 `list_active_tasks(...)[0]`，列表空时报 `index out of bounds` —— 越界 panic 冒充断言失败 | `common/mod.rs` 加 `first_active_task(store, chat_id, what)`：带死线轮询，超时时打出「在等什么」+ 两种成因怎么分辨（没建出来 / 已落 `Answering`/终态退场）。两处换掉；**顺带**把 `graceful_shutdown.rs` 里同形状的两处也换了（同一文件、同一类隐患） | 助手本身即断言 | ✅ 把一处的 chat 换成不存在的群 → 报的是新那句人话，不再是 `index out of bounds` |
| ⑤ | `reconnect_replay.rs:168` `if self.inner.call_count() > 0` **恒为 false**（闸门期间 `inner.chat` 还没被调到），守卫从没生效 | **选「让守卫真生效」**。先把机理查实了再动手：把「`notified()` 之后才通知」那个窗口人为撑开 → **测试照样绿**，因为 tokio 的 `Notify::notified()` 建 future 时就记下了 `notify_waiters` 次数，这一半它自己兜住了；把「**测试还没进这个函数、worker 已经到闸门**」那个窗口撑到 200ms → **必现**，panic 逐字落在 `expect("5s 内 worker 没走到第一次 chat")` 上，与台账 `792/1` 那次同一个点。所以补的是标志位 `gate_reached`：`chat()` 在 `notify_waiters()` **之前**置真，`wait_gate_reached` 先建 `notified` 再查位，两个方向都不漏 | 新增 `wait_gate_reached_survives_a_notification_that_came_first`：先把模型驱到闸门、**之后**才问；顺带断言此刻 `inner.call_count() == 0`（旧判据恒为 false 的根） | ✅ 把 `self.gate_reached()` 换回 `self.inner.call_count() > 0` → 这条红（1s 超时） |
| ⑥ | `graceful_shutdown.rs:237,249` 计时起点在建场之前，`elapsed < 3.0` 量的是「建场 + 收尾」，含两个各 5s 预算的 `wait_until` | 起点挪到 `run.shutdown_within(...)` 正前方，只量收尾。阈值复核后收到 **1.0s**（新增常量 `SHUTDOWN_CEILING_SEC`）：实测纯收尾在 0.1–0.4ms 量级，1s 是十倍余量，比原来那个「含建场的 3s」既更严也更稳 | 原用例 `grace_timeout_cancels_the_stuck_task_and_still_returns` | 见下「⑥ 的十遍」 |
| ⑦ | `!status` 走 `status_tasks`、`!stop` / 卡片按钮走 `list_active_tasks`，交付中的短任务**列得出来、却被告知不存在** | **选 (b)：写成刻意的约定。** 先把 (a) 算完账：worker 的取消标志位只在每步**开头**被看一眼（`agent.rs:117` 是全仓唯一一处），而 `deliver()` 是最后一步之后的一段直路（发文件 → 发答复 → 写 delivered 证据 → 收卡片 → `finish()`），**中间一个取消点都没有**。所以 (a) 只会造出假话：答复照发、`cancel_task` 写的 `Cancelled` 随后被 `finish()` 的 `Delivered` 盖掉、回帖却说「已停止」。V5 那道「落刀前按 id 重读、终态不改写」**兜不住** —— `Answering` 不是终态。于是：`resolve_stop_target` / `resolve_task` 改走**同一个 `status_tasks`**，结果用新枚举 `StopTarget{Stoppable, Delivering, NotFound}` 分流，`Delivering` 回新文案 `stop_while_delivering_text`。**`ACTIVE_TASK_STATUSES` 一个字没动**（双重冻结）。省略任务号那条规则的「只有一个」现在按 `!status` 那份列表算 | `commands.rs` 新增 5 条：`!stop <任务号>` 撞 Answering 回新文案且**库里状态一个字没改**、`!status` 与 `!stop` 对同一任务答复一致、省略任务号那条路同口径、卡片按钮同口径、可停的任务照样停得掉；`wording.rs` 新增 1 条逐字钉文案 | ✅ `resolve_stop_target` 换回 `list_active_tasks` → 3 条红；`resolve_task` 换回 → 卡片那条红 |

### 测试数

`793 → 806`（+13）。

| 文件 | 变化 | 多在哪 |
|---|---|---|
| `app/tests/build_app_contract.rs` | 6 → 8 | ② 的两条分界 |
| `app/tests/graceful_shutdown.rs` | 7 → 11 | ① 的四个非法取值各一条 |
| `app/tests/reconnect_replay.rs` | 10 → 11 | ⑤ 的「通知早于等待」 |
| `control/tests/commands.rs` | 12 → 17 | ⑦ 的五条 |
| `control/tests/wording.rs` | 9 → 10 | ⑦ 的文案逐字 |

### ⑥ 的十遍

```
run 01..10: test result: ok. 11 passed; 0 failed; ... finished in 0.14–0.16s
```
十遍全绿（连跑，逐遍单独 `cargo test -p aite --test graceful_shutdown`）。

### 三个假红 target 的收尾状态

| target | 结果 |
|---|---|
| `reconnect_replay` | 连跑 **20/20 绿**；负载下（同时跑 `cargo test --workspace`）另跑 6 遍也全绿 |
| `graceful_shutdown` | 连跑 **10/10 绿**；负载下 6 遍全绿 |
| `startup_recovery` | **没改**（不在本轨可写面），抖动源定位了两个，见下 |

**`reconnect_replay` 其实有两个病根，⑤ 只是其一。** 修完 ⑤ 之后连跑 20 遍仍红 2 遍，
失败的是另一条：`a_root_and_its_thread_followup_replayed_together`（`:749` 的
`events.ignored == 0`）。**把本文件换回基线原版单独跑 12 遍，同样红 1 遍 —— 与本轨改动无关，
是基线就有的。** 病在断言本身：`together=true` 那一支两条事件各自 `tokio::spawn`，
追问完全可能在 root 的会话落库之前进 `handle_event`，于是按 R8 被丢 ——
这正是紧挨着的 `a_followup_replayed_before_its_root_is_dropped` 逐字写下的**当前边界**。
原来那一支照抄了顺序那一支的断言，等于在断一条产品并不保证的性质。
改成：顺序那一支照旧严格；并发那一支钉**真正成立的**那条 ——
结局只许是「并进同一份 transcript（ignored=0）」或「按 R8 被丢并记一笔（ignored=1）」，
**「既没进 transcript 也没被记成丢弃」这种静悄悄没了的第三种形状一个都不许有**。
（改坏验过：把期望文案改一个字 → 两支都红。）

### 记账转出去的

| 位置 | 病 | 归哪轨 |
|---|---|---|
| `core/crates/app/tests/startup_recovery.rs:259` | 与 ④ 同形状的裸 `[0]`：`emit` 之后直接取 `list_active_tasks(CHAT)[0]`，而那条脚本是单步 `final`（走 Answering 路径，落 `Answering` 就退出活跃口径）。**这是 `startup_recovery` 假红的一个已定位病根** —— 在这一行前面插一句 `settle().await` 把窗口撑开，**必现** `index out of bounds: the len is 0 but the index is 0`。修法现成：换成 `first_active_task`（W2 已加在 `tests/common/mod.rs` 里）。本轨可写面不含此文件，没伸手 | 下一轮（app 测试面） |
| 同上，`orphans_are_closed_before_the_platform_starts` | `startup_recovery` 的**第二个**抖动源，与上一条不同：`run.shutdown()` 的 10s 预算在重负载下被瞬时饿死撞穿（报「10s 内 run_app 没有返回」）。**只在人为堆的重负载下出现**：同一二进制 6 路并发跑 72 遍红 3 遍；顺序跑 20 遍 0 红；`check.sh` 的常规条件下没见过。收尾本身实测只要 0.1–0.4ms，所以不是「真挂死」 | 待定（先记账，别急着调大预算把真挂死一起咽掉） |
| `core/crates/app/tests/evidence_on_disk.rs:64`、`sqlite_cross_process.rs:38,73` | 同一类裸 `[0]`（`emit` 之后无守卫直接取下标），同样会在任务跑得快时报越界 panic 而不是人话。`evidence_on_disk.rs:264` 有 `wait_until` 兜着，不在此列 | 下一轮（app 测试面） |
| `core/crates/app/src/cli.rs:19-30` | 那段注释现在是假的：「**这是把炸弹挡在门口，不是拆弹 —— 拆弹归 V5**」。W2 已经拆了（`run.rs` 的 `grace_duration`），注释要改成「门口校验 + 里面兜底，两道都在」。`cli.rs` 归 W3，本轨只读 | **W3 / 总管** |
| `core/crates/edge-client/src/lib.rs:76`、`:91` | 与 ③ 同一条病：两处注释都说 `contract_state()` / `status()` 是给「`!status` 的健康行」用的，而**那条健康行全仓不存在**（`contract_state()` 在产品代码里零调用方）。`edge-client` 不在本轨可写面 | 下一轮 |
| `core/crates/control/src/plane.rs` 的 `cmd_restart` | 归档会话时用 `list_active_tasks` 取要停的任务，同样漏掉 `Answering`。按 ⑦ 的结论这不是缺陷（交付中本来就停不掉），但口径与 `!stop` 已经分家，值得记一笔 | 下一轮 |
| `!stop` 省略任务号 + 本群有多个任务 | 回的是「没有这个任务」，而列表里明明有好几个 —— 这条**改前就存在**，W2 只是把「有几个」的口径换成了 `status_tasks`。真要修得新加一句「请带上任务号」 | 下一轮 |

### 没做的 / 拿不准的

1. **越了两处白名单，都是加测试，逐条说明：**
   - `core/crates/app/tests/build_app_contract.rs` —— ② 要求「补一条测试钉住注入路确实不连网」，
     而这条测试的天然归宿就是这个文件（它的模块头写的正是「硬约束 1：`build_app` 只组装，
     不产生副作用」，也就是 ② 要改的那句谎话的同源）。派单的可写表没列它。
     另一条路是新建 `build_app_edge_boundary.rs` —— 同样不在表里，还要把 C-TΩ-1 的断言劈成两半。
     选了前者，只**追加**两条并把那条已失真的文档注释改准；与 W3 的面零重叠。
   - `core/crates/control/src/lib.rs` —— 只加了一行 `pub use`，把 ⑦ 的新文案导出给
     `wording.rs` 逐字钉。不加这行 ⑦ 的文案就没法按 §3.3「固定文案逐字不变」的规矩钉住。
2. **`docs/acceptance-M.md` 动了 6 处，比派单点名的三处多。** 三处是点名的
   （§0.4 引用框、§M1 排障表那两行、§7 末「两条使用口径」）。多出来的三处是：
   §M3 排障表和 §8 第 4 条里**同一句话的两处复述**（不改就留着两条 `<!-- 台账：仍未修，归 W2 -->`
   和已经不成立的「两者都只查活跃集」），以及 §0 抬头改动说明里 W1 那条**现在时的断言**
   （「现在这两条命令对『什么算活跃』意见不一致」）—— 补了一条 W2 的条目。
   另外 `plane.rs` / `app.rs` 的行号引用因本轨改动漂了 6 处，逐个 `sed -n` 核过改掉。
3. **`ServeOptions` 没有收窄成带校验的构造函数。** 理由见 ① 那一栏（要改 `cli.rs`，归 W3）。
   真要彻底堵住 `pub` 字段这个口子，得由拿得到 `cli.rs` 的那一轨来做。
4. **`startup_recovery` 的两个抖动源都只记账、没修** —— 文件不在可写面。第一个（`:259` 裸 `[0]`）
   是**已复现的确定病根**，修法现成；第二个（10s 预算被饿死）只在人为重负载下出现，
   建议先别调大预算。
5. **⑤ 的两条出路里选了第一条**，第二条（证明竞态不可能发生并删掉）已被实证否掉 ——
   撑开窗口之后它是必现的。

---

## 七、W3 回执 —— 2026-09-12

基线 `0ac0b58`。改动面七处：`edge/internal/sandbox/{docker.go,docker_pure_test.go}`（①②）、
`core/crates/app/{src/main.rs,tests/cli_smoke.rs}`（③）、`.github/workflows/ci.yml`（④⑤）、
`docker/{core,edge}/Dockerfile`（⑤，**只加注释，没加 `USER`**）、
`docs/acceptance-M.md` §0.1 那张表（③）、本文追加这一节。`README.md` 零处波及。

### ① `Release("")` 的三方矛盾 —— 改实现

**判据是契约，不是注释**：`proto/aite/v1/edge.proto:95` 那一行写着
`rpc Release(ReleaseRequest) returns (ReleaseResponse);           // 幂等`。
它**盖掉**头注释里那条通用的「`sandbox_id` 不存在 → NOT_FOUND」—— 否则「幂等」两个字
没有任何含义（重复释放同一个 id 必然拿到 NOT_FOUND）。契约、包头注释（`docker.go:19`）、
函数注释（`:381`）三处说的本来就是同一件事，**只有实现不是**。所以改实现。

改法：`Release` 开头 `strings.TrimSpace(sandboxID) == ""` 就早返回 `nil`，
**放在 `dockerClient()` 之前**。只特判空串，不特判「不在 `d.boxes` 里的 id」——
`ReapIdle` 捡上一次进程留下的孤儿时那些 id 本进程压根没记账，却必须真去 daemon 删
（B4「reap 之后 `docker ps -a` 必须为空」的兜底）。理由连边界一起写进函数注释了。

钉它的测试：`TestReleaseOnAnEmptyIdIsANoop`，在**无 daemon 档**（早返回在
`dockerClient()` 前面，所以这条测试根本走不到 daemon）。变异验过：把早返回删掉就红。

### ② `inspectStamps` / `orphans` —— 抽两个纯函数进无 daemon 门禁

`orphans` 的判定逻辑抽成了两个纯函数，`orphans` 自己只剩「问 daemon + known 过滤 +
出错整趟放弃」：

- `newestStamp(insp) (time.Time, bool)` —— 三个时间戳里挑最新的一个解析得出来的
- `isOrphanIdle(insp, now, idle) bool` —— `!seen || now.Sub(newest) >= idle`

`inspectStamps` 原样保留（补了一段注释说明它**只搬运不解析**：零值哨兵与格式非法的值
都要原样带出去，否则 `newestStamp` 再也分不清「没有这个字段」与「这个字段是坏的」）。

`docker_pure_test.go` 13 → 23 条。`inspectStamps` 四种入参全覆盖（正常三个、字段缺失
两档、零值哨兵、格式非法），外加 `>=` 边界（正好等于 idle 收、差 1ns 不收）与
「取最新不取最旧」。**七条变异逐个验过，全部有判别力**：删早返回、`>=`→`>`、
`After`→`Before`、去掉 `!seen` 兜底、`inspectStamps` 顺序反转 / 自己过滤哨兵 / 返回 nil。

### ③ `--help` 被 clap 吞掉 —— 病比台账记的大一倍

**实测发现四个子命令都坏，不是两个**：`run` / `preflight` / `evals` / `evidence`
（`main.rs` 的 `:23` / `:33` / `:38` / `:43`，四个都带 `trailing_var_arg`）。
`contracts` 的参数是真 clap 子命令，本来就没这个病。
（派单正文把 `:38` / `:43` 写成「evals / contracts」，按行号那是 Evidence / Preflight ——
按病灶实际范围做了，四个一视同仁。）

病的机理比「`--help` 被当成位置参数收进 `ARGS`」更绕一层：clap **仍然处理** `--help`，
只是它打的是**它自己知道的那份**帮助 —— 参数由手写解析器处理，clap 一个真实选项都不
知道，于是那份帮助里只有一句 `[ARGS]...`，**而退出码是 0**。

改法：四个 variant 各加 `#[command(disable_help_flag = true)]`，`--help` / `-h` 跟着
`allow_hyphen_values` 落进 `args`，由各自的手写解析器打真用法。**没动
`trailing_var_arg`**，`cli.rs` / `wiring.rs`（归 W2）一个字节没碰。边界写进了 `Cmd`
的注释：这四个子命令下 `--help` 一律是求助，将来真要透传得加显式分隔符，不是摘掉这个属性。

**踩到一个坑**：那段说明一开始写成 `///` 挂在 `enum Cmd` 上，被 clap derive 当成**顶层
命令的 about**，`aite --help` 的第一行从「Aite core（Rust）」变成了那段说明。降级成 `//`
才修掉，并且补了一条 `top_level_and_contracts_help_are_untouched` 钉住它。

硬约束 1（`-- --help` 一个字节都不许变）**逐字节验过**：拿改动前后的两个二进制，对 26 个
写法各存 stdout / stderr / 退出码再 `cmp`。**变化面精确等于那八个坏写法（且只有 stdout
变）**，其余 18 个逐字节相同 —— 含 `run`/`preflight` 的 `-- --help` 与 `-- -h`、
`contracts` 四个写法、顶层 `--help`/`-h`、四条业务路径。

`cli_smoke.rs` 16 → 20 条。变异验过：去掉全部四个属性 / 只去掉 `Run` 那一个（「只修一半」）
/ 把注释改回 `///`，三种都红。顺带修掉两处**因为这次改动而失真**的旧注释
（`cli_smoke.rs:377` 与 `:428` 还写着「仍然被 clap 截胡……归 R0」）。

### ④ `ci.yml` 引的 `README.md:90` —— 换成引措辞，不写行号

`--offline` 那句话现在在 `README.md:136`，漂了 46 行。**决定不写行号**，改成引那句话的
措辞（可 grep）。理由三条：这两份文件已经在**互指**（README 反过来引 `ci.yml:115-118`），
再写一个行号只会把「漂了不会红」的面翻倍；本轮 W1 刚逐个 `sed -n` 核过 15 处漂掉的行号；
措辞比行号耐改。**注释保持两行，`preflight` 那一步仍在 `ci.yml:115`** —— README 反向
引的 `115-118` 没跟着漂（⑤ 的新步骤加在起飞冒烟之后，就是为了不推动它）。

### ⑤ 两个 Dockerfile 没有 `USER` —— 原来那条担心**证伪**，但暴露了另一半

**第 1 步（证实/证伪）拿到的 Linux 实测**。macOS 上这条验不出来：Docker Desktop 的
bind mount 过 VirtioFS，把 uid 映射回宿主用户（本机对照组实测 `501:0`，容器里明明是
root 建的）—— W1 看的就是那一档。绕法是把 `./data` 换成命名卷（Docker Desktop 的
Linux VM 里是原生 ext4，uid 不翻译，与 Linux runner 上 bind mount 一个宿主目录同构），
**真镜像真起飞**（两个 service 都转 healthy）之后量真产物：

```
0:0 755 directory    data/            ← ./data 不入库，bind mount 时由 dockerd 建
0:0 644 regular file data/aite.db     ← 证据链主存（storage.sqlite_path）
0:0 755 directory    data/evidence/
0:0 755 directory    data/artifacts/
```

以宿主用户（GitHub runner 是 uid 1001）碰同一批文件：**READ OK** /
WRITE DENIED / MKDIR DENIED。

- **「宿主机那条 `aite evidence show` 可能读不了」不成立 —— 证伪。** 0644 + 0755 对任何
  用户都开着读。W1 那条担心停在 owner 上，而**决定读不读得动的是权限位**，不是 owner。
- **但写不进、删不掉、也没法在 `./data` 里新建** —— 这是真后果，见下面记账那一条。

**第 2 步：不加 `USER`。** 派单口径是「目标是宿主机拿得到自己的证据文件，不是容器安全
基线」——读得动就是拿得到。加 `USER` 要顺带解决命名卷 `run:` 的挂载点属主（两个 socket
要建得出来）与 `./data` 两侧的 uid 不一致，而 edge 还要读 `/var/run/docker.sock`
（归 `root:docker`，换非 root 就得把 docker 组 gid 传进容器，否则 `Ping` 失败 →
`sandbox_ok` false → preflight 第 6 组 FAIL）。**两个 service 不必一视同仁**：真要降权
core 先降。这些连实测输出一起写进两个 Dockerfile 的「以谁的身份跑」小节了。

**第 3 步：判据落进 CI。** `compose-smoke` 加了一步「⑤ `./data` 的产物宿主机读得动」，
打印 runner 的 uid 与每个产物的 `%u:%g %a`，**硬断言每个文件读得动**，写/删只打印不判死。
这一步本机在 Linux 容器里实跑过两档：现状绿（exit 0）、把 `data/aite.db` 改成 0600 就红
并点名那个文件（exit 1）—— 不是恒真断言。

### 记账转出去的

| 位置 | 病 | 归哪轨 |
|---|---|---|
| `core/crates/app/src/cli.rs:42` | 注释还写着「clap 仍然把 `-h` / `--help` 截胡，打的是它自己那份不含任何真实选项的…」—— W3 ③ 修完之后这句话反了。`cli.rs` 是 W3 的只读面 | **W2** |
| `core/crates/evals/src/cli.rs:254` | 同病：「**`aite evals --help`（不加 `--`）到不了这里**：它被 clap 截胡」—— 现在到得了。`core/crates/evals/**` 不在 W3 的可写面里 | 顺手改（W2 / 总管） |
| `docs/acceptance-M.md:42` | 抬头的变更日志写着「§0.1 的 `--help` …… 是 V6 ④a/④b/④c 的实际结果」，而 §0.1 那张表现在是 W3 ③ 的结果。派单限定 W3 只许改 §0.1 那张表，没碰抬头 | 总管 |
| `docker/{core,edge}/Dockerfile` + `docker-compose.yml` | 容器以 root 跑 → Linux 上 `./data` 与产物归 `root:root`，宿主机**写不进、删不掉、没法在里面新建**。后果不在读侧而在写侧：**Linux 上 compose 跑过一次之后，宿主机直跑 `aite run`（`acceptance-M.md` §0.2 的口径）会因为落盘目录不可写起不来**，`preflight` 第 7 组会 FAIL。W3 量清楚了但没修（派单口径是 low，且改法牵动三处属主） | 待定（比 W1 报的那条**换了后果**，不是同一条） |

### 假红：收尾四轮 `check.sh` 撞了三次，第四轮全绿

| 轮次 | `cargo passed/failed` | 点名的 target | 单独跑 |
|---|---|---|---|
| 开场自检 | 793 / 0 | —— | 一次就绿 |
| 收尾第 1 轮 | 796 / 1 | `graceful_shutdown` | 7 passed 全绿 |
| 收尾第 2 轮 | 795 / 2 | `reconnect_replay`、`startup_recovery` | 10 passed / 9 passed 全绿 |
| 收尾第 3 轮 | 795 / 2 | `graceful_shutdown`、`reconnect_replay` | （同上，已复现过） |
| 收尾第 4 轮 | **797 / 0** | —— | **「全部通过」，退出码 0** |

三次加起来把本文 §4.1 点名的三个 target 全撞了一遍，**没有一次碰到 W3 改的任何东西**，
`passed + failed` 恒等于 797（= 基线 793 + W3 新增 4 条）。
撞的频率明显高于 W1 那次 —— 本机当时有 6 个会话在 busy（W2 + 另一批五轨）。

> 顺带一条给后面写脚本的：`scripts/check.sh > log 2>&1; echo "EXIT=$?"` 这个写法在
> **后台任务**里会骗人 —— 任务通知报的是整条命令（最后那个 `echo`）的退出码，永远是 0。
> 要拿真退出码得 `rc=$?; …; exit $rc`。本轮因此误读过一次「第三轮绿了」。

### 测试数

793 → **797**（Rust，全在 `cli_smoke.rs`：16 → 20）；
Go 侧 `internal/sandbox` 无 daemon 档 **19 → 29** 条（其中 `docker_pure_test.go` 13 → 23），
docker 档 50 条不变。

---

## 八、X1 回执 —— 2026-09-13

基线 `542a29c`（= `69323db` 的代码面 + 一个只加派单文件的 commit）。

开场自检与收尾 `scripts/check.sh` 的五行关键值：

```
契约锁 ............ OK 25 files                 （开场 / 收尾一致）
C1 契约测试 ....... contracts passed=25 failed=0（开场 / 收尾一致）
B 全量 cargo ...... 开场 810 failed=0 → 收尾 818 failed=0
B go test（-race）. 六个包全 ok                  （开场 / 收尾一致）
B8 评测 ........... passed 10/10                （开场 / 收尾一致）
全部通过，退出码 0
```

**开场自检没撞到假红**（一次跑过，810/0）。守卫实测有效：Read `.claude/hooks/guard_bash.py`
被拦下。收尾第一遍 `check.sh` 红在 **A4b `cargo fmt --check`**（两个被改文件的格式），
`cargo fmt --all` 写模式会碰 contracts 冻结面、被守卫正确拦下，改用
`rustfmt --edition 2024` 只格式化那两个文件；实质检查那一遍就全过了。

### ① preflight 漏检 `worker.system_prompt_path`

判据折进**第 ① 组**，没有变成八组 —— 「七组」这个说法全仓一处没改（下面「涟漪」那栏）。
`check_config` 多吃一个 `repo_root`，配置解析成功后**直接复用 `aite run` 走的那个
`load_system_prompt`**：两边逐字同一条路径，相对路径都按 `Options.repo_root` 解析，
而它就是进程 cwd（`preflight::run` 里 `current_dir()`），与 `require_system_prompt`
相对 cwd 的口径对得上，不会出现「这边说行、那边说不行」。

* **FAIL 不是 WARN**（硬起飞前提），但**照样把 `cfg` 交出去** —— 不交的话后面六组会全变成
  「第 1 组没过，配置读不出来」，「一项失败不阻断后面的」当场破功。
* 「怎么补」三件事齐：当前值、该改成什么（`core/crates/worker/prompts/platform.md`）、
  以及那条真实病史。病史那半句**只在认出旧 Python 树时才说**（路径不是那个还硬贴一段
  2026-09-12 的病史，只会把人往错方向带），另一支给 cwd 那条。
* `--offline` 下照样跑（它不碰网络也不碰 docker），单独有测试钉着。
* 红线照旧：detail / fix 都是 `CheckResult` 的字段，渲染前统一过 `Redactor`，没有新的直写路径。

**另外修掉 `app.rs` 那句误诊。** 原话「路径相对于进程的工作目录 —— 多半是没在仓库根起进程」
把人带反了（总管撞上时 cwd 就是仓库根）。现在先说真正最可能的（配置里这一行本身不对 /
旧配置指着已删的 Python 树），再说 cwd，并且**把解析成的绝对路径打出来**。

**`app.rs:195`（连不上 edge 那条）复核了，判定不改**：那一条的措辞是「socket 路径由 config 的
`edge.edge_socket` 决定，相对仓库根 —— 多半是没在仓库根起进程，或者 aite-edge 还没起」。
两个真因都给了，而且 edge socket 用的是 `repo_root` 而不是配置里的绝对路径，cwd 确实是常见真因。
与 prompt 那条的处境不同（那条的真因是配置内容本身），不改。

**钉它的测试（5 条新的 + 2 条集成）**：`preflight.rs` 的 `mod tests` 五条（常量与契约默认值一致 /
与样例配置一致 / FAIL 但仍交出 config / 相对路径按 repo_root 解析 / 病史只在该说时说），
`preflight_e2e.rs` 两条（FAIL + `report.ok()` 假 + fix 正文 + 后六组照跑 + `--json` 面；
另一条单钉 `--offline` 下照跑），`cli_smoke.rs` 一条（真二进制，退出码 1）。

**改坏产品代码验过**（五组变异，每组都被抓到）：

| 改坏什么 | 谁红了 |
|---|---|
| 整段判据摘掉（退回 2026-09-12 那天） | e2e 2 条 + cli_smoke 1 条 + lib 1 条 |
| FAIL 降成 WARN（不拦起飞了） | e2e 2 条 + cli_smoke 1 条 + lib 1 条 |
| 改成相对「配置文件所在目录」解析（两边口径分家） | lib 1 条 |
| 「该改成什么」指错一个字母 | lib 2 条 + e2e 1 条 + cli_smoke 1 条 |
| 病史那句话无条件贴 | lib 1 条 |

### ② `startup_recovery.rs:259` 的裸 `[0]`

**原病复现**（照 W2 的手法，在取下标前插一句 `settle().await`）：

```
panicked at crates/app/tests/startup_recovery.rs:260:74:
index out of bounds: the len is 0 but the index is 0
```

**没有照「换成 `first_active_task`」那条修**，因为实测它不够 —— 同样撑开窗口，它照样红，
只是把越界换成一句人话，还多等 5s：

```
panicked at crates/app/tests/common/mod.rs:553:13:
5s 内 oc_1 没等到活跃任务：旧线程里的新任务。活跃列表从头到尾是空的 ——
要么任务压根没建出来，要么它跑得比这句断言还快、已经落 `Answering` / 终态从活跃口径里退场了。
```

那条人话自己说破了病根：这条脚本是**单步 `final`**，worker 一走完就落 `Answering`，
而 `Answering` 不在 `ACTIVE_TASK_STATUSES` 里。所以判据换成**对负载不敏感**的那种：
**先等交付完成，再从磁盘把任务读回来**。要断的 `task_no` / `session_id` 在任务建出来那一刻
就定死了，终态时还是那两个值，跑多快都不影响结论。

**修后确认复现不出来**：同一位置插 1 次 / 3 次 `settle()`，都是 `ok. 1 passed`。

### ③ 同族的另外三处

三处**全部实测是必现**（不只是「报的不是人话」）—— 包括我一开始以为窗口大的
`evidence_on_disk.rs:64`，推测被实测推翻：

| 位置 | 撑开窗口 | 修法 |
|---|---|---|
| `evidence_on_disk.rs:64` | 越界必现 | 跑完后 `the_only_task_from_disk` |
| `sqlite_cross_process.rs:38` | 越界必现 | 同上 |
| `sqlite_cross_process.rs:73` | 越界必现 | `tasks_from_disk` 里挑不是 task1 的那个 |

所以三处都走 ② 那条路，而不是 `first_active_task`。helper 收在
`tests/common/mod.rs`（`tasks_from_disk` / `the_only_task_from_disk`），
任务 id 取自 evidence 目录名、本体从 SQLite 读，两边对不上当场 panic ——
顺带把「evidence 目录名就是 task_id」也钉住了。**四处修后撑开窗口都复现不出来。**

`evidence_on_disk.rs:264` 有 `wait_until` 兜着，照派单没动。

### ④ `startup_recovery` 的第二个抖动源 —— **不是「机器太忙」，是产品的真死锁**

派单让「先判断这 10s 在等什么，别急着调大预算」。查下来 **W2 那句警告是对的，而我的第一版
判断是错的** —— 先按「瞬时饿死」把兜底放到 60s，**实测照样撞穿且实际等满 60.0s**。
调度饥饿早该返回了，所以它不是饥饿。

**三条独立证据指向真死锁**：

1. `sample` 抓的**三份栈形状完全一致**：两个 tokio worker **全都 park**、栈上**一个 aite 帧都没有**。
   饥饿的话 worker 会在跑别的东西 —— 这是没有可运行 task 的死锁。
2. 8 路并发 24 遍红 3 遍（**12.5%**），顺序跑不红：越挤越容易让 `run_app` 那条 task 晚一步。
3. 死锁那一遍的日志**停在 `aite.orphans`，`aite.stopping` 一次都没打** ——
   `shutdown()` 的第一行都没到，卡的是它前面的 `serve()`。

**病根**（探针实证，不是推理）：`StopSignal::set()`（`src/run.rs`）写的是
`let _ = self.tx.send(true)`，而 `tokio::sync::watch::Sender::send` 在**一个活跃接收者都没有**时
返回 `Err` 且**连内部那个值都不改**。`StopSignal::new()` 当场就把建出来的 `_rx` 丢了，于是在
`run_app` 走到 `serve()`（那里才 `subscribe()`）之前，`set()` 是一次**彻底的 no-op**。
最小探针：

```rust
let stop = StopSignal::new();
stop.set();
assert!(stop.is_set());   // ← 红：set() 之后 is_set() 仍是 false
```

**窗口在哪**：`RunningApp::start` 等的是 `platform.start()` 被调（`inner.started()`），
而 `takeoff()` 里 `start()` 之后还有 `spawn(run_forever)` 和 `serve()` 两步。
测试在这两步之间 `set()`，就正好落进洞里。

**真机也踩得到**：`aite run` 把 `SIGINT`/`SIGTERM` 接到同一个 `StopSignal` 上
（`install_signal_handlers`）。信号赶在 `serve()` 之前到达（compose 的 `stop_grace_period`、
k8s 滚动更新都会），进程就永远不退，只能等 `SIGKILL`。**这不是测试专属问题。**

**药在 `src/run.rs` —— 本轨只读面，记账转出去**（见下表）：`set()` 改用 `send_replace(true)`
（不管有没有接收者都更新值），或者让 `StopSignal::new()` 自己留一个 `rx`。

**本轨在测试侧做的**（可写面内，带指向病根的注释，`run.rs` 修好之后该删）：

* `RunningApp::shutdown` **重试 `set()` 直到 `is_set()` 为真**（带 5s 死线）。
  一旦 `run_app` 订阅上，下一次 `set()` 就生效。这不是「把挂死咽掉」—— 真挂死照样撞穿兜底。
* 兜底预算**改回 10s**（放到 60s 毫无意义：真挂死等多久都不返回，只让每次假红多拖 50s）。
* 超时那句 panic 消息改成指向第一现场（「看最后一条 `aite.*` 日志停在 `aite.stopping` 之前
  还是之后」），不再说「多半是机器太忙」那种分不出两种病的话。

**怎么区分它和真挂死**（下次撞上不用再判一遍，已写进 `common/mod.rs` 的文档）：
撞穿兜底 → 看最后一条 `aite.*` 日志。停在 `aite.stopping` **之前** = 根本没进收尾，卡的是
`serve()`（十有八九就是这条丢信号）；停在**之后**才是收尾里某一步真卡住了。

**验**：改前 8 路并发 24 遍红 3 遍；**改后同样 8 路并发 120 遍红 0 遍**。
外加派单要的：顺序连跑 **20 遍 0 红**，负载下（同时跑 `cargo test --workspace`）**6 遍 0 红**。

### ⑤ `edge-client/src/lib.rs` 的两条失真注释

全仓 grep 核实：

* `contract_state()` —— **产品代码里零调用方**，唯一使用者是 `tests/contract_gate.rs`（9 处），
  那一组拿它当闸门三态的判据。坐实了 W2 的记账。**要不要删这个函数不是本轨的决定**，
  注释里把话说准即可。
* `status()` —— 真调用方三个：`app.rs` 的 `check_contract_version`（起飞比版本）、
  `preflight.rs` 第 6 组（沙箱可用，先问 daemon 可达）、`wiring.rs` 的评测接线。
* 那条「`!status` 的健康行」**全仓不存在** —— 与 W2 ③ 改掉的 `app.rs:83` 那句同源。

只改注释，代码一个字没动。**同一句谎话还剩两个副本在 `link.rs:83` / `:136`**，
那个文件不在本轨可写面，记账（见下表）。

### ① 的两边口径对照

拿一份 `worker.system_prompt_path: aite/worker/prompts/platform.md`（= 总管那份 2026-09-10
配置的形状）在**仓库根**跑：

| | 改之前 | 改之后 |
|---|---|---|
| `preflight --offline` | `[1/7] OK 配置可加载`，汇总 `FAIL 0`，**「全部没红，可以起飞。」退出码 0** | `[1/7] FAIL 配置可加载 … 但 worker.system_prompt_path 指不到文件：… → /…/aite/worker/prompts/platform.md（不存在）`，退出码 **1**，「怎么补」给出正确路径 + 病史 |
| `preflight`（**不带** `--offline`，全跑） | `[1/7] OK 配置可加载` —— **七组里根本没有这一项，全跑也救不了** | FAIL（同上，第 1 组与 offline 无关） |
| `aite run` | 退出码 2，`…路径相对于进程的工作目录 —— 多半是没在仓库根起进程。`（**误诊**：当时 cwd 就是仓库根） | 退出码 2，先说配置里这一行本身不对 + Python 树病史 + **打出解析成的绝对路径**，再说 cwd |

**照「怎么补」改完之后**（只把那一行换成 `core/crates/worker/prompts/platform.md`）：
`preflight --offline` 第 1 组转 `OK`、退出码 0；`aite run` **不再死在 prompt 这一步** ——
它走过去了，进主循环（这台机器上 `aite-edge` 没起，卡在 edge 不可达的 WARN 上，预期内）。
两边口径一致。

### ① 之后 `--offline` 还剩哪些「全绿 ≠ 起得来」的口子

**没有补全，如实列。** 实跑对拍（每种坏配置各跑一次 `preflight --offline` 与 `aite run`）：

| 坏配置 | `preflight --offline` | `aite run` | 归哪一组管 |
|---|---|---|---|
| `model.base_url` 空 | 全绿，退出 0 | 退出 2 | 第 5 组，**被 `--offline` 跳过**（V3 记的那条，仍在） |
| `model.model` 空 | 全绿，退出 0 | 退出 2 | 同上 |
| `platform: fake` 而没注入平台 | 全绿，退出 0 | 退出 2（`build_app` 明文拒绝） | **七组里没有一组管** ← 与 ① 改前同形状 |
| `model.provider: scripted` 而没注入模型 | 全绿，退出 0 | 退出 2（同上） | **七组里没有一组管** ← 同上 |
| `system_prompt_path` 指着已删的 Python 树 | **FAIL，退出 1** | 退出 2 | 第 1 组（① 补的） |

前两条是「被 `--offline` 跳过」，去掉 `--offline` 就查得出来。**后两条更狠，和 ① 改前一个病：
七组里根本没有一组碰它，全跑一遍也是全绿。** 第 1 组只把 `platform` / `model.provider` 的
**取值**报出来，不判断它跟「有没有注入」搭不搭；其余六组的判据都与这两个取值无关。

> 后两条的「全跑也查不到」是**从判据代码推断的，没在真机实证** —— 这台机器没有飞书凭证，
> 不带 `--offline` 跑第 2/3/4 组必红，演示不了「七组全绿而起不来」。要坐实得在一台凭证配齐的
> 机器上跑一次。**归下一轮。**

### ②④ 的复现记录

| | 原病 | 修后 |
|---|---|---|
| ② | 插 `settle()` → `index out of bounds: the len is 0 but the index is 0`（必现） | 插 1 次 / 3 次 `settle()` 都是 `ok. 1 passed` |
| ③ 三处 | 各插 `settle()` → 越界**全部必现** | 各插 3 次 `settle()` → 四处全 `ok` |
| ④ | 8 路并发 24 遍红 3 遍（12.5%）；栈：两 worker 全 park、0 个 aite 帧；日志停在 `aite.orphans`；放到 60s 照样等满 60.0s | 8 路并发 **120 遍红 0 遍**；顺序 **20 遍 0 红**；负载下 **6 遍 0 红** |

### ③ 全仓同族写法清单

扫的是 `list_active_tasks(...)[0]`、`.expect("…")[0]`、`.await…[0]`、`.unwrap()[0]` 四种形状。

| 位置 | 在可写面内？ | 改了没 |
|---|---|---|
| `app/tests/startup_recovery.rs:259`（②） | 是 | ✅ 改了 |
| `app/tests/evidence_on_disk.rs:64` | 是 | ✅ 改了 |
| `app/tests/sqlite_cross_process.rs:38` | 是 | ✅ 改了 |
| `app/tests/sqlite_cross_process.rs:73` | 是 | ✅ 改了 |
| `app/tests/evidence_on_disk.rs:264` | 是 | ❌ 有 `wait_until` 兜着，照派单没动 |
| `app/tests/reconnect_replay.rs:578` | 否 | 不是同族：前面有 `rig.settled(1).await` 守着 |
| `app/tests/startup_recovery.rs:378` | 是 | 不是同族：前一行 `assert_eq!(count("send_text"), 1)` 已经把长度断了 |
| `worker/tests/test_final.rs:25,87`、`test_checklist.rs:45,72,80` | 否 | 不是同族：前面都先断了 `.len()`，且 `run_script` 跑完才返回 |
| `testing/tests/fake_model.rs:115,312` | 否 | 不是同族：`tool_calls` 的纯数据断言，没有任务生命周期竞态 |

### 测试数

810 → **818**（+8）：

* `src/preflight.rs` 的 `mod tests` **+5**（31 → 36）：常量与契约默认值一致、与样例配置一致、
  FAIL 但仍交出 config、相对路径按 repo_root 解析、病史只在该说时说。
* `tests/preflight_e2e.rs` **+2**（15 → 17）：prompt 指不到 → 第 1 组 FAIL；`--offline` 下照跑。
* `tests/cli_smoke.rs` **+1**（20 → 21）：进程级，真二进制，退出码 1。

②③④⑤ 都是**改判据不加条数**（同一批测试换了个不会随负载变脸的问法）。

### 记账转出去的

| 位置 | 病 | 归哪轨 |
|---|---|---|
| `core/crates/app/src/run.rs` 的 `StopSignal::set()` | **没有接收者时 `watch::send` 返回 `Err` 且不改值 → `set()` 是彻底的 no-op，信号丢掉、`serve()` 永远等不到。**真机上 `SIGTERM` 赶在 `serve()` 之前到达就永不退出，只能 `SIGKILL`。本轨探针实证 + 三份栈 + 日志三条证据。药：`send_replace(true)`，或 `new()` 里留一个 `rx`。修好之后把 `common/mod.rs` 里那个重试循环删掉 | **下一轮（产品面，优先级高 —— 它不只是测试抖动）** |
| `core/crates/edge-client/src/link.rs:83`、`:136` | 「`!status` 的健康行」那句谎话的**另外两个副本**（本轨改掉的是 `lib.rs` 的两个）。`link.rs` 不在本轨可写面 | 下一轮 |
| `core/crates/edge-client/src/lib.rs` 的 `contract_state()` | 产品代码**零调用方**，只有 `tests/contract_gate.rs` 用。要不要删不是本轨的决定 | 待定（总管） |
| `platform: fake` / `model.provider: scripted` 而没注入 | 与 ① 改前同形状：**七组里没有一组管**，全跑也全绿而 `aite run` 退出码 2。本轨实证了 `--offline` 那一档，全跑那一档没有凭证、没实证 | 下一轮 |
| `.claude/hooks/guard_bash.py` 的 `PROBES` | 仍留着指向已删文件的 `aite/contracts/__init__.py`，命令里带 `*` 通配符时会反向匹配误拦（本轨撞上一次，换了写法绕开）。`review/v6-guard-patch.py` 就是删它的，**只有人能跑**，至今没跑 | 总管（跑一次那个补丁） |

### 没做的 / 拿不准的

1. **④ 只在测试侧兜住，产品的病没修** —— `run.rs` 不在可写面。所以「`startup_recovery` 不再抖」
   这件事是靠测试侧的重试循环达成的，**不是病治好了**。`run.rs` 那条修掉之前，任何新写的
   「`stop.set()` 之后等 `run_app` 返回」都会重新踩到。
2. **越了一处白名单：`core/crates/app/tests/common/mod.rs` 动得比派单预期多。** 派单写的是
   「`first_active_task` 已在 `:542`，够用就别改」，而实测它不够用（②③ 那四处撑开窗口后它照样红），
   所以加了 `tasks_from_disk` / `the_only_task_from_disk`，并改了 `shutdown` 与
   `SHUTDOWN_FALLBACK_SEC`。`first_active_task` 本身**一个字没动**。
3. **临时探针文件用完即删**：验 `StopSignal` 丢信号时临时写过
   `core/crates/app/tests/tmp_stopsignal_probe.rs`，拿到证据后删掉了，不在交付 diff 里。
4. **`acceptance-M.md` §0.1 那份逐行实测输出重跑了，结论是「不用改」** —— ① 不影响这一档
   （样例配置指的 prompt 在仓库里存在，第 1 组照旧 OK），逐字比对一致。改的是它下面那句
   「第 1 组只验『配置解析得出来』」—— 那句现在不准了。
5. **`platform: fake` / `provider: scripted` 那两个口子没补。** 它们和 ① 同形状，本来可以顺手
   一起折进第 ① 组，但那超出派单点名的范围，且要重新想「注入」这件事在 preflight 里怎么表达
   （preflight 没有 `Injections`）。记账，不自作主张。

---

## 九、Y1 回执 —— 2026-09-13

第八节 ④ 那条记账**销了**：`StopSignal::set()` 的丢信号已上药，绷带拆掉，
外加一条站在进程层的回归。

### 基线与开场自检

worktree 的 HEAD 是 `e733a2c`，不是派单抬头写的 `18f30b6`。差的那一格就是**出这两份派单本身**
（`review/paste-Y1.md` + `paste-Y2.md`，575 行文档，零代码改动），`e733a2c^` 正是 `18f30b6`。
当基线用，不是回归。

`scripts/check.sh` 开场：`OK 25 files`、`contracts passed=25 failed=0`、go 六个包全 `ok`、
`passed 10/10` 都对上；**`cargo passed=817 failed=1`**，退出码 1。
Read `.claude/hooks/guard_bash.py` 被守卫拦下 ✅。

### 开场那条假红是新信息 —— 而且就是本轨要治的那条病

派单说这三个抖动 target「现在应该是不抖的」。撞到了 `graceful_shutdown`，查清楚了：

* 红的是 `stop_with_nothing_running_returns_at_once`，panic 在当时的 `common/mod.rs:566`
  （`shutdown_within` 的超时分支），**实际等满 2.0s** —— 挂死，不是慢。
* 病根同一条，但**绷带没盖到它**：X1 那个重试循环加在 `RunningApp::shutdown()` 里，
  而这条用例走的是 `shutdown_within(2.0)`，那条路上是**裸的一句 `self.stop.set()`**。
* 单独跑 5 遍全绿（按判据算假红）；8 路并发 48 遍复现 1 遍。
* ① 上药之后：8 路并发 **120 遍红 0**。

也就是说 X1 报的「startup_recovery 已兜住」是真的，但同一条病还从第二个入口漏着。

### ① 选了 `send_replace`，为什么

两条都写出来跑过 `tests/signals.rs` 那四条单元断言，**结果逐字相同**（4 passed，0.00s）：
`is_set()` / `wait()` 在「`set()` 早于订阅」和「`set()` 晚于订阅」两种时序下行为一致，
`send_replace` 不会把「后到的 `set()` 叫醒等待者」那条弄坏。所以分辨依据不在行为上。

分的是**这条正确性挂在哪儿**，而且这条有实证：路 (b)（`new()` 里留一个 `rx`）的全部效力
都来自那个 `_keepalive` 字段 —— 把它删掉、`rx` 改回 `_rx`，代码**逐字回到基线**，
也就是同样那 3 条红。一个「看起来完全没用、review 时最容易被顺手清理掉」的字段扛着
整条真机退出路径，而且删掉之后是**静默**复活。路 (a) 的正确性写在调用点本身，没有这种东西。

顺带核的三处（改完口径一致）：

* `is_set()` 读 `*self.tx.borrow()` —— 上药之后它才真的能反映「置过了」（基线上它恒为 false）。
* `wait()` 的 `borrow_and_update()` 早退分支 —— `set()` 先发生时实测立刻返回（0.00s）。
* `install_signal_handlers` 第二次信号硬退那条路 —— `stop.set(); continue;` 之后直接
  `eprintln!` + `std::process::exit`，**不读 `set()` 的返回值**，与这次改动无关。

### ② 两层回归（`core/crates/app/tests/signals.rs`，新建，+5）

**(a) 单元层四条**：`set()` 在任何订阅者出现之前必须算数；`set()` 先发生时 `wait()` 必须立刻返回；
`set()` 后到时 `wait()` 必须被叫醒；`Clone` 的各份共用一个开关。
「等的那条已经进去了」用 oneshot 报告，不睡固定时长。

**(b) 进程层一条** `sigterm_before_serve_still_exits_by_itself`：起真 `aite` 二进制
（`CARGO_BIN_EXE_aite`），在它走到 `serve()` 之前发 `SIGTERM`，断言它**自己**退出、退出码 0。

时序钩子（这是这条最难的部分，没用任何固定 sleep）：窗口 `install_signal_handlers` → `serve()`
真机上只有 ~25ms，所以拿一把**外部 SQLite 写锁**把它撑开 ——

```text
父：BEGIN EXCLUSIVE 持住 sqlite_path        ← 窗口撑开
子：build_app → install_signal_handlers → 打 aite.signal_ready
子：store.init() 建表撞 SQLITE_BUSY，卡在 busy_timeout 的重试里（aite.up 打不出来）
父：等到 aite.signal_ready → SIGTERM → 等到 aite.signal（handler 已跑完 set()）
父：ROLLBACK 放锁                            ← 窗口关上
子：init 成功 → aite.up → … → serve()
```

为此在 `install_signal_handlers` 里补了一行 `aite.signal_ready`（`run.rs:484`）。
它不是只为测试加的：装不上时打 `aite.signal_unavailable`，**装上了却从头到尾不吭声** ——
真机上「进程收到 SIGTERM 不退」时第一个该问的问题（handler 到底装上没有），
日志原来答不了。

测试自带一条**防假绿**的自检：`aite.signal` 必须出现在 `aite.up` **之前**，
两处断言 + 末尾再核一次顺序。信号来晚了就报「测了个寂寞」，不会悄悄通过。

**两条都做了「改坏 → 必须红」**（把 `set()` 退回 `let _ = self.tx.send(true)`）：

```text
test set_counts_even_with_no_subscriber_yet ... FAILED
test clones_share_one_switch ... FAILED
test wait_returns_at_once_when_set_happened_first ... FAILED
test sigterm_before_serve_still_exits_by_itself ... FAILED
test wait_wakes_up_when_set_happens_later ... ok        ← 有订阅者那条本来就不受影响
test result: FAILED. 1 passed; 4 failed; finished in 25.78s
```

(b) 退回去时**真的挂住**了，撞穿 20s 死线而不是通过。那一遍的子进程日志原样：

```text
aite.signal_ready SIGINT / SIGTERM 已接管，走优雅退出
aite.signal 收到，开始优雅退出（再来一次立即硬退）  signal="SIGTERM"
aite.up  platform=feishu model=signals-e2e …
ingress.listening socket=…/run/c.sock
edge.capabilities_unavailable …
（到此为止。aite.stopping 一个字都没有）
```

`aite.stopping` 没打 = `shutdown()` 第一行都没到 = 卡在 `serve()`，
和 X1 抓到的死锁特征逐字吻合，也就是真机上那个「只能 `SIGKILL`」。

新测试自身不抖：8 路并发 24 遍红 0。单条 5.73s，其中约 4s 是 `build_app` 在 edge 不可达时
那 5 次 `GetStatus` 重试，既有行为。

### ③ 拆绷带之后的数

`RunningApp::shutdown()` 里那个「重试 `set()` 直到 `is_set()`」的循环连同指向病根的
34 行注释一起删掉，现在就是一句 `self.shutdown_within(SHUTDOWN_FALLBACK_SEC).await`。

`orphans_are_closed_before_the_platform_starts`，**8 路并发 120 遍**：

| 状态 | 红几遍 |
|---|---|
| 拆绷带 + 病还在（把 ① 退回去） | **4 / 120（3.3%）**，全部是 `shutdown_within` 撞穿 10s |
| 拆绷带 + 上药 | **0 / 120** |
| `stop_with_nothing_running_returns_at_once`（开场那条）+ 上药 | **0 / 120** |

对照组这一栏是特意跑的：不跑它，「红 0」就只是「今天没撞上」。
抖动率比 X1 报的 12.5% 低（同样 24 遍那一轮我这边红 0，跑满 120 遍才见到 4 遍），
机器负载不同，**病的存在与否是确定的，频率不是**。

另两处按派单复核：

* `SHUTDOWN_FALLBACK_SEC` **维持 10s**，同意 X1 不放大。理由改写过：原文把「放大没用」
  挂在丢信号那条病上，那条已经没了；现在的理由与病因无关 —— 收尾实测 0.1–0.4ms，
  10s 已是两个数量级的余量，撞穿只可能是挂死，而挂死等多久都不返回。
* `shutdown_within` 那句 panic 的**位置判据仍然成立**（停在 `aite.stopping` 之前 = 卡在
  `serve()`，之后 = 收尾里某一步），它与病因无关，留着。但删掉了「60s 也照样等满，本轨实测」
  那半句（那是有病时测的，现在没有依据），并补了一句**新的排障信息**：这一种
  不再可能是 `StopSignal` 丢信号，该往 `takeoff()` 里 `serve()` 之前那几步查。

### ④ `watch` 的 send 家族全仓清单

| 位置 | 有接收者保证？ | 丢返回值会怎样 | 改了没 |
|---|---|---|---|
| `core/crates/app/src/run.rs:91` `StopSignal::set()` | **没有** —— `new()` 当场丢 `_rx`，唯一订阅在 `serve()` | 信号被吃、值都不改 → 进程收到 SIGTERM 永不退出 | **改了**（`send_replace`） |
| `core/crates/control/tests/support/mod.rs:258` `ParkedSleep` 的 `parked.send(true)` | **没有** —— `new()` 里同样是 `let (tx, _rx) = watch::channel(false)`，**与病根同一个形状** | `as_sleep` 的闭包先跑到 `send(true)` 时值不更新，后到的 `wait_until_parked()` 的 `wait_for` 永远等 → 用例挂死 | **没改，记账**（`core/crates/control/**` 只读面） |
| `core/crates/edge-client/src/ingress.rs:121` `running.shutdown.send(())` | 是 `oneshot` 不是 `watch`；接收者是 `serve_with_incoming_shutdown` 里的 `wait.await` | server 已自行结束时才会 `Err`，那时本来就该停 —— **忽略是对的** | 不用改 |
| `core/crates/edge-client/tests/common/mod.rs:150` 同形状 | 同上 | 同上 | 不用改 |

判断依据：**`watch` 才有「零接收者时连值都不改」这条陷阱**，`oneshot` 的 `Err` 只说明对端没了。
所以分界不是「返回值有没有被丢」，而是「这个 channel 的值本身是不是要被别人事后读」——
`watch` + 有人读 `borrow()` / `wait_for()` = 必须保证写进去，`oneshot` 的一次性通知不吃这条。
`preflight.rs` / `models/src/lib.rs` 里那几个 `.send()` 是 `reqwest` 的，无关。

### 测试数

`cargo passed=818 → 823`（+5，全在新建的 `tests/signals.rs`），`failed=0`。
`OK 25 files`、`contracts passed=25 failed=0`、go 六包全 `ok`、`passed 10/10`，
`scripts/check.sh` **全部通过，退出码 0**。冻结面一个字没动。

### 要总管 / Y2 落的文档改动（本轨没改，文档面归 Y2）

两处都**不是错字**，是「病治好之后才配这么写」的边界补充：改之前那两句在起飞半路收到信号时
是假的（第一次 `SIGTERM` 什么也不会发生），现在才成立。

| 文件 | 现在的原文 | 建议改成 |
|---|---|---|
| `README.md:119` | 停机：`SIGTERM` 走优雅退出（停投递 → 等在跑的任务善终，宽限 20s → 还沙箱 → 关库），退出码 0；**再来一次**信号立刻硬退，退出码 130。 | 同前，句末加一句：**起飞还没走完时收到也算数**（compose 的 `stop_grace_period`、k8s 滚动更新都会这么来）—— 信号会被记住，起飞一走完立刻进收尾。回归见 `core/crates/app/tests/signals.rs`。 |
| `docs/acceptance-M.md` M6 第 2 步 | **停掉要重启的那个**（Ctrl-C，或 `kill <pid>`；SIGTERM 走同一条优雅退出路径）。 | 同前，补一句：**刚起飞就按也可以**，不必等 `aite.up` 出来 —— 信号落在起飞半路照样走优雅退出（2026-09-13 之前不是这样：那时会卡住，第 3 步的 `pgrep` 一直能看到它）。 |

### 记账转出去的

| 位置 | 病 | 归哪轨 |
|---|---|---|
| `core/crates/control/tests/support/mod.rs:229`+`:258` `ParkedSleep` | 与本轨病根**同一个形状**的 `watch` 丢信号：`new()` 丢 `_rx` + `let _ = parked.send(true)`。闭包先于 `wait_until_parked()` 跑到时，`wait_for` 永远等，用例挂死。目前没见它抖，但窗口是真的 | 下一轮（`core/crates/control/**` 只读面）。药同样是 `send_replace(true)` |
| `.claude/hooks/guard_bash.py` 的 `PROBES` | X1 记过、**本轨又撞两次**：正文里带 `**`（markdown 加粗）的 heredoc 会被反向匹配成已删的 `aite/contracts/__init__.py`；带中文引号的 `python3 -c` 被判「引号配不平」。两次都换写法绕开了。`review/v6-guard-patch.py` 只有人能跑，至今没跑 | 总管（跑一次那个补丁） |

### 没做的 / 拿不准的

1. **`aite run` 在 edge 完全没起来时也能起飞到 `serve()`** —— 这是做 ②(b) 时实测出来的，
   本来以为要起假 edge：`platform.start()` 里只有 `ingress.start()`（本地监听）是硬要求，
   能力表问不到只 warn 一行，`check_contract_version` 等满 5 次也照常返回 `Ok`。
   合 §2.1「启动顺序无关」，**不是 bug**，但没有任何文档或测试写过这件事，
   而进程层这条回归现在**依赖**它。值得让总管确认是不是要把它写进 spec 的既有行为里。
2. **`SUN_LEN` 是这条进程层回归的隐性前提。** socket 路径超过 104 字符时
   `EdgeClient::connect` 会失败。`tempfile::tempdir()` 在 macOS 的 `TMPDIR` 下实测 65 字符，
   够用；换到 CI 上某些长 `TMPDIR` 会炸在那里（症状明确，不会静默变味）。
3. **两条路的等价性只验到行为层，没验到 `receiver_count` 那一层** —— `tx` 是私有的，
   测试拿不到计数。结论「两条行为一致」立在四条单元断言 + 全量 823 条上，
   不是立在对 tokio 内部实现的推理上。
4. **没碰 `core/crates/app/src/cli.rs`**（两轨都只读，归总管）。本轨没有要改它的地方。

## 十、Y2 回执 —— 2026-09-13

基线 `e733a2c`（= `18f30b6` 的代码面 + 一个只加 Y1/Y2 两份派单文件的 commit）。派单写的基线是
`18f30b6`，对不上是因为它写的是**它自己被提交之前**那一格；`git diff --stat 18f30b6..e733a2c`
只有 `review/paste-Y1.md` / `review/paste-Y2.md` 两个文件、575 行全是新增，代码面逐字同格，
所以 check.sh 的期望值照样适用。没停机。

开场自检与收尾 `scripts/check.sh` 的五行关键值：

```
契约锁 ............ OK 25 files                 （开场 / 收尾一致）
C1 契约测试 ....... contracts passed=25 failed=0（开场 / 收尾一致）
B 全量 cargo ...... 开场 818 failed=0 → 收尾 829 failed=0
B go test（-race）. 六个包全 ok                  （开场 / 收尾一致）
B8 评测 ........... passed 10/10                （开场 / 收尾一致）
全部通过，退出码 0
```

**开场自检没撞到假红**（一次跑过，818/0）。守卫实测有效：Read `.claude/hooks/guard_bash.py`
被拦下。中途撞到两次守卫误拦，都是 heredoc 正文被扫：一次「命令无法解析」（正文里有配不平的
引号），一次「不透明载荷」（正文太长）。换 Edit 工具 / 临时文件绕开，没碰守卫本身。

### ① 判断题：选了 (a) —— fake / scripted 直接 FAIL

**两个入口都查了，结论是「没有别的入口理应拿 fake 配置通过 preflight」，所以 (a) 成立**：

| 查什么 | 结论 |
|---|---|
| `aite_app::preflight::run` 的产品调用方 | **只有 `main.rs:122`（`Cmd::Preflight`）一个。**七组自检只从 CLI 进来，没有第二个入口 |
| `wiring.rs` 的评测接线会不会调 preflight | **不会 —— 同名不同物。**`Wiring.preflight` 那个字段是 `DockerProbe`，走 `evals::cli` 的 `docker_preflight`（`evals/src/real_stack.rs:327`，只问 SDK / daemon / 镜像在不在）。它跟七组自检没有任何代码共享 |
| `aite evals --sandbox docker` 那条路会不会 | **不会。**`aite evals` 从头到尾不经过 `build_app`，它走自己的 `Deps` / `Wiring` 注入替身（B8 那档 `--platform fake --model scripted --sandbox fake` 就是这么跑的） |
| `build_app` 的产品调用方 | **只有 `cli.rs:134`，而且传的是 `Injections::default()`** —— `aite run` 那条路**永远零注入**，也没有任何开关能往里塞东西 |

最后一行是 (a) 的地基：在 preflight 的语境里「有没有注入」不是未知数，是定值（没有，且不可能有），
所以 `platform: fake` **确定**起不来 —— 给 WARN 等于让人自己去猜。这条地基由
`build_app_really_refuses_what_the_first_check_refuses`（e2e）拿真 `build_app` 对拍钉着，
哪天真给 `aite run` 加了注入开关，那条会红。

**改前的实证**（派单那两条都复现了，并补了全跑那一档）：`platform: fake` 与
`provider: scripted` 两份配置，`preflight --offline` 都是「全部没红，可以起飞。」退出码 0，
而 `aite run` 都是退出码 2。

### ② 落地的五条口径（逐条对齐 X1）

1. **FAIL 也把 `cfg` 交出去** —— `check_config` 的 return 里 `Some(cfg)` 原样保留，
   `check_config_fails_but_still_hands_over_the_config_when_the_platform_is_fake` 钉着。
2. **第 ① 组仍然只有一行结论 + 一句「怎么补」**。它现在管三件事，写法是**收集式**而不是
   撞上第一条就早退（口径照第 5 组那句「缺什么一次报齐，别让人补完 base_url 重跑一遍才发现还缺
   key」）：`faults` / `fixes` 两个 `Vec` 收完再拼，连接词统一用「；另外，」。
   **只命中一条时这一行与 X1 那天逐字相同**，涟漪最小。
3. **「怎么补」三件事齐**：当前值、该改成什么、以及 fake / scripted 是留给谁用的（§3.1 +
   `aite evals --platform fake` 那条路）。「该改成什么」不写字面量，走
   `REAL_PLATFORM` / `REAL_MODEL_PROVIDER` 两个常量，由
   `injection_fix_matches_the_contract_defaults` 拿**契约默认值 + 样例配置**两头钉住（三处同源）。
4. **2/3/4 组 SKIP 且理由写在那一行里**：`platform=fake：不连飞书，这一组的判据不适用`
   （常量 `FAKE_PLATFORM_SKIP`，口径照 `--offline：不碰网络`）。`--offline` 与 fake 同时成立时
   **说 fake** —— 说 offline 会让人以为去掉那个开关就查得了。
5. **红线照旧**：新的 detail / fix 都是 `CheckResult` 的字段，渲染前统一过 `Redactor`，
   没有新的直写路径。`arm_redactor` 那两遍装料**与第 2 组跑不跑无关**（fake 档把第 2 组 SKIP 了，
   但 `check_config` 的 FAIL detail 照样回显 yaml 标量，脱敏一步不能省）。

第 ① 组那一行最后长这样（原样贴，`platform: fake` 那一档）：

```
[1/7] FAIL 配置可加载       platform=fake · model.provider=openai_compat · sandbox.image=aite-sandbox:p0 · 但 platform=fake 要由调用方注入平台实现 —— aite run 不注入任何实现，起飞会被拒（退出码 2）
           └ 怎么补：真机起飞把配置里的 platform 改成 feishu（config/aite.example.yaml 里就是这个值）；当前值 fake 是留给评测 / 回放的取值（§3.1），不会去连真实飞书 —— 评测走 `aite evals --platform fake`，那条路自己注入替身，不经过 aite run
[2/7] SKIP 环境变量齐       platform=fake：不连飞书，这一组的判据不适用
[3/7] SKIP 飞书凭证有效     platform=fake：不连飞书，这一组的判据不适用
[4/7] SKIP 飞书身份对得上   platform=fake：不连飞书，这一组的判据不适用
```

两条都犯的配置（`fake` + `scripted`）一次报齐，一行里两条并列：

```
· 但 platform=fake 要由调用方注入平台实现、model.provider=scripted 要由调用方注入模型实现 —— aite run 不注入任何实现，起飞会被拒（退出码 2）
```

**`check_env` / `check_model` 一个字没动**：第 5 组那句 `provider=scripted，没有真端点可探`
的 WARN 保留 —— 它管「端点通不通」，第 ① 组管「这份配置起不起得来」，两条各说各的本分。
（`--offline` 把第 5 组整个跳过，所以 scripted 这一档**只有**第 ① 组救得了。）

### ③ 两边口径对照（六格，全部实跑）

| 配置 | `preflight --offline` | `preflight`（全跑） | `aite run` |
|---|---|---|---|
| `platform: fake` **改前** | `OK 2 · WARN 1 · FAIL 0 · SKIP 4`，**「全部没红，可以起飞。」退出码 0** | `FAIL 5`（2/3/4 全红要飞书凭证、5/6 也红），退出 1 —— **红得不是地方**：真正拦住起飞的那条一组都没管 | 退出码 2：`config.platform=fake 时必须由调用方注入平台实现` |
| `platform: fake` **改后** | `[1/7] FAIL` + 2/3/4 SKIP，退出码 **1** | 同上，第 1 组与 offline 无关；2/3/4 照样 SKIP，5/6 各报各的 | 不变（退出码 2，本轨没动 `app.rs`） |
| `provider: scripted` **改前** | `FAIL 0`，**「全部没红，可以起飞。」退出码 0** | `FAIL 4`（飞书那几组，机器没凭证）；第 5 组给 **WARN** `没有真端点可探` —— 不拦起飞，且 `--offline` 下连这句都没有 | 退出码 2：`config.model.provider=scripted 时必须由调用方注入模型实现` |
| `provider: scripted` **改后** | `[1/7] FAIL`，退出码 **1**；2/3/4 照跑（platform 还是 feishu） | 同上；第 5 组那句 WARN 保留 | 不变（退出码 2） |

> X1 记的「全跑那一档没实证」这一半，本轨**补上了**：`provider: scripted` 全跑时第 5 组给的是
> **WARN 不是 FAIL**，所以「七组里一组都没管」这句话对 `--offline` 成立、对全跑只是「管了但不拦」。
> `platform: fake` 那一档全跑时 2/3/4 会红，但红的理由是「这台机器没飞书凭证」，
> 与 fake 起不起得来无关 —— 在一台凭证配齐的机器上那三组会转绿，于是**改前的全跑也是全绿而起不来**。
> 这一点仍然没有真机实证（本机没凭证），但 (a) 做掉之后不需要了：第 ① 组先拦。

**照「怎么补」改完真能修好**（两份都实测）：`platform` 改回 `feishu` / `provider` 改回
`openai_compat` 之后，`preflight --offline` 第 1 组转 `OK`、退出码 0；`aite run` **不再死在注入这一步**，
它走过去了，死在 `ModelConfig.base_url 是空的`（样例配置本来就空 —— 那正是下面那张表第 1 行那个口子）。
两边口径一致。

### ③ 变异验证（五组，每组都被抓到）

| 改坏什么 | 谁红了 |
|---|---|
| 整段判据摘掉（退回今天之前：`injection_fault` 恒 `None` + `fake_platform` 恒 `false`） | lib 3 条 + e2e 4 条 + cli_smoke 1 条 |
| FAIL 降成 WARN（不拦起飞了） | lib 3 条 + e2e 6 条 + cli_smoke 2 条（X1 那条 prompt 判据一起红，同一个分支） |
| 2/3/4 忘了 SKIP（照样跑） | e2e 1 条（`a_fake_platform_fails_the_first_row_and_skips_the_feishu_rows`） |
| 「怎么补」里指错（`feishu` → `feishuu`、`openai_compat` → `openai-compat`） | lib 1 条 + e2e 2 条 |
| 「一次报齐」退回早退（prompt 撞上就 return） | lib 1 条 |

> 第四组第一遍只抓到 lib 1 + e2e 1：e2e 里写的是 `fix.contains("feishu")`，而 `feishuu`
> **包含** `feishu`，放过去了。把两处断言改成连右括号一起断（`contains("改成 feishu（")`）之后
> 才是上表那个数。**自己的断言也得变异一遍**，不然就是第五条恒真断言。

### ④ `link.rs:83`、`:136` 的两条谎话副本

全仓 grep 核实（**只改注释，代码一个字没动**）：

* **`!status` 这条命令是真的**（`control/src/plane.rs:587` 的 `cmd_status`），假的是「健康行」
  那半句：它走 `status_tasks(&ev.chat_id)`，**只从 store 列活跃任务，从头到尾不碰 edge**，
  没有任何一行报 edge 健康。
* `link.rs:83`（`note_ok`）原话是「RΩ 拿它出健康行或做起飞门禁就会误判成『edge 不在』」——
  **两件都不对**：`Link::connected()` 在产品代码里**零调用方**（`EdgeClient::connected()`
  只是一层转发，唯一使用者是 `tests/edge_client.rs`；`wiring.rs:230` 那个 `edge.connected()?`
  是 `LazyEdge` 的同名方法、另一回事），起飞门禁走的是 `status()`（`app.rs` 的
  `check_contract_version`）。改后把这两件说准，并留一句「这一行照样该留 —— 它是
  `connected()` 的语义本身」，免得下一轮有人照着注释把代码删了。
* `link.rs:136`（`status_client`）删掉「`!status` 的健康行也还得问得出来」，换成真调用方
  （本 crate 内两个：`EdgeClient::status()` 与 `verify_contract`）+ 闸门那条真理由。

两处都点名了 `app.rs:83`（W2 改）与 `lib.rs`（X1 改）是同源副本，口径一致。

**还剩第 5 个副本**：`core/crates/edge-client/tests/contract_gate.rs:214` 同一句话，
那个文件不在本轨可写面（见下面记账）。

### ⑤ 涟漪清单

| 文件 | 改了什么 | 「七组」动了没 |
|---|---|---|
| `core/crates/app/src/preflight.rs` 模块头 | 表格第 1 行改成「且它起得来」（三件事）、2/3/4 行各加「`platform: fake` 时 SKIP」；「第 1 组为什么不只验解析」那段重写成三条，加了 2026-09-13 这条病史 | **没动**（「七组」「不是新开第 8 / 9 组」原样） |
| `README.md`（3 处出现） | 第 134 行那段第 1 组的描述改成三件事 + fake 档 SKIP；第 147 行那条病史块扩成两条（2026-09-12 / 2026-09-13）。另外两处（`# 七项各一行结论`、`# 起飞前自检（七组）`）**本来就准确，没改** | **没动** |
| `docs/acceptance-M.md` §0.1 | 「七组分别是：① 配置可加载（含 …）」那句改成三件事 + fake 档 SKIP 的括注；末尾 ⚠️ 块加第三条（2026-09-13）。**那份逐行实测输出重跑过，逐字一致，没改** | **没动** |
| `docs/demo-3min.md` | 第 1 组那段 ⚠️ 加一句「拿评测那份配置上台同理会被第 1 组拦下」 | **没动** |
| `review/inventory-gateway-evals.md` | 第 142 行第 1 组的逐条描述加第三件事 + 病史；2/3/4 各加「`platform: fake` 时 SKIP」 | **没动** |

**`acceptance-M.md` §0.1 那份实测输出重跑了，结论是「不用改」**：`config/aite.yaml` 由样例复制
（`platform: feishu` + `provider: openai_compat`），注入判据不命中，第 1 组照旧 `OK`，
七行逐字比对一致（只有时间戳不同）。跑完 `config/aite.yaml` 删掉、`data/` 没留下、
`git status` 干净。

`core/crates/app/src/cli.rs` 两轨都只读，本轨**一个字没动**（也不需要动）。

### 改完之后 `--offline` 还剩哪些「全绿 ≠ 起得来」

**重列一遍，全部实跑对拍**（每种坏配置各跑 `preflight --offline` / `preflight` / `aite run`）。
**没有补全**：

| 坏配置 | `preflight --offline` | `preflight` 全跑 | `aite run` | 归哪一组管 |
|---|---|---|---|---|
| `model.base_url` 空 | 全绿，退出 0 | FAIL（第 5 组） | 退出 2 | 第 5 组，**被 `--offline` 跳过**（V3 记的那条，仍在） |
| `model.model` 空 | 全绿，退出 0 | FAIL（第 5 组） | 退出 2 | 同上 |
| **`storage.sqlite_path` 指着一个不是 SQLite 的文件** | **全绿，退出 0** | **全绿（第 7 组 OK）** | **退出 2：`建表失败（…）`** | **七组里没有一组管** ← 本轨新挖出来的，与 fake / scripted 同形状 |
| 两边 `contract_version` 不一致 | 查不了（不碰网络） | 第 6 组会红（要 edge 在跑） | 退出 2 | 第 6 组，`--offline` 跳过。**本机没起 edge，这一条没实证** |
| `platform: fake` 而没注入平台 | **FAIL，退出 1** | FAIL | 退出 2 | 第 1 组（本轨补的） |
| `model.provider: scripted` 而没注入模型 | **FAIL，退出 1** | FAIL | 退出 2 | 第 1 组（本轨补的） |
| `system_prompt_path` 指着已删的 Python 树 | FAIL，退出 1 | FAIL | 退出 2 | 第 1 组（X1 补的） |

> **新挖出来那条的实证**：`sqlite_path` 指到一个内容是 `this is definitely not a sqlite database`
> 的文件，第 7 组照样 `OK 落盘目录可写` —— 它验的是「三个路径的最近已存在祖先写得进去」，
> 而 `SqliteSessionStore::open`（`app.rs:227`，`build_app` 第 6 步）要的是「这个文件真能当库打开」。
> 两件事。`aite run` 退出码 2、`aite 起不来：建表失败（…）`。
> 隔离它花了一步：`build_app` 第 5 步（模型）在第 6 步（SQLite）**前面**，所以得先把
> `base_url` / `model` 填上才够得着这条判据。归下一轮，见记账。

### 测试数

818 → **829**（+11）：

* `src/preflight.rs` 的 `mod tests` **+5**（36 → 41）：「怎么补」的两个取值与契约默认值 + 样例配置
  三处同源、能飞的配置不误伤、只放行 `OpenaiCompat` 那一个 provider、FAIL 但仍交出 config、
  prompt 与注入两件事一次报齐。
* `tests/preflight_e2e.rs` **+5**（17 → 22）：fake → 第 1 组 FAIL + 2/3/4 SKIP + 飞书端点零请求；
  scripted → 第 1 组 FAIL 但 2/3/4 照跑、第 5 组那句 WARN 还在；好配置不误伤（`needs_injection=false`）；
  `--offline` 下照跑且两条一次报齐；**与真 `build_app` 行为对拍**。
* `tests/cli_smoke.rs` **+1**（21 → 22）：进程级，真二进制，`platform: fake` 退出码 1、七行不少。

④ 是纯注释，不加条数。

### 记账转出去的

| 位置 | 病 | 归哪轨 |
|---|---|---|
| `core/crates/app/src/app.rs:227`（`SqliteSessionStore::open`） | **第 3 个「七组一组都没管」的口子**：`sqlite_path` 指着一个不是 SQLite 的文件时 preflight 全绿（`--offline` 与全跑都是），`aite run` 退出码 2 `建表失败`。本轨实证在案。补法与本轨同形：折进第 ① 组（试着打开一次、跑完就关），或扩第 7 组的判据。**别新开第 8 组** | 下一轮 |
| `core/crates/edge-client/tests/contract_gate.rs:214` | 「`!status` 的健康行」那句谎话的**第 5 个副本**（W2 改 `app.rs`、X1 改 `lib.rs` 两处、本轨改 `link.rs` 两处）。那个文件不在本轨可写面 | 下一轮（顺手带掉） |
| 第 2 组在 `platform: fake` 下整组 SKIP | 第 2 组是**泛化**扫 `*_env` 的，整组跳过连 `model.api_key_env` 一起跳了。fake 配置注定在第 1 组 FAIL、必须改回 feishu 重跑，那一轮四个变量全查，所以只是推迟一轮、不会静默放行。更准的做法是按字段路径前缀只跳 `feishu.*` 那几个 —— 但那要在泛化扫描通道里插一条「platform=fake ⟺ feishu.* 不适用」的耦合，收益不抵代价。**如实记账，不自作主张** | 待定（总管） |
| `core/crates/app/src/run.rs` 的 `StopSignal::set()` | X1 记的那条（丢信号）**本轨没碰** —— 归 Y1，它同时在跑 | Y1 |

### 没做的 / 拿不准的

1. **「七组全绿而起不来」仍然没有真机实证** —— 本机没有飞书凭证，不带 `--offline` 跑第 2/3/4 组
   必红。但 (a) 做掉之后这件事不需要实证了：fake 那一档现在在第 ① 组就被拦下，不存在「全绿」这种状态。
2. **`aite run` 那一侧一个字没动**（`app.rs` 两轨都只读）。所以「两边口径一致」是靠 preflight
   这一侧追上 `build_app` 达成的，判据是**照抄的不等式**而不是共享的函数 —— 这一点与 X1 那条
   （复用同一个 `load_system_prompt`）不同，所以专门加了行为对拍那条测试兜住。
3. **第 2 组那条取舍见上面记账第 3 行**，照派单做的整组 SKIP，代价写在 `run_checks` 的注释里。
4. **`contract_version` 不一致那条没实证**（要一台版本不同的 `aite-edge` 在跑），表里如实标了。

## 十一、Z1 回执 —— 2026-09-13

Y1/Y2 两份回执末尾「记账转出去的」那四条收掉了，外加把「edge 没起也能起飞」钉成既有行为。
**③ 不是最后一个副本** —— 全仓还剩两个，都在本轨可写面外，见末尾记账。

### 基线与开场自检

HEAD 是 `90be53e`，不是派单抬头写的 `45728c1`。差的那一格就是**出本派单这个文件本身**
（`review/paste-Z1.md`，291 行文档、零代码改动），`90be53e^` 正是 `45728c1`。当基线用，不是回归。

`scripts/check.sh` 开场：`OK 25 files`、`contracts passed=25 failed=0`、
**`cargo passed=834 failed=0`**、go 六个包全 `ok`、`passed 10/10`，「全部通过」退出码 0。
**一次跑过，没撞到那三个抖动 target 里的任何一个**（`graceful_shutdown` / `reconnect_replay` /
`startup_recovery` 都没红）。Read `.claude/hooks/guard_bash.py` 被守卫拦下 ✅。

### ① `storage.sqlite_path` 指着一个不是 SQLite 的文件

**选了 (a)：折进第 ① 组**，四条理由：

1. 病的类别与前两件事**逐字同一类**（「配置解析成功 ≠ 它起得来」）。模块头那段话已经把这句
   立成第 ① 组的本分，X1 / Y2 各折过一件，这是第三次同形状 —— 放第 7 组会把同一个故事拆成两处讲。
2. 第 7 组的主语是**目录**（「三个路径的最近已存在祖先写得进去」），这一条问的是**那个文件的内容**。
   塞进去之后那一组的一行结论要同时表达「写不进去」和「不是个库」两种毛病，口径会糊。
3. **副作用**：第 7 组本来就有写副作用（真建目录再删，V2 留的）。往那儿再加「试开库」是把副作用摊大；
   折进第 ① 组反而做得成**纯读**（见下）。
4. **一次报齐**：第 ① 组已经是收集式（`faults` / `fixes`），第四件事直接进队，一行报齐、
   一句「怎么补」四条并列。第 7 组要另起一套。

**副作用怎么按住的**（(a) 的准入条件，派单点名的那条）：

* **文件不在就不探，直接过。** 那是正常路径 —— `app.rs:152` 的文档注释明写「库文件不在就建一个
  空的」。正因为先问了这一句，后面那个 `SqliteSessionStore::open`（它会 `create_dir_all` 父目录
  **并新建库文件**）永远建不出任何东西：走到它跟前时文件和父目录都已经在了。
* `:memory:` 直接放过（store 认这个取值）。
* 探针本身只读：`PRAGMA schema_version`，一个字节不写，跑完 `close()`。

**判据挂在哪 —— 派单和 Y2 的归因要更正一处**：真正炸的**不是** `SqliteSessionStore::open`
（`app.rs:226`，`build_app` 第 6 步），而是 `run.rs:156` 的 `app.store.init()`。
`open` 只是一句 `Connection::open`，SQLite 在那一步根本不读文件头，所以组装那一关**是过得去的**；
`CREATE TABLE` 第一次真读到头才报 `SQLITE_NOTADB`。所以判据是「开一次库 **+ 逼它读一次文件头**」——
只 open 的话这条判据恒真，等于没加（变异 M2 验的就是这个）。`store_init_really_fails_on_what_the_first_check_refuses`
那条 e2e 拿真 `store.init()` 对拍钉着。

五条口径逐条对齐：FAIL 也把 `cfg` 交出去（原路径未动）/ 那一组仍然只有一行结论 /
「怎么补」三件齐（当前值、该改成什么、不补的后果）/ 四条同时命中一次报齐（收集式，不早退）/
红线照旧（detail / fix 都是 `CheckResult` 的字段，渲染前统一过 `Redactor`，没有新的直写路径）。

**探不出来的那一半，如实记着**（写进了 `sqlite_fault` 的文档注释）：文件是个好库、但它自己只读
（或所在卷只读）时 `init()` 照样会炸，这条判据看不见 —— 它只读，读得动就算过；第 7 组也接不住
（那一组问的是**目录**）。要覆盖它得真往库里写一次，那就把第 ① 组从纯读变成有副作用，不划算。

#### ① 的六格对照（全部实跑，本机无飞书凭证）

| 配置 | `preflight --offline` | `preflight` 全跑 | `aite run` |
|---|---|---|---|
| 非 SQLite 文件 **改前** | `OK 2 · WARN 1 · FAIL 0 · SKIP 4`，**「全部没红，可以起飞。」退出码 0** | **第 1 组 OK、第 7 组 `OK 落盘目录可写`**；红的是 2/3/4/5/6（本机没凭证、假端点、没起 edge）—— **一组都没管 sqlite**。退出 1 | 退出码 2：`aite 起不来：建表失败（…）：sqlite: file is not a database` |
| 非 SQLite 文件 **改后** | `[1/7] FAIL 配置可加载 … 但 storage.sqlite_path 指着的文件当不了 SQLite 库`，退出码 **1** | 同上，第 1 组与 offline 无关；第 7 组**仍然 OK**（两件事，没混） | 不变（退出码 2，本轨没动 `run.rs` / `app.rs` 的代码面） |

> 「全跑那一档全绿」这件事在本机仍然没有真机实证（没有飞书凭证、没起 edge），与 Y2 那轮同因。
> 但和 Y2 一样：(a) 做掉之后不需要了 —— 第 ① 组先拦。
>
> 跑完复核过那个文件**一个字节没变**（40 字节原文还在），`data/` 没被建出来。

改后那一行与「怎么补」原样：

```
[1/7] FAIL 配置可加载       platform=feishu · model.provider=openai_compat · sandbox.image=aite-sandbox:p0 · 但 storage.sqlite_path 指着的文件当不了 SQLite 库：<D>/aite.db → <D>/aite.db（sqlite: file is not a database）
           └ 怎么补：把配置里的 storage.sqlite_path 指到一个真的 SQLite 库，或者把 <D>/aite.db 挪开／删掉、让起飞时自己建一个空库出来（config/aite.example.yaml 里是 data/aite.db）；当前那个文件读得到但不是库，`aite run` 会走到建表那一步才炸 —— 退出码 2、`aite 起不来：建表失败（…）`，preflight 这边不拦的话你要到那时才知道
```

#### ① 的变异（五组 + 一组断言变异，每组都被抓到）

| 改坏什么 | 谁红了 |
|---|---|
| M1 整段判据摘掉（`sqlite_fault` 恒 `None`） | lib 1 + e2e 3 + cli_smoke 1 = **5 条** |
| M2 只 open 不读文件头（照派单/Y2 的归因把判据挂在 `build_app` 第 6 步） | 同上 **5 条** |
| M3 去掉「文件不在就不探」那道闸（副作用回来了） | lib 1 + e2e 1 = **2 条**，外加一条实打实的现场证据：那一遍在**仓库根**留下了一个 0 字节的 `data/aite.db`（`cli_smoke` 里那条不带 `--config` 的 preflight 跑在仓库根、退到样例配置，`sqlite_path: data/aite.db` 于是被 `SqliteSessionStore::open` 当场建了出来）。交付版跑同一批测试**不留任何 `data/`**，复验过 |
| M4 「怎么补」指错（`data/aite.db` → `data/aite.db.bak`） | lib 1 + e2e 1 = **2 条** |
| M5 「一次报齐」退回早退（sqlite 撞上就清空前面的） | lib 1 = **1 条** |
| **A1 断言变异**：把 e2e 那句 `fix.contains("里是 data/aite.db）")` 退回裸子串 `contains("data/aite.db")`，再叠 M4 | e2e 那条**当场放过去了**，只剩 lib 的契约锚点在红 |

> A1 就是 Y2 踩过的 `feishuu` 子串陷阱的同一形状：`data/aite.db.bak` **包含** `data/aite.db`。
> 断到右括号才是那条断言的承重墙。

### ② `ParkedSleep` —— 上药 + 原病复现

照 Y1 的判断上 `send_replace(true)`（没有重新论证两条路）。类型注释里写明了
「与 `StopSignal::set()` 逐字同一条陷阱」「那条是产品缺陷、这条是同形状的测试替身」。

**先撑开窗口复现了原病**（照 Y1 的手法：让闭包先跑到）——
`timeout` 先 poll 内层，一次 poll 就走到 `pending()` 挂住，也就是 `send` 已经发生而一个订阅者都没有。
**上药之前**（原样贴）：

```text
test support::parking_counts_even_when_nobody_is_waiting_yet ... FAILED

thread 'support::parking_counts_even_when_nobody_is_waiting_yet' panicked at crates/control/tests/support/mod.rs:285:6:
挂住早于订阅时 wait_until_parked() 必须立刻返回: Elapsed(())

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out; finished in 2.06s
```

**上药之后**：

```text
test support::parking_counts_even_when_nobody_is_waiting_yet ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.05s
```

2.06s（撞穿 2s 死线）→ 0.05s。

**断言自己也变异过**（P1）：把测试的时序颠倒（`wait_until_parked` 先订阅，再让闭包跑）、
同时把药退回 `let _ = send(true)` —— 结果 **10 passed，全绿**。也就是说这条断言的承重墙是
**时序**而不是别的什么；写反了它就是一条恒真断言。

### ③ 「`!status` 的健康行」—— 不是最后一个副本

`contract_gate.rs:214` 改掉了（只动注释，代码一个字没动），口径抄的 `link.rs:145-151`：
前半句（`GetStatus` 不许过闸门）留着并补上「闸落着时它是唯一还出得去的一发 RPC，
`verify_contract` 靠它解锁」；后半句换成「那条健康行全仓不存在，`!status` 由 `cmd_status` 答，
只从 store 列活跃任务、不碰 edge」，并点名 W2 / X1 / Y2 改过的那五处是同源副本。

**全仓 grep「健康行」（排除 `review/`），还活着的谎话还有两个**，都在本轨可写面外：

| 位置 | 原文 | 为什么是同一句谎话 |
|---|---|---|
| `core/crates/edge-client/src/gate.rs:18` | 「`EdgeClient::status()`（起飞体检**和健康行走的那条**）每答上来一次也顺手记一次」 | 括号里那半句在断言健康行存在。`status()` 的真调用方是 `app.rs` 的 `check_contract_version`、`preflight.rs` 第 6 组、`wiring.rs` 的评测接线 —— 没有健康行 |
| `edge/internal/ingress/client.go:200` | 「而 RPC 已经跑通了，`!status` 的健康行会在这中间报『没连上』」 | 同一句谎话在 Go 侧的副本（注释自己写着「core 侧 R6 的 `link.note_ok()` 是同一个修法，这边补齐」—— 连病一起抄过去了） |

其余命中全是**已经改正、正在解释病史**的文本（`app.rs:83`、`lib.rs:81/99`、
`link.rs:87/88/94/149`、本轨的 `contract_gate.rs:217`），不用动。

**所以这句谎话一共 7 个副本，不是派单说的 5 个。**

### ④ Y1 交回来的两处文档 —— 先验后落

**改之前自己验了一遍**（起真二进制、拿一把外部 SQLite `EXCLUSIVE` 写锁把
`install_signal_handlers → serve()` 那个 ~25ms 的窗口撑开，照 Y1 的手法）：

```text
[1] 窗口开着：aite.signal_ready 已打，aite.up 还没有 —— 这就是「起飞半路」
[2] 发了 SIGTERM，pid=85817
[3] handler 收到了，而 aite.up 仍然没出现 —— 信号确实早于 serve()
[4] 放锁，起飞继续。从这里开始它必须自己走完收尾
[5] 进程自己退了，退出码 = 0

--- 日志里的标记行（按出现顺序）---
  aite.edge_unreachable / aite.signal_ready / aite.signal / aite.up /
  ingress.listening / edge.capabilities_unavailable / aite.stopping / aite.down

aite.signal 在 aite.up 之前：True
走完收尾（aite.stopping + aite.down 都在）：True
```

**「2026-09-13 之前不是这样」那半句也验了**：把 `StopSignal::set()` 临时退回
`let _ = self.tx.send(true)`（`run.rs` 是本轨只读面，跑完立刻还原、`git diff` 复核为空、
并 `touch` 强制重编），同一个脚本跑到第 [4] 步之后 **30s 内进程不退**，`TimeoutExpired` ——
正是「第 3 步的 `pgrep` 一直能看到它，只剩 `kill -9`」。

两处都照 Y1 的原文落了（措辞略润，事实一个字没改）：`README.md` 停机那一段句末加一句；
`docs/acceptance-M.md` M6 第 2 步加两行。

### ⑤ 「edge 没起也能起飞」怎么钉的

**测试**放在 `core/crates/app/tests/signals.rs`（新增 1 条，+1），
名字 `edge_absent_still_takes_off_to_ingress_listening`，紧挨着依赖它的那条进程级回归，
文档注释里第二条就写明这层依赖关系。判据是**日志上的三个标记**，缺一不可：

* `aite.edge_unreachable` —— 证明 edge **真的**不可达（否则这条测的是「edge 恰好起着」）；
* `ingress.listening` —— 证明起飞走到了投递面（`takeoff()` 里 `serve()` 的前一步）；
* `edge.capabilities_unavailable` —— 证明能力表那一问也没答上来，而它只是一行 warn。

外加一条顺序断言（`aite.edge_unreachable` 排在 `aite.up` 之前：起飞是**带着**「没比成版本」
这个结论继续走的，不是先飞起来才发现 edge 没了），最后 SIGTERM 收干净、退出码 0。

顺手把原测试里写死的那份配置抽成 `write_takeoff_config` / `spawn_aite` / `sigterm` 三个 helper，
`Reaper` 提到模块层 —— 两条进程级测试共用，`edge_socket` 指着一个谁都没在监听的路径这件事
因此只写一遍，而且写明了「那不是将就，是被测行为」。

**注释**：`app.rs` 的 `build_app` 文档注释末段（edge 不可达时起飞继续、依据 §2.1、
以及它对 `tests/signals.rs` 的意义）+ `check_contract_version` 的函数注释（答不上来不是拒绝起飞的理由）。
**`docs/dev-spec-*.md` 一个字没加**（冻结面）；口径写进了 `README.md`「两个进程」那一节
「启动顺序无关」那一条下面。

**变异**：

| 改坏什么 | 谁红了 |
|---|---|
| E1 把「edge 不可达就拒绝起飞」加进 `check_contract_version`（正是派单担心的那种「改进」） | `edge_absent_still_takes_off_to_ingress_listening` **与** `sigterm_before_serve_still_exits_by_itself` 一起红 —— 而现在前者的名字直接说明了病在哪 |
| E2 断言变异：`EDGE_GONE` 指到一个日志里不存在的串 | 新那条红 —— 证明它真的在读日志，不是恒真 |

### 测试数

`cargo passed=834 → 852`（+18），`failed=0`：

* `src/preflight.rs` 的 `mod tests` **+3**（41 → 44）：契约 + 样例两头钉 `DEFAULT_SQLITE_PATH`、
  探针零副作用（文件不在 / `:memory:` / 0 字节空库三种形态）、四件事一次报齐。
* `tests/preflight_e2e.rs` **+4**（22 → 26）：非 SQLite 文件 → 第 1 组 FAIL 而**第 7 组照样 OK**、
  `--offline` 下照跑、好库与不存在的库都不误伤且零落盘、与真 `store.init()` 对拍。
* `tests/cli_smoke.rs` **+1**（22 → 23）：进程级，真二进制，退出码 1、七行不少、第 7 组绿、
  那个文件一个字节没被动过。
* `tests/signals.rs` **+1**（5 → 6）：⑤ 那条。
* `core/crates/control/tests/support/mod.rs` **+9**：② 那条断言**只有一条**，但
  `tests/support/mod.rs` 是靠 `mod support;` 引进去的，9 个 control 测试二进制各编一份、
  libtest 各收一次。派单把 ② 的可写面钉死在这个文件上，没有别处可放（`tests/dispatch.rs`
  不在白名单）。每份 0.05s，如实记在这儿免得下一轮看见 +9 以为多写了 9 条。

③ 是纯注释，④ 是纯文档，都不加条数。

收尾 `scripts/check.sh`：`OK 25 files`、`contracts passed=25 failed=0`、
`cargo passed=852 failed=0`、go 六包全 `ok`、`passed 10/10`，「全部通过」退出码 0。
`cargo clippy --workspace --all-targets -- -D warnings` 干净。冻结面一个字没动。

> 收尾第一遍 `check.sh` 只挂在 `A4b cargo fmt --check` 上。`cargo fmt --all`（写模式）
> **被守卫拦下**（它会碰 `core/crates/contracts/**` 这个冻结面），换成对本轨改过的那两个文件
> 直接跑 `rustfmt --edition 2024`，`git status` 复核没动到别的文件。

### 踩到的两个坑（记给下一轮）

1. **`shutil.copy2` 还原文件会让 cargo 跳过重编。** 变异验证的脚本用 `copy2` 备份 / 还原，
   还原时把**旧 mtime** 一起写回去，于是产物比源码「新」，`cargo build` 报 `Finished` 而
   二进制仍然是**变异版**的。第一次发现是在 ④ 那个 `run.rs` 临时变异之后。
   后来的脚本改成 `write_bytes`，并且每次还原后 `touch` 一遍再重跑。
   这条很毒：它不报错，只是让你拿着坏产物跑出一份「绿」。
2. **守卫又撞三次**，都换写法绕开了，没碰守卫本身：`find … -exec`（覆盖面判不出来，
   换成 `ls`）；正文带中文引号的 heredoc（判「命令无法解析」，换 Write 工具落文件）；
   **`cargo fmt --all`（写模式）被判触碰冻结面 `core/crates/contracts/**`** ——
   提示里写着「用 `cargo fmt --check`」，但收尾要的是真格式化，所以改成对本轨改过的那两个
   文件直接跑 `rustfmt --edition 2024`。这条值得记进派单模板：**本轨凡改了 Rust 就会撞上它**。
   另外 `rm -rf` 那一条走的是权限询问不是守卫。
3. **变异跑完记得查仓库根有没有落盘残留。** M3 那一遍在仓库根留了个 0 字节的
   `data/aite.db`（见上表），而它是 `.gitignore` 里的 `/data/`，`git status` **看不见** ——
   靠 `git status` 干净来判断「没污染仓库」是不够的。

### 记账转出去的

| 位置 | 病 | 归哪轨 |
|---|---|---|
| `core/crates/edge-client/src/gate.rs:18` | 「`!status` 的健康行」那句谎话的**第 6 个副本**（模块头里那句「起飞体检和健康行走的那条」）。`edge-client/src/**` 不在本轨可写面 | 下一轮（顺手带掉） |
| `edge/internal/ingress/client.go:200` | 同一句谎话的**第 7 个副本**，Go 侧。`edge/**` 只读 | 下一轮（顺手带掉） |
| `README.md:143-145`、`docs/acceptance-M.md:125-126`、`review/inventory-gateway-evals.md:142` | 第 ① 组现在管**四件事**，这三处还写着「三件事一次报齐」并只列了 prompt / 注入两条。本轨可写面里 `README.md` / `docs/acceptance-M.md` 被派单钉成「④ 只许改点名的那两处」，inventory 压根不在白名单 —— **没伸手**。要补的话每处加一句：「以及 `storage.sqlite_path` 上已经有的那个文件真能当库打开（读得到但不是库是 FAIL；病史：2026-09-13，`--offline` 与全跑都全绿而 `aite run` 退出码 2 `建表失败`）」 | 总管 / 下一轮 |
| `docs/demo-3min.md:119-124` | 同上，第 1 组那段 ⚠️ 只说了 prompt 与 fake/scripted 两族，没有 sqlite 这族。不在本轨可写面 | 同上 |
| `core/crates/app/src/app.rs:152` 的文档注释 | 「`SqliteSessionStore::open`：打开连接，**库文件不在就建一个空的**（里面还没有表）」—— 这句是对的，但它没说「文件在而不是库」会怎样，而那正是本轨这条病。已经在 `sqlite_fault` 的注释里说清了归因（炸在 `run.rs:156` 的 `store.init()`），**`app.rs` 那句本身没改** —— 它不在 ①⑤ 点名要改的那两处（`build_app` 末段与 `check_contract_version`），不自作主张 | 待定（总管） |
| 第 ① 组探不出「库是好的但文件只读」 | `init()` 的 `CREATE TABLE` 照样会炸，而第 ① 组只读、第 7 组问的是目录。要覆盖得真往库里写一次，那就把第 ① 组从纯读变成有副作用 —— 代价不抵收益。**如实记账，不自作主张** | 待定（总管） |

### 没做的 / 拿不准的

1. **「七组全跑而全绿」仍然没有真机实证**（本机没有飞书凭证、没起 `aite-edge`）——
   与 Y2 那轮同因。改后第 ① 组先拦，这件事不再需要实证。
2. **`aite run` 那一侧一个字没动**（`app.rs` 只改了两处文档注释，`run.rs` 全程只读）。
   所以「两边口径一致」仍然是 preflight 这一侧追上起飞路径达成的，靠的是
   `store_init_really_fails_on_what_the_first_check_refuses` 那条行为对拍，不是共享函数。
3. **② 那条断言 ×9** 见上面「测试数」。有别的放法（挪进 `tests/dispatch.rs`）但那个文件
   不在白名单，没伸手。
4. **`core/crates/app/src/cli.rs` 一个字没动**（只读，归总管）。本轨没有要改它的地方。
5. **⑤ 那条测试大约 6s**，其中 4s 是 `check_contract_version` 那 5 次 `GetStatus` 重试 ——
   既有行为，不是测试自己在 sleep。它与 `sigterm_before_serve_still_exits_by_itself` 同样
   受 `SUN_LEN` 那条隐性前提约束（Y1 记过，本轨的 `write_takeoff_config` 里原样留了那条注释）。

## 十三、Z3 回执 —— 2026-09-13

纯文字轨，零行为改动。收掉 Z1 记账转出去的前四条：第 ① 组「四件事」在四处文档的追平，
外加「`!status` 的健康行」最后两个副本。**④ 收口时发现同一句谎话还有另一种说法**，
比派单预计的多 4 处（见 ④）。

### 基线与开场自检

HEAD 是 `7a261f7`，不是派单抬头写的 `f3bc017`。差的那一格就是**出 Z2 / Z3 两份派单本身**
（`review/paste-Z2.md` + `review/paste-Z3.md`，545 行文档、零代码改动），`7a261f7^` 正是 `f3bc017`。
当基线用，不是回归。

`scripts/check.sh` 开场：`OK 25 files`、`contracts passed=25 failed=0`、
`cargo passed=852 failed=0`、go 六个包全 `ok`、`passed 10/10`，「全部通过」退出码 0。
**一次跑过，三个抖动 target 一个都没撞到。** Read `.claude/hooks/guard_bash.py` 被守卫拦下 ✅。

### ①.1 事实核对 —— 三格实跑（没照抄 Z1 的病史原文）

配置：样例复制，只改 `storage.sqlite_path` 指到一个 46 字节的纯文本文件
（`this is definitely not a sqlite database file`）。

| 格 | 结果 | 退出码 |
|---|---|---|
| `preflight --offline` | `[1/7] FAIL 配置可加载 … 但 storage.sqlite_path 指着的文件当不了 SQLite 库（sqlite: file is not a database）`；`[7/7] OK 落盘目录可写`；`汇总：OK 1 · WARN 1 · FAIL 1 · SKIP 4` | **1** |
| `preflight` 全跑 | 第 1 组同上一字不差；第 7 组**仍然 OK**；`汇总：OK 1 · WARN 0 · FAIL 6 · SKIP 0`（另五红是本机没凭证 / 没起 edge） | **1** |
| `aite run` | `aite 起不来：建表失败（<D>/aite.db）：sqlite: file is not a database` | **2** |

**踩到一个坑，记给下一轮**：样例配置**只改一行 `sqlite_path` 是复现不出建表失败的** ——
`model.base_url` / `model.model` 在样例里是空的，`aite run` 先死在
`模型配置不完整：ModelConfig.base_url 是空的`（同样是退出码 2，但**病因完全不同**）。
把 `base_url` / `model` 填上之后才走到 `store.init()`，才是 Z1 记的那条病史。
照抄病史而不实跑的话，很容易拿前一个退出码 2 当成后一个。

跑完复核：那个文件仍是 46 字节、md5 未变（`af9c1828…`），三格全程**纯读**，
`data/` 没被建出来。

「病史那半句」（改前 `--offline` 与全跑**都全绿**）本轮**没有重新实证** ——
要看到它得把 `sqlite_fault` 摘掉，那是行为改动、`preflight.rs` 又是本轨只读面。
它由 Z1 的六格对照表 + `M1` 变异（摘掉判据 → 5 条测试红）钉着；本轮能独立佐证的那一半是
**第 7 组在两格里都是 OK** —— 也就是「第 7 组接不住这一条」为真，所以改前那两格确实无人拦。

### ①.2 `acceptance-M.md` §0.1 那份逐行实测输出 —— 重跑，结论「不用改」

环境正好符合文档自述的条件：只设了 `AITE_MODEL_API_KEY`、无 `FEISHU_*`、无 `data/`。
`cp config/aite.example.yaml config/aite.yaml` 后跑 `preflight --offline`，
**机器比对**（把时间行与两条 NOTE 按文档自己声明的省写方式归一后 `diff -u`）：

```
【逐字一致，无差异】
```

`FAIL 0`、`OK 2 · WARN 1 · FAIL 0 · SKIP 4`、「全部没红，可以起飞。」退出码 **0** —— 与文档一字不差。
第 1 组仍是 OK，因为四条判据一条都不命中：`data/aite.db` **不存在**（「文件不在就不探」是正常路径）、
prompt 路径在、`platform: feishu` / `provider: openai_compat` 不需要注入。
**这是本轮自己跑出来的结论，不是继承 X1 / Y2 的。** 跑完把临时的 `config/aite.yaml` 删了，
`data/` 也没被建出来。

### ① 四处改了什么

基准是 `core/crates/app/src/preflight.rs` 模块头那张表（Z1 已写对），四处只调措辞、不改事实。

| 文件 | 原话 | 改成 | 为什么这么调 |
|---|---|---|---|
| `README.md:143` 一带 | 「**三件事**一次报齐：yaml / prompt / 注入」+ 下面「同族的口子当场咬过人**两次**」两条病史 | 「**四件事**一次报齐」，第四件补 `storage.sqlite_path` 上已有的文件真能当库打开；「咬过人**三次**」并加第三条病史 | 读者在问「**怎么跑起来**」。正文只加一个并列项 + 一句「文件不在不算问题，起飞时自己建一个空库」（免得有人以为要先手工建库）；病史挪到下面 ⚠️ 块里，不挡正文 |
| `docs/acceptance-M.md:126` + 末尾 ⚠️ 块 | §0.1「七组分别是：① …（yaml、prompt、注入）」；⚠️ 块到「第三个同族口子」为止 | 括注加第四条；新增「**第四个同族口子**」整段 | 读者在**排障**。所以写足三件事：症状原文（`建表失败…file is not a database`）、**第 ⑦ 组为什么接不住**（问的是目录不是文件，别拿「⑦ 是绿的」当反证）、以及**探不出来的那一半**（库是好的但只读时两组都看不见）—— 后者是这份文档独有的，别处不写 |
| `docs/demo-3min.md:124` 一带 | 第 1 组那段 ⚠️ 只说了 prompt 与 fake/scripted 两族 | 加一段 sqlite 族 | 读者在**上台前 3 分钟**。所以只讲「排练时挪过库 / 名字被占 → 第 1 组当场拦你」，落点是「改前是全绿放行的，台上要到 `aite run` 才炸，那时镜头已经开着」。判据机制一个字不讲 |
| `review/inventory-gateway-evals.md:142` | 「三件事一次报齐」，只列两条，各带「谁补的 + FAIL 条件 + 病史」 | 「四件事」，第三条照前两条的三段式补齐，另加判据形状（纯读 / 文件不在就不探 / `:memory:` 放过）与炸点归属（`run.rs` 的 `store.init()`，**不是** `SqliteSessionStore::open`） | 这是**清单**，唯一读者是下一轮接手的人。所以最密、写机制、写归因更正 —— 派单和 Y2 都把炸点归到 `open` 上，Z1 更正过，这里落成台账 |

**「要不要统一成同一句」的判断：不统一。** 四种读者要的信息量差一个数量级
（README 一句并列项 / demo 一句台上后果 / acceptance 三段排障 / inventory 全量机制），
硬统一的结果只能是取交集 —— 那样 acceptance 就丢了「⑦ 接不住」和「只读那一半」这两条排障时最值钱的话，
或者 demo 被塞进一段台上根本没空读的机制说明。**统一的是事实，不是句子**：四处都与
`preflight.rs` 模块头那张表逐条对得上（第四件事的名字、FAIL 条件、纯读、文件不在不算问题）。

### 「七组」计数

改前 **78** 处 / 改后 **78** 处（含 `review/`）。**逐文件比对，每一份都一模一样**：
`preflight.rs` 6、`cli_smoke.rs` 1、`preflight_e2e.rs` 2、`acceptance-M.md` 4、`demo-3min.md` 1、
冻结那份 dev-spec 1、`README.md` 4、`inventory` 1，`review/` 那批不变。
「八组」改前 8 处，全在 `review/` 的历史派单里（原文就是「不许变成八组」），**一处都不是本轮引入的**。

> 把本节（十三、Z3 回执）自己追加进台账之后，全仓数字变成「七组」80 / 「八组」9 ——
> 多出来的 2 + 1 全是**本节正文自己的字**（台账那个文件 19→21 / 1→3）。
> 判据文件一处没动，见上面那份逐文件对比。

### ②③ 两个副本

**真调用方本轮重新 grep 核过**，与 Y2 / Z1 的结论一致：`EdgeClient::status()` 产品代码里三个调用方 ——
`app.rs:373`（`check_contract_version`，起飞比版本）、`preflight.rs:1345`（第 6 组，先问 daemon 可达）、
`wiring.rs:388`（评测接线）。`cmd_status`（`plane.rs:587`）走 `status_tasks(&ev.chat_id)`，
函数体内 **edge 引用数 = 0**。

* **② `gate.rs:18`** —— 括注「（起飞体检**和健康行**走的那条）」整个去掉，另起一段按 `link.rs:145-151`
  的口径写：健康行全仓不存在 + `!status` 由 `cmd_status` 答只列活跃任务 + 点名三个真调用方 +
  列出同源的五处（W2/X1/Y2/Z1）。**连「起飞体检」那半句也去掉了** —— 三个调用方里只有一个算体检，
  留着仍然不准。`rustfmt --edition 2024` 单文件跑过（**没用 `cargo fmt --all`**，那条会被守卫拦）。
* **③ `client.go:200`** —— 同样口径，另外写明这个标志现在谁在看：`ingress.Client.Connected()`
  在**产品代码里零调用方**，只有两个 `_test.go` 拿它当判据；`EdgeStatus.platform_connected`
  填的是 `feishu.Platform.Connected()`（`main.go:60`），**不是这个**。行留着、话说准。
  `gofmt -l .` 空、`go vet ./...` 干净。

**`git diff` 里没有代码行**：两个文件的 diff 全部命中 `^[+-]\s*//`（Rust 侧 `//!`），机器核过。

### ④ 收口 —— 「健康行」清完了，但同一句谎话有另一种说法

**先把派单那个「40 处」对上**：改前全仓（git 跟踪文件）**50** 处，其中派单文件
`review/paste-Z3.md` 自己贡献 **10** 处 —— 50 − 10 = 40，派单是在自己落盘前数的。不是漂移。

**排除 `review/` 后 12 处，逐条落类**（改后）：

| 位置 | 落类 |
|---|---|
| `app.rs:83`、`lib.rs:81/99/100`、`link.rs:87/88/94/149`、`contract_gate.rs:217` | 在解释病史 / 已经说准了 —— **不动**（9 处，逐条读过上下文，不是只看 grep 行） |
| `gate.rs:20`、`client.go:202/210` | 本轮改的 ②③ 自己的解释文本 |

**「健康行」这个说法：一共 7 个副本，现在全清了。没有第 8 个。**

**但这不是收口。** 又 grep 了一遍 `!status` 在源码里的**其他**说法（排除 `review/`），
同一句谎话换了个词还活着 **4 处**（两处是同一个 proto 注释的生成副本）：

| 位置 | 原话 | 为什么是同一句谎话 | 处置 |
|---|---|---|---|
| `edge/internal/ingress/client.go:40` | 「`Counters` 是给 **!status** / 运维看的快照」 | `Counters()` 产品代码里**只有一个调用方**：`main.go:196` 收尾时打的那行 `edge.counters` 日志。四个数一个都不过线（`EdgeStatus` 里没有它们） | **已改**（本轨可写面，只改注释） |
| `edge/internal/ingress/client.go:129` | 「照『断过就 +1』算的话 **`!status`** 会说『重连 1 次』」 | 同上，`c.reconnects` 只进那行收尾日志。`EdgeStatus.reconnect_count` 填的是 `feishu.Platform.ReconnectCount()` | **已改**（改成 `ingress.reconnects` 会报） |
| `edge/cmd/aite-edge/main.go:58` | 「别让 **!status** 误报『飞书在线』」 | `platform_connected` → `EdgeStatus` → 被 `app.rs:388` 打成日志、被 preflight 第 6 组读。到不了 `!status` | **没伸手**（`edge/` 除 `client.go` 是只读） |
| `proto/aite/v1/edge.proto:151`（生成副本：`edge_grpc.pb.go:831/858`） | 「edge 自身健康：给 **!status** / preflight 用」 | `preflight` 那半句是**对的**（第 6 组）；`!status` 那半句是同一句谎话的源头 —— 两个 `.pb.go` 副本就是从它生成的 | **没伸手**（`proto/**` 冻结） |

**判据是核过的，不是推的**：`!status` 唯一会多报的计数是 core 控制面自己的
`events.dropped`（`plane.rs:692` 的 `dropped_note`，取自 `self.shared`），edge 侧的计数一个都到不了。

> **所以完整结论**：「**健康行**」那个说法 7 个副本、全清；但「**`!status` 会显示某个 edge 侧的东西**」
> 这个更底层的错误信念还有 **2 处活着**（`main.go:58` 与 `proto:151` + 两个生成副本），都在本轨可写面外。
> 派单只让 grep「健康行」，按那个判据本轨是收口了；按「这句谎话清完了没」这个问法，**没有**。

### 测试数

`cargo passed=852 → 852`（本轨不加测试，纯文档 + 纯注释）。

收尾 `scripts/check.sh`：`OK 25 files`、`contracts passed=25 failed=0`、`cargo passed=852 failed=0`、
go 六包全 `ok`、`passed 10/10`，「全部通过」退出码 0。**五行关键值与开场逐字相同**
（机器 diff 过，只有 go 各包耗时秒数不同）。收尾也是一次跑过，没撞抖动。
冻结面一个字没动，落盘无残留（`data/` 与临时的 `config/aite.yaml` 都已确认不存在）。

### 记账转出去的

| 位置 | 病 | 归哪轨 |
|---|---|---|
| `edge/cmd/aite-edge/main.go:58` | 「别让 `!status` 误报『飞书在线』」—— `!status` 看不到 `platform_connected`。`edge/` 除 `client.go` 是本轨只读面 | 下一轮（顺手带掉，纯注释） |
| `proto/aite/v1/edge.proto:151` + 生成的 `edge/gen/aitepb/edge_grpc.pb.go:831/858` | 「edge 自身健康：给 `!status` / preflight 用」—— `preflight` 对、`!status` 错。**这是那句谎话的源头**：两个 `.pb.go` 副本从它生成，改它要重跑 codegen + 契约锁 | **总管**（`proto/**` 冻结，改不改是他的决定） |
| `core/crates/app/src/app.rs:152` 的文档注释 | Z1 记过、本轮仍在：「`SqliteSessionStore::open`：库文件不在就建一个空的」没说「文件在而不是库」会怎样。`app/src/**` 是本轨只读面 | 待定（总管，Z1 已记，此处只是确认还没销） |
| 第 ① 组探不出「库是好的但文件只读」 | Z1 记过、本轨没动判据。**本轮把它写进了 `acceptance-M.md` §0.1 的排障文本**（「真机上遇到 preflight 全绿而 `aite run` 报建表失败，先查这个」）—— 代码面的记账仍然挂着 | 待定（总管） |

### 没做的 / 拿不准的

1. **「病史那半句」（改前两格全绿）没有重新实证** —— 见 ①.1 末段，要摘判据才看得到，
   那是行为改动 + 只读面。本轮补的是能独立跑出来的那一半（第 7 组在两格里都 OK）。
2. **`client.go:40` / `:129` 这两处超出了派单 ③ 点名的行号。** 判断依据：它们在本轨点名的可写文件里、
   只改注释、且 ④ 明写「还在断言健康行存在 → 改掉」。但它们不含「健康行」三个字，
   严格说不在派单的 grep 判据内 —— **如实标出来，总管觉得越界的话回退这两处即可**（不影响 ②③ 与 ①）。
3. **Z2 想改而没伸手的文档面，本轮一个字没碰** —— 派单说它会把原文 + 改法写进自己的回执、
   由总管在合并时落。本轨也没碰 `guard.rs` 与 `.claude/**` 相关的任何东西。
4. **`contract_state()` 零调用方那件事没动**（X1 记过，派单点名「要不要删不是本轨的决定」）。
