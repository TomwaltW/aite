# 任务 T23 — 真沙箱评测：把 M3 那条主干真的跑通一次

## 背景：这轨是从哪来的

T13–T18 六轨全部合进 main（最新 `6ee30d4`）。全仓 **1061 passed**，评测 **passed 10/10**，
`scripts/check.sh` 全部通过，契约锁 `OK 11 files`。P0 的代码面齐了，剩 §2.4 的 M1–M6
卡在飞书凭证。

**但有一条路，到今天为止一次都没有真的走通过。**

M3 / `04_csv_to_chart` / 3 分钟演示第二幕，走的是同一条：
**附件 → `download_attachment` → `run_python` 画图 → `final(artifacts)` → `send_file` 回线程。**

它被验过三次，每次都缺一块：

| 验过的地方 | 模型 | 沙箱 | 缺什么 |
|---|---|---|---|
| `evals/p0/04_csv_to_chart.yaml`（`passed 10/10` 的一员） | `FakeModel` 按脚本 | `FakeSandbox` 按 `exec_script` | 两头都是演的 |
| `tests/sandbox`（B4，36 条） | 没有模型 | **真 Docker** | 没有模型驱动，也不过 Gateway |
| T17 的 `--model live`（`evals/live-report-2026-09-10.md`） | **真 DeepSeek** | `FakeSandbox` | **两次都飞了** |

T17 那次飞得很典型：替身沙箱对不含 `savefig` 的代码一律回 `exit_code=0` + 空 stdout，
模型想先读一眼 CSV 就撞上空返回，自己诊断了几步（原话 `The sandbox is returning empty
output for every command`），然后对逐字节相同的 `run_python` 连发 33 次烧满 `max_steps`。

**换句话说：真模型 + 真沙箱这个组合，今天在仓库里根本造不出来。**
`aite/evals/wiring.py:122` 是写死的：

```python
sandbox = FakeSandbox(exec_script=list(sc.sandbox.exec_script))
gateway = FakeToolGateway(platform=platform, sandbox=sandbox, sandbox_image=config.sandbox.image)
```

`__main__.py:36` 也是写死的：`PLATFORMS = ("fake",)`、`MODELS = ("scripted", "live")`，
**没有 sandbox 这一档**。

真机 M3 第一次跑出问题，排查成本比现在高得多 —— 那时候你手上只有群里一条没回的消息。

## 这轨的形状

给评测加一档 `--sandbox docker`，然后**拿真模型 + 真 Docker 把 `04` 跑到 delivered**，
把过程中撞到的每一件事写下来。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t23
分支     : task-t23
基线     : 6ee30d4   ← main 的 HEAD（完整 sha 6ee30d4e6013782ff7cdd4e69ec8efa101f4b2db）
Python   : python3.12（3.12.1）—— venv 要你自己建
```

本机默认 `python3` 是 **3.11.7**，满足不了 `requires-python>=3.12`，**必须用 `python3.12`**：

```bash
python3.12 -m venv .venv
.venv/bin/python -m pip install -q -e ".[dev]"     # 期望退出码 0
```

镜像 `aite-sandbox:p0` 本机已经有了（2026-09-09 建的）。没有就 `docker build -t
aite-sandbox:p0 docker/sandbox`（那个目录只读，别改）。

## 真模型凭证

跑 live 要 `model.base_url` / `model.model`（`config/aite.yaml`，总管填）+ 环境变量
`AITE_MODEL_API_KEY`。上一轮总管**当场授权**借用 DeepSeek 那份（端点
`https://api.deepseek.com/v1`，模型 `deepseek-chat`），全套 10 个场景约 ¥0.10。

**起来时这三样缺任何一样 —— 停下问总管。不要去 `~/.bash_profile` 或别的地方自己翻密钥。**
拿不到 key 的话：`--sandbox docker` 那半边照做（`--model scripted --sandbox docker`
也能跑，验的是「真沙箱能不能替上」），真模型那半边写「待 key」，**不要拿 scripted 冒充
live**，也不要写「真模型大概会……」这种推测。`config/aite.yaml` 不许进 git。

## ⚠️ 这一轨会把别轨的测试弄红 —— 先读这条

`tests/sandbox/` 里有几条断言是 `docker ps -a --filter label=aite.task` 为空，而那个
label 是**全机器共享**的命名空间。上一轮六轨里五轨都撞过：别人跑 sandbox 测试时，
你的容器会被人家的 `reap_idle` 收走；你跑的时候也会把别人的收走。

**这一轮你是这个串扰的主要来源** —— 只有你会长时间开着真容器。所以：

