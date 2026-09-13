# 任务 Z2 — 守卫把整个会话锁死了：给它一条恢复路径，并把八次误拦落成账

## 背景：这轨是从哪来的

**2026-09-13，总管这一侧的会话被守卫整体锁死了一次，而且从会话内部出不来。** 这不是记账，
是工具链自己在挡活。

V6 的补丁 (3c) 把 hook 命令从 `$CLAUDE_PROJECT_DIR` 改成了：

```
python3 "$(git rev-parse --show-toplevel)/.claude/hooks/guard_bash.py"
```

它治好了原来那条真病（`CLAUDE_PROJECT_DIR` 在进程启动那一刻定死，会话在仓库子目录里起就
指错路径 → hook 执行失败 → **非阻塞放行且不报警** → 整场守卫静默失效，2026-09-12 真踩过）。
V6 的分析里写着这个新写法是 **fail-closed**，「不会退化成静默放行」—— 那句话是对的。

**但它漏了恢复路径。** 实测经过：

```
会话 cwd 被留在了 ~/.claude/projects/.../memory（不在任何 git 仓库里）
  → git rev-parse --show-toplevel 失败
  → 命令展开成 python3 "/.claude/hooks/guard_bash.py"
  → 文件不存在，python3 退出码 2
  → PreToolUse 读作「拦截」
```

**而会话 cwd 只能靠 Bash 的 `cd` 改，Bash 已经被拦。** 实测 `Bash` / `Read` / `Write`
三个工具全部同一条错 —— 也就是说**会话变成砖头，只能由人重启**。
旧写法有它自己的病，但从不锁死会话。

**药是一行**（两种失效模式一起治，fail-closed 保住）：

```bash
d=$(git rev-parse --show-toplevel 2>/dev/null) || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"
```

- 在仓库里（任意子目录、任意 worktree）：`git` 赢 → 指对根，(3c) 要修的漂移病还是治好的；
- 不在任何仓库里：退回 `CLAUDE_PROJECT_DIR` → **照样找得到守卫、照样拦** → 不锁死；
- 两个都失败：`python3` 打不开文件 → 退出码 2 → **fail-closed 仍然保住**。

**但别照抄这一行就交活** —— 它是总管随手写的，你要自己验（见 ② 的要求）。

## 必读（按顺序）

1. **`review/v6-guard-patch.py`** —— 你的交付形状就是它：`.claude/**` 在守卫的 `PROT_PATHS` 里
   （`readable=False`，**读和写都拦**），所以你**读不了那两个文件一个字节**，
   只能交一份「逐条精确替换、锚点必须唯一命中否则整份拒写」的脚本给总管跑。
   它的抬头 docstring 把这个处境写得很清楚，照它的骨架来。
2. **`core/crates/app/tests/guard.rs`** —— 9 条现行回归。三件事对你最要紧：
   - `settings_path()`（`:48`）与 `pretooluse_commands()` —— **编译后的测试代码用 `std::fs`
     读 `settings.json` 是可以的**（守卫只管工具调用，不管测试运行时读什么）。
     ② 的行为回归就靠这个。
   - `the_hook_command_uses_python3_not_python`（`:270`）—— 现有那条对 hook 命令的断言，
     你要在它旁边加新的。注意它已经做了「含 `python3` 且不含裸 `python`」的放宽判据，
     新命令里有 `python3` 也有 `$d`，**别把它弄红**。
   - **模块头 `:25-31` 那段现在是假的**：它还写着 hook 命令是
     `python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"`，并解释了那条已经修掉的病。
     ③ 要改它。
3. `review/review-findings-2026-09-12-vmerge.md` 第八~十一节末尾的「记账转出去的」——
   八次误拦的原始记录散在 X1 / Y1 / Z1 三份回执里，④ 要把它们归拢。
4. `docs/dev-spec-2026-09-11-rustgo.md` §5 归属表（`.claude/**` 归 R0）。**冻结、只读。**

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-z2
分支     : task-z2
基线     : f3bc017（= merge(task-z1) 并入 main 那一格）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-z2
git log --oneline -1        # 期望 f3bc017（若多一格、只加了本派单文件，那也对，记下实际 sha）
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（总管 2026-09-13 在这个 worktree 里实跑过，原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=852 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

