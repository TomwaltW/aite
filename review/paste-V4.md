# 任务 V4 — `--model live` 全场景实测：十个场景 × 两档 × 两遍，外加把 checklist 那条「翻转」查清楚

## 背景：这轨是从哪来的

Aite 从 2026-09-11 起用 **Rust(core) + Go(edge)** 重写：R0 骨架 → R1–R7 七轨并行 → RΩ 组装起飞
（`aite run` / `aite preflight` / 评测接线 / compose 双服务），并删掉整棵 Python 树
（178 个文件、−33272 行）。RΩ 刚刚合入 main（`11322b3`）。

`docs/dev-spec-2026-09-09.md`（P0 的权威文档）里还没兑现的只剩两件：§2.4 的 **M1–M6 真机验收**
（真实飞书测试群，总管亲自跑）和「**能录一段 3 分钟演示**」。§1 又明令禁止「任何『顺手做一点 P1』
的行为」。所以这一批六轨的定位统一是：**把 P0 收尾到「总管可以坐下来跑 M1–M6、可以开录」**，
外加把 RΩ 审核留下的账清掉。

你这一轨盯的是**开录之前的最后一道保险**。Rust 版目前唯一一份真模型实测报告是
`evals/live-report-2026-09-12-romega.md`，它只跑了 **1 个场景**（`04_csv_to_chart`）、
每档**只跑了一遍**，而它自己在 §5「没测到的」里点名了三件事：另外九个场景在 `--model live`
下的形状没测、稳定性没跑第二遍、以及一条它称之为「与 Python 版结论相反」的翻转 ——
**checklist 工具**：Python 版实测「一次都没用过」，Rust 版两跑都用满了四个 checklist 工具。
而那张会原地更新的 checklist 卡片，**正是 3 分钟演示第二幕的主角**
（`docs/demo-3min.md:324` 起的整节，全片 60 秒的一半押在这一幕上）。

**一次不算数。** 演示要靠它，就不能建立在一个场景、一遍、一条报告自己都说「不敢归因」的观察上。
你的活是把这三件「没测到」变成实测：十个场景、两档、每档两遍，写成一份新报告。

> **写派单时替你先查了一件事，它会改变你第 ⑥ 项的形状 —— 那条「翻转」很可能根本不是翻转。**
> 结论先放这儿，证据在 ⑥：Rust 用的提示词与 Python 删除前那一份**逐字节相同**（都是 3878 字节，
> `diff` 无输出），而 RΩ 报告拿来对照的是 **T17（基线 `0d6939c`，在 T19 改提示词之前）** 的数，
> 不是 Python 侧的最终态。你要做的不是「解释一个谜」，是「验证一个已知结论在换语言之后还成立、
> 而且稳定」，顺便把那张对照表改对。

---

## 必读（按顺序，四份）

1. **`evals/live-report-2026-09-12-romega.md`（全文，139 行）** —— 你要接着写的那份，骨架就是它。
   重点四处：**§2**（`--model live --sandbox fake` 对带 `exec_script` 的场景是错配 ——
   这条读法你要抄进自己报告的最前面，但**范围要收窄**，见下面「读法 1」）、
   **§3**（`passed k/n` 不是判据，docker 档那条失败是场景耦合了替身的排版）、
   **§4**（与 Python 三份报告的对照表 —— **checklist 那一行比错了基线，你要在自己的报告里更正它**）、
   **§5**（没测到的三条 = 你这轨的任务书）。

2. **`evals/live-report-2026-09-10-t19.md` §3 / §2 / §4** —— **这一份对你最要紧。**
   T19 是 Python 时代专门为「让 DeepSeek 真的去用 checklist」跑的一轨：改了提示词四句话，
   `checklist_*` 从 0 次变成 **31 次**，而 §3 那张 **十个场景 × 前后各两遍**的全表
   （`checklist_*` / `update_card` / 终态 / 步数，外加「第一步出的牌」那张）
   **就是你 fake 档那两遍要逐格填上「Rust 列」的表**。§2 解释这个数是怎么变出来的；
   §4 是 M3 的逐条判据，演示第二幕看的就是它。

3. **`evals/live-report-2026-09-10-t23.md` §一 / §三 / §五** —— Python 侧的**最终态实测**。
   §三那张表是 `--model live --sandbox fake` **全套十场景的实跑结果：`passed 5/10`，
   五条失败逐条列了原因**，这是你 fake 档最直接的对照基线（下面 ④ 那张预判表就建在它上面）。
   §一是真沙箱那一档（04 三遍全 delivered、7 步、PNG 40139 字节）。
   §五第 1 条是 04 那条 `exit_code=0` 耦合的处置建议，与你第 ⑤ 项同题 —— 它当时倾向的路
   和现在总管的倾向**不一样**，差别的理由见 ⑤。

4. **`review/review-findings-2026-09-12-romega.md` 第四节 4.1 + 第五节** ——
   4.1 的八条 medium 里有两条你会正面撞上（`wiring.rs:98` 与 `wiring.rs:153`），
   **你不修它们**，但要认得出来，免得当成新缺陷报（见 ⑨）；
   第五节是这一批统一的起跑线数字，也就是你开场自检那张表。

（`review/paste-ROMEGA.md` 是上一轮的派单，想知道接线是怎么做的可以扫一眼，不必读全。
`review/inventory-gateway-evals.md:85` 那一行是 Python 时代四轨真模型实测的一行总账，
独立印证上面第 2、3 条的读法，一句话就能读完。）

---

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-v4
分支     : task-v4
基线     : 0bc8d55（main 的 HEAD，worktree 已按这个 sha 建好）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、go 1.27.1、protoc 36.1、Docker 29.6.1
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。
`scripts/check.sh` 自己会 `export` 这一行，手跑 `cargo` / `go` 时要自己带上。

**全程在 worktree 根跑命令。** 不是洁癖：`aite evals` 读的 system prompt 路径
（`core/crates/worker/prompts/platform.md`，契约默认值，`config.rs:101`）和
edge socket 路径（`data/run/aite-edge.sock`）**都相对进程 cwd**，
`agent.rs:305` 是在每个任务开跑那一刻才去读提示词的 —— 在子目录里跑，
要么读不到提示词、要么连不上 edge。

---


