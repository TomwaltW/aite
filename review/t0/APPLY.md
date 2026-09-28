# APPLY：总管落地契约 p1.0（H11）

> 起草：T0 云端轨（2026-09-28）。脚本：`review/t0/p1-contract-patch.py`；数据：`review/t0/data/`；规格：`docs/p1/contract-p1.md`（先审它再打）。
> **每条「期望」都抄自自测日志的实测**（T0 回执 `review/p1/ledger/T0.md` §4 附全文；`--check` 那一段是同一份脚本在
> 「B0 + AA4 + BB2 + BB4」副本上的实测），**只有两处例外**：第 6 步的 `edge/gen` 文件头（抄自已提交的 `edge/gen` 与 `CLAUDE.md` 工具链一节——
> 云端 protoc 31.x 生成的头本来就不同）和第 12 步 `git status --short` 的 32 行（按补丁文件清单推算）；第 9、10 步的输出格式抄自
> `core/crates/app/src/lock.rs`（`:128` 写 `wrote {n} files -> .contracts.lock`、`:155` 写 `OK {n} files`）——自测副本从不重锁，量不到。

## 0. 前提与纪律

- 在 `~/Documents/Projects/Aite`，`main` 上：**W1 已合完、P0-CLOSE（H6：AA4 + BB2 + BB4 + 一次重锁）已推到 main**，`scripts/check.sh` 全绿、
  `core/target/debug/aite contracts lock --check` → `OK 25 files`。你已经审过 `docs/p1/contract-p1.md`（尤其 §8.6 的五个 trait、§14 的缺口处置、§15 的审稿重点）。
- 这几行在**普通终端**里敲（总计划 §8 开头：带重锁变量的命令别用 Claude 会话的 `!` 前缀，会经过守卫 hook 被拦）。
- 每条命令 `cd` 与命令写在同一行。
- **工作区其余部分按预期编不过、`scripts/check.sh` 按预期红，直到 T0c 合并**：`contract/p1.0` 上只看本文列的几条判据。分支上 CI 不跑（CI 只在 PR / 推 main / 手动触发时跑，T0c 开 PR 之后才有）。
- 任何一步与「期望」对不上 → 停，别往下走。尤其第 4 步报 `命中 0 条` = P0-CLOSE 的实际落盘与脚本预料的不同 → 停，另开一个云会话按设计文档在新 sha 上重锚（脚本 + data 是可再生的，锚点逐字放在 `data/<键>.old.txt`）。

## 1. 编补丁前的二进制（打完补丁后工作区编不过，所以必须先编）

```bash
cd ~/Documents/Projects/Aite && git checkout main && git pull --ff-only && git status --short && (cd core && cargo build -p aite) && cp core/target/debug/aite /tmp/aite-prelock && /tmp/aite-prelock contracts lock --check
```

期望：`git status --short` 无输出；`cargo build` exit 0；最后一行 `OK 25 files`。
（`lock.rs` 只按磁盘上 `proto/aite/v1/**` 与 `core/crates/contracts/**` 的字节算哈希，所以补丁前的二进制重锁出来的结果是对的；自测第 c / k 步就是这个用法。）

## 2. 开分支

```bash
cd ~/Documents/Projects/Aite && git checkout -b contract/p1.0
```

## 3. 未授权时脚本必须拒（顺手验一下门是关着的）

```bash
cd ~/Documents/Projects/Aite && python3 review/t0/p1-contract-patch.py --check; echo "exit=$?"
```

期望（实测原文）：

```
拒绝改真仓库：这份补丁改的是冻结面（锁定面 + 守卫前缀面），故意只让人跑 —— 真树要总管的重锁授权。
  人跑：见 review/t0/APPLY.md（普通终端，别用 Claude 会话的 ! 前缀）
  自验：python3 review/t0/p1-contract-patch.py --root <仓库副本> --check
exit=2
```

## 4. 授权干跑

```bash
cd ~/Documents/Projects/Aite && AITE_RELOCK=1 python3 review/t0/p1-contract-patch.py --check; echo "exit=$?"
```

期望：85 行 `[ n/85 命中 1 条] <文件>: <说明>`（第 1–84 行）+ `[85/85 新建 1 个] domain.rs: 新文件 domain.rs：P1 领域类型与 Services`，**没有任何一行是 `命中 0 条` / `命中 2 条` / `已存在`**；然后（实测原文）：

