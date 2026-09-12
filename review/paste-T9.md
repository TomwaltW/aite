# 任务 T9 — 起飞前自检 `scripts/preflight.py`

## 背景：这轨是从哪来的

T1–T6 六轨全部合进 main（最新 `34e9dee`）。全仓 **863 条测试全绿**，
§3.8 的 10 个评测场景 **passed 10/10**。P0 的最后一步是 §2.4 的人工验收
**M1–M6：在真实飞书群里跑**（并行的 T7 正在补 `aite/app.py` 的组装，让进程能起飞）。

问题在于：**现在没有任何东西能在起飞前告诉你外部依赖齐不齐。**
凭证有没有设、飞书应用权限批没批、模型端点通不通、沙箱镜像在不在 ——
今天全都得等到 `python -m aite.app` 跑起来、在群里 @ 一句、然后看它怎么炸。
真机验收本来就难复现，再让它连"是环境没配好还是代码有问题"都分不清，就更难查了。

你这轨造那个"起飞前 60 秒"的自检：**一条命令，把所有外部依赖挨个点名，
每项 OK/FAIL + 一句人话说清怎么补。**

同时并行的还有三轨（T7 组装、T8 集成测试、T10 evidence 时间线），
它们都不碰 `scripts/preflight.py` 和 `tests/scripts/`。

## 工作区

```
worktree : /Users/shensikai/Documents/Aite/.worktrees/task-t9
分支     : task-t9
基线     : 34e9dee   ← main 的 HEAD，T7–T10 钉的是同一个 sha
Python   : 用 worktree 里的 .venv/bin/python（3.12.1）
```

本机默认 `python3` 是 3.11，满足不了 `requires-python>=3.12`。**venv 已经建好并
`pip install -e ".[dev]"` 过了，你不用再装**。文档正文里的 `python` 字样照原样保留，
只有本机执行时才换 `.venv/bin/python`。

## 第 1 步：开场自检（先跑这个，任何一条对不上就停下报告，别往下做）

```bash
git rev-parse --short HEAD              # 期望 34e9dee
git rev-parse --abbrev-ref HEAD         # 期望 task-t9
git status --porcelain                  # 期望 空输出
.venv/bin/python -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"   # 期望 p0.1
.venv/bin/python -m aite.contracts.lock --check   # 期望 OK 11 files，退出码 0
.venv/bin/ruff check .                            # 期望 All checks passed!，退出码 0
.venv/bin/python -m pytest -q --co                # 期望 863 tests collected
.venv/bin/python -m pytest tests/contracts -q     # 期望 335 passed
.venv/bin/python -m pytest -q -m "not docker"     # 期望 827 passed, 36 deselected，退出码 0
.venv/bin/python -m aite.evals run evals/p0 --platform fake --model scripted
                                        # 期望 末行 passed 10/10，退出码 0
ls scripts/                             # 期望 只有 check.sh ← 你的起点
docker images aite-sandbox              # 期望 列出 aite-sandbox:p0（本机已构建过）
```

上面这些是我在同基线的 worktree 里实跑出来的值，不是估计。

**第 13 条，必须做**：用 Read 工具读 `.claude/hooks/guard_bash.py`。
**被拦才算 hook 挂上了。** 读到内容说明 PreToolUse 守卫没生效（多半是会话不是从
worktree 根启动的），而 hook 失败是**非阻塞放行且不报警**的，守卫会静默失效。
这种情况停下报告，别接着做。

## 先读

- `docs/dev-spec-2026-09-09.md` 的 §2.4（M1–M6 要什么）、§3.7（飞书权限，含两条
  **待核实**项）、附录 A（飞书 API 备忘）。**不许改**。
- `aite/contracts/config.py` 的 `AiteConfig` —— 你要检查的每一项都从这里的字段来。
- `config/aite.example.yaml` —— 密钥只写**环境变量名**的口径，你的输出也要守这条。
- `aite/adapters/feishu/api.py` 的 `FeishuApiClient`：
  `tenant_access_token()` 换 token、`request()` 发请求、`aclose()` 收尾；
  错误统一是 `aite.adapters.feishu.errors.PlatformError`。
- `aite/models/openai_compat.py` 的 `resolve_api_key(cfg, env)` 和 `OpenAICompatModel`。
- `aite/sandbox/docker_sandbox.py` 的 `DockerSandbox`。
- `scripts/check.sh` —— 现有脚本的风格（输出格式、退出码），你的脚本照这个调子。

## 可写路径

```
scripts/preflight.py
tests/scripts/**
```

其余全仓只读。**`aite/` 下一个文件都别改** —— 你是只读地使用那些实现。
缺什么、接不上 → 按 §7 停下报告。

`aite/app.py` 是 T7 的，`tests/integration/**` 是 T8 的，
`scripts/evidence_show.py` 与 `tests/tools/**` 是 T10 的。都别碰。
`scripts/check.sh` 是既有文件，**不许改**。

