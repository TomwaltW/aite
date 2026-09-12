# 任务 T21 — 崩溃残行：让证据链自己站起来，而不是自锁

## 背景：这轨是从哪来的

T13–T18 六轨全部合进 main（最新 `6ee30d4`）。全仓 **1061 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过，契约锁 `OK 11 files`。P0 的代码面齐了，剩 §2.4 的 M1–M6
卡在飞书凭证。

T18 那一轨去验「`kill -9` 之后重开会怎样」，在 `aite/evidence/writer.py` 上撞出两个真 bug，
但那个文件不在它的可写面，所以它只把**当前行为钉成了测试**，改留给你：

`tests/integration/test_t18_crash_recovery.py:174` 和 `:195` 这两条，名字里都写着
**「钉住现状（真 bug）」**。你这一轨的交付形态很特别：**把这两条断言反过来。**

### bug 1 — 冷 writer 一碰就炸（`:174`）

进程在 `fh.write(...)` 中途没了，`events.jsonl` 末尾留下半行 JSON。重启后新建的
`FileEvidenceWriter`（`_tip` 缓存是空的）一调 `append()` 或 `finalize()` 就走
`_chain_tip` → `_read_events`（`writer.py:159`）：

```python
return [
    EvidenceEvent.model_validate_json(line)          # ← 没有 try/except
    for line in path.read_text(encoding="utf-8").splitlines()
    if line.strip()
]
```

半行喂给 pydantic 直接抛 `ValidationError`。

**为什么这条比看上去严重**：§3.3 最后一行写着「任何未捕获异常 → task `failed` + 回帖 +
evidence `failed` 事件；进程不退出」。而崩溃之后**写那条 `failed` 证据本身就是炸的那个操作**。
收尾链自锁：越是出事的时候越写不进去。

### bug 2 — 热 writer 静默坏链（`:195`）

同一个实例里 `_tip` 还热着，`_chain_tip` 就不去读文件了，新证据被**直接接在残行后面**：
两条 JSON 挤成一行。**全程不报错**，但 `verify()` 从此永远 `False`。
真实触发点是磁盘满 —— `write` 只落一半，进程还活着。

对照组：`verify()`（`writer.py:123`）自己是有 `try/except ValueError → return False` 的，
所以它对残行的判断是对的。**只有写入这条路上没有。**

## 这轨的形状

