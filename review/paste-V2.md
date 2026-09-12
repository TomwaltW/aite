# 任务 V2 — preflight 交付面补齐（接管 Python 25 条 + 网络故障可辨认 + 脱敏两个口子）

## 背景：这轨是从哪来的

Aite 的 P0 只剩两件事没兑现：`docs/dev-spec-2026-09-09.md` §2.4 的 **M1–M6 真机验收**（总管
亲自在真实飞书测试群跑），和「**能录一段 3 分钟演示**」。这一批六轨的定位就是把 P0 收尾到
「总管可以坐下来跑 M1–M6、可以开录」。§1 明令禁止「任何『顺手做一点 P1』的行为」—— 这一轨
一个字都不许越界。

M1–M6 的**第一步**是 `aite preflight`。`docs/acceptance-M.md` §0.1 就叫「起飞前 60 秒：外部
依赖自检」，正文写着「**任一 FAIL 就别往下走** —— M1–M6 里八成的『没反应』都是这七项里的某一项
没配好，在这里花 60 秒比在群里瞎试便宜得多」。也就是说：**preflight 是总管排障时唯一的入口，
它说错一句话，总管就要在飞书群里瞎试半小时。**

而这个交付面眼下的成色对不起这个位置：

- `core/crates/app/src/preflight.rs` **1831 行**（产品代码 1603 行 + `mod tests` 228 行），
  全仓覆盖它的只有**文件内 8 条单测**（3 条 `Redactor` 纯函数 + 1 条渲染面 + 2 条 `add()` 接线
  + 2 条第 6 组收尾判据）**加 `tests/cli_smoke.rs` 里 1 条 `--offline` 冒烟**。
  Python 那边被删掉的 `tests/scripts/test_preflight.py` 有 **564 行、25 条**测试，
  **一条都没有人接管**。3/4/5/6 四组检查（`preflight.rs:447–1193`，约 750 行）现在
  **一条测试都没有**。
- 更要命的是它最常见的三类真机故障 —— 公司代理不通 / DNS 污染 / TLS 被中间设备拦 ——
  **打出来一模一样**。这不是推测，是这台机器上刚跑出来的（见 §6.2）。
- 上一轮合并前刚补的三层脱敏测试堵住了「红线断言恒真」那条，但审核修复 agent 顺带发现
  `Redactor` 还有**两个口子**（台账 §4.2 最后一行），一个是「配置读不出来时兜底闸压根没上」，
  一个是「契约加第 5 个 `*_env` 的那天脱敏静默漏」。

所以这一轨要做的是：**把 preflight 从「能跑」补到「跑出来的每句话都能信」**，
并把 Python 那 25 条的语义接管过来，让它此后不会悄悄退化。

上一轮的全部结论在 `review/review-findings-2026-09-12-romega.md`，这一轨的活出自它的第四节。
**本派单里点名的每一处 `file:line` 都在 `11322b3` 上逐个打开核对过**，与台账不一致的已改成实测值
（对照表见 §6 各条）。

## 必读（按这个顺序）

| # | 材料 | 看哪一节 / 为什么 |
|---|---|---|
| 1 | `review/review-findings-2026-09-12-romega.md` | **§4.1** 的 `preflight.rs:576` / `:946` / `:1314` 三条 medium；**§4.2 最后一行**（Redactor 两个口子）；**§4.4** low 表里 preflight 那三条（`:212` / `:1252` / `:1218`）；**§2.5** 里 `preflight.rs + tests/cli_smoke.rs` 那一行 —— 它写清了上一轮补的三层脱敏测试是怎么补的，你**只能加强不能削弱**它 |
| 2 | `core/crates/app/src/preflight.rs` 全文 | 模块头（1–25 行）那张七组表是本轨的地图；`mod tests` 开头 1591–1603 行那段注释写明了「哪两件事别处钉不住、为什么钉在这儿」，你新加的测试要延续这个判断 |
| 3 | Python 原版三份（已删，取法见 §6.1） | `tests/scripts/test_preflight.py`（564 行 / 25 条，**你要接管的语义**）、`tests/scripts/conftest.py`（336 行，替身是怎么搭的）、`scripts/preflight.py`（1211 行，被测源码 —— 对拍行为时看它） |
| 4 | `docs/acceptance-M.md` §0（尤其 §0.1、§0.2） | 这一轨的**用户**是总管、场景是 M1–M6 排障。改任何一句输出文案前，先看它在这份文档里是怎么被引用的（§0.1 点名了「第 ④ 组尤其要过」，正文表格里还有三处「跑 preflight 第 ③/④/⑤/⑦ 组」的排障指引） |
| 5 | `review/paste-ROMEGA.md` §②「`aite preflight`：七组自检」+ §纪律 | 这个交付面的原始要求，尤其「红线：任何输出不得出现密钥取值」和「一项失败不阻断后面的检查」两条 |

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-v2      （已建好）
分支     : task-v2
基线     : 0bc8d55（main 的 HEAD，RΩ 组装起飞刚合入）
工具链   : cargo 1.98.1（/opt/homebrew/opt/rustup/bin）、go 1.27.1、protoc 36.1、Docker 29.6.1
```

PATH 必须含 `/opt/homebrew/opt/rustup/bin` 与 `~/go/bin`（已写进 `~/.bash_profile`）。


> **基线说明**：`0bc8d55` = RΩ 合入 main 那次（`11322b3`）**再往前一格**。
> 那一格只改了两个文件：`scripts/check.sh`（B 全量 cargo test 那步改成「失败测试名在前、计数在后」）
> 与 `review/review-findings-2026-09-12-romega.md`（收窄 Answering 那条 + 补记一次未复现的 717/1）。
> `git diff --stat 11322b3..0bc8d55` → `2 files changed, 11 insertions(+), 2 deletions(-)`。
> 本派单正文里凡是写「在 `11322b3` 上核过 / 实测」的，指的是核对当时那一格，**代码面与你的基线逐字相同**。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告）

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v2
git log --oneline -1                                   # 期望 0bc8d55（记下它，回执里当基线）
git status --short                                     # 期望空
scripts/check.sh                                       # 期望最后一行「全部通过」，退出码 0（首次全量编译 5–10 分钟）
```