> ⚠️ 那三个抖动 target（`graceful_shutdown` / `reconnect_replay` / `startup_recovery`）
> 的病根 W2 / Y1 / X1 都治过了，Z1 那一轮开场收尾各跑一次都没撞到。
> **你撞到任何一个都是新信息，贴进回执**（判据照旧：单独跑一遍那个 target，绿就是假红）。

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明守卫在静默失效，
> **停下报告**。（这一条就是本轨在治的那件事的反面：守卫**该**拦得住，只是不该把会话锁死。）

> 💡 **本轨凡改了 Rust 就会撞上一条**（Z1 收尾时撞过）：`cargo fmt --all`（写模式）
> **会被守卫拦**——它会碰 `core/crates/contracts/**` 这个冻结面。提示里让你用
> `cargo fmt --check`，但收尾要的是真格式化。解法：对你改过的那一两个文件直接跑
> `rustfmt --edition 2024 <file>`，然后 `git status` 复核没动到别的文件。
>
> 其余已知误拦（换写法，别碰守卫）：命令里出现受保护路径的**字面量**
> （`git add <那个路径>` 判写入位置 → 用 `git add -u`）；heredoc 正文里有配不平的引号、
> 中文引号、markdown 的 `**` 加粗、或正文太长（→ 改用 Write 工具落文件，别用 heredoc）；
> `find … -delete` / `-exec`（覆盖面判不出来 → 用 `ls` + 点名 `rm -f`）。
> **这些你在 ④ 里要逐条归类，所以撞到就记下来，那就是你的原始材料。**

## 可写路径

> **Z3 与你同时在跑。** 下面这张表两轨**逐字相同**，是同一份划分 ——
> W2/W3 那一轮出过「同一个文件在两份派单里互相指给对方」的事故（`cli.rs` 因此谁都没改、
> 留下两条反的注释），所以这次把边界写死在两边。

