# 任务 BB5 — 卡片上一个按钮都没有，「看证据」只能走 CLI

## 背景：这轨是从哪来的

契约 R3 给卡片定义了两个动作：`stop` 和 `evidence`。
`buildActions`（`edge/internal/feishu/cards.go:151-166`）也一直在，往返还被测着
（`cards_test.go:281` / `:320` / `:340`）。

**但卡片现在一个按钮都不渲染。** 原因写在 `README.md` §已知边界第一条：

> lark-oapi-go v3.12.0 在长连接上把非 event 帧**整条丢弃**
> （`ws/client_message.go:79`），唯一的 `WithCardHandler` 钩子是注释掉的 ——
> 渲染出来的按钮点了一定没反应，那比不渲染更糟。

于是卡片末尾改成一行文字提示，教人用 `!stop`。**这个替代方案只覆盖了一半**：

| 动作 | 替代路径 | 够不够 |
|---|---|---|
| `stop` | `!stop <任务号>`（在本话题里回复，或群里 @ 机器人） | 够 —— 提示里的投递条件是对的，卡片本身 `reply_in_thread=true` 发进任务话题，那条路一定走得通 |
| `evidence` | **只能走 CLI**（`aite evidence show`） | **不够。群里的人根本够不着** |

这是 P0 闭环里唯一一处「契约里有、产品里摸不到」的能力。

## ⚠️ 先把边界划清楚：三件事你不能自己决定

1. **`edge/go.mod` 在守卫的保护面里，读都被拦**
   （2026-09-15 实测：`grep -n lark edge/go.mod` → `blocked: …（读取位置）`）。
   你看不到当前依赖表的原文。README 说的 v3.12.0 是你能拿到的版本信息。
2. **依赖表在 `docs/dev-spec-2026-09-11-rustgo.md` §2.4 冻结**
   （「已在 `core/Cargo.toml` / `edge/go.mod` 里钉死；**加依赖 → 停下报告**」）。
   **升级 lark-oapi-go 属于改依赖表，要总管点头。**
3. **进程模型在 spec §2.1 冻结**：两个进程、unix socket、edge 管对外连接。
   **加一个 HTTP server 进 edge 是架构改动**，同样要总管点头。

所以本轨的形状是：**先把结论做扎实，再谈落地。**
spec §7.1「卡住了怎么办」写的就是这种情形 —— 拿着证据停下报告，不是硬闯。

## 必读（按顺序）

1. `edge/internal/feishu/cards.go` 全文 —— 尤其 `:133` 那段注释
   （它记着「渲染那条路没删，SDK 哪天放开钩子就改回去」）和 `buildActions`（`:145-166`）。
2. `edge/internal/feishu/cards_test.go` 的三条相关测试（`:281` / `:320` / `:340`）
   —— 它们钉的是「发出去的 value 与收回来的对得上」，**这套往返断言在新路径下要还成立**。
3. `proto/aite/v1/` 里 R3 那两个动作的定义（**只读**）+ core 侧怎么消费卡片回传
   （`core/crates/control/src/plane.rs` 的 `resolve_task`，按 `task_id` / `card_id` 找目标）。
4. `README.md` §已知边界第一条全文 + `docs/acceptance-M.md` §0.4
   「卡片上没有按钮，改成一行提示」全段 —— **那是当前口径，你改完要一起改**。
5. `docs/acceptance-M.md` §8 第 4 条（观测缺口里关于按钮的那条）。
6. lark-oapi-go 的实际源码：`ws/client_message.go:79` 附近。
   SDK 在 `$GOPATH/pkg/mod/` 里（**不在守卫保护面，读得到**），
   `go list -m -f '{{.Dir}}' <module>` 能定位。**核实那句「整条丢弃」是不是真的**
   —— 这是本轨第一个要落地的事实。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-bb5
分支     : task-bb5
基线     : 7019c48
PATH     : 要有 ~/go/bin 与 /opt/homebrew/opt/rustup/bin
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-bb5
git log --oneline -1        # 期望 7019c48
git status --short          # 期望空
scripts/check.sh            # 期望最后一行「全部通过」，退出码 0
```

**关键行期望**（2026-09-15 在 `task-bb1` 这个 worktree 里实跑的原样抄录）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=864 failed=0` |
| B 全量 go test（-race） | 六个包全 `ok`（aiteerr / config / feishu / ingress / sandbox / server） |
| B8 评测 | `passed 10/10` |

**还有一条，必须真跑**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
> 没被拦说明守卫在静默失效，**停下报告**。

## 要干的活

### ① 把「SDK 收不到卡片回传帧」这句话核实到源码级

README 里那句是二手结论（写它的人当时在排别的问题）。**本轨要把它变成一手的**：

1. 定位 SDK 源码，读 `ws/client_message.go:79` 附近，**贴出真正的那几行**；
2. 回答：它丢弃的到底是什么帧？判据是什么字段？卡片回传帧长什么样、
   走的是同一条长连接吗？
3. `WithCardHandler` 那个钩子在这个版本里是什么状态 ——
   真的是注释掉的，还是存在但没接上？
4. **查一次上游**：这个缺陷在更新的版本里修了吗？改动是什么？
   （只查、只报，**不许动依赖**。）

**这一步的产出是事实，不是方案。** 事实不扎实，后面的方案全是猜的。