`scripts/check.sh` 的关键行期望（**这就是起跑线，任何一条变小都是回归**）：

| 行 | 期望 |
|---|---|
| A3/C2 契约锁 | `OK 25 files` |
| C1 契约测试 | `contracts passed=25 failed=0` |
| B 全量 cargo test | `cargo passed=718 failed=0` |
| B 全量 go test（-race） | 八个包全 `ok`（`-race` 是硬门禁，别去掉） |
| B8 评测 | `passed 10/10` —— **RΩ 起它是硬门禁**，check.sh 会计分 |

另外单独跑一次（不在 check.sh 里）：

```bash
cd edge && go test -tags docker ./internal/sandbox/... -count=1   # 期望 ok
docker ps -a --filter label=aite.task -q | wc -l                  # 期望 0
```

> **已知的一条假红**：docker 档沙箱测试在多轨并行时会假红（六个 worktree 同时跑真容器会互相挤）。
> 它红了先看两件事：是不是**只有这一个包**红、**单独跑**是不是绿。是的话就是并行挤的，不是回归。

> **另一条未复现的现象**（台账第五节末尾）：合并后在 main 上复验时，第一次全量
> `cargo test` 出过一次 `717 passed / 1 failed`，紧接着连跑六遍都是 `718 / 0`，当时机器上
> 正有六个 agent 在抢 CPU，**失败的测试名没留下** —— 因为当时的 `check.sh` 把 cargo 输出
> grep 成只剩计数行。基线 `0bc8d55` 已经修掉这条（失败名现在会打在计数行前面）。
> **万一你撞上 `failed=1`：先别当回归，把 `error: test failed, to rerun pass ...` 那行原样贴进回执**
> （能不能复现都要贴，这个现场丢过一次了）。

还有一条，**必须真跑，不能跳**：

> **Read 一下 `.claude/hooks/guard_bash.py` 本身，必须被守卫拦下来。** 没被拦说明
> `CLAUDE_PROJECT_DIR` 不对、所有守卫都在静默失效，**停下报告**。
> （2026-09-12 总管这边真踩过：会话在仓库子目录里起，`CLAUDE_PROJECT_DIR` 就定死在那个子目录，
> hook 命令 `python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard_bash.py"` 找不到文件 →
> 执行失败 → **非阻塞放行、不报警** → 整个会话守卫静默失效。）

## 可写路径

| 面 | 规则 |
|---|---|
| `core/crates/app/src/preflight.rs` | **自由改**（本轨主战场） |
| `core/crates/app/tests/preflight_*.rs` | **自由新建**（本轨唯一的新文件面） |
| `core/crates/models/src/lib.rs` | **可写（总管 2026-09-12 已放行）**，但只许干一件事：把 `source()` 链拼进消息。**不许绕过 `redact()`**，也不许改 `ModelError` 的形状（它在 contracts、冻结）。改完必须补一条回归测试，照台账 #13 那两条的做法证明它不是摆设：**撤掉 `redact` 实测变红**。回执里单独说明 |
| `proto/**`、`core/crates/contracts/**`、`.contracts.lock` | **冻结**，全程 `OK 25 files`。`ModelError` 就住在 `core/crates/contracts/src/errors.rs:71–79`，想给它加 `#[source]` = 动契约 → **停下报告** |
| `core/crates/app/tests/cli_smoke.rs` | **归 V6，一个字不许动** |
| `core/crates/app/src/app.rs`、`src/run.rs` | **归 V5，不许动**（`load_config` 在 `app.rs:99`，你只读不改） |
| `core/crates/app/src/cli.rs`、`src/wiring.rs` | **归 V6，不许动** |
| `evals/p0/*.yaml` | 十个场景是验收面，**一个字不动** |
| `docs/dev-spec-2026-09-09.md`、`docs/dev-spec-2026-09-11-rustgo.md` | **冻结** |
| `core/Cargo.toml` 的依赖表 | **冻结**。加新依赖（wiremock / httpmock / mockito 之流）→ **停下报告**。你手上已有的够用：`tokio`（full，含 `net::TcpListener`）、`reqwest`、`serde_json`、`async-trait`、`aite-testing`（`FakeSandbox` 在 `core/crates/testing/src/fake_sandbox.rs`）、dev-dep `tempfile` |

`docs/acceptance-M.md` 不在冻结面上：**如果你改了 preflight 的输出文案**，把这份文档里引用到它的
地方同步过来（§0.1 那七组的说法、正文表格里四处「跑 preflight 第 ③/④/⑤/⑦ 组」）。

## 要做什么

六条。**每条都给了「病在哪 → 怎么验证它确实病着 → 边界 / 可选路 → 要你在回执里交代什么」。**
凡是没给死改法的，判断留给你，但回执里必须说清你选了哪条、为什么。

---

### 6.1 接管 Python 25 条 preflight 测试的语义（主戏）

