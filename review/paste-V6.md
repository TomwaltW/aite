# 任务 V6 — 评测 CLI 的顺序问题与门禁卫生

## 背景：这轨是从哪来的

Aite 的 P0 从 2026-09-11 起用 **Rust(core) + Go(edge)** 重写：R0 骨架 → R1–R7 七轨并行 →
RΩ 组装起飞（`aite run` / `aite preflight` / 评测接线 / compose 双服务）并删掉整棵 Python 树
（178 个文件、−33272 行）。RΩ 已经在 `11322b3` 合入 main。

合并前做了一轮 34 个 agent 的审核（十个面各一个 agent 对照 Python 原版读代码，每条结论再交
一个独立 agent 试图证伪），确认 22 条、证伪 2 条、low 26 条。全文在
`review/review-findings-2026-09-12-romega.md`。**你这一轨的活全部出自它的第四节「记账给下一批」。**

`docs/dev-spec-2026-09-09.md`（P0 的权威文档）里还没兑现的只剩两件：§2.4 的 **M1–M6 真机验收**
（真实飞书测试群，总管亲自跑）和「**能录一段 3 分钟演示**」。§1 又明令禁止「任何『顺手做一点 P1』
的行为」。所以这一批六轨的共同定位是：**把 P0 收尾到「总管可以坐下来跑 M1–M6、可以开录」，
外加把审核留下的账清掉。** 不加功能。

你这一轨要解决的是两件互不相干、但都属于「门禁卫生」的事：

**一、`aite evals` 的命令行在「配置没就绪」时用错误的优先级把人挡在门外。**
`main.rs:83` 在调 `cli::run_with_wiring` **之前**先无条件调一次 `wiring::evals_wiring(&args)`，
后者一见 argv 里有 `--model live`（或 `--sandbox docker`）就立刻 `load_config` + 试造，
失败即 `eprintln` + 退出 2。于是在一台没配好模型的机器上，`--list` 列不出场景名、`-h` 打不出用法、
连「`--platform` 拼错了」这种纯参数错误都被模型抢了先。更难看的是：RΩ **在同一次改动里**刚按
台账把 `-h` 从「stderr + exit 2」修成「stdout + exit 0」，而这条修复只要命令行上同时有
`--model live` 就当场失效 —— 而唯一钉住它的测试 `core/crates/evals/tests/cli_help.rs` 走的是
`run_capture` 直调、绕过 `main.rs` 这一层，所以测试全绿也看不见。**这是一条测试覆盖不到的
位置上的回归。**

**二、守卫是这个仓库唯一的冻结面机器强制，而它现在没人看着，并且此刻正在失效。**
`.claude/hooks/guard_bash.py`（368 行）挂在 `PreToolUse` 上。它唯一的回归测试
`tests/contracts/test_guard.py`（140 行）随 Python 树一起删了，**没有任何人接管**；
它的保护面里还留着已经不存在的 `aite/contracts/**`。而 2026-09-12 总管这边真踩过一次：
会话在 `~/Documents/Aite/aite`（正要被删的 Python 包目录）里起，`CLAUDE_PROJECT_DIR` 就定死在
那个子目录，hook 命令 `python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"` 找不到文件 →
执行失败 → **非阻塞放行、不报警** → 整场守卫静默失效。

**这不是历史事故，是此刻正在发生的事。** 写这份派单的会话 cwd 就是主仓根，Read
`.claude/hooks/guard_bash.py` 和 Read `.contracts.lock` **两条都没被拦、内容原样返回**；
而同一台机上把一模一样的 payload 直接喂给守卫脚本，两条都 `exit=2`（下面 ③ 有完整实测表）。
**守卫脚本本身是好的，挂载是坏的。** 一个有洞的守卫比没有守卫更危险，因为它给人
「契约有人看着」的错觉。

---

## 必读（按顺序）

| # | 文件 | 看哪一节 | 为什么 |
|---|---|---|---|
| 1 | `review/review-findings-2026-09-12-romega.md` | **§4.1 medium 表的第 5、6 行**（`wiring.rs:98` / `wiring.rs:153`）、**§4.4 low 表**（`cli.rs:47` / `cli.rs:52` / `cli.rs:108` / `demo_fixture.rs:421` / `cli_smoke.rs:1`）、**§六**（守卫静默失效那一节） | 你这一轨的四件活全部出自这里。§六是第 ③ 件活的起因 |
| 2 | `review/paste-ROMEGA.md` | §要做什么 ③ 的「四条已经替你堵好的坑」（第 121–126 行） | 其中「**`ModelFactory` 要在起飞前就试造一次**」和「**`preflight` 必须与 `sandbox` 成对注入**」是你改 ① 时**绝不能改回去**的两条 |
| 3 | `core/crates/evals/src/cli.rs` | `run_suite_cmd`（第 267–394 行）整段 | 这是 Python `aite/evals/__main__.py` 的顺序在 Rust 侧的落点；① 的改法就是把 `main.rs` 抢跑的那两件事搬回这里的对应位置 |
| 4 | `.claude/hooks/guard_bash.py` | 全文，重点保护面（第 24–84 行）、`check_segment` 的位置判定（第 265–313 行）与 `main()`（第 332–351 行） | ③ 要动它。**Read 这个文件本身应该被守卫拦**（见「第 1 步」），所以正文用 `cat -n` / `sed -n` 读 —— `cat` 在 `READ_SAFE`（`guard_bash.py:48`）里 |
| 5 | `review/review-findings-2026-09-11.md` | 第三节「记账给 RΩ」里 R7 的两条（`--help` 走 stderr + exit 2；`preserve_order`） | 上一轮台账。`--help` 那条就是 ① 里被打回原形的那条；`preserve_order` 已经在本轮 §4.3 销账，**别再捡起来** |

---

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-v6
分支     : task-v6
基线     : 0bc8d55（worktree 已经建好并钉在这个 sha 上）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、go 1.27.1（/opt/homebrew/bin/go）、
           protoc 36.1、Docker 29.6.1
           python3 3.11.7（/Library/Frameworks/Python.framework）—— 本机**没有** `python` 命令
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。

> **基线说明，先看这条。** RΩ 合入 main 是 `11322b3`。写这批派单的当口 main 又前进了一格到
> **`0bc8d55`**（`fix(rustgo): check.sh 的 B 段先打失败测试名再计数，并收窄台账里 Answering 那条`），
> 六个 worktree 全都钉在 `0bc8d55`。这一格只动了两个文件（`scripts/check.sh` 的 B 行、
> 台账 md），`git diff --stat 11322b3..0bc8d55` = `2 files changed, 11 insertions(+), 2 deletions(-)`，
> **不碰你这一轨的任何代码面**，下面所有期望值在 `0bc8d55` 上原样成立。
> 台账正文里凡是写「11322b3」的地方，指的是 RΩ 那次合入，**不是你的基线**。

`config/aite.yaml` 不入库（`.gitignore:10`），**你这个 worktree 里没有它**
（`config/` 下只有 `aite.example.yaml`）。这正好是 ① 的天然复现环境，**别去补一份** ——
补了 ① 的所有实测就都复现不出来了。（顺带：主仓根**有**一份 `config/aite.yaml`，
所以同样的命令在主仓跑出来的结果跟 worktree 不一样，别拿主仓的输出当对照。）

