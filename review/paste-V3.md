# 任务 V3 — 真机验收前的最后一公里：把 M1–M6 剧本和演示分镜走到「照着敲不会卡」

## 背景：这轨是从哪来的

`docs/dev-spec-2026-09-09.md`（P0 的权威文档）里还没兑现的只剩两件事：§2.4 的
**M1–M6 真机验收**（真实飞书测试群，总管亲自跑）和 §1 那句「**能录一段 3 分钟演示**」。
两件都不是写代码能交的活 —— 它们的交付形态是「总管抱着一份文档坐到飞书前面，
照着敲，一路敲到底不卡壳」。

问题是：`docs/acceptance-M.md`（M1–M6 剧本，514 行）和 `docs/demo-3min.md`（分镜，665 行）
**都是 Python 时代写的**。RΩ 合并时把命令口径换成了 Rust + Go 两进程，但那是一次
「换敲什么命令」的批量替换 —— **没有任何人照着这两份文档从头跑过一遍**。
就在这一轮 34 个 agent 的审核里，已经抓出两处「文档说的和代码干的不是一回事」
（卡片按钮、证据路径，见 `review/review-findings-2026-09-12-romega.md` §2.5 最后一行），
而那两处是顺手撞见的，不是系统查出来的。

本轨写派单时又核出四类新的对不上，全部有 file:line（详见下面 §要做什么）：
起飞命令会让两个进程**永远连不上**、三个日志关键字**代码里根本不存在**、
「回过来的是一条纯文本」这句**在协议层是假的**、`edge/bin/aite-edge` **不是 `make build` 的产物**。
其中第一条和第三条，一条会让总管在起飞那一步就卡死，一条会让他对着镜头念出一句能被当场证伪的话。

**这一轨的定位一句话**：文档里每一条会让总管卡住的地方，现在就把它卡在你这儿，
别留到他坐到飞书前面。§1 明令「禁止任何『顺手做一点 P1』的行为」——
所以这一轨只碰三个 `.md`，一行代码都不改；撞到真缺陷就停下、写进回执转给对应的轨。

---

## 必读（按顺序，别跳）

1. **`review/review-findings-2026-09-12-romega.md`** —— RΩ 合并前的审核台账。
   重点读 **§2.4**（卡片提示那条 high 的完整因果链，你要把它的结论贯穿全文）、
   **§2.5 最后一行**（RΩ 对这两份文档做了什么，你接着它改）、
   **§四「记账给下一批」**（4.2 里那条 `TaskStatus::Answering` 与 `!status` 的账，
   你要决定文档里提不提）、**§六**（守卫静默失效那条，你的开场自检就是为它准备的）。
2. **`docs/acceptance-M.md` 全文** —— 你的主战场之一。读的时候手边开着代码，
   每一条命令都当成「它可能是假的」来读。
3. **`docs/demo-3min.md` 全文** —— 你的主战场之二。它和 acceptance-M.md 的分工写在抬头：
   前者答「对不对」，后者答「凭什么值钱」。**两边有重叠的地方，demo 一律引它不抄它** ——
   改的时候别把这条分工改掉。
4. **`docs/dev-spec-2026-09-09.md` §1（明确不做）、§2.4（M1–M6 那张表）、§3.7（两条待核实）**
   —— 这三节是你两份文档的上游。**它冻结，一个字不动**；你的产出是把 §2.4 那张
   「操作 → 期望」的表变成能照着做的东西，把 §3.7 那两个问号变成两个实验步骤。
5. **`review/paste-ROMEGA.md`** —— 上一轮的派单，看它的语气和「每条结论挂实测」的写法。
   你写进文档的每一段输出也照这个规矩：真跑出来的才贴。

顺带扫一眼 `review/review-findings-2026-09-11.md`（七轨合流的台账），
`docs/acceptance-M.md` §8 那七条观测缺口就是从它那一轮记下来的。

---

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-v3
分支     : task-v3
基线     : 0bc8d55（main 的 HEAD，RΩ 组装起飞刚合入）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、go 1.27.1、protoc 36.1、Docker 29.6.1
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。

**worktree 里没有 `config/aite.yaml`**（`.gitignore:10` 不入库，它只在主仓里）。
你要跑 `aite preflight` / `aite evidence show` 就先：

```bash
cp config/aite.example.yaml config/aite.yaml     # 例子里全是契约默认值，一个取值都没有
```

**不许**把任何真实取值写进它（密钥只走环境变量，见纪律 6）。

---


> **基线说明**：`0bc8d55` = RΩ 合入 main 那次（`11322b3`）**再往前一格**。
> 那一格只改了两个文件：`scripts/check.sh`（B 全量 cargo test 那步改成「失败测试名在前、计数在后」）
> 与 `review/review-findings-2026-09-12-romega.md`（收窄 Answering 那条 + 补记一次未复现的 717/1）。
> `git diff --stat 11322b3..0bc8d55` → `2 files changed, 11 insertions(+), 2 deletions(-)`。
> 本派单正文里凡是写「在 `11322b3` 上核过 / 实测」的，指的是核对当时那一格，**代码面与你的基线逐字相同**。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v3
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

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
> （2026-09-12 总管这边真踩过，台账 §六 记了：会话在仓库子目录里起，`CLAUDE_PROJECT_DIR`
> 就定死在那个子目录，hook 命令 `python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"`
> 找不到文件 → 执行失败 → **非阻塞放行、不报警** → 整个会话守卫静默失效。）
> 守卫脚本自己写着「守卫脚本与本配置不允许 Read」（`guard_bash.py:9`），所以**被拦才是对的**。

---

## 可写路径