**台账**：§4.1 medium，锚在 `preflight.rs:946`。**核对结果**：946 行是 `check_sandbox` 上方
doc 注释的中间一行，不是有效锚点 —— 这一条的对象是**整个文件**，不是某一行。

**病在哪**：`preflight.rs` 产品代码 1603 行，文件内单测 8 条（`preflight.rs:1618 / 1638 / 1657 /
1676 / 1727 / 1748 / 1797 / 1823`），外加 `tests/cli_smoke.rs` 一条 `--offline` 冒烟。
第 3/4/5/6 四组（`preflight.rs:447–1193`，约 750 行）**零覆盖**。

**先把原版取出来**（Python 树已随 RΩ 删除，别自己摸，照抄这四行）：

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v2
git log --oneline --diff-filter=D -- tests/scripts/test_preflight.py    # → 598f476（删除那个 commit）
git show 598f476^:tests/scripts/test_preflight.py > /tmp/py-test_preflight.py   # 564 行 / 25 条：你要接管的语义
git show 598f476^:tests/scripts/conftest.py       > /tmp/py-conftest.py         # 336 行：替身是怎么搭的
git show 598f476^:scripts/preflight.py            > /tmp/py-preflight.py        # 1211 行：被测源码，对拍行为看它
```

（`598f476` 是 RΩ 那个 feature commit，`11322b3` 的第二父。四条命令实测都出得来。
**不要把这三份写回仓库**，`/tmp` 或 scratchpad 就好。）

**25 条逐条落点**（行号已在 `11322b3` 上核过）：

| # | Python 用例 | Rust 侧落在哪 | 现状 |
|---|---|---|---|
| 1 | `test_all_green_exits_zero` | `run_checks` 全绿（要 HTTP 桩 + 沙箱桩） | 无 |
| 2 | `test_missing_env_var_fails_and_names_it` | `check_env`（非 offline → FAIL 且点名，`preflight.rs:413–423`） | 无 |
| 3 | `test_missing_feishu_creds_block_checks_three_and_four` | `check_feishu` 的 `blocked()` 早退（`:483–489`；`blocked` 本体 `:157`） | 无 |
| 4 | `test_missing_image_names_the_build_command` | **语义已变**：Python 单独查镜像存在，Rust 合并进「真起一个容器」，对应 `acquire` 失败那支的 fix（`:1022–1035`，fix 里有 `docker build -t <image> docker/sandbox`） | 无；回执里说清你怎么接管这条 |
| 5 | `test_bot_open_id_mismatch_is_fail_without_printing_values` | `check_feishu_identity` 的不匹配支（`:720–740`） | 无 —— **这条尤其要接**：它钉的是「最想打出来的那两个取值，一个都不许打」，代码里那行注释也这么写 |
| 6 | `test_offline_touches_neither_network_nor_docker` | `run_checks` 的 offline 分支（`:1344–1352`）：一个包不发、`EdgeClient` 不建 | 无 |
| 7 | `test_offline_with_no_credentials_still_exits_zero` | `check_env` 的 offline 降级 WARN（`:399–412`） | 无 |
| 8 | `test_secret_values_never_reach_output` | 上一轮补的三层（`:1676` 渲染面 / `:1727` 第 5 组 / `:1748` 第 3-4 组） | ✅ 部分；缺 Python 那条的完整形态：**让上游把凭证回显进错误消息**，再端到端过一遍 `run_checks` + 渲染 |
| 9 | `test_redactor_scrubs_nested_extra` | `:1638` | ✅ |
| 10 | `test_redactor_leaves_short_values_alone` | `:1657` | ✅ ——但字节/字符那个口子就在这儿，见 §6.5 |
| 11 | `test_sandbox_releases_container_on_success` | `:1823`（判据纯函数层） | ✅ 判据层；缺「真调了 `release`」这一层 |
| 12 | `test_sandbox_releases_container_when_probe_blows_up` | **第 6 组最值钱的一条**：`exec`（`:1042`）与 `release`（`:1044`）的**先后顺序** —— 探针炸了容器照样收 | 无 |
| 13 | `test_sandbox_fails_when_imports_missing` | `exit_code != 0` / 缺 `AITE_PREFLIGHT_OK` 那支（`:1071–1093`） | 无 |
| 14 | `test_docker_daemon_down_is_one_fail_row_not_a_crash` | `status.sandbox_ok == false` 那支（`:1005–1013`） | 无 |
| 15 | `test_every_check_still_runs_when_feishu_is_unreachable` | `run_checks` 第 3 组红了不短路，5/6/7 照跑 | 无 |
| 16 | `test_unexpected_crash_becomes_one_fail_row` | **Rust 里没有对等物**：Python 用 `try/except` 把每组包起来，Rust 这边对应的是「不 panic」+ 类型化 `Result`。要你拍板，见下面 |
| 17 | `test_bad_config_fails_but_still_reports_seven_rows` | `run_checks` 的 `cfg is None` 早退（`:1322–1340`）—— **正是 §6.3 口子①所在** | 无 |
| 18 | `test_storage_fails_when_ancestor_not_writable` | `check_storage` 的 FAIL 支（`:1248`）—— **正是 §6.6「卡在 空」所在** | 无 |
| 19 | `test_storage_ok_when_parent_missing_but_creatable` | `check_storage` 的「待建」支（`:1269–1279`） | 无 |
| 20 | `test_json_output_shape` | `render_json`（`:1478`） | 无 |
| 21 | `test_json_carries_model_and_sandbox_evidence` | `extra` 里的 `latency_ms` / `packages` / `released` / `sandbox_gone` | 无 |
| 22 | `test_chat_id_probes_group_history` | `probe_history_scope` 成功支（`:788–800`，note `status="verified"` + 「**读得到**」） | 无 |
| 23 | `test_chat_id_probe_reports_permission_failure` | `probe_history_scope` 的 `code != 0` 支（`:772–786`，「读**不到**」） | 无 |
| 24 | `test_env_var_names_covers_every_env_field` | `env_var_names`（`:285`） | 无 —— **§6.4 口子②正好靠它**，两件事一起做 |
| 25 | `test_help_exits_zero` | `run(vec!["--help"])` → 0 + `USAGE`（`:1506` / `:1552`） | 无 |

**测试放哪儿**（`lib.rs` 已经 `pub mod preflight;`，两层都通）：

- `preflight.rs` 的 `mod tests`：私有函数层 —— `check_config` / `check_env` / `check_feishu*` /
  `check_model` / `check_storage` / `sandbox_release_verdict` / `Redactor` 内部。
- `core/crates/app/tests/preflight_*.rs`：`aite_app::preflight::{Options, run_checks, render_text,
  render_json, Redactor, env_var_names}` 这一层 —— 七行齐不齐、`--offline`、`--json` 形状、
  脱敏端到端。私有 `check_*` 在这儿看不见，要么留在文件内测，要么在 `preflight.rs` 里把它们
  提到 `pub`（这个文件是你的，随你）。

**怎么不连网也不起容器**（Python 靠 respx + 注入假 docker/model client，Rust 这边没有这些）：

- **第 3/4 组**：`check_feishu(cfg, env, redactor, chat_id, domain)` 的 `domain` 就是注入口，
  `Options.feishu_domain`（`:1303`）一路传下来。用 `tokio::net::TcpListener` 起一个几十行的
  HTTP 桩，按路径（`/open-apis/auth/v3/tenant_access_token/internal`、`/open-apis/bot/v3/info`、
  `/open-apis/im/v1/messages`）回死数据，`domain` 传 `http://127.0.0.1:<port>`。