```
[  合法] 5 份 .proto：protoc 一起解析通过
[  合法] 18 个 .rs：rustfmt 认，且已是 rustfmt 形态
[  合法] server.go：gofmt 认

--check：没写盘。这一份会动 25 个文件（其中新建 1 个）、85 处：
  config/aite.example.yaml（6 处）
  core/crates/contracts/src/capabilities.rs（1 处）
  core/crates/contracts/src/config.rs（7 处）
  core/crates/contracts/src/errors.rs（1 处）
  core/crates/contracts/src/events.rs（4 处）
  core/crates/contracts/src/gateway.rs（2 处）
  core/crates/contracts/src/lib.rs（4 处）
  core/crates/contracts/src/outbound.rs（5 处）
  core/crates/contracts/src/ports.rs（7 处）
  core/crates/contracts/src/protocol.rs（2 处）
  core/crates/contracts/src/sandbox.rs（2 处）
  core/crates/contracts/src/session.rs（3 处）
  core/crates/contracts/tests/config.rs（1 处）
  core/crates/contracts/tests/frozen_values.rs（1 处）
  core/crates/contracts/tests/layout.rs（1 处）
  core/crates/contracts/tests/roundtrip.rs（8 处）
  core/crates/proto/src/convert.rs（8 处）
  core/crates/proto/tests/convert.rs（5 处）
  edge/internal/server/server.go（1 处）
  proto/aite/v1/capabilities.proto（1 处）
  proto/aite/v1/edge.proto（4 处）
  proto/aite/v1/events.proto（4 处）
  proto/aite/v1/outbound.proto（4 处）
  proto/aite/v1/sandbox.proto（2 处）
  core/crates/contracts/src/domain.rs（新建）
exit=0
```

（「85 处」= 84 处锚点替换 + 1 个新文件。）

## 5. 真跑

```bash
cd ~/Documents/Projects/Aite && AITE_RELOCK=1 python3 review/t0/p1-contract-patch.py; echo "exit=$?"
```

期望：同第 4 步的 85 行命中 + 3 行合法，然后（实测原文）：

```
写了 25 个文件（新建 1 个）
[读回 OK] 84 处新内容全在盘上、旧锚点 0 条；新文件 1 个内容一致

接着必须跑（每步期望见 review/t0/APPLY.md）：make proto-gen → 两个 crate 的测试 → 用补丁前的二进制重锁。
exit=0
```

再跑一次必须拒（幂等；可选）：`cd ~/Documents/Projects/Aite && AITE_RELOCK=1 python3 review/t0/p1-contract-patch.py; echo "exit=$?"`
→ 末尾 `这份补丁已经打过了（每一处的新内容都在、旧锚点一条不剩）。不用再跑；一个字节都没写。` 与 `exit=1`。

## 6. codegen

```bash
cd ~/Documents/Projects/Aite && make proto-gen && git status --short edge/gen && git diff edge/gen | grep -E '^[-+]//.*protoc ' ; echo "grep-exit=$?"
```

期望：`git status --short edge/gen` 恰好 6 行（自测第 i 步实测的文件集）：

```
 M edge/gen/aitepb/capabilities.pb.go
 M edge/gen/aitepb/edge.pb.go
 M edge/gen/aitepb/edge_grpc.pb.go
 M edge/gen/aitepb/events.pb.go
 M edge/gen/aitepb/outbound.pb.go
 M edge/gen/aitepb/sandbox.pb.go
```

文件头仍是 `protoc v7.36.1`：最后那个 grep **无输出**、`grep-exit=1`（有输出 = 本机工具链不是 libprotoc 36.1 + protoc-gen-go v1.36.12 + protoc-gen-go-grpc 1.6.2，停下核工具链；
这条抄自已提交的 `edge/gen` 文件头与 `CLAUDE.md`，云端自测量不到——云端 protoc 31.x 生成的头是另一个版本号）。

## 7. 契约测试（两个 crate 分开跑、分开计数；口径 = check.sh C1 的 awk）

```bash
cd ~/Documents/Projects/Aite/core && cargo test -p aite-contracts 2>&1 | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print "contracts passed=" p " failed=" f; exit (f>0)}'
cd ~/Documents/Projects/Aite/core && cargo test -p aite-proto 2>&1 | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print "proto passed=" p " failed=" f; exit (f>0)}'
```

期望（自测第 g 步实测）：`contracts passed=42 failed=0`（= P0-CLOSE 后的 27 + K=15）与 `proto passed=10 failed=0`（= 5 + 5）。

K=15 条新契约测试：`frozen_values.rs` 的 `open_task_statuses_are_frozen`、`p1_enum_string_forms`、`capability_profiles_p1`；
`config.rs` 的 `dingtalk_and_wecom_platforms_accepted`、`p1_defaults_match_spec`、`p1_sections_reject_unknown_keys_and_bad_values`；
`roundtrip.rs` 的 `p0_json_without_p1_fields_still_loads`、`p1_event_shapes_round_trip`、`p1_outbound_shapes_round_trip`、`sandbox_egress_shapes`、
`domain_types_round_trip`、`network_event_reads_edge_jsonl_line`、`services_default_and_debug`、`model_error_p1_variants`、`old_impls_compile_and_new_methods_are_unimplemented`。
5 条新 proto 测试（`core/crates/proto/tests/convert.rs`）：`p1_enum_values_map_both_ways`、`p1_event_shapes_round_trip_through_pb`、
`p1_outbound_messages_round_trip_through_pb`、`capability_profiles_round_trip_through_pb`、`network_levels_and_egress_policy`。
原有的 `constants`（现钉 `p1.0`）、`rejects_bad_shapes`（现拒 `slack`）照样在、照样绿。

