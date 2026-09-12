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
| `acceptance-M.md:358` | `<!-- 台账 §4.2；V5 修掉后删本段 -->` | 整段引用框要删 |
| `:363` | 「**这是已知记账（台账 §4.2）**」 | 已不是记账 |
| `:435` | 「**已知记账，不是故障** … **别去查沙箱**」 | 排障表里这一行会把人引向错误结论：现在查不到就是真故障 |
| `:554` | 「这一路交付中 `!status` 查不到它」 | 同上 |
| `:864` | 「交付中的短任务查不到，见 §0.4」 | 同上 |

这五处是 V3 自己留的记账注释，约定就是「对应轨修掉后连注释一起删」。V5 修了，没人删。

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
| `tests/reconnect_replay.rs:168` | `if self.inner.call_count() > 0 { return; }` 是死代码 —— 闸门期间 `call_count()` 恒为 0，那个「防丢通知」的守卫从没生效过 |
| `tests/graceful_shutdown.rs:237,249` | `started` 起在建场之前，`elapsed < 3.0` 因此包含两个各 5s 预算的 `wait_until`，量错了区间 |

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
