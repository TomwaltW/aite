# 移植清单 · Gateway / 沙箱 / 模型客户端 / 测试替身 / 评测 / scripts（Python → Rust + Go）— 由只读探查 agent 于 2026-09-11 生成

目标：`aite-gateway` + `aite-edge-client`（R6）、`edge/internal/sandbox`（R2，Go）、`aite-models`（R5）、`aite-testing` + `aite-evals`（R7）、`aite preflight`（RΩ）。以 Python 源码为最终依据；本清单是索引与提醒。

---

## 1. Tool Gateway（`aite/gateway/`）

### `call()` 顺序（`tool_gateway.py:100-140`）
1. `started = perf_counter()`，`budget = 60`
2. **token 校验** `_check_token(ctx)`：期望值（`token_resolver(task_id)` 优先，否则内部 `_tokens` 表）为空 → `denied`；`ctx.session_token` 空 → `denied`；**`compare_digest(expected.encode(), got.encode())`** 不等 → `denied`（先 encode 是刻意的：含中文的 str 会让 compare_digest 抛 TypeError → 被兜成 upstream，而它应该是 denied）
3. **查工具存在**：不在 `GATEWAY_TOOLS` → `not_found`；不在 `TOOL_IMPLS` → 也 `not_found`（消息「在本次运行里没有实现」）
4. **schema 校验** `validate_arguments(spec.parameters, req.arguments)` → `SchemaViolation` → `invalid_args`；**返回补齐 default 后的新 dict**
5. **超时预算**：非 `run_python` = 60s；`run_python` = `args["timeout_sec"]`（默认 120）**+ 5s grace**
6. **执行** `wait_for(impl(env, ctx, args), budget)`
7. 成功 → `_ok()`：截断 content、带 data/artifacts/duration_ms

### 异常 → 错误码
token 缺失/不匹配 → `denied`；工具名不在目录/无实现 → `not_found`；`SchemaViolation`（类型、必填、边界、未知参数、非 dict）→ `invalid_args`；`wait_for` 超时、`run_python` 见到 exit 124 → `timeout`；平台侧异常（read_history/read_document/download_file 抛）、没接平台 → `upstream`；沙箱 acquire/exec/put_file 抛、没接沙箱 → `sandbox`；**工具实现里任何未预料异常** → `upstream`（消息带 `{ExceptionType}: {msg}`）；`CancelledError` **原样 re-raise**（Rust：任务取消不是 ToolResult）。

### schema.py 自研校验器
顶层必须 `type: object`；`properties`、`required`；子级 `type`（str 或 list）、`enum`、`default`；string `minLength/maxLength`；number/integer `minimum/maximum/exclusiveMinimum/exclusiveMaximum`；array `minItems/maxItems/items`（dict 时递归）；object 嵌套。类型谓词 boolean/integer/number/string/array/object/null，**bool 显式排除在 integer/number 之外**。三条本层策略：**未知参数直接拒**（消息列出可用参数名）；**不做类型转换**（"50" 不变 50）；不认识的关键字忽略但不支持的 `type` 值抛。
`aite/testing/jsonschema_mini.py` 是另一份独立实现（FakeGateway + ModelProbe 用）：关键字更少（无 minLength/maxLength/exclusive*），**不拒未知参数**。两份行为差异是有意的，分别保留。Rust：`aite-gateway::schema` 用 `jsonschema` crate + 前置「未知参数」检查；`aite-testing` 里保留一份 mini 版语义。

### content 截断
`_clip(text, 12000)`：`len <= 12000` 原样；否则 `text[:12000 - len(marker)] + marker`，`marker = "\n…[内容已截断]"`（8 字符）→ **结果长度恰好 12000**（按字符/码点，不是字节）。失败结果的 content 也走 `_clip`（`_failed()` 把 message 同时写进 content 和 error.message；error.message 不截断）。

### duration_ms / artifacts / data
`duration_ms = int((perf_counter() - started) * 1000)`，成功失败都写，从 `call()` 入口算；`artifacts` 只成功时带；`data` 成功透传，失败 None。`catalog(ctx)` 返回 `list(GATEWAY_TOOLS)`，不按 ctx 裁剪。
非契约附加面（Rust 契约已显式化）：`register_task(task_id, token)`（空 token 抛）、`unregister_task`、`sandbox_id_of(task_id)`、`release_task(task_id)`（幂等：撤 token + 丢 acquire lock + `sandbox.release`）。

---

## 2. 五个工具（`aite/tools/`）
工具不构造 ToolResult，失败 `raise ToolFailure(code, message)`。`ToolEnv` 给两个闭包：`acquire_sandbox()`（有就复用、没有才建）和 `current_sandbox_id()`（有就给、没有 None）。

### `read_group_history`
- `limit`（1–200，默认 50）、`thread_only`（默认 false）；`thread_only=True` 且 `ctx.thread_id is None` → 成功返回 `"本次对话不在话题里，没有话题历史可读。"`，data `{messages:[], count:0, filtered_out:0, thread_only:true}`
- `platform.read_history(chat_id, limit, thread_id)` 异常 → `upstream`
- **过滤** `str(m.sender_kind) == "human"`（核心职责；PlatformPort 不过滤）
- content：`本群最近 {N} 条真人消息（另有 {M} 条机器人/应用/系统消息已过滤）：\n[om_h1] Alice: 文本\n…`，每行 `f"[{message_id}] {sender_name or sender_id}: {text}".rstrip()`；`dropped==0` 时不带括号段；全被过滤：`"本群没有可引用的真人消息。（读到 N 条，全都不是真人发的，已丢弃）"`；一条都没有：括号里 `（这里还没有消息）`
- data `{messages: [逐条 JSON], count, filtered_out, thread_only}`

