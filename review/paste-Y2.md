# 任务 Y2 — preflight 对 `platform: fake` / `provider: scripted` 不设防，而且它对 fake 这一档整体不自洽

## 背景：这轨是从哪来的

X1 刚把「preflight 漏检 `worker.system_prompt_path`」补上（折进第 ① 组，**没有变成八组**）。
它同时如实记下：**同形状的口子还剩两个没补**，而它自己那一档的实证也只做了一半。

**总管 2026-09-13 把这两条实证到手了**，所以这一轨不是从推断出发的。

### 实证一：`platform: fake` 时 preflight 说「可以起飞」，`aite run` 退出码 2

```
$ aite preflight --offline --config <platform: fake 的配置>
汇总：OK 2 · WARN 1 · FAIL 0 · SKIP 4（共 7 项，过了 7 项）
全部没红，可以起飞。（--offline 只验了 1/2/7，真机起飞前请全跑一遍）
退出码 0

$ aite run --config <同一份>
aite 起不来：config.platform=fake 时必须由调用方注入平台实现（build_app 的 Injections.platform）。
退出码 2
```

**与 X1 补掉那条一模一样的形状**：preflight 全绿、`aite run` 起不来。
`model.provider: scripted` 而没注入模型是同一条（`build_app` 同样明文拒绝）。

### 实证二：`platform: fake` 时它还在要飞书凭证 —— 七组对这一档整体不自洽

同一份 `platform: fake` 的配置，**不带** `--offline` 全跑：

```
[1/7] OK   配置可加载       platform=fake · model.provider=openai_compat · sandbox.image=aite-sandbox:p0
[2/7] FAIL 环境变量齐       3/4 个未设置：FEISHU_APP_ID FEISHU_APP_SECRET FEISHU_BOT_OPEN_ID
[3/7] FAIL 飞书凭证有效     前置未满足：FEISHU_APP_ID FEISHU_APP_SECRET 未设置，这一项没法查
[4/7] FAIL 飞书身份对得上   前置未满足：FEISHU_APP_ID FEISHU_APP_SECRET 未设置，这一项没法查
```

**fake 平台压根不连飞书**，2/3/4 三组的判据对它毫无意义 —— 而真正拦住起飞的那条
（没注入实现）**一组都没管**。第 ① 组只把 `platform` / `model.provider` 的**取值报出来**，
不判断它跟「有没有注入」搭不搭；其余六组的判据都与这两个取值无关。

**X1 没能实证的那一半，本轨也大概率实证不了**：要演示「七组全绿而起不来」需要一台
凭证配齐的机器（本机 2/3/4 必红）。**但 ② 一旦做对，这件事就不需要实证了** ——
因为那时 fake 这一档会在第 ① 组就被拦下，不存在「全绿」这种状态。

## 必读（按顺序）

1. **`review/review-findings-2026-09-12-vmerge.md` 第八节（X1 回执）** ——
   ① 那一段是你的模板（它怎么把判据折进第 ① 组、怎么保住「一项失败不阻断后面的」、
   「怎么补」那三件事怎么写、五组变异怎么验），「`--offline` 还剩哪些口子」那张表是你的账单，
   末尾「记账转出去的」第 2、4 行是本轨。
2. `core/crates/app/src/preflight.rs` 的**模块头注释（1–40 行）** ——
   七组定义表、红线（绝不打印密钥取值）、「一项失败不阻断后面的」三条规矩。
   `check_config`（X1 刚给它加了 `repo_root` 参数）是你要动的地方。
3. `core/crates/app/src/app.rs` 的 `build_app`（**只读**）——
   两条拒绝起飞的判据原文就在里面（`platform=fake` 没注入、`provider=scripted` 没注入），
   你要让 preflight 跟它口径一致。**注意 preflight 没有 `Injections`**，
   这正是 X1 说「要重新想『注入』这件事在 preflight 里怎么表达」的地方。
4. `core/crates/edge-client/src/link.rs:83`、`:136`（④）与 X1 已经改好的
   `core/crates/edge-client/src/lib.rs:77`、`:92`（**照它的口径改，别另发明一套**）。