| 面 | 权限 |
|---|---|
| `docs/acceptance-M.md` | ✅ **自由写**（本轨主战场之一） |
| `docs/demo-3min.md` | ✅ **自由写**（本轨主战场之二） |
| `README.md` | ✅ **归你收口**。V1 会在回执里提「README 的 CI 那一节与 `ci.yml` 口径对不上」，V6 可能提守卫相关的说明 —— **三轨的改动都由你落**，别让三份各写各的 |
| `config/aite.yaml` | ✅ 可建可改，但 `.gitignore:10` 不入库，**不许写任何取值** |
| `data/**` | ✅ 你自己跑命令产生的落盘（`.gitignore:7` 已忽略） |
| `docs/dev-spec-2026-09-09.md`、`docs/dev-spec-2026-09-11-rustgo.md` | ❌ **冻结，一个字不动**。§3.7 的结论写进你那两份文档，**不要回写 spec**（守卫也拦着，见纪律 7） |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock` | ❌ 契约冻结，全程 `OK 25 files`。碰到了说明你走错地方了 → 停下报告 |
| `evals/p0/*.yaml` | ❌ 十个场景是验收面，一个字不动 |
| `core/**`、`edge/**` 的任何代码 | ❌ **一行都不改。** 下面 ③ 那条会让你很想去改 `SendText`，别改 —— 写进回执转给总管路由 |
| `docker-compose.yml`、`.github/workflows/ci.yml`、`Makefile` | ❌ **归 V1**。你只读它们，用来校准 README 和 runbook 的说法 |
| `.claude/**` | ❌ **归 V6**，而且守卫本身就拦着 |
| 仓库根的 `dev-spec-2026-09-09.md`（未入库副本） | ❌ 别动。见 ⑥ 最后一条 |

---

## 要做什么

六条，按顺序做 —— ① 是后面每一条的地基（起不来就什么都验不了），⑥ 最后收口。

---

### ① 起飞 runbook：现在照文档敲，两个进程永远连不上

**病在哪。** 三处文档教人这么起 edge：

| 位置 | 原文 |
|---|---|
| `docs/acceptance-M.md:44` | `cd edge && go run ./cmd/aite-edge --config ../config/aite.yaml` |
| `docs/demo-3min.md:30` | `cd edge && go run ./cmd/aite-edge --config ../config/aite.yaml` |
| `README.md:56` | `edge/bin/aite-edge --config config/aite.yaml &   # 或 cd edge && go run ./cmd/aite-edge` |

`cd edge` 之后进程的 cwd 是 `edge/`，而 **config 里所有相对路径按契约都相对仓库根**：

- `edge/cmd/aite-edge/main.go:72` —— flag 的帮助文本自己写着「配置文件路径（**相对仓库根**）」；
- `edge/cmd/aite-edge/main.go:102` —— `server.ListenUnix(cfg.Edge.EdgeSocket)`，
  收到的是 `data/run/aite-edge.sock` 这个**裸相对路径**，没有任何仓库根解析；
- `edge/internal/server/server.go:21-24` —— `ListenUnix` 第一件事是
  `os.MkdirAll(filepath.Dir(path), 0o755)`，**把错路径静默建出来**，不报错；
- core 那一侧相反：`core/crates/app/src/app.rs:137-141` 取 `std::env::current_dir()` 当 `repo_root`，
  `core/crates/edge-client/src/lib.rs:96-101` 的 `resolve()` 把 socket 落到 `repo_root/data/run/`。

于是 edge 监听 `edge/data/run/aite-edge.sock`，core 去连 `data/run/aite-edge.sock` ——
**两个进程各拿各的 socket，永远连不上**。

**这条不是新发现，是 RΩ 已经在 compose 上修过一遍的同一个坑。**
`docker-compose.yml:38-42` 的抬头注释第 2 条把这个教训写死在文件里了：

> `2. 两个进程都**以 /app（仓库根）为 cwd** exec。…… edge 会把 socket 建到
> /app/edge/data/run/（那儿不是共享卷），两个进程永远连不上。`

compose 修了，**人手起飞的 runbook 没修**。

**怎么验证它确实病着**（三条，都不要真凭证）：

```bash
cp config/aite.example.yaml config/aite.yaml
# 终端 A：照文档原样起 edge
cd edge && go run ./cmd/aite-edge --config ../config/aite.yaml
# 终端 B：照文档原样起 core
core/target/debug/aite run --config config/aite.yaml
```

- 看 core 的日志：应该出现 `aite.edge_unreachable`（`core/crates/app/src/app.rs:329-336`，
  5 次尝试、每次之间 sleep 1s，见 `app.rs:38` 的 `EDGE_STATUS_ATTEMPTS = 5`）；
- `ls edge/data/run/` —— socket 建在这儿了；
- `git status --short` —— **`edge/data/` 会冒出来**：`.gitignore` 只忽略了根上的 `/data/`（:7）
  和 `/edge/bin/`（:17），**没有 `edge/data/`**。这是这条病最便宜的判据。

（`aite.edge_unreachable` 之后 core 会**照常起飞**（§2.1 启动顺序无关），
所以它不会报错退出 —— 它会看起来一切正常，然后每一条群消息都石沉大海。这正是最难查的那种。）

**顺带还有一条：`edge/bin/aite-edge` 不是 `make build` 的产物。**
`Makefile:18-20` 的 build 是 `cd edge && go build ./...` —— Go 在包列表多于一个时
**丢弃产物、只当编译检查**。实测：主仓 `make build` 跑过无数遍，`edge/bin/` 至今不存在
（`ls edge/bin` → No such file or directory），而 `Makefile:55` 的 clean 又写着 `rm -rf edge/bin`。
compose 里正确的写法在 `docker-compose.yml:59` —— `go build -o /usr/local/bin/aite-edge ./cmd/aite-edge`。

**改法方向。** 把三份文档的起飞段统一成一份 runbook，要求：

1. **两个进程都在仓库根跑**，把「为什么」用一句话写进去（不写理由，下一个人还会 `cd edge`）；
2. 给出 edge 二进制的**真实**编法：`cd edge && go build -o bin/aite-edge ./cmd/aite-edge`
   （`edge/bin/` 在 `.gitignore:17`，不入库），然后回到仓库根跑 `edge/bin/aite-edge --config config/aite.yaml`；
3. `go run` 那条路要留就留，但必须写成在仓库根跑的形式；
4. `acceptance-M.md` 的「起飞」段（§0.2）与 `demo-3min.md` §0.1 **逐字对齐**
   —— 现在这两处已经不一致（demo 那份没提 `edge/bin`），别再留两套说法。
5. **compose 那一档写成占位**：V1 正在做真镜像 compose，`docker-compose.yml` 归它。
   你要在 runbook 里留出「compose 起」的位置（读日志用 `docker compose logs -f`、
   socket 在命名卷 `run:` 里宿主机看不到、`./:/app` 是 bind mount 所以
   `data/evidence` 在宿主机上直接看得到 —— 这几条现在成立），
   但**不要替 V1 决定 compose 长什么样**，也不要抄现在这份 compose 的命令行。
   在回执里点名「compose 那一档是占位，等 V1 回执后由总管合」。

---

### ② 逐条走查：每条命令都要「今天还存在、输出形状对得上、路径还在」

这一轨绝大部分不需要真实飞书凭证。文档里每一条命令，都去核三件事：
① 这个子命令/参数今天还在不在（`aite --help` 及各子命令）；② 输出形状与文档写的对不对得上；
③ 引用的文件路径还在不在（Python 树删了，`scripts/*.py` 全没了，`scripts/` 现在只剩 `check.sh`）。
**能本机跑的就真跑一遍，把实际输出贴进文档**。

下面这张表是本轨写派单时已经核过的，**逐条已在 11322b3 上实测**。
标 ❌ 的是文档错了要改；标 ✅ 的是文档对的，你复核一遍就行，别推倒重来。

| # | 文档位置 | 文档怎么说 | 实际 | 判 |
|---|---|---|---|---|
| a | `acceptance-M.md:82` | `--config 指到真配置，花费一栏才算得出来` | `--config` 的默认值就是 `config/aite.yaml`（`core/crates/evidence/src/cli.rs:1318-1320` `default_value`）。在仓库根跑根本不用带 | ❌ 改成「不在仓库根跑时才要 `--config`」 |
| b | `acceptance-M.md:63` | `--traceback`：起不来时打完整错误链 | 实际只是 `eprintln!("{e:?}")` 把同一句话用 Debug 再包一层引号（`core/crates/app/src/cli.rs:104-106`；台账 §4.4「Rust横切」那条）。**V6 ④c 正在改** | ❌ 软化成实际行为，并注明 V6 在改、合流后复核 |
| c | `acceptance-M.md` §0.2 / `demo-3min.md` | 未出现 `aite run --help` | 幸好没出现。**它今天打不出真选项**：`aite run --help` 被 clap 截胡，只打 `[ARGS]...`（实测退出码 0）；唯一能打出手写 USAGE 的写法是 `aite run -- --help`，走 stderr + **退出码 2**。`aite preflight -- --help` 反而是退出码 0 | ⚠️ **别在文档里写 `aite run --help`**。V6 ④b 在改这条，改法未定 |
| d | `demo-3min.md:578` | `demo_fixture.py 生成的 8 条里，第 4 条和第 7 条…` | `scripts/demo_fixture.py` 随 Python 树删了。等价物是 `aite evals demo-fixture history`（`core/crates/evals/src/demo_fixture.rs`） | ❌ 改掉这个文件名 |
| e | `demo-3min.md` §1④ | `demo-fixture all` 打出「最高 2025-11 140,494 元 / 最低 2025-07 60,700 元」 | **逐字一致**，实测：<br>`CSV      /tmp/aite-demo/sales.csv`<br>`  区间   2024-09 … 2026-08（24 个月）`<br>`  最高   2025-11  140,494 元`<br>`  最低   2025-07  60,700 元`<br>`  合计   2,491,172 元`<br>`群历史   /tmp/aite-demo/history.txt（8 条，其中 2 条是干扰项）` | ✅ 把完整六行贴进文档 |
| f | `README.md:99` | `make lock  # A3/C2 契约锁 --check` | `Makefile:32` 是 `lock c2: build`，真有这个 target | ✅ |
| g | `acceptance-M.md:26-29` | preflight 三条命令 | 全在。`aite preflight -- --help` 实测列出 `--config / --offline / --json / --chat-id` 四个参数，与文档一致 | ✅ 但把 §0.1 的期望换成**真输出**（下面 ④ 给了原文） |
| h | `acceptance-M.md` §0.3 / `demo-3min.md` §4.5 | `aite evidence show --list` / `<task_id>` / `--only` / `--tail` / `--json` | 全在（`cli.rs:1306-1335`）。`--list --json` 的 `.tasks[0].task_id` 也对（`cli.rs:1128-1138`），`demo-3min.md` §1⑨ 那个 `last()` 函数能用 | ✅ |
| i | `acceptance-M.md` §0.4 | 「输出**开头第二行** `目录 …`」 | `render_text` 确实是 `任务 …` / `目录 …` 两行开头（`cli.rs:976-977`），**但前面可能先插一行 `提示：…`**（`cli.rs:1279`、`1504-1508`）—— 配置读不到或单价为 0 时就会有 | ❌ 改成「开头那几行里的 `目录 …` 那一行」，别数第几行 |

**日志关键字这一块单列，错得最多（`acceptance-M.md` §7 那张速查表和 M2/M3/M6 都在用它）：**

| # | 文档位置 | 文档写的 | 代码里的真名 | 判 |
|---|---|---|---|---|
| j | `acceptance-M.md:260`、`:477` | `control.gateway_release_failed` / `worker.gateway_release_failed` | **这两个名字全仓不存在（各 0 处）**。真名是三个：`control.release_failed`（`core/crates/control/src/plane.rs:1130`）、`gateway.release_failed`（`core/crates/gateway/src/gateway.rs:385`）、`preflight.release_failed`（`core/crates/app/src/wiring.rs:208`） | ❌ 两处都要改 |
| k | `acceptance-M.md:409` | 只重启 edge 时 core 日志出现 `edge.reconnecting` → `edge.reconnected` | **`edge.reconnected` 不存在。** `core/crates/edge-client/src/link.rs:9` 的注释就写着只有三个名字：`edge.connecting` / `edge.connected` / `edge.reconnecting`（:143 / :150）。重连**成功**打的是 `edge.connected` | ❌ 改成 `edge.reconnecting` → `edge.connected`，并说明「首次连上和重连成功用的是同一个名字」 |
| l | `acceptance-M.md:170`、`:469` | `feishu.reconnecting attempt=N delay=Ns` / `feishu.reconnected after=N attempts` | 字段名是 `delay_sec` 不是 `delay`（`edge/internal/feishu/platform.go:251`）；成功那条是 `feishu.reconnected after=N`（:271）。而且是 Go slog 的 logfmt，形状与 Rust 那边的 tracing 不一样 | ❌ 按真字段名改 |
| m | `acceptance-M.md` §7 整张表 | 没写任何一条属于哪个进程 | `feishu.*` 和 `ingress.reconnecting/reconnected` 在 **edge（Go）** 那个终端；`control.*` / `worker.* `/ `aite.*` / `ingress.slow_callback` 在 **core（Rust）** 那个终端。`ingress.handle_failed` **两边都有**（`core/crates/control/src/ingress.rs:57` 与 `edge/internal/ingress/client.go:180`） | ❌ **给 §7 的表加一列「哪个进程」**。这条不是小事：`acceptance-M.md` §0.3「三个观察窗」把日志窗写成「`aite run` 那个终端」，而 **M2 整条要看的 `feishu.*` 全在 edge 那边** —— 照现在的文档做，M2 会盯着一个永远不出现那两行的窗口 |
| n | `acceptance-M.md` §7 末 | 计数器「目前没有对外查看入口」 | 对 core 侧成立（`plane.rs:1193` 的 `counters()` 没有 CLI 出口，`!status` 只回活跃任务列表，`plane.rs:458-481`）。**对 edge 侧不成立了**：`edge/cmd/aite-edge/main.go:196-199` 在退出时打一行 `edge.counters events.sent=… ingress.invalid=… ingress.errors=… ingress.reconnects=…` | ❌ 分两半写清 |
| o | `acceptance-M.md:469` 那张表 | 缺 `feishu.connect_failed` | 它存在且有测试钉着（`edge/internal/feishu/reconnect_test.go:257`），WARN 级 | ⚠️ 补一行 |

**这几条是对的，别顺手改坏：**

- reaper 60 秒一轮（`core/crates/control/src/plane.rs:28` `REAPER_INTERVAL_SEC = 60.0`）+
  `idle_sec` 默认 300 → M3 那句「最坏 300+60=360 秒、建议等满 6 分钟」成立；
- `run_python` 用请求里的 `timeout_sec`、其余工具默认 60s
  （`core/crates/gateway/src/gateway.rs:221-227`、`DEFAULT_TOOL_TIMEOUT_SEC`）；
- 群历史格式 `[message_id] 姓名: 文本`、只留真人（`core/crates/worker/src/context.rs:70-82`）；
- 提示词确实要求引用时带 `[message_id]`（`core/crates/worker/prompts/platform.md:54`）；
- `aite.up` 那一行确实有六个字段：platform / model / sandbox / sqlite / evidence / edge
  （`core/crates/app/src/run.rs:153-165`）；
- `!stop` 只有一个活跃任务时可以省略任务号，`#A17` / `a17` 都收
  （`core/crates/control/src/plane.rs:568-583` + `normalize_task_no`）。

---

### ③ 「一条纯文本回复（不是卡片）」—— 这句在协议层是假的

**病在哪。** 五处文档这么写：

| 位置 | 原文 |
|---|---|
| `acceptance-M.md:132` | 随后线程里出现一条**纯文本**回复（不是卡片） |
| `acceptance-M.md:217` | 线程里新增的消息是 **1 张卡片 + N 个产物文件 + 1 条文本** |
| `demo-3min.md:248` | 随后一条**纯文本**回复 |
| `demo-3min.md:300` | 话题里一条**纯文本**回复。**没有卡片** |
| `demo-3min.md:310` | （旁白）「回过来的是一条**纯文本**，没有卡片。」 |

实际上 **core 发的每一条文本，到飞书都是一张卡片**：

```go
// edge/internal/feishu/platform.go:379-390
func (p *Platform) SendText(ctx context.Context, msg *pb.OutboundText) (*pb.SendResult, error) {
    ...
    content := DumpsCard(BuildMarkdownCard(msg.GetText()))
    messageID, err := p.sendMessage(ctx, msg.GetChatId(), msg.ReplyTo, "interactive", content, msg.GetInThread())
```

`msg_type` 传的是 `"interactive"`，也就是交互式卡片。理由写在
`edge/internal/feishu/cards.go:239-243`：`OutboundText.text` 的契约是 markdown，
而「飞书里唯一真能渲染 markdown 的载体就是卡片的 markdown 元素 —— `msg_type=text`
会把 `**粗体**`、列表、链接原样当字面量吐出来」。这是**故意的、正确的实现**，
错的是文档。

**为什么这条要认真对待。** `demo-3min.md:310` 那句是**要对着镜头念出来的旁白**，
而 §4.2 把它列进了「两个点，都必须说到」。观众看到的如果是一个带边框的卡片气泡，
这句话当场就被画面证伪 —— 而这场演示的全部价值是「前面三分钟全是真的」（§4.6 收尾那句）。

**边界在哪，别改过头。** 文档想说的那半是对的：**Answering 路径不发 checklist 卡片**
（W3，第一步就 `final` 时不走 `send_card`）。所以要区分的是两件事：

- ✅ 「没有 checklist 进度卡片」—— 真的，这是 W3 的行为；
- ❌ 「不是卡片消息 / 是纯文本」—— 假的，协议层就是 `interactive`。

**怎么核。** 协议层已经核死了（上面那段代码）。**视觉层只能在真飞书里看一眼**：
一个只有 markdown 元素、没有 header 的卡片，在飞书里长什么样、和普通文本气泡差多少。
**这一条必须真机核**，核完再定文案 —— 你手上没凭证就把两种写法都留在文档里、
并在回执里点名「文案二选一，等总管现场看一眼再定」。

**别去改 `SendText`。** `edge/**` 不在你的可写面上，而且改它会破坏 markdown 渲染
（那是 R6 有意为之）。这一条的产出只有文案。

---

### ④ 把 §3.7 的两条待核实做成剧本里的两个实验步骤

`dev-spec-2026-09-09.md` §3.7 留了两个问号，原文：

> 待核实两点：(a) 只有 @ 权限时，话题里不带 @ 的回复是否投递；
> (b)「获取会话历史消息」API 是否要求「获取群组中所有消息」敏感权限。

RΩ 已经把这两条接进 `aite preflight` 了，会在第 4 组之后打两行 NOTE
（`core/crates/app/src/preflight.rs:76-77` 的 `NOTES_AFTER`、`:424-444` 两个 Note）。
本轨在 11322b3 上实测的**完整真输出**（`config/aite.yaml` 由 example 复制而来，
主仓上只设了 `AITE_MODEL_API_KEY`）：

```
$ core/target/debug/aite preflight --offline
Aite 起飞前自检
  配置：config/aite.yaml
  模式：--offline（只跑 1/2/7）
  时间：2026-09-12T15:50:21+08:00

[1/7] OK   配置可加载       platform=feishu · model.provider=openai_compat · sandbox.image=aite-sandbox:p0
[2/7] WARN 环境变量齐       3/4 个未设置：FEISHU_APP_ID FEISHU_APP_SECRET FEISHU_BOT_OPEN_ID（已设置：AITE_MODEL_API_KEY） —— --offline 下不作判据
           └ 怎么补：真机起飞前去掉 --offline 重跑一次，这几项必须是 OK
[3/7] SKIP 飞书凭证有效     --offline：不碰网络
[4/7] SKIP 飞书身份对得上   --offline：不碰网络
       NOTE §3.7 待核实      §3.7(a) 只有 @ 权限时话题内不带 @ 的回复是否投递 —— 未核实，且起飞前查不了：要真在话题里发一条不带 @ 的消息、看事件有没有投递才知道，M4 就是那个实验。当前 FEISHU_P0.supports_passive_listen=False（保守取值），M4 请带 @ 先走通。
       NOTE §3.7 待核实      §3.7(b) 群历史是否要「获取群组中所有消息」敏感权限 —— 未核实。飞书没有「列出本应用已授权范围」的免权限接口，光靠凭证问不出来；带 --chat-id <测试群 chat_id> 重跑，我就直接调一次群历史给你结论（M5 靠它）。
[5/7] SKIP 模型端点通       --offline：不碰网络
[6/7] SKIP 沙箱可用         --offline：不碰 docker
[7/7] OK   落盘目录可写     3 个路径都落得下去（data data/evidence data/artifacts 待建，起飞时自动 mkdir）：data/aite.db · data/evidence · data/artifacts

------------------------------------------------------------------------
汇总：OK 2 · WARN 1 · FAIL 0 · SKIP 4（共 7 项，过了 7 项）
全部没红，可以起飞。（--offline 只验了 1/2/7，真机起飞前请全跑一遍）
exit=0
```

**(b) 已经是可执行的了，你只要把它写成步骤。** 带 `--chat-id` 重跑时
preflight 会**真调一次群历史**（`preflight.rs:746-800` `probe_history_scope`，
`GET .../im/v1/messages?container_id_type=chat&container_id=<chat_id>&page_size=1`），
两种结局的原文已经写死在代码里，**逐字抄进剧本**：

- 读得到 → `§3.7(b) 实测：这套凭证**读得到**群历史（chat 返回 N 条，只取了 1 页 1 条）。
  也就是说当前已授予的权限足够 M5；至于是不是「获取群组中所有消息」那条在起作用，
  接口不回权限来源，问不出来 —— 但对起飞而言结论已经够用。`
- 读不到 → `§3.7(b) 实测：这套凭证读**不到**群历史（code=… msg=…）。去开放平台补权限
  （读取群历史消息；若提示需要「获取群组中所有消息」则它就是必需的敏感权限，要走审核），
  补完重跑本项。M5「汇总本群本周开放事项」在此之前一定过不了。`

剧本里要写清：**这一步排在 M1 之前跑**（60 秒的事，比在 M5 才发现权限没批便宜得多），
结论直接决定 **M5 演不演** 和 `demo-3min.md` §5 加演那段进不进 3 分钟版（§9 第 3 条问的就是它）。

**(a) 只能在群里做实验，而且判据现在缺一半。** 剧本 §M4（`acceptance-M.md:355-360`）
已经有雏形（先不带 @、30 秒没反应再补一条带 @ 的），要补的是**判「没投递」还是「投递了被丢」**：

- 进程侧现在**帮不上忙**：R8 丢弃事件时只 `bump("events.ignored")`
  （`core/crates/control/src/plane.rs:377-378`），INFO 级没有任何日志，计数器也没有查看入口
  （见 ② 的 n）。所以**唯一的判据是飞书开放平台的「事件订阅 → 推送记录」**。
- 现在这条写在 M4 的一个小节里、还带着「⚠️ 进程侧目前帮不上忙」的口吻。
  **把它提成明确的编号步骤**：第几步去哪个页面、查哪条消息、看到什么算「平台没投递」、
  看到什么算「投递了但进程丢了」（后者是 bug，要停下记录）。

**两条结论分别该怎么继续，写成表：**

| 结论 | 对 M4/M6 的影响 | 对演示的影响 | 要不要改代码 |
|---|---|---|---|
| (a) 不带 @ 也投递 | M4 第 1 步就成，M6 追问可以不带 @ | 第三幕可以不带 @（但 `demo-3min.md:411-415` 建议仍然带 @，理由是别在镜头前赌） | `supports_passive_listen` 该置 True —— **不是你的活**，写进回执转出去 |
| (a) 不带 @ 不投递 | M4/M6 全程带 @；群里的使用说明要写「话题里追问也要 @」 | 第三幕照现在的台词演 | 保持 False，无改动 |
| (b) 读得到 | M5 照跑 | 加演那段可选 | 无 |
| (b) 读不到 | M5 卡住，先去补权限 | 加演砍掉（`demo-3min.md:580` 已有这条应对） | 无 |

**结论只写进 `acceptance-M.md` 和 `demo-3min.md` + 回执，不要回写 `dev-spec-*.md`**
（§3.7 原文自己就写着「结果写在 dispatch 里，不改本文」，而且守卫拦着）。

---

### ⑤ 卡片没有按钮之后的替代路径，要贯穿全文而不是打补丁

RΩ 已经改了 `acceptance-M.md` §0.4 / §8 第 4 条和 `demo-3min.md` §6 三处，
但那是补丁 —— **M1–M6 各条里凡是暗示「点一下」的地方都要过一遍**。

**先把事实钉死（这几条本轨已核）：**

- 卡片上**一个按钮都不渲染**：`edge/internal/feishu/cards.go:120-140`。
  渲染那条路（`buildActions`）没删，SDK 放开钩子后改回去即可。
- 替代提示的**原文逐字**（`cards.go:183-192`，`no` 是 `#A17` 这样的任务号）：

  > `要停这个任务：在本话题里回复 !stop #A17，或在群里发「@我 !stop #A17」。（既不 @ 我、也不在本话题里的命令会被丢弃，不会有任何回应。）`

  **抄，不要转述。** `cards_test.go` 有一条断言钉着提示里必须出现「话题」或「@」
  （台账 §2.4：把文案改回旧说法这条测试变红）。文档里的说法和卡片上的说法不一致，
  用户会以为自己看错了。
- **终态卡片没有这行提示**：`cards.go:169` 注释 +`:183-192` 的循环 —— 只有 `actions`
  里含 `STOP` 时才加。所以「卡片底部有一行提示」这个说法要限定成「进行中的卡片」。
- 卡片一定在任务话题里：`SendCard` 的 `in_thread` 写死 `true`（`platform.go:394-396`），
  所以「在本话题里回复」这条路一定走得通。

**要过一遍的地方（至少这些，你自己再扫一遍全文）：**

- `acceptance-M.md` M3 的「在哪看 / 期望」里凡是提到卡片交互的；
- M4「不对时查哪」那张表里「点『回复』」——那是飞书的回复按钮，不是卡片按钮，
  **别误伤**，但要写清区别（现在混在一起读者会懵）；
- M6「顺手多验一条」里卡片被原地改成 failed 那段 —— 那是 core 主动 update，与按钮无关，✅；
- `demo-3min.md` §4.3 的信号表（`:344` 那行已经写了「卡片上一个按钮都没有」）✅，
  但 §4.5「第四幕」的引子说「`acceptance-M.md` 把 `evidence_show` 当排障工具用」——
  现在证据是**唯一**的看法，不只是排障工具，措辞要跟上；
- `demo-3min.md` §8「什么情况必须停下重录」那张表里有没有暗示按钮的。

**一条总管已经定了的（2026-09-12）：写，但要标可删。**

台账 §4.2 记了一条 —— `TaskStatus::Answering` 不在 `ACTIVE_TASK_STATUSES`
（= created/planning/working）里。**注意台账 2026-09-12 已经收窄过这条**：
`deliver()`（`agent.rs:643-648`）只在 `answering == true` 时置 `Answering`，
那是 **W3 那条路 —— 第一步就 `final`、没发过卡片的短任务**；
`answering == false`（发过卡片的正常任务）置的是 `Working`，仍在活跃集里。
`!status` 和 `!stop` 都走 `list_active_tasks`（`plane.rs:459`、`:573`），
所以口子只开在 W3 这一路：**这类短任务正在交付的那几秒，`!status` 查不到它、
`!stop` 会回「找不到该任务」**。

- 代码归 V5（`core/crates/control/**`），**你不改代码**。
- **文档要写**：写进 M3（或更贴切的那条 M）的「不对时查哪」一行，注明
  「这是已知记账（台账 §4.2），只影响第一步就 final 的短任务，不是环境问题，别去查沙箱」。
- 那一行**必须带一个显式标记**（例如 `<!-- 台账 §4.2；V5 修掉后删本行 -->`），
  这样 V5 真修掉时，总管合流一 grep 就能找到并删掉，不会留成过期文档。
- 回执里贴出你写的那一行原文。

---

### ⑥ README 收口（三轨的改动都落在你这儿）

**你自己已经能定的：**

| 位置 | 病 | 改法方向 |
|---|---|---|
| `README.md:56` | `edge/bin/aite-edge` 不是 `make build` 的产物；后面那句 `# 或 cd edge && go run ./cmd/aite-edge` 有 ① 的 cwd 坑，而且不带 `--config` 时默认找 `config/aite.yaml`（相对 `edge/`，根本不存在） | 照 ① 的 runbook 改，两处一起 |
| `README.md:90` | 「`--offline` …（不碰网络也不碰 docker，**CI 用这一档**）」 | **`ci.yml` 里没有任何 preflight 步骤**（实测：A1 build / A2 go build / A3 lock / A4 lint / A5 cargo test --no-run / C1 / B cargo test / B go test -race / B8 硬门禁 / compose config，十步，没有 preflight）。要么删掉「CI 用这一档」，要么等 V1 决定加不加那一步 —— **别自己去改 ci.yml**（归 V1） |
| `README.md:13` | 「三份 inventory 是 Python 树**唯一的存世记录**」 | 不准确：45KB 的旧 Python spec `docs/dev-spec-2026-09-09.md` 还在树里，而且它现在还是 P0 的权威文档（§2.4 / §3.7 就在里面）。措辞改准 |
| `README.md:185-189` CI 那一节 | 十步与 `ci.yml` 对得上（本轨核过） | ✅ 别动，除非 V1 改了 ci.yml |
| `README.md:191-205` 已知边界 | 卡片按钮那条与 `cards.go` 一致（本轨核过），乱序重推那条无对应改动 | ✅ 复核一遍即可 |

**要等别人的：**

- **V1** 会在回执里提「README 的 CI 那一节与 `ci.yml` 口径对不上」，还可能提 compose 真镜像
  之后「跑起来」那一段要怎么写。**等它的回执，由你落**；等不到就按当前 `ci.yml` 的实际步骤写，
  并在回执里点名「V1 若改了 ci.yml，README 这一段要跟着改，我没改」。
- **V6** 在做守卫相关的活（`.claude/hooks/**`、`cli_smoke.rs` 的测试接管），
  可能提出 README 要加一段守卫说明。同上：**它出文案，你落**。

**一条不归你、但要在回执里点名的：**

主仓（**不是 worktree**）根目录躺着一份未入库的 `dev-spec-2026-09-09.md`，
与 `docs/dev-spec-2026-09-09.md` **逐字节相同**（实测两份都是 45792 字节、`diff` 为空）。
同一份冻结 spec 有两份副本，改一份忘另一份是迟早的事。
主仓里还有 Python 树的构建残渣未清（`.venv` 202M、`aite/` 884K、`tests/` 2.9M、
`aite.egg-info/`、两处 `__pycache__/`，台账 §4.4「删Python」那条记的就是它）。
**这些都在主仓工作区、不在你的 worktree 里，你碰不到也不该碰** —— 回执里点名交总管。

---

## 纪律

1. **契约与锁**：`proto/**`、`core/crates/contracts/**`、`.contracts.lock` 冻结，
   全程 `OK 25 files`。要动 → 停下报告。
2. `evals/p0/*.yaml` 十个场景是验收面，**一个字不动**。
3. `docs/dev-spec-2026-09-09.md` 与 `docs/dev-spec-2026-09-11-rustgo.md` 冻结。
   §3.7 的结论写进你那两份文档，**不回写 spec**。
4. **一行代码都不改。** `core/**`、`edge/**`、`Makefile`、`docker-compose.yml`、
   `.github/workflows/ci.yml`、`.claude/**` 全不在你的面上。撞到真缺陷（③ 那条、
   ⑤ 那条 Answering、②c 的 `--help`）**停下，写进回执转给对应的轨**，
   别自己动手 —— 六轨同时在跑，你改一行别人就要处理冲突。
5. **每条结论挂实测。** 文档里贴的每一段输出都必须是你真跑出来的，不许照抄本派单里的示例
   再改几个字（本派单里的输出是 11322b3 上跑的，你重跑一遍确认还一样）。
   「应该会」「大概」「建议考虑」一句不要。
6. 密钥只从配置点名的环境变量读；`config/aite.yaml` 里只写变量名。
   **任何贴进文档或回执的输出，先自己扫一遍有没有取值** —— `docker compose config`
   会把 `${VAR}` 解析成取值（README:70-71 已经警告过），别往回执里贴它的完整输出。
7. **守卫会扫 bash 命令正文，写文档要绕开。** `guard_bash.py:175-190` 的 `scan_opaque`
   对命令文本做子串匹配：出现 `dev-spec-`、`.contracts.lock`、`go.mod`、`go.sum`、
   `proto/`、`core/crates/contracts/` 任何一个字样**就拦**，不管它在不在引号或 heredoc 里。
   而 `guard_bash.py:341-342` 明写 Edit / Write 的 **content 一律不看**。
   所以：**改这三份 `.md` 一律用 Write / Edit 工具**，别用 `cat > x.md <<EOF` 往里灌
   带 `dev-spec-` 的正文 —— 会被拦，而且拦得莫名其妙。
   （`grep -n 'dev-spec' docs/*.md` 这种也会被拦，用 Grep 工具。）
8. 不 panic、不引入新依赖 —— 你不写代码，这条对你只剩一个含义：
   **别为了「验证一下」去改任何测试**。
9. **不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v3` 分支上，回执贴出来。
10. 六轨同时在跑：cargo 构建锁与 Docker daemon 是全机共享的，你的命令可能要排队等几分钟，
    这是正常的，别以为卡死。

---

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v3

# 1. 起跑线没被你弄坏（你不改代码，这条应该原样绿）
scripts/check.sh                                                   # 期望 全部通过，退出码 0

# 2. 你写进文档的命令，本机能跑的都真跑过
cp config/aite.example.yaml config/aite.yaml
core/target/debug/aite preflight --offline                         # 期望 汇总：OK 2 · WARN 1 · FAIL 0 · SKIP 4，退出码 0
core/target/debug/aite preflight -- --help                         # 期望 四个参数都在，退出码 0
core/target/debug/aite evals demo-fixture all                      # 期望 最高 2025-11 140,494 元 / 最低 2025-07 60,700 元
core/target/debug/aite evidence show --list                        # 期望 证据根目录 …（N 个任务），退出码 0
core/target/debug/aite evidence show -- --help                     # 期望 --dir/--root/--config/--list/--only/--tail/--json 都在

# 3. 三条「不该再出现的字符串」全清了
grep -rn 'gateway_release_failed' docs/ README.md                  # 期望 0 行
grep -rn 'edge\.reconnected' docs/ README.md                       # 期望 0 行
grep -rn 'demo_fixture\.py' docs/ README.md                        # 期望 0 行

# 4. 起飞命令全都在仓库根跑（① 的判据）
grep -rn 'cd edge && go run' docs/ README.md                       # 期望 0 行
grep -rn 'go build -o bin/aite-edge' docs/ README.md               # 期望 ≥ 1 行

# 5. 冻结面一个字没动
git diff --stat 0bc8d55 -- docs/dev-spec-2026-09-09.md docs/dev-spec-2026-09-11-rustgo.md \
    proto core/crates/contracts .contracts.lock evals/p0            # 期望 空
git status --short                                                  # 期望只有 docs/acceptance-M.md、docs/demo-3min.md、README.md（config/aite.yaml 与 data/ 被 .gitignore 挡着）

# 6. ① 那条病的现场（做完 ① 之后照新 runbook 起，应该不再出现）
ls edge/data 2>&1                                                   # 期望 No such file or directory
```

**真机那一档**（要凭证，在你手上的话就跑；没有就在回执里写「没跑，缺凭证」）：

```bash
core/target/debug/aite preflight                                   # 七组全跑
core/target/debug/aite preflight --chat-id <测试群 chat_id>        # §3.7(b) 出结论
```

---

## 回执格式

```
## V3 回执

基线 0bc8d55 → 提交 <短 sha>

### 开场自检
$ scripts/check.sh
<最后 3 行；契约锁 / cargo / go / B8 四个数各一行>
Read .claude/hooks/guard_bash.py → <被拦 / 没被拦>   ← 没被拦就该停下，说明为什么还在跑

### ① 起飞 runbook
病的复现：<照旧文档起飞后 core 打的那行 + ls edge/data/run/ 的输出>
新 runbook 长什么样：<贴 acceptance-M.md §0.2 改后的原文>
demo-3min.md §0.1 与它对齐了没：<>
edge 二进制怎么编的（实测）：$ cd edge && go build -o bin/aite-edge ./cmd/aite-edge
                              $ ls -la edge/bin/aite-edge
compose 那一档：<占位怎么写的；要 V1 补什么>

### ② 逐条走查
| # | 文档位置 | 原文 | 实际 | 改成了什么 |
（a–o 逐条；派单里标 ✅ 的也要有一行说「复核过，成立」）
文档里出现的 aite 命令，我真跑过的清单：<逐条命令 + 退出码>
没跑的（为什么）：<>

### ③ 「纯文本回复」
协议层结论：<>
视觉层核了没：<真机看了 / 没凭证没看>
最后文案定成什么：<；两种写法都留的话说清等谁定>
要转给谁的：<>

### ④ §3.7 两条
(a) 写成了几步、判据是什么：<>
(b) 写成了几步、两种结局各自怎么继续：<>
真跑了没：$ aite preflight --chat-id …  → <输出 / 没凭证>
结论要不要改 supports_passive_listen：<转给谁>

### ⑤ 卡片替代路径
过了哪几处：<逐条>
Answering 那条（!status/!stop 查不到交付中的任务）：写进文档了没 → <①还是②> · 为什么

### ⑥ README
我自己定的三处：<逐条改了什么>
等 V1 的：<还差什么>
等 V6 的：<还差什么>
主仓根那份 dev-spec 副本 + Python 残渣：<原样交总管，我没碰>

### 验收命令的实际输出（粘实际的）
$ grep -rn 'gateway_release_failed' docs/ README.md      → <>
$ grep -rn 'edge\.reconnected' docs/ README.md           → <>
$ grep -rn 'demo_fixture\.py' docs/ README.md            → <>
$ grep -rn 'cd edge && go run' docs/ README.md           → <>
$ git status --short                                     → <>
$ git diff --stat 0bc8d55 -- <冻结面>                    → <期望空>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v3` 分支上，回执贴出来。
