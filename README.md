> **2026-09-11 起：本仓库正在用 Rust + Go 重写。** 权威文档是 `docs/dev-spec-2026-09-11-rustgo.md`。
> 新代码在 `core/`（Rust，状态与判定面）、`edge/`（Go，飞书与 Docker 对外连接面）、`proto/`（两者的 gRPC 契约）。
> `aite/`、`tests/`、`pyproject.toml` 这棵 Python 树是**移植参考，只读**，RΩ 合流后整体删除。
> 常用命令：`make build` / `make test` / `make lint` / `make lock` / `scripts/check.sh`。
> 下面是 Python 版（P0）的原 README，移植期间仍可按它跑旧代码。

# Aite

飞书单平台闭环 P0：群里 @Aite → 线程绑定会话 → Checklist 卡片原地更新 → 沙箱执行 →
结果与文件回到线程。

唯一权威文档是 [`docs/dev-spec-2026-09-09.md`](docs/dev-spec-2026-09-09.md)（已冻结）。
本 README 只讲「怎么跑起来」；口径冲突时以 spec 为准。

## 环境

- Python **3.12+**（`pyproject.toml` 里 `requires-python = ">=3.12"`）
- Docker（沙箱要用；不跑沙箱测试时可以没有）
- 依赖表冻结在 spec §3.0，新增依赖要先找总管

```bash
python -m venv .venv && . .venv/bin/activate
python -m pip install -e ".[dev]"
```

## 跑起来

```bash
cp config/aite.example.yaml config/aite.yaml     # 按需改；aite.yaml 不入库
export FEISHU_APP_ID=... FEISHU_APP_SECRET=... FEISHU_BOT_OPEN_ID=...
export AITE_MODEL_API_KEY=...                    # 变量名由 config 里的 *_env 决定
python -m aite.app
```

**密钥只走环境变量。** 配置文件里存的是变量名（`api_key_env: AITE_MODEL_API_KEY`），
不是取值 —— 别把密钥写进 `config/aite.yaml`，也别写进代码。

容器方式（单副本；同一飞书应用多副本长连接只有一个能收到事件）：

```bash
docker build -t aite-sandbox:p0 docker/sandbox   # 沙箱镜像
docker compose up -d
```

`docker-compose.yml` 把宿主机的 docker socket 挂了进去：沙箱不是 compose 里的
service，而是由 `aite/sandbox/` 按任务起的兄弟容器（打标签 `aite.task=<task_id>`）。

## 验收

spec §2 那几条，`make` 里一条一个 target，名字就是编号：

```bash
make check          # A2 A3 A4 A5 C1 + 替身自测 + 场景清单 + compose config
make a3             # python -m aite.contracts.lock --check
make a4             # ruff check .
make a5             # pytest -q --co
make c1             # pytest tests/contracts -q
make e2e            # pytest tests/e2e -q
make evals          # 10 个 P0 场景
scripts/check.sh    # 同上，但把每条的实际输出都打出来（写回执时用这个）
```

`make` 挑解释器的顺序是 `.venv/bin/python` → `python` → `python3`；
指定就 `make test PYTHON=/path/to/python`。CI 里用的是 `python`。

## 目录

```
aite/contracts/     冻结契约（§3.1 数据形状 / §3.2 调用面），改动 = 任务失败
aite/adapters/      飞书 adapter：长连接、事件归一化、出站
aite/ingress/       事件入口
aite/control/       ControlPlane 路由（§3.5 R1-R8）+ SQLite SessionStore
aite/worker/        Agent Loop、Checklist 协议、产出（§3.6 W1-W9）
aite/gateway/       Tool Gateway；aite/tools/ 是五个 Gateway 工具
aite/sandbox/       Docker 沙箱；docker/sandbox/ 是镜像
aite/evidence/      证据链落盘
aite/models/        ModelPort 的 live 实现（OpenAI 兼容，指向百炼/智谱）
aite/testing/       官方测试替身
aite/evals/         评测 runner
evals/p0/           §3.8 的 10 个场景
```

## 测试替身（`aite/testing/`）

`FakePlatform` / `FakeModel` / `FakeSandbox` / `FakeToolGateway` /
`FakeSessionStore` / `FakeEvidenceWriter` —— §3.2 六个 Port 的官方替身。
它们只**记账**不断言：谁被以什么参数调用了全进 `CallLog`，断言留给使用方。

```python
from aite.testing import FakeModel, FakePlatform

platform = FakePlatform(history=[...], documents={...}, files={...})
model = FakeModel([{"tool_calls": [{"name": "final", "arguments": {"reply": "好了"}}]}])
...
assert platform.count("send_card") == 1
assert platform.update_count >= 3
assert platform.card_count == 1          # 没有第二条卡片
```

几个刻意做严的地方：

