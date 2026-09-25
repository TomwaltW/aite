# CC1 回执：云端基建 · 基线 · 基建 R0 主人 · 镜像构建参数 · 骨架 crate

会话分支 `claude/intelligent-curie-nmk7nb` · 2026-09-25

## 1. 开场自检原文

- check.sh：**没全部通过**——末行 `以下没过：B 全量 cargo test`，`cargo passed=890 failed=7`（基线 897/0）；其余各行与情形 A 逐字一致（`OK 25 files`、`contracts passed=25 failed=0`、`passed 10/10`、Go 9 包全 ok）。7 条失败全是「只读目录 / 只读库文件」类测试，根因：云端会话以 **root（uid 0）** 跑，root 无视 `chmod 0500 / 0400`，写入照样成功（逐条见 1.4）。
- 守卫：Read `.claude/hooks/guard_bash.py` 被拦（原文：`blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。`）
- 工具链：`libprotoc 31.1` / `rustc 1.98.1 (48a229cea 2026-09-01)` / `go version go1.27.1 linux/amd64` / `protoc-gen-go v1.36.12` / `protoc-gen-go-grpc 1.6.2`
- 情形：A，B0=`c159d12`
- 可达性：deb.debian.org exit=0 · security.debian.org exit=0 · auth.docker.io exit=0 · production.cloudflare.docker.com exit=0 · pypi.org exit=0 · goproxy.cn exit=56（`CONNECT tunnel failed, response 403`）

**闸门结论：第 4 步没对上（失败原因是环境的运行身份，不是代码回归），按派单 §4「自检没过也照样推，写清哪一步没对上，然后停」——推完这一节就停，等总管定夺。**

### 1.1 第 1 步 代码基线

```
$ git log --oneline -3
c159d12 docs(p1): 修订 §4.1 云端安装脚本（setup 约 5 分钟上限、GitHub release 403 回退；依据见 PR #1 回执第 10 节）
18b75fc docs(p1): 对齐 Claude Tag 总计划 + 第 1 波派单 + 仓库卫生
98e4460 merge(task-bb6): 并入 main
$ git cat-file -e 98e4460     # 在（clone 不浅，没 fetch）
$ git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'
（空，exit 0）
```

→ 情形 A，B0 = `c159d12`。

### 1.2 第 2 步 守卫

Read 工具读 `.claude/hooks/guard_bash.py`：

```
PreToolUse:Read hook error: [d=$(git rev-parse --show-toplevel 2>/dev/null); [ -f "$d/.claude/hooks/guard_bash.py" ] || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"]: blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。
```

→ 被拦 = 通过（这一次是期望结果）。

### 1.3 第 3 步 工具链 + 环境

```
libprotoc 31.1
rustc 1.98.1 (48a229cea 2026-09-01)
go: downloading go1.27.1 (linux/amd64)        # GOTOOLCHAIN=auto 首次拉 1.27.1
go version go1.27.1 linux/amd64
protoc-gen-go v1.36.12
protoc-gen-go-grpc 1.6.2
```

PATH 不用补 `$HOME/.cargo/bin`（`rustc` 直接可用）。

环境（不判红）：

```
uname -m   x86_64
nproc      4
free -g    Mem: total 15  used 0  free 13  buff/cache 2  available 15；Swap 0
df -h .    /dev/vda  252G  8.6G  29G  23% /
docker info --format '{{.ServerVersion}}'
  failed to connect to the docker API at unix:///var/run/docker.sock; check if the path is correct and if the daemon is running: dial unix /var/run/docker.sock: connect: no such file or directory
id -u      0
环境变量   GOPROXY=（空） GOTOOLCHAIN=auto
```

可达性（原样）：

```
deb.debian.org exit=0
security.debian.org exit=0
auth.docker.io exit=0
production.cloudflare.docker.com exit=0
pypi.org exit=0
curl: (56) CONNECT tunnel failed, response 403
goproxy.cn exit=56
```

会话首编是否冷编的证据（check.sh 之前取）：

```
$ pwd; ls -d core/target/debug; du -sh ~/.cargo/registry
/home/user/aite
ls: cannot access 'core/target/debug': No such file or directory
du: cannot access '/root/.cargo/registry': No such file or directory
```