5. `docs/dev-spec-2026-09-11-rustgo.md` §3.1（`platform` / `provider` 的取值语义：
   fake 是「留给评测 / 回放」的）、line 308（「七组自检」）。**冻结、只读**，守卫会拦。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-y2
分支     : task-y2
基线     : 18f30b6（= merge(task-x1) 并入 main 那一格）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-y2
git log --oneline -1        # 期望 18f30b6，记下当基线
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（总管 2026-09-13 在 `task-y1` 那个 worktree 里实跑过，同一格代码，原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=818 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

> ⚠️ **已知假红。** `-p aite --test` 的 `startup_recovery` / `graceful_shutdown` /
> `reconnect_replay` 三个会偶发红一条。判据：单独跑那个 target，绿就是假红。
> **修它归 Y1，不归你**（Y1 正在治病根：`StopSignal::set()` 丢信号）。

> 💡 **写脚本注意**：`scripts/check.sh > log 2>&1; echo "EXIT=$?"` 在后台任务里会骗人 ——
> 报的是最后那个 `echo` 的退出码，永远 0。要 `rc=$?; …; exit $rc`。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
>
> 已知误拦（换个写法绕开）：守卫 `PROBES` 里还留着指向已删文件的
> `aite/contracts/__init__.py`，命令里带 `*` 通配符时会被反向匹配误拦。

## 可写路径

> **Y1 与你同时在跑。** 下面这张表两轨**逐字相同**，是同一份划分 ——
> 上一轮 W2/W3 出过「同一个文件在两份派单里互相指给对方」的事故（`cli.rs` 因此谁都没改、
> 留下两条反的注释），所以这次把边界写死在两边。