### `read_document`
`url_or_token` 必填；任何异常 → `upstream`；content `f"# {title}\n\n来源：{url}\n\n{text}"`；data `{title, url, chars}`。

### `download_attachment`
`file_key` 必填；**`ctx.attachments_message_id` 为空 → `upstream`**（"本次消息没有附件"）；`download_file` 异常 → `upstream`；`acquire_sandbox()` + `put_file` 异常 → `sandbox`（**两段两种码**）。落盘 `_safe_name(file_key)`：1 `PurePosixPath(file_key).name`（空则原串）2 正则 `[^0-9A-Za-z._一-鿿-]+` 折成 `_` 3 `.strip("._")` 4 空则 `"attachment"`，`[:120]` 5 `f"/work/in/{name}"`。content `f"附件已下载到沙箱：{path}（{len} 字节）"`；data `{path, size, file_key(原始)}`。⚠ FakeToolGateway 用未清洗的 `/work/in/{file_key}`。

### `run_python`
`code` 必填、`timeout_sec` 1–300 默认 120；沙箱归属：`env.acquire_sandbox()` → gateway 的 `_acquire_sandbox(task_id)`（per-task 双检锁，并发两个 tool_call 不会各建一个；`sandbox_id` 存 gateway 的 `_sandbox_ids: dict`，不存 Task 也不存 sandbox）；acquire 抛 → `sandbox`；exec 抛 → `sandbox`；exec 返回后、判超时前 `touch`（失败静默）；`exit_code == 124` → `ToolFailure(timeout, f"代码执行超过 {timeout_sec}s 上限，已在沙箱内终止")`；**用户代码报错不是工具失败**（非 124 的非零 exit 照样 ok=True）。content（段落 `\n\n`）：`执行成功（{ms} ms）` 或 `代码以退出码 {n} 结束（{ms} ms）`；`stdout:\n{stdout}`（空 → `stdout: （空）`）；`stderr:\n{stderr}`（非空才有）；`注意：输出太长已被截断。`（truncated）；`/work 下本次新增/修改的文件：\n  /work/out.png（40139 字节）` 或 `本次没有在 /work 下产出文件。`。data `{exit_code, stdout, stderr, truncated, duration_ms, files_out}`；artifacts 每个 files_out 一条 `ArtifactRef{path, title=basename, mime=guess}`。

### `list_files`
无参数；**不建沙箱**：`current_sandbox_id()` None → `"沙箱还没启动，/work 下还没有文件。"`，data `{files: [], count: 0}`；异常 → `sandbox`；空 `"/work 下还没有文件。"`；非空 `f"/work 下有 {n} 个文件：\n" + "\n".join(paths)`；data `{files, count}`。

---

## 3. DockerSandbox（`aite/sandbox/`）→ Go（R2）

### acquire
先 `images.get(spec.image)`：不存在 → `SandboxError("沙箱镜像不存在：{image}。先跑 docker build -t {image} docker/sandbox")`。`containers.create`：`image`、`command=["sleep","infinity"]`、`labels={"aite.task": task_id, "aite.managed": "p0"}`、`working_dir=/work`、`network_mode=none`、`mem_limit=f"{mem_mb}m"`、`nano_cpus=int(cpu*1e9)`、`pids_limit=256`、`cap_drop=["ALL"]`、`security_opt=["no-new-privileges:true"]`、`environment={"AITE_WORKDIR": workdir}`、`tty=False`、`stdin_open=False`、`detach=True`；无 tmpfs、无 read_only、无 volume；用户来自镜像 `USER aite`（uid 1000）。`start()`；记账 `_boxes[sandbox_id] = _Box(sandbox_id, task_id, workdir, last_active=monotonic())`；最后 `_check_ready`（容器内 python 检查 `timeout` 在、workdir 可写，打印 `AITE_SANDBOX_OK`），**不过就 release 再抛**。

### exec
1 `_require_box` → `SandboxNotFound`；容器 NotFound → 从 `_boxes` 删 + `SandboxNotFound` 2 **前快照**：容器内跑 `_SNAPSHOT_CODE`（`os.walk(AITE_WORKDIR)` 收 `{path: [size, mtime_ns]}` json 到 stdout） 3 代码写临时文件 `/tmp/aite_exec_{uuid}.py`（mode 0600，`put_archive`）—— **落 /tmp 不落 /work** 4 命令 `["timeout", "-k", "2", str(max(1, timeout_sec)), "python", "-u", script]`，`exec_run(workdir=box.workdir, demux=True)`；超时由**容器内 coreutils timeout** 强制 5 后快照 + `rm -f script` + `box.touch()` 6 超时判定 `exit_code == 124 or (exit_code == 137 and elapsed >= timeout_sec)`（137 也可能是 OOM），成立则 exit_code 改 124 并 stderr 追加 `[aite] 执行超过 {N}s 上限，已在沙箱内终止` 7 截断：stdout/stderr 各自 `_clip(20000)`，marker `"\n…[输出已截断]"`，任一截断 → `truncated=True` 8 `duration_ms` 从入口算（含两次快照） 9 `files_out = _diff_files(before, after)`：`sorted(after.items())` 里 `before.get(path) != meta` 的（新出现 + size/mtime_ns 变了的；删掉的不出现） 10 `exit_code is None → -1`；输出 `decode("utf-8","replace")`。

