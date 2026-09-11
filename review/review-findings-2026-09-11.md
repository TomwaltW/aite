# 七轨合流审核台账（R1–R7 → main）

> 审核时间：2026-09-11　基线 `c9d96d2`　审核方式：七个独立 agent 逐轨对照 Python 源码读代码（不看测试结果）
> 机器验收（整合树）：cargo 605 passed / go 8 包 ok / docker tag 22 条 ok / clippy·fmt·vet·gofmt 干净 / 锁 `OK 36 files` / 评测 `passed 0/10`（RΩ 未接线，预期）
>
> 分级：**阻断** = 不修不能合；**应修** = 本次合流一并修；**记账** = 交 RΩ 或后续轨，已在此登记。

---

## 一、已修（本次合流的总管提交）

| # | 轨 | 位置 | 问题 | 为什么要修 |
|---|---|---|---|---|
| F1 | R2 | `edge/internal/aiteerr/errors.go`、`sandbox/docker.go:328,353` | **`file_not_found:` 前缀被 kind token 顶掉**，拼出来是 `sandbox_not_found: file_not_found: …`，core 侧按开头判前缀，`FileNotFound` 永远走不到 | 阻断。worker 靠这条把「产物少一个」（跳过该产物照常交付）与「沙箱挂了」分开。**根因是我派单写错了**（paste-R2.md §邻居 让他们用 `SandboxNotFound` 配前缀），R2 发现了并在测试注释里留了话，但没权改 R0 的文件 |
| F2 | R3 | `core/crates/evidence/src/writer.rs:473,489` | **串行锁取在 `spawn_blocking` 外面**。future 在 `.await` 被取消时 tokio guard 立刻释放，而阻塞闭包不可取消、还在跑 → 下一个 append 与它真并发 → 两路读到同一个 tip、写出同一个 seq → 文件以换行收尾故 heal 不碰 → `verify` 永久 false | 移植新引入的数据损坏。同仓库 `SqliteSessionStore::with_conn` 是对的写法（锁在闭包内），照它改。RΩ 收尾序列里的 `runner.cancel()` 就是现成的取消点 |
| F3 | R3 | `core/crates/evidence/src/cli.rs:721` | `read_to_string(..).unwrap_or_default()` 让非法 UTF-8 的 events.jsonl 被当成空文件 → 报「0 条 · 链 OK」退出 0，而 `verify` 对同一文件判 false | 崩溃现场（写到一半被杀、残行截在多字节字符中间）正好命中，而这个子命令存在的全部理由就是那时候说真话 |
| F4 | R0 | `core/crates/app/tests/cli_smoke.rs` | R0 时期的断言「所有子命令都是 not implemented」在 R3/R7 落地后过期 | 改写成按各子命令当下该有的样子分别钉住，顺带把评测的退出码 / `passed 0/10` / stderr 零字节 / 失败原因是人话做成硬断言 |

---

## 二、交 RΩ 的能力缺口（需要产品决定，不是代码缺陷）

**G1 · 卡片按钮在 Go 版彻底失效**（R1 发现，我独立复核）

- lark-oapi-go v3.12.0 的 `ws/client_message.go:79` 把非 `event` 帧整条丢弃；唯一的 `WithCardHandler` 钩子在 `ws/client.go:56` **是注释掉的**；`MessageTypeCard` 常量定义了但全 SDK 无人引用。
- Python 靠 monkeypatch 私有方法 `_handle_data_frame` 绕过（T16 的 `route_card_frames_as_events`），Go 没有这条路。
- **影响边界（已核）**：`!stop` / `!status` 走普通消息事件，完全不受影响；评测 `07_commands` 用的正是文本命令，B8 不受阻；`docs/acceptance-M.md` 的 M1–M6 没有一条依赖按钮。**实际损失只是卡片上那两个按钮点了没反应。**
- **衍生问题**：`cards.go` 照常渲染这两个按钮 —— 用户会看到点不动的按钮，比不显示更糟。
- 选项：①自定义 WebSocket dialer 在字节流上改帧头（脏，要自己解分帧）②fork/vendor 一份 ws 包打补丁 ③卡片回传改走 HTTP webhook ④先不渲染按钮，等上游修。
- **建议**：短期取 ④（一行改动，不骗用户），中期视飞书 SDK 更新取 ③。