- **第 5 组**：`check_model` 走 `cfg.model.base_url`，同一个桩加一条 `/chat/completions` 即可。
- **代理这一坑先看清楚**：本机实测 `HTTP_PROXY=http://127.0.0.1:7897`、
  `HTTPS_PROXY=http://127.0.0.1:7897`、`NO_PROXY=localhost,127.0.0.1,::1`。所以桩**必须绑
  `127.0.0.1`**（在 `NO_PROXY` 里，reqwest 直连）；绑主机名或 `0.0.0.0` 会被代理截胡，
  测试就变成看运气。跑之前先 `env | grep -i proxy` 确认一遍这台机器上是什么状况，把结果写进回执。
  （`std::env::set_var` 在 Rust 2024 是 `unsafe`，多线程测试进程里别用它去清代理变量。）
- **第 6 组**：`check_sandbox(cfg, repo_root)`（`:950`）目前**没有注入口** —— 它自己
  `EdgeClient::connect`。要覆盖 12/13/14 三条，得把「拿到 `Arc<dyn SandboxPort>` 之后的那段」
  抽成一个吃 `&dyn SandboxPort` 的函数（`SandboxPort` 在 `core/crates/contracts/src/ports.rs:90`，
  `EdgeClient::sandbox()` 返回的就是它）。抽完用测试内自写的假 `SandboxPort`（`async-trait`
  已在依赖表里）就能造「exec 炸了」「import 缺了」「release 失败」三种现场。
  `aite-testing::FakeSandbox` 也能用，但它**没有「让 release 失败」的开关**，而
  `core/crates/testing/**` **不在你的可写面上** —— 别去改它。
- **第 14 条（daemon down）** 落在 `status.sandbox_ok == false`，在 `EdgeClient::connect` 成功
  之后 —— 抽函数时把这一步也考虑进去，或者单独抽一个吃 `EdgeStatus` 的判据函数。

**第 16 条要你拍板**：Python 那条钉的是「任何一组自己炸掉也只红一行，不许把整轮掀翻」。
Rust 里 `run_checks` 的每一组都返回 `CheckResult`，没有 `catch_unwind`。三条路：
① 有依据地销账（写清「Rust 侧对应的保证是纪律 4『不 panic』+ 类型化 Result，Python 那条
兜底在 Rust 里没有对象」）；② 加一层 `catch_unwind` —— 但 async 里做这个很难做干净，而且
`paste-ROMEGA.md` 已经说过「catch_unwind 是安全网不是许可证」；③ 换个接管方式：钉「任何一组
FAIL 都不影响后面几组出结论」（第 15 条已经覆盖了这个语义的一半）。
**总管的倾向是 ① 或 ③，不要 ②。** 但你自己判断，回执里说清选了哪条。

**覆盖到什么程度算够**：不要求 25 条一比一。要求是 **3/4/5/6 四组的每一条判据分支都至少有
一条测试**，且每条新测试你都能回答「把被测行为破坏掉，它会不会红」。做不到的逐条在回执里说明。

---

### 6.2 网络类 FAIL 丢掉 `source()` 链 —— 三种病打出来一模一样

**台账**：§4.1 medium，`preflight.rs:576`。**核对结果：576 行准确**，就是
`check_feishu_token` 里 `send()` 失败那一支：

```rust
format!("换 tenant_access_token 失败：{}", tail(&e.to_string(), 300)),
```