### put_file / get_file / list_files
`put_file`：`require_file_path` → `_put_bytes(container, path, data, mode=0o644)`：① 容器内 `mkdir -p parent` ② 内存 tar（`TarInfo{name=basename, size, mode, uid=1000, gid=1000, mtime}`）③ `put_archive(parent, tar)`，falsy → `SandboxError("Docker 拒绝了这次 put_archive")`。`get_file`：`get_archive(path)` → 取第一个 `isfile()` member；NotFound → `SandboxFileNotFound`（多继承 FileNotFoundError 是有意的：worker 据此「跳过该产物」）。成功都 `touch`。`list_files`：复用 `_snapshot()`，`sorted(keys)`，不 touch。

### touch / release / reap_idle
`touch(unknown)` 静默 no-op；`release`：先 `_boxes.pop`（重复调用第二次不碰 docker），`containers.get + remove(force=True)`；NotFound → return（幂等）；`reap_idle(idle_sec)`：① 本进程记账的 `idle_for() >= idle_sec` ② 加 `_orphans()`：`containers.list(all=True, filters={"label": "aite.task"})`，跳过 known，取 `Created / State.StartedAt / State.FinishedAt` 最大值，`(now - max) >= idle_sec` 收；**三个时间戳都解析不出也收**；`DockerException` → `[]` ③ 去重后逐个 `release`，返回 id 列表。`_parse_docker_time`：RFC3339 纳秒，`0001-01-01` 当没发生过。非契约 `aclose()`：释放全部 box + `client.close()`。docker client 懒建（`from_env`），连不上 → `SandboxError("连不上 Docker daemon：…")`。

### workdir.py
`normalize_work_path(path, workdir="/work")`：非 str 或空 → `SandboxPathError`；不以 `/` 开头 → 拒；逐段归一（`.` 跳过、`..` 弹栈、栈空抛「路径越过了根目录」）；归一后必须 `== /work` 或在 `/work` 下；**不解析符号链接**。`require_file_path`：外加不能恰好等于 `/work`（"/work 是目录，不是文件"）。`EXEC_TIMEOUT_EXIT_CODE = 124`。错误层次：`SandboxError` → `SandboxPathError(…, ValueError)`、`SandboxNotFound`、`SandboxFileNotFound(…, FileNotFoundError)`。

### Dockerfile（`docker/sandbox/`，内容不变）
`python:3.11-slim`（不写 `--platform`）；apt `fontconfig fonts-wqy-microhei fonts-wqy-zenhei`；build 期断言 `fc-list :lang=zh` 非空、`command -v timeout`；pip 钉死 `pandas==2.2.3 matplotlib==3.9.2 openpyxl==3.1.5 python-docx==1.1.2 pillow==11.0.0`；`matplotlibrc` → `/etc/aite/matplotlibrc`（中文字体、`axes.unicode_minus: False`、Agg、dpi 110、bbox tight）；`useradd --uid 1000 aite`，`/work /work/in` chown；build 期自检 + 预热字体缓存；`CMD ["sleep","infinity"]`；无网络靠 runtime `network_mode=none`。

---

## 4. OpenAI-compat ModelPort（`aite/models/openai_compat.py`）→ Rust（R5）
请求体：`{"model", "messages": to_openai_messages(...), "max_tokens", "temperature"}` + 仅当 tools 非空：`"tools": [{"type":"function","function":{name,description,parameters}}]`、`"tool_choice": "auto"`（永远 auto）。`to_openai_messages`：assistant 带 tool_calls 时 `arguments` 用 `json.dumps(ensure_ascii=False)`（不转义中文）且 **content 写空串不写 null**（兼容国内网关）；`role=tool` 必须有 `tool_call_id` 否则 ValueError；`name` 有才带。
响应解析 `turn_from_response`：`choices` 空 → ValueError；只取 `[0]`；tool_calls 的 `arguments` 是 JSON 字符串 → `json.loads`，非 str → `dict()`，解析出来不是 dict 也算错；**解析失败不抛**：`args = {}`，`{call_id, error, raw}` 追加到 `raw["arg_parse_errors"]`（让它走 invalid_args 那条路）；`call_id = str(tc["id"]) or f"call_{i}"`；`finish_reason = choice.finish_reason or "stop"`；`raw = {"id","model"[, "arg_parse_errors"]}`，`chat()` 返回前 `raw["cost_cny"] = round(total_cost, 6)`；content null → `""`；tool_calls 空 list → None。
usage：`input_tokens = prompt_tokens or 0`、`output_tokens = completion_tokens or 0`、`cached_tokens = prompt_tokens_details.cached_tokens or 0`；缺 usage → 全 0。
**这一层不重试、不做纯文本兜底**（都归 worker）；不设 HTTP 超时（SDK 默认）；`ModelConfigError`：缺 base_url / model / 密钥变量（**消息只出现变量名不出现取值**）；`resolve_api_key(cfg, env)` 从 `cfg.api_key_env` 读，strip 后空则抛；懒建客户端；`cost_of(usage, cfg) = (in*price_in + out*price_out) / 1e6`；`total_cost` 累加、`last_usage`；`__repr__` 不泄露 key。
T12/T17/T19/T23 真模型（DeepSeek `https://api.deepseek.com/v1`，`deepseek-chat`）结论：严格守协议（0 协议外工具、0 schema 违规）；曾 0 次用 checklist → T19 改提示词后 31 次；「工具 ok=true 但内容空 → 模型原地重复」（33 次）→ T20 加重复检测；`08_step_limit` 在 live 下不稳定；T23 真沙箱三遍 delivered 各 7 步，`run_python` ~700ms，PNG ~40KB；模型自己取名 `monthly_trend.png`；唯一红的断言是 `gateway_result run_python contains "exit_code=0"`（耦合了 FakeGateway 的排版）。