---

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v6
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
>
> **另一条**：`cargo passed` 偶尔会出 `717/1`。`0bc8d55` 起 check.sh 会把失败的测试名打出来
> （`error: test failed, to rerun pass …`，`scripts/check.sh:37`）—— 真撞到了，
> **把那一行原样贴进回执**。上一次撞到时现场没留下，之后连跑六遍复现不出来。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来**，报
> ```
> blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。
> ```
> （这行文案出自 `guard_bash.py:360–361`，`（读取位置）`来自 `_check_one` 的 `:173`。）
> 没被拦说明 `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
>
> **这条对你这一轨是生死线**：③ 就是在修这个病。你要是在一个守卫已经失效的会话里改守卫，
> 改完也验不出来，而且你会以为自己验过了。**写这份派单的那个会话就没被拦**（见 (3c) 的现场证据），
> 所以这不是走过场。
>
> 被拦之后，正文用 `cat -n .claude/hooks/guard_bash.py` 或 `sed -n '1,80p' …` 读。

---

## 可写路径

**能写：**

| 路径 | 备注 |
|---|---|
| `core/crates/app/src/cli.rs` | ①（部分）与 ④ 的 a/b/c |
| `core/crates/app/src/wiring.rs` | ① 与 ② 的主战场 |
| `core/crates/app/tests/cli_smoke.rs` | ① 的回归测试只能钉在这里（它起的是真进程，走 `main.rs`） |
| `core/crates/app/tests/guard.rs` | **本派单显式放行的唯一一个新增文件**，给 ③ 的测试接管用（详见 (3b)）。除它之外不新增文件 |
| `core/crates/evals/**` | ①（`src/cli.rs` 的顺序）、②（可能落在 `src/real_stack.rs`）与 ④d（`src/demo_fixture.rs`）。整个 crate 归你 —— 但注意 `evals/p0/**` 是仓库根下的场景目录、**不属于这个 crate**，见下面「一个字不许动」 |
| `.claude/hooks/**` | ③。**注意下面「要走授权」** |
| `.claude/settings.json` | ③。**注意下面「要走授权」** |

**要走授权（开工前就得问清，别改到一半才发现）：**

`.claude/hooks/guard_bash.py` 与 `.claude/settings.json` **本身就在守卫的保护面里**
（`guard_bash.py:31–39` 的 `PROT_PATHS`，命中时 `readable=False` —— 读和写都不许，
见 `hits()` 的 `:136–141` 与 `_check_one` 的 `:171–173`）。守卫只在 `AITE_RELOCK=1` 时整体放行
（`guard_bash.py:355–356`：进 `main()` 之前第一件事就 `sys.exit(0)`），而它同时禁止在 Bash 命令里
给 `AITE_RELOCK` 赋值（`:78–79` 定义 `RELOCK_WRITE_RE`、`:318–319` 抛拦截；实测
`AITE_RELOCK=1 echo hi` → `exit=2`）。所以你**没法自己给自己授权**。两条路，都要总管点头：

- **① 总管用 `AITE_RELOCK=1 claude "$(cat review/paste-V6.md)"` 重起这个会话** ——
  代价是**守卫整场全放行**，契约面从此只靠自律 + `contracts lock --check` 兜底；
- **② 总管自己动那两个文件**，你只出改法与实测。

另外 `.claude/settings.json:13–14` 有 `"Edit(/.claude/hooks/guard_bash.py)"` 和
`"Edit(/.claude/settings.json)"` 两条 deny 规则，那是 Claude Code 的权限系统、跟守卫是两回事，
`AITE_RELOCK` 管不着它。

**开场自检跑完、确认守卫真的挂上之后，第一件事就是把这个授权问题报给总管，然后先做 ①②④
（它们一个字都不碰 `.claude/`），③ 等授权到位再做。** 别卡在这里空转。

**一个字不许动：**

- `proto/**`、`core/crates/contracts/**`、`.contracts.lock` —— 契约冻结，全程 `OK 25 files`
- `evals/p0/*.yaml` —— 十个场景是验收面
- `docs/dev-spec-2026-09-09.md`、`docs/dev-spec-2026-09-11-rustgo.md` —— 冻结
- `core/crates/app/src/app.rs`、`core/crates/app/src/run.rs` —— **归 V5**
- `core/crates/app/src/preflight.rs` —— **归 V2**
- `scripts/check.sh` —— 不在你的可写面里。(3b) 选路 1 时会撞上它，见那一条
- `core/crates/app/src/main.rs` —— 归 R0（`main.rs:1–2` 写着「各轨落地时不需要动这个文件；
  要加子命令 → 停下报告」）。①(c) 和 ④b(ii) 都会撞上它，见各自那一条

---

## 要做什么

### ① `--model live` / `--sandbox docker` 的校验跑在参数解析之前

**病在哪。** `core/crates/app/src/main.rs:83`：

```rust
Cmd::Evals { args } => match aite_app::wiring::evals_wiring(&args) {
    Ok(wiring) => aite_evals::cli::run_with_wiring(args, &wiring),
    Err(e) => { eprintln!("{e}"); 2 }
},
```

`evals_wiring`（`wiring.rs:87`）自己 peek argv 拿 `--config` / `--model` / `--sandbox`
（`wiring.rs:93–96`），然后：

- `wiring.rs:98–100`：`--model live` → `live_model_factory(&config_path)` → `wiring.rs:131`
  `load_config` + `wiring.rs:134` `OpenAiCompatModel::from_config`，任一失败就 `Err`；
- `wiring.rs:101–120`：`--sandbox docker` → `wiring.rs:102` `load_config` + `wiring.rs:109`
  `EdgeClient::connect`，任一失败就 `Err`。

这两件 I/O 全都发生在 `run_suite_cmd`（`evals/src/cli.rs:267`）的 `parse_args`（`:269`）之前。

**怎么验证它确实病着。** 下面这张表是 **2026-09-12 在 `0bc8d55` 的 task-v6 worktree 里
实跑出来的**（worktree 没有 `config/aite.yaml`，所以不用给 `--config`；用主仓已编好的二进制跑
也行：`B=/Users/shensikai/Documents/Aite/core/target/debug/aite`）。

| 命令（在 worktree 根跑） | 现在的输出 | 退出码 | 该有的 |
|---|---|---|---|
| `aite evals run evals/p0 --list --model live` | `--model live 起不来：配置文件不存在：config/aite.yaml（可从 config/aite.example.yaml 复制）` | 2 | 十个场景名的 JSON 数组，0 |
| `aite evals run -h --model live` | 同上 | 2 | 用法进 **stdout**，stderr 为空，0 |
| `aite evals run evals/p0 --platform feishu --model live` | 同上 | 2 | `不认识的 --platform "feishu"，只认 ["fake"]`，2 |
| `aite evals run evals/nope --model live` | 同上 | 2 | `场景加载失败：场景目录不存在：evals/nope`，2 |
| `aite evals run evals/p0 --only nosuch --model live` | 同上 | 2 | `没有这些场景：["nosuch"]`，2 |
| `aite evals run evals/p0 --list --sandbox docker` | `--sandbox docker 起不来：配置文件不存在：config/aite.yaml（可从 config/aite.example.yaml 复制）（真沙箱在 edge，要读 config 的 edge: 段）` | 2 | 十个场景名的 JSON 数组，0 |
| `aite evals run -h --sandbox docker` | 同上 | 2 | 用法进 stdout，0 |

**对照组**（同一个 worktree、同一台机，只把 `--model live` 换成 `--model scripted`）——
证明「坏配置」跟上面每一件事本来都无关：

```
$ aite evals run evals/p0 --list --model scripted            → ["01_simple_qa", "02_thread_followup", …]  exit=0
$ aite evals run evals/p0 --platform feishu --model scripted → 不认识的 --platform "feishu"，只认 ["fake"]  exit=2
$ aite evals run evals/nope --model scripted                 → 场景加载失败：场景目录不存在：evals/nope    exit=2
$ aite evals run evals/p0 --only nosuch --model scripted      → 没有这些场景：["nosuch"]                    exit=2
$ aite evals run -h                                           → 用法：…（stdout，stderr 为空）              exit=0
```

> **一条要留神的观测**：在**主仓**（有 `config/aite.yaml`、`aite-edge` 没起）跑
> `aite evals run evals/p0 --list --sandbox docker` 是 **exit=0、场景名照列** ——
> 说明 `EdgeClient::connect`（`wiring.rs:109`）是**惰性**的，daemon 不在照样「连得上」。
> 所以 docker 那一档在 worktree 里真正炸的是 `load_config`（`wiring.rs:102`）那一发。
> 别拿主仓的输出当基准，一律在 worktree 里跑。

**为什么测试看不见。** `core/crates/evals/tests/cli_help.rs:6,9` 用的是
`aite_evals::cli::run_capture(args, &Wiring::default())` —— 直调库函数，`main.rs` 那一层
（以及 `evals_wiring`）根本不在调用链上。三条用例全绿，但它们验的是「`run_suite_cmd`
内部的 `-h` 处理对不对」，不是「`aite evals run -h` 这条命令对不对」。

**Python 原版的顺序**（`aite/evals/__main__.py`，见 `review/inventory-gateway-evals.md`）：

```
parse → load_suite → --only → --list（return 0）→ docker 体检 → scale → _live_model_factory
```

live 校验排在**最后**。Rust 侧 `run_suite_cmd` 里这个顺序**已经是对的**：

| Python 步 | Rust 落点 |
|---|---|
| parse | `evals/src/cli.rs:269`（`parse_args`；`-h` 在 `:272–278` 返回 stdout + 0） |
| load_suite | `evals/src/cli.rs:282` |
| `--only` | `evals/src/cli.rs:287–299` |
| `--list` → return 0 | `evals/src/cli.rs:301–306` |
| docker 体检 | `evals/src/cli.rs:308–334`（成对注入检查在 `:318`，探针在 `:324`） |
| scale | `evals/src/cli.rs:336–347` |
| `_live_model_factory` | `evals/src/cli.rs:349–361` |

**所以病不在 `cli.rs`，在 `main.rs` 那一层抢跑。**

**复核明确否掉了「加个 peek_only 白名单」的快手改法**（就是在 `evals_wiring` 里 peek 一下
argv 有没有 `--list` / `-h`，有就直接返回空 Wiring）。三个缺口，去台账里看原文；
简单说就是：白名单永远补不齐（`--only` 写错、suite 路径写错、`--platform` 拼错都不在名单上，
上面那张表里有三条正是这种），而且它把「哪些参数该短路」这个知识复制到了第二处，
两处迟早不一致。

**改法方向（不定死，你选）。** 硬约束先摆出来：

| 约束 | 出处 | 破坏它的后果 |
|---|---|---|
| `evals_wiring` 在 parse 之前**一件 I/O 都不做** | 本条的病根 | 改了个寂寞 |
| `ModelFactory` **必须仍然在跑场景之前试造一次** | `paste-ROMEGA.md:124`、`wiring.rs:124–129` 的注释 | 配置缺一样会变成十个场景各自跑到第一次 chat 才抛，被 worker 当模型 5xx 白重试 2 次（2s + 5s），十次 7 秒空等，真正的原因一个字看不到 |
| `preflight` 与 `sandbox` **仍然成对**，`evals/src/cli.rs:318` 那条拒绝还要能触发 | `paste-ROMEGA.md:123`、`wiring.rs:117` 的注释 | daemon 没起时十个场景各自烂在第一个工具调用上 |
| scripted 路径的 **stderr 必须一个字节都没有** | `cli_smoke.rs:144–148`、`scripts/check.sh:45` | B8 硬门禁当场红 |
| B8 仍然 `passed 10/10`、退出码 0 | `scripts/check.sh:43–45` | 同上 |

在这些约束下，至少有这么几条路：

- **(a) 让 `Wiring` 的两个档都变成「惰性」**：`evals_wiring` 只装闭包（捕获 `config_path`
  这个 `String`，不做 I/O），真正的 `load_config` / `EdgeClient::connect` /
  `OpenAiCompatModel::from_config` 推迟到闭包**第一次被调用**时。`--model live` 那一档天然
  合拍 —— `evals/src/cli.rs:349–361` 拿到 factory 之后**多调一次并丢弃结果**，就是「起飞前
  试造一次」，位置正好是 Python 的 `_live_model_factory`。docker 那一档让
  `docker_sandbox_factory`（`wiring.rs:151`）与 `docker_probe`（`wiring.rs:171`）共享一个
  `OnceLock<Arc<EdgeClient>>`，连接发生在 `evals/src/cli.rs:324` 的 `docker_preflight` 里 ——
  也正好是 Python 的位置。
  代价：live 档 `load_config` 会多读一次盘；错误消息的前缀要小心别叠成
  「`--sandbox docker 起不来：--sandbox docker 起不来：…`」。
- **(b) 给 `Wiring` 加一个显式的 `validate` 钩子**（`Option<Arc<dyn Fn() -> Result<(), String>>>`），
  由 `evals/src/cli.rs` 在 Python 对应位置调用。比 (a) 直白，代价是 `Wiring` 多一个字段，
  且「验过了」和「造出来了」是两件事，可能验完还得再造一次。
- **(c) 把 argv 解析整个提前**：让 `main.rs` 先拿到解析结果再决定要不要接线。**这条要改
  `main.rs`，而 `main.rs` 不在你的可写面里**（归 R0）。要走这条得先停下问总管。

**总管的倾向是 (a)** —— 它不动 `main.rs`、不加公共字段，而且「惰性构造 + 在 Python 的位置
强制求值一次」这个形状本身就把顺序钉住了。但如果你在实现里发现 (a) 的错误消息拼不干净、
或者 `OnceLock` 那层让 `--sandbox docker` 的失败诊断变糊，选 (b) 也行。**回执里说清选了哪条、
为什么。**

**回归测试钉在哪。** 必须是**进程级**的，即 `core/crates/app/tests/cli_smoke.rs`
（它用 `Command::new(env!("CARGO_BIN_EXE_aite"))`（`:15–17`）起真进程、`current_dir(repo_root())`
（`:19–21`）走仓库根，所以走 `main.rs`）。往 `core/crates/evals/tests/cli_help.rs` 里加是无效的 ——
那正是让这条回归躲过去的地方。至少钉住四条：

1. `--list --model live` 出十个场景名、退出码 0；
2. `-h --model live` 进 stdout、stderr 为空、退出码 0；
3. `--platform feishu --model live` 报的是 `--platform` 而不是模型；
4. **反向那条**：`--only 01_simple_qa --model live --config <坏路径>` 仍然必须报
   「`--model live` 起不来」+ 退出 2 —— 把「起飞前试造一次」保住。没有它，你很容易把 ①
   改成「live 校验彻底不做了」，而前三条会一起变绿。

> **注意**：`cli_smoke.rs` 跑在仓库根（`repo_root()` = `CARGO_MANIFEST_DIR/../../..`），
> 而**主仓根有 `config/aite.yaml`**。所以测试里一律显式给 `--config no/such/aite.yaml`，
> 别依赖「默认配置不存在」—— 否则它在你的 worktree 里绿、在主仓里行为不同，
> 就是又一条环境相关的假绿。

改完要能说出「把被测行为破坏掉，这条会不会红」，并把**破坏 → 红、还原 → 绿**两次输出贴进回执。

---

### ② `--sandbox docker` 档「场景收尾把容器收干净」是空操作

**病在哪。** 台账指的是 `core/crates/app/src/wiring.rs:153`，那一行是
`let probe = Arc::new(SandboxProbe::new(edge.sandbox()));` —— **它是接线点，不是空操作本身**。
真正的空操作在 `core/crates/edge-client/src/sandbox.rs:137–140`：

```rust
/// 本地 no-op：沙箱记账在 edge（进程收尾时 edge 自己收容器），core 这边没有要释放的东西。
async fn close_all(&self) -> Result<(), SandboxError> {
    Ok(())
}
```

完整链路（每一环都核过）：

```
runner.rs:411–412  deps.close().await                  ← 每个场景收尾都调
  → deps.rs:105–107        Deps::close → self.sandbox.close()
  → real_stack.rs:178–181  SandboxProbe::close → SandboxPort::close_all(self)
  → real_stack.rs:162–165  记一笔 "close_all" 到 CallLog，转发给 inner
  → edge-client/src/sandbox.rs:137–140  Ok(())   ← 什么都没做
```

于是：异常收场（超时被取消、场景 panic、plane 硬取消）时 worker 没走到自己的 release，
容器就一直占着 `cpu: 1` / `mem_mb: 1024` 活到 `aite-edge` 停机。十个场景连跑会叠加 ——
**而 V4 那一轨正要连跑十个场景 × 两档 × 两遍**。

**怎么验证它确实病着。** 需要 `aite-edge` 在跑（`platform: fake` 就够，不需要飞书凭证）
和 `aite-sandbox:p0` 镜像。造一个异常收场，然后数容器：

```bash
# 大意如此，具体参数你自己调到能稳定复现
core/target/debug/aite evals run evals/p0 --only 04_csv_to_chart --sandbox docker --timeout-scale 0.05
docker ps -a --filter label=aite.task -q | wc -l     # 病着时 > 0
```

**六轨并行时这条命令会数到别人的容器。** 别用光秃秃的 `label=aite.task` 下判断 ——
用 `--filter label=aite.task=<你这次的具体 task_id>`，或者跟总管确认此刻没有别的轨在跑 docker 档。
造完残局**自己 `docker rm -f` 收干净**再交，验收那条 `wc -l` 期望 0。

**复核给了两条实施约束，必须遵守。**

1. **适配器要裹在 `edge.sandbox()` 上 —— 在探针的下面，绝不能裹在探针之上。**
   理由是硬的：`SandboxProbe`（它的 `SandboxPort` impl 在 `real_stack.rs:80–166`）每个方法
   都往 `CallLog` 记一笔。适配器如果在探针**外面**，它收尾时发的那几发 release 会经过探针 →
   被记进 `deps.sandbox.calls()`。而 `deps.close()`（`runner.rs:412`）跑在 `run_checks()`
   （`runner.rs:427`）**之前** —— 断言读到的就是被污染过的计数。受影响的面：
   - `evals/p0/07_commands.yaml:83` `{check: sandbox_calls, method: release, min: 1}`
     和 `:90` `{check: sandbox_calls, method: acquire, min: 1}`
     （`checks.rs:474–481` → `compare`，`checks.rs:77–99` 支持 `equals` / `min` / `max`；
     `min` 这次侥幸挡得住，换成 `equals` 就当场破）；
   - `deps.rs:124` `"sandbox_calls" => json!(self.sandbox.calls().len())` ——
     这个数进每个场景的 JSON 摘要，V4 拿它对基线时会莫名其妙多出来几笔。

   裹在下面就没这个问题：适配器直接对着 `EdgeSandbox` 发 release，一笔都不进 `CallLog`。
2. **收尾失败不许盖掉场景结论。** release 失败最多是 `tracing::warn!`，
   不能变成 `PhaseError`、不能改 `ScenarioResult.passed`。一个「收尾没收干净」的警告
   不该把一个本来通过的场景判成失败。

**顺带一条对你有利的事实**：edge 侧的 `Release` 是幂等的
（`edge/internal/sandbox/docker.go:381–402`，第 381 行注释就写着「Release 幂等：不认识的 id、
已经没了的容器，都当成已经释放」，`:393–394` 的 `IsErrNotFound` 直接 `return nil`）。
所以适配器重复 release 一个已经被 worker 正常释放掉的 id 是安全的，不用去做精细的
「谁还活着」记账。

**改法方向留给你**：最直白的是一个记账用的透明包装（`acquire` 记下 id、`release` 划掉、
`close_all` 把剩下的逐个 release），塞在 `wiring.rs:153` 的 `edge.sandbox()` 与
`SandboxProbe::new` 之间。它放 `wiring.rs` 里还是放 `core/crates/evals/src/real_stack.rs`
（跟 `SandboxProbe` 做邻居）你定，两处都在你的可写面里。**回执里说清放在哪、为什么。**

**要能说出「破坏掉会不会红」。** 这一档没有自动化门禁（docker 档不在 check.sh 里），
所以这条的实测就是：**造残局 → 数容器 > 0 → 接上适配器 → 同样的残局 → 数容器 = 0**，
两次输出都贴。顺带跑一遍 `--sandbox docker` 的正常路径，确认 `07_commands` 的两条
`sandbox_calls` 断言没被你污染。

---

### ③ 守卫接管

三件事：清死账、接管测试、让静默失效变得可见。

#### (3a) 清掉已删的保护面

逐条核过的死引用（行号是 `0bc8d55` 上的，我逐个打开文件对过）：

| 位置 | 内容 | 状态 |
|---|---|---|
| `.claude/hooks/guard_bash.py:4` | 文档串里的 `aite/contracts/**（Python 旧契约）` | 死 |
| `guard_bash.py:27` | `PROT_PREFIXES = ("aite/contracts/", "proto/", "core/crates/contracts/")` | 第一项死 |
| `guard_bash.py:41–42` | `PROBES = ["aite/contracts/__init__.py", "docs/dev-spec-2026-09-09.md", "proto/aite/v1/events.proto", "core/crates/contracts/src/lib.rs"]` | 第一项指向已删文件。剩下三个探针都还在，所以通配符反向匹配（`hits()` 的 `:144–150`）这条功能没瘫，只是少了一个探针 |
| `guard_bash.py:52` | `READ_SAFE` 里的 `"pytest"` | 死（仓库里没有 pytest 了） |
| `guard_bash.py:61` | `INTERPRETERS` 里的 `"python"` / `"python3.12"` | 死。**`"python3"` 必须留着** —— 守卫自己就是 python3 脚本，而 `python3 <脚本>` 要按写入/执行位置判 |
| `guard_bash.py:66` | `REWRITERS = {"ruff","black","isort","autopep8","yapf","docformatter"}` | 六个工具全死。活着的等价物是 `cargo fmt`（已在 `:285–287` 单独拦，实测 `cargo fmt` → exit=2、`cargo fmt --check` 放行） |
| `guard_bash.py:72` | `RELOCK_MODULE_RE` 的第一个分支 `aite[./]contracts[./]lock\b` | 死。第二个分支 `contracts\s+lock\b` 覆盖 `aite contracts lock --write`，**活着，实测 exit=2，别动** |
| `guard_bash.py:281` | `raise Blocked("aite/contracts/**", f"全树重写工具（{prog} 写模式）")` | 拦截**消息**指着已删路径（拦截本身还在生效） |
| `guard_bash.py:300` | `raise Blocked("aite/contracts/**", "find 的 -delete/-exec 覆盖面判不出来")` | 同上 |
| `.claude/settings.json:4` | `"Edit(/aite/contracts/**)"` | 死 deny 规则 |

**关于 `gofmt -w`：** 实测 `gofmt -w .` → **exit=0（放行）**、`gofmt -w edge/go.mod` → **exit=2**。
前者放行是对的：gofmt 只改 `*.go`，而受保护面里一个 `.go` 都没有（契约是 Rust/proto；
`edge/go.mod` / `go.sum` 不归 gofmt 管，而真被点名了就会被 `PROT_PATHS` 拦住）。
**判断是不用给 gofmt 补 `REWRITERS` 条目**；你不同意就在回执里说理由。

**台账 §4.4 里 `.gitignore:14` 和 `.gitignore:1` 那两条已经不成立**：`.gitignore` 在 RΩ 的
§2.5 里清干净了（现行 19 行，一条 Python 条目都没有，我逐行核过）。`.gitignore:14` 那条的
**描述**「守卫的保护面/探针还指着已删的 aite/contracts」是真的，但**位置写错了** ——
它的真身就是上面这张表。**回执里把这两条标成「已在 RΩ 完成 / 台账位置写错，真身在 (3a) 表里」。**

顺带：`.claude/settings.json` 的 deny 列表用的是 `Edit(/…)` 这种带前导斜杠的写法。
删掉的 `test_guard.py:133–140` 曾经断言「规则必须以 `Edit(` / `Read(` 开头」（理由：
Claude Code 只对 `Edit(path)` / `Read(path)` 做文件权限匹配，`Write(path)` / `NotebookEdit(path)`
会被接受但从不查询）。**但前导斜杠的匹配语义没人验过。** 清死条目的时候顺手实测一下这些规则
到底匹不匹配，结论写进回执。真要是全都不匹配，那守卫 hook 就是唯一的强制，这件事总管需要知道。

#### (3b) 测试接管

被删的 `tests/contracts/test_guard.py` 是 140 行、pytest 参数化（我从 `598f476^` 里调出来
逐行看过）。覆盖面：

- **14 条**「必须拦下的写入姿态」：重定向覆盖、`sed -i`、通配符、命令替换藏路径、变量回填、
  `git checkout --`、heredoc、全树重写工具两种（`ruff format .` / `ruff check --fix .`）、
  **APFS 大小写不敏感**、`xargs` 管道、`key=value` 藏路径（`dd of=`）、`find -delete`、
  点分模块 relock；
- **5 条**「必须拦下的工具调用」：Write 契约 / Edit 锁 / Write 冻结 spec / **Read 守卫自身** / Read 锁；
- **6 条**「必须放行的日常操作」+ **2 条**（Read 契约与 spec 必须放行、`Glob(**/*.py)` 必须放行）；
- **2 条 hook 接线断言**：`test_hook_command_uses_python3_not_python`（hook 命令第一个词必须是
  `python3` 而不是 `python`）、`test_deny_rules_use_only_edit_prefix`。

后面这两条尤其值钱 —— 「hook 命令写成 `python` 会 command not found，而 hook 失败是
非阻塞放行且不报警」这条纪律，现在全仓没有任何东西钉着。

**好消息：这 14+5 条几乎全都能一一对应地搬到 Rust/proto 面上，而且我已经实跑验过了。**
下面每一行都是 `printf '<payload>' | python3 .claude/hooks/guard_bash.py` 的**真实退出码**
（2026-09-12，`0bc8d55`，主仓根）：

| 旧用例（Python 面） | 换成 | 实测 exit |
|---|---|---|
| Read 守卫自身 | 同 | **2** |
| Read `.contracts.lock` | 同 | **2** |
| Write `aite/contracts/events.py` | Write `core/crates/contracts/src/lib.rs` | **2** |
| （新增）Write proto | Write `proto/aite/v1/events.proto` | **2** |
| Write 冻结 spec | Write `docs/dev-spec-2026-09-09.md` | **2** |
| （新增）Edit hook 配置 | Edit `.claude/settings.json` | **2** |
| 重定向覆盖 | `echo x > core/crates/contracts/src/lib.rs` | **2** |
| `sed -i` | `sed -i "" s/a/b/ core/crates/contracts/src/lib.rs` | **2** |
| 通配符 | `rm core/crates/contracts/src/*.rs` | **2** |
| 命令替换藏路径 | `cat $(echo proto/aite/v1/events.proto) > /tmp/x` | **2** |
| 变量回填 | `F=core/crates/contracts/src/lib.rs; rm $F` | **2** |
| `git checkout --` | `git checkout HEAD -- proto/aite/v1/events.proto` | **2** |
| heredoc | `cat > core/crates/contracts/src/x.rs <<EOF …` | **2** |
| APFS 大小写 | `echo x > core/crates/Contracts/src/lib.rs` | **2** |
| `xargs` 管道 | `echo core/crates/contracts/src/lib.rs \| xargs rm` | **2** |
| `key=value` 藏路径 | `dd if=/dev/zero of=proto/aite/v1/events.proto` | **2** |
| `find -delete` | `find . -name "*.rs" -delete` | **2** |
| 全树重写工具 | `ruff format .`（工具已死、拦截仍在）／活着的等价物 `cargo fmt` | **2 / 2** |
| 点分模块 relock | `aite contracts lock --write` | **2** |
| （新增）授权变量赋值 | `AITE_RELOCK=1 echo hi` | **2** |
| **必须放行**：Read 新契约 | `core/crates/contracts/src/lib.rs` | **0** |
| **必须放行**：Read 冻结 spec | `docs/dev-spec-2026-09-09.md` | **0** |
| **必须放行**：`lock --check` | `aite contracts lock --check` | **0** |
| **必须放行**：Glob 全树 | `Glob(**/*.rs)` | **0** |
| **必须放行**：跑测试 | `cargo test --workspace` | **0** |
| **必须放行**：写自己轨的文件 | `echo x > core/crates/app/src/cli.rs` | **0** |
| **必须放行**：`gofmt -w .` | 同 | **0** |

**所以「守卫脚本本身好使」是有实测背书的 —— 缺的只是把这套变成会自动跑的门禁。**
（旧用例里 `pytest tests/contracts -q` 和 `ruff check .` 两条放行用例随 Python 树一起作废，
不用搬；`python3 -m aite.contracts.lock --check` 换成 `aite contracts lock --check`。）

守卫本身是 python3 脚本（本机只有 `python3`，没有 `python`），但 Python 已经不是这个仓库的
语言。接管方式三条路，代价都摆出来：

| 路 | 怎么做 | 好处 | 代价 |
|---|---|---|---|
| **1. 最小 python3 自测** | `.claude/hooks/test_guard.py`，只用 stdlib（`unittest` 或裸 `assert`），旧那 140 行去掉 pytest 与 `from aite.contracts.lock import REPO_ROOT` 几乎能原样搬 | 改动最小；被测对象与测试同语言同解释器，验的就是真正会执行守卫的那个 python3 | **挂不上任何现有门禁** —— `cargo test` / `go test` 都跑不到它，要进 check.sh，而 **`scripts/check.sh` 不在你的可写面里**，得再要一次授权。不挂门禁 = 写了等于没写 |
| **2. Rust 集成测试** | 新增 `core/crates/app/tests/guard.rs`，用 `std::process::Command` 起 `python3 .claude/hooks/guard_bash.py`、喂 JSON payload、断言退出码 2 / 0。`repo_root()` 抄 `cli_smoke.rs:19–21` 那三行 | 自动进 `cargo test --workspace` → check.sh 的 B 行 → CI，**一行 check.sh 都不用改**；起跑线 718 会变大，回执里说清多了几条 | 一个 Rust crate 依赖外部 `python3`。没有 python3 的机器要 skip，**而 skip 逻辑写歪了就是又一条恒真测试**（这一轮台账刚抓到三条恒真断言，别再造第四条） |
| **3. Go 测试** | 塞进 `edge/internal/…` | —— | 守卫是仓库根的事，跟 edge 毫无关系，还会让 `go test ./...` 依赖 python3。**总管排除这条** |

**总管的倾向是 2**，主要就为「不用改 check.sh、自动进现有门禁」这一条。
如果你选 2，skip 那件事要处理干净：本机 python3 一定在（3.11.7，已核，
`/Library/Frameworks/Python.framework/Versions/3.11/bin/python3`），
所以**本机不许 skip** —— 要么干脆断言 python3 存在，要么 skip 分支里打印明确原因并让
「本机跑到了 skip」这件事一眼看得出来。回执里要证明测试不是空跑：
**把守卫的某条保护面拆掉 → 测试红；还原 → 绿**，两次输出都贴。

选 1 也不是不行（比如你能论证 CI 上的 skip 语义更好控），但要连带说清它挂在哪条门禁上、
以及你打算怎么拿到改 `scripts/check.sh` 的授权。**回执里说清选了哪条、为什么。**

> **一个会咬人的细节**：守卫活着的时候，你**没法用 Bash 手动调它** ——
> `python3 .claude/hooks/guard_bash.py` 实测 exit=2（`python3` 在 `INTERPRETERS`（`:61`）里
> → 写入/执行位置 → 路径命中 `PROT_PATHS`；`BARE_MATCH`（`:44`）连裸 `guard_bash.py` 都认）。
> 而 `cargo test` 走 `cargo` 分支（`:283–290`，`sub="test"` → `write_pos=False`）→ 放行，
> 测试进程再去 spawn python3 **不经过 hook**。这也是路 2 比路 1 顺的一个理由 ——
> 路 2 的测试跑得起来，路 1 你连手动跑一次都要先要授权。

#### (3c) 让静默失效变得可见（要你出主意）

**病根。** hook 命令是 `.claude/settings.json:25` 的
`python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"`。`CLAUDE_PROJECT_DIR`
在 claude 进程启动那一刻定死；会话要是在仓库子目录里起（或者先起 claude 再 `cd`），
它就指着子目录，文件不存在 → hook 执行失败 → **非阻塞放行、不报警** → 整场守卫静默失效。