**病在哪**：`reqwest::Error` 的 `Display` 只写 kind + url，真正的原因在 `source()` 链上
（连接被拒 / DNS 解析失败 / TLS 证书被中间设备换掉 / 代理不通），一个字到不了输出。
Python 那版打的是 `f"{type(exc).__name__}: {_tail(str(exc))}"`，httpx 的异常 `str` 自带 errno。

**怎么验证它确实病着**（2026-09-12 在 `11322b3` 上实跑，两种**完全不同**的病，输出**逐字相同**）：

```bash
# 病 A：代理端口关着（连接被拒）
FEISHU_APP_ID=cli_fake FEISHU_APP_SECRET=fakesecret1234 \
  HTTPS_PROXY=http://127.0.0.1:9 core/target/debug/aite preflight --config <一份填好的 config>

# 病 B：代理主机名解析不了（DNS）
FEISHU_APP_ID=cli_fake FEISHU_APP_SECRET=fakesecret1234 \
  HTTPS_PROXY=http://no-such-proxy-host.invalid:8080 core/target/debug/aite preflight --config <同一份>
```

两次都是：

```
[3/7] FAIL 飞书凭证有效     换 tenant_access_token 失败：error sending request for url (https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal)
           └ 怎么补：先确认本机能出网访问 open.feishu.cn，再核对两个凭证环境变量
```

**一个字都不差。** 总管拿到这一行，不知道是该去查代理、查 DNS、还是查公司的 TLS 拦截。

**同病的还有三处**（都是 `reqwest::Error` 走 `Display`）：`:563`（token 响应读不懂）、
`:650`（查机器人信息失败）、`:766`（§3.7(b) 探测没跑成）。一起改。

**改法方向**：写一个走 `std::error::Error::source()` 的链式渲染小工具（`e.to_string()` 起头，
一路 `.source()` 拼到底），把这四处的 `tail(&e.to_string(), 300)` 换掉。三条约束：
① 拼出来的串仍要过 `tail()`（自检行是给人一眼扫的）；② 渲染时仍走 `redactor.scrub()`
（`render_text` `:1400` / `render_json` `:1478` 已经统一做了，别绕开）；
③ **判据是「三种病打出来必须不一样」** —— 回执里贴至少两种真实输出的对照（上面两条命令直接可用），
不许只说「加了 source 链」。

**第 5 组这样改不好使 —— 这条要单独看**（复核 agent 点名的坑，已核实）：
`check_model` 拿到的是 `ModelError`（`:899` 的 `Ok(Err(e))` 那支），而链在上游就被拍平了：

- `core/crates/models/src/lib.rs:423`：`.map_err(|e| ModelError::Upstream(redact(&e.to_string(), &self.api_key)))?`
- `core/crates/models/src/lib.rs:429`：同样一句（读响应体那发）

`ModelError`（`core/crates/contracts/src/errors.rs:71–79`）是三个 `String` 变体，**没有 `#[source]`
字段，而且 contracts crate 在冻结面上** —— 给它加 source 就是动契约，**停下报告**。
唯一的杠杆是在 `models/lib.rs` 那两行把链**拼进字符串**（拼完仍要过那里的 `redact()`，
那是模型密钥的兜底闸，不许绕）。而 `core/crates/models/**` **不在你的可写面上**。

所以第 5 组这一条有两条路：**① 停下问总管要授权，改 `models/lib.rs:423/429` 那两行；
② 不动，在回执里写明第 5 组为什么打不出真原因、下一批该谁做。**
总管的倾向是 ①（两行、不改行为、不动契约、`aite-models` 不在契约锁面上所以 `OK 25 files` 不变），
但**必须先问过再动**。`:869`（`from_config` 失败）那支是 `ModelError::Config`，本来就没有链，不用管。

---

### 6.3 Redactor 口子①：`check_config` 失败时兜底闸压根没上

**台账**：§4.2 最后一行。**锚点核对**：`run_checks` 在 `preflight.rs:1307`，
早退那一段在 `:1322–1340`（台账 §4.1 里的 `:1314` 指的就是这个函数）。

**病在哪**：`run_checks` 读完配置就分叉 ——

```
:1319   let (cfg_result, cfg) = check_config(&config_path, fell_back);
:1322   let Some(cfg) = cfg else {
:1332       checks.push(skipped(name, title, "第 1 组没过，配置读不出来"));   // ×6
:1334       return Report { ... };                                          // ← 这里就回去了
:1342   checks.push(check_env(&cfg, env, opts.offline));
```

而 `redactor.add()` 的**全部五个调用点**都在这一行之后：`:472` / `:473` / `:474`（第 3-4 组）、
`:612`（tenant_access_token）、`:813` / `:864`（第 5 组）。也就是说 **配置读不出来的那条路上，
`Redactor` 是空的**，而 `check_config` 的 FAIL detail（`:316`）回显的是 serde_yaml 的错误 ——
**serde_yaml 会把出错的标量原样打出来**。

**怎么验证它确实病着**（2026-09-12 实跑）：拿一份 config，把某个数字字段写成一个字符串，
且这个字符串**恰好就是环境变量里那个凭证的取值**（真机上最典型的形态：用户把 app_secret
粘进了 `config/aite.yaml`，而 preflight 的 fix 正好在教他「密钥只写环境变量名，不写取值」）：

```bash
# bad.yaml：history_window: cli-a1b2c3SUPERSECRETVALUE
FEISHU_APP_SECRET="cli-a1b2c3SUPERSECRETVALUE" \
  core/target/debug/aite preflight --offline --json --config bad.yaml
```

