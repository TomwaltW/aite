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
