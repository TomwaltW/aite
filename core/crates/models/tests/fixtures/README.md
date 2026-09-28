# aite-models 的厂商响应夹具（CC6，2026-09-28）

**这些不是云端真录的 —— 云端没有任何模型 key（总计划 D19：key 只在本机）。**
每份都是照「官方文档里的响应形状 + 09-25 总管本机实测的三条事实」手写的最小样本，
只保留本 crate 读得到的字段；数值（token 数、id、时间戳）是编的。

| 文件 | 厂商 | 形状来源 | 用在哪条测试 | 钉的点 |
|---|---|---|---|---|
| `deepseek_thinking_tool_call.json` | DeepSeek | 总计划附录 A「国产模型接入要点」DeepSeek 一节（思考模式 `message.reasoning_content`、`usage.prompt_cache_hit_tokens`）+ 09-25 实测事实 2（`deepseek-chat` 被静默路由到 `deepseek-flash`，响应 `model` 字段是后者）与事实 3（第 2 轮不回传也 200） | `deepseek_thinking_tool_roundtrip_reattaches_reasoning`、`reasoning_captured_in_raw`、`cache_hit_field_variants` | `arguments` 故意写成**带空格**的 `"{\"q\": 1}"`：回挂指纹若误用原始串就对不上 |
| `qwen_tool_call.json` | 通义千问（百炼） | 附录 A Qwen 一节（`parallel_tool_calls` 默认关、`usage.prompt_tokens_details.cached_tokens`） | `qwen_sets_parallel_tool_calls` | `arguments` 故意是**对象**而不是字符串（有的网关这么回，lib.rs `parse_arguments` 照收） |
| `glm_tool_call.json` | 智谱 GLM | 附录 A GLM 一节（`tool_choice` 只支持 `auto`） | `glm_tool_choice_auto_only` | — |
| `kimi_tool_call.json` | Kimi（月之暗面） | 附录 A Kimi 一节（传 `temperature` 会 400；`usage.cached_tokens` 在顶层；tool_call id 形如 `search:0`，按序号编） | `kimi_request_omits_temperature`、`cache_hit_field_variants` | — |
| `doubao_encrypted.json` | 豆包（火山方舟） | 附录 A 豆包一节（`message.encrypted_content`） | `reasoning_captured_in_raw` | `encrypted_content` 原样进 `raw["provider_extra"]` |
| `minimax_think.json` | MiniMax | 附录 A MiniMax 一节（`<think>…</think>` 混在 `content` 开头） | `minimax_think_stripped` | 剥下来的正文进 `provider_extra["reasoning_content"]`，回帖里不许出现 `<think>` |
| `deepseek_models.json` | DeepSeek | 09-25 实测事实 1：`GET /models` 只剩 `deepseek-flash` 与 `deepseek-v4-pro` 两个 id | 无（本 crate 没有 `/models` 调用，只以数据形式存档） | — |

## 09-25 DeepSeek 实测（总管本机真 key；云端未复测）

1. `GET /models` 只剩 `deepseek-flash` 与 `deepseek-v4-pro` 两个 id。
2. 配置里的 `deepseek-chat` 仍被接受，静默路由到 `deepseek-flash`（非思考模式），带工具能用。
3. `deepseek-flash` 思考模式返回了 `reasoning_content`；第二轮工具调用**不回传**它也返回 200 —— 调研里「会 400」没复现。

真端点的回归走 `tests/cc6_live.rs`（`#[ignore]`，`AITE_LIVE_MODEL=1` 才发请求，只在总管本机跑）。
