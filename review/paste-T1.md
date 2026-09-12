# 任务 T1 — 飞书 Adapter（Aite P0）

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t1
分支     : task-t1
基线     : 0fa8348   ← main 的 HEAD，四轨钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。文档正文里的 `python` 字样照原样保留，
执行时换成 `.venv/bin/python` 即可。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 0fa8348
git rev-parse --abbrev-ref HEAD         # 期望 task-t1
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check      # 期望 OK 11 files，退出码 0
.venv/bin/ruff check .                               # 期望 All checks passed!，退出码 0
.venv/bin/python -m pytest -q --co                   # 期望 335 tests collected
.venv/bin/python -m pytest tests/contracts -q        # 期望 335 passed
```

上面这些是实测值，不是估计。

**第 9 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 如果你读到了内容，说明 PreToolUse 守卫没生效 —— 多半是
会话不是从 worktree 根启动的（`CLAUDE_PROJECT_DIR` 在进程启动那一刻定死，之后 `cd`
补不回来），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。这种情况下停下报告。

## 先读

`docs/dev-spec-2026-09-09.md` 全文。本轮唯一权威文档，**不许改**。
重点：§3.1（events / outbound / capabilities）、§3.2 的 `PlatformPort`、§3.3 失败面里
adapter 那几行、§3.4 归属表 T1 那行、§6 的 T1 那一行、附录 A 的飞书 API 备忘。

## 可写路径（§3.4，只有这三条）

```
aite/adapters/feishu/**
tests/adapters/**
tests/fixtures/feishu/**
```

其余一律只读。`aite/contracts/**`、`.contracts.lock`、`docs/dev-spec-*.md` 有守卫拦着，
改动即任务失败。别的轨的目录（`aite/control/`、`aite/sandbox/`、`aite/models/` …）对你**不可见**——
不要读、不要依赖、不要以为里面的 stub 是最终形态。

`aite/adapters/feishu/platform.py` 里现在是 T0 落的 `FeishuPlatform` 空实现，
方法签名与 §3.2 的 `PlatformPort` 逐字对齐过。**它是你的文件，随便改**：改名、加
`__init__` 参数、加私有辅助方法都行。

## 目标

把 `PlatformPort`（§3.2）在飞书上落成真的：长连接收事件 → 归一化成 `NormalizedEvent`
→ 出站（文本 / 卡片 / 文件 / 表情）→ 群历史与云文档读取。

具体：

1. **长连接**：`lark_oapi.ws.Client`，订阅 `im.message.receive_v1` 与 `card.action.trigger`。
   `start(on_event)` 建连并持续投递；`on_event` 必须在 1s 内返回（只做入队），重活不在回调里做。
   断线指数退避重连 1s→2s→…→30s 封顶，无限重试，重连成功打 INFO 日志 `feishu.reconnected`。
   **adapter 不负责去重**（去重是 ControlPlane 靠 `event_id` 做的，§3.3）。
2. **归一化**：飞书事件 → `NormalizedEvent`。要点：
   - `sender.sender_type`：`user` → `SenderKind.human`，其余 → app/bot
   - @ 识别：`mentions[].id.open_id` 与 `FEISHU_BOT_OPEN_ID` 比对；`text` 里的 `@_user_1`
     占位要剥掉，`text` 存的是**去掉 @Aite 后 strip 过的纯文本**，非文本消息为 `""`
   - 话题锚点：`root_id` / `parent_id` / `thread_id` → `Anchor.thread_id`（顶层消息为 None）
   - `raw` 存原始事件仅供审计，**任何逻辑不得依赖 raw**
3. **出站**：`send_text` / `send_card` / `update_card` / `send_file` / `add_reaction`。
   `update_card` **必须原地更新同一条消息**（PATCH 同一 `message_id`），绝不新发消息。
   出站限速按 `FEISHU_P0.outbound_rate_per_min`（60/分钟），adapter 自己令牌桶。
4. **读取**：`read_history`（按时间正序返回，**不做 sender_kind 过滤**——过滤归 T3 的
   Gateway 工具）、`read_document`、`download_file`。
5. **失败面**（§3.3）：429/5xx 退避重试 3 次（0.5s / 1s / 2s），仍失败抛
   `PlatformError(code, retryable=True)`；4xx（非 429）不重试，抛 `retryable=False`。

## ⚠️ §3.7 飞书权限：总管尚未核实，你这里拿不到结论

spec §3.7 写明「T1 起飞前由总管核实，结果写在 T1 的 dispatch 里」。**这份派单交出来时
总管还没填**。待核实的两点：

- (a) 只有「接收群聊中 @ 机器人消息」权限时，话题里**不带 @** 的回复是否会投递给应用
- (b)「获取会话历史消息」API 是否要求「获取群组中所有消息」这个敏感权限

**在总管补上之前你怎么做**：按契约默认值走，即 `FEISHU_P0.supports_passive_listen = False`
保持不变，`read_history` 照实现。§3.7 明说这两点「只影响 `supports_passive_listen` 的
**运行时**取值和 M4 的操作方式，不影响任何契约」，所以**不阻塞你**。

但有两条硬要求：
- 把 `supports_passive_listen` 做成**运行时可改**的（adapter 实例上的能力副本），
  不要就地改 `FEISHU_P0` 这个契约常量 —— 它是全局共享的可变单例，改了会污染所有引用方。
  T0 落的 stub 已经用 `FEISHU_P0.model_copy()`，沿用这个做法。
- 在回执里单独列一节，写清你实现时对 (a)(b) 做了什么假设。

## 不要做的事

- 不要实现 ControlPlane / worker / 沙箱 / 模型客户端 —— 那是别人的格子。
- 不要 `import aite.testing`（T4 的官方替身，并行期间还是空包）。你要 FakePlatform / 假连接
  对象，就在 `tests/adapters/` 下写**私有版本**。§3.4 原话：重复远比冲突便宜。
- fixture 与 pytest fixture 放 `tests/adapters/` 下自己的 `conftest.py`，**不要动
  `tests/conftest.py`**（那是 T0 的，里面是 worktree 的 sys.path 引导，动了你就测到别人的树上去了）。
- 不要加 §3.0 以外的依赖。需要新依赖 → 停，报告（`pyproject.toml` 归 T0）。
- 契约里找不到你需要的接口 → **停，报告**，不要自己发明一个。

## 验收（自己跑，全绿才算完）

必须通过 **A2–A5, C1, C2, B1**：

```bash
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # A2  p0.1
.venv/bin/python -m aite.contracts.lock --check          # A3/C2  始终 OK 11 files
.venv/bin/ruff check .                                   # A4  退出码 0
.venv/bin/python -m pytest -q --co                       # A5  退出码 0，无 ImportError
.venv/bin/python -m pytest tests/contracts -q            # C1  >= 335 passed，不得减少
.venv/bin/python -m pytest tests/adapters/feishu/test_normalize.py -q   # B1  全绿
```

独有的额外验收（§6 T1 原文）：

- `tests/fixtures/feishu/` 至少含这 6 个 fixture，每个都有 `.expected.json`，
  `x.json` → `x.expected.json` 逐字段一致：
  `message_at_bot_toplevel`、`message_in_thread_no_at`、`message_in_thread_with_at`、
  `message_from_bot`（sender_kind=bot 或 app）、`message_with_file`、`card_action_stop`
- `pytest tests/adapters -q` 里要有**出站测试**：`ChecklistCard` → 飞书卡片 JSON 通过
  SDK/Schema 构造不报错；且 `update_card` 走的是「更新消息」而非「发送消息」——
  用 respx 断言 HTTP 方法与路径
- 重连策略对假连接对象的退避序列断言为 `[1, 2, 4, 8, 16, 30, 30]`

**C1 说明**：`tests/contracts` 是 T0 独占、你只读。T0 已经把「stub 必须还是空实现」那类
断言全部撤掉了，所以你把 `FeishuPlatform` 写成真的**不会**打红 C1。如果你发现自己非改
`tests/contracts` 不可才能变绿 —— **停，报告**，那说明判据错了，不是你错了。

**测试文件命名**：`tests/` 下没有 `__init__.py`，同名测试文件在并轨时会让 pytest 报
import file mismatch。你的文件都放在 `tests/adapters/` 下，命名带 `feishu` 语义即可，
不要起 `test_client.py` / `test_utils.py` 这种四条轨都可能用的名字。

## 卡住了怎么办（§7）

1. 需要的接口在契约里找不到 → **停，报告**。不要自己发明。
2. 需要改的文件不在白名单里 → **停，报告**。不要「就改一行」。
3. 需要新的第三方依赖 → **停，报告**。
4. 验收命令跑不起来（环境问题）→ 先自己排查，排查不动 → 报告。
5. 验收过不了但代码自认为对 → **以验收为准**，继续改；确信验收写错了 → 停，报告。
6. 飞书 API 与附录 A 不一致 → **以 open.feishu.cn 官方文档为准**，在回执里写明差异；
   只要契约不变就不用停。

## 完成后

```bash
git add -A && git commit -m "T1: 飞书 Adapter"
```

不要 push，不要合并到 main —— 并轨归 TΩ。

然后输出回执，最后一行严格按这个格式：

```
RECEIPT T1 status=<done|blocked> commit=<短hash> checks=<通过数>/6 files=<改动文件数>
```

（checks 的分母 6 = A2 / A3 / A4 / A5 / C1 / B1。）

回执正文包含：
- A2–A5、C1、B1 每条的实际输出（关键几行，不是「通过了」）
- C1 的实际条数，与基线 335 对比
- 6 个 fixture 的清单与各自验的是什么
- 出站测试怎么断言「更新消息 ≠ 发送消息」的（贴 respx 断言那几行）
- 退避序列 `[1,2,4,8,16,30,30]` 的实际断言输出
- §3.7 那两点你做了什么假设
- 与附录 A 不一致的飞书 API（如有）
- `git show --stat HEAD` 的文件树
- 如果 blocked：卡在哪、试过什么、需要什么决定
