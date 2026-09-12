# RΩ 合并前审核台账 —— 2026-09-12

> 审核对象：RΩ（组装起飞 + preflight + 评测接线 + compose + 删 Python 树）合并前的未提交改动
> 规模：208 文件、+1234 / −33272（其中 178 个删除全是 Python 树）
> 方法：十个面各一个 agent 对照 Python 原版读代码 → 每条结论再交一个独立 agent **试图证伪**，
> 共 34 个 agent。**确认 22 条、证伪 2 条、未复核的 low 26 条。**
> 上一轮（七轨合流）的台账是 `review-findings-2026-09-11.md`，本文接着它记。

判据沿用上一轮：**每条结论挂实测**，「应该会」「大概」不算数。合并前修掉的那些，
逐条都做了「改前红 / 改后绿」两次实跑。

---

## 一、先说结论

**能合。** 三件最要紧的事都核实过了：

1. **B8 的 `passed 10/10` 是真接线跑出来的，不是 DemoPlane。** 调用链
   `main.rs:83 → wiring::evals_wiring → Wiring.plane（wiring.rs:89）→ cli.rs:370 →
   runner.rs:394/289 → wiring.rs:60 AgentWorker::new + wiring.rs:70 InProcessControlPlane::new`；
   硬证据是 `nm -U core/target/debug/aite`：`InProcessControlPlane` 333 个符号、`AgentWorker` 180 个、
   **`DemoPlane` 0 个**（`demo_plane.rs` 只被 `tests/` 引用，压根没链进二进制）。
   `evals/p0/**` 冻结面 `git diff` 为空。
2. **组装与收尾这两条冻结序列是对的。** 十步顺序、`contract_version` 比对排在一串重对象之前
   且真的 `return Err`、`recover_orphans` 在 `platform.start()` 之前、
   收尾七步（`platform.stop()` → `plane.join()` 限时 → **超时前先抄 in_flight** → `abort` →
   `cancel_task(notify=false)` → `sandbox.close_all()` → `store.close()`）一步不缺一步不反、
   退出码 0/2/130 —— 都对得上 `git show HEAD:aite/app.py`。
3. **密钥红线守住了。** 复核 agent 带着真值环境变量实跑 `aite preflight --offline --json`，
   全量输出里一个取值都没有，只有变量名；`AiteApp` 的手写 Debug、`OpenAiCompatModel`（无 Debug 派生）、
   `resolve_api_key` 的错误消息都只出现变量名。

**但不能就这么合** —— 有 6 条 blocker、2 条 high。全部修掉了，见第二节。

---

## 二、合并前修掉的

### 2.1 compose 的四条 blocker —— 这份 compose 从没被实跑过

`docker compose config` 只解析 YAML，四条一条都抓不到；真 `up` 一次全撞上。

| # | 位置 | 病 |
|---|---|---|
| 1 | `docker-compose.yml` 两个 service | `bash -lc` 走登录 shell，Debian 的 `/etc/profile` 把 PATH 重置，`cargo` / `go` 当场 command not found。上一版用 `python:3.12-slim` 时 `python` 恰好在 `/usr/local/bin`，换镜像后这个写法**静默失效** |
| 2 | core service | `rust:1.98` 里没有 protoc，而 `core/crates/proto/build.rs` 必须要它（CI 专门装了一步，compose 没有） |
| 3 | core service | command 里 `cd core` 之后 exec，进程 cwd 是 `/app/core`；config 里所有路径按契约都相对**仓库根**，组装第 2 步 `require_system_prompt` 就 StartupError 退 2，配 `restart: unless-stopped` = 无限崩溃循环 |
| 4 | edge service | `working_dir: /app/edge` → 两个 unix socket 落在 `/app/edge/data/run/`，而共享卷挂的是 `/app/data/run`；`ListenUnix` 之前会 `MkdirAll` 把错路径静默建出来，两个进程各拿各的 socket，永远连不上 |

改法：两个 service 都以 `/app` 为进程 cwd exec，脚本第一行显式 `export PATH`，
core 现装 protoc（来源与 CI 的 `arduino/setup-protoc` 同一个官方 release，钉 v31.1，装进 CARGO_HOME 卷、重启不重下）。

**实测（真 `docker compose up`）**：
```
core-1  | INFO aite_edge_client::link: edge.connected socket=/app/data/run/aite-edge.sock
core-1  | INFO aite.app: aite.edge_status edge_version=0.0.1 contract_version=p0.2 sandbox_ok=true
core-1  | INFO aite.app: aite.up platform=feishu model=deepseek-chat sandbox=aite-sandbox:p0
core-1  | INFO aite_edge_client::ingress: ingress.listening socket=/app/data/run/aite-core.sock
$ docker inspect --format '{{.RestartCount}} {{.State.Status}}' aite-core-1 aite-edge-1
0 running / 0 running
```
SIGTERM 后两边 `aite.down` / `edge.down`、exit 0；`docker compose down -v` 收干净。

