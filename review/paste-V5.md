# 任务 V5 — 收尾与控制面的剩余记账（进程活久了、或者两件事同时发生的那几条）

## 背景：这轨是从哪来的

Aite 是飞书上的 Claude Tag 闭环。原来是 Python，2026-09-11 起用 Rust(core) + Go(edge) 重写：
R0 骨架 → R1–R7 七轨并行 → RΩ 组装起飞（`aite run` / `aite preflight` / 评测接线 / compose 双服务），
顺带删掉整棵 Python 树（178 个文件、−33272 行）。RΩ 刚刚合入 main（`11322b3`）。

合并前做了一轮 34 个 agent 的审核：十个面各一个 agent 对照 Python 原版读代码，**每条结论再交一个
独立 agent 试图证伪**，确认 22 条、证伪 2 条、low 26 条。全文在
`review/review-findings-2026-09-12-romega.md`。**你这一轨的活全部出自它的第四节「记账给下一批」。**

P0 的权威文档 `dev-spec-2026-09-09.md` 里还没兑现的只剩两件：§2.4 的 **M1–M6 真机验收**
（真实飞书测试群，总管亲自跑）和「能录一段 3 分钟演示」。§1 又明令禁止「任何『顺手做一点 P1』的行为」。
所以这一批六轨的共同定位是：**把 P0 收尾到「总管可以坐下来跑 M1–M6、可以开录」，外加把审核留下的账清掉。**

你这一轨的四条账有一个共同点：**它们在替身测试里永远不出现。** 单进程、几十毫秒跑完的集成测试里，
「edge 晚 4 秒才起来」「一个任务恰好在收尾那一刻跑完」「模型思考了 5 分钟」这些条件一个都构不成。
而 M1–M6 恰恰是它们全都成立的场合 —— `docs/acceptance-M.md:398` 明写 **M6 要跑三遍**
（只重启 edge / 只重启 core / 两个都重启），`dev-spec-2026-09-09.md:71` 的 M3 判据里明写着
「**5 分钟后 `docker ps` 无该任务容器**」—— 那 5 分钟就是本轨④的 `idle_sec`。
换句话说：**这四条不是理论洁癖，是总管坐下来跑验收时最可能撞上的那几条。**

四条里有三条的正确改法**不唯一**，其中两条会顶到冻结面的边界。派单不替你定死改法，
但要求你在回执里说清选了哪条、为什么、代价是什么。

> **关于行号**：下面点到的每一个 `file:line` 都在 `11322b3` 上逐个打开核过（2026-09-12）。
> 台账里的行号是审核当时的，有几处已经偏了 —— 本派单写的是**核对后的**行号，与台账不一致时以本派单为准，
> 差异都在各条里注明了。你动手前如果发现又偏了，说明有别的轨先动了那个文件，**停下报告**。

## 必读（按顺序）

1. `review/review-findings-2026-09-12-romega.md` —— **第四节 4.1 的第 1、2、7 行 + 4.2 的第 3 行**
   就是你的四条活；**第五节**是你的起跑线基线；**第六节**讲守卫为什么会静默失效，
   开场自检最后一条就是为它准备的。
2. `review/inventory-core.md` **§7（第 266 行起）** —— Python `app.py` 的 `build_app` 十步（268 行）、
   `_shutdown` 冻结序列（272 行）。**272 行那句「（先抄再取消）」正是本轨②要动的那条**，
   动它之前先把这一行读完。
3. `docs/dev-spec-2026-09-11-rustgo.md` **§2.1**（启动顺序无关 / `contract_version` 门禁 —— ①的依据）、
   **§3.2**（`SessionStore` / `SandboxPort` / `ToolGateway` 的冻结签名 —— ③④的边界）、
   **§3.3**（失败与取消）。**这份文档冻结，只读。**
4. `docs/acceptance-M.md` **§M3（194 行起）与 §M6（392 行起）** —— 你这四条病的真实现场。
   顺带看它的「已知盲区」一节（490 行往后），里面第 7 条「沙箱 id 不进证据」跟你的④是同一件事的两面。
5. `review/paste-ROMEGA.md` —— 上一轨的派单。看它的语气，以及「每条结论挂实测」是什么标准。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-v5
分支     : task-v5
基线     : 0bc8d55（建 worktree 时钉的 main HEAD 具体 sha，不是分支名）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、go 1.27.1、protoc 36.1、Docker 29.6.1
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。


> **基线说明**：`0bc8d55` = RΩ 合入 main 那次（`11322b3`）**再往前一格**。
> 那一格只改了两个文件：`scripts/check.sh`（B 全量 cargo test 那步改成「失败测试名在前、计数在后」）
> 与 `review/review-findings-2026-09-12-romega.md`（收窄 Answering 那条 + 补记一次未复现的 717/1）。
> `git diff --stat 11322b3..0bc8d55` → `2 files changed, 11 insertions(+), 2 deletions(-)`。
> 本派单正文里凡是写「在 `11322b3` 上核过 / 实测」的，指的是核对当时那一格，**代码面与你的基线逐字相同**。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v5
git log --oneline -1                                   # 期望 0bc8d55（记下它，回执里当基线）
git status --short                                     # 期望空
scripts/check.sh                                       # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

