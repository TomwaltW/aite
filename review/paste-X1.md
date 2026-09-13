# 任务 X1 — preflight 说「可以起飞」然后起不来；外加裸 `[0]` 家族与 `startup_recovery` 的两个抖动源

## 背景：这轨是从哪来的

V1–V6 + W1/W2/W3 都已合进 `main`，`check.sh` 全绿（`cargo passed=810 failed=0`）。
P0 没兑现的只剩 `docs/dev-spec-2026-09-09.md` §2.4 / `docs/dev-spec-2026-09-11-rustgo.md` §4.4
的 **M1–M6 真机验收**与 **3 分钟演示** —— 那是总管自己的活，而他**马上就要去跑**。

**这一轨的 ① 不是记账，是 2026-09-12 当场咬到总管的一条。** 他想试机器，
`aite preflight --offline` 报「**全部没红，可以起飞**」，紧接着 `aite run` 退出码 2：

```
aite 起不来：读不到 system prompt：system prompt 不存在：aite/worker/prompts/platform.md。
配置项是 worker.system_prompt_path（当前值 aite/worker/prompts/platform.md），
路径相对于进程的工作目录 —— 多半是没在仓库根起进程。
```

真因是他那份 `config/aite.yaml`（2026-09-10 写的）还指着 **2026-09-12 被删掉的 Python 树**
（`aite/worker/prompts/` → 现在是 `core/crates/worker/prompts/`）。两件事都坏：

- **preflight 七组里没有一组管这件事**，所以它给的「可以起飞」是假的；
- **那句「多半是没在仓库根起进程」是误诊** —— 当时 cwd 就是仓库根。

V3 记过一条同族的（「`--offline` 全绿 ≠ 起得来」，因为它跳过第 5 组）。**这条更狠：
去掉 `--offline` 全跑一遍也救不了你**，七组里根本没有这一项。

其余四条是 W2 转出来的同一族小账 —— 它们共同的形状是「**失败时报的不是人话**」
（越界 panic 冒充断言失败），以及两条指着不存在的东西的注释。

## 必读（按顺序）

1. **`review/review-findings-2026-09-12-vmerge.md` §六（W2 回执）末尾的「记账转出去的」表** ——
   ②③④⑤ 四条的原文都在那儿，连同 W2 已经**复现过**的证据（②那条撑开窗口是**必现**的）。
2. `core/crates/app/src/preflight.rs` 的**模块头注释**（1–40 行）——
   七组的定义表、红线（绝不打印密钥取值）、「一项失败不阻断后面的」三条规矩都在那儿。
   **① 要改的正是那张表。**
3. `core/crates/app/src/app.rs:154`（`build_app` 的文档注释，W2 刚改成说实话的那份）与
   `:310` 的 `require_system_prompt` —— ① 的判据来源。
4. `docs/dev-spec-2026-09-11-rustgo.md` §4（DoD）。**这份冻结、只读，守卫会拦。**
   注意 line 308 写着「`aite preflight`（**七组**自检）」—— 见下面 ① 的硬约束。
5. `docs/acceptance-M.md` §0.1 / §0.1.1（preflight 那一节，W1/W3 刚校过）。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-x1
分支     : task-x1
基线     : 69323db 的代码面（worktree 建在它上面）
工具链   : cargo 1.98.1、go 1.27.1、libprotoc 36.1、Docker 29.6.1、Docker Compose v5.3.0
PATH     : 要有 /opt/homebrew/opt/rustup/bin 与 ~/go/bin（check.sh 自己 export，手敲 cargo 要注意）
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-x1
git log --oneline -1        # 记下 sha，回执里当基线
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