**现场证据（不是历史，是此刻）。** 写这份派单的会话 cwd 就是主仓根：

```
Read .claude/hooks/guard_bash.py   →  内容原样返回，没被拦
Read .contracts.lock               →  内容原样返回，没被拦
```

而同一台机、同一时刻，把一模一样的两个 payload 直接喂给守卫脚本：

```
{"tool_name":"Read","tool_input":{"file_path":".claude/hooks/guard_bash.py"}}  →  exit=2
{"tool_name":"Read","tool_input":{"file_path":".contracts.lock"}}              →  exit=2
```

**守卫脚本是好的，挂载是坏的。** 那个会话的 scratchpad 路径 slug 是
`-Users-shensikai-Documents-Aite-aite`，指向已经被删除的 `~/Documents/Aite/aite` ——
正是台账 §六 那个坑，一模一样。所以这条不是理论风险，你要修的是一个**正在漏水的**东西。

已知的四条路，各自的边界：

| 路 | 做什么 | 边界 |
|---|---|---|
| **A. 修根因** | hook 命令不再依赖 `CLAUDE_PROJECT_DIR`，改成从 cwd 往上找仓库根，例如 `python3 "$(git rev-parse --show-toplevel)/.claude/hooks/guard_bash.py"` | **实测过**：在 `/…/.worktrees/task-v6` 和它的子目录 `core/` 下，`git rev-parse --show-toplevel` 都返回 `/Users/shensikai/Documents/Aite/.worktrees/task-v6` —— 正是该指的地方，worktree 也认得对。代价：每次工具调用多起一个 `git` 进程；不在 git 仓库里跑时会拿到空串（要兜底）。**这是唯一真正消灭这个坑的一条**，其余三条都是在坑上装报警器 |
| **B. 守卫自证心跳** | 守卫每次成功执行就往 `.claude/hooks/.guard-heartbeat` 追加一行（时间戳 + cwd + `CLAUDE_PROJECT_DIR`） | 守卫没跑就没有新记录 —— 而「没有新记录」得有人去看才知道，单独用没意义，必须配一条开场自检或门禁去读它。另外要记得 gitignore |
| **C. 把「必须被拦」做成可跑的一条命令** | 一个脚本／测试，用真实 payload 调守卫并断言 exit 2，同时把 hook 配置里那条命令的 `$CLAUDE_PROJECT_DIR` 换成实际值试跑一遍 | 它验的是「守卫脚本本身好使」和「配置里那条命令在这个 cwd 下能执行」，**仍然验不了「这个会话的 hook 真的挂上了」** —— 后者只有开场自检那条（Read 守卫必须被拦）能验。而且前半 (3b) 已经覆盖，别重复造 |
| **D. SessionStart hook 报警** | 加一条 SessionStart hook，验 `$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py` 存在，不存在就往 stdout 打一段醒目的话 | **同一个坑**：SessionStart hook 命令自己也依赖 `$CLAUDE_PROJECT_DIR`，路径错的时候它一样执行失败、一样静默。除非把它写成不依赖那个变量的形式 —— 那就回到 A 了 |