---

## 三、记账给 RΩ / 后续轨

### R1 飞书 adapter
- `connection.go:197-204` 建连失败分支漏 `cancel()`，`platform.go:248` 失败分支不 `safeClose` → 每失败一次在长生命周期 ctx 上挂一个永不摘除的 cancelCtx（断网一夜数千个）。`go vet -lostcancel` 抓不到。**已修**（见四）
- `platform.go:216` `Capabilities()` 注释写「副本」实返本体，`SetPassiveListen` 无锁写，而 R2 的 gRPC server 并发读 → `-race` 下真 data race。**已修**（见四）
- `platform.go:363,427` `msg == nil` 时裸字段访问会 nil panic；`sink == nil`（合法构造）时 `dispatchRaw` 同理。**已修**（见四）
- `card_frames_test.go:332-337` 一条断言方向写反（`bare.Do` 永远返回 nil error），是全轨唯一的空洞断言，注释也与 SDK 源码不符。**已修**（见四）
- `gorilla/websocket` 成了直接依赖但仍挂 `// indirect`、未进 `pin.go`。**已修**（见四）
- 建议：mention 循环里每条消息每个 mention 编译一次正则；`cards.go:90` 状态兜底会把 `CARD_STATUS_UNSPECIFIED` 渲染给用户看；`Platform` 无 `Stop()`，`http.Client` 不回收。

### R2 沙箱 / 守护进程
- `docker.go` 多处 daemon 中途不可达报 `SandboxInternal` → `INTERNAL` → core 侧 `is_retryable` 为 false。契约冻结原文是「docker daemon 不可达 → UNAVAILABLE」。**已修**（见四）
- `workdir.go:58` workdir 带尾斜杠时所有 Put/GetFile 变 InvalidPath，且报「路径必须在 /work/ 下」这种自相矛盾的话。**已修**（见四）
- `ReapIdle` 单个 Release 失败会把**已经真删掉**的 id 随 error 丢成 nil → core 手上的 `task → sandbox_id` 不会被清，之后拿死 id 去 exec。
- `ingress/client.go` `Connected()` 落后于 RPC 成功（测试有 flake 风险）；首次连上被计成一次 reconnect（`!status` 会显示「重连 1 次」但其实从没连上过）；`Close()` 后再调 `HandleEvent` 会把连接和 watch goroutine 复活。
- `clip` / `diffFiles` / `parseDockerTime` / `orphans` 这些纯函数只在 `docker` tag 下被跑到，没有 daemon 的 CI 上等于裸奔。建议拆一个不带 tag 的 `docker_pure_test.go`。
- `Release("")` 报 Internal 而非幂等返回 nil（v28 SDK 空 id 走 `InvalidParameter` 不是 `NotFound`）。

### R3 存储 / 证据
- `store/src/lib.rs:81` 的 `stamp()` 与 Python `isoformat()` 格式不同（`...Z` 定长 6 位 vs `...+00:00` 变长）。**Rust 的写法是对的**（Python 变长那套在同一秒内字典序 ≠ 时间序，`find_session_by_thread` 的 ORDER BY 会挑错），但老 .db 直接接过来读有缝。
- `busy_timeout=5000` 设了但没有测试钉住（现成 `pragma()` helper 只能取文本列，取不了 INTEGER）。
- `ACTIVE_TASK_STATUSES` 手抄成三个 `?`，契约哪天加到 4 个会静默少一个状态。
- `find_session_by_thread` 排除 archived、多条取最新这两条语义一条断言都没有；`list_turns` 取「最近 N」而非「最早 N」没钉。
- `evidence show` 若干渲染分歧（`true/false` vs `True/False`、参数按键排序 vs 插入序、`--only ""` 的空白名单行为）。