| 面 | Z2（你） | Z3 |
|---|---|---|
| `review/z2-guard-patch.py`（**你新建**） | **读写** | 不存在于它的视野 |
| `core/crates/app/tests/guard.rs` | **读写** | 只读 |
| `core/crates/edge-client/src/gate.rs` | 只读 | **读写** |
| `edge/internal/ingress/client.go` | 只读 | **读写** |
| `README.md`、`docs/acceptance-M.md`、`docs/demo-3min.md`、`review/inventory-gateway-evals.md` | **只读**（见下） | **读写** |
| `.claude/hooks/guard_bash.py`、`.claude/settings.json` | **读写不了**（守卫 `readable=False`）—— 只能出补丁脚本 | 同 |
| `core/crates/app/src/**`、`core/crates/control/**`、`core/crates/edge-client/src/`（除 `gate.rs`）、`edge/`（除 `client.go`） | 只读 | 只读 |
| `.github/**`、`Makefile`、`scripts/**`、`docker-compose.yml`、`docker/**` | 只读 | 只读 |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` | **冻结** | **冻结** |

**文档面归 Z3。** 你想改 `README.md` / `docs/**` 的话，**把原文 + 改成什么写进回执**，
合并时由总管落 —— 别伸手（Y1 就是这么交的，行得通）。
你自己的文字出口是：`guard.rs` 的模块头注释（③）+ 补丁脚本的 docstring。

**台账：只许追加一节「十二、Z2 回执」。** Z3 追加的是「十三」。

---

## 要做什么

### ① 出补丁脚本 `review/z2-guard-patch.py`

照 `review/v6-guard-patch.py` 的骨架：

- **每条锚点必须唯一命中，否则整份拒绝执行、一个字节都不写**（`--check` 只报告不写盘、
  `--partial` 只写对得上的）；
- `settings.json` 改完**必须仍是合法 JSON**（V6 那份有这道检查，照抄 —— 写坏了 hook 整个不加载，
  那就是又一次静默失效）；
- 抬头 docstring 写清：为什么是脚本而不是 diff（`.claude/**` 的 `readable=False`）、
  为什么 agent 自己跑不了（守卫故意拦自我授权的环境变量赋值）、
  以及**改完必须跑什么**（`cd core && cargo test -p aite --test guard`）。

**锚点怎么定**：`settings.json` 里的 hook 命令字符串在 JSON 里是**带转义**的
（命令本身含双引号）。V6 那份用 `json.dumps(s)[1:-1]` 让 `json` 自己算转义 —— 照抄那个办法，
别手写转义。

> **你读不到 `settings.json`，所以锚点只能靠两处交叉确认**：
> ① V6 补丁里 (3c) 那条 `Edit` 的 `new` 值（它就是现在的形态）；
> ② `guard.rs` 的 `pretooluse_commands()` 能在**测试运行时**把真实命令打出来 ——
> 写一条临时测试 `println!` 出来看一眼，是最可靠的办法。**用完删掉那条临时测试。**

**改几处**：`settings.json` 里可能有多个 PreToolUse hook 条目（`pretooluse_commands()`
返回的是 `Vec`）。**全部都要换**，而且脚本要断言「换掉的条数 == 命中的条数」。

### ② 补一条**本来就该抓住它**的行为回归

这是本轨的核心交付。`the_hook_command_uses_python3_not_python` 只断言命令的**形状**，
抓不到「cwd 在仓库外时它会不会锁死」。

**要做的**：在 `tests/guard.rs` 里加一条测试，**真跑那条 hook 命令**：

1. 从 `settings.json` 读出 PreToolUse 命令（`pretooluse_commands()` 已经有了）；
2. 在**一个仓库外的临时目录**里（`tempfile::tempdir()`，注意 `TMPDIR` 通常不在任何 git 仓库里
   —— 自己先 `git rev-parse` 确认一次，别假设）执行它，`CLAUDE_PROJECT_DIR` 设成仓库根；
3. 喂一个**该被拦**的 payload（比如写 `proto/**`），断言退出码是 `BLOCKED`（2）；
4. 再喂一个**该放行**的 payload（比如 `echo hello`），断言退出码 0。

**第 4 步是这条测试的承重墙**：只断言「该拦的拦住了」是**恒真断言** —— 现在这个坏命令
把**一切**都拦了，所以它也会「通过」。必须两边都断，才能分辨「守卫在工作」和「守卫在乱拦」。

**改坏必须红**：把命令退回 `python3 "$(git rev-parse --show-toplevel)/…"`（不带 fallback），
这条测试必须红，而且**红在第 4 步**（该放行的被拦了）。把那次的输出贴进回执。

> ⚠️ 这条测试依赖 `settings.json` 的内容，而那个文件由总管跑补丁才会变。
> 所以**交付时它会是红的**（补丁还没跑）—— 这是对的，不是失败。
> 在测试的文档注释里写明这件事，并在回执里显著标出：**「这条测试在补丁跑之前必须红，
> 跑之后必须绿」**，两种状态的输出都贴。

### ③ 修掉 `guard.rs` 模块头那段已经过期的话

`:25-31` 还写着 hook 命令是 `python3 "$CLAUDE_PROJECT_DIR/…"`，并花五行解释那条**已经被
V6 修掉**的病。现在它是假的。

改成说现状：命令是什么形态、它解决了什么（cwd 漂移）、它**曾经**引入了什么
（仓库外锁死 —— 本轨修的）、以及「这份测试验不了的那件事」那一段还成不成立
（**自己判断**：现在 `Read` 守卫脚本被拦这条开场自检是不是仍然是唯一能验「它挂上了」的办法）。

### ④ 把八次误拦归拢成一张表

材料散在四份回执里（X1 第八节 1 次、Y1 第九节 2 次、Z1 第十一节 3 次、总管这一侧 2 次），
外加你自己这一轨撞到的。**归拢成一张表**，每行三件：撞到的写法、守卫报的原话、判定。

判定只有三种，**每行必须落到其中一个**：

- **设计如此，别绕**（例：`AITE_RELOCK=1` 自我授权、`find -exec` 覆盖面判不出来）；
- **扫命令文本的固有代价**（例：heredoc 正文里的引号 / `**` / 受保护路径字面量、
  `cargo fmt --all` 碰冻结面）—— 这一类要给出**标准绕法**，那就是下一批派单模板要抄的东西；
- **还能收窄**（真误拦，值得改守卫）—— 这一类**只记账、别动守卫**，
  它们不在本轨范围（本轨只改 hook 命令那一处），写进「记账转出去的」。

表放哪：台账的「十二、Z2 回执」里，**外加**一份精简版进 `guard.rs` 的模块头
（下一个执行者最可能在那儿找它）。

---

## 纪律

1. **② 的第 4 步（该放行的要放行）不许省。** 只断言「该拦的拦住」是恒真断言 ——
   这几轮台账已经抓到四条恒真断言了，别造第五条。
2. **新断言写完自己验一遍：把命令改坏，它必须红，而且红在你预期的那一步。**
3. **别照抄总管写的那行药。** 自己验一遍三种情形（仓库内 / 仓库外 / 两者都不可用），
   验不过就改，并在回执里说清你改了什么、为什么。
4. **不许动守卫脚本本身**（`guard_bash.py`）。本轨只改 hook 命令那一处配置 ——
   `PROBES` / `PROT_PREFIXES` 那些收窄的活归下一轮。
5. **契约锁始终 `OK 25 files`**，冻结面一个字别动。
6. **测试数只许涨。** `cargo passed=852` 是起跑线（② 那条在补丁跑之前是红的 ——
   如实报「N 条里 1 条红，原因是补丁未跑」，别把它 `#[ignore]` 掉）。
7. 卡住了：锚点确认不了（`settings.json` 读不到、临时测试也打不出来）→ **停下报告**，
   别猜着写锚点。

## 验收

```bash
cd core && cargo test -p aite --test guard          # 9 + 新增；② 那条在补丁跑前是红的
cd core && cargo clippy --workspace --all-targets -- -D warnings
rustfmt --edition 2024 crates/app/tests/guard.rs    # 别用 cargo fmt --all，它会被守卫拦
scripts/check.sh                                    # 除 ② 那条外全绿
```

**外加一件只有总管能做的**，在回执里把命令写全给他：

```bash
AITE_RELOCK=1 python3 review/z2-guard-patch.py --check   # 先干跑
AITE_RELOCK=1 python3 review/z2-guard-patch.py           # 真写
cd core && cargo test -p aite --test guard               # ② 那条这时必须转绿
```

自己交叉验一遍：

- [ ] ① 的锚点是**交叉确认**过的（V6 补丁的 `new` 值 + 临时测试打出来的真实命令），不是猜的。
- [ ] ① 的脚本断言了「换掉条数 == 命中条数」，且 JSON 合法性检查在。
- [ ] ② 两个 payload（该拦 / 该放）都断了；改坏命令后**红在第 4 步**。
- [ ] ② 的文档注释和回执都写明「补丁跑前红、跑后绿」。
- [ ] ③ 模块头里没有一句话还在描述已经修掉的病。
- [ ] ④ 每一行都落到三种判定之一；「固有代价」那一类都给了标准绕法。

## 回执格式

在回复里写（**不要**新建回执文件；台账只许追加**「十二、Z2 回执」**这一节）：

```
## Z2 回执

基线 : <开场自检那个 sha>
check.sh : <五行关键值>  退出码 <n>
假红 : <撞到没有；撞到了贴失败名 + 单独跑的结果>
守卫 : Read .claude/hooks/guard_bash.py 被拦 ✅

### ① 补丁脚本
（锚点几条、怎么交叉确认的、--check 的输出原样贴）

### ② 行为回归
（测试形状；**补丁跑前的红** 与 **改坏命令后的红** 两份输出都贴；说明红在第几步）

### ③ 模块头改了什么

### ④ 误拦归类表
| 撞到的写法 | 守卫报的原话 | 判定（设计如此 / 固有代价 + 标准绕法 / 还能收窄） |

### 给总管的三条命令
（补丁 --check、真写、跑 guard 测试）

### 测试数
852 → <N>（其中 <n> 条在补丁跑前为红，原因）

### 记账转出去的
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
```
