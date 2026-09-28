## T0：契约 p1.0 补丁起草（draft）

本轨只起草、不落地：补丁脚本 + 冻结规格，由总管在 W1 合并、P0-CLOSE 上 main 之后按 `review/t0/APPLY.md` 在 `contract/p1.0` 上打（H11）。
重锁命令见 `review/t0/APPLY.md`，这里不抄原文。**真树零代码改动**；diff 只在 `docs/p1/contract-p1.md`、`review/t0/**`、`review/p1/ledger/T0.md`。

### 交付

- `docs/p1/contract-p1.md`：p1.0 冻结规格（配置 13 段、事件 / 出站 / 能力位 / 出网、12 个新 RPC 与 Go 可选接口签名、`SessionStore` +14 默认方法、五个新 trait 方法集、domain.rs 32 个类型、op 登记表、开放通道、T0c 伴随清单、W1 缺口 ④–⑩ 全部进 p1.0）。
- `review/t0/p1-contract-patch.py` + `review/t0/data/`：84 处锚点替换 + 新建 `domain.rs`，共动 25 个文件（锁面 21 + 锁面外 4）；全有或全无、幂等闸门、protoc / rustfmt / gofmt 闸门；
  `--check` / `--root` / `--self-test` / `--codegen` / `--estimate`；真树未授权退出 2。
- `review/t0/APPLY.md`：H11 的 13 步人跑链，每步期望抄自自测实测。
- `review/t0/companion-todo.md`：`--estimate` 实跑的 T0c 起点（28 条 error / 13 个文件 / 7 个 crate）。
- `review/p1/ledger/T0.md`：回执；`review/t0/logs/`：日志原文。

### 实测

- 开场自检：情形 A；守卫 Read 被拦（通过）；工具链 protoc 31.1 / rustc 1.98.1 / go1.27.1 / protoc-gen-go v1.36.12 / protoc-gen-go-grpc 1.6.2。
- `--self-test`：a–l 全 PASS，`[T0 self-test] 全部通过`。contracts `27 → 42`（K = 15）、proto `5 → 10`（M' = 5）；4 处变异红 → 绿；
  codegen ok；副本 `go build` / `go vet` exit 0；补丁前二进制 `lock --check` → `MISMATCH 23 file(s):`（情形 A），按路径集合相等；总管真跑只见 T0 的 21 条。
- 验收 `scripts/check.sh` 与开场逐行相同（Δ = 0）：`cargo passed=946 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`go packages ok=9 fail=0`、`passed 10/10`、`B9 skip`。
- `--root <已打过的副本>` → exit 1「已经打过」；`--check`（真树）→ exit 2；`--codegen`（无 `--root`）→ exit 2。

### 请总管审

- 五个新 trait 的方法集是按 DD1–DD3 / EE / FF 的 goal 推出来的（原卡说的「契约先行草稿」没入库）。
- 超出原卡的：`MessageIndexEntry`、`SteerMessage`、`compute_snapshot_hash()`、`find_task_by_no` 按群限定、`DocDeleter` 单列。
- 回执 §6：两次守卫误拦（跨行引号、`'.*'` 通配）我改写法继续了，请复核这个判断。

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_015LnXdMAcLp4uxiiDQ56yk1