### 2.2 一条从来没有真正成立过的测试断言（blocker）

`core/crates/app/tests/crash_recovery.rs` 的 `store_failure_does_not_kill_the_process`：
空载连跑 20 次 **pass=11 fail=9**。

根因不是并发抖动，是**这条断言物理上走不通**：它想验「盘恢复后新任务还能交付」，
但模型脚本只有一步、已经被第一个任务吃掉了；第二个任务必然 `ScriptExhausted`，
worker 走 `[0s, 2s, 5s]` 三发重试 = **7 秒**之后才可能发出任何 `send_text`，而死线是 **5 秒**。
它偶尔变绿，是因为那个 44ms 只读窗口有时正好砸在第一个任务的 `update_task` 上，
worker 走 `fail()` 先发一条错误回帖，把 `count(send_text)==2` 凑到 2 ——
**即「系统表现更差时测试才绿」**。

改法：给恢复后的任务喂它自己的脚本；判据从数条数换成「那句答复原文发出去了 + 库里落成
`delivered` + `task_no == #A2`」；只读窗口后加一条 `count==1` 的精确断言，把「靠失败回帖凑数」
这条路堵死。**改后连跑 20 次 pass=20 fail=0（0.07s，原来 5.07/7.07s）**，
并用两把刀验过不是空跑（盘不恢复 → 红；恢复后的任务发不出回帖 → 红）。

### 2.3 preflight 会「一边漏容器一边报绿」（high）

第 6 组用 `list_files()` 回 `NotFound` 当「容器已收干净」的判据。但 edge 的 `Release`
是**先删记账再删容器**（`docker.go:382-402`）—— `ContainerRemove` 失败时记账已经没了，
探针照样回 `NotFound`，于是输出「· 容器已收干净」、preflight 退 0，
而打着 `aite.task=preflight-xxxx` 标签的真容器还在宿主机上占着 1 CPU / 1024MB。

改法：判据换成 `release()` 自己的回答，失败即 FAIL 并带出错误原文，
`fix` 给可直接粘的 `docker rm -f $(docker ps -aq --filter label=aite.task=<task_id>)`；
`list_files` 那发探针降级成纯 extra。判据函数抽成纯函数并配了两条单测。

### 2.4 卡片上那行提示教用户做一件会被静默丢弃的事（high）

RΩ 按派单 §⑦ 取了「不渲染死按钮、改成一行文字提示」。但提示写的是
「要停这个任务，请在**群里发**：`!stop #A17`」—— 用户照字面在群聊主输入框发一条，
路由 R5（要 `mentioned` 或已在话题内）不命中 → R6 不命中 → R7 不命中 → 落到 **R8「其余丢弃」**，
**零回复、零表情**。为了消灭「点了没反应的按钮」而加的提示，本身就是一条点了没反应的指令。

改法（三处同步）：提示改成点明投递条件的两条可行路径 ——「在本话题里回复 `!stop #A17`，
或在群里发『@我 `!stop #A17`』」（卡片本身 `reply_in_thread=true` 发进话题，
所以话题那条路一定走得通）；README 同步；`cards_test.go` 加一条会拦住回退的断言
（提示里必须出现「话题」或「@」），实测把文案改回旧说法这条测试变红。

### 2.5 顺带修掉的