> **基线说明**：`0bc8d55` = RΩ 合入 main 那次（`11322b3`）**再往前一格**。
> 那一格只改了两个文件：`scripts/check.sh`（B 全量 cargo test 那步改成「失败测试名在前、计数在后」）
> 与 `review/review-findings-2026-09-12-romega.md`（收窄 Answering 那条 + 补记一次未复现的 717/1）。
> `git diff --stat 11322b3..0bc8d55` → `2 files changed, 11 insertions(+), 2 deletions(-)`。
> 本派单正文里凡是写「在 `11322b3` 上核过 / 实测」的，指的是核对当时那一格，**代码面与你的基线逐字相同**。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v4
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
| B8 评测 | `passed 10/10` —— **RΩ 起它是硬门禁**（`scripts/check.sh:43-45`：退出码 0 **且**最后一行逐字 `passed 10/10`，两条都判） |

另外单独跑一次（不在 check.sh 里）：

```bash
cd edge && go test -tags docker ./internal/sandbox/... -count=1   # 期望 ok
docker ps -a --filter label=aite.task -q | wc -l                  # 期望 0
```

> **已知的假红之一**：docker 档沙箱测试在多轨并行时会假红（六个 worktree 同时跑真容器会互相挤）。
> 它红了先看两件事：是不是**只有这一个包**红、**单独跑**是不是绿。是的话就是并行挤的，不是回归。
>
> **已知的假红之二**：`cargo passed=717 failed=1`。合并后在 main 上复验时出过一次，
> 紧接着连跑六遍都是 `718 / 0`，当时机器上正有六个 agent 在抢 CPU
> （台账第五节末尾那条注记）。**撞到它请把 `error: test failed, to rerun pass ...` 那行贴进回执** ——
> 上一次的现场就是这么丢的（`check.sh` 已改成会把失败测试名打出来）。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
> （2026-09-12 总管这边真踩过：会话在仓库子目录里起，`CLAUDE_PROJECT_DIR` 就定死在那个子目录，
> hook 命令 `python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"` 找不到文件 →
> 执行失败 → **非阻塞放行、不报警** → 整个会话守卫静默失效。台账第六节记的就是这一条。）
> 守卫认得它自己：`guard_bash.py` 在 `BARE_MATCH` 里（`.claude/hooks/guard_bash.py:44`），
> hook 的 matcher 覆盖 `Read`（`.claude/settings.json`）。

### V4 专属的四条前置自检（跑真模型之前）

```bash
test -n "$AITE_MODEL_API_KEY" && echo "key 已设置（长度 ${#AITE_MODEL_API_KEY}）"   # 期望「长度 35」
docker info --format '{{.ServerVersion}}'                                          # 期望 29.6.1
docker images aite-sandbox:p0 --format '{{.Repository}}:{{.Tag}} {{.Size}}'        # 期望 aite-sandbox:p0 646MB
wc -c core/crates/worker/prompts/platform.md                                       # 期望 3878
```

最后那条是你第 ⑥ 项的地基：**3878 就是 T19 改完提示词之后那份的字节数**
（`live-report-2026-09-10-t19.md:92` 原话「文件 2246 → 3878 字节」）。

**任何时候都不要 `echo $AITE_MODEL_API_KEY`，不要把取值写进报告、日志或提交。**
上面那条只打长度，是故意的。

---

## 可写路径