→ target 与 cargo registry 都不在这份 checkout / 这个 HOME 里；A1 那格有 `Compiling` 行、`Finished … in 1m 46s`（分钟级）= **冷编**。

### 1.4 第 4 步 `scripts/check.sh`（冷启动，后台跑，日志全文）

命令：`{ time scripts/check.sh; } > /tmp/cc1-check-open.log 2>&1; echo "exit=$?" >> /tmp/cc1-check-open.log`

```

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
   Compiling rustls-platform-verifier v0.7.0
   Compiling hyper-rustls v0.27.9
   Compiling reqwest v0.13.5
   Compiling jsonschema v0.56.0
   Compiling aite-models v0.0.1 (/home/user/aite/core/crates/models)
   Compiling aite-gateway v0.0.1 (/home/user/aite/core/crates/gateway)
   Compiling aite v0.0.1 (/home/user/aite/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 46s
-> exit 0

=== A2 go build ./... ===
$ bash -c cd edge && go build ./...
go: downloading golang.org/x/sys v0.47.0
go: downloading github.com/felixge/httpsnoop v1.0.4
go: downloading go.opentelemetry.io/otel/metric v1.46.0
go: downloading github.com/go-logr/logr v1.4.4
go: downloading golang.org/x/text v0.41.0
go: downloading github.com/go-logr/stdr v1.2.2
go: downloading go.opentelemetry.io/auto/sdk v1.2.1
go: downloading github.com/cespare/xxhash/v2 v2.3.0
-> exit 0

=== A3/C2 契约锁 --check ===
$ core/target/debug/aite contracts lock --check
OK 25 files
-> exit 0

=== A4a cargo clippy -D warnings ===
$ bash -c cd core && cargo clippy --workspace --all-targets -- -D warnings
    Checking rustls-platform-verifier v0.7.0
    Checking hyper-rustls v0.27.9
    Checking reqwest v0.13.5
    Checking jsonschema v0.56.0
    Checking aite-models v0.0.1 (/home/user/aite/core/crates/models)
    Checking aite-gateway v0.0.1 (/home/user/aite/core/crates/gateway)
    Checking aite v0.0.1 (/home/user/aite/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 05s
-> exit 0

=== A4b cargo fmt --check ===
$ bash -c cd core && cargo fmt --check

-> exit 0

=== A4c go vet ===
$ bash -c cd edge && go vet ./...

-> exit 0

=== A4d gofmt ===
$ bash -c cd edge && test -z "$(gofmt -l .)"

-> exit 0

=== A5 cargo test --no-run（全部测试可编译） ===
$ bash -c cd core && cargo test --workspace --no-run
  Executable tests/test_context.rs (target/debug/deps/test_context-0c9eaf634dbf20e3)
  Executable tests/test_final.rs (target/debug/deps/test_final-6e70a09cb9f97835)
  Executable tests/test_in_flight.rs (target/debug/deps/test_in_flight-9cf74d9da741dd8b)
  Executable tests/test_limits.rs (target/debug/deps/test_limits-37cdb5e9a29c39a5)
  Executable tests/test_loop_fallbacks.rs (target/debug/deps/test_loop_fallbacks-4493b78700529fd8)
  Executable tests/test_prompts_checklist.rs (target/debug/deps/test_prompts_checklist-e9d7c93d76b8d9d7)
  Executable tests/test_sandbox_handoff.rs (target/debug/deps/test_sandbox_handoff-0384a5fa7bf2bfc7)
  Executable tests/test_steer.rs (target/debug/deps/test_steer-0e71edf6a13bb207)
-> exit 0

=== C1 契约测试 ===
$ bash -c cd core && cargo test -p aite-contracts 2>&1 | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"contracts passed=\" p \" failed=\" f; exit (f>0)}"
contracts passed=25 failed=0
-> exit 0

=== B 全量 cargo test ===
$ bash -c cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); printf "%s\n" "$o" | grep -E "^error: test failed" | sort -u | head -n 5; printf "%s\n" "$o" | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p \" failed=\" f; exit (f>0)}"
error: test failed, to rerun pass `-p aite --lib`
error: test failed, to rerun pass `-p aite --test cli_smoke`
error: test failed, to rerun pass `-p aite --test crash_recovery`
error: test failed, to rerun pass `-p aite --test preflight_e2e`
error: test failed, to rerun pass `-p aite-store --test crash_recovery`
cargo passed=890 failed=7
-> exit 1  ✗

=== B 全量 go test（-race） ===
$ bash -c cd edge && go test -race ./... -count=1
?   	aite/edge/gen/aitepb	[no test files]
ok  	aite/edge/internal/aiteerr	1.023s
ok  	aite/edge/internal/config	1.035s
ok  	aite/edge/internal/feishu	4.876s
ok  	aite/edge/internal/ingress	1.346s
?   	aite/edge/internal/pin	[no test files]
ok  	aite/edge/internal/sandbox	1.036s
ok  	aite/edge/internal/server	1.044s
-> exit 0

=== B8 评测（passed 10/10） ===
$ bash -c o=$(core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | grep -qx "passed 10/10"
}
passed 10/10
-> exit 0

以下没过：B 全量 cargo test

real	7m30.350s
user	18m25.955s
sys	4m31.430s
exit=1
```