### R4 控制面
- `plane.rs:784-791` / `:1056-1057` **「查 cancelled + 登记 running」与「抄 running + 落 cancelled」不在同一临界区**。Python 靠单事件循环白拿的原子性，Rust 多线程下拆成三次独立取锁 → 同一条证据链可能写两条 `cancelled`、manifest finalize 两遍。**已修**（见四）
- `cancel_task` 全路径吞错后仍继续收尾、仍回帖「已停止」→ 库里任务还是 `created`，下条 `!status` 继续列它；`events.dropped` 和 `ingress.errors` 全是 0，`_dropped_note` 那句进程级警告不会出现。注：`ControlPlane::cancel_task -> Task` 是冻结契约没有错误通道，「不能上抛」不是本轨的锅，但「写失败之后还接着写证据、还回帖说停了」是。**已修**（见四）
- `run_one` 的 `queue.task_done()` 不是 drop-safe（对照 Python 的 `finally`）；`FinishGuard::drop` 两次 remove 不在一个临界区。
- `initiator_of` 对非字符串 `initiator_name` 的退化与 Python 不同；`cancel_task` 的 `chat_id: Some("")` 不退回 `session.chat_id`。
- 未覆盖：上面两条竞态都造不出交错（用例全在 current_thread 上串行驱动）；`update_task`/`evidence.append`/`send_text` 在取消路径上失败的错误面零覆盖；`_resolve_stop_target` 第三条（裸 `!stop` 且活跃任务 0 或 ≥2 → 「没有这个任务」）**Python 也没测**，是继承的空洞，而它正是「多任务群里按停止没反应」的那个分支。

### R5 worker / 模型
- `models/src/lib.rs:430,437` HTTP 错误体与解析失败没走 `redact`，而同函数 423/429 两处走了。国内网关在 4xx 里回显 `Authorization` 不是没有过的事。**已修**（见四）
- `agent.rs:977` `final.artifacts` 是「假值但非 null」（`""` / `{}` / `0`）时 Python 放行（`or []`）、Rust 判 invalid_args 退回重来 —— 交付路径上的行为翻转，弱模型给 `artifacts: ""` 不罕见。
- `agent.rs:654` artifact `title` 回退用「字符串化后为空」而非 Python 的真值语义（`title: false` → Rust 得到 `"false"`）。
- `mime.rs` 4 个扩展名（`.md`/`.yaml`/`.yml`/`.gz`）与 `mimetypes.guess_type` 口径不同 —— Rust 更准，但与注释自述的「同口径」不符。
- T20 指纹的 5 组向量里没有一组含「2 个以上 key 的嵌套对象」，而「嵌套也排序」恰恰是与 Python 唯一的差异点。
- `models` 的 22 条测试全在 200 路径上，`FakeServer::start_raw` 写了没人调，非 2xx 分支零覆盖。