### ② 列出所有可行路径，每条给代价

已知的候选（不限于这些，你可以有更好的）：

| 路径 | 要动什么 | 谁点头 |
|---|---|---|
| 升级 lark-oapi-go | `edge/go.mod` + spec §2.4 依赖表 | 总管 |
| 不升级，自己在 SDK 之上解帧 | 只动 `edge/internal/feishu/**`？要核实做不做得到 | 可能不用 |
| 卡片回调走 HTTP（飞书开放平台的「消息卡片请求网址」） | edge 里加 HTTP server = 改 spec §2.1 进程模型 | 总管 |
| 保持现状，把「看证据」做成命令 | 加一条 `!evidence <任务号>` 走现有消息通道 | 可能不用 |

**最后那条值得认真算一次**：`!stop` 已经证明「命令 + 话题内回复」这条路走得通，
而「看证据」的诉求本质是「把 `aite evidence show` 的输出摘要发回线程」。
**它不需要按钮，也不需要改 SDK、依赖或架构。**
如果它能覆盖 80% 的实际需求，那它可能比按钮更值得先做 —— **给判断，别只列选项。**

### ③ 落地：做那条不需要点头的

按 ② 的判断，把**不需要总管点头**的那条做掉。做之前注意：

- **`!evidence` 这条路要是你选的**：它要新增一条命令，而命令的路由归 R5、
  文案归 `wording.rs`（逐字钉着）—— **这两处在 BB1 那一轨的可写面里**。
  跟 BB1 抢文件就是合并冲突。**做法**：edge 侧你自己做，core 侧的路由与文案
  **写成一份规格转给 BB1 或总管**，别自己伸手。
- **要是你判断「必须升级 SDK / 必须加 HTTP」**：那就**别落地**，
  把 ①② 做扎实，出一份决策材料，停下报告。**这完全合格** ——
  spec §7.1 就是这么规定的。
- `buildActions` 那条路**不许删**，也不许「顺手启用」——
  它现在是刻意不渲染的，改这个决定要有 ① 的事实支撑。

### ④ 文档追平

改完之后 `README.md` §已知边界第一条、`docs/acceptance-M.md` §0.4 与 §8 第 4 条
会有几句变成废话。**先 grep 后改**，别漏副本（这个项目里同一句话散在 3–5 处是常态，
X1/Z3 各清过一轮，每次都还有漏网的）。

> ⚠️ `docs/acceptance-M.md` 是冲突高发区（AA1/AA2 两轨都碰过）。
> 只改**与你改动直接对应**的那几句，抬头变更日志那块合并时由总管定口径。

## 纪律

1. **可写面**：`edge/internal/feishu/**`、`edge/cmd/**`、
   `README.md` §已知边界、`docs/acceptance-M.md` §0.4 / §8 第 4 条、
   台账里**只追加**你自己那一节。
2. **只读面**：其余一切。特别是 `core/**`（BB1/BB2/BB6 的面）、
   `proto/**` 与 `core/crates/contracts/**`（守卫保护面）、
   `edge/internal/sandbox/**`、`edge/internal/ingress/**`。
3. **`edge/go.mod` / `edge/go.sum` 读写都被守卫拦，别去碰。**
   要改依赖 → **停下报告**，别出补丁脚本自己打（依赖表是 spec 冻结面，
   不是守卫一个门禁的事）。
4. **不许改 spec** —— `docs/dev-spec-*.md` 可读不可写，也不该改。
5. Go 侧改完要过 `go vet` + `gofmt`，**`-race` 不是可选项**
   （长连接、令牌桶、容器记账表都不是单线程的，竞态在普通 `go test` 下完全隐形）。
6. **落盘无残留**：探测 SDK 用的临时文件收尾前清干净。

## 验收

```bash
scripts/check.sh                                   # 全部通过，退出码 0
cd edge && go test -race ./... -count=1            # 六个包全 ok
cd edge && go vet ./... && test -z "$(gofmt -l .)"
```

- `cargo passed=864 failed=0` **必须逐字不变**（本轨零 Rust 改动）；
- `OK 25 files` / `contracts passed=25 failed=0` 逐字不变；
- `passed 10/10` 不许掉；
- 改了 `cards.go` 的话，那三条往返测试要么全绿、要么你解释清楚为什么该变。

## 回执

追加进台账，标题「二十二、BB5 回执 —— 2026-09-15」。要有：

- 基线与开场自检（五行 + 守卫拦截逐字原话）；
- **① 的一手事实**：SDK 源码那几行逐字、丢弃的判据、`WithCardHandler` 的真实状态、
  上游修没修（带版本号）。**哪句是你读到的、哪句是你推的，分开写**；
- ② 的路径表：每条的代价、谁点头、你的推荐 + 理由（**要有一个推荐**）；
- ③ 做了什么 / 为什么只做这些；转给 BB1 或总管的那份规格（如果有）；
- ④ grep 出来的文档副本清单 + 逐处改了什么；
- **记账转出去的**（表格：位置 / 病 / 归哪轨）；
- **没做的 / 拿不准的**（编号列表）。

**如果结论是「必须总管点头才能往下走」**，那回执的主体就是那份决策材料：
问题是什么、三条路各自要付什么、你推荐哪条、点头之后第一步做什么。
**这是合格交付，不是失败。**