- 每次真沙箱跑完，确认容器收干净了（`docker ps -a --filter label=aite.task`）。
- 别把 `--sandbox docker` 挂进 `scripts/check.sh` 或任何默认路径（那会让全仓测试
  从此依赖 Docker daemon，CI 上直接红）。
- 你自己看到 `tests/sandbox` 红 1–3 条、每次红的不一样时，**同样的判据**：单跑
  `pytest tests/sandbox -q` 是 **36 passed** → 串扰不是回归。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-t23
git log --oneline -1                        # 期望 6ee30d4 ...
git status --short                          # 期望空
.venv/bin/python -m pytest -q               # 期望 1061 passed
.venv/bin/python -m pytest tests/e2e -q     # 期望 200 passed
scripts/check.sh                            # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
docker image inspect aite-sandbox:p0 -f '{{.Id}}'   # 期望有 sha256:…
```

还有一条：**Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。**
被拦才说明 hook 挂上了；没被拦说明 `CLAUDE_PROJECT_DIR` 不对，所有守卫都在静默失效，
停下报告。

## 可写路径（白名单，之外一律只读）

```
aite/evals/**                         ← 主战场（__main__.py / wiring.py）
evals/**                              ← 场景 yaml + 你的实测报告
tests/e2e/**                          ← 评测自己的测试
```

**邻居**：`aite/worker/loop.py` 归 **T20**（它在加原地打转的兜底，口径要和
`protocol_probe.py::_repeat_loops` 对齐 —— 那个文件是你的，它只读，**别改口径的时候
不打招呼**）；`aite/worker/prompts/platform.md` 归 **T19**；`aite/sandbox/**` /
`aite/gateway/**` / `docker/sandbox/**` **只读**（要改 → 停下报告）。

## 要做什么

### ① 加 `--sandbox {fake,docker}`

照 `--platform` / `--model` 现成的形状加（`__main__.py:36` 的常量、`add_argument`、
一路传到 `wiring.build_deps`）。默认必须是 `fake` —— **默认路径逐字节不变**是这一轨的
硬约束（`scripts/check.sh` 的 B8、CI、T4 的 200 条都吃这条路）。

`docker` 那一档要造的是**真** `DockerSandbox` + **真** `ToolGateway`：

- `aite/sandbox/docker_sandbox.py` 里的 `DockerSandbox`（`aite/app.py:_build_sandbox()`
  是现成的参考，连 `SandboxSpec` 怎么从 config 出来都写好了：`aite/app.py:_sandbox_spec`）。
- 真 `ToolGateway`（`aite/gateway/tool_gateway.py:68`）的构造面是
  `platform / sandbox / sandbox_spec / token_resolver / …`，跟 `FakeToolGateway` 不是一套。
  **`token_resolver` 那一格要当心**：真 Gateway 要校验 `session_token`，而 `Task.session_token`
  是 ControlPlane 建任务时才生成的。生产侧的接法在 `aite/app.py` 的 `AppWorker.run` 里
  （任务开跑那一刻登记）—— T13 做过变异检验：把那三行摘掉，八条集成用例红六条。
  评测这边怎么接，你自己看 `wiring.py` 是怎么发现并构造 ControlPlane 的再定。
  **接不上就是每个工具调用都 `denied`**，这会是你第一个撞上的墙。
- 平台仍然是 `FakePlatform`（附件从它来，`04` 的 `platform.files.content: "builtin:csv"`）。
  真沙箱要能从假平台拿到那份 CSV 并落进 `/work/in/`。

场景 yaml 里的 `sandbox.exec_script` 在 docker 档下自然就没用了 —— 想清楚是忽略、
还是显式报错说「这个场景是给 fake 沙箱写的」。

### ② 拿真模型 + 真沙箱把 `04` 跑到 delivered

这是靶心。`04` 的 `expect` 全是 Gateway / 平台层的判据，**不吃替身内部**，所以真沙箱下
它们本来就该成立：

```yaml
- {check: gateway_result, name: run_python, ok: true, contains: "exit_code=0"}
- {check: platform_calls, method: send_file, equals: 1}
- {check: file, index: 0, magic: "89504e470d0a1a0a"}     # 真 PNG 的魔数
- {check: task, which: last, status: delivered}
```

也就是说：**跑通的话，那张 PNG 是镜像里的 matplotlib 真画出来的。**

跑法：

```bash
.venv/bin/python -m aite.evals run evals/p0 --only 04_csv_to_chart \
    --platform fake --model live --sandbox docker --protocol-report /tmp/t23-04.json
```

先用 `--model scripted --sandbox docker` 跑通（排掉沙箱接线的问题），再上 `--model live`
（这时才是模型的问题）。**两步分开，不然出了错你不知道该怪谁。**

真模型有随机性，`04` 至少跑**三遍**再下结论 —— T17 那轮它是 2/2 都飞，你要能说出
「现在是 k/3 通」。

### ③ 把撞到的每一件事写下来

新建 `evals/live-report-2026-09-10-t23.md`（或你觉得更好的名字）。至少要答：

- **真沙箱下模型的出牌变了吗？** 空返回消失之后，它还打转吗？（打转那条兜底归 **T20**，
  你只如实记录，**别去修 `loop.py`**）
- **一趟真沙箱评测要多久、几只容器、收干净了吗？**（`W7` 说任务结束不立即释放，交给
  `reap_idle`；评测这条路上谁来 reap？）
- **`--timeout-scale`（live 默认 12.0）够不够？** 真沙箱起容器 + 装载 + 画图比替身慢得多。
- **别的 9 个场景在 docker 档下会怎样？** 能跑就跑一遍全套（`--model scripted --sandbox
  docker`），把哪些场景在真沙箱下不成立列出来 —— 它们多半是给替身写的（比如
  `08_step_limit` 压根不碰沙箱）。**不成立不等于要改 yaml**，先报告。

### ④ 测试

`tests/e2e/` 里补上：`--sandbox` 的参数解析与默认值；docker 档在**没有 Docker daemon**
时给人话而不是异常栈；默认 fake 档的行为逐字节没变。
**真容器的用例要打 `@pytest.mark.docker` 并在 daemon / 镜像缺席时 skip**
（`tests/tools/test_demo_fixture.py:206` 有现成的 `_docker_ready()` 写法可以照抄）。

## 纪律

1. 契约（`aite/contracts/**`）一个字都不许动，锁必须全程 `OK 11 files`。
2. **默认路径不许变**：`--platform fake --model scripted`（不带 `--sandbox`）必须还是
   `passed 10/10`，`--list` 还是那 10 个场景名，`scripts/check.sh` 不依赖 Docker。
3. 白名单之外的文件只读。`aite/sandbox/**` 和 `aite/gateway/**` 要改 → **停下报告**。
4. 每条结论挂实测。推测一句不要。

## 验收

```bash
.venv/bin/python -m pytest tests/e2e -q          # 期望 ≥200 passed，一条不许红
.venv/bin/python -m pytest -q                    # 期望 ≥1061 passed
scripts/check.sh                                 # 期望 全部通过，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted   # 期望 passed 10/10
.venv/bin/python -m aite.evals run evals/p0 --list                             # 期望还是那 10 个场景名
git status --porcelain                           # 期望里面没有 config/aite.yaml
docker ps -a --filter label=aite.task            # 收工时期望是空的
```

契约锁 `--check` 必须始终 `OK 11 files`（C2）。

## 回执格式

```
## T23 回执

基线 6ee30d4 → 提交 <短 sha>

### --sandbox 怎么加的
CLI：<>
wiring：<真 DockerSandbox / 真 ToolGateway 怎么造的>
session_token 怎么接上的：<这条单独写，它是第一堵墙>
场景 yaml 里的 exec_script 在 docker 档下：<忽略 / 报错，为什么>

### 04_csv_to_chart 真沙箱实测
--model scripted --sandbox docker：<通/不通，几遍>
--model live --sandbox docker：<k/3 通>
那张 PNG 是真画出来的吗：<魔数 + 字节数 + 你有没有真的看过图>
真模型在真沙箱下还打转吗：<T17 那次 33 连发，现在什么样>
一趟多久 / 几只容器 / 收干净了吗：<>
--timeout-scale 够不够：<>

### 全套在 docker 档下（--model scripted --sandbox docker）
passed k/10，不成立的场景 + 原因：<别改 yaml，先报告>

### 改了什么
- <文件:行> <一句话>

### 实测输出（粘实际的，不要写"通过了"）
$ .venv/bin/python -m pytest -q
<最后一行>

$ .venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
<最后一行>

$ scripts/check.sh
<最后 3 行>

$ docker ps -a --filter label=aite.task
<收工时的实际输出>

### M3 现在的把握有多大
<拿实测说话：这条路真模型 + 真沙箱下通了几次、还差什么>

### 要总管决定的
<比如：演示要不要用真沙箱；docker 档要不要进 CI（我倾向不进）。没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-t23` 分支上，回执贴出来。
