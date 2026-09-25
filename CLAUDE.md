# Aite 仓库约定（给本机与 Claude Code 云端会话共用）

云端会话看不到总管本机的 `~/.claude/CLAUDE.md`、记忆与 skills —— 这份文件就是云端唯一能读到的
仓库级约定。派单（`review/paste-*.md`）与它冲突时以派单为准。

## 这是什么

Aite = 企业 IM 里的共享 AI 同事，对标 Anthropic 的 Claude Tag。core 是 Rust（`core/`，状态与判定），
edge 是 Go（`edge/`，飞书长连接 + Docker 沙箱），两边只通过 `proto/aite/v1/` 的 gRPC 契约说话。
P0 冻结规格：`docs/dev-spec-2026-09-11-rustgo.md`（Python 时代那份 `docs/dev-spec-2026-09-09.md`
里有 M1–M6 与飞书权限，也冻结）。对齐 Claude Tag 的总计划：`review/plan-2026-09-25-claude-tag-parity.md`。

## 语言

- 文档正文、代码注释、commit message、回执、派单：**中文**，跟随仓库现有写法。
- 用户可见文案（回帖、卡片）一律中文，改动要配测试（很多文案被 `tests/wording.rs` 逐字钉住）。

## 环境

- 没有 `python` 命令，一律 `python3`（守卫 hook 就是 `python3` 跑的）。
- Rust 工具链钉在 `core/rust-toolchain.toml`（1.98.1，带 clippy / rustfmt）；Go ≥ 1.27（云端环境变量 `GOTOOLCHAIN=auto`）。
- `protoc`：云端 31.x（`core/crates/proto/build.rs` 每次重编都要它）；本机 `make proto-gen` 必须用 libprotoc 36.1 +
  `protoc-gen-go v1.36.12` + `protoc-gen-go-grpc 1.6.2`（`edge/gen` 文件头钉 `protoc v7.36.1`），云端生成的 `edge/gen` 永远不提交。
- 云端环境的安装脚本见 `scripts/cloud-setup.sh`（CC1 落地前以总计划 §4.1 里那份为准）。
- `make proto-gen` 与重锁只在总管本机跑（要 `AITE_RELOCK=1`，守卫拦 agent），云端不跑。

## 守卫（`.claude/hooks/guard_bash.py`，云端单仓会话里照样生效）

受保护面：`proto/**`、`core/crates/contracts/**`、`.contracts.lock`、`.claude/**`、`edge/go.mod`、
`edge/go.sum`、`docs/dev-spec-*.md`；`AITE_RELOCK=1` 的赋值也拦。

- **被拦就停下、原文引用拦截信息写进回执，不许绕**（换写法绕过守卫 = 越界）。
- 要改受保护面：在 `review/` 下写补丁脚本（如 `review/t0/p1-contract-patch.py`；照 `review/aa4-proto-patch.py` 的形状：
  锚点恰好命中一次、全有或全无、`--check` 干跑、`--root` 在副本上自测、有效性闸门、逐步人跑命令链
  带期望输出），由总管在本机跑并重锁。
- 读受保护文件用 Read 工具（`.contracts.lock`、`edge/go.mod` 连 Read 都拦，那就别读，
  从 `core/crates/app/src/lock.rs`、`docs/dev-spec-2026-09-11-rustgo.md` 间接取信息）。
- 已知误拦与绕法：
  - ASCII 引号跨行（多行 `python3 -c "…"`、跨行的 heredoc 引号）→ 判「无法解析」。把脚本写成文件再跑。
  - 对受保护路径用 `find -exec` / `xargs` / `$(…)` → 判「覆盖面判不出」。换成不碰受保护面的写法，
    或者干脆别在命令里列那些路径。
  - `cat .claude/settings.json` 之类 → 判「读取位置」。不需要读它。

## 验收

- 一条命令：`scripts/check.sh`（= `make check`）。**别在它后面接 `| tail`**。
- 2026-09-25 在 `98e4460` 的实测基线：`cargo passed=897 failed=0`、`contracts passed=25 failed=0`、
  `OK 25 files`、`passed 10/10`、`go packages ok=9 fail=0`、`B9 skip：evals/p1 尚无场景`。
  CC1 起的新口径：Go 那一格不再逐包列 `ok`，先打失败包名（最多 5 个）、末行 `go packages ok=N fail=M`
  （N 含 `[no test files]`，以这行为准）；B9 跑 `evals/p1`，有场景要 `passed k/k`，没场景打 skip 不算红；
  `scripts/check.sh --docker`（可与 `--quick` 同给）另跑真容器那组，与 `make docker-test`、CI 的 `sandbox-docker` 同口径。
  以 root 跑时（云端会话就是 uid 0）两格全量测试自动经 `setpriv` 摘掉 `CAP_DAC_OVERRIDE` / `CAP_DAC_READ_SEARCH`，
  否则 7 条只读类测试必红（890/7）。
  P0-CLOSE（AA4 + BB2 + BB4，总管本机打）落地之后：`cargo passed=901`、`contracts passed=27`、`OK 25 files`（以实测为准）。
  之后每个同步点的新基线由对应轨写进下面「云端基线」一节。
- **B8 不变量**（`evals/p0` 钉死的，任何一轨弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条
  （所以 AIGC 标识之类必须放进同一条回复）；顶层 @ 恰好建 1 个 Task 会话；`!stop` 仍立即释放沙箱；对 bot 不加表情。
