# 规格：`!evidence <任务号>` —— 让「看证据」在群里够得着

- **出处**：BB5 轨（2026-09-15），基线 `7019c48`。
- **收件人**：BB1（core 侧路由与文案在它的可写面里）+ 总管（有一处要点头，见 §4）。
- **状态**：**未落地。** BB5 只出规格，一行 core 代码都没动。
- **为什么不是 BB5 自己做**：`plane.rs` 的 R5 路由与 `commands.rs` 的文案在 BB1 的可写面，
  抢文件就是合并冲突；而 §4 那一处在契约冻结面上，不是哪一轨能自己决定的。

---

## 1. 要解决的是什么

契约 R3 给卡片定义了两个动作，`stop` 和 `evidence`。`stop` 有等价的命令路径
（`!stop <任务号>`，在任务话题里回复或群里 @ 机器人）；**`evidence` 一条群里够得着的路径都没有**，
只能在终端 `aite evidence show <task_id>`。这是 P0 闭环里唯一一处「契约里有、产品里摸不到」的能力。

## 2. 为什么补它的办法不是「把按钮修好」

两条独立的事实，各自都足以否掉按钮那条路（一手复核见 BB5 回执 §①）：

1. **SDK 收不到卡片回传帧，而且没有可升的版本。** lark-oapi-go v3.12.0 的
   `ws/client_message.go:79` 把 `type` 头不等于 `event` 的数据帧整条 return；
   唯一的 `WithCardHandler`（`ws/client.go:56-60`）是注释掉的，`cardHandler` 字段
   没人写也没人读。v3.12.0 已是 proxy.golang.org 上的最新版，开发主干 `v3_main`
   里那五行仍是注释，上游自己的 card 例子走的是 HTTP webhook。
2. **就算帧收到了，「证据」按钮也不会出现。** core 侧**没有任何生产者**往
   `ChecklistCard.actions` 里写 `EVIDENCE` —— `core/crates/worker/src/card.rs:83` 与
   `core/crates/control/src/card.rs:83` 都硬编码 `vec![CardActionKind::Stop]`。

也就是说：升级 SDK / 改用 HTTP 回调，**都不能交付这个能力**，还得另外改 core 的卡片生产者。
命令那条路绕开了这两件事。

## 3. 规格

### 3.1 路由

`core/crates/control/src/plane.rs:528-532`，在 `on_command` 的 match 里加一条：

```rust
"!evidence" => self.cmd_evidence(ev, &rest).await,
```

R5 的收口条件不动（`text.starts_with('!') && (ev.mentioned || session.is_some())`）。
卡片是 `reply_in_thread=true` 发进任务话题的，所以「在卡片那条话题里回复 `!evidence #A17`」
一定满足投递条件 —— 与 `!stop` 完全同一条路。

### 3.2 参数

复用 `normalize_task_no`（`commands.rs:24`）：`a17` / `A17` / `#a17` 一律归成 `#A17`。
**不支持省略任务号**（与 `!stop` 不同）：`!stop` 省略时按「`!status` 列得出来的只有一个」兜底，
而证据要查的多半是**已经结束**的任务，不在那份列表里，兜不出有意义的默认值。
空参数回帖见 §3.4。

### 3.3 解析到哪个任务 —— **这是唯一的拦路石**

`!stop` 走 `resolve_stop_target` → `status_tasks`，而 `status_tasks` **主动滤掉终态任务**
（`plane.rs:558-591`）。证据的常见问法恰恰是「那个跑完的任务证据在哪」，所以
**不能复用 `status_tasks`** —— 复用的话 `!evidence #A17` 对刚交付的任务回「没有这个任务」，
又造出一个「点了没反应的按钮」。

当前 `SessionStore` 上能用的只有两个（`core/crates/contracts/src/ports.rs:149/151`）：

| 方法 | 能不能用 |
|---|---|
| `get_task(task_id)` | 不限状态，但要先有 `task_id`；用户手里只有 `#A17` |
| `list_active_tasks(chat_id)` | 只有 created / planning / working，终态查不到 |

`Session` 结构里没有任何指向 task 的字段（`contracts/src/session.rs:88-106`），
`EvidenceWriter` 端口的四个方法全部以 `task_id` 为键、没有列举面（`ports.rs:162-179`）。
**所以「按任务号找一个不限状态的任务」这件事，现在做不到。** 要补一个端口方法：

```rust
/// 按任务号在某个群里找任务，**不限状态** —— 终态也要找得到。
/// 与 list_active_tasks 的区别就是不看 status；证据要查的多半是已经结束的任务。
async fn find_task_by_no(&self, chat_id: &str, task_no: &str) -> Result<Option<Task>, StoreError>;
```

- 加在 `core/crates/contracts/src/ports.rs`（`list_active_tasks` 后面）；
- 实现在 `core/crates/store/src/lib.rs:411` 旁边，SQL 与 `list_active_tasks` 同形、去掉
  status 过滤、加 `AND task_no = ?`；
- **要刷契约锁**（`aite contracts lock`）。文件数不变，仍是 `OK 25 files`，变的是哈希。
  不刷的话 `cli_smoke` 会红（见台账里「契约锁没刷会红两格」那条）。

> ⚠️ **这一处在守卫保护面 + spec §2.4 的冻结面上，要总管点头。** 见 §4。

### 3.4 回帖文案

和卡片按钮那条路**同口径**（`plane.rs:485-491` 现在就是这么回的）：