---

## 5. 测试替身（`aite/testing/`）→ Rust `aite-testing`（R7）。原则：**只记账、不断言**。

- `recorder.py`：`Call{method, kwargs, result, error, seq}`，**`seq` 来自模块级全局计数器，跨替身单调**（能排出不同替身之间的先后）；`CallLog.record/of/count/methods/last/clear/calls/len/iter`。
- `FakePlatform`：`capabilities = FAKE_P0`（同 FEISHU_P0 除 `supports_passive_listen=True`、`outbound_rate_per_min=6000`）；可注入 `history` / `documents` / `files{(message_id,file_key): bytes}`；**假 id 单一共享计数器** `f"{prefix}-{n}"`（msg/card/file 跨类型递增）；`send_card` 返回 `SendResult(message_id=card_id, card_id=card_id)`；`cards: dict[card_id, [快照…]]`（`[0]` 是 send，其后每次 update 深拷贝追加）；三条刻意做严：`update_card` 只认已存在 card_id 否则抛 `FakePlatformError`；`read_history` 不过滤 sender_kind（按 thread 过滤 + created_at 正序 + `rows[-limit:]`）；`read_document`/`download_file` 查不到抛并列出已备键；`fail_next: dict[method, Exception]` 抛完即清；`emit(ev)` 测试侧注入（未 start 抛）；断言面 `count(method)`、`card_count`、`update_count`、`outbound_count`（send_text+send_card+update_card+send_file+add_reaction）、`texts()`、`card_snapshots()`；`history_message(...)` 辅助。
- `FakeModel`：`ScriptStep{text="", tool_calls=[ScriptedToolCall{name, arguments={}, call_id=None}], usage=Usage(), finish_reason=None(自动 tool_calls/stop), repeat=1|"inf", error=None, hold_ticks=0}`；**repeat 与 hold_ticks 正交**；`hold_ticks` 让出 event loop tick（不是墙钟；让满 N 次自动放行；`release_holds()` 可提前放行）；`call_id` 缺省 `f"call_{turns_served}_{j}"`；`raw = {"scripted_step": cursor, "served": turns_served}`；脚本用完 → `ScriptExhausted("模型脚本已用尽：共 N 步，这是第 M 次调用。给 model_script 补步骤，或给最后一步写 repeat: inf")`；记账 `calls/turns_served/holds/hold_ticks_yielded/holds_released`，断言面 `call_count`、`tool_names_emitted()`。
- `FakeSandbox`：内存 FS + 脚本化 exec：`ExecScriptStep{match=None, exit_code=0, stdout="", stderr="", duration_ms=5, truncated=False, writes={path: 内容}, error=None, times=None}`；`_match(code)` 顺序扫，`match is None` 匹配任何，否则 `match in code` 子串；`times` 用完跳过；**一条都不匹配 → `ExecResult(exit_code=0, stdout="", stderr="", duration_ms=1)`**（T17 死循环诱因）；`step.error` → 抛；`writes` 经 `as_bytes()`（bytes / `{"b64": …}` / `{"builtin": key}` 或 `"builtin:png"` / str utf-8）写进内存 FS 并进 files_out；`_check_path` 只要求 `startswith("/work")`；`release` 幂等，已 release 的 id 再用抛「沙箱 X 已经 release 过了」；时钟可注入；`sandbox_id = f"sb-{n}"`；断言面 `alive`、`all_files()`、`released_ids`。
- `samples.py`：`png_bytes(width=1, height=1, rgb=(255,255,255))` 手写真 PNG（magic + IHDR(8,2,0,0,0) + IDAT zlib + IEND，CRC32）；`PNG_1X1`；`CSV_SAMPLE = "month,amount\n2026-01,120\n2026-02,180\n2026-03,90\n"`；`BUILTINS = {"png": PNG_1X1, "csv": CSV_SAMPLE.encode()}`。
- `FakeToolGateway`：`expected_token=None` 时不校验 token，给了严格 `!=`；用 `jsonschema_mini`（不拒未知参数）；超时 `run_python` 用 `timeout_sec`，其余 60，无 grace；`TimeoutError` → timeout、`FakeSandboxError` → sandbox、其余 → upstream；content `[:12000]` 无截断标记；**各工具 content 排版与真实现不同**（场景断言耦合这一套，必须逐字保留）：history `"[id] name: text"` 逐行无表头；document `f"# {title}\n\n{text}"`（无来源行）；attachment `已下载到 /work/in/{file_key}（N 字节）`（未清洗）；run_python `exit_code=0\n--- stdout ---\n...`；list_files `"\n".join(files)` 或 `"(空)"`；data 也不同（history `{count, dropped}`）；必须实现 `sandbox_id_of` / `release_task`；断言面 `count(name)`、`results_of(name)`、`calls`、`results`。
- `FakeSessionStore`：内存 dict；`append_turn` 重复 seq 抛；`next_task_no` 原子 + `encode_task_no`；`seen_event` 首次 False；`list_active_tasks` 只回 created/planning/working 且 join session 的 chat_id；**存取全走深拷贝**。`FakeEvidenceWriter`：hash 链真算；`verify` 检查 seq 连续、prev_hash 链接、payload 未篡改；`finalize` 返回最后一条 hash（空链 GENESIS）。
- `jsonschema_mini.py`：`type/properties/required/items/minItems/maxItems/minimum/maximum/enum/default`；bool 排除；不拒未知参数；错误消息带路径（`arguments.limit`、`arguments.items[0]`）。