- `update_card` 只认已存在的 `card_id`，拿别的 id 更新会当场抛错 —— 「原地更新，
  不新发消息」这条必须在替身层就炸，而不是绕着弯被断言发现。
- `FakePlatform.read_history` **不**过滤 `sender_kind`（契约注释写死：过滤归
  Gateway 的 `read_group_history`）。
- `FakeModel` 支持只回文本、不带任何 tool_call 的出牌，用来验 §3.3 那条兜底
  （`steps==0` 视为 `final`，`steps>0` 回 system 提示）——**兜底逻辑本身归 worker**。
- `FakeSandbox` 的时钟可注入，`reap_idle` 因此不用真等 5 分钟。

> 并行开发期间 T1/T2/T3 **不要** import 这个包（§3.4 测试替身规则），各自在
> `tests/<自己的目录>/` 下写私有替身。这一份是给 `tests/e2e/`、评测 runner 和 TΩ 用的。

## 评测（`aite/evals/`）

```bash
python -m aite.evals run evals/p0 --list                          # 列出场景名
python -m aite.evals run evals/p0 --platform fake --model scripted # 跑，最后一行 passed k/10
python -m aite.evals run evals/p0 --only 04_csv_to_chart --traceback
```

跑完打印 JSON 摘要，最后一行是 `passed k/10`；全过退出 0，否则 1。
**每个没过的场景都会给一句人话原因和阶段标记**，不吐异常栈：

| phase | 意思 |
|---|---|
| `wiring` | 接不上被测系统（某条轨还没合入就停在这里） |
| `dispatch` | `handle_event` 抛了 |
| `drive` | `run_forever` 起不来 / 中途炸了 / 超时没静下来 |
| `assert` | 跑到了，但断言没过（`failures` 里逐条写明期望与实际） |
| `ok` | 全过 |

### 场景文件长什么样

一个场景 = 喂什么（`events` / `platform` / `sandbox`）+ 模型怎么出牌（`model_script`）
+ 该看到什么（`expect`）。字段默认值给得很足，只写关心的那几个：

```yaml
name: 01_simple_qa
title: 一问一答不发卡片
verifies: 第一步就 final → 只有 1 条 send_text，没有卡片
spec_ref: "§3.8 01_simple_qa；§3.6 W3"

events:
  - {event_id: e1, text: 今天北京天气怎么样, mentioned: true}

model_script:
  - tool_calls:
      - {name: final, arguments: {reply: 北京今天晴。}}

expect:
  - {check: platform_calls, method: send_text, equals: 1}
  - {check: platform_calls, method: send_card, equals: 0}
  - {check: task, which: last, status: delivered}
```

`expect` 的 check 类型见 [`aite/evals/checks.py`](aite/evals/checks.py) 的 `REGISTRY`：
`platform_calls` / `outbound_total` / `cards` / `text` / `distinct_matches` / `file` /
`gateway_calls` / `gateway_result` / `model_tools` / `model_calls` / `sandbox_calls` /
`store` / `task` / `evidence`。比较子统一是 `equals` / `min` / `max`。

### runner 怎么接到被测系统上

`aite/evals/wiring.py` 不写死任何类名，而是运行时发现，按优先级：

1. `aite.control` 暴露 `build_control_plane(**deps)` 工厂（最省事，参数名随便起）
2. 否则找 `aite.control.plane` 里同时带 `handle_event` 和 `run_forever` 的类，
   按参数名喂 `config` / `platform` / `model` / `sandbox` / `gateway` / `store` /
   `evidence`（别名表在 `PARAM_ALIASES`）
3. 跑起来后优先调 plane 的 `drain()` / `run_until_idle()`；没有就让 `run_forever()`
   后台跑、轮询到所有替身都不再被调用为止再取消

> §3.2 里没有「跑到队列空为止」这个接口，第 3 条是 runner 自己的兜底。TΩ 接线时
> 给 plane 加一个 `drain()` 会让评测更确定，也更快。

平台与模型按 `--platform fake --model scripted` 用替身；沙箱、Gateway、
SessionStore、EvidenceWriter 在 runner 里也用替身，这样评测不依赖 Docker、
结果可复现。真沙箱由 `pytest tests/sandbox -q -m docker` 单独验（§2.2 B4）。

`--model live` 会用 `config/aite.yaml` 里的模型真跑一遍（模型实测用），
这时 `model_script` 不生效、`expect` 也不该当验收看。

## CI

`.github/workflows/ci.yml`，一步一条判据：A1 → A2 → **A3 `lock --check`** →
**A4 `ruff check .`** → **A5 `pytest -q --co`** → C1 契约测试 → 替身自测 →
全量测试 → 场景清单 → B8 评测 → `docker compose config`。

B8 那步现在挂着 `continue-on-error: true`：并行期间 T1/T2/T3 还没合进来，
必然是 `passed 0/10`。TΩ 合流后删掉那一行，它就变成硬门禁。
