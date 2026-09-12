# 任务 T16 — 拿官方文档核对飞书 API：把附录 A 的假设逐条验掉

## 背景：这轨是从哪来的

T1–T12 十二轨全部合进 main（最新 `0d6939c`）。全仓 **969 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过。P0 代码面齐了，剩 §2.4 的 M1–M6 卡在飞书凭证。

`aite/adapters/feishu/` 那 8 个文件是照 spec **附录 A** 写的，验收方式是 respx mock +
自造 fixture。也就是说：**adapter 与飞书的每一处约定，今天都只被"我们以为它是这样"验过，
没有任何一条对过官方文档。**

附录 A 自己就写着「实施时以 open.feishu.cn 官方文档为准」，spec §7.6 也留了口子：

> 飞书 API 与本文附录 A 或立项方案附录 A 不一致 → 以官方文档为准，
> 在回执里写明差异；只要契约不变就不用停。

这轨就是去把那个"为准"做掉。真机第一次跑，字段名、权限标识、卡片 schema 版本
任何一处对不上就是一个 400，而那时候排查成本比现在高得多 —— 你手上有官方文档，
真机上只有一条群里没回的消息。

§3.7 还挂着两个待核实项，其中 (b) 是 `preflight.py` 明确说"光靠凭证问不出来"的：

- **(a)** 只有 @ 权限时，话题里不带 @ 的回复是否投递
- **(b)** "获取会话历史消息" API 是否要求"获取群组中所有消息"这个敏感权限

(a) 只能真机实验（M4 就是那个实验），**但 (b) 大概率能从权限文档里查出来** ——
每个 API 的文档页都列了所需权限。查出来就能让总管提前把权限申请对，不然 M5
（汇总本群本周开放事项）会在真机上直接 403。

## 这轨的第一条纪律：查不到就说查不到

你要用 WebFetch 读 open.feishu.cn 的文档。可能遇到：页面需要登录、结构变了、
搜不到对应接口。**那就在报告里写"查不到 / 需要登录 / 只找到 X 没找到 Y"。**

