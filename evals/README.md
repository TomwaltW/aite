# evals —— P0 评测场景与模型实测

两种跑法，看的是**两件不同的事**。别把它们的结论混着读。

## 1. scripted：P0 验收面（B8）

```bash
python -m aite.evals run evals/p0 --platform fake --model scripted
```

模型是 `aite.testing.FakeModel`，按每个场景 yaml 里的 `model_script` 演。
`expect:` 里的判据钉的就是这条路径 —— **期望 `passed 10/10`，退出码 0**。
`scripts/check.sh` 的 B8 读的是最后一行。

这一条变红 = 回归，要查。

## 2. live：真模型实测（§14.2）

```bash
cp config/aite.example.yaml config/aite.yaml     # 填 model.base_url / model.model
export AITE_MODEL_API_KEY=…                      # 密钥只放环境变量，不进 yaml（§3.1）

python -m aite.evals run evals/p0 --platform fake --model live \
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

它按 `aite/contracts/protocol.py` 逐步核对模型的出牌，回答五个问题：

| 问题 | 报告里的字段 |
|---|---|
| 每步调了什么 | `runs[].steps_detail[].tool_calls` |
| 参数合不合 `ToolSpec.parameters` | `schema_violations` |
| 调了协议外的名字吗 | `unknown_tools`（外加 `fallbacks.not_found`） |
| 几步收敛 | `runs[].steps_to_final`、`hit_max_steps` |
| §3.3 的兜底触发了没 | `fallbacks.*`（逐条见下） |

`fallbacks` 逐条对应 §3.3：

* `text_only_as_final` —— 模型既无 tool_call 也无 `final` 只回文本，且 `steps==0`：视为 `final`
* `text_only_nudge` —— 同上但 `steps>0`：回一条 system 提示并计 1 步
* `invalid_args` —— 参数不合法：回 `invalid_args`，同一任务连续 3 次 → `failed`
* `model_retry` —— 模型异常 / 5xx：重试 2 次，`delays_ms` 是实测的退避（应在 2000 / 5000 附近）
* `not_found` / `sandbox_errors` —— 工具名查无此人 / 沙箱连续 2 次不可用

摘要打 stderr（人话），完整 JSON 写 `--protocol-report` 给的路径。
**stdout 那份 JSON 摘要不开这个开关时一个字段都不多** —— CI 读的还是原来那份。

### 超时

场景 yaml 里的 `timeout_sec: 10` / `after_timeout_sec: 5` 是照替身的尺度定的
（脚本化模型瞬时返回）。真模型一次调用就 2–20s，所以 `--model live` 默认把两个
上限一起乘 `LIVE_TIMEOUT_SCALE`（见 `aite/evals/__main__.py`），会在 stderr 说一句。
自己调用 `--timeout-scale K` 覆盖。**场景文件一个字都不改。**

## 场景文件

10 个场景的形状见 `aite/evals/scenario.py`，断言 DSL 见 `aite/evals/checks.py`。
`--list` 只列名字不执行。