**总管的倾向是 A**（真修根因），外加 (3b) 那套测试里钉一条「hook 命令不得依赖会随 cwd 漂移的
变量」的断言（形状照 `test_hook_command_uses_python3_not_python`），让 A 不会被将来某次改动
悄悄退回去；那条 `python3` 断言本身也要一起搬过来。B/C/D 不排斥，但别拿它们代替 A。

**注意 A 要改 `.claude/settings.json`，属于上面「要走授权」那一节。** 还有一件事要留神：
Claude Code 在会话启动时读 settings，你在会话中途改了 hook 命令，**当前这个会话未必生效** ——
所以「改完之后守卫还好使」这件事，你多半只能靠 (3b) 的测试来证，而不能靠自己再 Read 一次
守卫看被不被拦。这条限制要写进回执，让总管知道 A 的真实验证方式是「下一个会话的开场自检」。

**六轨并行下的一个好消息**：`git ls-files .claude/` 只有两个文件
（`hooks/guard_bash.py` 与 `settings.json`），都是入库文件，每个 worktree 有自己的一份。
你在 task-v6 里改它们**不会影响另外五轨**，合并之后才生效。

---

### ④ 顺带（台账 §4.4 的四条 low，都在你的文件里）

这四条的共同点是「不影响 M1–M6，但会在总管录演示或排障时正面撞上」。逐条给了实测输出
（`0bc8d55`），按性价比排序，做不完的在回执里逐条说明为什么。