**关键行期望**（总管 2026-09-13 在这个 worktree 里实跑过，原样抄的）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=810 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok` |
| B8 评测 | `passed 10/10` |

> ⚠️ **已知假红，别当回归。** `-p aite --test` 的 `startup_recovery` /
> `graceful_shutdown` / `reconnect_replay` 三个 target 会偶发红一条（2026-09-12 记录在案
> 至少五次，**单轨也会撞上**）。判据：把 `error: test failed, to rerun pass …` 点名的
> target **单独跑一遍**，绿就是假红，直接开工。
>
> W2 已经把 `graceful_shutdown`（计时区间量错）和 `reconnect_replay`（两个病根）
> 都修了并各连跑 20/10 遍验过。**剩下的 `startup_recovery` 就是你的 ②④** ——
> 换句话说，这一轨收尾时的判据不只是「全绿」，还要**它不再抖**。

> 💡 **写脚本注意**（W3 踩过）：`scripts/check.sh > log 2>&1; echo "EXIT=$?"` 在后台任务里
> 会骗人 —— 任务通知报的是最后那个 `echo` 的退出码，永远 0。要 `rc=$?; …; exit $rc`。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
>
> 顺带一个已知误拦（不是故障，绕开就行）：守卫的 `PROBES` 里还留着指向已删文件的
> `aite/contracts/__init__.py`，命令里带 `*` 通配符时会被反向匹配误拦。
> V6 的 `review/v6-guard-patch.py` 就是删它的，但那份补丁**只有人能跑**
> （守卫故意拦 agent 自我授权），至今没跑。撞上了换个不带通配符的写法。

## 可写路径

| 面 | 权限 |
|---|---|
| `core/crates/app/src/preflight.rs` | 读写（①） |
| `core/crates/app/src/app.rs` | 读写（①，只许改 `require_system_prompt` 的报错文案与相关注释） |
| `core/crates/app/tests/preflight_e2e.rs`、`cli_smoke.rs` | 读写（① 的回归） |
| `core/crates/app/tests/startup_recovery.rs`、`evidence_on_disk.rs`、`sqlite_cross_process.rs` | 读写（②③④） |
| `core/crates/app/tests/common/mod.rs` | 读写（`first_active_task` 已在 `:542`，够用就别改） |
| `core/crates/edge-client/src/lib.rs` | 读写（⑤，**只许改注释**） |
| `README.md`、`docs/acceptance-M.md`、`docs/demo-3min.md` | 读写（① 的涟漪，只许改 preflight 相关段落） |
| `review/inventory-gateway-evals.md` | 读写（① 的涟漪，那份清单里有七组的逐条描述） |
| `review/review-findings-2026-09-12-vmerge.md` | 只许**追加**一节「X1 回执」 |
| `docs/dev-spec-*.md`、`proto/**`、`core/crates/contracts/**`、`.contracts.lock` | **冻结**，守卫会拦 |
| `core/crates/control/**`、`edge/**`、`.github/**`、`Makefile`、`scripts/**`、`.claude/**` | 只读 |

---

## 要做什么

### ① preflight 漏检 `worker.system_prompt_path`（本轨的主事）

**病**：`require_system_prompt`（`app.rs:310`）只在 `build_app` 里跑，preflight 七组
一组都不碰它。于是配置指错文件时，preflight 全绿、`aite run` 退出码 2。

**硬约束：不许变成「八组」。** `docs/dev-spec-2026-09-11-rustgo.md:308` 写着
「`aite preflight`（七组自检）」，那份文件**冻结**。「七组」这个说法还散在
`README.md`（3 处）、`docs/acceptance-M.md`（多处，含一份逐行实测输出）、
`docs/demo-3min.md`、`preflight.rs` 模块头、`tests/cli_smoke.rs`、
`tests/preflight_e2e.rs`（**「七项齐、顺序固定」是硬断言**）、
`review/inventory-gateway-evals.md`。

**所以折进第 ① 组「配置可加载」**：它现在的定义是「`config/aite.yaml` 读得出来
（不存在退到样例，算 WARN）」，扩成「读得出来**且它指到的文件真的在**」是自然延伸 ——
配置解析成功但指着不存在的文件，本来就不该叫「配置可加载」。

**要满足的**：

1. `system_prompt_path` 读不到时第 ① 组**报 FAIL**（不是 WARN —— 它是硬起飞前提），
   `fix` 那一行要说清楚**怎么补**：当前值是什么、应该是什么
   （`core/crates/worker/prompts/platform.md`，`config/aite.example.yaml:37` 就是它）、
   以及「Python 树 2026-09-12 删了，旧配置里的 `aite/worker/prompts/` 已经不存在」
   这条**真实病史** —— 这正是总管撞上的那一种。
2. **第 ① 组仍然只有一行结论**（七组的口径是「每项一行结论 + 非 OK 时一句怎么补」）。
3. **红线照旧**：输出不得出现密钥取值，新加的 detail / fix 一样要过 `Redactor`。
4. **`--offline` 下这一项照样跑** —— 它不碰网络也不碰 docker，没有理由跳。
   （顺手复核：V3 记的「`--offline` 全绿 ≠ 起得来」现在还剩哪些口子？
   ① 补上之后，`model.base_url` 空那条仍在第 5 组、仍被 `--offline` 跳过。
   **如实写进回执，别声称补全了。**）
5. **回归两条**：`preflight_e2e.rs` 里一条（指一个不存在的 prompt → 第 ① 组 FAIL、
   退出码 1、fix 里有正确路径），`cli_smoke.rs` 里一条（进程级，走真二进制）。
   两条都要做「**把产品代码改回去，它必须红**」。

**另外修那句误诊**（`app.rs:314`）：现在写的是「路径相对于进程的工作目录 ——
多半是没在仓库根起进程」。总管撞上时 cwd 就是仓库根，这句话把人带反了。
改成先说**真正最可能的**（配置里的路径本身不对 / 旧配置指着已删的 Python 树），
再说 cwd 那一种；最好把「解析成的绝对路径」也打出来，让人一眼看出它找的是哪儿。
`app.rs:195`（连不上 edge 那条）有同样的措辞，**一并复核** ——
那一条里 cwd 确实是常见真因，不一定要改，判断完写进回执。

### ② `startup_recovery.rs:259` 的裸 `[0]` —— 已复现的必现病根

```rust
let new_task = app.store.list_active_tasks(CHAT).await.expect("list")[0].clone();
```

W2 的回执写着：那条脚本是**单步 `final`**（走 Answering 路径，落 `Answering` 就退出活跃
口径），在这一行前面插一句 `settle().await` 把窗口撑开，**必现**
`index out of bounds: the len is 0 but the index is 0`。

**这是 `startup_recovery` 假红的一个已定位病根。** 修法现成：换成
`first_active_task`（`tests/common/mod.rs:542`，W2 加的，自带超时 + 人话消息）。

**要做的**：改完先**复现一次原病**（插 `settle()` → 必现越界），再改，再确认复现不出来。
两次输出都贴进回执 —— 只说「改好了」不算。

### ③ 同一族的另外三处

```
core/crates/app/tests/evidence_on_disk.rs:64
core/crates/app/tests/sqlite_cross_process.rs:38
core/crates/app/tests/sqlite_cross_process.rs:73
```

同形状：`emit` 之后无守卫直接取下标，任务跑得快时报越界 panic 而不是人话。
（`evidence_on_disk.rs:264` 有 `wait_until` 兜着，**不在此列**，别顺手改。）

顺带**全仓扫一遍同族**：`list_active_tasks(...)[0]` / `.expect("list")[0]` 这类写法
还有没有别处（W2 已经改掉 `crash_recovery.rs:53,125` 和 `graceful_shutdown.rs` 两处）。
扫到的都列进回执，**在可写面内的就改**，不在的记账。

### ④ `startup_recovery` 的第二个抖动源 —— 先查清楚，别急着调大预算

`orphans_are_closed_before_the_platform_starts`（`:210`）：`run.shutdown()` 的 **10s 预算**
在重负载下被瞬时饿死撞穿（报「10s 内 run_app 没有返回」）。W2 实测：同一二进制
**6 路并发跑 72 遍红 3 遍；顺序跑 20 遍 0 红**；收尾本身只要 **0.1–0.4ms**，
所以不是「真挂死」。

> ⚠️ **W2 特意留了一句：别急着调大预算，那会把「真挂死」一起咽掉。**

**要做的**：先判断这 10s 到底在等什么（是 `wait_until` 类的轮询、还是一整段收尾的总预算），
再决定怎么办。可接受的收场有三种，**选哪种都要给理由**：

- 把预算换成**对负载不敏感的判据**（比如等一个真事件而不是等墙钟）；
- 保留墙钟但把它**只用作兜底**，真正的判据换成别的，兜底阈值调大不影响灵敏度；
- 查清楚之后判定「这就是机器太忙，测试没病」，**记账不改** —— 但要给出
  「怎么区分它和真挂死」的说法，否则这条抖动以后每次都要重新判一遍。

**做完要验**：`cargo test -p aite --test startup_recovery` 连跑 **20 遍**全绿，
外加**负载下**（同时跑 `cargo test --workspace`）再 6 遍。都贴回执。

### ⑤ `edge-client/src/lib.rs:77`、`:92` 的两条失真注释

两处都说 `contract_state()` / `status()` 是给「`!status` 的健康行」用的，
而**那条健康行全仓不存在**（`contract_state()` 在产品代码里零调用方）。
这跟 W2 ③ 修掉的 `app.rs` 那条是同一句谎话的另外两个副本。

全仓 grep 核实之后按**实际用途**重写（契约闸门 `gate.rs` 才是真用途）。
**只改注释，不动代码。** 如果核实发现 `contract_state()` 确实零调用方，
在回执里点出来 —— 「要不要删」不是本轨的决定。

---

## 纪律

1. **每条改动都要有测试钉着**，新断言写完自己验一遍：**把产品代码改坏，它必须红**。
   这几轮台账一共抓到四条恒真断言了，别造第五条。
2. **②④ 要先复现再修。** 只说「改好了」不算 —— 原病的复现输出和修后的输出都要贴。
3. **不许 skip。** 跑不了的环境该红就红。
4. **契约锁必须始终 `OK 25 files`**，冻结面一个字别动。
   ① **不许变成八组** —— 理由见 ① 的硬约束。
5. **测试数只许涨。** `cargo passed=810` 是起跑线，收尾报新数并说明每条多在哪。
6. 卡住了：要改的文件不在白名单 → 停下报告；④ 查不出所以然 → **写清楚查到哪一步、
   排除了什么**，选「记账不改」那条收场，别硬做也别装作查清了。

## 验收

```bash
scripts/check.sh                    # 「全部通过」，退出码 0；关键行除 cargo 计数外不变
cd core && cargo test -p aite --test startup_recovery      # ②④ 之后连跑 20 遍
cd core && cargo test -p aite --test preflight_e2e         # ① 的主战场
cd core && cargo test -p aite --test cli_smoke             # ① 的进程级回归
cd core && cargo clippy --workspace --all-targets -- -D warnings
core/target/debug/aite preflight --offline                 # ① 的人眼判据
```

外加**自己交叉验一遍**：

- [ ] ① 拿一份**故意指错 `system_prompt_path`** 的配置跑 `preflight`，
      第 ① 组 FAIL、退出码 1、`fix` 那行照着做真能修好；再跑 `aite run` 确认两边口径一致
      （preflight 说 FAIL 的，`aite run` 就该起不来；preflight 全绿的，`aite run` 至少
      不该死在 prompt 这一步）。
- [ ] ① 的涟漪：`README.md` / `acceptance-M.md` / `demo-3min.md` /
      `inventory-gateway-evals.md` 里第 ① 组的描述与新行为一致，**「七组」这个说法一处没变**。
      `acceptance-M.md` §0.1 里那份逐行实测输出要重跑一遍对齐。
- [ ] ② 的原病复现过、修后复现不出来。
- [ ] ③ 全仓扫过同族写法，清单在回执里。
- [ ] ④ 20 遍 + 负载下 6 遍都绿，且给出了「怎么区分它和真挂死」的说法。
- [ ] 新加的每条断言都做过「改坏产品代码 → 它必须红」。

## 回执格式

在回复里写（**不要**新建回执文件；台账 `review-findings-2026-09-12-vmerge.md`
只许**追加**一节「八、X1 回执」）：

```
## X1 回执

基线 : <开场自检那个 sha>
check.sh : <关键行>  退出码 <n>
假红 : <撞到没有；撞到了贴失败名 + 单独跑的结果>
守卫 : Read .claude/hooks/guard_bash.py 被拦 ✅

### ①–⑤ 逐条
（每条：病在哪 / 判据 / 改法 / 为什么选这条 / 钉它的测试 / 改坏产品代码验过没）

### ① 的两边口径对照
（故意指错配置时 preflight 与 aite run 各说什么；改完之后各说什么）

### ① 之后 --offline 还剩哪些「全绿≠起得来」的口子
（如实列，别声称补全了）

### ②④ 的复现记录
（原病复现的输出 + 修后的输出；④ 的 20 遍与负载下 6 遍）

### ③ 全仓同族写法清单
| 位置 | 在可写面内？ | 改了没 |

### 测试数
810 → <N>，每条多在哪

### 记账转出去的
| 位置 | 病 | 归哪轨 |

### 没做的 / 拿不准的
```