| 位置 | 改了什么 |
|---|---|
| `.contracts.lock` + `core/crates/app/src/lock.rs` | 删 Python 树之后**没重锁**（`lock --check` 红、`cli_smoke` 连带红）。重锁 → **`OK 25 files`**；`LOCK_DIRS` / `HEADER` / `skip()` / 单测里指着已删 `aite/contracts/**` 的死条目一并摘掉 |
| `docs/dev-spec-2026-09-11-rustgo.md` | 三处写死的 `OK 36 files` → `OK 25 files`，§3.0 锁定面删掉 Python 那条并注明 36 → 25 的算法 |
| `core/crates/testing/tests/fake_model.rs` | `hold_spends_scheduler_ticks_not_wall_clock` 用「两万个 tick 必须 1 秒内跑完」当判据，机器一忙就假红（满载 30 跑全红）。改成暂停时钟下虚拟时间必须为 0（确定性）+ 10s 墙钟兜底挡 `std::thread::sleep`。满载 30 跑全绿，注入 tokio timer 实测变红 |
| `core/crates/worker/src/agent.rs` | 台账 ⑥ 里 RΩ 漏掉的一条：单个 artifact 的 `title` / `mime` 仍用「字符串化后为空」而非 Python 真值语义，`title: false` 会把文件名显示成字面量 `"false"`。补 3 条用例（六种假值各一路） |
| `core/crates/app/src/preflight.rs` + `tests/cli_smoke.rs` | 「密钥绝不出现在输出里」这条唯一的守门断言是**恒真**的（跑的是 `--offline`，而 `redactor.add()` 的调用点全在 3/4/5 组、offline 下全 skip）。补三层测试：纯函数（scrub / 嵌套 extra / 短值不替换）、接线（拆掉任一处 `redactor.add` 就红）、渲染面；cli_smoke 那条改名成名副其实的「报变量名不报取值」 |
| `scripts/check.sh` | B8 从「只打印不计分」改成硬门禁：退出码 0 **且**最后一行逐字 `passed 10/10`，两条都判 |
| `Makefile` | `make test` 的 go 侧补 `-race`（check.sh 与 CI 早就有了，就它落下） |
| `.gitignore` | 随 Python 树作废的条目清掉 |
| `docs/demo-3min.md` / `docs/acceptance-M.md` | 两份文档里还写着「一个红色停止按钮」「卡片上只有停止按钮」，按钮已经没了；改成实际形态并给出「本来想点的按钮 → 现在怎么做」的替代路径（停止走 `!stop`，证据走 `aite evidence show <task_id>`，输出第二行 `目录 …` 就是原来那个按钮会回帖的路径） |

---

## 三、被证伪的两条（记下来，免得后人再报）

1. **「preflight 第 6 组四发 RPC 一个超时都没有，daemon 卡住时会无限期挂起」** ——
   行号和「裸 await」这个事实都对，但据以定级的场景在上游已经被挡掉了：
   第 6 组的第一发（status）在 edge 侧有 2s 硬上限，而且它后面就是短路闸门。
2. **「『一个字节都不许写进仓库 data/』这条硬约束的两处断言全是恒真」** ——
   「恒真」这一半属实，但据以定 medium 的因果链（真出这个回归时两条一条都不会红）实测不成立。

---

## 四、记账给下一批


### 4.1 medium（8 条）

| 严重度 | 位置 | 病 |
| --- | --- | --- |
| medium | `core/crates/app/src/app.rs:329` | contract_version 门禁在 edge 未就绪时静默降级成一行 warn，且此后整个进程生命周期不再比对 |
| medium | `core/crates/app/src/run.rs:185` | 宽限期超时那一支：抄 in_flight 与 runner.abort() 之间在多线程 runtime 下是真并发窗口，Python 单事件循环下为零 |
| medium | `core/crates/app/src/preflight.rs:576` | 网络类 FAIL 丢掉了错误的 source() 链，代理/DNS/TLS 三种病打出来一模一样 |
| medium | `core/crates/app/src/preflight.rs:1314` | Redactor 这道红线兜底在全仓零测试覆盖：唯一的红线测试跑的是 --offline，而 --offline 下 Redactor 恒为空 |
| medium | `core/crates/app/src/wiring.rs:98` | `--model live` 的配置校验跑在参数解析之前，把 `--list` / `-h` / 参数错误全抢了先，并让同一次改动里刚修好的 `-h` 又失效 |
| medium | `core/crates/app/src/wiring.rs:153` | `--sandbox docker` 档「场景收尾把容器收干净」实际是空操作，超时/异常收场的容器会留到 aite-edge 停机 |
| medium | `edge/internal/sandbox/docker.go:423` | ReapIdle 交回来的 released 在 core 侧没有任何消费者，台账那条的症状原样还在 |
| medium | `core/crates/app/src/preflight.rs:946` | aite preflight 交付面 1557 行只有 1 条 offline 冒烟测试，Python 侧 25 条 preflight 测试一条都没接管 |

### 4.2 修复 agent 顺带发现、本次没动的（4 条）

