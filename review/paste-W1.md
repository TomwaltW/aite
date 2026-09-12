# 任务 W1 — 合并引入的文档失真：把总管马上要照着敲的三份文档，修成合并后的真相

## 背景：这轨是从哪来的

V1–V6 六轨已经零冲突合进 `main`，`scripts/check.sh` 全绿（`cargo passed=793 failed=0`，
起跑线 718）。两份 spec 查下来，P0 没兑现的**只剩** `docs/dev-spec-2026-09-09.md` §2.4 /
`docs/dev-spec-2026-09-11-rustgo.md` §4.4 的 **M1–M6 真机验收**与**3 分钟演示** ——
那是总管自己的活，代码面已经没有拦着他的账了。

**所以「总管接下来就照着 `docs/acceptance-M.md` 和 `docs/demo-3min.md` 坐下来跑」是确定的。**

问题出在这儿：**六轨各自都对，合起来自相矛盾。** V3 那一轨把三份文档逐条跑了一遍、
改到「照着敲不会卡」，但它写的时候 V1 和 V5 还没并进来。合完之后：

- V5 修好的事，文档还在说它没修，而且**只修了一半**（下面 §A 是本轨最要紧的一段）；
- V1 换掉的 CI 和 compose 形态，文档还停在换之前；
- V4 那份 529 行的实测报告，全仓只有它自己引用自己。

**这些失真的危险性比它们的严重度高。** 它们不会让任何测试变红 —— 门禁全绿 ——
但总管照着排障表走，会被引到错误的结论上去；照着容器段敲，起不来。

> **这一轨不改任何代码。** 撞见代码侧问题就记账转出去（写进回执，
> 并在正文留 `<!-- 台账 -->` 注释），**不要伸手改** —— W2/W3 两轨专门收这些。

## 必读（按顺序）

1. **`review/review-findings-2026-09-12-vmerge.md`** —— 本轨的料全在它的 **§三**
   （合并造出的新失真）。**§3.1 的那个引用框逐字读完**，它是本轨唯一容易做错的地方。
   §一给你合并后的实测基线，§二告诉你哪些账已经销了（文档里凡提到「已知记账」的都要对一遍）。
2. `review/review-findings-2026-09-12-romega.md` §4.2、§4.4 —— 文档里引的「台账 §4.2 /
   §4.4」指的就是这两节。你要判断某条记账还成不成立，去这儿查它原本说的是什么。
3. `docs/acceptance-M.md` **全文 898 行**。这是本轨的主战场，别只读要改的那几段 ——
   §0.4 / §7 排障表 / §8 观测缺口三处互相引用，改一处要顺着引用链走一遍。
4. `docs/demo-3min.md`（767 行）、`README.md`（233 行）、`evals/README.md`（121 行）。
5. `.github/workflows/ci.yml` 与 `docker-compose.yml` 的抬头注释 —— **只读**，
   它们是 §B/§C 的事实来源。

`docs/dev-spec-*.md` 三份**冻结**，守卫会拦，一个字都别动。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-w1
分支     : task-w1
基线     : 见下（已建好；HEAD 就是 main 的当前位置）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-w1
git log --oneline -1        # subject 应含「W1 派单」或「V5 只修了 !status」，记下 sha 当基线
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