输出（`--json` 的第一条 check，原样）：

```json
{
  "detail": "配置文件读不懂：…/bad.yaml：yaml: invalid type: string \"cli-a1b2c3SUPERSECRETVALUE\", expected u32",
  "extra": {},
  "fix": "照 config/aite.example.yaml 的形状修 …/bad.yaml；密钥只写环境变量名，不写取值",
  "name": "config",
  "status": "fail",
  "title": "配置可加载"
}
```

**取值原样漏出来了**，尽管它在 `FEISHU_APP_SECRET` 里、Redactor 本来完全认得它。
纯文本档（`--offline` 不带 `--json`）同样漏，退出码 1。

**改法方向**：让 `Redactor` 在 `check_config` 之前（或至少在**渲染之前**）就装上料。
边界要想清楚：`redactor.add` 现在的输入是「从 config 里读出的 `*_env` 名字 → 环境变量取值」，
而这条路上**恰恰是 config 读不出来**，拿不到 `*_env` 的名字。三条路：
① 早退前用一份**契约默认** `AiteConfig`（`AiteConfig::default()`）的 `env_var_names` 兜一遍
（默认那四个名字就是真机上的那四个）；② 直接扫环境变量里所有名字命中 `FEISHU_*` / `*_API_KEY`
这类模式的取值；③ 把「装料」提到 `run()`（`:1518`）里、`run_checks` 之前，用默认 config 兜底、
读到真 config 后再补一遍。**总管的倾向是 ①ד早退前也要装料"这个位置**（最小改动、语义最直白，
且顺带把 §6.4 一并解决），但②③也说得通。回执里说清选了哪条，并贴「改前漏 / 改后被抹」两次实测。

**这条一定要配一条测试**（放 `tests/preflight_*.rs`，对应 Python 第 17 条 +第 8 条的合体）：
配置读不出来 + 环境变量里有取值 + 出错标量就是那个取值 → 输出里不许出现取值、必须出现
`的取值已隐去`。**把你新加的装料那一步拆掉，这条要红。**

---

### 6.4 Redactor 口子②：`add` 硬编码四个变量，而 `env_var_names` 是泛化扫的

**台账**：§4.2 最后一行的后半句。**核对结果**：属实。

**病在哪**：`env_var_names`（`:285`）是**泛化**扫 config 里所有 `*_env` 字段的，它自己的注释
（`:281–284`）明说：

> generic 地扫而不是写死那四个名字：契约以后再加一个 `*_env`，这里自动跟上，
> 不会出现「新加的凭证自检查不到」这种静默盲区。

而 `redactor.add` 的五个调用点是**写死的四个字段**：`cfg.feishu.app_id_env`（`:472`）、
`cfg.feishu.app_secret_env`（`:473`）、`cfg.feishu.bot_open_id_env`（`:474`）、
`cfg.model.api_key_env`（`:813` / `:864`），外加一个运行时才有的 `tenant_access_token`（`:612`）。
**契约加第 5 个 `*_env` 的那天**：第 2 组（`check_env`）会自动报它「在不在」，
而 Redactor 永远不认识它的取值 —— **脱敏静默漏，且没有任何一条测试会红**。

**怎么验证它确实病着**：`env_var_names` 现在返回 4 项（`{feishu.app_id_env, feishu.app_secret_env,
feishu.bot_open_id_env, model.api_key_env}`，实跑第 2 组输出「3/4 个未设置…（已设置：AITE_MODEL_API_KEY）」
就是这四项）；把这 4 项跟 `:472/:473/:474/:813` 那四个字段名一比，是**手工对齐**的，
没有任何东西保证它们同步。

**改法方向**：让装料走 `env_var_names(cfg)` 的返回值，而不是写死四个字段。
配一条测试（对应 Python 第 24 条 `test_env_var_names_covers_every_env_field`），
判据不要写成「等于那四个名字」的快照 —— 那样契约加第 5 个的时候测试会红，但红的是**测试过期**，
不是**脱敏漏了**。判据要写成「`env_var_names` 报出来的每一项，只要环境里有取值，
Redactor 就认得它」—— 这样契约加第 5 个的那天，测试**自动跟着覆盖**它。
**要能说出「把装料改回写死四个，这条会不会红」，并贴出破坏→红、还原→绿两次输出。**

---

### 6.5 `MIN_REDACT_LEN` 判的是字节不是字符 —— 非 ASCII 取值会把正常输出打成马赛克

**台账**：§4.4 low，`preflight.rs:212`。**核对结果：212 行准确**：

```rust
if value.len() < MIN_REDACT_LEN || self.items.iter().any(|(v, _)| v == value) {
```

`MIN_REDACT_LEN = 4`（`:72`），`String::len()` 是**字节数**。Python 那版是 `len(value) < 4`，
Python 的 `len` 是**字符数** —— 移植时语义静默变了。两个汉字 = 6 字节 = 过闸，
于是任何输出里出现这两个字都会被替换。

**怎么验证它确实病着**（2026-09-12 实跑，`AITE_MODEL_API_KEY` 设成两个汉字「配置」）：

```
[5/7] FAIL 模型端点通       «AITE_MODEL_API_KEY 的取值已隐去»不全，缺：model.base_url / model.model
[6/7] FAIL 沙箱可用         …
           └ 怎么补：先起 `aite-edge --config <«AITE_MODEL_API_KEY 的取值已隐去»>`；起着的话看它的日志为什么不应答
```