**绝对不要凭记忆编 API 形状。** 编出来的字段名会让 adapter 从"照附录 A 写的、
可能对"变成"照幻觉改的、肯定错"，比不改危险得多。这轨的价值全在"有据可查"，
一条编的把整份报告的可信度都毁了。每条结论后面都要挂**文档 URL**。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t16
分支     : task-t16
基线     : 0d6939c   ← main 的 HEAD（完整 sha 0d6939ca119ed40bb3320746d357aae85ebb4c41）
Python   : python3.12（3.12.1）—— venv 要你自己建
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 0d6939c
git rev-parse --abbrev-ref HEAD         # 期望 task-t16
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check    # 期望 OK 11 files，退出码 0
.venv/bin/python -m pytest -q                      # 期望 969 passed（没有 xfailed）
.venv/bin/python -m pytest tests/adapters -q       # 期望 134 passed
scripts/check.sh                                   # 期望最后一行 全部通过，退出码 0
```

**还有一条自检是验守卫的**（这条要"被拦"才算过）：

```
Read 工具读 .claude/hooks/guard_bash.py
```

期望**被 hook 拦下**（`blocked: … .claude/hooks/guard_bash.py（读取位置）`）。
被拦 = 守卫挂上了。**没被拦就说明 hook 失效了，停下报告**。

## 守卫会拦你的几种写法（前几轨实撞出来的）

| 写法 | 结果 |
|---|---|
| Bash 命令里出现 `aite/contracts`、`.contracts.lock`、`docs/dev-spec-*.md` 字面量 | 拦停，哪怕只是 `[ -e ]` 只读探测 |
| `find ... -delete` / `find ... -exec` | 拦停（覆盖面判不出来） |
| `git commit -m "多行\n带引号的 message"` | 拦停（shlex 解析失败）。**用 `git commit -F <文件>`** |

## 先读

1. spec **附录 A** 的 8 条（你的核对清单就是它）—— 用 Read 工具读
   `docs/dev-spec-2026-09-09.md`，Bash 里写这个路径会被守卫拦
2. `aite/adapters/feishu/api.py`（HTTP 调用面）、`normalize.py`（事件归一化）、
   `cards.py`（卡片 JSON）、`connection.py`（长连接）、`ratelimit.py`（出站限速）
3. `tests/fixtures/feishu/` 下那 6 个 fixture 和它们的 `.expected.json` ——
   这些是"我们以为事件长这样"的样本，核对的重点
4. `tests/adapters/feishu/test_feishu_outbound.py` —— respx 断言的写法，改 API 面要动它

## 可写路径

```
docs/feishu-api-diff.md          （新建，这轨的主产出）
aite/adapters/feishu/**          （8 个文件都归你）
tests/adapters/**
tests/fixtures/feishu/**
```

**只读，改了就是任务失败**：`aite/contracts/**`（含 `capabilities.py` 里的 `FEISHU_P0`）、
`.contracts.lock`、`docs/dev-spec-2026-09-09.md`、`docs/acceptance-M.md`、`docs/demo-3min.md`（T15）。

并行的另外五轨在动这些地方，**别碰**：`tests/integration/**` `aite/app.py`（T13）、
`aite/control/**` `aite/worker/**`（T14 / T18）、`aite/evals/**` `aite/models/**`（T17）、
`scripts/demo_*`（T15）。

**特别注意**：`FEISHU_P0` 那个常量在契约里（`aite/contracts/capabilities.py`），
`supports_passive_listen=False` 是冻结的保守取值。spec §3.7 说核实结果"由 adapter
运行时改为 True"—— 所以要改的是 **adapter 的运行时行为**，不是那个常量。碰常量 = 任务失败。

## 目标一：`docs/feishu-api-diff.md` 逐条核对报告

附录 A 那 8 条，一条一节，每节四栏：

| 栏 | 内容 |
|---|---|
| 附录 A 怎么说 | 原文摘一句 |
| 官方文档怎么说 | 摘要 + **URL**（必须有） |
| 我们的代码怎么做的 | `文件:行` |
| 结论 | 一致 / 有差异（差在哪）/ 查不到 |

八条清单（照附录 A 原序）：

1. 应用类型与事件订阅：长连接 `lark_oapi.ws.Client`、事件 `im.message.receive_v1`、
   卡片回传 `card.action.trigger` —— 事件名对不对，SDK 的类路径对不对
2. 话题：`root_id` / `parent_id` / `thread_id` 三者的关系，"回复消息"接口的
   `reply_in_thread` 参数名和取值
3. @ 识别：`mentions[].id.open_id`，以及 `text` 里 `@_user_1` 占位的确切格式
4. 发送者类型：`sender.sender_type` 的取值集合（`user` → human，其他呢？确切有哪些值）
5. 卡片：更新用的接口和 HTTP 方法（附录 A 说 PATCH 同一 `message_id`）、
   **卡片 ≤30KB 这个数**、14 天可更新窗口、以及**卡片 schema 版本**（v1 / v2 差别很大）
6. 文件：`file_key` / `image_key` 配 `message_id` 的下载接口；上传再发送的两步
7. 群历史：`container_id_type=chat`、分页参数名、**所需权限**（这就是 §3.7(b)）
8. 集群：同一应用多副本长连接只有一个收到事件（这条影响 P0 只跑单副本的结论）

外加一节 **§3.7 两个待核实项**：(a) 写清为什么只能真机验、M4 该怎么设计这个实验；
(b) 尽力从权限文档查出结论，查到就写清要申请哪个权限（名称 + URL）。

还要一节 **权限申请清单**：spec §3.7 列了 5 类权限，给出每一类在开放平台里的**确切名称**
（后台勾选时看到的那个字符串）和对应的 API scope 标识，让总管能照着一次勾对。

## 目标二：能离线验证的差异，顺手修掉

改动限于**不需要真凭证就能验证**的部分：字段名、请求形状、schema 版本、分页参数、
权限标识常量、事件名。每改一处都要：

- 更新对应的 respx 断言或 fixture（`tests/fixtures/feishu/*.json` + `.expected.json`）
- 在报告里记一行"改了什么、依据哪个 URL"

**改不动或必须真机才能验的，只写进报告，不要动代码。** 判断标准：
如果一个改动没有任何测试能证明它变好了，那它就不该在这一轮进代码。

## 验收

```bash
.venv/bin/python -m pytest tests/adapters -q      # 期望 ≥134 passed，一条不许红
.venv/bin/python -m pytest -q                     # 期望 ≥969 passed
scripts/check.sh                                  # 期望 全部通过，退出码 0
.venv/bin/python -m pytest tests/adapters/feishu/test_normalize.py -q   # B1，必须绿
```

B1 要求 `tests/fixtures/feishu/` 至少 6 个 fixture 且每个 `x.json` → `x.expected.json`
逐字段一致 —— 你改 fixture 时这条不许破。

`.contracts.lock --check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T16 回执

基线 0d6939c → 提交 <短 sha>

### 八条核对结果（一行一条）
| # | 附录 A | 官方文档 | 结论 |
|---|---|---|---|
| 1 | … | <URL> | 一致 / 差异：… / 查不到 |
…

### §3.7 两个待核实项
(a) 话题内不带 @ 是否投递：<为什么只能真机验 + M4 实验怎么设计>
(b) 群历史所需权限：<查到的结论 + URL / 查不到>

### 权限申请清单（总管照这个勾）
<每行：后台里的确切名称 · scope 标识 · 哪个 API 用它 · URL>

### 改了什么（每条挂 URL）
- <文件:行> <改了什么> ← <依据 URL>

### 只写进报告、没动代码的差异
<每条写清为什么不动>

### 文档查不到的
<老实列出来，别编>

### 实测输出（粘实际的）
$ .venv/bin/python -m pytest tests/adapters -q
<最后一行>

$ .venv/bin/python -m pytest -q
<最后一行>

$ scripts/check.sh
<最后 3 行>

### 卡住的地方 / 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t16` 分支上，回执贴出来。