**(a) `aite run --grace` 传超大有限数会在收尾时 panic。**

- 校验在 `core/crates/app/src/cli.rs:47–49`，只判了 `is_finite()` 和 `>= 0.0`；
  引爆点在 `core/crates/app/src/run.rs:180`
  `Duration::from_secs_f64(grace.max(0.0))` —— `max(0.0)` 挡住了负数，挡不住上溢。
- `Duration` 的上限是 `u64::MAX` 秒（约 1.8e19），`--grace 1e300` 一路穿过校验，
  到收尾那一刻 panic，**`sandbox.close_all()` / `store.close()` 整段被跳过**
  （`run.rs:207–214`，都排在 `:180` 后面）。
- 实测（`1e300` 确实被当成合法值收下了 —— 报的是配置不存在，不是「`--grace` 要是 …」）：
  ```
  $ aite run --grace 1e300 --config no/such/aite.yaml
  aite 起不来：配置文件不存在：no/such/aite.yaml（可从 config/aite.example.yaml 复制）
  用的配置是 no/such/aite.yaml（样例见 config/aite.example.yaml）
  exit=2
  ```
  对照：`--grace -1` 会被 `:47–49` 挡下并报「`--grace 要是 >= 0 的有限数`」。
- **`run.rs` 归 V5，你不能动。** 你能做的是在 `cli.rs:47–49` 加上界 + 人话。
  **这是把炸弹挡在门口，不是拆弹** —— `ServeOptions::shutdown_grace_sec` 还能由测试
  直接构造。回执里必须写明这一点，好让总管把「拆弹」那半路由给 V5。
  上界取多少你定（一天？一小时？），说清理由。

