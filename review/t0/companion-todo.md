# T0c 伴随清单起点（T0 在自测副本上的估计，不是代码）

> 生成：`python3 review/t0/p1-contract-patch.py --estimate --root <副本>`（副本 = B0 + P0-CLOSE + T0 补丁）。
> `cargo check --workspace --all-targets --keep-going` 的 error 按文件归并。**只是估计**：一个 crate 编不过时，
> 依赖它的 crate 不会被检查，所以下游 crate 的条目在上游修好之前看不见（见末尾两节）。
> 行号是副本上的（B0 = `8458435` + P0-CLOSE），T0c 起点若在更新的 main 上，按符号找。

共 28 条 error，13 个文件。

## `crates/control/src/card.rs`（1 条）

- `:61` E0063 missing fields `approval` and `links` in initializer of `ChecklistCard`: missing `approval` and `links`

## `crates/control/src/dispatch.rs`（3 条）

- `:89` E0063 missing fields `dedupe_key` and `mentions` in initializer of `OutboundText`: missing `dedupe_key` and `mentions`
- `:219` E0063 missing fields `cancelled_by` and `take_decision` in initializer of `RunHooks`: missing `cancelled_by` and `take_decision`
- `:223` E0308 mismatched types: expected `Vec<SteerMessage>`, found `Vec<String>`

## `crates/control/src/routing/mod.rs`（1 条）

- `:225` E0004 non-exhaustive patterns: `CardActionKind::Approve`, `CardActionKind::Reject` and `CardActionKind::Submit` not covered: patterns `CardActionKind::Approve`, `CardActionKind::Reject` and `CardActionKind::Submit` not covered

## `crates/control/src/sessions.rs`（3 条）

- `:66` E0063 missing field `meta` in initializer of `Session`: missing `meta`
- `:321` E0063 missing fields `message_id`, `provider_extra` and `sender_name` in initializer of `Turn`: missing `message_id`, `provider_extra` and `sender_name`
- `:354` E0063 missing fields `dedupe_key` and `mentions` in initializer of `OutboundText`: missing `dedupe_key` and `mentions`

## `crates/edge-client/src/sandbox.rs`（1 条）

- `:30` E0063 missing field `egress` in initializer of `AcquireRequest`: missing `egress`

## `crates/gateway/tests/common/mod.rs`（2 条）

- `:546` E0063 missing fields `deleted`, `reply_to` and `updated_at` in initializer of `HistoryMessage`: missing `deleted`, `reply_to` and `updated_at`
- `:639` E0063 missing fields `chat_type`, `initiator_external` and `initiator_id` in initializer of `ToolContext`: missing `chat_type`, `initiator_external` and `initiator_id`

## `crates/models/src/lib.rs`（2 条）

- `:114` E0063 missing field `cache_write_tokens` in initializer of `Usage`: missing `cache_write_tokens`
- `:190` E0063 missing field `provider_extra` in initializer of `aite_contracts::Message`: missing `provider_extra`

## `crates/store/tests/common/mod.rs`（2 条）

- `:23` E0063 missing field `meta` in initializer of `Session`: missing `meta`
- `:72` E0063 missing fields `message_id`, `provider_extra` and `sender_name` in initializer of `Turn`: missing `message_id`, `provider_extra` and `sender_name`

## `crates/testing/src/fake_model.rs`（1 条）

- `:358` E0063 missing field `provider_extra` in initializer of `aite_contracts::Message`: missing `provider_extra`

## `crates/testing/src/fake_platform.rs`（2 条）

- `:31` E0063 missing fields `card_action_deadline_ms`, `max_card_bytes`, `max_text_chars` and 11 other fields in initializer of `PlatformCapabilities`: missing `card_action_deadline_ms`, `max_card_bytes`, `max_text_chars` and 11 other fields
- `:492` E0063 missing fields `deleted`, `reply_to` and `updated_at` in initializer of `HistoryMessage`: missing `deleted`, `reply_to` and `updated_at`

## `crates/worker/src/card.rs`（1 条）

- `:76` E0063 missing fields `approval` and `links` in initializer of `ChecklistCard`: missing `approval` and `links`

## `crates/worker/src/deliver.rs`（3 条）

- `:35` E0063 missing fields `message_id`, `provider_extra` and `sender_name` in initializer of `Turn`: missing `message_id`, `provider_extra` and `sender_name`
- `:142` E0063 missing fields `dedupe_key` and `mentions` in initializer of `OutboundText`: missing `dedupe_key` and `mentions`
- `:178` E0063 missing fields `dedupe_key` and `mentions` in initializer of `OutboundText`: missing `dedupe_key` and `mentions`

## `crates/worker/src/loop.rs`（6 条）

- `:55` E0004 non-exhaustive patterns: `&ModelError::RateLimited { .. }`, `&ModelError::Auth(_)` and `&ModelError::BadRequest(_)` not covered: patterns `&ModelError::RateLimited { .. }`, `&ModelError::Auth(_)` and `&ModelError::BadRequest(_)` not covered
- `:213` E0308 mismatched types: expected `&str`, found `&SteerMessage`
- `:214` E0308 mismatched types: expected `&str`, found `&SteerMessage`
- `:215` E0308 `match` arms have incompatible types: expected `String`, found `SteerMessage`
- `:634` E0063 missing fields `chat_type`, `initiator_external` and `initiator_id` in initializer of `ToolContext`: missing `chat_type`, `initiator_external` and `initiator_id`
- `:791` E0063 missing field `provider_extra` in initializer of `aite_contracts::Message`: missing `provider_extra`

## 编译失败的 crate（cargo 报 could not compile）

- `aite-control`
- `aite-edge-client`
- `aite-gateway`
- `aite-models`
- `aite-store`
- `aite-testing`
- `aite-worker`

## 本估计看不到、但已知要改的（交接清单见 docs/p1/contract-p1.md §13）

- 依赖上面编不过的 crate 的（至少 `aite-evals`、`aite`〈app〉及其测试）没被检查：它们的结构体字面量与穷举 match 要等上游修好才报。
- `edge-client/tests/common/mod.rs` 的 fake `PlatformService`：tonic 生成的服务 trait 多了 12 个 RPC，没有默认实现就要补。
- 5 处 `p0.2` 字面量钉：`edge/cmd/aite-edge/main_test.go`、`core/crates/evidence/tests/manifest.rs`、`core/crates/evals/tests/evals_runner.rs`、`core/crates/evals/tests/protocol_probe.rs`、`core/crates/app/tests/cli_smoke.rs`（运行期才红，cargo check 看不到）。
- `RunHooks.drain_steer` 改成 `Vec<SteerMessage>`：control 入队处与 worker 消费处（上面 `dispatch.rs` / `loop.rs` 的 E0308）要一起改，
  入队时就把事件的 `sender_id` / `sender_name` / `message_id` 带上。
- `core/crates/app/src/lock.rs` 注释里的「= 25」→ 26。
- Go：`go build` / `go vet` 在补丁后已过（自测第 j 步）；`go test` 已知会红的是 `main_test.go` 的版本钉（T0 只另跑过 `internal/config` 与 `internal/server` 两个包，都绿；其余包没跑）。