| 情况 | 回帖 |
|---|---|
| 找到了 | `任务 {task_id} 的证据目录：{evidence.task_dir(task_id)}` |
| 任务号找不到 | `NO_SUCH_TASK_TEXT`（`"没有这个任务"`，已有常量，逐字不变） |
| 没带任务号 | 新常量，建议 `"要看哪个任务的证据？用 !evidence <任务号>，任务号在卡片标题上。"` |

「找到了」那一句**照抄按钮那条路的格式，一个字都别改** —— 两条路给的是同一个东西，
说法岔开就又要有人来对齐（`!stop` 与卡片 stop 按钮已经为这件事对过一轮，W2）。

### 3.5 帮助文案 —— 也要总管定口径

`UNKNOWN_COMMAND_TEXT`（`commands.rs:6`）现在是：

```
未知命令，可用：!status !stop <任务号> !restart !new
```

加了命令就得加进这句，而这句**在三个地方被逐字钉着**：

| 位置 | 性质 |
|---|---|
| `core/crates/control/src/commands.rs:6` | 常量本体 |
| `core/crates/control/tests/wording.rs:25` | 逐字断言 |
| `core/crates/control/src/commands.rs:9` `KNOWN_COMMANDS: [&str; 4]` | 数组长度要改成 5；`commands.rs:61-68` 那条测试会要求新命令出现在帮助里 |
| `review/inventory-core.md:48` / `:246` | 台账里的原文抄录 |
| `docs/dev-spec-2026-09-09.md:742` | **spec 里的那张表** —— 「不许改 spec」 |

**最后一行是个真问题**：帮助文案的原文写在 spec 的规则表里，加命令就会和 spec 不一致。
BB5 不动 spec，这条留给总管：要么 spec 一起改，要么明确「帮助文案不含 `!evidence`」
（那样用户发 `!evidence` 能用，但 `!xxx` 的提示里看不到它 —— 发现不了的能力等于没有）。

### 3.6 要补的测试

放在 `core/crates/control/tests/`（与 `commands.rs` / `routing.rs` / `wording.rs` 同处）：

1. `!evidence #A17` 对**终态**任务回得出目录 —— 这条是整件事的要害，
   拿 `status_tasks` 实现的话它必须红；
2. `!evidence #A17` 对**进行中**任务同样回得出目录；
3. 任务号不存在 → 逐字 `"没有这个任务"`；
4. 不带任务号 → 逐字 §3.4 那句新文案；
5. 投递条件：群里不 @ 也不在话题内发 `!evidence #A17` → 命中 R8，零回复、
   只 bump `events.ignored`（与 `!stop` 同形，抄 `routing.rs` 现成的）；
6. `commands{name}` 计数器 key 是 `commands!evidence`（`plane.rs:526` 是拼出来的）；
7. 大小写与 `#`：`!EVIDENCE a17` 能命中（`parse_command` 小写化 + `normalize_task_no`）。

### 3.7 edge 侧要不要动

**命令本身：不用动一行。** edge 没有任何命令清单（全仓 grep：`edge/**` 非测试代码里
零个 `"!stop"` 之类的字面量），任何文本消息都同样归一化后转给 core，路由全在 R5。

**卡片上那行提示：等 core 落地后再动，别提前。** `cards.go` 的 `actionHint` 现在只讲
`!stop`。等 `!evidence` 真能用了，再把它加进提示 —— **提前加就是新的死按钮**：
core 会回 `UNKNOWN_COMMAND_TEXT`，而这一整轨存在的理由就是不要这种东西。
建议的加法（core 落地后，由拿这份规格的人一并提）：终态卡片也该有这一行 ——
`actionHint` 现在挂在「`actions` 里有 STOP」上，而终态卡片 `actions` 为空，
恰恰是最需要看证据的时候却没有提示。这是一条真的行为改动，要配一条测试
（现有 `cards_test.go:TestNoDeadButtonsAndAHintInstead` 里「终态卡片不该有这一行」
那半条断言会跟着变，改的时候要说清为什么该变）。

---

## 4. 要总管点头的，一共两处

| # | 要动什么 | 为什么绕不开 |
|---|---|---|
| 1 | `SessionStore` 加 `find_task_by_no`（`contracts/src/ports.rs` + `store` 实现 + 刷契约锁） | 没有它就只能复用 `status_tasks`，而那条路对终态任务回「没有这个任务」—— 等于没做 |
| 2 | 帮助文案 `UNKNOWN_COMMAND_TEXT` 加 `!evidence`，而原文钉在 `docs/dev-spec-2026-09-09.md:742` | 「不许改 spec」；不加则这条命令没有发现路径 |

两处都是**加法**，不改任何已冻结的值、不动 `ACTIVE_TASK_STATUSES`、不动进程模型、
不动依赖表。契约文件数不变（仍 `OK 25 files`）。

## 5. 一个更大的产品问题，请一并定

按契约，`evidence` 动作的回帖内容就是**一行文件系统路径**（`plane.rs:485-491`）。
把它从按钮搬到命令，**群里的人仍然读不到那个目录** —— 除非他能登上跑 core 那台机器。
也就是说 §3 交付的是「契约完整了」，不完全是「群里的人看得到证据了」。

真要解决后者，回帖该是一段**摘要**（几步、调了哪些工具、哪一步失败、终态），
也就是 `aite evidence show` 已经渲染的东西（`core/crates/evidence/src/cli.rs`）。
那要在 evidence 侧抽一个摘要渲染函数，并且要定「往群里贴多少算合适」。
**这是 R3 owner 的口径问题，BB5 不替它定。** 建议分两步：

- **第一步（本规格 §3）**：`!evidence <任务号>` 回目录路径，与按钮同口径。能力有了入口。
- **第二步（另开一轨）**：回帖改成摘要。要定摘要长什么样、多长、失败任务贴不贴报错原文。