**(b) `aite run --help` 被 clap 截胡。**

- `main.rs:22–24` 把 `run` 声明成 `#[arg(trailing_var_arg = true, allow_hyphen_values = true)]`
  的 `Vec<String>`，但 clap 仍然把 `-h` / `--help` 吃掉了。实测（**stdout**，exit 0）：
  ```
  $ aite run --help
  组装并起飞（飞书 + edge + 模型；owner RΩ）

  Usage: aite run [ARGS]...

  Arguments:
    [ARGS]...

  Options:
    -h, --help  Print help
  exit=0
  ```
  一个真实选项都没有 —— `--config` / `--grace` / `--traceback` 一个都不在里面。
- 手写的 `USAGE`（`cli.rs:7–13`）不是完全的死代码，但**能打出它的两条路都不体面**（实测）：
  ```
  $ aite run -- -h      →  用法：…（stdout 为空，全走 stderr）   exit=2
  $ aite run --nope     →  不认识的参数 "--nope" + 用法（stderr）  exit=2
  ```
  也就是说 `cli.rs:52` 那条 `-h | --help => return Err(USAGE)` **只有加了 `--` 才够得着，
  而且给的是 stderr + 退出码 2** —— 正是 RΩ 在同一次改动里给 `aite evals run` 修掉的那个毛病。