- 已知时序抖动（CPU 紧时会假红，先单跑一遍再下结论）：`aite --test graceful_shutdown`、
  `--test startup_recovery`、`--test reconnect_replay`、`aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、
  Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。
- 真容器那组：`docker build -t aite-sandbox:p0 docker/sandbox && (cd edge && go test -tags docker ./internal/sandbox/... -count=1)`，
  跑完 `docker ps -a --filter label=aite.task -q | wc -l` 必须是 0。
- `evals/p0/*.yaml` 是 P0 验收面，**不许改**；新行为的场景放 `evals/p1/`。

## 规则

- 不往 `main` 提交；一轨一个分支、一个 PR，PR 标题以轨号开头（如 `CC3: …`）。
- 只写派单列出的可写面；别的文件要改 → 写进回执「记账转出去的」，不动手。
- 每个行为改动配回归测试，并做一次变异验证（把改动撤回、测试必须红），把红的输出贴进回执。
- 格式化用 `rustfmt --edition 2024 <改过的文件>`，**别用 `cargo fmt --all`**（会改到别轨的文件）；
  Go 用 `gofmt -w <文件>`。
- 新的第三方依赖先停下问总管（Go 依赖还要人跑 `edge/go.mod` 补丁）。
- `cargo passed` 的增量要在回执里逐条列出来源（哪个文件加了哪几条）。
- 回执写到 `review/p1/ledger/<轨号>.md`（每轨一个文件，避免并行 PR 在同一个台账上冲突），PR 描述贴摘要；
  老台账 `review/review-findings-2026-09-12-vmerge.md` 只追加、不改旧节。
- 云端会话只能推自己那条 `claude/*` 分支；尽早开 **draft PR**（CI 只在 PR / 推 main / 手动触发时跑）。
- 云端命令里永远不出现：`AITE_RELOCK=1`、守卫点名的路径、包着受保护路径的 `$(…)`；云端 protoc 生成的 `edge/gen` 永远不提交。
- Docker 测试必须封闭：只连本地测试服务器，不连公网主机。
- 还原源码别用会保留旧 mtime 的拷贝（`cp -p` / `shutil.copy2`）：cargo 会跳过重编，变异验证会拿旧产物跑出假绿。

## 云端会话的三条操作纪律（派单没写也照做）

- **长命令放后台**：冷编的 `scripts/check.sh`、`bash scripts/cloud-setup.sh`、全量 `cargo test`、docker 那组都可能超过
  Bash 工具的 600 秒上限。一律用 `run_in_background` 跑，输出重定向到 `/tmp/<轨号>-<名字>.log`，末尾 `echo "exit=$?"` 追加进同一文件，
  跑完用 Read 工具读**全文**（不接 `| tail`）。
- **cwd 会跨调用保留**：带 `cd` 的命令一律写成子 shell——`(cd core && …)`、`(cd edge && …)`——每条都从仓库根起跑，
  否则下一条的 `scripts/check.sh` / `cd edge` 会在错的目录里失败。
- **rustfmt 在 `core/` 下跑**：工具链只由 `core/rust-toolchain.toml` 钉（云端 rustup 可能没有默认工具链），
  所以写成 `(cd core && rustfmt --edition 2024 crates/<…>.rs)`。
- 开场自检第 2 步（Read 守卫脚本）**被拦就是通过**，拦截原文里「停止当前工作」对这一步不适用；其它任何一次被拦都照「守卫」一节停下。
- 回执、PR 描述、多行提交信息一律先用 Write 工具落文件，再 `gh pr create --draft --body-file <文件>` / `git commit -F <文件>`；
  不走 heredoc、不写多行 `--body "…"`（守卫拒多行引号，而回执要原样引用的拦截文字里有受保护路径名）。

## 云端基线

| 基线 | sha | cargo / contracts / 锁 | go packages | B8 / B9 | 墙钟（冷 / 热） | 来源 |
|---|---|---|---|---|---|---|
| B0 | `c159d12`（D0 之后的 main） | `897/0` / `25/0` / `OK 25 files` | `ok=9 fail=0` | `passed 10/10` / skip | 5m58s / 42s（开场自检旧 check.sh 冷编 7m30s） | CC1 云端实测（2026-09-25，4 vCPU、uid 0；`reconnect_replay` 的 `a_root_and_its_thread_followup_replayed_together` 单跑约 1/5 会红，是 R7 的真竞态，见 runbook §7） |
| P0-CLOSE 之后 | — | `901` / `27` / `OK 25 files` | `ok=9 fail=0` | `passed 10/10` / skip | — | **计划值（§5.1），待总管实测** |

云端要点（详见 `docs/p1/cloud-runbook.md`）：会话是 root（check.sh 自动降权，单跑只读类测试要自己带 `setpriv` 前缀）；
每个会话都从冷编起跑；docker daemon 默认不在（`dockerd &` 手动起）；Docker Hub 匿名拉取会 429、出口 TLS 拦截代理让容器里的 HTTPS 验不过证书，
所以真容器那组在云端建不出沙箱镜像，以 CI 的 `sandbox-docker` 为准；一份 `core/target` 约 15G，会话磁盘额度装不下第二份。
CC1 的五个骨架 crate（admin / search / githost / routines / memory）已由补丁脚本 `review/p1/cc1-crates-patch.py` 打进主树（总管放开 `core/Cargo.toml` 写权限后在会话里跑）：cargo 条数不变（Δ=0），包数 12 → 17。
（T0c 合并后补 B1 行。）