让写入侧也能面对一个「上一条命写到一半」的文件：识别出残行、收拾干净、把发生过这件事
记下来，然后继续写。**不要让它变成「无脑容错」** —— 中间某一行被人改坏和末尾写了一半
是两回事，前者必须继续报 `False`。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t21
分支     : task-t21
基线     : 6ee30d4   ← main 的 HEAD（完整 sha 6ee30d4e6013782ff7cdd4e69ec8efa101f4b2db）
Python   : python3.12（3.12.1）—— venv 要你自己建
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-t21
git log --oneline -1                            # 期望 6ee30d4 ...
git status --short                              # 期望空
.venv/bin/python -m pytest -q                   # 期望 1061 passed
.venv/bin/python -m pytest tests/evidence -q    # 期望 14 passed
.venv/bin/python -m pytest tests/integration -q # 期望 36 passed
scripts/check.sh                                # 期望 全部通过，退出码 0
```

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
被拦才说明 hook 挂上了；没被拦说明 `CLAUDE_PROJECT_DIR` 不对，所有守卫都在静默失效，
停下报告。

**关于 `tests/sandbox` 假红（先看这条，能省你十分钟）**：六轨同时跑测试时
`tests/sandbox/test_docker_sandbox.py` 会红 1–3 条，每次红的用例还不一样。根因是
`docker ps -a --filter label=aite.task` 是**全机器**的命名空间。**判据**：红的只在
`tests/sandbox/`、失败断言形如 `labelled_ids() == []` 或 `reap_idle(...) == []`、
单跑 `pytest tests/sandbox -q` 是 **36 passed** → 串扰不是回归，等别轨安静了重跑。

## 可写路径（白名单，之外一律只读）

```
aite/evidence/**                                   ← 主战场（writer.py）
tests/evidence/**                                  ← test_chain.py / test_manifest.py
tests/integration/test_t18_crash_recovery.py       ← 只有这一个 integration 文件归你
```

**邻居**：`tests/integration/` 下别的文件归 **T22**（它在接 `recover_orphan_tasks`）
和基线（T8/T13 那几条）；`aite/control/store.py` 归 T18 已合入，只读。
`aite/contracts/evidence.py` 是**契约**，一个字都不许动。

## 要做什么

### ① 分清「写了一半」和「被改坏」

这是本轨的核心判断，先把口径定下来再动手：

- **末尾一行不完整** = 上一条命写到一半。那条记录从来没写完，也就从来没有效过 —— 收拾掉，
  链在它之前是完整的。
- **中间某一行不合法 / hash 对不上 / seq 不连续** = 篡改或真损坏。**必须继续报 `False`，
  不许"修复"**。`tests/integration/test_t8_evidence_on_disk.py` 里有篡改检测的用例，
  `tests/evidence/test_chain.py` 里也有，那些必须一条不红。

### ② 让写入侧能自愈

`_read_events` / `_chain_tip` / `append` 这条路上要能面对残行。几个必须想清楚的点：

- **是只在内存里跳过残行，还是真的把文件收拾干净？** 只跳过的话，文件永远是坏的，
  下一条新证据写进去以后 `verify()` 还是 `False`，等于没修。倾向真收拾（把文件截到
  最后一个完整记录的换行处），但你要自己论证，并写清「截掉的那半行去哪了」。
- **`append` 写之前怎么确认文件是以换行结尾的？** 这一条同时能挡住 bug 2（热缓存那条），
  因为它不依赖 `_tip` 是冷是热。成本要低 —— 这是每条证据都会走的路，别每次都全文读一遍。
- **收拾这件事要留痕**：静默截掉一条记录，跟静默坏链一样难查。计一个 counter、打一条
  WARNING 日志，或者别的方式，你定；但「发生过一次崩溃恢复」这件事必须能被看见。
  （`scripts/evidence_show.py` 是 T10 建的时间线工具，只读，可以去看它怎么讲故事。）
- **`finalize()` 也走 `_read_events`**：manifest 里的 `event_count` 和 `root_hash` 会不会
  因为你的收拾而变？变了对不对？（我认为对 —— 那半行本来就不算一条事件 —— 但要写进回执。）

### ③ 把 T18 钉的两条断言反过来

`tests/integration/test_t18_crash_recovery.py:174` / `:195` 现在断言的是 bug 存在
（`pytest.raises(ValueError)` 和「坏链」）。改完它们应该断言的是修好之后的行为。
**docstring 里那句「钉住现状（真 bug）」一并改掉**，别让下一个人以为它还是 bug。

### ④ 补测试

至少要覆盖：冷 writer 遇残行能接着写且链是通的；热 writer（磁盘满形状）遇残行不再坏链；
中间行被篡改仍然 `verify() == False`；残行是**空文件 / 只有半行 / 半行在多条完整记录之后**
三种形状；`finalize()` 在残行之后能正常出 manifest。

## 纪律

1. 契约（`aite/contracts/**`，含 `evidence.py`）一个字都不许动，锁必须全程 `OK 11 files`。
   §3.1 里那两个 hash 测试向量和 `manifest` 的字段集是冻结的。
2. 白名单之外的文件只读。要改别人的面 → **停下报告**。
3. 「容错」不许扩大成「什么都能吞」。篡改检测是这套证据链存在的理由，弄丢它这一轨就白做了。

## 验收

```bash
.venv/bin/python -m pytest tests/evidence -q      # 期望 >14 passed，一条不许红
.venv/bin/python -m pytest tests/integration -q   # 期望 ≥36 passed
.venv/bin/python -m pytest -q                     # 期望 ≥1061 passed
scripts/check.sh                                  # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
```

特别盯 `tests/integration/test_t8_evidence_on_disk.py`（5 条，落盘 + 篡改检测）和
`tests/evidence/test_chain.py` —— 基线那些一条都不许弄红。
契约锁 `--check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T21 回执

基线 6ee30d4 → 提交 <短 sha>

### 口径
末尾残行怎么判：<>
中间坏行怎么判：<>
两者是怎么区分开的：<给出实现依据，不是"应该能区分">

### 修法
文件收拾了没有：<截了 / 只在内存跳过，为什么>
append 怎么确认文件以换行结尾：<方法 + 每条证据的额外成本>
留痕方式：<counter / 日志 / 别的>
finalize 的 event_count 与 root_hash 变了吗：<变没变，为什么对>

### T18 那两条断言
:174 <原来断言什么 → 现在断言什么>
:195 <原来断言什么 → 现在断言什么>

### 新增测试（几条，各覆盖什么）
- <>

### 篡改检测还在吗
<贴出 test_t8_evidence_on_disk.py 和 tests/evidence 的实测输出>

### 改了什么
- <文件:行> <一句话>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m pytest tests/evidence -q
<最后一行>

$ .venv/bin/python -m pytest tests/integration -q
<最后一行>

$ scripts/check.sh
<最后 3 行>

### 还没解决的 / 要总管决定的
<比如：磁盘满这条路上还有没有别的坑；没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t21` 分支上，回执贴出来。