第 5 行本来是「**配置**不全，缺：…」，第 6 行的 fix 本来是「`aite-edge --config <配置文件>`」。
**preflight 的排障输出被自己的脱敏闸打烂了** —— 而这正是它最该说人话的时刻。

**改法方向**：闸门改成判字符数（`value.chars().count()`），对齐 Python。
**边界要在回执里说清**：改完之后，一个真的只有 2–3 个字符的凭证就**不会**被抹了。总管的判断是
「2–3 个字符的东西不是密钥，而把正常输出打成马赛克是实打实的伤害」，Python 也是这个取舍 ——
但这是个取舍，你要在回执里承认它，别当成纯粹的 bugfix。
`:1657` 那条已有的测试（`scrub_leaves_values_shorter_than_the_floor_alone`）现在拿
`"x".repeat(MIN_REDACT_LEN)` 做边界，字节与字符恰好一致所以钉不住这条 —— **补一条非 ASCII 的**。

---

### 6.6 顺带两条（都在 `check_storage`）

**(a) 落盘检查 FAIL 时「卡在 」可能是空的。**
台账 §4.4 low 记的是 `preflight.rs:1218`；**核对结果：实际是 `:1248`**（`bad.push` 那行），
成因在 `:1215–1220` 的 `rel()` 闭包 —— 卡住的那层恰好是仓库根时 `strip_prefix` 得到空路径，
`display()` 出空串。

实测（在一个 `chmod 500` 的目录里、拿默认相对路径的 config 跑 `preflight --offline`）：

```
[7/7] FAIL 落盘目录可写     写不下去：data/aite.db（卡在 ）；data/evidence（卡在 ）；data/artifacts（卡在 ）
           └ 怎么补：给这几层目录写权限，或把 config 里的 storage.* 指到一个可写的位置
```

「卡在 」后面什么都没有 —— 总管看不出卡在哪一层。改法方向：`rel()` 在空串时回退成
`.`、仓库根的绝对路径、或「仓库根」三个字，随你，但要让人一眼知道是哪一层。
配 Python 第 18 条那种测试（造一个不可写的祖先），**注意别在测试里 `chmod` 仓库自己的目录**，
用 `tempfile` 建。

**(b) 可写探测有写副作用。**
台账 §4.4 low 记的是 `:1252`；**核对结果：调用点在 `:1237`，`writable_dir` 本体在 `:1282`**。
它是**真建一个 `.aite-preflight-<nonce>` 目录再删掉**（不是 `access()`）。
后果：`aite preflight` 从此对「三个落盘路径的最近已存在祖先」有写副作用 —— 真机上
`evidence_dir` / `artifacts_dir` 已经存在时，探针就落在它们里面。

这条**不一定要改**：函数自己的注释写明了理由（「问的就是要问的那件事」），而 preflight
本来就是起飞前跑的。要你做的是**判断 + 交代**：如果留着，回执里说清「留着的理由 + 崩溃/被 kill
时会留下一个空目录」这个残留面；如果改（比如探测完立刻删、失败也删，或改成在父目录探测），
说清改成了什么。**顺带注意**：`core/crates/app/tests/` 有一条整轨硬约束「一个字节都不许写进
仓库的 `data/`」（`tests/cold_start_to_delivery.rs:220`、`tests/evidence_on_disk.rs:340`）——
**你新加的 `tests/preflight_*.rs` 也必须守它**，`storage.*` 一律指到 `tempfile` 建的临时目录。

---

## 纪律

1. **契约与锁**：`proto/**`、`core/crates/contracts/**`、`.contracts.lock` 冻结，
   全程 `OK 25 files`。要动（包括给 `ModelError` 加 `#[source]`）→ **停下报告**。
2. `evals/p0/*.yaml` 十个场景是验收面，**一个字不动**。
3. `docs/dev-spec-2026-09-09.md` 与 `docs/dev-spec-2026-09-11-rustgo.md` 冻结。
4. 不 panic；不在 async 里阻塞；**测试不靠真实 sleep**，也不靠墙钟阈值当判据
   （上一轮刚修掉一条「两万个 tick 必须 1 秒内跑完」的假红门禁，别再造）。
   你的 HTTP 桩要用 `tokio::net::TcpListener` 拿到**真端口**再传给被测代码，不要「睡 100ms 等它起来」。
5. `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check`、
   `go vet`、`gofmt`、`go test -race` 全干净。
6. **密钥红线是这一轨的中心，不是附带条款。** 密钥只从配置点名的环境变量读，任何日志 / 错误 /
   Debug 输出不得出现取值。上一轮刚补的三层测试（纯函数 `:1618/:1638/:1657` / 接线 `:1727/:1748` /
   渲染面 `:1676`）是这条红线唯一的守门人 —— **不许削弱，只能加强**。测试里的假取值一律用
   `preflight.rs:1608–1612` 那种一眼认得出是假的常量，**真密钥一个字都不许进代码或回执**。
7. **每条结论挂实测。**「应该会」「大概」一句不要。改了测试的，要能说出
   「把被测行为破坏掉，这条会不会红」，并把破坏→红、还原→绿两次输出贴出来。
8. **改了 preflight 的输出文案**（第 6.2/6.5/6.6 都可能改），把 `docs/acceptance-M.md` 里
   引用到它的地方同步过来。
9. **不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v2` 分支上，回执贴出来。
10. 六轨同时在跑：cargo 构建锁与 Docker daemon 是全机共享的，你的命令可能要排队等几分钟，
    这是正常的，别以为卡死。

## 验收

```bash
cd /Users/shensikai/Documents/Aite/.worktrees/task-v2