---

## 6. 评测 runner（`aite/evals/`）→ `aite-evals`（R7）

### 场景 YAML schema（`scenario.py`；`evals/p0/*.yaml` 原文件不动）
`Scenario`：`name`（必须等于文件名 stem）、`title`、`verifies`、`spec_ref`、`config`（覆盖 AiteConfig 片段，实际 `AiteConfig.model_validate({"platform": "fake", **config})`）、`platform: PlatformFixture{history[], documents[], files[]}`、`sandbox: SandboxFixture{exec_script[]}`、`model_script: [ScriptStep]`、`events: [EventSpec]`、`expect: [dict]`、`timeout_sec=10.0`、`source`。
`EventSpec`：`event_id`(必填) / `kind`(message) / `text`("") / `raw_text` / `mentioned`(True) / `sender_id`("ou_alice") / `sender_kind`(human) / `sender_name`("Alice") / `chat_id`("oc_demo") / `chat_type`("group") / `workspace_id`("cli_fake_app") / `tenant_id`("default") / `message_id`(None → `f"om_{event_id}"`) / `thread_id` / `task_no` / `attachments: [AttachmentSpec{kind=file, file_key, name, size, mime}]` / `card_action` / `at_sec`(None → 下标) / **`after`**("none" | "idle" | "running") / **`after_timeout_sec`**(5.0)。`after` 三档（C-T5T6-1）：`none` 紧接上一条投；`idle` 等系统静默（复用 `settle()` 判据）；`running` 等最近建的任务离开 `created`；**等不到就以 `phase="dispatch"` 失败收场**。
`HistorySpec{message_id, text, sender_id="ou_someone", sender_kind="human", sender_name(None→sender_id), thread_id, at_sec}`（时间 `BASE_TIME + (-600 + at_sec or index)` 秒）；`DocumentSpec{key, title, text="", url(None→key)}`；`FileSpec{message_id, file_key, content}`（`as_bytes`）；`BASE_TIME = 2026-09-09 09:00:00 UTC`。

### runner 流程（`_execute`）
1 `build_deps(sc, model, sandbox_kind)` 2 `build_control_plane(deps)`（运行时发现或注入 `plane_factory`；Rust 版：`PlaneFactory` 注入）3 `store.init()` + `platform.start(plane.handle_event)`（失败 → phase `wiring`）4 `start_loop(plane)`：`run_forever()` 挂后台，让两个 tick，立刻带异常收场 → phase `drive` 5 逐条投事件：`after != none` 先 `wait_before_dispatch`，再 `platform.emit(ev)`（抛 → `dispatch`）6 `settle(plane, deps, loop_task, timeout_sec)`：先找 `drain/run_until_idle/process_pending/run_once` 任一存在就调；否则轮询（5ms）等「所有替身 `len(calls)` 之和不变 且 `not busy(deps)`」持续 150ms → `"quiesce"`；超时 `"timeout"`；`busy = ModelProbe.in_flight / SandboxProbe.in_flight 非 0，或上一发模型抛了且还有任务未落终态` 7 `stop_loop` 8 `timeout` → phase `drive` 9 `run_checks(deps, expect)` → `assert` / `ok` 10 `deps.aclose()`。phase ∈ `wiring/dispatch/drive/assert/ok/error`；**每个场景报人话原因不报异常栈**（`--traceback` 时栈只进 stderr）。

### checks（比较子 `equals/min/max`，至少一个；返回 None=过，str=原因）
`platform_calls{method}`、`outbound_total`、`cards{distinct_equals/updates_min/updates_equals/final_status}`、`text{where: any|last|all, contains/not_contains/matches}`、`distinct_matches{source: last_text|all_texts, pattern}`（数不重复的正则命中）、`file{index=0, magic/name_suffix/mime/min_size}`、`gateway_calls{name}`、`gateway_result{name, index=-1, ok/contains/not_contains/error_code}`、`model_tools{name}`（含本地工具与 final）、`model_calls`、`sandbox_calls{method}`、`store{sessions_*/tasks_*/task_sessions_*/seen_events_*}`、`task{which: last|first|any|all, status}`、`evidence{verified, 比较子}`。未知 check / 畸形条目 / CheckError → 失败行不抛。

### CLI（`python -m aite.evals run <suite>` → `aite evals run <suite>`）
`--platform`（只有 fake）、`--model scripted|live`、`--sandbox fake|docker`、`--list`（`json.dumps([names], indent=2)` 退出 0）、`--only NAME`（可多次；未知名 → 一行人话 + 退出 2）、`--json PATH`、`--traceback`、`--config`（默认 config/aite.yaml，live 用）、`--protocol-report [PATH]`（摘要 stderr；给 PATH 另存 JSON）、`--timeout-scale K`（scripted 1.0 / live 12.0；同时乘 `timeout_sec` 与每个事件的 `after_timeout_sec`，只在内存里改）。
输出：stdout `json.dumps(payload, ensure_ascii=False, indent=2)` + **最后一行 `passed k/n`**；`SuiteResult{suite, platform, model, contract_version, total, passed, failed, scenarios[]}`；`ScenarioResult{name, passed, phase, duration_ms[, reason, failures, settled_by, stats, protocol]}`；`stats{platform_calls, model_calls, gateway_calls, sandbox_calls, sessions, tasks}`。退出码：全过 0；有失败 1；参数/环境问题（场景加载失败、未知场景、live 起不来、docker 体检不过、runner 自己炸）2。**不开 `--protocol-report` 时 stdout 一个字段都不多；scripted 路径 stderr 为空。**