### R6 网关 / edge 客户端
- `gateway.rs:317` **`CatchPanic` 只包住工具 future**，`check_token`（含 resolver 闭包）、`validate_arguments`、`from_secs_f64` 都在保护圈外 → resolver panic 会打穿 `ToolGateway::call`，违反契约「永远不失败」。RΩ 要把 resolver 接到 SessionStore 上。**已修**（见四）
- `tests/tools.rs:581` **并发双检锁的测试是空跑**：`#[tokio::test]` 默认单线程 + 假沙箱 `acquire` 无挂起点 → 4 个 task 串行跑完，把锁整段删掉测试照样绿。**已修**（见四）
- `ingress.rs:71` `start()` 无条件删已存在的 socket，不区分「上一条命的残留」和「另一个 core 正在监听」→ 第二个实例静默抢走监听。**已修**（见四）
- `ingress.rs:132` 自己复制了一份 `invalid_event:` 前缀，派单明令禁止。**已修**（见四）
- `link.rs:68` `connected()` 探针拨通才置 true，进了退避 sleep（最长 30s）期间即使 RPC 已跑通也报 false —— RΩ 若拿它做健康行/起飞门禁会误判。**已修**（见四）
- `attachments.rs:21` `safe_name` 是手写复刻 pathlib+正则，只有一条测试，且已与 Python 分歧：`file_key` 以 `/.` 结尾时 pathlib 丢掉 `.` 分量、Rust 拿到 `"."` 折空 → `attachment`。风险低（file_key 不透明）但测试薄得不成比例。
- `link.rs:125` 后台探针 task 无主，`EdgeClient` drop 也不停。
- `DEFAULT_RUN_PYTHON_GRACE_SEC = 5.0` 没有任何测试钉住（两条超时测试都换成了 0.05/0.5，常量改成 0 一条都不会红）—— 而它正是「余量为 0 则外层先赢、模型永远拿不到超时前的部分输出」那条的唯一保障。
- `constant_time_eq` 语义对但缺编译屏障（建议 `black_box`，**不要**为此加 `subtle` 依赖）；`from_secs_f64` 负数会 panic；`jsonschema::validator_for` 每次 call 重编译；`max_message_mb = 0` 无兜底；`guess_mime` 对 `.png` 这种点开头文件与 Python 不同。
- 未使用依赖：`gateway` 的 `sha2`/`hex`/`tempfile`，`edge-client` 的 `tonic-prost`/`prost`/`thiserror`。

### R7 替身 / 评测
- `checks.rs:357` `hex_decode` 按**字节**切片，`magic` 里有多字节字符（全角 `"８９"`，6 字节、能过偶数长度那关）会在 char 边界 panic，而 runner 一路没有兜底 → 整套评测 exit 101，JSON 摘要与 `passed k/n` 一个字都出不来。**已修**（改用已在依赖表里的 `hex` crate）
- `runner.rs` **没有任何 panic 兜底**，`phase="error"` 是个测试白名单里写着、代码却永远产不出的死分支。R4 的真 plane 一个 unwrap 就能让 B8 的 10 条一起丢。**已修**
- `checks.rs:61` 非整数比较子（`equals: "1"` / `1.0`）被**静默跳过** → 断言凭空消失（Python 那边 `1 != "1"` 会报失败）。**已修**
- `checks.rs:308` `file` 的负 index 与 Python 相反，且与同文件的 `gateway_result` 自相矛盾。**已修**
- `regex_mini.rs` 五类静默错判，与它自己模块头写的「不支持的语法一律编译期报错」矛盾：`\b \B \A \Z` 当成普通字母；`.` 匹配 `\n`；`x$` 不匹配 `"x\n"`（YAML 块标量天然带尾换行）；带捕获组的 `find_all` 返回整段而非分组；组内量词不回溯。**当前 10 个场景打不到**（只有一处 `om_h[0-9]+`），但 `text.matches` 是对外 DSL 键。**整个 `src/` 零单元测试**。
- `real_stack.rs:320` docker 体检在 `wired=true` 时短路成 None —— RΩ 一注入 SandboxFactory，体检就彻底消失，回到「十个场景各自烂在第一个工具调用上」。
- `cli.rs:326` `--config` 解析了但全文件只出现在一句错误消息里；`ModelFactory` 零参数，RΩ 只能闭包捕获死路径，做不到 Python 那样「起飞前就 load_config + 建 client，失败一行人话 exit 2」。
- `SandboxFactory` 不含 `TokenResolver`，而契约写死「令牌校验失败关闭」→ RΩ 忘了接就是每个工具调用都 `denied`，报出来的是「denied」不是「你没接 token_resolver」；`build_docker_stack` 只在注释里存在。
- `demo_fixture` 扁平 flag 解析：`all -o /tmp/x.csv` 静默忽略 `-o`（用户以为落到指定路径，实际落在 `/tmp/aite-demo/`）；`csv --count 3` 等越界参数 Python 是 exit 2、Rust 是 exit 0 静默忽略。
- `fake_sandbox.rs:66` 手写 base64 不校验长度/padding，少一位也安静解出来；`writes: BTreeMap` 让 `files_out` 变字典序（Python 保插入序，多文件 `send_file` 顺序会不同，04 目前只写一个文件所以潜伏）。
- `cli.rs:119` `-h/--help` 走 stderr + exit 2（Python argparse 是 stdout + exit 0）；`--json` 写失败只 warn，破了「scripted 路径 stderr 为空」。
- `serde_json` 没开 `preserve_order` → 所有 JSON 输出按字母序而非插入序（与 Python 对拍会满屏 diff）；`f64` 的 Display 让三处文案变成 `x12` / `10s` / `5s`（Python 是 `x12.0` / `10.0s` / `5.0s`）。
- `protocol_probe` 的 `fallback_text_only`（Python 5 条，含 T17 拿真模型撞出来的回归）与 `retry_bursts`（2 条）**零覆盖**，而它们是纯函数、现成 helper 就能测。
- **14 类 check 里有 9 类在 Rust 侧没有直接断言**，只能靠场景间接跑到；而 DemoPlane 只覆盖 4 个场景 —— 换句话说没接真 ControlPlane 时这 9 类的代码路径在 CI 里一次都不会执行。上面那条 `hex_decode` 的 panic 正是因为这块没测才漏到现在。