| 面 | 权限 |
|---|---|
| `evals/live-report-2026-09-<日期>-v4.md`（新建） | ✅ 这一轨唯一的交付物，也是唯一该进提交的文件 |
| `config/aite.yaml` | ✅ 但它 `.gitignore:10` 不入库，改它不进提交，也不许把密钥取值写进去 |
| `data/**` | ✅ 跑出来的 JSON / 日志 / edge 日志放这儿（`.gitignore:7` 已忽略，`git status --short` 才保持干净） |
| worktree 外的临时目录（⑧ 那条对照套件） | ✅ 放 `/private/tmp/...` 或 `data/` 下都行，**不要放进 `evals/`** |
| `evals/p0/**` | ❌ **一个字不动。** 十个场景是 §3.8 的验收面 |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock` | ❌ 契约冻结，全程 `OK 25 files`。要动 → 停下报告 |
| `docs/dev-spec-2026-09-09.md`、`docs/dev-spec-2026-09-11-rustgo.md` | ❌ 冻结 |
| `core/**`、`edge/**` 的任何代码 | ❌ **撞出真缺陷先停下报告**，那是别的轨的文件 |

---

## 要做什么

### 读法先行 —— 这四条不先读进去，你会把设计如此的东西当缺陷报

**读法 1：`--model live --sandbox fake` 是错配，但只对两个场景。**

RΩ 报告 §2 给的原话是「**`--model live` 之后请接着写 `--sandbox docker`**」。这话成立，
但范围要收窄。场景的 `sandbox.exec_script` 是照 `model_script` 那份台词写的
（`04_csv_to_chart.yaml:19-25` 只认代码里出现 `savefig`，`07_commands.yaml:37-41` 只认 `while`），
真模型写的代码对不上，`FakeSandbox` 就返回「无害的默认值」——
退出码 0、stdout 空、没有产出文件（`inventory-gateway-evals.md` §11 第 22 条）。
RΩ 实测：04 在这一档下模型连试六次 `run_python`，连 `print("hello")` 都没输出，
最后判定「沙箱执行环境异常」，交付了一份说清卡点的失败说明。**那不是缺陷，是这个组合本身的错配。**

**我核过十个 yaml：只有 `04_csv_to_chart` 和 `07_commands` 有 `sandbox.exec_script`。**
另外八个场景压根不碰沙箱，`--sandbox fake` 对它们**不是错配**。而且——

**读法 2：fake 档不是可跑可不跳的一档，它是唯一能和 Python 那两张表逐格对上的一档。**

T19 §3 那张十场景 × 前后各两遍的表、T23 §三那张全套五条失败的表，
跑的**都是 `--model live --sandbox fake`**（T23 §二的命令逐字写着
`python -m aite.evals run evals/p0 --platform fake --model live`）。
**所以两档各有各的用**：

- **fake 档**比的是「换语言之后，模型的出牌形状变没变」—— 与 T19/T23 逐格可比；
- **docker 档**比的是「真沙箱 + 真 Gateway 那一段还通不通」—— Python 侧只在 04 上跑过。

**别只跑一档。**

**读法 3：`passed k/n` 不是判据，「哪一条为什么红」才是。** 把这张 2×2 摆出来就清楚了：

| | `--model scripted` | `--model live` |
|---|---|---|
| `--sandbox fake` | **10/10** —— B8 硬门禁，已知 | Python 侧 **5/10**（T23 §三，五条失败逐条有解释）· **Rust 侧没测过 = 你要填的** |
| `--sandbox docker` | Python 侧 **9/10**（T23 §五第 1 条，只红 04 那条 `exit_code=0`）· **Rust 侧没测过 = 一条不花钱的对照，见 ③** | Rust 侧只测过 04（`passed 0/1`，RΩ）· **全套没测过 = 你要填的** |

四个格子里只有左上角是验收数。其余三个格子里的红，**默认假设是场景耦合了替身台词**，
要逐条归因之后才能说别的。

**读法 4（并行相关）：`docker ps -a --filter label=aite.task` 看到的容器不全是你的。**
六轨同时在跑，Docker daemon 是全机共享的，标签是 `aite.task=<task_id>`
（`edge/internal/sandbox/docker.go:56-57`），task_id 是 UUID，没法按轨过滤。
**跑之前先记一次基数，跑完看增量**，别拿绝对值当判据：

```bash
docker ps -a --filter label=aite.task --format '{{.ID}} {{.Label "aite.task"}} {{.CreatedAt}} {{.Status}}'
```

---

### ① 先把「跑得起来」这件事解决（worktree 里没有 `config/aite.yaml`）

**病在哪**：`config/aite.yaml` 是 `.gitignore:10` 忽略的本地实配。实测
`ls .worktrees/task-v4/config/` **只有 `aite.example.yaml` 一个文件**。
而 `aite evals` 的默认配置路径写死在 `core/crates/app/src/app.rs:30`
（`DEFAULT_CONFIG_PATH = "config/aite.yaml"`），`aite-edge` 的默认也是它
（`edge/cmd/aite-edge/main.go:72`）。

**怎么验证它确实病着**：不建 config 直接跑 `--model live`，会撞
`core/crates/app/src/wiring.rs:99` 的 `live_model_factory` →
一行「`--model live` 起不来：…」+ 退出码 2。

**先搞清楚 `aite evals` 到底读 config 的哪几段** —— 这决定了你哪些字段必须填对、
哪些填错也不影响结论。我把三处读取点都核过了：

| config 段 | evals 读不读 | 出处 |
|---|---|---|
| `model:` **全段** | ✅ 读。`--model live` 时 `live_model_factory` 把它整段 clone 进每个场景的 `OpenAiCompatModel` | `wiring.rs:130-144` |
| `edge:` 段 | ✅ 读。`--sandbox docker` 时用它连 edge | `wiring.rs:102-115` |
| `worker:` / `storage:` / `sandbox:` / `platform:` / `feishu:` | ❌ **一律不读。** 场景的配置走 `config_of(sc)` = **契约默认值 + 场景 yaml 自己的 `config:` 片段**，`platform` 硬写成 `fake` | `core/crates/evals/src/deps.rs:267-275` |

**推论一（要写进你报告的「怎么跑的」那节）**：这一轨用的 system prompt 是**契约默认值**
`core/crates/worker/prompts/platform.md`（`core/crates/contracts/src/config.rs:95-104`），
由 `core/crates/worker/src/agent.rs:305` 按相对 cwd 读。**config 里的 `worker:` 段影响不到它。**

**推论二**：`--sandbox docker` 那一档用的镜像也不是 config 里那个 —— `docker_sandbox_factory`
里的 `sandbox_spec_of(config)` 拿的是**场景**的 config（`wiring.rs:151-158` 那个闭包参数），
所以是契约默认的 `aite-sandbox:p0`。`cli.rs:305-315` 起飞前体检查的也是这一个。

**别抄主仓那份 `~/Documents/Aite/config/aite.yaml`**，三条理由（我逐条核过）：

1. `platform: feishu`（第 6 行）—— 你的 `aite-edge` 会去起飞书长连接
   （`edge/cmd/aite-edge/main.go:128`）。docker 档只借 edge 的沙箱面，
   `platform: fake` 就够（`wiring.rs:13-15` 写明，RΩ 实测过），fake 分支只打一行
   `edge.platform_fake` 就不碰飞书了（`main.go:141-143`）。
2. **它整份没有 `edge:` 段** —— 两边都有默认值兜得住（`config.rs:138-147`、
   `edge/internal/config/config.go:65-70`），但你不该靠一个看不见的默认跑四遍实测。
3. `system_prompt_path: aite/worker/prompts/platform.md`（第 36 行）指着**已经删掉的 Python 树**。
   按推论一，这条**污染不了 evals 的结论**（上一版派单初稿说它会让你全部结论作废，
   **那是错的，别信**）—— 但它会让 `aite run` 在组装第 2 步 `require_system_prompt`
   就 StartupError 退 2（`core/crates/app/src/app.rs:271-277`），别拿它去起 `aite run`。

**改法**：从 `config/aite.example.yaml` 复制一份，只改三处：

| 字段 | 改成 | 为什么 |
|---|---|---|
| `platform`（第 7 行） | `fake` | 上面理由 1 |
| `model.base_url`（第 18 行，样例是空串） | `https://api.deepseek.com/v1` | 与 Python 三份报告同一个端点，才可比 |
| `model.model`（第 20 行，样例是空串） | `deepseek-chat` | 同上 |

`model.api_key_env: AITE_MODEL_API_KEY`（第 19 行）已经对了，**别动** —— 密钥只走环境变量。
`worker.system_prompt_path`（第 37 行）样例里已经是对的，核一眼就好。
`model.price_in_per_mtok` / `price_out_per_mtok`（第 23-24 行）填不填见 ②。

**起 edge**（`edge/bin/` 在 `.gitignore:17`，二进制要自己编）：

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v4
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/go/bin:/opt/homebrew/bin:$PATH"
mkdir -p data/v4
cp config/aite.example.yaml config/aite.yaml && $EDITOR config/aite.yaml   # 改上面那三处
( cd edge && go build -o bin/aite-edge ./cmd/aite-edge )
edge/bin/aite-edge --config config/aite.yaml > data/v4/edge.log 2>&1 &
echo $! > data/v4/edge.pid
```

**怎么确认它真起来了**（三条都要，别只看进程在）：

```bash
grep edge.takeoff data/v4/edge.log      # 期望一行，带 contract_version / platform=fake / image（main.go:114-117）
grep edge.platform_fake data/v4/edge.log # 期望一行 —— 证明它没去连飞书
ls -l data/run/aite-edge.sock           # 期望 socket 存在
```

每个 worktree 的 socket 是 `<worktree>/data/run/aite-edge.sock`（相对进程 cwd，
`config/aite.example.yaml:44-49`），**六轨各是各的，不会撞**。

---

### ② 成本：开跑之前先把估算写进回执

这一轨会真花钱。**别闷头跑十个场景 × 两档 × 两遍。**

**三个实测锚点**（都不是估算）：

| 出处 | 什么 | input | output |
|---|---|---|---|
| T23 §四 | Python 侧全套 × 2（live × fake） | 268,657 | 11,544 |
| T23 §四 | Python 侧 04 × 3（live × docker） | 60,741 | 2,994 |
| RΩ §1 | Rust 侧 04 × 1（live × docker），7 步 | 20,213（cached 17,788） | 994 |

→ fake 档一遍约 **134k input / 5.8k output**；docker 档步数少得多，一遍**推测**在 100k 上下。
四遍合计**推测** 400–500k input / 20–25k output。

**价格**：`config/aite.example.yaml:23-24` 是 `0.0 / 0.0`；T17/T19/T23 三份报告统一用的假设是
**cache miss 输入 ¥2/百万、cache hit 输入 ¥0.2/百万、输出 ¥8/百万**。
按全部 miss 算的**上界**约 **¥1.1**；按 T17 实测 ~91% 的缓存命中率算约 **¥0.35**。
**个位数人民币以内，大概率不到 ¥1。**

上限还有兜底：`worker.max_steps: 40`、`max_wall_sec: 1200`（契约默认，`config.rs:95-104`），
`08_step_limit.yaml:13` 自带 `max_steps: 3`、`07_commands.yaml:34` 自带 `max_steps: 8`；
每个场景的等待上限是 `10.0 × 12 = 120` 秒（`scenario.rs:95-97` 的默认值 × `cli.rs:39`
的 `LIVE_TIMEOUT_SCALE = 12.0`）。

**要你做的三件事**：

1. **开跑前**用上面的方法自己算一遍，把方法和数字写进回执；
2. **跑完**从四份 `--protocol-report` JSON 里把每一步的 `usage` 三个字段
   （`input_tokens` / `output_tokens` / `cached_tokens`，`protocol_probe.rs:371-375`，
   落在 `runs[].steps_detail[].usage` 下）汇总出实际值。**估算和实际两个数一起放**，
   差得离谱本身就是一条结论；
3. **价格填不填进 `config/aite.yaml` 由你定**，在回执里说清选了哪条。
   填了的话卡片上的「已用 ¥」才不是 0（不填时 `aite preflight` 第 ⑤ 组会说
   「¥0（config 里 price_in/out_per_mtok 都是 0，没配价格）」，`preflight.rs:911-915`）；
   **不填也不影响 `--protocol-report` 里的 token 数** —— 那三个字段直接来自 API 的 usage。
   注意 `cost_cny` 只进 `turn.raw`（`core/crates/models/src/lib.rs:458`），
   **不进协议报告**，所以钱要你自己按 token 折。

---

### ③ 四条主命令 + 一条不花钱的对照

**先跑那条不花钱的对照**（scripted × docker）。它把「真 Gateway / 真沙箱带来的红」
和「真模型带来的红」分开，是你归因 ④ 那张表的必要前提，而且一分钱不花：

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v4
A=core/target/debug/aite
$A evals run evals/p0 --sandbox docker --json data/v4/scripted-docker.summary.json \
   > data/v4/scripted-docker.stdout 2> data/v4/scripted-docker.stderr
tail -1 data/v4/scripted-docker.stdout      # Python 侧实测 9/10（T23 §五第 1 条）；Rust 侧没测过
```

**Rust 侧这一格没人跑过，所以它不是「期望 9/10」，是「跑出来是几就是几」。**
不是 9/10 的话，多出来的那些红**必然与真模型无关** —— 这正是这条对照的价值。

然后是四条主命令：

```bash
# fake 档两遍 —— 与 T19 §3 / T23 §三那两张表逐格可比的一档
for i in 1 2; do
  $A evals run evals/p0 --model live \
     --protocol-report data/v4/fake-$i.json --json data/v4/fake-$i.summary.json \
     > data/v4/fake-$i.stdout 2> data/v4/fake-$i.stderr
  echo "fake-$i exit=$? : $(tail -1 data/v4/fake-$i.stdout)"
done

# docker 档两遍 —— 真沙箱那一档（edge 必须在跑）
for i in 1 2; do
  $A evals run evals/p0 --model live --sandbox docker \
     --protocol-report data/v4/docker-$i.json --json data/v4/docker-$i.summary.json \
     > data/v4/docker-$i.stdout 2> data/v4/docker-$i.stderr
  echo "docker-$i exit=$? : $(tail -1 data/v4/docker-$i.stdout)"
done
```

几条你会看到、但**不是缺陷**的输出：

- `场景等待上限 x12（--timeout-scale）` —— `--model live` 的默认放大倍数（`cli.rs:334-345`），
  场景 yaml 一个字没改。
- `--sandbox docker：这些场景的 sandbox.exec_script 不生效（代码交给真容器跑）：04_csv_to_chart、07_commands`
  —— `cli.rs:329-332` 主动打的提示，正确行为（`inventory-gateway-evals.md` §11 第 25 条）。
- **stdout 是「一份 JSON 摘要 + 最后一行 `passed k/10`」，协议摘要走 stderr**
  （`cli.rs:413-414` 的注释写死了这个分工）。别去 stdout 里找协议摘要。

**`--protocol-report` 是你这轨的主仪表。** 它一次写下全部十个场景，每个场景带
`tool_names`（工具名计数器 → ⑥ 那条结论直接从这里读）、`runs[].steps_detail[]`
（每步的 `finish_reason` / `usage` / `tool_calls` / `elapsed_ms` / `n_messages`）、
`unknown_tools`、`schema_violations`、`repeat_loops`、`hit_max_steps`、`outbound_texts`
（`protocol_probe.rs:693` 的 `analyze()`，字段清单在 `:742` 与 `:754-771`）。

**两遍之间要不要重启 edge**：**建议重启。** 理由在 ⑨ 那张已知项表里 ——
docker 档的「场景收尾把容器收干净」是空操作，**重启 edge 是目前唯一真能把残留容器收干净的动作**。

**盯 07 那一格**：`07_commands` 的任务描述是「把全量数据重算一遍」，
它的 scripted 台词写的就是 `while True: pass`（`07_commands.yaml:67`），
本来就是设计来诱导长跑的。docker 档下真模型写的代码交给真容器跑，
撞上 `sandbox.exec_timeout_sec: 120` 是可能的。跑的时候另开一个窗口 `docker ps` 看着。

---

### ④ 逐条读 `passed k/10` —— 预判表已经建在 T23 的实测上

`passed k/n` 不是判据，但**「哪一条为什么红」是判据**。
下面这张表里，「Python 侧实测」那一列**全部来自 T23 §三的全套实跑**（不是猜），
每个 yaml 行号我都逐个打开核过。

你要做的是：跑完之后**逐条**把实际失败对上这张表，并把每条归到三类之一 ——
**(a) 耦合台词 / (b) 结构性不可测 / (c) 真缺陷**。落到 (c) 的**停下报告**。

| 场景 | live 下有风险的断言（行号已核） | Python 侧实测（T23/T19） | 你要盯什么 |
|---|---|---|---|
| `01_simple_qa` | `:27 {check: text, where: last, contains: 北京今天晴}` | **红**。模型拒绝编天气：「沙箱没有网络，也没接天气数据源」—— 正是 W9 铁律 4。T23 原话：「替身的验收判据在惩罚模型的正确行为」→ **(a)** | 另两条是**免费的仪表**：`:24 send_card equals 0`、`:26 cards distinct_equals 0`，只要模型第一步出 `checklist_add` 就会红。T19 §3 实测 01 前后两遍第一步都是 `final`、checklist `0/0`。**Rust 侧这两条红了就是出牌形状变了，要单独记一段** |
| `02_thread_followup` | `:38 send_text equals 2`；`:39 contains 季度` | T23 没列它 = **两条都过**。T19：步数 9/8 → 18/12，checklist `0/0 → 8/8` | 步数会翻倍，但 `send_text` 仍应是 2 |
| `03_checklist_progress` | `:47 model_tools checklist_add equals 1`；`:48 model_tools checklist_check min 3`；`:45 cards distinct_equals 1, updates_min 3, final_status delivered` | **`:48` 红，实际 0**。T23 的解释：替身沙箱空返回 → 模型按铁律用 `checklist_fail` 而不是 `check`，「卡片确实在动，只是勾变成了叉」→ **(a)** | **这是演示主角的仪表盘，最要紧的一格。docker 档下叉会不会变回勾，是你这轨最有价值的一个数** |
| `04_csv_to_chart` | `:69 {check: gateway_result, name: run_python, ok: true, contains: "exit_code=0"}`；`:71 send_file equals 1` | fake 档 **`:71` 红**（没真 PNG 可发）；docker 档 **`:69` 必红**（已证，见 ⑤）→ 两条都 **(a)** | docker 档另两条最硬的应该过：`:72 {check: file, index: 0, magic: "89504e470d0a1a0a"}`（发回去的真是 PNG）、`:74 {check: task, which: last, status: delivered}`。RΩ 实测这两条都过了 |
| `05_history_summary` | `:45 {check: distinct_matches, source: last_text, pattern: "om_h[0-9]+", min: 3}` | T23 没列它 = **过**。T19：步数 5/5 → 6/6，checklist `0/0 → 4/4`，两遍都 delivered | 要模型在回复正文里逐字带三个 message_id，看着险，Python 侧四遍都过了 |
| `06_read_document` | `:38 contains Q3 交付计划` | T23 没列它 = **过**。T19：步数 2/2 → 4/4 | 这个串来自文档正文本身（`:36` 的 `gateway_result` 也判它），风险最低的一个 |
| `07_commands` | `:83 sandbox_calls release min 1`；`:90 sandbox_calls acquire min 1`；`:80 text where any contains "#A1"`；`:82 task any status cancelled` | **`:83`/`:90` 红，实际 0**。T23：「真模型被 `!stop` 掐掉之前还没用到沙箱」→ **(a)** | T19：07 前后 checklist 都 `0/0`、步数 `1/1`、两遍都 cancelled ——**这一条在 Python 侧对提示词改动完全不敏感，是个好基准**。docker 档下模型可能跑得更快、真摸到沙箱，那样反而变绿 |
| `08_step_limit` | `:29 model_calls min 3`；`:30 task last status failed`；`:33 send_card equals 1`；`:34 cards final_status failed` | T23 没把它列进五条失败（`08` 在 live 下照旧 3 步 failed）。**但 T19 §3 实测它两遍不一致：`failed/failed → delivered/failed`** | **结构性不可测 (b)**：yaml `:21-26` 用 `repeat: inf` 的退化脚本人为造死循环，真模型不会这么打。`inventory-gateway-evals.md:85` 记着「`08_step_limit` 在 live 下不稳定」。**别把它红了当回归**，但要如实记「live 下这个场景验不了它想验的东西」 |
| `09_bot_ignored` | `:20 outbound_total equals 0`；`:21 model_calls equals 0` | **过**（模型压根不被调用，`model_script: []`） | **唯一一个 live 与 scripted 走同一条路的场景。两档四遍都必须绿。红了是真缺陷 (c)，立刻停。这是你的金丝雀** |
| `10_duplicate_event` | `:30 model_calls equals 1` | **红，实际 4**。T23：「脚本一步答完，真模型走了 4 步。去重本身是对的」→ **(a)** | 它真正要验的那条（`:27 store tasks_equals 1, sessions_equals 1` 去重）在 live 下**照样有效**，要单独说清「这条过了没」 |

**Python 侧 fake 档的合计就是 `passed 5/10`**（红的是 01/03/04/07/10）。
**Rust 侧 fake 档如果也是 5/10、而且红的是同样这五条，那本身就是一条强结论**：
换语言之后模型的出牌形状没变。**不一样的每一格都要单独解释。**

---

### ⑤ 04 那条耦合：给处置建议，**不要自己改**

**病在哪**（RΩ §3.1 已证，我复核了四处行号，全对）：

```
evals/p0/04_csv_to_chart.yaml:69
  {check: gateway_result, name: run_python, ok: true, contains: "exit_code=0"}
```

`exit_code=0` 是 **`FakeToolGateway` 自己的排版** —— `core/crates/testing/src/fake_gateway.rs:295-298`：
`format!("exit_code={}\n--- stdout ---\n{}", res.exit_code, res.stdout)`。
**真** `P0ToolGateway` 的排版是「执行成功（N ms）…」——
`core/crates/gateway/src/tools/python_exec.rs:84`：
`parts.push(format!("执行成功（{} ms）", result.duration_ms));`，
而它与 Python 版**逐字一致**（RΩ 对拍过 `aite/tools/python_exec.py:78`，
T23 §一那段实际输出与 RΩ 这次的输出也逐字对得上）。

所以这条在 Python 时代就一样红（T23 §五第 1 条：scripted × docker 是 9/10，
live × docker 那条也是同一条红），是 `evals/p0/*.yaml` 里继承下来的耦合，
**不是本轮的回归**。`review/inventory-gateway-evals.md` §11 第 26 条
（「场景断言耦合 FakeGateway 排版」）记的就是它。

**`evals/p0/**` 是冻结面，一个字不动。** 这条**不归你改**，你的活是给处置建议交总管拍板。
四条路，代价我列在这儿，你补上自己实测出来的判断：

| 路 | 做法 | 代价 |
|---|---|---|
| A | 改 `:69` 的 `contains` 成两档共有的子串（或去掉 `contains` 只留 `ok: true`，或换成断产物文件名） | 改的是**验收面本身**（§3.8），而且 scripted 档的 B8 硬门禁跟着变，`passed 10/10` 要重新证。**T23 §五当时倾向这条** |
| B | 让真 `P0ToolGateway` 的排版带上 `exit_code=N` | 「与 Python 版逐字一致」这条就破了。Python 树已删，这条还剩多少价值要拍板。T23 当时的评价是「改实现去迁就测试，不建议」 |
| C | 让 `FakeToolGateway`（`fake_gateway.rs:295-298`）改成和真 Gateway 一致 | scripted 档十个场景的 `contains` 断言要全部重过；`core/crates/testing/tests/fake_gateway.rs:314-315` 有一条逐字断言（连注释都写着「04 的断言耦合它」）会红 |
| D | 不动，记成「这条断言在 docker 档下永远红」的已知项，docker 档改看别的判据 | 零改动。docker 档 spec §6.1 定的判据本来就是 `:74` 的 `{check: task, which: last, status: delivered}`，加上 `:72` 的 PNG 魔数 —— RΩ 实测这两条最硬的都过了 |

**总管现在的倾向是 D，理由与 T23 当时不同**：这一批的定位是 P0 收尾，
`dev-spec-2026-09-09.md` §1 明令禁止顺手做 P1；这条耦合在 Python 时代就红，不是本轮回归；
而 B8 的 `passed 10/10` 现在是 `check.sh` 的硬门禁，动验收面的代价比 T23 那时高。

**但这只是倾向。** 你跑完十个场景之后，如果发现别的场景也有同类耦合
（照 ④ 那张表，(a) 类至少还有 01 和 10），**路 A 的性价比可能反而上来了** ——
一次性把「live 下注定红」的几条一起处理，比留一堆脚注划算。
**在回执里说清你选了哪条、为什么，并把你实测到的同类耦合清单一起给出来。**

---

### ⑥ checklist 那条「翻转」：三条硬证据我替你查好了，你要做的是验证它稳定

RΩ §4 的原话是「**与 Python 版结论相反**」，并说「中间隔着换语言、换提示词渲染、
模型本身的版本漂移，本轮没有做对照实验，不敢归因」。**这三个混淆项里，前两个已经可以排掉。**

**证据 1 —— 提示词逐字节没变。**

```bash
diff <(git show d8bf391:aite/worker/prompts/platform.md) core/crates/worker/prompts/platform.md
# 我实测：无输出。两边都是 3878 字节
```

`d8bf391` 是删 Python 树前的最后一个提交。**3878 这个数不是巧合** ——
`live-report-2026-09-10-t19.md:92` 原话：「文件 **2246 → 3878 字节**」。
也就是说 **Rust 用的就是 T19 改完之后的那份提示词，一个字都没动。**

**证据 2 —— checklist 四个工具的描述与 schema 也逐字没变。**
`core/crates/contracts/src/protocol.rs:44-67` 的 `CHECKLIST_TOOLS` 与
`git show d8bf391:aite/contracts/protocol.py` 里那四条 `description` / `parameters` 逐字对得上，
包括 T19 §1④ 专门点名的那句劝阻性描述：
「添加待办项，**仅在**任务开始或发现新步骤时调用；每项 ≤20 字」。

**证据 3 —— RΩ 报告比错了基线。**
它引的「Python 版实测一次都没用过 checklist」出自 `evals/live-report-2026-09-10.md`（T17），
**基线 `0d6939c`，在 T19 之前**。Python 侧的**最终态**是 T19 之后：
`checklist_*` 0 → **31 次**，覆盖 6 个场景，**after 两遍一模一样**
（`checklist_add 7 · checklist_fail 18 · checklist_check 6 · checklist_note 0`，T19 §3）。
T23 又在 T19–T24 合流后的 main 上复核过一次。
`review/inventory-gateway-evals.md:85` 那条一行总账写得更直白：
「曾 **0 次**用 checklist → **T19 改提示词后 31 次**」。

**所以这不是「翻转」，是拿 T17 的数去比 T19 之后的实现。** 你在这条上要做四件事：

1. **验证它在 Rust 栈上还成立、且稳定。** 判据：两遍之间 `tool_names` 里四个 `checklist_*`
   的计数**逐场景**是否一致。给出「**稳定用 / 稳定不用 / 不稳定**」三选一的明确结论，
   不许写「基本稳定」。
2. **填 T19 §3 那张表的 Rust 列**（fake 档，逐场景 `checklist_*` 计数 / `update_card` / 终态 / 步数，
   两遍都列），差异大的格子单独说。**特别盯两格**：
   - `checklist_note`：Python 侧 T19 两遍都是 **0**，而 RΩ 说 fake 档用了 —— 这一格才是真正没对上的地方；
   - `checklist_fail` 与 `checklist_check` 的比例：T19 是 18:6（fail 是 check 的三倍），
     它自己说「真沙箱下这个比例会变成什么样，**没测到**」。**你的 docker 档正好能回答它。**
3. **回答演示第二幕最关心的那个问题**：`03_checklist_progress` 在 docker 档下
   `checklist_check` 是真勾（`:48 min 3` 变绿）还是仍然变成叉。
4. **把 RΩ §4 那一行的说法在你的新报告里更正**（更正写在你自己的报告里，
   **不要去改 RΩ 那份已经合入的报告**）。

**剩下真正没排掉的两个混淆项**，如实处理：

- **提示词渲染**：同一份 `platform.md` 在 Rust 侧摆进 messages 的位置 / 顺序 / 前后拼接
  是否与 Python 相同 —— 这个你**查得了**：读 `core/crates/worker/src/context.rs:124-129`
  （`Message::text(Role::System, system_prompt)` 摆在第一条），
  再拿 `--protocol-report` 里 `runs[].steps_detail[].n_messages` 与第一步的消息条数
  去对 T19/T17 报告里的实录。查得清就下结论，查不清就写「没测到」。
- **模型版本漂移**：`deepseek-chat` 是滚动别名，9-10 到 9-12 之间可能已经换过权重。
  **这个查不了**，如实写「没测到，不敢归因」。

---

### ⑦ 产出报告：`evals/live-report-2026-09-<日期>-v4.md`

照 `evals/live-report-2026-09-12-romega.md` 的骨架，六节：

1. **一句话** —— 结论先行。抬头那三行元信息照抄格式（日期 · 轨号 · 基线 sha · 装置 · 判据声明）。
2. **怎么跑的** —— 那张两列表（端点 / 模型 / 密钥 / 平台 / 沙箱 / 超时 / 场景 / 跑了几遍）
   + 可直接复制的命令。**把「读法 1」那条错配读法放在这一节的最前面**，
   并写清「哪两个场景是错配、另外八个不是」。
3. **每跑的形状** —— 逐场景 × 两档 × 两遍。至少要有：步数、到 final 第几步、任务终态、
   chat 次数/成功、`finish_reason` 分布、协议外工具名、参数不合 schema、`repeat_loops`、
   `checklist_*` 四个计数、token（input / output / cached）、耗时。
4. **失败逐条是什么** —— ④ 那张预判表的实测版，每条归到 (a)/(b)/(c)，
   **(c) 一条都不该有**（有的话你在这之前就该停下报告过）。把 ③ 那条 scripted × docker
   对照的结果也放这节，它是「这条红跟真模型无关」的直接证据。
5. **与 Python 版三份报告的对照** —— 重点是 T19 §3 那张表的 Rust 列，
   以及 ⑥ 那条对 RΩ §4 的更正。
6. **没测到的** —— 真实飞书（M1–M6 才碰）、模型版本漂移、以及你自己发现的。
   **一次不算数这条纪律对你自己也生效**：只跑了一遍的东西，写「跑了一遍，没测到第二遍」。

**Python 版那三份报告没有被删，还在树里**（上一版派单初稿说它们「已随 Python 树删除、
要用 `git log --diff-filter=D` 去历史里挖」，**那是错的，我核过**）：

```
evals/live-report-2026-09-10.md         # T17，基线 0d6939c，checklist 0 次那份
evals/live-report-2026-09-10-t19.md     # T19，基线 6ee30d4，checklist 0 → 31 那份
evals/live-report-2026-09-10-t23.md     # T23/T20，提交 0960272，真沙箱 + 全套 5/10 那份
```

`git ls-files evals/` 四份全在。**直接读，不用去 git 历史里挖。**

---

### ⑧ 加分项：演示第二幕那句台词，跟场景不是同一句（做不完就如实写没做）

**这不是必做项。** 四条主命令跑完、报告写完、还有预算，再做这一条。

**病在哪**：`docs/demo-3min.md:333-336` 里演示第二幕要发的那句话是

> `@Aite 把这个 CSV 画成月度趋势图，标出最高和最低的月份，最后用一句话说结论`

而 `04_csv_to_chart.yaml:29` 的文案是「把这个 CSV 画成月度趋势图」—— **少了后两个动作**。
demo 文档自己在那句下面写明了为什么：「这句话里的三个动作是**故意的**，
它们让模型建出至少 3 项 checklist，卡片才会更新 ≥3 次。**只说『画成月度趋势图』，
模型很可能两步就干完，卡片只变一次，主戏就没了。**」

附件也不是同一份：场景用的是内置的 48 字节 3 行 CSV
（`core/crates/testing/src/samples.rs:105`，`2026-01/02/03` 的 `120/180/90`），
演示用的是 `aite evals demo-fixture all` 生成的 **24 个月**那份
（`core/crates/evals/src/demo_fixture.rs:22-32`，落 `/tmp/aite-demo/sales.csv`）。

**所以**：你把 04 跑绿了，**不等于**演示第二幕那句话跑得起来。
而 demo 文档把「模型一步就 `final`、卡片压根没出现」列为**全片最致命的一种翻车**
（`docs/demo-3min.md:249` 的风险列 + `:135` 的「彩排必须验到这条不发生」）。

**怎么在不碰 `evals/p0/**` 的前提下验它**：`load_suite` 收的是**任意目录**
（`core/crates/evals/src/scenario.rs:509-526`：只要目录下有 `*.yaml` 就行），
所以把 04 复制到一个 worktree 外（或 `data/` 下）的目录，只改两处 ——
`events[0].text` 换成演示那句、`platform.files[0].content` 换成真 fixture 的内容 ——
然后 `aite evals run <那个目录> --model live --sandbox docker` 跑两遍。

**判据只有一个，不看 `passed`**：`--protocol-report` 里这个场景的第一步是不是 `final`。
是 `final` 就是翻车形态；出 `checklist_add`、且 `checklist_*` 计数 ≥3，就是演示能用的形态。

**边界**：这条**只做观测，不给改法**。要是两遍都翻车，那是演示台词要改（总管的事），
不是代码缺陷，**别去改任何东西**，写进报告「没测到的」上面那一节交总管。

---

### ⑨ 撞出真缺陷怎么办

**停下报告。** 代码是别的轨的文件。报告里要有：复现命令、`--protocol-report` 里的对应片段、
以及你判断它是真缺陷而不是 (a)/(b) 的理由。

**下面两条是已知的，撞上不算你的发现，别报**（`review-findings-2026-09-12-romega.md` §4.1）：

| 已知项 | 你会怎么撞上它 |
|---|---|
| `core/crates/app/src/wiring.rs:98`：`--model live` 的配置校验跑在参数解析之前 | `aite evals run evals/p0 --model live --list` 在 config 没配好时不会列场景，而是报「`--model live` 起不来」+ 退出码 2。`-h` 同样被抢先（`wiring.rs:98-100` 在 `cli::parse_args` 之前就跑了） |
| `core/crates/app/src/wiring.rs:153` 那条记的「docker 档场景收尾把容器收干净是空操作」 | 真正的空操作在 **`core/crates/edge-client/src/sandbox.rs:138-140`**：`async fn close_all(&self) -> Result<(), SandboxError> { Ok(()) }`。所以 `SandboxProbe::close()`（`core/crates/evals/src/real_stack.rs:179-181`）在 docker 档下什么也没收。**超时 / 异常收场的容器会留到 `aite-edge` 停机** —— `CloseAll`（`edge/internal/sandbox/docker.go:468`）只在 `edge/cmd/aite-edge/main.go:202` 被调。这就是「两遍之间重启 edge」那条建议的由来 |

---

## 纪律

1. **契约与锁**：`proto/**`、`core/crates/contracts/**`、`.contracts.lock` 冻结，全程 `OK 25 files`。
   要动 → 停下报告。
2. **`evals/p0/*.yaml` 十个场景是验收面，一个字不动。** 你这一轨最容易犯的就是这条 ——
   跑到一半发现某条断言在 live 下过不去，顺手改个 `contains`。**不许。** 写进 ⑤ 那张表交总管。
   ⑧ 那条对照套件必须建在 `evals/` 之外。
3. `docs/dev-spec-2026-09-09.md` 与 `docs/dev-spec-2026-09-11-rustgo.md` 冻结。
4. **一行代码都不改。** 这一轨的交付物是一份 `.md`，外加一份改不进提交的 `config/aite.yaml`。
   撞出真缺陷先停下报告。
5. **密钥红线**：只从 `AITE_MODEL_API_KEY` 读，任何日志 / 报告 / 提交 / 错误输出不得出现取值。
   四份 `--protocol-report` JSON 落在 `data/`（不入库），往报告里粘片段之前自己扫一遍。
6. **每条结论挂实测。**「应该会」「大概」「建议考虑」一句不要。
   **只跑了一遍的东西不许写成结论**，写「跑了一遍，没测到第二遍」。
   推测一律标「推测」，没测到的一律写「没测到」—— 这是那四份报告统一的体例。
7. **不靠墙钟当判据。** 上一轮刚修掉一条「两万个 tick 必须 1 秒内跑完」的假红门禁
   （台账 §2.5），你这轨的耗时数字全部是**记录**，不是判据。
8. **不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v4` 分支上，回执贴出来。
   提交里应该**只有** `evals/live-report-2026-09-<日期>-v4.md` 一个文件。
9. 六轨同时在跑：cargo 构建锁与 Docker daemon 是全机共享的，你的命令可能要排队等几分钟，
   这是正常的，别以为卡死。

---

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v4

scripts/check.sh                                        # 期望「全部通过」，退出码 0（B8 仍是 passed 10/10）
core/target/debug/aite contracts lock --check           # 期望 OK 25 files
git status --short                                      # 期望只有新报告一个文件（data/ 与 config/aite.yaml 已被 .gitignore）
git diff --stat -- evals/p0/                            # 期望无输出
ls -l data/v4/fake-1.json data/v4/fake-2.json data/v4/docker-1.json data/v4/docker-2.json
                                                        # 期望四份都在，且都不是 0 字节

# 收尾：停掉 edge（这一步才真收容器），再看增量
kill "$(cat data/v4/edge.pid)" ; sleep 2
grep -c 'edge.down' data/v4/edge.log                    # 期望 1
docker ps -a --filter label=aite.task -q | wc -l        # 期望回到你开跑前记的那个基数
```

---

## 回执格式

```
## V4 回执

基线 0bc8d55 → 提交 <短 sha>

### 开场自检
$ scripts/check.sh
<最后 3 行；期望「全部通过」+ exit 0>
契约锁 / C1 / cargo / go -race / B8 五行：<逐行贴，与派单表格对照>
$ cd edge && go test -tags docker ./internal/sandbox/... -count=1
<>
守卫自检：Read .claude/hooks/guard_bash.py → <被拦 / 没被拦>
V4 四条前置：key 长度 <> / docker <> / 镜像 <> / platform.md 字节数 <期望 3878>
开跑前容器基数：<N>

### 环境
config/aite.yaml 我从 example 复制后改了哪几处：<>
$ grep edge.takeoff data/v4/edge.log
<>
$ grep edge.platform_fake data/v4/edge.log
<>

### 成本（估算在前，实际在后）
开跑前的估算：<方法 + 数字>
四遍实测汇总：input <> / output <> / cached <>（占比 <>%）→ 按 <单价假设> 折 <> 元
差多少、为什么：<>
price_in/out_per_mtok 我填了没：<> 为什么：<>

### 五跑的 passed（含那条不花钱的对照）
scripted × docker <k/10>   ← Python 侧 9/10，Rust 侧此前没测过
fake-1 <k/10>  fake-2 <k/10>  docker-1 <k/10>  docker-2 <k/10>
（提醒：passed 不是判据，逐条见下）

### 逐条失败归类（派单④那张表的实测版）
| 场景 | 哪条断言红了 | (a)耦合台词 / (b)结构性不可测 / (c)真缺陷 | 依据 |
fake 档红的是不是就是 T23 那五条（01/03/04/07/10）：<是 / 不是，差在哪>
09_bot_ignored 四遍全绿了吗：<>  ← 金丝雀
(c) 有几条：<期望 0；非 0 的话上面必须已经停下报告过>

### checklist 那条（⑥）
稳定用 / 稳定不用 / 不稳定：<三选一，不许写「基本稳定」>
逐场景两遍的 checklist_* 计数（fake 档，对 T19 §3 那张表）：
| 场景 | T19 after（跑1/跑2） | V4 fake（跑1/跑2） | 差在哪 |
checklist_note 那一格：<Python 侧 0，Rust 侧 ?>
fail : check 的比例，fake 档 <> / docker 档 <>（T19 fake 档是 18:6，真沙箱下没测过）
03_checklist_progress 在 docker 档下 checklist_check 是真勾还是变成叉：<>
三条硬证据我复核了吗（提示词 diff / 工具描述 / RΩ §4 比错基线）：<逐条，diff 输出贴出来>
提示词渲染这个混淆项我查了吗、结论：<查清了 / 没测到>
模型版本漂移：<没测到>
对 RΩ §4 那一行的更正写在新报告的哪一节：<>

### 04 那条耦合（⑤）
我建议走哪条路（A/B/C/D）、为什么：<>
跑完之后发现的同类耦合清单：<>
它有没有改变我的倾向：<>
（提醒：我一个字都没改 evals/p0/，见 git diff --stat -- evals/p0/ 为空）

### ⑧ 演示台词那条（加分项）
做了没：<做了 / 没做，为什么>
做了的话：两遍第一步出的是 final 还是 checklist_add：<>  checklist_* 计数：<>
结论：演示那句台词是能用的形态还是翻车形态：<>

### 新报告
路径：evals/live-report-2026-09-<日期>-v4.md
六节齐了吗：<>
「没测到的」那一节列了几条：<>

### 实测输出（粘实际的）
$ git status --short
<>
$ git diff --stat -- evals/p0/
<期望无输出>
$ docker ps -a --filter label=aite.task -q | wc -l
<收尾后，对比开跑前的基数>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v4` 分支上，回执贴出来。