### protocol_probe（T12）
`ModelProbe` 透明包装 ModelPort：补 `calls/call_count`；`in_flight`（settle 的静默判据看不见长 await）；`awaiting_retry`（配合「还有任务未落终态」算忙，不写死退避时长）；每次 chat 记 `Observation{index, run, attempt, n_messages, delta, elapsed_ms, gap_ms, error, text, finish_reason, tool_calls, usage}`（记账在调用前）；逐张牌 `in_protocol` + `schema_ok`；切「轮」两条判据缺一不可（前缀指纹相同 **且** 第 N 条是上次的 assistant 回牌；长度相同且上一发抛了 → 重试同轮）；`analyze(deps)` 输出 `{model, chat_attempts, chat_ok, runs[], reached_final, tool_names, unknown_tools, schema_violations, repeat_loops(MIN_REPEAT_RUN=3，故意不进 fallbacks), fallbacks{text_only_as_final, text_only_nudge, invalid_args, model_retry, not_found, sandbox_errors}, tasks, hit_max_steps, outbound_texts}`；`_fallback_text_only` 判据照抄 worker 的 `step_index == 0 and text`；`render_digest` 写 stderr。

### real_stack（T23）
`--sandbox docker`：真 DockerSandbox + 真 P0ToolGateway 各套一层探针（`SandboxProbe`/`GatewayProbe`，方法与参数名跟 FakeSandbox 记的逐字对齐）；平台仍 FakePlatform；`session_token` 走 `token_resolver_of(store)`（同步、直接读 store 的 dict 不走方法，否则记账永不静默）；`docker_preflight(images)`：SDK → `from_env` → `ping` → 每个 image `images.get`，起飞前一行人话；`build_docker_stack` 不碰 daemon；`scenarios_with_exec_script`：docker 档下 `exec_script` 一律忽略但 stderr 点名。Rust 版：真 `EdgeSandbox`（gRPC 到 edge）+ 真 `P0ToolGateway`，探针同理。

## 7. 10 个场景一览

| 场景 | events | model_script | checks |
|---|---|---|---|
| 01_simple_qa | e1 @ "今天北京天气怎么样" | `final` | `add_reaction>=1`、`send_text==1`、`send_card==0`、`update_card==0`、`cards distinct==0`、`text last contains 北京今天晴`、`store sessions==1 tasks==1`、`task last delivered` |
| 02_thread_followup | e1 @；e2 同 thread(`om_e1`)、`mentioned:false`、`after: idle` | 两个 `final` | `sessions==1 tasks==2 task_sessions==1`、`send_text==2`、`text last contains 季度`、`task all delivered` |
| 03_checklist_progress | e1 @ "把上个月的对账跑一遍"；`card_update_min_interval_ms: 0` | `checklist_add([3项])` → `checklist_check` ×3 → `final` | `send_card==1`、`cards distinct==1 updates>=3 final_status=delivered`、`send_text==1`、`model_tools checklist_add==1`、`checklist_check>=3`、delivered |
| 04_csv_to_chart | e1 @ 带附件 `file_sales_csv`（`builtin:csv`）；`exec_script: match savefig → writes /work/out.png = builtin:png`；interval 0 | `checklist_add` → `download_attachment` → `run_python(含 savefig)` → `final(artifacts=[/work/out.png])` | `gateway_calls download_attachment>=1`、`gateway_result download_attachment ok contains /work/in/`、`gateway_calls run_python>=1`、`gateway_result run_python ok contains "exit_code=0"`、`send_file==1`、`file[0] magic 89504e470d0a1a0a`、`send_text==1`、delivered |
| 05_history_summary | e1 @ "汇总本群本周开放事项"；history 5 条混 bot(om_h2)/app(om_h4) | `read_group_history(limit:50)` → `final(引用 om_h1/h3/h5)` | `gateway_calls read_group_history>=1`、`gateway_result ok not_contains 【机器人播报】`、`contains [om_h1]`、`distinct_matches last_text /om_h[0-9]+/ >=3`、`send_text==1`、delivered |
| 06_read_document | e1 @ 带文档 URL；documents 备 `Q3 交付计划` | `read_document` → `final` | `gateway_calls read_document>=1`、`gateway_result ok contains Q3 交付计划`、`platform_calls read_document>=1`、`text last contains Q3 交付计划`、`send_text==1`、delivered |
| 07_commands | e1 @；e2 `!status` `after: running`；e3 `!stop #A1` `after: running`；`max_steps: 8`、interval 0；`exec_script: match "while"` | ① `run_python(while True…, timeout_sec:5)` ② `hold_ticks: 200` + `checklist_note` + `repeat: inf` | `text any contains #A1`、`task any cancelled`、`sandbox_calls release>=1`、`cards final_status=cancelled`、`store tasks==1`、`cards distinct==1`、`sandbox_calls acquire>=1` |
| 08_step_limit | e1 @；`max_steps: 3`、interval 0 | `checklist_note` + `repeat: inf` | `model_calls>=3`、`task last failed`、`text any contains 上限`、`send_card==1`、`cards distinct==1 final_status=failed` |
| 09_bot_ignored | e1 `sender_kind: bot`、`mentioned: true` | 空脚本 | `outbound_total==0`、`model_calls==0`、`store sessions==0 tasks==0`、`add_reaction==0` |
| 10_duplicate_event | e1 投两次（同 event_id） | `final` | `store tasks==1 sessions==1`、`send_text==1`、`model_calls==1`、delivered |