`scripts/check.sh` 的关键行期望（**总管 2026-09-12 在这个 worktree 里实跑过一遍，
下面是原样抄的**，任何一条变小都是回归）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=793 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok`（aiteerr / config / feishu / ingress / sandbox / server） |
| B8 评测 | `passed 10/10` |

> **本轨不碰代码，所以收尾时这五行必须逐字不变。** 变了说明你改错了地方。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
> （守卫失败是非阻塞放行且不报警 —— 不主动验一次，你不会知道它已经没了。）

## 可写路径

| 面 | 权限 |
|---|---|
| `docs/acceptance-M.md`、`docs/demo-3min.md` | 读写 |
| `README.md`、`evals/README.md` | 读写 |
| `review/review-findings-2026-09-12-vmerge.md` | 只许**追加**一节「W1 回执」，不改前面的 |
| `docs/dev-spec-*.md`、`proto/**`、`core/crates/contracts/**`、`.contracts.lock` | **冻结**，守卫会拦 |
| `core/**`、`edge/**`、`.github/**`、`docker-compose.yml`、`Makefile`、`scripts/**`、`evals/p0/**`、`.claude/**` | **只读**（它们是你的事实来源，不是你的改动面） |

---

## 要做什么

### §A `!status` / `!stop` 那五处 —— 本轨最要紧、也最容易做错的一段

`docs/acceptance-M.md` 有五处在教总管一件**已经只对一半**的事。

**先把事实核清楚，不要信这份派单的转述**，自己开文件看这三行：

```
core/crates/control/src/plane.rs:481   status_tasks()         list_active_tasks + 并进控制面 running
core/crates/control/src/plane.rs:651   resolve_stop_target()  仍然只有 list_active_tasks
core/crates/control/src/plane.rs:669   resolve_task()         仍然只有 list_active_tasks（卡片 stop 按钮走这条）
```

结论应当是：

- **`!status` 这一半：V5 修好了。** `status_tasks` 把控制面自己的 `running` 并了进来
  （过滤终态 + 用 `get_session` 过滤到本群），交付中的短任务现在**列得出来**。
- **`!stop` 这一半：没修，还在原地。** `resolve_stop_target` 和卡片 stop 按钮走的
  `resolve_task` 都只查 `list_active_tasks`。

**所以改法不是「删掉这段记账」，是「把它拆成两半，并且点明两半现在互相矛盾」：**

> 交付中的短任务**在 `!status` 里列得出来**（V5 修的），**`!stop` 却会回「没有这个任务」**。
> 用户看见它在列表里、伸手去停却被告知不存在 —— 这比改之前（两条都查不到）**更费解**，
> 排障时要认得出这个形状。

五处的位置与现状：

| 行 | 现在写的 | 要怎么改 |
|---|---|---|
| `acceptance-M.md:358` | `<!-- 台账 §4.2；V5 修掉后删本段 -->` | 注释换成只指 `!stop` 那一半 |
| `:359-365` | 整段 ⚠️ 引用框，标题是「`!status` 查不到、`!stop` 会回…」 | 重写成上面那个拆两半的说法；`Answering` / `ACTIVE_TASK_STATUSES` 的机理还是对的，留着 |
| `:435` | 排障表：「**已知记账，不是故障** … **别去查沙箱**」 | **最要紧的一行**。现在 `!status` 查不到就是**真故障**了，「别去查沙箱」这个指引会把人引错方向 |
| `:554` | 「这一路交付中 `!status` 查不到它」 | 同 §A 的口径 |
| `:864` | 「交付中的短任务查不到，见 §0.4」 | 同上 |

**验证要求**：你改完之后，文档里对 `plane.rs` 的每一处行号引用都要**当场核一遍**
（合并后行号可能已经漂了）。引用写成 `文件:行号` 的，行号对不上就是新的失真。

### §B README 的 CI 章节停在 V1 之前

- **`README.md:217`** 的十步链条以「→ compose 双 service 可解析」收尾。V1 把那一步
  **换掉了**：`ci.yml` 现在是两个并行 job —— `checks`（A1…B8）与独立的 `compose-smoke`。
  后者真 build 两个镜像 → `preflight --offline` → 两个进程真起 → 断言两个 socket 都在
  共享卷里、cwd 是 `/app`、运行层没有工具链、core 侧四行起飞日志、`RestartCount` 为 0。
  **照着 `ci.yml` 重写这一段**，别照着这份派单抄。
- **`README.md:114`** 的「ℹ️ **CI 里目前没有 preflight 这一步**」**现在是假的** ——
  `ci.yml` 里那一步就在（自己 grep 确认行号再写）。这句是 V3 加的，加的时候它是真的。
  改成说清楚：CI 的哪个 job 里跑了哪一档 preflight。

### §C README 的容器段会让人起不来

`README.md:82-89` 现在只有一句 `docker compose up -d`。V1 已经把 compose 从
「挂仓库进去现编」换成**两个多阶段真镜像** —— 镜像不在就起不来，而 `docker-compose.yml`
的抬头写着「`make compose-up` 会连 `aite-sandbox:p0` 一起建」。

这一段要补：先建镜像这一步、新增的 `make compose-*` 那几个 target、healthcheck
（edge 探标准 gRPC health，core 判据是 ingress socket 真 connect 得上 —— `test -S` 会假绿）、
以及日志轮转（`max-size 10m` / `max-file 5`）。**事实来源是 `docker-compose.yml` 抬头
注释 + `Makefile` 的 `compose-*` target + `docker/{core,edge}/Dockerfile`，只读它们。**

原来那条「⚠️ `docker compose config` 会把 `${VAR}` 解析成取值」**保留** —— 它仍然成立，
而且 V1 把 `make compose-config` 也改成了 `env -u` 口径，顺带提一句。

### §D `docs/acceptance-M.md:277-284` 的 compose 占位段 —— 现在可以填了

那一段正文写着「**compose 起（占位，等 V1）**：…… 正在被 V1 改成真镜像，命令行与
service 形状**这里先不写死**」。**V1 已经合进来了，把这个占位填掉。**

它下面列的三条「现在成立、改完大概还成立」要逐条**实证**（不是推断）：
看日志用 `docker compose logs -f`；两个 socket 在命名卷 `run:` 里、宿主机上看不到；
仓库是不是还 bind mount 进去（**这条八成变了** —— V1 改成真镜像之后 `./:/app`
那个挂载还在不在，直接决定「`aite evidence show` 照常在宿主机跑」这句还成不成立）。

同时，**§0.3 的四个观察窗在 compose 形态下分别是什么命令**，V1 的派单点名要求过，
这里要给出说法。

### §E 另两处 `<!-- 台账 -->`

- **`acceptance-M.md:49-53`** ——「`aite run -- --help` 走 **stderr**、**退出码 2**」。
  V6 ④b 已改成 **stdout + 退出码 0**。**但别整段删**：`aite run --help`（不带 `--`）
  **仍被 clap 截胡**，根治要动 `main.rs`，归口 R0，至今没人接。
  改成「哪个写法现在对、哪个还不对」，台账注释指向仍未修的那一半。
  **四个写法各跑一遍**（`aite preflight --help` / `aite preflight -- --help` /
  `aite run --help` / `aite run -- --help`），退出码和走 stdout 还是 stderr 都实测，别推断。
- **`acceptance-M.md:276`** —— `--traceback` 那条。V6 ④c 已经把承诺改小了
  （不再说「完整错误链」）。核一遍 `core/crates/app/src/cli.rs` 现在的实际文案与行号，
  对上了就把台账注释删掉。

### §F V4 的实测报告没有入口

`evals/live-report-2026-09-12-v4.md`（529 行，十场景 × 两档 × 两遍 + 一遍不花钱的
scripted × docker 对照）全仓只有它自己引用自己。`evals/README.md` 121 行里一个字没提。

**它同时是一处更正**：RΩ 台账 §4 有一条「与 Python 版结论相反」，V4 查出来那不是翻转、
是**比错了基线**（引的 T17 基线 `0d6939c` 上 `platform.md` 还是 T19 改提示词之前的
2246 字节版；Rust 用的这份与 Python 删除前最后一版逐字节相同，3878 字节、sha256
同为 `0d2ecfe8…`）。**这个更正现在没有任何人看得见。**

在 `evals/README.md` §2（live）里加一份报告索引：五份 `live-report-*.md` 各自是哪一轮、
结论是什么、哪些结论被后来的报告更正了。**别把报告正文抄过去**，给指针和一句话结论。

### §G 顺带核一遍，别只改上面点名的

三份文档里凡出现下面这些，都对一遍（**只报告，不猜**）：

- `文件:行号` 形式的代码引用 —— 合并后行号大面积漂过（V2 给 `preflight.rs` 加了 849 行、
  V5 给 `plane.rs` 加了 144 行、V6 给 `wiring.rs` 加了 404 行）；
- 「已知记账 / 台账 §…」的说法 —— 对着 `vmerge.md` §二那张表，销掉的账不能还挂在文档里；
- 实测输出块（preflight 的七行、起飞日志那几行）—— 本轨不要求重跑真机，
  但 `preflight --offline` 这一档**可以也应该真跑一次**，输出变了就更新。

`README.md:57,72` 那条「`make build` 不产出 `edge/bin/aite-edge`」——
**核过了，V1 没动 `build` target，这条仍然成立**，别顺手改掉。

---

## 纪律

1. **不改代码。** 撞见代码侧问题 → 正文留 `<!-- 台账 -->` 注释 + 写进回执，**不伸手**。
2. **每一条都要有依据。** 写「实测」的必须真跑过并贴输出；没跑的写「未验证」。
   派单里的转述**不算依据** —— 尤其 §A，自己开 `plane.rs` 看。
3. **不许整段删。** 上面每一处都是「一半对一半错」，删掉对的那一半就是造新的失真。
4. **行号引用必须当场核。** 写下 `文件:行号` 之前先 `sed -n` 看一眼那一行是不是它。
5. 冻结面（`docs/dev-spec-*.md`、`proto/**`、`core/crates/contracts/**`）一个字都别动。
6. 卡住了：范围外的文件要改 → 停下报告；文档与代码冲突且不知道哪个对 → **以代码为准**，
   并在回执里点名。

## 验收

```bash
scripts/check.sh                      # 五行关键值逐字不变，最后一行「全部通过」，退出码 0
git status --short                    # 只该有你改的那几份文档
git diff --stat                       # 自己看一眼改动面有没有越界
core/target/debug/aite preflight --offline   # §G 要用
```

外加**自己交叉验一遍**（这几条没有自动化判据，靠你走一遍）：

- [ ] §A 五处改完后，`acceptance-M.md` 里对 `!status` / `!stop` 的说法**前后一致**，
      §0.4 引用框、§7 排障表、§8 三处互相引用的链条走得通。
- [ ] `acceptance-M.md` 与 `demo-3min.md` 对同一件事的说法**逐字对得上**
      （V3 立的规矩是「引它不抄它」，别在第二份里造出第三种说法）。
- [ ] 文档里每一处 `文件:行号` 都核过。
- [ ] `<!-- 台账 -->` 注释：修掉的删、没修的留并指向 W2/W3。
- [ ] README 的 CI 章节与 `ci.yml` 的实际步骤逐条对得上。

## 回执格式

在回复里写（**不要**新建回执文件）：

```
## W1 回执

基线 : <开场自检那个 sha>
check.sh : <五行关键值原样抄>  退出码 <n>

### 改了什么
（按 §A–§G 分条；每条写「原来说什么 / 现在说什么 / 依据是什么」）

### 实测记录
（真跑过的命令与输出，尤其 §E 那四个 --help 写法、§G 的 preflight --offline）

### 记账转出去的（没伸手改的代码侧问题）
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
（明确写，别含糊过去）
```