## 目标

`scripts/preflight.py`，一条命令跑完下面七组检查，每组一行结果：

| # | 检查 | 判据 |
|---|---|---|
| 1 | 配置可加载 | `load_config(path)` 不抛；打出 platform / model.provider / sandbox.image |
| 2 | 环境变量齐 | 按 config 里每个 `*_env` 字段的**变量名**去查在不在。`feishu.app_id_env` / `app_secret_env` / `bot_open_id_env` / `model.api_key_env` |
| 3 | 飞书凭证有效 | `FeishuApiClient.tenant_access_token()` 换得到 token |
| 4 | 飞书身份对得上 | 用 token 查机器人自身信息，与 `FEISHU_BOT_OPEN_ID` 的取值比对（对不上就是配错了应用，M1 会静默不响应） |
| 5 | 模型端点通 | 拿 `OpenAICompatModel` 发一次最小 chat，报延迟、token 数、按 config 里的价格算出的花费 |
| 6 | 沙箱可用 | docker daemon 连得上、`config.sandbox.image` 在本机、能起一个容器跑通 `import pandas, matplotlib, openpyxl, docx`，跑完**必须把容器收掉** |
| 7 | 落盘目录可写 | `storage.sqlite_path` / `evidence_dir` / `artifacts_dir` 三个路径的父目录存在且可写 |

硬要求：

1. **绝不打印任何密钥取值。** 第 2 组只报「`FEISHU_APP_ID` 已设置 / 未设置」，
   连前几位都不许打。第 3–5 组报的是"通没通"，不是拿到了什么。
   这条是这个脚本能不能进仓库的红线。
2. **一项失败不阻断后面的检查**，全部跑完再汇总。真机排障时最烦的就是修一条重跑一次。
3. **每条 FAIL 都要带一句"怎么补"**：缺变量就说 export 哪个名字；镜像不在就说
   `docker build -t aite-sandbox:p0 docker/sandbox`；飞书 401 就指向 §3.7 的权限清单。
4. `--offline`：只跑 1、2、7（不需要网络和 docker 的那些），CI 与没凭证的机器上能跑。
5. `--json`：机器可读输出（每项 name / status / detail），给以后接 CI 用。
6. 退出码：全 OK → 0；任一 FAIL → 1。`--offline` 下同理。
7. **§3.7 的两条待核实项**（`supports_passive_listen`、群历史是否要敏感权限）
   在真实应用上是什么结论，你**顺手能查到就写进输出**（比如第 4 组之后加一行提示），
   查不到就明确打印「未核实」——别假装知道。

`tests/scripts/` 下配测试：把外部依赖（httpx / docker / 环境变量）都打桩，
**测试里不许真连网、不许真起容器**（`-m docker` 那个标记是给真沙箱测试用的，
你的测试不该需要它）。至少钉住：全通时退出码 0、某项缺失时退出码 1 且
FAIL 行里点名了缺的东西、`--offline` 不碰网络和 docker、密钥取值不出现在任何输出里
（这条要有专门一条测试，拿一个假密钥跑一遍然后断言它没出现在 stdout/stderr）。

## 验收

```bash
.venv/bin/ruff check .                          # All checks passed!
.venv/bin/python -m aite.contracts.lock --check # OK 11 files
.venv/bin/python -m pytest tests/scripts -q     # 你新增的，全绿
.venv/bin/python -m pytest -q -m "not docker"   # 827 + 你新增的条数，一条不许红
.venv/bin/python scripts/preflight.py --offline # 退出码 0（本机有 config/aite.example.yaml）
.venv/bin/python scripts/preflight.py --help    # 退出码 0
```

本机**没有**飞书凭证也**没有**模型 key，所以 3/4/5 三组你在这里只能跑到
「未设置 → FAIL + 提示」这个分支。那正是它该有的样子：把这个输出**原样贴进回执**，
我拿真凭证跑的时候对照着看。第 6 组（docker + 镜像）本机是齐的，**要真跑通**。

## 回执格式

做完在最后贴一段，格式照这个：

```
RECEIPT T9 status=done commit=<短 sha> checks=<过了几项>/<共几项> files=<改了几个文件>
```

另外用人话写清楚：

- `scripts/preflight.py`（无参数）在这台没凭证的机器上的**完整输出**，原样贴
- 第 6 组你怎么保证容器一定被收掉（异常路径也要收）
- 「不打印密钥」你是怎么保证的，以及那条测试是怎么写的
- §3.7 那两条待核实项你查到了什么；没查到就直说
- 有没有哪一项你判断**没法在起飞前查**（比如某个权限只有真收到消息才知道有没有），
  逐条列出来 —— 这直接决定 M1–M6 出问题时还剩多少盲区

**不要 push，不要合 main。** 我这边统一并轨。
