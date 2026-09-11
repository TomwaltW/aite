# evals —— P0 评测场景与模型实测

两种跑法，看的是**两件不同的事**。别把它们的结论混着读。

## 1. scripted：P0 验收面（B8）

```bash
core/target/debug/aite evals run evals/p0 --platform fake --model scripted
```

模型是 `aite-testing` 的 `FakeModel`，按每个场景 yaml 里的 `model_script` 演。
`expect:` 里的判据钉的就是这条路径 —— **期望 `passed 10/10`，退出码 0**。
`scripts/check.sh` 的 B8 读的是最后一行。

这一条变红 = 回归，要查。

### 并行期间它是 `passed 0/10`

真 `ControlPlane`（R4 + R5）在 RΩ 组装时才接上。在那之前每个场景以
`phase="wiring"` 收场，原因是一行人话「ControlPlane 未接线（RΩ）」，退出码 1 ——
不是 panic，也不是异常栈。RΩ 把 `PlaneFactory` 注进来之后才要求满分。

接线点在 `aite_evals::cli::run_with_wiring(args, &Wiring{plane, model, sandbox})`：

| 字段 | 类型 | 谁提供 |
|---|---|---|
| `plane` | `PlaneFactory = Arc<dyn Fn(&Deps) -> Result<Arc<dyn ControlPlane>, String>>` | RΩ（R4 的 `InProcessControlPlane` + R5 的 `AgentWorker`） |
| `model` | `ModelFactory = Arc<dyn Fn() -> Result<Arc<dyn ModelPort>, String>>` | RΩ（R5 的 `aite-models`），`--model live` 用 |
| `sandbox` | `SandboxFactory`（造 `SandboxFacade` + `GatewayFacade`） | RΩ（R2 的真沙箱 + R6 的 `P0ToolGateway`），`--sandbox docker` 用 |

`cli::run(args)` = 三个都不注入；`cli::run_with_factory(args, plane)` 只注入 plane。

## 2. live：真模型实测

```bash
cp config/aite.example.yaml config/aite.yaml     # 填 model.base_url / model.model
export AITE_MODEL_API_KEY=…                      # 密钥只放环境变量，不进 yaml

core/target/debug/aite evals run evals/p0 --platform fake --model live \
    --only 01_simple_qa \
    --protocol-report live-protocol.json
```

先单场景过一遍再跑全套，省 token。

### 这里的 `passed k/10` 不是判据

场景的 `expect` 是照 `model_script` 里的台词写的，比如：

```yaml
- {check: text, where: last, contains: 北京今天晴}
```

真模型不可能复现这句话，所以 **live 模式下判据必然大面积红，这是预期的，不是 bug**。
谁也别为了凑绿去改场景判据 —— 那些判据是 scripted 路径的 P0 验收面（B8），
改了就是把 P0 的验收面拆了。

### 要看的是 `--protocol-report`

它按 `aite-contracts::protocol` 逐步核对模型的出牌，回答五个问题：

| 问题 | 报告里的字段 |
|---|---|
| 每步调了什么 | `runs[].steps_detail[].tool_calls` |
| 参数合不合 `ToolSpec.parameters` | `schema_violations` |
| 调了协议外的名字吗 | `unknown_tools`（外加 `fallbacks.not_found`） |
| 几步收敛 | `runs[].steps_to_final`、`hit_max_steps` |
| 兜底触发了没 | `fallbacks.*`（逐条见下） |

`fallbacks` 逐条对应 worker 的失败面：

* `text_only_as_final` —— 模型既无 tool_call 也无 `final` 只回文本，且 `steps==0`：视为 `final`
* `text_only_nudge` —— 同上但 `steps>0`：回一条 system 提示并计 1 步
* `invalid_args` —— 参数不合法：回 `invalid_args`，同一任务连续 3 次 → `failed`
* `model_retry` —— 模型异常 / 5xx：重试 2 次，`delays_ms` 是实测的退避（应在 2000 / 5000 附近）
* `not_found` / `sandbox_errors` —— 工具名查无此人 / 沙箱连续 2 次不可用

另有一格不在 `fallbacks` 里：`repeat_loops` —— 同一张牌**连着出 3 次以上**。
没有哪条兜底接得住原地打转（工具返回的是 `ok=true`，只是内容为空），只有 `max_steps`。
它是「缺兜底」的证据，不是「兜底触发了」的记录，所以单列。

摘要打 stderr（人话），完整 JSON 写 `--protocol-report` 给的路径。
**stdout 那份 JSON 摘要不开这个开关时一个字段都不多** —— CI 读的还是原来那份。

### 超时

场景 yaml 里的 `timeout_sec: 10` / `after_timeout_sec: 5` 是照替身的尺度定的
（脚本化模型瞬时返回）。真模型一次调用就 2–20s，所以 `--model live` 默认把两个
上限一起乘 `LIVE_TIMEOUT_SCALE`（见 `core/crates/evals/src/cli.rs`），会在 stderr 说一句。
自己调用 `--timeout-scale K` 覆盖。**场景文件一个字都不改。**

## 3. `--sandbox docker`：真沙箱那一档

```bash
core/target/debug/aite evals run evals/p0 --only 04_csv_to_chart \
    --platform fake --model live --sandbox docker
```

代码在 `aite-sandbox:p0` 容器里真跑，PNG 是镜像里的 matplotlib 真画出来的。
默认仍是 `fake`，**默认路径逐字节不变**（B8 / CI 都吃那条路）。
这一档下场景的 `sandbox.exec_script` 一律忽略（代码交给真容器跑），起飞时在 stderr 点名。

RΩ 接上 `SandboxFactory` 之前，这一档是一行人话 + 退出码 2。

## 4. 演示素材

```bash
core/target/debug/aite evals demo-fixture all -d /tmp/aite-demo
core/target/debug/aite evals demo-fixture csv -o ~/demo.csv --months 12
core/target/debug/aite evals demo-fixture history --count 6
```

同样参数跑两次字节一致（抖动走 CPython `random.Random` 那一套的梅森旋转，
所以与旧 `scripts/demo_fixture.py` 的产物 `cmp` 得上）。默认落 `/tmp/aite-demo/`，
不往 `data/` 写 —— `data/` 是运行时目录，演示前经常要清空。

## 场景文件

10 个场景的形状见 `core/crates/evals/src/scenario.rs`，断言 DSL 见
`core/crates/evals/src/checks.rs`。`--list` 只列名字不执行。
**`evals/p0/*.yaml` 是验收面，谁都不动。**