## 8. scripts
- `check.sh`：已改为 Rust/Go 口径（R0）。
- `preflight.py`（1211 行）→ `aite preflight`（RΩ）：七组检查（1 配置可加载 —— yaml 解析得出来**且 `worker.system_prompt_path` 指到的文件真的在**（X1 补，读不到是 FAIL；病史：旧配置指着 2026-09-12 删掉的 Python 树时，改前七组一组都不碰它，preflight 报「可以起飞」而 `aite run` 退出码 2） 2 环境变量齐——只报在不在不打取值 3 飞书凭证有效 4 机器人身份对上（`/open-apis/bot/v3/info`，`bot` 在顶层不在 `data`）5 模型端点通（`ping`、`max_tokens=16`）6 沙箱可用（daemon、镜像、起容器四个 import，**跑完必须收掉容器**）7 落盘目录可写）；`--config`（不存在退到 example）、`--offline`（只跑 1/2/7）、`--json`、`--chat-id`；`FEISHU_TIMEOUT_SEC=20`、`MODEL_TIMEOUT_SEC=60`；状态 ok/fail/warn/skip，任一 FAIL → 退出 1，一项失败不阻断；**红线：绝不打印密钥取值**（`Redactor` 兜底替换四个变量值 + token，短于 4 字符不替换）；`Note{name, status: verified|unverified|unverifiable, detail}`。
- `evidence_show.py`（841 行）→ `aite evidence show`（R3）：`task_id`（可选）、`--dir`、`--root`（默认 config 的 evidence_dir）、`--config`（取 evidence_dir 与单价）、`--list`、`--only kind1,kind2`、`--tail N`、`--json`；退出码 0 链过 / 1 链断或 manifest 对不上或 payload 缺 / 2 找不到任务或参数错；`--list` 表格「最后写入 / 任务 / 事件数 / 终态 / manifest / 链」按 mtime 倒序，JSON `{root, tasks: [{task_id, dir, mtime, events, terminal, finalized, chain_ok}]}`，任一链断非零退出；单任务 JSON `{task_id, dir, finalized, manifest, root_hash, chain: {ok, checked, issues, manifest_problems}, summary, events}`；文本形态逐 kind 一行人话（checklist id→文本跨事件记）；三条纪律：链校验是骨头（断在第几条、期望/实际）；没有 manifest 也能渲染（"未 finalize"，ok 不受影响）；不打印密钥/token/消息全文（白名单字段；工具参数键名撞 `SECRET_HINTS`（token/secret/password/passwd/api_key/apikey/credential/auth）→ `***`；长参数截断）。
- `demo_fixture.py`（292 行）→ `aite evals demo-fixture`（R7）：子命令 `csv`（`--start 2024-09`、`--months 24`、`--seed 20260910`、`-o /tmp/aite-demo/sales.csv`）/ `history`（`--count`、`-o /tmp/aite-demo/history.txt`）/ `all`（`-d`、…）；同参两次字节一致；列名 `month,amount`；季节性 + 增长 + 一处塌陷让最高/最低月唯一；默认落 `/tmp/aite-demo/` 不往 `data/` 写；垫场文本一个 @ 都没有。

## 9. Makefile / CI / compose / pyproject
R0 已改写 Makefile / check.sh / CI。compose（RΩ）：现为单 service `python:3.12-slim` + `./:/app` + `/var/run/docker.sock` + 四个环境变量名 + `restart: unless-stopped` + `stop_grace_period: 20s` + 单副本；Rust/Go 版改成 `core` + `edge` 两个 service 共享 `data/run` 卷与 docker.sock，沙箱仍是兄弟容器。pyproject 冻结依赖（RΩ 删除）。