| 面 | Y1 | Y2（你） |
|---|---|---|
| `core/crates/app/src/run.rs` | **读写** | 只读 |
| `core/crates/app/src/preflight.rs` | 只读 | **读写** |
| `core/crates/app/tests/preflight_e2e.rs`、`cli_smoke.rs` | 只读 | **读写** |
| `core/crates/app/tests/` 其余全部（含 `common/mod.rs`） | **读写** | 只读 |
| `core/crates/app/tests/signals.rs`（**Y1 新建**） | **读写** | 别碰 |
| `core/crates/edge-client/src/link.rs` | 只读 | **读写** |
| `README.md`、`docs/**`（非 spec）、`review/inventory-*.md` | 只读 | **读写** |
| `core/crates/app/src/{app,cli,wiring,main}.rs` | 只读 | 只读 |
| `core/crates/control/**`、`edge/**`、`.github/**`、`Makefile`、`scripts/**`、`.claude/**` | 只读 | 只读 |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` | **冻结** | **冻结** |

**`core/crates/app/src/cli.rs` 两轨都只读，归总管** —— 上一轮就是把它互相指给对方才出的事。
要改它 → 写进回执，别伸手。

**文档面归你。** Y1 那一轨会把它想改的文档（`README.md` 的「停机」段、
`docs/acceptance-M.md` 的 M6）**写进它的回执**而不是自己动手 ——
合并时由总管落，**不要你去猜它想改什么**。你只管 preflight 那一侧的涟漪。

**台账：你只许追加一节「十、Y2 回执」。** Y1 追加的是「九」。
（上一轮 W2/W3 都写「六」，合并时撞了，这次预先错开。）

---

## 要做什么

### ① 判断题先做：preflight 该拿 `platform: fake` 怎么办

**这是本轨唯一需要设计的地方，先想清楚再动手。** 两种语义，选一种：

- **(a) fake 就是「不能用 `aite run` 起飞」** —— 第 ① 组直接 FAIL，
  「怎么补」说「真机起飞把 `platform` 改成 `feishu`；fake 是留给评测 / 回放的（§3.1），
  走 `aite evals`」，后面 2/3/4 组 **SKIP**（理由：fake 不连飞书，那三组的判据不适用）。
- **(b) fake 是合法配置，只是 preflight 管不了它** —— 第 ① 组给 WARN，
  明说「这份配置只能被注入着用，`aite run` 会拒绝起飞」，2/3/4 照样 SKIP。

**总管的倾向是 (a)**，理由：preflight 的存在意义就是「`aite run` 起不起得来」，
而 fake 这一档**确定起不来**（`build_app` 明文拒绝、退出码 2 实证在案）。
说 WARN 等于让人自己去猜。**但你要自己核一遍再定** ——
特别是：有没有别的入口会拿 `platform: fake` 的配置跑 preflight 而且**理应通过**？
（`aite evals --sandbox docker` 那条路会不会？`wiring.rs` 的评测接线会不会调 preflight？
**这两处一定要查**，查完写进回执。查出来「有」就选 (b)。）

`model.provider: scripted` 同理，同一处判断、同一种收场。

> ⚠️ **不许变成八组。** `docs/dev-spec-2026-09-11-rustgo.md:308` 冻结着「七组」，
> 而这个说法还散在 `README.md`（3 处）、`docs/acceptance-M.md`（多处，含一份逐行实测输出）、
> `docs/demo-3min.md`、`preflight.rs` 模块头、`tests/cli_smoke.rs`、
> `tests/preflight_e2e.rs`（**「七项齐、顺序固定」是硬断言**）、
> `review/inventory-gateway-evals.md`。**照 X1 的办法折进第 ① 组。**

### ② 落地，照 X1 的五条口径

X1 那条已经把模板立好了，逐条对齐（别另发明一套）：

1. **FAIL 也要把 `cfg` 交出去** —— 不交的话后面六组会全变成「第 1 组没过，配置读不出来」，
   「一项失败不阻断后面的」当场破功。X1 回执里点名了这一条。
2. **第 ① 组仍然只有一行结论**（七组的口径是「每项一行结论 + 非 OK 时一句怎么补」）。
   现在第 ① 组要承担三件事了（配置解析 / prompt 文件在不在 / platform+provider 跟注入搭不搭），
   **一行里怎么把话说清楚**是这一轨的手艺活。
3. **「怎么补」要说清三件**：当前值、该改成什么、以及为什么（fake 是留给谁用的）。
4. **2/3/4 组 SKIP 的理由要写在那一行里**，不能默默跳过 —— 口径参照 `--offline` 那三行
   （`--offline：不碰网络`）。
5. **红线照旧**：新的 detail / fix 都走 `CheckResult` 的字段，渲染前统一过 `Redactor`，
   别开新的直写路径。

### ③ 回归 + 变异验证

**回归**：`tests/preflight_e2e.rs` 至少三条（`platform: fake` → 第 ① 组按 ① 的结论 +
2/3/4 SKIP + 后面照跑；`provider: scripted` 同；两者都正常时不受影响），
`tests/cli_smoke.rs` 一条（真二进制，退出码对）。

**变异**：照 X1 那张表的力度，至少四组，每组都要被抓到：

| 改坏什么 | 期望谁红 |
|---|---|
| 整段判据摘掉（退回今天之前） | |
| FAIL 降成 WARN（或 (b) 的 WARN 升成 FAIL） | |
| 2/3/4 忘了 SKIP（照样 FAIL） | |
| 「怎么补」里指错 | |

**两边口径对照**（X1 交过同样的表，你也要交）：拿 `platform: fake` 与
`provider: scripted` 两份坏配置，各跑 `preflight --offline` / `preflight` / `aite run`，
改前改后各一遍，六格填满。

### ④ `link.rs:83`、`:136` 的两条谎话副本

两处都说 `contract_state()` / `status()` 是给「`!status` 的健康行」用的，
而**那条健康行全仓不存在**。这是同一句谎话的第 3、4 个副本 ——
W2 改掉了 `app.rs` 那个，X1 改掉了 `lib.rs` 那两个，**照它们的口径改**，别另起一套说法。

全仓 grep 核实真调用方再写（X1 的结论：`contract_state()` 产品代码零调用方，
唯一使用者是 `tests/contract_gate.rs`；`status()` 有三个真调用方）。
**只改注释，代码一个字不动。**

### ⑤ 涟漪：把 ① 的结论落进文档

第 ① 组的描述散在这些地方，逐个对齐（**「七组」这个说法一处都不许变**）：

- `core/crates/app/src/preflight.rs` 模块头那张表的第 1 行
- `README.md`（3 处）
- `docs/acceptance-M.md` §0.1（**含一份逐行实测输出** —— X1 重跑过并判定「不用改」，
  你这一轨改了第 ① 组的行为，**要重新跑一遍再判**）
- `docs/demo-3min.md`
- `review/inventory-gateway-evals.md`（那份清单里有七组的逐条描述）

X1 还留了一句给你：它把 `acceptance-M.md` 里「第 1 组只验『配置解析得出来』」那句改了，
**现在第 ① 组又多一件事，那句话要再改一次。**

---

## 纪律

1. **① 是判断题，先核再做。** 特别是「有没有别的入口理应拿 fake 配置通过 preflight」这条 ——
   查 `aite evals` 与 `wiring.rs` 两处，查完写进回执。查出来「有」就选 (b)，别硬上 (a)。
2. **新断言写完自己验一遍：把产品代码改坏，它必须红。**
   这几轮台账已经抓到四条恒真断言，别造第五条。
3. **不许变成八组**，冻结面一个字别动，契约锁始终 `OK 25 files`。
4. **别越界**：`run.rs` 与 `tests/` 下除你那两个文件之外的全部都归 Y1，它同时在跑。
   `cli.rs` 两轨都只读。
5. **测试数只许涨。** `cargo passed=818` 是起跑线。
6. 卡住了：要改的文件不在白名单 → 停下报告；① 两种语义都算不过账 →
   **把两边的代价写清楚**，选一条做，别停着不动。

## 验收

```bash
scripts/check.sh                                       # 「全部通过」，退出码 0
cd core && cargo test -p aite --test preflight_e2e      # ③ 的主战场
cd core && cargo test -p aite --test cli_smoke
cd core && cargo test -p aite --lib                     # preflight.rs 的 mod tests
cd core && cargo clippy --workspace --all-targets -- -D warnings
core/target/debug/aite preflight --offline              # 人眼判据
```

外加**自己交叉验一遍**：

- [ ] ① 的两个入口（`aite evals`、`wiring.rs`）都查过，结论写进回执了。
- [ ] ② 的五条口径逐条对齐 X1，尤其「FAIL 也交出 cfg」和「第 ① 组仍然一行」。
- [ ] ③ 的六格对照表填满；四组变异每组都被抓到。
- [ ] ④ 只改注释，口径与 W2/X1 改的那三处一致。
- [ ] ⑤ 涟漪扫完，**「七组」一处没变**；`acceptance-M.md` §0.1 那份实测输出重跑过。
- [ ] 改完之后，`--offline` 还剩哪些「全绿≠起得来」的口子 —— **重新列一遍那张表**
      （X1 交过一版，你这一轨会改掉其中两行，剩下的如实写，别声称补全了）。

## 回执格式

在回复里写（**不要**新建回执文件；台账只许追加**「十、Y2 回执」**这一节）：

```
## Y2 回执

基线 : <开场自检那个 sha>
check.sh : <五行关键值>  退出码 <n>
假红 : <撞到没有；撞到了贴失败名 + 单独跑的结果>
守卫 : Read .claude/hooks/guard_bash.py 被拦 ✅

### ① 选了 (a) 还是 (b)、为什么
（两个入口的核查结论：aite evals / wiring.rs 会不会拿 fake 配置跑 preflight）

### ② 落地的五条口径
（逐条对齐情况；第 ① 组那一行最后长什么样，原样贴）

### ③ 两边口径对照（六格）
| 配置 | preflight --offline | preflight（全跑） | aite run |
| platform: fake 改前 / 改后 | | | |
| provider: scripted 改前 / 改后 | | | |

### ③ 变异验证
| 改坏什么 | 谁红了 |

### ④ link.rs 两处
（真调用方 grep 结论；改成什么）

### ⑤ 涟漪清单
| 文件 | 改了什么 | 「七组」动了没 |

### 改完之后 --offline 还剩哪些「全绿≠起得来」
（重列那张表）

### 测试数
818 → <N>，每条多在哪

### 记账转出去的
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
```