**DemoPlane 复核结论**：不是空壳。全文件 grep 场景名只命中文档注释，代码区零命中；字符串字面量里没有任何 YAML 期望文本；用的是官方替身、实现的是契约 trait；09 按 `sender_kind` 字段判、10 靠 `seen_event` 的集合 test-and-set，都是真逻辑。两条要记的小账：02 验的是「同一 session + 两个 task」而非「历史喂进 prompt」（与 Python 版 DemoPlane 一致，但别误以为上下文续接已被评测覆盖）；`run_forever` 是 `yield_now` 忙等，RΩ 别照抄。

---

## 四、本次合流一并修掉的清单

取舍线：**panic / 数据损坏 / 契约违反 / 测试没测到它自称测的东西** —— 修；
行为细微分歧、性能、措辞 —— 记账（上面第三节）。

| # | 位置 | 改了什么 | 配套测试 |
|---|---|---|---|
| 1 | `edge/internal/aiteerr/errors.go` + `sandbox/docker.go` | 新增 `SandboxFileNotFound` kind（token `file_not_found`，映射 NotFound），两处 GetFile 改用它 | `errors_test.go` 新增「两种 NotFound 在线路上分得开」，`docker_test.go` 把 `t.Logf` 换成对 `st.Message()` 前缀的硬断言 |
| 2 | `core/crates/evidence/src/writer.rs` | 串行锁挪进 `Inner`、在 `spawn_blocking` **内部**取（照抄同仓库 `SqliteSessionStore::with_conn`） | 原 64 条全绿 |
| 3 | `core/crates/evidence/src/cli.rs` | 非法 UTF-8 不再 `unwrap_or_default()`，压一条 ChainIssue → `ok()` 变 false、退出码 1 | 原 32 条全绿 |
| 4 | `core/crates/app/tests/cli_smoke.rs` | 过期断言改写成按子命令分别钉住；新增评测退出码 / `passed 0/10` / stderr 零字节 / 原因是人话四条硬断言 | 3 → 6 条 |
| 5 | `core/crates/control/src/plane.rs` | 「查 cancelled + 登记 running」与「抄 running + 落 cancelled」做成同序临界区（cancelled → running） | 原 84 条全绿 |
| 6 | 同上 | `cancel_task` 落库失败时 `bump("events.dropped")` 后直接返回，不再接着写证据 / finalize / 回帖「已停止」 | 同上 |
| 7 | `core/crates/evals/src/checks.rs` | `hex_decode` 改用 `hex` crate（不再按字节切片 panic）；比较子非整数改报错不静默跳过；`file` 负 index 从尾部数 | 新增「全角 magic 不打穿」用例 |
| 8 | `core/crates/evals/src/runner.rs` | 整个场景执行 + 断言各包一层 `catch_unwind` → `phase="error"`（原来是死分支） | 新增「plane panic 收成 error 相」用例；29 → 31 条 |
| 9 | `core/crates/gateway/src/gateway.rs` | `CatchPanic` 从「只包工具 future」扩到整个 `dispatch`（含 token_resolver 闭包、schema 校验、算预算） | 原 82 条全绿 |
| 10 | `core/crates/gateway/tests/` | 并发双检锁那条用例换 `multi_thread` runtime + 假沙箱 `acquire` 加挂起点 | **拆掉锁实测变红（4 ≠ 1），还原变绿** —— 之前删掉锁照样过 |
| 11 | `core/crates/edge-client/src/ingress.rs` | socket 已有人监听时报错而不是抢占；`invalid_event:` 前缀改走 `status.rs` 的映射函数 | 原 9 条全绿 |
| 12 | `core/crates/edge-client/src/link.rs` + 三个调用面 | 新增 `note_ok()`，每次成功 RPC 都刷新连接态（不再等探针下一次拨号） | 原 15 条全绿 |
| 13 | `core/crates/models/src/lib.rs` | HTTP 非 2xx 响应体与 JSON 解析失败两处补 `redact` | **新增两条密钥回归测试；撤掉 redact 实测变红** |
| 14 | `edge/internal/feishu/connection.go` | 建连失败分支补 `cancel()`（原来每失败一次泄漏一个 cancelCtx） | 原用例全绿 |
| 15 | `edge/internal/feishu/platform.go` | `Capabilities()` 返回 `proto.Clone` 的副本、`capsMu` 读写锁护住；`SendText`/`SendFile`/`dispatchRaw` 三处补 nil 保护 | 同上 |
| 16 | `edge/internal/feishu/card_frames_test.go` | 方向写反的空断言换成真判据（两条路拿到的对象不同） | — |
| 17 | `edge/internal/pin/pin.go` | 收编 `gorilla/websocket`（已是直接依赖却还挂 indirect） | — |
| 18 | `edge/internal/sandbox/docker.go` | 新增 `dockerErr`，13 处 daemon 调用点在 `IsErrConnectionFailed` 时改判 `Unavailable`（契约冻结：daemon 不可达 → UNAVAILABLE） | — |
| 19 | `edge/internal/sandbox/workdir.go` | workdir 自身也归一化（尾斜杠不再让所有 Put/GetFile 变 InvalidPath） | 新增「workdir 带尾斜杠仍接受」用例 |

| 20 | `edge/internal/feishu/helpers_test.go` + `scripts/check.sh` | `route` 的读取侧（count/called/last/at）补锁，指回 `f.mu`；`-race` 写进 check.sh 的全量 go test | 合流后在 main 上跑 `-race` 抓到现行（`TestTransportErrorIsRetryable`），修后八个包全 ok |

> 第 20 条是审核**没抓准**的一条：R1 把它列成「建议」并判断「`-race` 实际抓不到」。
> 实际抓得到 —— 传输错误那条路上连接被掐断，服务端 goroutine 还在跑，断言就先读了。
> 这也是为什么把 `-race` 放进门禁比靠人记得跑一次可靠。

三处「拆掉修复验证测试会红」的实测（#10 / #13 以及 F2 的对照）是这次唯一能证明「新测试不是摆设」的手段，回执里保留了实际输出。