- 两条路：
  - **(i) 只改 `cli.rs` 侧**，把 `-h` / `--help` 那条从「`Err(USAGE)`」改成 stdout + 退出码 0
    （跟 `evals/src/cli.rs:82–92`、`:272–278` 的 `ParseOutcome::Help` 一个口径）。
    这样 `aite run -- -h` 对了，`aite run --help` 仍被 clap 截胡。
  - **(ii) 动 `main.rs`**，在 `Run` 那个 variant 上加 `#[command(disable_help_flag = true)]`。
    一行、纯加法、不加子命令 —— 但 `main.rs:1–2` 明写「各轨落地时不需要动这个文件」，
    **必须先停下问总管**。
- **总管的倾向是 (i)，并在回执里点名 `aite run --help` 仍然被 clap 截胡、要动 `main.rs`
  才能根治**，由总管决定要不要单独放行那一行。不要自己越过 `main.rs` 那条线。

**(c) `--traceback` 承诺「完整错误链」，实际只是把同一句话用 Debug 再包一层引号。**

- `cli.rs:13` 的帮助文案写着「起不来时把**完整错误链**打到 stderr」，
  实现在 `cli.rs:108–110` 就是 `eprintln!("{e:?}")`。而 `StartupError` 是
  `core/crates/app/src/app.rs:41–43`：
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
  #[error("{0}")]
  pub struct StartupError(pub String);
  ```
  一个 newtype，**没有 `source()`，压根没有链**。实测：
  ```
  $ aite run --traceback --config no/such/aite.yaml
  aite 起不来：配置文件不存在：no/such/aite.yaml（可从 config/aite.example.yaml 复制）
  用的配置是 no/such/aite.yaml（样例见 config/aite.example.yaml）
  StartupError("配置文件不存在：no/such/aite.yaml（可从 config/aite.example.yaml 复制）")
  exit=2
  ```
  第三行就是第一行外面套了个 `StartupError("…")`。
- 两条路：**(i) 改文案说实话**（`cli.rs:13` 的 USAGE + `cli.rs:3` 那句注释），别再承诺链；
  **(ii) 让 `StartupError` 带 source** —— 但那是 `app.rs`，**归 V5**，要跨轨协调。
  顺带一个事实：`load_config`（`app.rs:99–111`）已经把底层错误 `{e}` 格式化进字符串了，
  就算加了 source 链，这条路径上也没东西可链。
- **总管的倾向是 (i)**：P0 收尾期不为一句帮助文案跨轨改类型。你要是发现 `build_app` 那边
  有真的多层错误值得链起来，写进回执让总管路由给 V5，别自己伸手。

**(d) `aite evals demo-fixture --help` 仍是 stderr + 退出码 2。**

- `core/crates/evals/src/demo_fixture.rs:416–425`：`run()` 拿 `args.first()` 当子命令，
  `:421` 一句 `if !["csv","history","all"].contains(...)` 就把 `--help` 判成「不认识的子命令」，
  返回 `FixtureError` → `evals/src/cli.rs:234–238` → stderr + 退出码 2。
  实测**两个写法都中**：
  ```
  $ aite evals demo-fixture --help   →  不认识的子命令 "--help"，只认 csv / history / all   （stderr）exit=2
  $ aite evals demo-fixture -h       →  不认识的子命令 "-h"，只认 csv / history / all       （stderr）exit=2
  ```
- 这是 RΩ 那次只修了 `evals run` 一半的账（台账 §4.4「`--help` 这条账只修了 `evals run`」）。
- 改法直白：照 `evals/src/cli.rs:82–92`（`ParseOutcome`）与 `:272–278` 的口径，
  给 `demo-fixture` 也加 `-h` / `--help` → stdout + 退出码 0。用法文本
  `evals/src/cli.rs:71–77` 的 `USAGE` 里已经有 demo-fixture 那两行，别再写第二份。

> **一条流传中的说法是错的，别照抄**：`aite evals --help`（不带子命令）**不是**
> 「不认识的子命令 + stderr + 2」。实测它被 **clap 截胡**，打的是 clap 那份不含任何真实选项的
> 帮助，**stdout + exit 0**：
> ```
> $ aite evals --help
> 评测：`aite evals run evals/p0 --platform fake --model scripted`（owner R7）
>
> Usage: aite evals [ARGS]...
> …
> exit=0
> ```
> 要走到 `evals/src/cli.rs:240–244` 那条「不认识的子命令」，得写成 `aite evals -- --help`
> （实测 stderr + exit 2）。另外裸 `aite evals` 走 `cli.rs:219–225`，USAGE 进 **stderr** + exit 2。
>
> 也就是说 **`aite evals --help` 跟 ④b 的 `aite run --help` 是同一个病（clap 截胡），
> 不是 ④d 的病。** 要不要一并处理、怎么处理，跟 ④b 一起决定（同样受 `main.rs` 那条线约束），
> 别把它塞进 ④d 里当成 `demo-fixture` 的账。改了在回执里说；不改也要说一句为什么。

---

## 纪律

1. **契约与锁**：`proto/**`、`core/crates/contracts/**`、`.contracts.lock` 冻结，
   全程 `OK 25 files`。要动 → 停下报告。
2. `evals/p0/*.yaml` 十个场景是验收面，**一个字不动**。②「不许污染 `sandbox_calls`」
   那条约束的另一半就是这个 —— 不许靠改场景断言来迁就实现。
3. `docs/dev-spec-2026-09-09.md` 与 `docs/dev-spec-2026-09-11-rustgo.md` 冻结。
4. 不 panic；不在 async 里阻塞；**测试不靠真实 sleep**，也不靠墙钟阈值当判据
   （上一轮刚修掉一条「两万个 tick 必须 1 秒内跑完」的假红门禁，别再造）。
   ④a 是在修一条 panic，别在修它的路上引入新的。
5. `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check`、
   `go vet`、`gofmt`、`go test -race` 全干净。
6. 密钥只从配置点名的环境变量读，任何日志 / 错误 / Debug 输出不得出现取值。
   ① 改的是 `--model live` 的错误路径 —— `wiring.rs:135` 那条消息现在只带
   **配置文件路径**，不带任何取值，改完保持这样。
7. **每条结论挂实测。**「应该会」「大概」一句不要。改了测试的，要能说出
   「把被测行为破坏掉，这条会不会红」，并把破坏→红、还原→绿两次输出贴出来。
8. **不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v6` 分支上，回执贴出来。
9. 六轨同时在跑：cargo 构建锁与 Docker daemon 是全机共享的，你的命令可能要排队等几分钟，
   这是正常的，别以为卡死。**② 的容器计数尤其要小心别数到别人的容器**（见 ② 那一节）。
10. `.claude/**` 的改动要**单独一次 commit**，别跟代码混在一起 —— 总管可能要单独审它。
11. 你的实测一律在 **worktree** 里跑，别在主仓根跑。主仓有 `config/aite.yaml`、还有别的轨在动，
    两处输出不一样（① 那一节已经踩到过一次）。

---

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v6

# 起跑线不许变小（718 只许变大，回执里说清多了几条）
scripts/check.sh
#   期望最后一行「全部通过」，退出码 0
#   A3/C2  OK 25 files
#   C1     contracts passed=25 failed=0
#   B      cargo passed=<不小于 718> failed=0
#   B      go test -race 八个包全 ok
#   B8     passed 10/10

# ① 顺序：这七条改前的实际输出在「要做什么 ①」那张表里，改后必须是右边那样
core/target/debug/aite evals run evals/p0 --list --model live
#   期望：10 个场景名的 JSON 数组，stdout，退出码 0
core/target/debug/aite evals run -h --model live
#   期望：用法进 stdout，stderr 为空，退出码 0
core/target/debug/aite evals run evals/p0 --platform feishu --model live
#   期望：不认识的 --platform "feishu"，只认 ["fake"]，退出码 2
core/target/debug/aite evals run evals/nope --model live
#   期望：场景加载失败：场景目录不存在：evals/nope，退出码 2
core/target/debug/aite evals run evals/p0 --only nosuch --model live
#   期望：没有这些场景：["nosuch"]，退出码 2
core/target/debug/aite evals run evals/p0 --list --sandbox docker
#   期望：10 个场景名的 JSON 数组，退出码 0
core/target/debug/aite evals run evals/p0 --only 01_simple_qa --model live
#   期望：--model live 起不来：…，退出码 2   ← 这条反过来，必须仍然被拦（试造一次没丢）

# ④d
core/target/debug/aite evals demo-fixture --help
core/target/debug/aite evals demo-fixture -h
#   期望：两条都是用法进 stdout，退出码 0

# ④b（取 (i) 的话）
core/target/debug/aite run -- -h
#   期望：用法进 stdout，退出码 0

# ② 容器（**先确认此刻没有别的轨在跑 docker 档**）
docker ps -a --filter label=aite.task -q | wc -l
#   期望 0

# 并行档的假红看这里
cd edge && go test -tags docker ./internal/sandbox/... -count=1
#   期望 ok（红了先看是不是只有这一个包、单独跑绿不绿）
```

② 的 docker 档收尾、③ 的守卫测试，两者的实测都要**自己造场景**（见各自那一节），
把「破坏 → 红 / 残留、还原 → 绿 / 干净」两次输出贴进回执。

---

## 回执格式

```
## V6 回执

基线 0bc8d55 → 提交 <短 sha>

### ① 评测 CLI 的顺序
选了哪条路（a 惰性 / b validate 钩子 / c 提前解析）：<>  理由：<一两句>
$ aite evals run evals/p0 --list --model live
<实际输出 + 退出码>
$ aite evals run -h --model live
<实际输出 + 退出码>
$ aite evals run evals/p0 --platform feishu --model live
<实际输出 + 退出码>
$ aite evals run evals/nope --model live
<实际输出 + 退出码>
$ aite evals run evals/p0 --only nosuch --model live
<实际输出 + 退出码>
$ aite evals run evals/p0 --list --sandbox docker
<实际输出 + 退出码>
$ aite evals run evals/p0 --only 01_simple_qa --model live
<实际输出 + 退出码；这条必须仍然被拦>
「ModelFactory 起飞前试造一次」怎么保住的：<一句>
「preflight 与 sandbox 成对」怎么保住的：<一句>
回归测试加在哪、加了几条：<>
破坏 → 红 / 还原 → 绿：
<两次实际输出>

### ② docker 档场景收尾
适配器裹在哪一层（要在探针下面）：<>  放在哪个文件：<>  为什么：<一句>
造残局的命令：<>
改前：$ docker ps -a --filter label=aite.task=<task_id> -q | wc -l  →  <n>
改后：$ docker ps -a --filter label=aite.task=<task_id> -q | wc -l  →  <n>
收尾失败不盖掉场景结论，怎么保证的：<一句>
docker 档正常路径跑过一遍，07_commands 的两条 sandbox_calls 断言没被污染：<输出>

### ③ 守卫
授权拿到没、走的哪条（AITE_RELOCK=1 起会话 / 总管自己改）：<>
(3a) 清了哪些死条目：<逐条>
     gofmt 要不要补 REWRITERS：<结论 + 理由>
     台账 .gitignore:14 / .gitignore:1 那两条的真实状态：<>
     deny 规则前导斜杠的实测结论：<>
(3b) 测试接管选了哪条（1 python3 自测 / 2 Rust 集成测试）：<>  理由：<一两句>
     加了几条用例、覆盖了旧那 14+5+8+2 条里的哪些面、放弃了哪些、为什么：<>
     挂在哪条门禁上：<>
     cargo passed 从 718 变成 <n>：<>
     破坏 → 红 / 还原 → 绿：
     <两次实际输出>
(3c) 让静默失效可见，选了哪条（A 修根因 / B 心跳 / C 可跑断言 / D SessionStart）：<>  理由：<>
     A 的验证限制（会话中途改 settings 未必生效）怎么处理的：<>
     「hook 命令不得依赖会漂移的变量」这条断言加了没、加在哪：<>

### ④ 四条 low
| 项 | 做了没 | 选了哪条路 | 没做的理由 |
| a --grace 上溢 |  |  |  |
| b run --help 被 clap 截胡 |  |  |  |
| c --traceback 不是错误链 |  |  |  |
| d demo-fixture --help / -h |  |  |  |
`aite evals --help` 也被 clap 截胡（跟 b 同病）这条怎么处理的：<>
a 的「拆弹在 run.rs（V5）」这半，要转给 V5 的原话：<>
c 如果发现 build_app 那边有真的多层错误，要转给 V5 的原话：<>

### 实测输出（粘实际的）
$ scripts/check.sh
<最后 3 行 + A3/C2、C1、B、B8 四行>
$ cd edge && go test -tags docker ./internal/sandbox/... -count=1
<>
$ docker ps -a --filter label=aite.task -q | wc -l
<>

### 开场自检
Read 守卫脚本被拦了没：<贴那行 blocked>
git log --oneline -1：<>
git status --short：<>

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v6` 分支上，回执贴出来。