`scripts/check.sh` 的关键行期望（**这就是起跑线，任何一条变小都是回归**）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=718 failed=0` |
| B 全量 go test（-race） | 八个包全 `ok`（`-race` 是硬门禁，别去掉） |
| B8 评测 | `passed 10/10` —— **RΩ 起它是硬门禁**，check.sh 会计分 |

另外单独跑一次（不在 check.sh 里）：

```bash
cd edge && go test -tags docker ./internal/sandbox/... -count=1   # 期望 ok
docker ps -a --filter label=aite.task -q | wc -l                  # 期望 0
```

> **已知的一条假红**：docker 档沙箱测试在多轨并行时会假红（六个 worktree 同时跑真容器会互相挤）。
> 它红了先看两件事：是不是**只有这一个包**红、**单独跑**是不是绿。是的话就是并行挤的，不是回归。

> **另一条已知现象**（台账第五节末）：合并后曾出过一次 `717 passed / 1 failed`，紧接着连跑六遍全是
> `718 / 0`，当时机器上有六个 agent 抢 CPU，**失败的测试名没留下**。你再撞到 `failed=1`，
> 请把 `error: test failed, to rerun pass ...` 那行原样贴进回执 —— 现在 check.sh 会打出来了。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
> （2026-09-12 总管这边真踩过：会话在仓库子目录里起，`CLAUDE_PROJECT_DIR` 就定死在那个子目录，
> hook 命令 `python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"` 找不到文件 →
> 执行失败 → **非阻塞放行、不报警** → 整个会话守卫静默失效。台账第六节记的就是这件事。
> 那一次连「改冻结的 `docs/dev-spec-*.md`」都没被拦。）

## 可写路径