scripts/check.sh
#   期望最后一行「全部通过」，退出码 0
#   契约锁          OK 25 files                （不许变）
#   契约测试        contracts passed=25 failed=0（不许变）
#   B8 评测         passed 10/10               （不许变）
#   全量 cargo test cargo passed=718+N failed=0（N = 你新加的条数；718 是起跑线，只许涨）

cd core && cargo test -p aite preflight -- --nocapture ; cd ..
#   期望 0 failed；把 N 和 718+N 都写进回执

cd core && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check ; cd ..
#   期望无输出、退出码 0

core/target/debug/aite preflight --offline
#   期望：七行都有结论，1/2/7 有判据、3/4/5/6 是 SKIP，退出码 0
#   （合并时的基线是「汇总：OK 2 · WARN 1 · FAIL 0 · SKIP 4（共 7 项，过了 7 项）」，
#     这台机器上 AITE_MODEL_API_KEY 设没设会让第 2 组在 OK/WARN 之间摆，两种都对）

core/target/debug/aite preflight --offline --json | head -20
#   期望：合法 JSON，checks 七项，每项都有 name/title/status/detail/fix/extra

core/target/debug/aite preflight --help ; echo "exit=$?"
#   期望：打出 USAGE，exit=0
```

**§6.2 的对照实测**（这一条不许只说「改了」，要贴输出）：

```bash
FEISHU_APP_ID=cli_fake FEISHU_APP_SECRET=fakesecret1234 \
  HTTPS_PROXY=http://127.0.0.1:9 core/target/debug/aite preflight --config <填好的 config> 2>&1 | sed -n '/3\/7/,/4\/7/p'
FEISHU_APP_ID=cli_fake FEISHU_APP_SECRET=fakesecret1234 \
  HTTPS_PROXY=http://no-such-proxy-host.invalid:8080 core/target/debug/aite preflight --config <同一份> 2>&1 | sed -n '/3\/7/,/4\/7/p'
#   改前：两次逐字相同（见 §6.2）。改后：两次必须不一样，且能看出一个是连接被拒、一个是解析不了主机名
```

**§6.3 / §6.5 的对照实测**：照 §6.3、§6.5 里给的两条命令跑，贴改前改后。

真机那一档（有凭证时才跑，没有就在回执里写「无凭证，未跑」）：

```bash
core/target/debug/aite preflight                       # 期望七组全 OK，退出码 0
core/target/debug/aite preflight --chat-id <测试群 chat_id>   # 期望 §3.7(b) 那条 note 从 unverified 变 verified
```

## 回执格式

```
## V2 回执

基线 0bc8d55 → 提交 <短 sha>

### 6.1 接管 Python 25 条
取原版用的命令跑通了没：<>
新增测试：<条数> 条，落在 <preflight.rs mod tests: N 条 / tests/preflight_*.rs: M 条>
25 条逐条交代（照派单那张表）：
| # | Python 用例 | 我怎么接管的 | 破坏被测行为会不会红 |
第 16 条（unexpected_crash）我选了：<①销账 / ③换语义 / 其他>，理由 <>
第 6 组抽函数抽成了什么形状：<签名 + 为什么>
HTTP 桩怎么搭的：<> ； 本机 proxy 环境实测：<env | grep -i proxy 的输出>

### 6.2 source() 链
改了哪几处：<行号>
改前（两条命令的输出，逐字相同）：
<贴>
改后（两条命令的输出，必须不同）：
<贴>
第 5 组（模型端点）：<① 问过总管、改了 models/lib.rs:423/429，贴改前改后 / ② 没动，理由 + 下一批该谁做>

### 6.3 Redactor 口子①（check_config 早退）
我选了哪条路：<①/②/③/其他>，理由 <>
改前（取值原样漏出来）：<贴>
改后（被抹成 «…的取值已隐去»）：<贴>
拆掉装料那一步，新测试红了没：<贴红的输出>

### 6.4 Redactor 口子②（硬编码四个变量）
改法：<>
测试的判据是怎么写的（为什么契约加第 5 个 *_env 时它会自动跟上、而不是变成过期快照）：<>
把装料改回写死四个 → 红：<贴> ；还原 → 绿：<贴>

### 6.5 MIN_REDACT_LEN 字节 → 字符
改前（正常输出被打成马赛克）：<贴>
改后：<贴>
我承认的取舍：<2–3 个字符的取值此后不再被抹，理由>

### 6.6 check_storage 两条
(a)「卡在 」空：改成了 <>，改前改后各贴一行
(b) 可写探测的写副作用：<留着 / 改了>，理由 <>；新测试有没有写进仓库 data/：<没有，用的 tempfile>

### 文档同步
改了哪些输出文案 → docs/acceptance-M.md 同步了哪几处：<没改就写"没改">

### 实测输出（粘实际的）
$ scripts/check.sh
<最后 5 行；cargo passed=718+N failed=0，契约锁 OK 25 files，B8 passed 10/10>
$ cd core && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
<>
$ core/target/debug/aite preflight --offline
<汇总那两行 + 退出码>
$ cd edge && go test -tags docker ./internal/sandbox/... -count=1
<>
$ docker ps -a --filter label=aite.task -q | wc -l
<期望 0>

### 开场自检
guard_bash.py 被守卫拦下来了没：<必须"拦下来了">

### 要总管决定的
<没有就写"没有">
```

**不要 commit 到 main，不要 merge，不要 push。** 做完提交在 `task-v2` 分支上，回执贴出来。