| 位置 | 病 |
|---|---|
| `docker-compose.yml` | `cargo build` 写进仓库里的 `core/target`（本机已 13G 且是 darwin 产物），linux/darwin 共用一个 target 会互相全量重编。应给 core 加 `CARGO_TARGET_DIR` 指到命名卷 |
| `docker-compose.yml` | `GOPATH=/go` 是容器层，`down` 之后再 `up` 会把全部 Go 依赖重下一遍（实测约 1 分钟）。加一个 `gomod:/go/pkg/mod` 卷就好 |
| `core/crates/control/**` | `TaskStatus::Answering` 不在 `ACTIVE_TASK_STATUSES`（=created/planning/working）里。`deliver()`（`agent.rs:643-648`）在 `answering == true` 时把状态置成 `Answering` —— 那是 W3 那条路（**第一步就 `final`、没发过卡片**的短任务）；`answering == false` 时置成 `Working`，仍在活跃集里。所以口子只开在 W3 这一路：`list_active_tasks` 空掉的那一刻，任务后面还有 send_text + evidence + 最后一笔 update_task 没做完，**`!status` 这段时间查不到它**。（2026-09-12 复核收窄：原先记成「每次交付都这样」是写宽了。）|
| `core/crates/app/src/preflight.rs` | Redactor 还有两个口子：① `check_config` 失败时 run_checks 直接返回，`redactor.add()` 一次都没执行，而 detail 里会回显 serde_yaml 的出错标量；② `redactor.add` 硬编码那 4 个变量，而 `env_var_names` 是泛化扫 `*_env` —— 契约加第 5 个 `*_env` 的那天脱敏会静默漏掉它 |

### 4.3 销账（有依据地不做）

**R7「`serde_json` 没开 `preserve_order`」建议销账**，三条理由：
① 立账理由（「与 Python 对拍会满屏 diff」）随 Python 树删除已经蒸发，没有受益方；
② 真正吃顺序的两处（`evidence.rs` 的 `canonical_json()` 哈希、`fingerprint.rs` 的 T20 指纹）
都已显式排序，且注释里写明「哪怕别的 crate 开了 preserve_order 也不受影响」；
③ 它是 workspace 级 feature，feature unification 会让全部 13 个 crate 的 `serde_json::Map`
从 BTreeMap 变 IndexMap，换来的只是人眼看的键序好看点。

### 4.4 low（26 条）


| 面 | 位置 | 病 |
| --- | --- | --- |
| 组装 | `core/crates/app/src/app.rs:124` | `build_app` 的副作用超出 C-TΩ-1 声明的范围（连网 + 最多 4s 睡眠 + 建 SQLite 库文件），而钉这条的测试只覆盖注入路 |
| 组装 | `core/crates/app/src/cli.rs:52` | `aite run --help` 被 clap 截胡，打的是不含任何真实选项的帮助；手写 USAGE 那一行基本是死代码 |
| 组装 | `core/crates/app/src/app.rs:73` | `AiteApp.edge` 的注释说它是给 `!status` 健康行和收尾日志留的，这两处都不存在 |
| 收尾 | `core/crates/app/src/run.rs:180` | --grace 放行超大有限数，收尾里 Duration::from_secs_f64 直接 panic，sandbox.close_all / store.close 整段被跳过 |
| 收尾 | `core/crates/app/tests/crash_recovery.rs:123` | emit 之后直接 list_active_tasks(...)[0]，没有守卫；任务已交付时会越界 panic |
| preflight | `core/crates/app/src/preflight.rs:212` | Redactor 的「短于 4 个字符不替换」判的是字节数不是字符数，非 ASCII 取值会被打成马赛克 |
| preflight | `core/crates/app/src/preflight.rs:1252` | check_storage 的可写探测改成了真建目录再删，preflight 从此对 evidence_dir / artifacts_dir 有写副作用 |
| 评测接线 | `scripts/check.sh:41` | `scripts/check.sh` 的 B8 仍然只打印不计分，横幅还写着「RΩ 前允许不满分 / not implemented」 |
| 记账-Rust | `core/Cargo.toml:33` | R7 `serde_json` 的 `preserve_order` 这条账没做，JSON 输出仍是字母序 |
| 记账-Rust | `core/crates/evals/src/demo_fixture.rs:421` | `--help` 这条账只修了 `evals run`，`evals demo-fixture --help` 仍是 stderr + 退出码 2 |
| 记账-Rust | `core/crates/control/src/plane.rs:200` | `FinishGuard::drop` 的新注释把保证写大了：`continue_session` 的 steer 竞态窗口仍在 await 上，且这条改动零断言 |
| 记账-Go | `edge/internal/sandbox/docker.go:396` | Release("") 仍返回 SandboxInternal，台账这条没动，且与 proto 注释和函数自己的注释都矛盾 |
| 记账-Go | `edge/internal/sandbox/docker_pure_test.go:1` | 台账点名的 orphans 仍只在 docker tag 下被跑到，可纯化的 inspectStamps 也没测 |
| 删Python | `../../.venv:1` | 主仓（非 worktree）里的 Python 构建残渣一个没清，含 202M 的 .venv |
| 删Python | `.gitignore:14` | 几个仍在用的文件里留着 Python 时代的说法，其中守卫的保护面/探针还指着已删的 aite/contracts |
| 删Python | `README.md:13` | README 说三份 inventory 是 Python 树「唯一的存世记录」，但 45KB 的旧 Python spec 也还在树里 |
| compose-CI | `Makefile:24` | `make test` 的 go 测试没有 -race，而 check.sh 和 CI 都有 |
| compose-CI | `README.md:90` | README 新写的「`--offline` … CI 用这一档」在 ci.yml 里没有对应步骤，且和 README 自己的 CI 章节冲突 |
| compose-CI | `.gitignore:1` | Python 树删完了，.gitignore 里的 Python 条目全是死的 |
| Rust横切 | `core/crates/app/src/cli.rs:47` | aite run --grace 传一个超出 Duration 上限的有限数会在收尾时 panic（cli 只校验了 finite 和 >= 0） |
| Rust横切 | `core/crates/app/src/cli.rs:52` | aite run --help 打不出自己的用法；唯一能打出来的写法走 stderr + 退出码 2 —— 正是 RΩ 在同一次改动里给 evals 修掉的那条 |
| Rust横切 | `core/crates/app/src/preflight.rs:1218` | preflight 落盘检查 FAIL 时「卡在 」是空的：卡住的那层目录恰好是仓库根时，rel() 返回空串 |
| Rust横切 | `core/crates/app/src/cli.rs:108` | --traceback 承诺「完整错误链」，实际只是把同一句话用 Debug 再包一层引号 |
| 测试成色 | `core/crates/app/tests/graceful_shutdown.rs:248` | grace_timeout 那条的 elapsed<3.0 量错了区间：计时起点在建场之前，包含两个各 5s 预算的 wait_until |
| 测试成色 | `core/crates/app/tests/reconnect_replay.rs:165` | GatedModel::wait_gate_reached 防丢通知的守卫是死代码，闸门期间 call_count() 恒为 0 |
| 测试成色 | `core/crates/app/tests/cli_smoke.rs:1` | 守卫 guard_bash.py 还挂在 settings.json 上，但它唯一的回归测试随 Python 树一起删了，没接管 |