## 8. 重锁前看清楚锁在抱怨谁

```bash
cd ~/Documents/Projects/Aite && /tmp/aite-prelock contracts lock --check; echo "exit=$?"
```

期望：`MISMATCH 21 file(s):`（stderr）、`exit=1`，**按路径集合**恰好是下面 21 条（20 条 changed + 1 条 added）；每条 changed 后面跟的
`    locked <hash>` / `    actual <hash>` 两行不要求逐字（与自测副本不同：你的锁已经过 P0-CLOSE 重锁）：

```
  changed  core/crates/contracts/src/capabilities.rs
  changed  core/crates/contracts/src/config.rs
  added    core/crates/contracts/src/domain.rs（新文件未入锁）
  changed  core/crates/contracts/src/errors.rs
  changed  core/crates/contracts/src/events.rs
  changed  core/crates/contracts/src/gateway.rs
  changed  core/crates/contracts/src/lib.rs
  changed  core/crates/contracts/src/outbound.rs
  changed  core/crates/contracts/src/ports.rs
  changed  core/crates/contracts/src/protocol.rs
  changed  core/crates/contracts/src/sandbox.rs
  changed  core/crates/contracts/src/session.rs
  changed  core/crates/contracts/tests/config.rs
  changed  core/crates/contracts/tests/frozen_values.rs
  changed  core/crates/contracts/tests/layout.rs
  changed  core/crates/contracts/tests/roundtrip.rs
  changed  proto/aite/v1/capabilities.proto
  changed  proto/aite/v1/edge.proto
  changed  proto/aite/v1/events.proto
  changed  proto/aite/v1/outbound.proto
  changed  proto/aite/v1/sandbox.proto
```

与自测的差异（自测第 k 步实测是 `MISMATCH 23 file(s):`）：自测副本是「情形 A」——P0-CLOSE 是在副本里现套的、**没重锁**，所以 BB2 独有的两个锁面文件
`core/crates/contracts/src/evidence.rs`、`core/crates/contracts/tests/evidence_vectors.rs` 也出现在那 23 条里（P0-CLOSE 的另两个锁面文件
`lib.rs`、`edge.proto` T0 也改，两边重叠）。你的 main 已经在 H6 重锁过它们，所以**你这里只见 T0 的 21 条**；
**那两个 evidence 文件出现在清单里就停**（说明 H6 的重锁没落在 main 上）。

## 9. 重锁

```bash
cd ~/Documents/Projects/Aite && AITE_RELOCK=1 /tmp/aite-prelock contracts lock --write
```

期望：`wrote 26 files -> .contracts.lock`（25 + 新的 `domain.rs`）。

## 10. 复核锁与 Go 侧

```bash
cd ~/Documents/Projects/Aite && /tmp/aite-prelock contracts lock --check && (cd edge && go build ./... && go vet ./...); echo "exit=$?"
```

期望：`OK 26 files`，`exit=0`（go build / vet 自测第 j 步实测 exit 0）。**不跑 `go test`**：`edge/cmd/aite-edge/main_test.go` 还钉着 `p0.2`，归 T0c。

## 11. 暂存并核对

```bash
cd ~/Documents/Projects/Aite && git add -A proto core/crates/contracts core/crates/proto edge config .contracts.lock && git status --short
```

期望：恰好 **32 行**（未跟踪的 `??` 行不算）：

- `A  core/crates/contracts/src/domain.rs` —— **没有这一行就停**（`commit -a` 带不上新文件；漏了它 `contract/p1.0` 上 contracts crate 就编不过，T0c 和之后各波全从坏分支起步）；
- 24 行 `M `：第 4 步清单里除 `domain.rs` 外的 24 个文件；
- 6 行 `M  edge/gen/aitepb/…`：第 6 步那 6 个；
- 1 行 `M  .contracts.lock`。

## 12. 提交、推送、回 main

```bash
cd ~/Documents/Projects/Aite && git commit -m "contract(p1.0): T0" && git push origin contract/p1.0 && git checkout main
```

（与总计划 §8 H11 第 4 行是同一串命令，只是把 `git status --short` 拆到上一步，好在提交前核对 32 行。）

## 13. 之后

- H12：按总计划 §5.3 生成 T0c 派单，**提交到 `contract/p1.0` 并推送**，派 T0c。T0c 的起点估计见 `review/t0/companion-todo.md`，交接清单见 `docs/p1/contract-p1.md` §13。
- `main` 不动：`contract/p1.0` 由 T0c 的 PR 带进 main（PR 里带着你的契约提交），合并后删 `contract/p1.0`。