## 10. 测试（def test_ 数与关键断言）
- `tests/gateway`（76）：`test_gateway_catalog.py`（5：catalog 逐字、新 list、不依赖 ctx、名字集合 == TOOL_IMPLS 键集合、五个名钉死）；`test_gateway_denied.py`（6：正确 token 过；前缀被拒；尾随空格被拒；大小写翻转被拒；**非 ASCII 伪 token 是 denied 不是 upstream**；resolver 抛不外泄）；`test_gateway_errors.py`（25：六码触发；run_python 超时两条路（容器内 124、外层预算）；预算跟请求走；意外异常 → upstream；CancelledError 外抛；执行顺序三条（错 token 调不存在工具 → denied；不存在工具 + 乱参数 → not_found；参数不合 schema 时工具压根不执行）；截断恰好 12000 且以 `[内容已截断]` 结尾；错误消息也截断；call_id/name 回显 + duration>=0）；`test_gateway_schema.py`（17）；`test_gateway_tools.py`（23：history 丢非真人、W1 行格式、limit 透传、thread_only、无 thread 诚实回答、全 bot 措辞；document markdown / 未知 ref upstream；attachment 落 `/work/in/`、用 ctx 的 message_id、`../../etc/passwd` → `/work/in/passwd`、无附件上下文 upstream；run_python 在沙箱执行、files_out 变 artifacts、用户代码报错不是失败、会 touch、truncated；list_files 不建沙箱；一 task 一沙箱且复用 / 不同 task 不同沙箱；release_task 同时丢沙箱和 token；token_resolver 可替代 registry）。conftest 私有替身保留真实现两条脾气：read_history 不过滤、put_file 走 require_file_path。
- `tests/sandbox`（23，整份自动打 `docker` mark，daemon 不在或镜像没 build 整份 skip）：`test_docker_sandbox.py`（18：标签；network none；镜像不存在被拒；exec 返回；files_out 只报本次；用户代码报错不是沙箱失败；超 20000 截断；超时容器内强制；put/get 往返；put 拒 /work 外；get 缺失抛 FileNotFoundError；list_files；未知 id 被拒；release 幂等；touch 推后 reaper；reap 不动新鲜的；捡另一实例的孤儿；B4）；`test_sandbox_workdir.py`（5）。
- `tests/e2e`（222）：`test_t4_fake_platform.py`（14）、`test_t4_fake_model.py`（20，含 hold_ticks 七条）、`test_t4_fake_sandbox.py`（12）、`test_t4_fake_store.py`（12）、`test_t4_fake_gateway.py`（18）、`test_t4_model_openai_compat.py`（22）、`test_t4_evals_runner.py`（22：人话原因；摘要可序列化；wiring 失败归因；坏 factory 被接住；handle_event 失败报 dispatch；永不静默超时；**DemoPlane 把 01/02/09/10 跑绿**；去重/忽略 bot；失败断言点名差距；未知 check 等是报不是抛；CLI 七条）、`test_t4_evals_scenarios.py`（19）、`test_t4_dispatch_timing.py`（6）+ `test_t5_event_timing.py`（11）、`test_t12_protocol_probe.py`（41，用真 control + 真 worker，只换模型；RΩ 接）、`test_t23_sandbox_lane.py`（25，两条带 docker mark；RΩ 接）。
- `tests/tools`（52）：`test_demo_fixture.py`（20）、`test_evidence_show.py`（32：用真 writer 写 tmp 目录；渲染 + 校验；篡改被抓并指出断点；截断一行导致下一条断链；manifest root_hash 对不上；垃圾行不崩；未 finalize 退出 0；外部 payload 跟随/缺失降级；`--only/--tail` 只影响显示；`--json`；`--list` 最新在前并标损坏；缺任务退出 2；`--list` 与 `--dir` 同给被拒；疑似密钥 `***`；长参数截断；只有 hash 的字段不变文本；坏 manifest 不误报未 finalize；负 `--tail` 被拒；根目录不存在说明；`--tail 0`）。
- `tests/scripts`（25）：`test_preflight.py`。

## 11. 非显然的行为
1 `compare_digest` 先 encode；2 `run_python` 外层预算 = timeout + 5s；3 超时判定 `124 or (137 and elapsed >= timeout)`，成立改写 124 并追加 stderr；4 用户代码写 `/tmp` 不写 `/work`；5 `files_out` 比 `[size, mtime_ns]` 相等性；6 `_check_ready` 不过就 release；7 三个时间戳都解析不出也收；8 `touch/release(unknown)` 静默；9 `_orphans` 的 list 抛 → `[]`；10 `list_files` 不建沙箱；11 `download_attachment` 两段两种码；12 `file_key` 清洗只在真实现；13 token 校验失败关闭，登记点只能在任务开跑那一刻；14 `token_resolver_of` 不走 `store.get_task()`（记账会让 settle 永不静默）；15 worker 取产物先问 `sandbox_id_of`；16 `settle` 看不见长 await（`in_flight` 是 live/docker 能跑的前提）；17 `model_busy` 第二条件不写死退避时长；18 探针切轮两条判据；19 `_fallback_text_only` 不能只看下一步 delta；20 `repeat_loops` 不进 fallbacks；21 `hold_ticks` 让出 tick 不是墙钟；22 FakeSandbox 未匹配返回无害默认值；23 FakePlatform 假 id 单一计数器；24 `CallLog.seq` 全局；25 docker 档 `exec_script` 忽略但点名；26 场景断言耦合 FakeGateway 排版；27 不开 `--protocol-report` 时 stdout 逐字节不变；28 默认路径是恒等变换、新功能 opt-in；29 `sandbox/__init__` 只 re-export 不依赖 docker 的面；30 `from docker.client import DockerClient`（仓库根 `docker/` 目录遮蔽）；31 scripts 把仓库根顶到 `sys.path[0]`；32 `Redactor` 短于 4 字符不替换；33 「未 finalize」不算损坏，坏 manifest 也算 finalized；34 `/open-apis/bot/v3/info` 的 `bot` 在顶层；35 `03/04/07/08` 把 `card_update_min_interval_ms` 调 0；36 `07` 的 `max_steps: 8` 只是兜底；37 Dockerfile build 期断言；38 `axes.unicode_minus: False`；39 uid 1000 三处一致；40 `--model live` 起飞前就 `_ensure_client()`（否则每场景白重试 7 秒）。

## 各目录 def test_ 总数
`tests/gateway` 76 · `tests/sandbox` 23 · `tests/e2e` 222 · `tests/tools` 52 · `tests/scripts` 25 · 合计 398。