---

## 五、合并时的实测基线（下一批的起跑线）

```
$ scripts/check.sh
契约锁 --check ............ OK 25 files
C1 契约测试 ............... contracts passed=25 failed=0
B 全量 cargo test ......... cargo passed=718 failed=0     （连跑三遍，三遍都是这个数）
B 全量 go test（-race）.... 八个包全 ok
B8 评测 ................... passed 10/10                  （现在是硬门禁）
clippy -D warnings / fmt --check / go vet / gofmt ... 全干净
全部通过，退出码 0

$ cd edge && go test -tags docker ./internal/sandbox/... -count=1
ok  	aite/edge/internal/sandbox	13.611s
$ docker ps -a --filter label=aite.task -q | wc -l
0
$ docker compose config -q ; echo $?
0
$ core/target/debug/aite preflight --offline
汇总：OK 2 · WARN 1 · FAIL 0 · SKIP 4（共 7 项，过了 7 项）   退出码 0
```

> **合并后在 main 上复验时的一条未复现现象**：第一次全量 `cargo test` 出了
> `717 passed / 1 failed`，紧接着连跑六遍都是 `718 / 0`。当时机器上正有六个 agent 在抢 CPU。
> **失败的测试名没留下** —— 因为 `check.sh` 把 cargo 的输出 grep 成只剩计数行。
> 已经改掉（失败名现在会打出来），但这一次的现场找不回来了。
> 下一批谁再撞到 `failed=1`，请把 `error: test failed, to rerun pass ...` 那行贴进回执。

---

## 六、一件与代码无关、但要记一笔的

这次审核与合并的会话是在 `~/Documents/Aite/aite`（正要被删的 Python 包目录）里起的，
`CLAUDE_PROJECT_DIR` 就定死在那儿，而 hook 命令是
`python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"` —— 文件不存在 → hook 执行失败
→ 非阻塞放行、不报警。**整个会话里守卫是静默失效的**（实测：Read 守卫脚本自身没被拦、
改冻结的 `docs/dev-spec-*.md` 也没被拦）。

已知的那条纪律是「`cd` 必须和 `claude` 写在同一行」，这次是它的另一个形态：
**在仓库子目录里起 claude 同样会让守卫失效**。派单里那条开场自检（Read 守卫脚本必须被拦）
就是为这个准备的 —— 它有效，但只在子会话里跑过，总管自己这一侧没跑。