冷启动墙钟：**`real 7m30.350s`**。

Go 那一格被截掉的包单跑：

```
$ (cd edge && go test -race ./cmd/... -count=1)
ok  	aite/edge/cmd/aite-edge	5.906s
$ (cd edge && go test -race ./... -count=1 2>&1) | grep -cE '^(ok|\?)'
9
```

### 1.5 七条失败逐条单跑（不是抖动：确定性复现，单跑同样红）

| 测试 | 断言位置 | 共同形状 |
|---|---|---|
| `aite --lib` `preflight::tests::check_storage_fails_and_says_which_layer_is_stuck` | `crates/app/src/preflight.rs:3704` | 把目录 / 库文件改成只读，期望写失败 |
| `aite --lib` `preflight::tests::the_readonly_probe_never_writes_anything` | `crates/app/src/preflight.rs:3216` | 同上 |
| `aite --lib` `preflight::tests::the_db_probe_also_asks_whether_it_can_write` | `crates/app/src/preflight.rs:3255` | 同上 |
| `aite --test cli_smoke` `preflight_exits_one_when_the_database_file_is_readonly` | `crates/app/tests/cli_smoke.rs:394`（`退出码必须是 1`） | 同上 |
| `aite --test crash_recovery` `store_failure_does_not_kill_the_process` | `crates/app/tests/crash_recovery.rs:268` | 同上 |
| `aite --test preflight_e2e` `a_readonly_database_fails_the_first_row_and_still_leaves_the_seventh_green` | `crates/app/tests/preflight_e2e.rs:1199` | 同上 |
| `aite-store --test crash_recovery` `store_failure_surfaces_as_an_exception_not_a_silent_false` | `crates/store/tests/crash_recovery.rs:199` | 同上 |

代表性原文（`aite-store`）：

```
thread 'store_failure_surfaces_as_an_exception_not_a_silent_false' (12762) panicked at crates/store/tests/crash_recovery.rs:199:5:
只读目录下 seen_event 必须 Err，实际 Ok(false)
```

测试体（`crates/store/tests/crash_recovery.rs:190-199`）：`set_permissions(&dir, 0o500)` 之后期望 SQLite 写失败——而 `id -u` = `0`，root 无视权限位，写成功了。
7 条每个都是「把东西 chmod 成只读 → 断言写不进去」，与本机 / CI（非 root）不同的只有运行身份。

### 1.6 这一步怎么解（我的判断，未动手，等总管定）

- 这不是代码回归（情形 A，代码与 `98e4460` 一致；CI 与本机非 root 都是 897/0），是**云端环境以 root 跑测试**。
- 可选解法（都不在我这一节里做）：
  1. 云端基线就按 `890/7` 记，派单里点名这 7 条为「root 下必红」——最省事，但每轨自检都得自己扣这 7 条，容易把真回归混进去；
  2. check.sh（CC1 可写面）在 `id -u` = 0 时给「B 全量 cargo test」换一个非 root 身份跑（例如 `setpriv` / `runuser` 到一个普通用户，target 目录权限要跟着处理）——改动面在我可写面内，但改变了闸门本身的口径，需要总管点头；
  3. 让这 7 条测试在 root 下自己跳过（改的是 `crates/app`、`crates/store` 的测试，不在 CC1 可写面，要派给别轨）。
- 建议：2（在 check.sh 里降权跑，外加 CLAUDE.md「云端基线」写明）或 3；1 不推荐。