| 面 | 能不能动 |
|---|---|
| `core/crates/app/src/app.rs`、`core/crates/app/src/run.rs` | 可写（①②） |
| `core/crates/control/**` | 可写（②③） |
| `core/crates/gateway/**` | 可写（④） |
| `core/crates/edge-client/**` | 可写（①的假 edge 测试与比对下沉落这里） |
| `core/crates/worker/src/agent.rs` | 可写，但**只为③的窗口收窄**服务，别顺手改别的 |
| `edge/internal/sandbox/**` | 可写，但④**不要**改 `ReapIdle` 的语义（上一轮刚改对，见下） |
| 上述这些的 `tests/`、以及这些 crate 自己的 `Cargo.toml` | 可写 |
| `core/crates/app/src/preflight.rs` | **归 V2，一个字不许动** |
| `core/crates/app/src/cli.rs`、`src/wiring.rs`、`tests/cli_smoke.rs` | **归 V6，一个字不许动**（要引用可以只读） |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock` | **冻结**。要动 → 停下报告 |
| `evals/p0/*.yaml` | **一个字不动**（十个场景是验收面） |
| `docs/dev-spec-2026-09-09.md`、`docs/dev-spec-2026-09-11-rustgo.md` | **冻结** |
| `core/Cargo.toml` | **守卫保护面**（`guard_bash.py:33`），不许动。**crate 自己的 `Cargo.toml` 不在保护面里，可以动** |

`review/inventory-core.md` 不是冻结面 —— ②如果真动了收尾序列，§7 第 272 行那句要跟着改。

---

## 要做什么

### ① `contract_version` 门禁只在起飞那一次生效

**病在哪。** `build_app` 的第 4 步比对 `contract_version`（`core/crates/app/src/app.rs:162–163`），
比对函数是 `check_contract_version`（`app.rs:297–339`）。版本不一致时真的 `return Err`
（`app.rs:302–308`，消息把两边版本都印出来）—— 这一半审核已经确认是对的。出问题的是**拨不通**那一半：

| 行 | 干了什么 |
|---|---|
| `app.rs:38` | `const EDGE_STATUS_ATTEMPTS: u32 = 5;`（34–37 行的注释把折中理由写清楚了，值得读） |
| `app.rs:299` | `for attempt in 1..=EDGE_STATUS_ATTEMPTS` |
| `app.rs:323–325` | 失败就 `tokio::time::sleep(1s)`（最后一发不睡）→ **最多 4 秒** |
| `app.rs:331–337` | 5 发全败 → 一行 `tracing::warn!(… "aite.edge_unreachable …")` |
| `app.rs:338` | `Ok(())` —— **放行，而且此后整个进程生命周期不再比对** |

**全仓再没有第二个比对点落在 `aite run` 这条路上。** 另外两处都不在起飞路径里，而且分属别的轨：
`core/crates/app/src/preflight.rs:993`（`aite preflight` 第 6 组，V2 的文件）、
`core/crates/app/src/wiring.rs:187`（评测 `--sandbox docker` 的体检，V6 的文件）。**这两个只能读，不能改。**

**为什么这是常态路径，不是异常路径。** 三条各自独立可达：

1. `docker-compose.yml:17` 逐字写着「所以刻意**不写 depends_on** —— 谁先起都行，先起的那个会等另一个」
   （§2.1「启动顺序无关」）。**这是对的，别改。** 于是「core 先起完、edge 编译/启动超过 ~4s 才就绪」
   是完全正常的一次 `docker compose up`。
2. M6 三遍里的**「只重启 edge」**那一遍：core 长活不动、edge 换了新契约起来 ——
   这条路上门禁**必然**不生效，没有任何随机性。
3. 真机排障时手动重启 aite-edge 同理。

**怎么验证它确实病着**（选一条跑出来贴回执）：

- **甲（糙但快）**：不起 edge 直接起 core，4 秒后应看到 `aite.edge_unreachable`；然后把 edge 起起来，
  确认日志里**从此再没有** `aite.edge_status`（那行在 `app.rs:310–318`，比对成功才打）。
- **乙（硬）**：用假 edge，第一发 `GetStatus` 回 `UNAVAILABLE`、之后回一个**错的** `contract_version`，
  看 core 从头到尾发不发得现。现成零件见下面第 1 步。

**复核已经否掉的两条改法，别再提：**

- 给 compose 加 `depends_on` —— 违反 §2.1「启动顺序无关」，而且「edge 起来了」≠「契约对得上」。
- 把入参抽成闭包 —— 只换了个写法，第二次仍然没人调。

**改法方向（三步；可以只做前两步，但要在回执里说清停在哪一步、为什么）：**

**第 1 步，先补测试。** 现成的假 edge 在 `core/crates/edge-client/tests/common/mod.rs`（434 行）：

| 零件 | 行 |
|---|---|
| `EdgeStatusSvc`（struct / `impl EdgeStatusService`） | 414–417 / 419–434 |
| `contract_version` **写死成 `CONTRACT_VERSION`** | `common/mod.rs:427` ← 要测不匹配，先得让它可设 |
| `EdgeState`（假 edge 的共享账本，无 `contract_version` 字段） | 30–41 |
| `EdgeState::enter`（「让某个方法下次返回指定 status」，UNAVAILABLE 那一半现成） | 43–51 |
| `serve()`（三套 service 挂一个 UDS，含 `UnixListenerStream`） | 138–169 |

**测试落点二选一，倾向前者：**

| 落点 | 代价 |
|---|---|
| `core/crates/edge-client/tests/` | `tonic` / `tokio-stream(net)` / `tempfile` 都已是这个 crate 的依赖（见 `core/crates/edge-client/Cargo.toml`），**一行 Cargo 都不用改**，假 edge 就在隔壁 |
| `core/crates/app/tests/` | 要给 `core/crates/app/Cargo.toml` 的 `[dev-dependencies]` 加 **`tonic.workspace = true`** 和 **`tokio-stream = { workspace = true, features = ["net"] }`**（`UnixListenerStream` 在后者里；台账只提了 `tonic`，**核过，少一个编不过**）。两者都已在 `core/Cargo.toml` 的 `[workspace.dependencies]` 钉过版本（`tokio-stream = "0.1"` 第 29 行、`tonic = "0.14"` 第 50 行），**不是新增 workspace 依赖**；`aite-proto` 已经是 app 的正式依赖，`tokio` 是 `features = ["full"]`。但要复制一份假 edge |

**第 2 步，把比对下沉到 `EdgeClient`。** 两个抓手都核过：

- `core/crates/edge-client/src/lib.rs:74–83` 的 `status()` 是**唯一**的出口（内部走 `link.status_client()`，
  成功时 `link.note_ok()`）。
- `core/crates/edge-client/src/link.rs:129–159` 的后台探针 `probe()` 是「重新连上」的**唯一**事件源：
  `link.rs:140–145`，`connected` 由 false 翻 true 那一刻打 `edge.connected`（143 行）。
  探针由 `note()`（`link.rs:120–126`）在见到 `Unavailable` / `DeadlineExceeded` 时拉起。

让「连上就比一次」变成 `EdgeClient` 自己的行为，`app.rs` 那边只保留「起飞这一次要么比过、要么等 4s」。

**第 3 步，比出不一致之后做什么 —— 这条你拍板，回执说清选了哪条。**

| 选项 | 说明 |
|---|---|
| (a) | 只记一条 ERROR + 一个计数器，让 `!status` 说得出来。最轻 |
| (b) | 闸门：置一个 latch，此后每发 platform / sandbox RPC 直接失败并带人话。「不干活但不自杀」 |
| (c) | 触发优雅停机、退出码 2（与「拒绝起飞」同口径）。最重 |

**总管的倾向：不要 (c)。** M6 那一遍「只重启 edge」如果 core 自己也退，compose 的
`restart: unless-stopped` 会把两个进程拖进互相重启；(a) 又太轻（真机上没人盯日志）。
**倾向 (b)，但必须能自愈** —— edge 换回对的版本之后 latch 得解得开（探针那条路给了你解锁的时机）。

**附带代价（必须写进回执，别偷偷吃掉）：**

- edge 没起时每次 `aite run` 白等 4 秒（`app.rs:323–325` 那 4 次 `sleep(1s)`）。
- 而这 4 秒排在第 5 步建模型（`app.rs:179–182` 调 `build_model`，函数在 `app.rs:282–294`）**之前** ——
  `base_url` 写错、密钥环境变量没设，也得等满 4 秒才看到原因。
- **想靠调整组装顺序来治这一条的话，先停下。** 组装十步的顺序是 C-TΩ-1 冻结的
  （`review/inventory-core.md:268`，`contract_version` 比对是第 4 步），改顺序等于改冻结序列。
  可以在回执里提议，**别自己动**。治不了就明说「这 4 秒还在」。

---

### ② 收尾宽限期超时那一支的并发窗口

**病在哪。** `core/crates/app/src/run.rs` 的 `shutdown()`（171 行起）：

| 行 | 干了什么 |
|---|---|
| 178–182 | 宽限期内 `plane.join()` 没等到 → 走超时这一支 |
| **185** | `stranded = app.worker.in_flight();`（抄快照） |
| 186–192 | `tracing::warn!(… "aite.shutdown_timeout …")` |
| 195–198 | `if let Some(h) = runner { h.abort(); let _ = h.await; }` |
| 203–204 | 对 `stranded` 逐个 `app.plane.cancel_task(task, None, None, false).await` |

185 与 196 之间确实没有 `.await`。**但这不等于没有窗口**：派发循环跑在另一条 tokio 任务上
（`run.rs:133` spawn 的 `plane.run_forever()`），runtime 是 `new_multi_thread`
（`core/crates/app/src/cli.rs:72`，**V6 的文件，只读**），两条线是真并行的；而且 `abort()` 只是打标记，
真正生效要等那条任务下一次被 poll 到某个 await 点。

Python 那边是单事件循环：`_shutdown` 里 `stranded = list(...in_flight.values())`（`aite/app.py:531`）
到 `runner.cancel()`（`app.py:542`）之间没有 await，**窗口精确为零**
（`git show 598f476^:aite/app.py` 能把原文调出来，Python 树在 `598f476` 被删）。

派发是串行的（`core/crates/control/src/plane.rs:774–777`，`dispatch_loop` 里
`self.run_one(&task_id).await`），所以 `in_flight` 最多一条。窗口窄，但**两个方向都真**：

**方向甲（台账点的那条）。** 抄完之后派发循环又从队列里 pop 出一个任务并塞进 `in_flight`
（`core/crates/worker/src/agent.rs:875–877`），它不在 `stranded` 里 → 不会走 `cancel_task(notify=false)`
→ 库里停在 `created`/`working`。兜底是下次起飞的 `recover_orphan_tasks`，所以不是数据丢失，
是「这次收尾漏了一个，得等下次起飞才收干净」。

**方向乙（台账没写，本派单核出来的，比甲更疼）。** 抄的时候任务 X 还在飞；抄完之后 X 自己
正常跑完了（`agent.rs:918–920` 把它从 `in_flight` 摘掉，`run_one` 返回，`dispatch_loop` 回到
`queue.pop()` 的 await —— abort 正好落在那儿）。于是 `stranded` 里那份 X 是**过期快照**，
而 `cancel_task`（`plane.rs:1093`）**无条件**把状态改掉，全程没有一句终态判断：

- `plane.rs:1109` `task.status = TaskStatus::Cancelled;` → `plane.rs:1111` `update_task` ——
  **库里一条已经 `delivered` 的任务被改写成 `cancelled`**；
- 控制面的 `running` 此刻已空（`RunningGuard` 在 `plane.rs:224/229`，`dispatch_task` 于 837 行登记，
  函数返回即 drop），于是走 `if !running` 分支（`plane.rs:1144`）：
  `release_gateway_sandbox`（1145）、再 append 一条 `cancelled` 证据（1151）、
  把卡片改成 `CardStatus::Cancelled`（1164）、`finalize_evidence`（1171）；
- `finalize_evidence`（`plane.rs:885–894`）的幂等判据读的是**快照里**的 `evidence_root_hash`。
  worker 的 `finish()`（`agent.rs:777–796`）是先 `finalize`（789）再 `save()`（796），
  而 `save()` 会回写 `in_flight` 里的那一份（`agent.rs:801–804`）—— **所以快照抄在 `save()` 之后
  就不会 finalize 两遍，抄在之前就会**。而抄的时机恰恰不可控。
- 用户侧看到的：答复已经发出去了、文件也收到了，卡片却翻成「已取消」。

**怎么验证它确实病着。** 方向乙有一条可确定性复现的路，零件全在
`core/crates/app/tests/graceful_shutdown.rs`（352 行）：

| 零件 | 行 |
|---|---|
| `TINY_GRACE_SEC = 0.05`（宽限期压到 50ms，`plane.join()` 必然超时） | 39 |
| `holding_script()`（模型停在第 2 步不返回，`release_holds()` 才放行） | 30–37 |
| `drive_until_stuck()`（把任务推到「沙箱已建、模型停在第 2 步」这个确定状态） | 207–220 |
| 现成的超时用例（**已经是 `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`**） | 222 起 |

组装成：进到超时分支、抄完之后 `release_holds()` 让任务跑完，再断言库里那条是 `delivered`
不是 `cancelled`。**这条测试现在必须是红的** —— 你写完发现它绿，说明窗口没被你打开，
调整时序（用显式放行开关，不是 `sleep`）直到它红为止。破坏 → 红、修好 → 绿两份输出都要贴。

**改法方向（别自己定死，回执说清选了哪条；可以组合）：**

| 选项 | 做法 | 治哪个方向 | 代价 |
|---|---|---|---|
| (甲) | 把快照挪到 `h.abort(); h.await;` **之后**再抄 | 甲 + 乙 一次治完 | **偏离 C-TΩ-1 冻结的收尾序列** |
| (乙) | 保序，只在 cancel 之前按 id 从库里重读一次，终态就跳过（`store.get_task`，`core/crates/contracts/src/ports.rs:149`，在冻结 trait 里、能直接用；`AiteApp.store` 就在手边，`run.rs:212` 已经在用它）；终态判据用 `TaskStatus::is_terminal()`（`contracts/src/session.rs:44–49`） | 只治乙 | 改动最小、不碰冻结序列 |
| (丙) | 收尾开始时先给派发循环一个「不再 pop 新任务」的闸门 | 只治甲 | 要动 `plane.rs` 的队列面 |

**(甲) 为什么成立（已核）**：`in_flight` 的 remove **只在 `run()` 正常返回时发生**
（`agent.rs:918–920`，没有 Drop guard 兜着），被 abort 掉的任务条目会**留在表里**。
所以「抄后置」拿到的正好就是「真正被硬取消的那批」。

**(甲) 的代价说清楚**：`review/inventory-core.md:272` 逐字写着「（**先抄再取消**）」，
`run.rs:3–13` 的模块注释也照抄了这句（「先抄」在 `run.rs:8`）。
**总管的倾向是：(甲) 最干净。如果你能把它证明成「等价于原序列 + 修掉两个窗口」，就走 (甲)，
并把 `run.rs:8` 的注释与 `inventory-core.md:272` 的说法一起改，然后在回执里单独一节标明
「这是对 C-TΩ-1 冻结收尾序列的一处改动」。** 证不动就退 (乙)+(丙)。

---

### ③ `!status` 可能查不到「正在交付中」的任务

**先说核对结果 —— 台账 4.2 第 3 行说得比实际宽，三处要改。**

**改正一：不是所有 `deliver()` 都落 `Answering`。** 台账写的是「`deliver()` 第一件事就是把状态改成
Answering」。实际是 `core/crates/worker/src/agent.rs:643–648`：

```
// Answering 路径（W3：第一步就 final，没发过卡片）在状态机上是独立的一格
ctx.task.status = if answering { TaskStatus::Answering } else { TaskStatus::Working };
```

**只有 `answering == true` 那一路才落 `Answering`；`false` 那一路落的是 `Working`，仍在活跃口径里。**
`answering` 只有两个真值来源：

- `agent.rs:166–167`：第 0 步模型只回纯文本 → `self.deliver(ctx, &text, &[], true)`；
- `agent.rs:222–223`：`let answering = !ctx.card.sent();` —— 卡片一次都没发过时为真
  （`ensure_card` 在 `agent.rs:203`，只有非 final 的工具调用才发卡片）。

所以窟窿只有一个入口：**从头到尾没发过卡片的那条路。** 常见形态是「第一步纯文本答完」。
窗口是 `deliver()` 里 `save()`（649）之后到 `finish()`（736）之间那几笔：
`send_text`（714–721）→ 状态改 `Delivered`（723）→ delivered 证据（725–734）→ `close_card`（735）
→ `finish`（736，里面还有 `finalize` 789 + `release_task` 793 + `save` 796）。
理论上它也能带产物（222 行的判据是「卡片没发过」，不是「没跑过工具」），那时窗口里还多几笔 `send_file`。

**改正二：`!status` 的口径确实就是那三个状态**，台账这一半猜对了：

- `core/crates/control/src/plane.rs:458` 的 `cmd_status`，第一句（459）就是
  `self.store.list_active_tasks(&ev.chat_id)`。
- `ACTIVE_TASK_STATUSES = [Created, Planning, Working]`，`core/crates/contracts/src/session.rs:33–37`。

**改正三：Python 原版一模一样，这不是 Rust 引入的偏差。** 逐条对过：

| Python（`git show 598f476^:…`） | Rust |
|---|---|
| `aite/control/store.py:24` `ACTIVE_TASK_STATUSES = (created, planning, working)` | `contracts/src/session.rs:33–37` 同 |
| `aite/control/store.py:208` `list_active_tasks` | `contracts/src/ports.rs:151` 同口径 |
| `aite/control/plane.py:267` `!status` 走 `list_active_tasks` | `plane.rs:458–459` 同 |
| `aite/worker/loop.py:459` `task.status = answering if answering else working` | `agent.rs:643–648` 同 |

**边界（写死）：`ACTIVE_TASK_STATUSES` 不许动。** 它在 `core/crates/contracts/src/session.rs`
（`.contracts.lock` 第 14 行），而且被**两条**冻结测试钉着：
`core/crates/contracts/tests/frozen_values.rs:10–17` 逐值断言、
`core/crates/contracts/tests/roundtrip.rs:242–246` 断言 `!TaskStatus::Answering.is_active()`
（roundtrip.rs 也在锁里，第 20 行）。**「把 Answering 加进去」这条路是双重关死的。**

**另一件本派单核出来、台账没写的事**：**全仓没有任何一条测试断言 `deliver()` 会把状态落成
`Answering`。** `grep -rn "TaskStatus::Answering" core/ --include=*.rs` 只有四处命中：
`agent.rs:645`（实现本身）、`contracts/tests/roundtrip.rs:244`（断言它**不**活跃）、
`store/tests/concurrency.rs:368` 与 `store/tests/persistence.rs:163`（拿它当普通枚举值用）。
`worker/tests/test_final.rs` 只在第 1 行的模块注释里提了一句「W3 的 Answering 路径」，没有断言。
**这意味着下面的 (丙) 改起来不会撞红任何现存测试 —— 这本身就是问题，你要顺手补上钉子。**

**可选的方向（回执说清选了哪条）：**

| 选项 | 做法 | 备注 |
|---|---|---|
| (甲) | `cmd_status` 在 `list_active_tasks` 之外，再把控制面自己的 `running` 集合并进来：逐个 `store.get_task(id)`（`ports.rs:149`，冻结 trait 里现成）、用 `get_session` 过滤到本群 | **不改 `ports.rs`，可行性已核**：`Shared.running` 在 `plane.rs:153`，`steer_target`（`plane.rs:1025`）已经在这么用。**注意锁序**：`plane.rs:141` 写死了全局锁序 `cancelled → owned → running → steer → counters`，不许反向取 |
| (乙) | 什么都不改，销账：Python 继承来的既有行为，窗口只有交付的最后几笔，用户重发一次 `!status` 就好 | 要在回执里把「Python 也是这个行为」写清楚 |
| (丙) | 收窄窗口：把状态改成 `Answering` 的时机挪到 `send_text` 之后（`agent.rs:714–723` 之间），让「还在发东西」那段仍算 `Working` | 偏离 Python，但偏的方向是「活跃口径更诚实」。**现存测试一条都不会红（见上），所以走这条必须自己补钉子**，并说清补的那条为什么不算放水 |

**总管的倾向：走 (甲)。** 它不碰契约、不碰 Python 语义之外的东西，而且 `!status` 本来就该
回答「现在到底什么情况」—— 一个正在把文件发给你的任务不该从这句话里消失。走不通再在 (丙) / (乙)
里选，并把「Python 也是这个行为」写进回执：**这条账不值得为它破契约。**

---

### ④ `ReapIdle` 交回来的 `released` 在 core 侧没有消费者

**edge 那半边上一轮已经改对了，别动它。** `edge/internal/sandbox/docker.go` 的 `ReapIdle`（405 行起）：

```
docker.go:423    released, err := reapVictims(ctx, victims, d.Release)
docker.go:427    return released, err
```

`reapVictims`（`docker.go:441` 起）「一个失败不连累其余，也不把已经真删掉的 id 弄丢」，
函数头注释 `docker.go:430–440` 把理由写得很清楚：成功释放的 id 一个不少地回给 core，
只有一个都没释放成（`docker.go:459–460`）才把错误抛上去。

**core 收到之后把它扔了。** `core/crates/control/src/plane.rs:860–881` 的 `reaper_loop`：

| 行 | 干了什么 |
|---|---|
| 862 | `(self.sleep)(self.reaper_interval_sec).await` —— 默认 60s（`plane.rs:28` `REAPER_INTERVAL_SEC = 60.0`） |
| 866 | `sandbox.reap_idle(self.config.sandbox.idle_sec).await` —— 默认 300s（`contracts/src/config.rs:79`，`config/aite.yaml:29`） |
| 871–875 | 按条数 bump `sandbox.reaped` 计数器 |
| 876 | 打一行 `control.reaped` |
| — | **`released` 这个 `Vec<String>` 到此为止，没有别的消费者** |

于是 Gateway 的 `task_id → sandbox_id` 表（`core/crates/gateway/src/gateway.rs:56`）
照样留着那个已经没了的 id。清它的唯一入口是 `release_task`（`gateway.rs:369–387`），
而它只在任务收尾时被调（`agent.rs:793`）或被 `cancel_task` 的 `!running` 分支调（`plane.rs:1145`）。

**症状（一条可复现的路，每一环都核过）：**

1. 一个任务两次工具调用之间超过 `idle_sec`（300s）—— 模型思考慢、或者一步里生成长代码。
   `touch` 只在沙箱 RPC 时发生（edge 侧 `docker.go:265`/`307`/`350`/`377`；core 侧
   `gateway/src/tools/python_exec.rs:44`）。**`list_files` 刻意不 touch** ——
   `docker.go:370` 逐字写着「刻意不 touch：列目录是只读动作，不该把 reaper 往后推」。
   模型思考那段一次都不刷。
2. reaper 每 60s 扫一次（`plane.rs:862`），把它收走。
3. 下一次 `run_python` 走 `env.acquire_sandbox()`（`gateway/src/tools/python_exec.rs:33–37`
   → `tools/mod.rs:126–127` → `Inner::acquire_sandbox`，`gateway.rs:79`）——
   **第一句（`gateway.rs:80–82`）就是「有就返回」**，表里还有 id，于是**直接复用死 id**；
   `exec` 拿它去打（`python_exec.rs:39–42`），edge 回 `NotFound`，
   `map_err` 把它收成 `ToolFailure::sandbox` → `code=sandbox`。
4. worker `ctx.sandbox_errors += 1`（`agent.rs:243`），连撞
   `MAX_CONSECUTIVE_SANDBOX_ERRORS = 2`（`agent.rs:31`，判在 `agent.rs:255–259`）→ **任务 `failed`**。
5. `list_files` 那条路更糊：它用 `env.current_sandbox_id()`（`gateway/src/tools/files.rs:17`），
   拿到死 id 之后报沙箱失败 —— 而它本来的设计是「没有容器就如实说 `/work` 是空的」
   （`files.rs:1–4` 的模块注释 + 17–22 行那条 `else` 分支）。

这条正落在 M3 的验收判据上：`dev-spec-2026-09-09.md:71` 明写「5 分钟后 `docker ps` 无该任务容器」
—— 那 5 分钟就是 `idle_sec`。`docs/acceptance-M.md:307` 也提醒总管「M3 的容器多半已经被 reaper 收了」。

**边界（写死，别越线）：**

- **不要给 `ToolGateway` trait 加 `forget_sandbox`。** `core/crates/contracts/src/ports.rs`
  在契约锁里（`.contracts.lock` 第 11 行，trait 在 `ports.rs:112`），加方法 = 改冻结 trait + 重锁
  + 补齐**五个**实现方 —— `gateway/src/gateway.rs:311`、`testing/src/fake_gateway.rs:379`、
  `evals/src/real_stack.rs:206`、`worker/tests/common/mod.rs:569`、`control/tests/support/mod.rs:909`
  （**台账写的「四个」漏了一个，核过是五个**）。**复核已经否掉这条路。**
- **不要改 `edge/internal/sandbox/docker.go` 的 `ReapIdle` 语义**，它上一轮刚改对。

**可选的方向（回执说清选了哪条、为什么）：**

| 选项 | 做法 | 边界 |
|---|---|---|
| (甲) | **gateway 内部自愈**：`exec` / `list_files` / `put_file` / `get_file` 拿到 `SandboxErrorKind::NotFound`（`contracts/src/errors.rs:27–37`，`NotFound` 在 31 行）时，把 `sandbox_ids` 里那一条摘掉，然后重试一次（`run_python` 重新 `acquire` 一个新容器再执行） | 全部关在 `core/crates/gateway/**` 里，一行契约不碰 |
| (乙) | **不让它被收**：任务在飞的时候周期性 `touch`。gateway 手上有 `sandbox_ids` 表，知道谁还活着 | 要在 gateway 里养一条后台任务（生命周期、停机时怎么收），比 (甲) 重 |
| (丙) | **把 `released` 接到 gateway 上但不动 trait**：控制面手上的 gateway 是 `Arc<dyn ToolGateway>`（trait 对象，拿不到 `P0ToolGateway` 的私有方法），要接就得在 `ControlDeps` 上多一个可选闭包 —— 注入点在 `core/crates/app/src/wiring.rs`（**V6 的文件**）和 `app.rs`（你的） | **跨轨，合流会撞。不推荐；真要走先跟总管说** |

**总管的倾向：(甲)。** 理由：修复关在一个 crate 里；测试面现成；而且它顺带把
「edge 那半边把 id 交回来了、core 却没人接」这件事变成**不需要接** —— 沙箱没了就重建，谁收走的都一样。

走 (甲) 的话，三个点要拿准，回执里都要回答：

- **新容器的 `/work` 是空的。** 之前几步写出来的文件没了，模型必须被明确告知 ——
  这句话要放进 `ToolOutcome` 的正文开头给模型看到，**不能只写进 `tracing`**。
  让模型以为文件还在，比直接报错更糟。
- **只重试一次。** 第二次还 `NotFound` 就照常收成 `code=sandbox`，别把
  `MAX_CONSECUTIVE_SANDBOX_ERRORS` 那道闸门架空。
- **替身要先改一处才测得动（核过）**：`core/crates/gateway/tests/common/mod.rs` 的
  `FakeGatewaySandbox`（297 行起，`impl SandboxPort` 在 391 行）目前的 `fail_exec`（313 行）
  是**黏的** —— `exec` 每次都读 `state.exec_error`（420 行）且不清除，所以「第一次 NotFound、
  第二次成功」这个时序造不出来。要么加一个 `fail_exec_once`，要么按 `exec_requests`
  （407–411 行记录 `(sandbox_id, req)`）断言两次 exec 的 **sandbox_id 不同**（`fake-sbx-1` → `fake-sbx-2`）。
  现成的 `NotFound` 用例在 `core/crates/gateway/tests/errors.rs:237–246`，照它起手最快。

**最后，你要在回执里正面回答一句：走完 (甲) 之后，`ReapIdle` 那个 `released` 返回值还有存在的必要吗？**
没有就说没有（并说明为什么不删它 —— 它在 `proto/**` 里，proto 冻结）；有就说它还剩什么用。

---

## 纪律

1. **契约与锁**：`proto/**`、`core/crates/contracts/**`、`.contracts.lock` 冻结，全程 `OK 25 files`。
   要动 → **停下报告**。③的 `ACTIVE_TASK_STATUSES`、④的 `ToolGateway` trait 都在这条线后面。
2. `evals/p0/*.yaml` 十个场景是验收面，**一个字不动**。
3. `docs/dev-spec-2026-09-09.md` 与 `docs/dev-spec-2026-09-11-rustgo.md` 冻结。
4. 不 panic；不在 async 里阻塞；**测试不靠真实 sleep**，也不靠墙钟阈值当判据 ——
   上一轮刚修掉一条「两万个 tick 必须 1 秒内跑完」的假红门禁（台账 §2.5），别再造。
   ②那条并发窗口的测试尤其容易踩：**用注入的时钟 / 显式的放行开关，不要用 `sleep(50ms)` 赌时序。**
5. `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check`、`go vet`、
   `gofmt`、`go test -race` 全干净。**`-race` 是硬门禁**（合流时它抓出过一处测试辅助的竞态）。
   注意：**`cargo fmt` 的写模式会被守卫拦**（它会顺带格式化 `core/crates/contracts`），用 `--check`。
6. 密钥只从配置点名的环境变量读，任何日志 / 错误 / Debug 输出不得出现取值。
7. **每条结论挂实测。**「应该会」「大概」一句不要。改了测试的，要能说出「把被测行为破坏掉，
   这条会不会红」，并把**破坏 → 红、还原 → 绿**两次输出贴出来。①②③④ 四条都要有这两份输出。
8. **不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v5` 分支上，回执贴出来。
9. 六轨同时在跑：cargo 构建锁与 Docker daemon 是全机共享的，你的命令可能要排队等几分钟，
   这是正常的，别以为卡死。
10. **守卫会扫整条 bash 命令字符串，heredoc 正文也算。** 写 `.md` / `.rs` 这类含复杂引号的文件时
    直接用 Write 工具，别走 `cat > x <<'EOF'`（shlex 不认 heredoc，引号一配不平整条命令被拦）；
    命令里也别出现 `docs/dev-spec-` / `core/crates/contracts/` 这类受保护路径的字面量，
    连只读的 `git diff -- core/crates/contracts/` 都会被拦。被拦不是事故，改写命令即可，
    但要在回复里如实报一句。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v5
scripts/check.sh                                 # 期望最后一行「全部通过」，退出码 0
```

关键行一条都不许变小：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files`（**不许变**） |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=` **≥ 718**、`failed=0`（你会往上加，说清加了几条） |
| B 全量 go test（-race） | 八个包全 `ok` |
| B8 评测 | `passed 10/10` |

分包再各跑一遍（改哪个跑哪个，输出贴回执）：

```bash
cd core && cargo test -p aite --test graceful_shutdown        # ② 的主场
cd core && cargo test -p aite-control                         # ②③
cd core && cargo test -p aite-gateway                         # ④
cd core && cargo test -p aite-edge-client                     # ①
cd core && cargo test -p aite-worker                          # ③ 如果动了 agent.rs
cd core && cargo clippy --workspace --all-targets -- -D warnings
cd core && cargo fmt --check
```

沙箱那一档（并行时可能假红，判据见开场自检的提示）：

```bash
cd edge && go test -tags docker ./internal/sandbox/... -count=1   # 期望 ok
docker ps -a --filter label=aite.task -q | wc -l                  # 期望 0
```

①如果做到了第 2 步（比对下沉），补一条真机口径的实测：

```bash
# edge 不起，直接起 core：4 秒后应看到 aite.edge_unreachable；
# 然后把 edge 起起来，看你新增的「连上就比一次」有没有真的出现在日志里
core/target/debug/aite run --config config/aite.yaml
```

## 回执格式

```
## V5 回执

基线 0bc8d55 → 提交 <短 sha>（分支 task-v5）

### 开场自检
$ scripts/check.sh
<最后 3 行>
契约锁 / C1 / B cargo / B go / B8 五行：<逐行贴>
Read guard_bash.py：<被拦了 / 没被拦>

### 行号核对
派单里点名的 file:line 有对不上的吗：<没有 / 逐条列出：写的是什么、实际是什么>

### ① contract_version 门禁
病是怎么复现的（甲/乙哪条）：<贴实测输出>
做到第几步：<1 / 1+2 / 1+2+3>
测试落在哪个 crate、为什么；动了哪个 Cargo.toml：<>
不一致之后的处置选了 (a)/(b)/(c) 哪条、为什么；(b) 的话 latch 怎么自愈的：<>
破坏 → 红 / 还原 → 绿：
<两份输出>
「edge 没起白等 4s、而且排在模型配置校验之前」这条代价：<还在 / 顺手治了，怎么治的>

### ② 收尾并发窗口
方向甲（漏抄新任务）、方向乙（把已交付的改写成 cancelled）各自的复现输出：
<>
改法选了 (甲)/(乙)/(丙) 哪条（可组合）、为什么：<>
如果选了 (甲)：这是对 C-TΩ-1 冻结收尾序列的改动。run.rs:8 的注释与
review/inventory-core.md:272 改成了什么：<>
破坏 → 红 / 还原 → 绿：
<两份输出>

### ③ !status 与 Answering
台账那三处说宽了的地方，你复核的结果：<>
`!status` 走的口径确认：<>
Python 原版行为对拍结果：<>
「全仓没有一条测试钉 deliver() 落 Answering」这条你复核成立吗：<>
改法选了 (甲)/(乙)/(丙) 哪条、为什么：<>
如果动了锁：取锁顺序守住 plane.rs:141 那条全局序了吗：<怎么证的>
破坏 → 红 / 还原 → 绿：
<两份输出>

### ④ ReapIdle 的 released
症状复现：<贴实测输出>
改法选了 (甲)/(乙)/(丙) 哪条、为什么：<>
如果选了 (甲)：
  新容器 /work 是空的这件事怎么告诉模型的（贴那句话的原文）：<>
  只重试一次、MAX_CONSECUTIVE_SANDBOX_ERRORS 那道闸门还在：<怎么证的>
  FakeGatewaySandbox 的黏性 fail_exec 你怎么绕的：<加了 fail_exec_once / 按 exec_requests 断言>
正面回答：走完这条之后 ReapIdle 的 released 返回值还有必要吗：<>
破坏 → 红 / 还原 → 绿：
<两份输出>

### 实测输出（粘实际的）
$ scripts/check.sh
<最后 3 行 + 五个关键行>
$ cd core && cargo test -p aite --test graceful_shutdown
<test result 那行>
$ cd core && cargo test -p aite-gateway
<test result 那行>
$ cd core && cargo test -p aite-edge-client
<test result 那行>
$ cd edge && go test -tags docker ./internal/sandbox/... -count=1
<>
$ docker ps -a --filter label=aite.task -q | wc -l
<>
新增测试 <N> 条，cargo passed 从 718 → <>

### 碰到冻结面了吗
<没有就写"没有"；有的话逐条写：碰的是哪个面、为什么、停下来了没有>

### 被守卫拦过吗
<没有就写"没有"；有的话写：拦的哪一条、命令改成了什么>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v5` 分支上，回执贴出来。
