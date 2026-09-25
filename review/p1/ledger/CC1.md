# CC1 回执：云端基建 · 基线 · 基建 R0 主人 · 镜像构建参数 · 骨架 crate

会话分支 `claude/intelligent-curie-nmk7nb` · 2026-09-25

## 1. 开场自检原文

- check.sh：**没全部通过**——末行 `以下没过：B 全量 cargo test`，`cargo passed=890 failed=7`（基线 897/0）；其余各行与情形 A 逐字一致（`OK 25 files`、`contracts passed=25 failed=0`、`passed 10/10`、Go 9 包全 ok）。7 条失败全是「只读目录 / 只读库文件」类测试，根因：云端会话以 **root（uid 0）** 跑，root 无视 `chmod 0500 / 0400`，写入照样成功（逐条见 1.4）。
- 守卫：Read `.claude/hooks/guard_bash.py` 被拦（原文：`blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。`）
- 工具链：`libprotoc 31.1` / `rustc 1.98.1 (48a229cea 2026-09-01)` / `go version go1.27.1 linux/amd64` / `protoc-gen-go v1.36.12` / `protoc-gen-go-grpc 1.6.2`
- 情形：A，B0=`c159d12`
- 可达性：deb.debian.org exit=0 · security.debian.org exit=0 · auth.docker.io exit=0 · production.cloudflare.docker.com exit=0 · pypi.org exit=0 · goproxy.cn exit=56（`CONNECT tunnel failed, response 403`）

**闸门结论：第 4 步没对上（失败原因是环境的运行身份，不是代码回归），按派单 §4「自检没过也照样推，写清哪一步没对上，然后停」——推完这一节就停，等总管定夺。**

> **后续（总管在会话里拍板）**：选「check.sh 里降权跑」。工作项 2 里 check.sh 在 root 下给两格全量测试套 `setpriv` 摘掉
> `CAP_DAC_OVERRIDE` / `CAP_DAC_READ_SEARCH`，云端回到 `cargo passed=897 failed=0`、**全部通过**（见 1.7、第 4 节）。
> **给第 1 波其余各轨的闸门行**：情形 A 下 check.sh 各行 = `OK 25 files`、`contracts passed=25 failed=0`、`cargo passed=897 failed=0`、
> `go packages ok=9 fail=0`、`passed 10/10`、`B9 skip：evals/p1 尚无场景`——**前提是用本 PR 的 check.sh**；用 main 上的旧 check.sh 在云端必是 890/7。

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

### 1.7 闸门之后：root 的修法与上一次 CC1 回执

- 总管选了「check.sh 里降权跑」（会话里的问答）。实现是 `setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search --`，
  不换用户：target、`~/.cargo` 都归 root，换人就得跟着改权限。先在会话里验过可行：

  ```
  plain root: write OK                      # 0500 的目录，root 直接写得进
  touch: cannot touch '/tmp/tmp.TmFkf8ulEy/b': Permission denied
  dropped: write DENIED                     # 摘掉两个能力之后
  0                                         # 仍是 uid 0
  ```

  降权后单跑 5 个红的测试二进制：`52 passed` / `25 passed` / `4 passed` / `30 passed` / `7 passed`，全 `0 failed`。
- 上一次 CC1（PR #1，分支 `claude/determined-galileo-e2wlol`）停在第 3 步（没有 protoc），没走到能暴露 root 问题的那一步；
  它第 4 步末尾记的「B 全量 cargo test 编不过时假绿」在本轨工作项 2 里修了（在 check.sh 可写面内、就是那一格），绿 → 红 → 绿见 3.2。

## 2. 工作项逐条（改了哪些文件:行，行号以本分支 HEAD 为准）

| 工作项 | 文件 | 位置与内容 |
|---|---|---|
| 1 | `scripts/cloud-setup.sh`（新建，`100755`） | 计划 §4.1 修订版逐字（`sed -n '/^#!\/usr\/bin\/env bash$/,/^echo "cloud-setup 完成"$/p' <计划> \| diff - scripts/cloud-setup.sh` 无输出）。注：派单工作项 1 说「第 ④ 步有 `cargo test --workspace --no-run`」——修订后的 §4.1 已经去掉了预编译，照抄的是修订后的 |
| 2 | `scripts/check.sh` | `:5-7` 用法；`:12-21` 参数循环（`--quick` / `--docker`，不认识的参数退 2）；`:23-36` root 降权 `NOROOT`；`:57-63` B cargo 那格（捕获 cargo 退出码、`NR==0` 兜底、打编译错误）；`:64-72` Go 那格（失败包名在前、末行 `go packages ok=N fail=M`、退 go test 的码）；`:74-76` **B8 逐字未动**；`:78-86` B9；`:88-93` `--docker` 那格 |
| 3 | `Makefile` | `:16-29` 五个镜像参数变量与注释；`:32-33` `.PHONY` 补 `evals-p1 docker-test cloud-setup`；`:62-67` `evals-p1`；`:75-81` `docker-test: docker-image`；`:83-84` `cloud-setup`；`:95-101` `compose-build` 透传五个 `--build-arg`。`proto-gen` 没动 |
| 4 | `.github/workflows/ci.yml` | `:61-72` checks 里 B8 后加 B9 步；`:294-314` 新 job `sandbox-docker`（checkout → setup-go 照抄 → 建镜像 → `-tags docker` → `if: always()` 的遗留容器断言；`timeout-minutes: 25`）。`on:` 没动 |
| 5 | `docker/core/Dockerfile` | `:14-25` 参数说明 + 全局 `ARG BASE_REGISTRY=docker.io`；`:29` `FROM ${BASE_REGISTRY}/library/rust:…`；`:49-50` `ARG APT_MIRROR=` / `ARG PROTOC_URL=…`；`:57-60` builder 的 apt 源替换；`:65` protoc URL 用前缀；`:87-97` `ARG CARGO_REGISTRY=` 与 `replace-with`；`:101-102` 运行层 `FROM` + `ARG APT_MIRROR=`；运行层 apt 源替换 |
| 5 | `docker/edge/Dockerfile` | `:9-16` 参数说明 + 全局 `ARG BASE_REGISTRY=docker.io`；`:20` builder `FROM`；`:63-64` 运行层 `FROM` + `ARG APT_MIRROR=`；运行层 apt 源替换。BB3 的 `GOPROXY` / `GOSUMDB` 原样 |
| 5 | `docker-compose.yml` | **没改**：不写 `build.args`，免得 `${X:-}` 的空串盖掉 Dockerfile 默认值；透传只走 `make compose-build` 的全局 `--build-arg` |
| 6 | `review/p1/cc1-crates-patch.py`（新建，`100755`） | **改成补丁脚本**（原因见第 7 节）：五个 crate 全文 + `core/Cargo.toml` 两处锚点 + `core/crates/app/Cargo.toml` 一处锚点 + 让 cargo 最小改锁；`--check` / `--root` / 有效性闸门。`core/Cargo.toml`、`core/Cargo.lock`、`core/crates/app/Cargo.toml`、`core/crates/{admin,search,githost,routines,memory}/**` 在本 PR 里**都没动** |
| 7 | `.gitignore` | 没改：`.venv/`、`*.egg-info/` 两条在（`:34-35`）；`git ls-files .venv aite.egg-info \| wc -l` → `0`；`git ls-files '*.md' \| grep -v /` → `CLAUDE.md`、`README.md` |
| 8 | `docs/p1/cloud-runbook.md`（新建） | 环境实测、可达性表、setup 两次墙钟与 WARN、首编是否冷编、root、五个镜像参数的国内取值、check.sh 三次墙钟、抖动记录、改 setup 要贴回环境设置 |
| 8 | `CLAUDE.md` | 「## 验收」基线那一条换成新口径（`go packages` 行、B9、`--docker`、root 降权）；「## 云端基线」填表（B0 实测一行；P0-CLOSE 之后一行标「计划值（§5.1），待总管实测」）。别的节没动 |

## 3. 验证逐条

### 3.1 Go 行（`/tmp/cc1-verify2.log`，`scripts/check.sh --quick`）

```
################ 0 绿：scripts/check.sh --quick（无探针） ################
=== B 全量 go test（-race） ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd edge && o=$(go test -race ./... -count=1 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^FAIL[[:space:]]+aite/edge/" | head -n 5; ok=$(printf "%s\n" "$o" | grep -cE "^(ok|\?)[[:space:]]"); fail=$(printf "%s\n" "$o" | grep -cE "^FAIL[[:space:]]+aite/edge/"); echo "go packages ok=${ok} fail=${fail}"; exit "$c"
go packages ok=9 fail=0
-> exit 0
全部通过
check exit=0

################ 1 红：Go 探针 edge/cmd/aite-edge/zz_cc1_probe_test.go ################
=== B 全量 go test（-race） ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd edge && o=$(go test -race ./... -count=1 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^FAIL[[:space:]]+aite/edge/" | head -n 5; ok=$(printf "%s\n" "$o" | grep -cE "^(ok|\?)[[:space:]]"); fail=$(printf "%s\n" "$o" | grep -cE "^FAIL[[:space:]]+aite/edge/"); echo "go packages ok=${ok} fail=${fail}"; exit "$c"
FAIL	aite/edge/cmd/aite-edge	4.986s
go packages ok=8 fail=1
-> exit 1  ✗
以下没过：B 全量 go test（-race）
check exit=1
```

探针删掉之后（步骤 2、3 与终版）都是 `go packages ok=9 fail=0`。「以下没过」只列了 go test 那一格，`gofmt -l` 为空（A4d 没被带红）。

### 3.2 B 全量 cargo test 那格（降权与「编不过假绿」两处改动的变异，`/tmp/cc1-verify2b.log`）

```
######## 降权的变异：新格子去掉 setpriv（= 撤回降权），root 直跑
error: test failed, to rerun pass `-p aite --lib`
error: test failed, to rerun pass `-p aite --test cli_smoke`
error: test failed, to rerun pass `-p aite --test crash_recovery`
error: test failed, to rerun pass `-p aite --test preflight_e2e`
error: test failed, to rerun pass `-p aite-store --test crash_recovery`
cargo passed=890 failed=7
cell exit=1
######## 编不过的探针：core/crates/store/tests/zz_cc1_probe.rs
---- 旧格子（撤回修复）
cargo passed= failed=
cell exit=0
---- 新格子
error: could not compile `aite-store` (test "zz_cc1_probe") due to 1 previous error
error[E0308]: mismatched types
cargo passed=0 failed=0
cell exit=1
---- 删探针后新格子
cargo passed=897 failed=0
cell exit=0
######## 不认识的参数
不认识的参数：--bogus（只认 --quick / --docker）
check exit=2
 M .github/workflows/ci.yml
 M Makefile
 M docker/core/Dockerfile
 M docker/edge/Dockerfile
 M scripts/check.sh
exit=0
```

- 撤回降权（新格子不套 `setpriv`、root 直跑）→ `890/7`、红；
- 撤回假绿修复（旧格子）+ 编不过的探针 → `cargo passed= failed=`、**`cell exit=0`（假绿复现）**；新格子 → 打出 `error[E0308]` / `could not compile`、`exit=1`；删探针 → 897/0 绿；
- 不认识的参数退 2。

### 3.3 B9（`/tmp/cc1-verify2.log`）

```
################ 0 绿：scripts/check.sh --quick（无探针） ################
=== B9 评测 evals/p1 ===
B9 skip：evals/p1 尚无场景
全部通过
check exit=0

################ 2 绿：B9 探针 evals/p1/zz_cc1_probe.yaml（passed 1/1） ################
=== B9 评测 evals/p1（passed k/k） ===
$ bash -c o=$(core/target/debug/aite evals run evals/p1 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | awk "\$1 == \"passed\" && split(\$2, a, \"/\") == 2 && a[1] == a[2] {ok = 1} END {exit !ok}"
}
passed 1/1
-> exit 0
全部通过
check exit=0

################ 3 红：B9 探针改成 equals: 2 ################
=== B9 评测 evals/p1（passed k/k） ===
$ bash -c o=$(core/target/debug/aite evals run evals/p1 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | awk "\$1 == \"passed\" && split(\$2, a, \"/\") == 2 && a[1] == a[2] {ok = 1} END {exit !ok}"
}
passed 0/1
-> exit 1  ✗
以下没过：B9 评测 evals/p1（passed k/k）
check exit=1
```

探针删掉、`rmdir evals/p1` 之后回到 skip（终版两次即是）。

CI 的 B9 步（自查补的一处）：第一版写的是 `aite … | tee /tmp/b9.log` 再判末行，而 GitHub 没写 `shell:` 时用 `bash -e {0}`、**不带 pipefail**，
aite 的退出码会被 tee 吞掉。用一个「打出 `passed 1/1` 却退 1」的替身实测（`bash -e -c` 模拟 runner）：

```
#### 旧步骤（bash -e，无 pipefail）
passed 1/1
step exit=0
#### 新步骤（先 set -o pipefail）
passed 1/1
step exit=1
```

所以那一步开头加了 `set -o pipefail`（与 B8 那格「退出码 0 **且**末行」的两条判据对齐）。check.sh 的 B9 是先捕获 `c=$?` 再判，没有这个问题。

`.yml`：`load_suite`（`core/crates/evals/src/scenario.rs:520-525`）认 `.yaml` 与 `.yml`，所以 check.sh / Makefile / CI 三处的判空都是
`ls evals/p1/*.yaml >/dev/null 2>&1 || ls evals/p1/*.yml >/dev/null 2>&1`，与它对齐。`make evals-p1`：无场景打 `B9 skip：evals/p1 尚无场景` 退 0；放探针后 `passed 1/1`。

### 3.4 `--docker` / `make docker-test` / 遗留容器断言

云端 dockerd 默认不在，CC1 手动 `dockerd &` 起了一个。

```
#### DOCKER_HOST=unix:///nonexistent scripts/check.sh --quick --docker
=== D docker 组（-tags docker + 遗留容器 0） ===
$ bash -c docker build -q -t aite-sandbox:p0 docker/sandbox >/dev/null || { echo "沙箱镜像没建成"; exit 1; }; (cd edge && go test -tags docker ./internal/sandbox/... -count=1); c=$?; n=$(docker ps -a --filter label=aite.task -q | wc -l | tr -d " "); echo "遗留 aite.task 容器：${n}"; [ "$c" = 0 ] && [ "$n" = 0 ]
ERROR: failed to connect to the docker API at unix:///nonexistent; check if the path is correct and if the daemon is running: dial unix /nonexistent: connect: no such file or directory
沙箱镜像没建成
-> exit 1  ✗
以下没过：D docker 组（-tags docker + 遗留容器 0）
check exit=1

#### scripts/check.sh --quick --docker
=== D docker 组（-tags docker + 遗留容器 0） ===
$ bash -c docker build -q -t aite-sandbox:p0 docker/sandbox >/dev/null || { echo "沙箱镜像没建成"; exit 1; }; (cd edge && go test -tags docker ./internal/sandbox/... -count=1); c=$?; n=$(docker ps -a --filter label=aite.task -q | wc -l | tr -d " "); echo "遗留 aite.task 容器：${n}"; [ "$c" = 0 ] && [ "$n" = 0 ]
--------------------
  31 |     COPY requirements.txt /tmp/requirements.txt
  32 | >>> RUN pip install --no-cache-dir --root-user-action=ignore -r /tmp/requirements.txt \
  33 | >>>  && rm /tmp/requirements.txt
  34 |     
--------------------
ERROR: failed to build: failed to solve: process "/bin/sh -c pip install --no-cache-dir --root-user-action=ignore -r /tmp/requirements.txt  && rm /tmp/requirements.txt" did not complete successfully: exit code: 1
沙箱镜像没建成
-> exit 1  ✗
以下没过：D docker 组（-tags docker + 遗留容器 0）
check exit=1
exit=0
```

- 坏 `DOCKER_HOST` → docker 那格红（符合派单）；
- 正常一次在云端**也红**，红在沙箱镜像的 `pip install`：出口 TLS 拦截代理，容器里验不过证书（`self-signed certificate in certificate chain`）；
  之前还先撞了 Docker Hub 429。这不是 check.sh / 沙箱 Dockerfile 的问题——**这一条交 CI 的 `sandbox-docker` 判：本 PR 上它是绿的**（约 1 分钟）。
- 为了验「测试那一段 + 断言」本身，用注入代理 CA 的**临时副本**（不入库）建了一个 `aite-sandbox:p0`：

```
sha256:47914c24157aef1aada475e99932624d6189e9546c6898f8b4a2ba8de0601184

real	0m55.295s
user	0m0.314s
sys	0m0.260s
exit=0
ok  	aite/edge/internal/sandbox	21.289s
gotest exit=0
遗留 aite.task 容器：0
```

  然后 `make -o docker-image docker-test`（`-o` 跳过重建镜像那一步）：

```
ok  	aite/edge/internal/sandbox	29.352s
遗留 aite.task 容器：0
exit=0
```

  遗留容器断言的变异：先 `docker create --label aite.task=zz-cc1-probe …` 埋一个，再跑 `make -o docker-image docker-test` ——
  **沙箱测试自己把它回收了**（孤儿清扫），断言看到 0、照样绿；所以改用 `GO=true` 把 go test 换成空操作，单独验断言：

```
遗留 aite.task 容器：1
make: *** [Makefile:78: docker-test] Error 1
exit(planted, GO=true)=2
遗留 aite.task 容器：0
exit(clean, GO=true)=0
```

  坏 `DOCKER_HOST` 下 `make docker-test` 红在 `docker-image`（`make: *** [Makefile:73: docker-image] Error 1`）。

### 3.5 五个镜像参数（`/tmp/cc1-verify5.log`）

`make -n compose-build`：

```
docker compose --profile images build \
	--build-arg "BASE_REGISTRY=docker.io" \
	--build-arg "GOPROXY=https://proxy.golang.org,direct" \
	--build-arg "PROTOC_URL=https://github.com/protocolbuffers/protobuf/releases/download" \
	--build-arg "CARGO_REGISTRY=" \
	--build-arg "APT_MIRROR="
```

口径：云端容器里 HTTPS 验不过证书（见 3.4），所以凡要在容器里走 HTTPS 的步骤用注入 CA 的临时副本（`--build-context ccr=/root/.ccr`，每个 `FROM` 后加
`COPY` CA + `ENV SSL_CERT_FILE / CURL_CA_BUNDLE / CARGO_HTTP_CAINFO`，其余逐字同仓库那份）；只到 `FROM` / apt（http）就该红的直接用仓库里的 Dockerfile。
基础镜像是从 `mirror.gcr.io` 拉下来打成 docker.io 的 tag 的（Docker Hub 429）。

| 编号 | 构建 | 期望 | 实际 |
|---|---|---|---|
| C0 | core 默认值（副本） | 绿 | `exit 0`；运行层 sources 仍是 `deb.debian.org`；`aite 0.0.1` |
| C1 | core `BASE_REGISTRY=invalid.example`（真） | 红在 FROM | `>>> FROM ${BASE_REGISTRY}/library/debian:trixie-slim` … `invalid.example/library/debian:trixie-slim: failed to resolve source metadata`，`exit 1` |
| C2 | core `APT_MIRROR=invalid.example`（真） | 红在 apt | 运行层 `E: Package 'ca-certificates' has no installation candidate`，`exit code: 100`（builder 那层见下） |
| C3 | core `PROTOC_URL=https://invalid.example/protobuf`（副本） | 红在 curl protoc | `curl … "https://invalid.example/protobuf/v31.1/protoc-31.1-linux-${arch}.zip"` → `curl: (6) Could not resolve host: invalid.example`，`exit code: 6` |
| C4 | core `CARGO_REGISTRY=sparse+https://invalid.example/index/`（副本） | 红在 cargo build | `failed to query replaced source registry \`crates-io\`` / `Could not resolve host: invalid.example`，`exit code: 101` |
| C5 | core `BASE_REGISTRY=mirror.gcr.io` + `APT_MIRROR=deb.debian.org`（副本） | 绿 | `exit 0`（真的从 mirror.gcr.io 拉基础镜像；APT_MIRROR 走了替换分支） |
| E0 | edge 默认值（副本） | 绿 | `exit 0` |
| E1 | edge `BASE_REGISTRY=invalid.example`（真） | 红在 FROM | `>>> FROM ${BASE_REGISTRY}/library/golang:${GO_VERSION}-trixie AS builder`，`exit 1` |
| E2 | edge `APT_MIRROR=invalid.example`（副本） | 红在运行层 apt | `Err:1 http://invalid.example/debian trixie InRelease` … `http://invalid.example/debian-security/dists/trixie-security/InRelease`（security 那条也落到镜像上）… `exit code: 100` |
| E3 | edge `GOPROXY=https://invalid.example`（副本） | 红在 Go 取模块 | `go build` 过了（cache mount 已热），红在 `go install grpc-health-probe@v0.4.57`（查 deprecation 要走 proxy，BB3 注释里写过），`exit 1` |
| M1 | `make compose-build BASE_REGISTRY=invalid.example` | 透传到位 | core 与 edge 都红在 `load metadata for invalid.example/library/…`，`exit 2` |

说明：

- C2 builder 那层：`apt-get update;` 抓取失败只告警不退非 0（apt 默认行为，旧代码同样），而 rust 镜像里本来就有 `curl unzip`，所以 builder 那层
  不会因为 APT_MIRROR 写错而红；在云端它接着红在 protoc 的 curl（TLS），CI 上会一路走下去——整次构建仍然红在运行层的 apt。要让 builder 那层也严格，
  得改 `apt-get update` 的错误模式，那会改变默认路径的行为（上游偶发抓取失败时也会红），不在「不传时逐字等价」之内，没做。
- M1 里 `sandbox-image` 没出现在失败行里：今天的沙箱 Dockerfile（CC12 之前）没声明 `BASE_REGISTRY`，docker 只告警。CC12 落地后才会一起红在 FROM。
- 「默认值下 CI 那条路不变」由 CI 判：`compose-smoke`（直接 `docker compose build`）在本 PR 上绿。

完整矩阵输出：

```

######## C0 core 默认值（测试副本）（期望：绿）
$ docker build -f /tmp/claude-0/-home-user-aite/d84cdf19-3c24-519e-a192-a51424f6cc29/scratchpad/core.test.Dockerfile --build-context ccr=/root/.ccr -t cc1-core-default .
#15 [builder 5/7] COPY proto ./proto
#16 [builder 6/7] COPY core ./core
#12 [stage-1 3/9] RUN if [ -n "" ]; then       sed -i -e "s#deb\.debian\.org##g" -e "s#security\.debian\.org##g"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates netcat-openbsd  && rm -rf /var/lib/apt/lists/*
#17 [builder 7/7] RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked     --mount=type=cache,target=/usr/local/cargo/git,sharing=locked     --mount=type=cache,target=/target,sharing=locked     if [ -n "" ]; then       printf '[source.crates-io]\nreplace-with = "aite-mirror"\n\n[source.aite-mirror]\nregistry = "%s"\n'         "" > "/usr/local/cargo/config.toml";     fi     && cd core && cargo build --workspace --locked     && mkdir -p /out && cp /target/debug/aite /out/aite
#12 [stage-1 3/9] RUN if [ -n "" ]; then       sed -i -e "s#deb\.debian\.org##g" -e "s#security\.debian\.org##g"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates netcat-openbsd  && rm -rf /var/lib/apt/lists/*
#18 [stage-1 4/9] WORKDIR /app
#17 [builder 7/7] RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked     --mount=type=cache,target=/usr/local/cargo/git,sharing=locked     --mount=type=cache,target=/target,sharing=locked     if [ -n "" ]; then       printf '[source.crates-io]\nreplace-with = "aite-mirror"\n\n[source.aite-mirror]\nregistry = "%s"\n'         "" > "/usr/local/cargo/config.toml";     fi     && cd core && cargo build --workspace --locked     && mkdir -p /out && cp /target/debug/aite /out/aite
#19 [stage-1 5/9] COPY --from=builder /out/aite /usr/local/bin/aite
#20 [stage-1 6/9] COPY core/crates/worker/prompts /app/core/crates/worker/prompts
#21 [stage-1 7/9] COPY config/aite.example.yaml /app/config/aite.example.yaml
#22 [stage-1 8/9] COPY evals /app/evals
#23 [stage-1 9/9] RUN mkdir -p /app/data/run && chmod 1777 /app/data/run
-> exit 0

######## C1 core BASE_REGISTRY=invalid.example（真 Dockerfile）（期望：红在 FROM）
$ docker build -f docker/core/Dockerfile --build-arg BASE_REGISTRY=invalid.example .
#1 [internal] load build definition from Dockerfile
#2 [internal] load metadata for invalid.example/library/debian:trixie-slim
#2 ERROR: failed to do request: Head "https://invalid.example/v2/library/debian/manifests/trixie-slim": Forbidden
#3 [internal] load metadata for invalid.example/library/rust:1.98.1-trixie
 101 | >>> FROM ${BASE_REGISTRY}/library/debian:trixie-slim
ERROR: failed to build: failed to solve: invalid.example/library/debian:trixie-slim: failed to resolve source metadata for invalid.example/library/debian:trixie-slim: failed to do request: Head "https://invalid.example/v2/library/debian/manifests/trixie-slim": Forbidden
-> exit 1

######## C2 core APT_MIRROR=invalid.example（真 Dockerfile）（期望：红在 builder 的 apt-get update）
$ docker build -f docker/core/Dockerfile --build-arg APT_MIRROR=invalid.example .
#9 7.337 W: Some index files failed to download. They have been ignored, or old ones used instead.
#9 7.456 curl: (60) SSL certificate problem: self-signed certificate in certificate chain
#9 7.456 curl failed to verify the legitimacy of the server and therefore could not
#9 ERROR: process "/bin/sh -c set -eu;     case \"${TARGETARCH}\" in       arm64) arch=aarch_64 ;;       amd64) arch=x86_64 ;;       *) echo \"不支持的 TARGETARCH=${TARGETARCH}（只接 arm64 / amd64）\" >&2; exit 1 ;;     esac;     if [ -n \"${APT_MIRROR}\" ]; then       sed -i -e \"s#deb\\.debian\\.org#${APT_MIRROR}#g\" -e \"s#security\\.debian\\.org#${APT_MIRROR}#g\"         /etc/apt/sources.list.d/debian.sources;     fi;     apt-get update;     apt-get install -y --no-install-recommends curl unzip;     rm -rf /var/lib/apt/lists/*;     curl -fsSL -o /tmp/protoc.zip       \"${PROTOC_URL:-https://github.com/protocolbuffers/protobuf/releases/download}/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip\";     unzip -q /tmp/protoc.zip -d /usr/local bin/protoc 'include/*';     rm /tmp/protoc.zip;     protoc --version" did not complete successfully: exit code: 60
#8 [stage-1 2/8] RUN if [ -n "invalid.example" ]; then       sed -i -e "s#deb\.debian\.org#invalid.example#g" -e "s#security\.debian\.org#invalid.example#g"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates netcat-openbsd  && rm -rf /var/lib/apt/lists/*
#8 ERROR: process "/bin/sh -c if [ -n \"${APT_MIRROR}\" ]; then       sed -i -e \"s#deb\\.debian\\.org#${APT_MIRROR}#g\" -e \"s#security\\.debian\\.org#${APT_MIRROR}#g\"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates netcat-openbsd  && rm -rf /var/lib/apt/lists/*" did not complete successfully: exit code: 100
7.456 curl: (60) SSL certificate problem: self-signed certificate in certificate chain
7.456 curl failed to verify the legitimacy of the server and therefore could not
7.302 W: Some index files failed to download. They have been ignored, or old ones used instead.
7.329 E: Package 'ca-certificates' has no installation candidate
7.329 E: Unable to locate package netcat-openbsd
ERROR: failed to build: failed to solve: process "/bin/sh -c set -eu;     case \"${TARGETARCH}\" in       arm64) arch=aarch_64 ;;       amd64) arch=x86_64 ;;       *) echo \"不支持的 TARGETARCH=${TARGETARCH}（只接 arm64 / amd64）\" >&2; exit 1 ;;     esac;     if [ -n \"${APT_MIRROR}\" ]; then       sed -i -e \"s#deb\\.debian\\.org#${APT_MIRROR}#g\" -e \"s#security\\.debian\\.org#${APT_MIRROR}#g\"         /etc/apt/sources.list.d/debian.sources;     fi;     apt-get update;     apt-get install -y --no-install-recommends curl unzip;     rm -rf /var/lib/apt/lists/*;     curl -fsSL -o /tmp/protoc.zip       \"${PROTOC_URL:-https://github.com/protocolbuffers/protobuf/releases/download}/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip\";     unzip -q /tmp/protoc.zip -d /usr/local bin/protoc 'include/*';     rm /tmp/protoc.zip;     protoc --version" did not complete successfully: exit code: 60
-> exit 1

######## C3 core PROTOC_URL=https://invalid.example/protobuf（测试副本）（期望：红在 curl protoc）
$ docker build -f /tmp/claude-0/-home-user-aite/d84cdf19-3c24-519e-a192-a51424f6cc29/scratchpad/core.test.Dockerfile --build-context ccr=/root/.ccr --build-arg PROTOC_URL=https://invalid.example/protobuf .
#4 [internal] load metadata for docker.io/library/rust:1.98.1-trixie
#5 [internal] load .dockerignore
#6 [stage-1 1/9] FROM docker.io/library/debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
#7 [builder 1/7] FROM docker.io/library/rust:1.98.1-trixie@sha256:a8a5f0a1e5fe7dfe1d352591e4a1c7dd2c08fd70475cae872cf3458ba0df0546
#8 [context ccr] load from client
#9 [internal] load build context
#10 [builder 2/7] COPY --from=ccr ca-bundle.crt /etc/ssl/certs/ccr-bundle.crt
#11 [builder 3/7] RUN set -eu;     case "amd64" in       arm64) arch=aarch_64 ;;       amd64) arch=x86_64 ;;       *) echo "不支持的 TARGETARCH=amd64（只接 arm64 / amd64）" >&2; exit 1 ;;     esac;     if [ -n "" ]; then       sed -i -e "s#deb\.debian\.org##g" -e "s#security\.debian\.org##g"         /etc/apt/sources.list.d/debian.sources;     fi;     apt-get update;     apt-get install -y --no-install-recommends curl unzip;     rm -rf /var/lib/apt/lists/*;     curl -fsSL -o /tmp/protoc.zip       "https://invalid.example/protobuf/v31.1/protoc-31.1-linux-${arch}.zip";     unzip -q /tmp/protoc.zip -d /usr/local bin/protoc 'include/*';     rm /tmp/protoc.zip;     protoc --version
#11 4.276 curl: (6) Could not resolve host: invalid.example
#11 ERROR: process "/bin/sh -c set -eu;     case \"${TARGETARCH}\" in       arm64) arch=aarch_64 ;;       amd64) arch=x86_64 ;;       *) echo \"不支持的 TARGETARCH=${TARGETARCH}（只接 arm64 / amd64）\" >&2; exit 1 ;;     esac;     if [ -n \"${APT_MIRROR}\" ]; then       sed -i -e \"s#deb\\.debian\\.org#${APT_MIRROR}#g\" -e \"s#security\\.debian\\.org#${APT_MIRROR}#g\"         /etc/apt/sources.list.d/debian.sources;     fi;     apt-get update;     apt-get install -y --no-install-recommends curl unzip;     rm -rf /var/lib/apt/lists/*;     curl -fsSL -o /tmp/protoc.zip       \"${PROTOC_URL:-https://github.com/protocolbuffers/protobuf/releases/download}/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip\";     unzip -q /tmp/protoc.zip -d /usr/local bin/protoc 'include/*';     rm /tmp/protoc.zip;     protoc --version" did not complete successfully: exit code: 6
4.276 curl: (6) Could not resolve host: invalid.example
ERROR: failed to build: failed to solve: process "/bin/sh -c set -eu;     case \"${TARGETARCH}\" in       arm64) arch=aarch_64 ;;       amd64) arch=x86_64 ;;       *) echo \"不支持的 TARGETARCH=${TARGETARCH}（只接 arm64 / amd64）\" >&2; exit 1 ;;     esac;     if [ -n \"${APT_MIRROR}\" ]; then       sed -i -e \"s#deb\\.debian\\.org#${APT_MIRROR}#g\" -e \"s#security\\.debian\\.org#${APT_MIRROR}#g\"         /etc/apt/sources.list.d/debian.sources;     fi;     apt-get update;     apt-get install -y --no-install-recommends curl unzip;     rm -rf /var/lib/apt/lists/*;     curl -fsSL -o /tmp/protoc.zip       \"${PROTOC_URL:-https://github.com/protocolbuffers/protobuf/releases/download}/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip\";     unzip -q /tmp/protoc.zip -d /usr/local bin/protoc 'include/*';     rm /tmp/protoc.zip;     protoc --version" did not complete successfully: exit code: 6
-> exit 1

######## C4 core CARGO_REGISTRY=sparse+https://invalid.example/index/（测试副本）（期望：红在 cargo build）
$ docker build -f /tmp/claude-0/-home-user-aite/d84cdf19-3c24-519e-a192-a51424f6cc29/scratchpad/core.test.Dockerfile --build-context ccr=/root/.ccr --build-arg CARGO_REGISTRY=sparse+https://invalid.example/index/ .
#15 278.5 warning: spurious network error (2 tries remaining): [6] Could not resolve hostname (Could not resolve host: invalid.example)
#15 282.0 warning: spurious network error (1 try remaining): [6] Could not resolve hostname (Could not resolve host: invalid.example)
#15 288.5 error: failed to get `anyhow` as a dependency of package `aite v0.0.1 (/src/core/crates/app)`
#15 288.5   failed to load source for dependency `anyhow`
#15 288.5   failed to query replaced source registry `crates-io`
#15 288.5   download of config.json failed
#15 288.5   [6] Could not resolve hostname (Could not resolve host: invalid.example)
#15 ERROR: process "/bin/sh -c if [ -n \"${CARGO_REGISTRY}\" ]; then       printf '[source.crates-io]\\nreplace-with = \"aite-mirror\"\\n\\n[source.aite-mirror]\\nregistry = \"%s\"\\n'         \"${CARGO_REGISTRY}\" > \"${CARGO_HOME}/config.toml\";     fi     && cd core && cargo build --workspace --locked     && mkdir -p /out && cp /target/debug/aite /out/aite" did not complete successfully: exit code: 101
288.5   failed to query replaced source registry `crates-io`
288.5   download of config.json failed
288.5   [6] Could not resolve hostname (Could not resolve host: invalid.example)
ERROR: failed to build: failed to solve: process "/bin/sh -c if [ -n \"${CARGO_REGISTRY}\" ]; then       printf '[source.crates-io]\\nreplace-with = \"aite-mirror\"\\n\\n[source.aite-mirror]\\nregistry = \"%s\"\\n'         \"${CARGO_REGISTRY}\" > \"${CARGO_HOME}/config.toml\";     fi     && cd core && cargo build --workspace --locked     && mkdir -p /out && cp /target/debug/aite /out/aite" did not complete successfully: exit code: 101
-> exit 1

######## C5 core BASE_REGISTRY=mirror.gcr.io + APT_MIRROR=deb.debian.org（测试副本）（期望：绿）
$ docker build -f /tmp/claude-0/-home-user-aite/d84cdf19-3c24-519e-a192-a51424f6cc29/scratchpad/core.test.Dockerfile --build-context ccr=/root/.ccr --build-arg BASE_REGISTRY=mirror.gcr.io --build-arg APT_MIRROR=deb.debian.org -t cc1-core-mirror .
#15 [builder 5/7] COPY proto ./proto
#13 [stage-1 3/9] RUN if [ -n "deb.debian.org" ]; then       sed -i -e "s#deb\.debian\.org#deb.debian.org#g" -e "s#security\.debian\.org#deb.debian.org#g"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates netcat-openbsd  && rm -rf /var/lib/apt/lists/*
#16 [builder 6/7] COPY core ./core
#13 [stage-1 3/9] RUN if [ -n "deb.debian.org" ]; then       sed -i -e "s#deb\.debian\.org#deb.debian.org#g" -e "s#security\.debian\.org#deb.debian.org#g"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates netcat-openbsd  && rm -rf /var/lib/apt/lists/*
#17 [builder 7/7] RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked     --mount=type=cache,target=/usr/local/cargo/git,sharing=locked     --mount=type=cache,target=/target,sharing=locked     if [ -n "" ]; then       printf '[source.crates-io]\nreplace-with = "aite-mirror"\n\n[source.aite-mirror]\nregistry = "%s"\n'         "" > "/usr/local/cargo/config.toml";     fi     && cd core && cargo build --workspace --locked     && mkdir -p /out && cp /target/debug/aite /out/aite
#13 [stage-1 3/9] RUN if [ -n "deb.debian.org" ]; then       sed -i -e "s#deb\.debian\.org#deb.debian.org#g" -e "s#security\.debian\.org#deb.debian.org#g"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates netcat-openbsd  && rm -rf /var/lib/apt/lists/*
#18 [stage-1 4/9] WORKDIR /app
#19 [stage-1 5/9] COPY --from=builder /out/aite /usr/local/bin/aite
#20 [stage-1 6/9] COPY core/crates/worker/prompts /app/core/crates/worker/prompts
#21 [stage-1 7/9] COPY config/aite.example.yaml /app/config/aite.example.yaml
#22 [stage-1 8/9] COPY evals /app/evals
#23 [stage-1 9/9] RUN mkdir -p /app/data/run && chmod 1777 /app/data/run
-> exit 0
---- C5 镜像里的 sources（运行层）
URIs: http://deb.debian.org/debian
URIs: http://deb.debian.org/debian-security
---- C0 默认镜像里的 sources（运行层，应为上游原样）
URIs: http://deb.debian.org/debian
URIs: http://deb.debian.org/debian-security
aite 0.0.1

######## E0 edge 默认值（测试副本）（期望：绿）
$ docker build -f /tmp/claude-0/-home-user-aite/d84cdf19-3c24-519e-a192-a51424f6cc29/scratchpad/edge.test.Dockerfile --build-context ccr=/root/.ccr -t cc1-edge-default .
#13 [builder 3/6] WORKDIR /src
#14 [builder 4/6] COPY edge ./edge
#12 [stage-1 3/7] RUN if [ -n "" ]; then       sed -i -e "s#deb\.debian\.org##g" -e "s#security\.debian\.org##g"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates  && rm -rf /var/lib/apt/lists/*
#15 [builder 5/6] RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked     --mount=type=cache,target=/root/.cache/go-build,sharing=locked     cd edge && go build -o /out/aite-edge ./cmd/aite-edge
#12 [stage-1 3/7] RUN if [ -n "" ]; then       sed -i -e "s#deb\.debian\.org##g" -e "s#security\.debian\.org##g"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates  && rm -rf /var/lib/apt/lists/*
#15 [builder 5/6] RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked     --mount=type=cache,target=/root/.cache/go-build,sharing=locked     cd edge && go build -o /out/aite-edge ./cmd/aite-edge
#16 [stage-1 4/7] WORKDIR /app
#15 [builder 5/6] RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked     --mount=type=cache,target=/root/.cache/go-build,sharing=locked     cd edge && go build -o /out/aite-edge ./cmd/aite-edge
#17 [builder 6/6] RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked     --mount=type=cache,target=/root/.cache/go-build,sharing=locked     GOBIN=/out go install "github.com/grpc-ecosystem/grpc-health-probe@v0.4.57"
#18 [stage-1 5/7] COPY --from=builder /out/aite-edge /usr/local/bin/aite-edge
#19 [stage-1 6/7] COPY --from=builder /out/grpc-health-probe /usr/local/bin/grpc-health-probe
#20 [stage-1 7/7] RUN mkdir -p /app/data/run && chmod 1777 /app/data/run
-> exit 0

######## E1 edge BASE_REGISTRY=invalid.example（真 Dockerfile）（期望：红在 FROM）
$ docker build -f docker/edge/Dockerfile --build-arg BASE_REGISTRY=invalid.example .
#1 [internal] load build definition from Dockerfile
#2 [internal] load metadata for invalid.example/library/debian:trixie-slim
#3 [internal] load metadata for invalid.example/library/golang:1.27.1-trixie
#3 ERROR: failed to do request: Head "https://invalid.example/v2/library/golang/manifests/1.27.1-trixie": Forbidden
#2 [internal] load metadata for invalid.example/library/debian:trixie-slim
  20 | >>> FROM ${BASE_REGISTRY}/library/golang:${GO_VERSION}-trixie AS builder
ERROR: failed to build: failed to solve: invalid.example/library/golang:1.27.1-trixie: failed to resolve source metadata for invalid.example/library/golang:1.27.1-trixie: failed to do request: Head "https://invalid.example/v2/library/golang/manifests/1.27.1-trixie": Forbidden
-> exit 1

######## E2 edge APT_MIRROR=invalid.example（测试副本）（期望：红在运行层 apt-get update）
$ docker build -f /tmp/claude-0/-home-user-aite/d84cdf19-3c24-519e-a192-a51424f6cc29/scratchpad/edge.test.Dockerfile --build-context ccr=/root/.ccr --build-arg APT_MIRROR=invalid.example .
#16 7.261 Err:1 http://invalid.example/debian trixie InRelease
#16 7.261   Could not resolve 'invalid.example'
#16 7.281 W: Failed to fetch http://invalid.example/debian/dists/trixie/InRelease  Could not resolve 'invalid.example'
#16 7.281 W: Failed to fetch http://invalid.example/debian/dists/trixie-updates/InRelease  Could not resolve 'invalid.example'
#16 7.281 W: Failed to fetch http://invalid.example/debian-security/dists/trixie-security/InRelease  Could not resolve 'invalid.example'
#16 7.281 W: Some index files failed to download. They have been ignored, or old ones used instead.
#16 7.309 E: Package 'ca-certificates' has no installation candidate
#16 ERROR: process "/bin/sh -c if [ -n \"${APT_MIRROR}\" ]; then       sed -i -e \"s#deb\\.debian\\.org#${APT_MIRROR}#g\" -e \"s#security\\.debian\\.org#${APT_MIRROR}#g\"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates  && rm -rf /var/lib/apt/lists/*" did not complete successfully: exit code: 100
7.281 W: Failed to fetch http://invalid.example/debian-security/dists/trixie-security/InRelease  Could not resolve 'invalid.example'
7.281 W: Some index files failed to download. They have been ignored, or old ones used instead.
7.309 E: Package 'ca-certificates' has no installation candidate
ERROR: failed to build: failed to solve: process "/bin/sh -c if [ -n \"${APT_MIRROR}\" ]; then       sed -i -e \"s#deb\\.debian\\.org#${APT_MIRROR}#g\" -e \"s#security\\.debian\\.org#${APT_MIRROR}#g\"         /etc/apt/sources.list.d/debian.sources;     fi  && apt-get update  && apt-get install -y --no-install-recommends ca-certificates  && rm -rf /var/lib/apt/lists/*" did not complete successfully: exit code: 100
-> exit 1

######## E3 edge GOPROXY=https://invalid.example（测试副本）（期望：红在 go build / go install）
$ docker build -f /tmp/claude-0/-home-user-aite/d84cdf19-3c24-519e-a192-a51424f6cc29/scratchpad/edge.test.Dockerfile --build-context ccr=/root/.ccr --build-arg GOPROXY=https://invalid.example .
#5 [internal] load .dockerignore
#6 [stage-1 1/7] FROM docker.io/library/debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
#7 [builder 1/6] FROM docker.io/library/golang:1.27.1-trixie@sha256:433790e515d27dc6003e847e644cc0af956985cf315c1c58a3b73ee2dd305183
#8 [context ccr] load from client
#9 [internal] load build context
#10 [builder 3/6] WORKDIR /src
#11 [builder 2/6] COPY --from=ccr ca-bundle.crt /etc/ssl/certs/ccr-bundle.crt
#12 [builder 4/6] COPY edge ./edge
#13 [builder 5/6] RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked     --mount=type=cache,target=/root/.cache/go-build,sharing=locked     cd edge && go build -o /out/aite-edge ./cmd/aite-edge
#14 [builder 6/6] RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked     --mount=type=cache,target=/root/.cache/go-build,sharing=locked     GOBIN=/out go install "github.com/grpc-ecosystem/grpc-health-probe@v0.4.57"
#14 ERROR: process "/bin/sh -c GOBIN=/out go install \"github.com/grpc-ecosystem/grpc-health-probe@${HEALTH_PROBE_VERSION}\"" did not complete successfully: exit code: 1
ERROR: failed to build: failed to solve: process "/bin/sh -c GOBIN=/out go install \"github.com/grpc-ecosystem/grpc-health-probe@${HEALTH_PROBE_VERSION}\"" did not complete successfully: exit code: 1
-> exit 1

######## M1 make compose-build BASE_REGISTRY=invalid.example（期望：三个镜像都红在 FROM，证明透传到 core / edge / sandbox-image）
	--build-arg "BASE_REGISTRY=invalid.example" \
	--build-arg "GOPROXY=https://proxy.golang.org,direct" \
	--build-arg "PROTOC_URL=https://github.com/protocolbuffers/protobuf/releases/download" \
	--build-arg "CARGO_REGISTRY=" \
	--build-arg "APT_MIRROR="
#9 [edge internal] load metadata for invalid.example/library/debian:trixie-slim
#10 [core internal] load metadata for invalid.example/library/rust:1.98.1-trixie
#10 ERROR: failed to do request: Head "https://invalid.example/v2/library/rust/manifests/1.98.1-trixie": Forbidden
#11 [edge internal] load metadata for invalid.example/library/golang:1.27.1-trixie
#11 ERROR: failed to do request: Head "https://invalid.example/v2/library/golang/manifests/1.27.1-trixie": Forbidden
#9 [edge internal] load metadata for invalid.example/library/debian:trixie-slim
 > [core internal] load metadata for invalid.example/library/rust:1.98.1-trixie:
 > [edge internal] load metadata for invalid.example/library/golang:1.27.1-trixie:
target edge: failed to solve: invalid.example/library/golang:1.27.1-trixie: failed to resolve source metadata for invalid.example/library/golang:1.27.1-trixie: failed to do request: Head "https://invalid.example/v2/library/golang/manifests/1.27.1-trixie": Forbidden
-> exit 2
verify5 done
exit=0
```

### 3.6 cloud-setup 两次

| 次 | `real` | 退出码 | 末行 | `WARN:` |
|---|---|---|---|---|
| 1 | 0m4.288s | `exit=0` | `cloud-setup 完成` | `WARN: 沙箱镜像没建成（多半是 deb.debian.org / pypi 不通），会话里要跑 docker 那组时再建` |
| 2 | 0m2.770s | `exit=0` | `cloud-setup 完成` | 同上 |

第 2 次日志（第 1 次多了 60 行 `Downloaded <crate>`，其余相同）：

```
info: syncing channel updates for 1.98.1-x86_64-unknown-linux-gnu
info: latest update on 2026-09-03 for version 1.98.1 (48a229cea 2026-09-01)
info: component clippy is up to date
info: component rustfmt is up to date

  1.98.1-x86_64-unknown-linux-gnu unchanged - rustc 1.98.1 (48a229cea 2026-09-01)

info: checking for self-update (current version: 1.29.1)
go version go1.24.7 linux/amd64
go: google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2 requires go >= 1.25.0; switching to go1.26.8
protoc-gen-go v1.36.12
protoc-gen-go-grpc 1.6.2
go version go1.27.1 linux/amd64
Dockerfile:15
--------------------
  13 |     # 不写 --platform：本机是 aarch64，指定 amd64 基底会让 pandas/matplotlib
  14 |     # 走 QEMU 模拟，慢到没法演示。
  15 | >>> FROM python:3.11-slim
  16 |     
  17 |     # --- 系统层：字体 + fontconfig ------------------------------------------------
--------------------
ERROR: failed to build: failed to solve: python:3.11-slim: failed to resolve source metadata for docker.io/library/python:3.11-slim: unexpected status from HEAD request to https://registry-1.docker.io/v2/library/python/manifests/3.11-slim: 429 Too Many Requests
WARN: 沙箱镜像没建成（多半是 deb.debian.org / pypi 不通），会话里要跑 docker 那组时再建
cloud-setup 完成

real	0m2.770s
user	0m0.960s
sys	0m0.580s
exit=0
```

WARN 的真实原因是 Docker Hub 429（日志里 `python:3.11-slim … 429 Too Many Requests`），过了 429 也会卡在 pip 的 TLS；只告警，符合脚本设计。
能走到 docker build 是因为 CC1 先在会话里手动起了 dockerd。

## 4. check.sh 完整输出

- 自检冷启动（旧 check.sh）：第 1.4 节，`real 7m30.350s`，890/7。
- 终版第一次（清掉 `core/target` 后冷编）：`real 5m58.476s`，全部通过。
- 终版第二次（热）：`real 0m42.042s`，全部通过。
- 两次终版都没有时序抖动；本会话 9 次 check.sh + 1 次副本上的全量，「已知时序抖动」那几条都没红——**但见 4.1：这不代表它们稳**。

### 4.1 CI 上的 `reconnect_replay` 红（不是本 PR 的，是 R7 的真竞态）

推到 `5301616` 之后 CI 的 `checks` 红在 `B cargo test（全量）`：

```
test a_root_and_its_thread_followup_replayed_together ... FAILED
thread 'a_root_and_its_thread_followup_replayed_together' (6338) panicked at crates/app/tests/reconnect_replay.rs:784:9:
并发重推只许落在两种结局上：追问并进同一份 transcript（ignored=0），或者它抢在 root 前面、按 R8 被丢掉并记一笔（ignored=1）。实际 transcript=["再按季度画一张", "按月画个图"]、events.ignored=0 —— 这是第三种：有东西静悄悄没了。计数器：{}
test result: FAILED. 10 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.35s
error: test failed, to rerun pass `-p aite --test reconnect_replay`
```

- **不是本 PR 的**：本 PR 一行 Rust 都没改；`checks` job 只多了 B9 一步，排在失败那步之后。同一份 Rust 代码在上一次 CI（`d915cf7`）上是绿的。
- **在未改动的 main 代码上复现**：会话里 `cargo test -q -p aite --test reconnect_replay` 连跑 40 次，**8 次红**（都是这一条），32 次绿。
- **病根**（`core/crates/control/src/plane.rs` 的 `new_session`，R7）：`self.store.create_session(&session)` 落库之后，先 `await` 一次
  `platform.add_reaction`，才 `append_turn` root 那条。同话题并发到达的追问在这个窗口里 `find_session_by_thread` 已经命中 → R6
  `continue_session` → 它的 `append_turn` 先拿到 seq=0，root 那条变成 seq=1。于是 transcript 是 `["再按季度画一张", "按月画个图"]`、
  `ignored=0`——两句都在，顺序反了；测试只认「正序并进」或「按 R8 丢弃」两种形状。BB1 派单里记过「它想防的竞态是真会发生的」，这就是那个竞态。
- **修法（在仓库副本上试过，没进本 PR）**：把 `create_session` 与 root 那条 `append_turn` 放进同一段 `turn_seq_lock` 临界区（拆出一个
  「调用方已持锁」的 `append_turn_locked`），ack 挪到临界区之外（别让平台往返攥着全局锁）。副本上：`reconnect_replay` 连跑 **40 次 0 红**；
  `cargo test -p aite-control`、`cargo test -p aite` 全绿；`clippy -p aite-control -D warnings` 绿；rustfmt 过。行为上唯一的变化：
  ack 表情现在落在 root 的 turn 写完之后（写 turn 失败时不再先贴 ack）。diff 见第 8 节那一行。
- `control/**` 在 W1 归 CC2（「`control/**` 这一波只有你一个主人」），所以本 PR 不改它；记账转给 CC2，并在 PR 上留了一条说明。

终版第一次：

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
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 49s
-> exit 0

=== A2 go build ./... ===
$ bash -c cd edge && go build ./...

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
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 18s
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
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^error(: test failed|: could not compile|\[E[0-9]+\])" | sort -u | head -n 5; printf "%s\n" "$o" | grep -E "^test result" | awk -v c="$c" "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p+0 \" failed=\" f+0; exit (c != 0 || f > 0 || NR == 0)}"
cargo passed=897 failed=0
-> exit 0

=== B 全量 go test（-race） ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd edge && o=$(go test -race ./... -count=1 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^FAIL[[:space:]]+aite/edge/" | head -n 5; ok=$(printf "%s\n" "$o" | grep -cE "^(ok|\?)[[:space:]]"); fail=$(printf "%s\n" "$o" | grep -cE "^FAIL[[:space:]]+aite/edge/"); echo "go packages ok=${ok} fail=${fail}"; exit "$c"
go packages ok=9 fail=0
-> exit 0

=== B8 评测（passed 10/10） ===
$ bash -c o=$(core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | grep -qx "passed 10/10"
}
passed 10/10
-> exit 0

=== B9 评测 evals/p1 ===
B9 skip：evals/p1 尚无场景

全部通过

real	5m58.476s
user	14m43.122s
sys	2m49.379s
exit=0
```

终版第二次：

```

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.36s
-> exit 0

=== A2 go build ./... ===
$ bash -c cd edge && go build ./...

-> exit 0

=== A3/C2 契约锁 --check ===
$ core/target/debug/aite contracts lock --check
OK 25 files
-> exit 0

=== A4a cargo clippy -D warnings ===
$ bash -c cd core && cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.48s
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
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^error(: test failed|: could not compile|\[E[0-9]+\])" | sort -u | head -n 5; printf "%s\n" "$o" | grep -E "^test result" | awk -v c="$c" "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p+0 \" failed=\" f+0; exit (c != 0 || f > 0 || NR == 0)}"
cargo passed=897 failed=0
-> exit 0

=== B 全量 go test（-race） ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd edge && o=$(go test -race ./... -count=1 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^FAIL[[:space:]]+aite/edge/" | head -n 5; ok=$(printf "%s\n" "$o" | grep -cE "^(ok|\?)[[:space:]]"); fail=$(printf "%s\n" "$o" | grep -cE "^FAIL[[:space:]]+aite/edge/"); echo "go packages ok=${ok} fail=${fail}"; exit "$c"
go packages ok=9 fail=0
-> exit 0

=== B8 评测（passed 10/10） ===
$ bash -c o=$(core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | grep -qx "passed 10/10"
}
passed 10/10
-> exit 0

=== B9 评测 evals/p1 ===
B9 skip：evals/p1 尚无场景

全部通过

real	0m42.042s
user	0m35.319s
sys	0m9.163s
exit=0
```

## 5. cargo passed 增量

Δ = 0：本 PR 在主树上 897 → 897（B 那格只加了降权，测试一条没加）。骨架 crate 打上之后（在仓库副本上跑补丁脚本 + 全量 check.sh，见第 6 节）也是 897：
五个空 crate 不加测试，每个只多两行 `0 passed`（lib 单测 + doctest）。

## 6. 骨架 crate 与 Cargo.lock（补丁脚本在仓库副本上的自测）

自测做法：`git worktree add --detach /tmp/cc1-selftest HEAD`，`python3 review/p1/cc1-crates-patch.py --root /tmp/cc1-selftest [--check]`，
然后在副本里跑全量 `scripts/check.sh`（不带 `--quick`，所以 A4a clippy 真跑了；会话磁盘装不下第二份 target，副本的 `core/target` 软链到主树）。

```
#### --check（--root 副本）
[命中 1 条] core/Cargo.toml: workspace 依赖表上方那句注释：放开四个骨架 crate 依赖 aite-gateway
[命中 1 条] core/Cargo.toml: workspace 依赖表：aite-evals 之后加五条 aite-*
[命中 1 条] core/crates/app/Cargo.toml: aite 的 [dependencies]：aite-evals 之后挂五个骨架 crate
[  合法] 五个 lib.rs：rustfmt 认、没有 doctest 形状
[  合法] 临时副本：cargo metadata 过、Cargo.lock 恰好多 5 个 aite-* 包、零删除行

--check：没写盘。会写这些文件：
  core/Cargo.toml
  core/crates/app/Cargo.toml
  core/crates/admin/Cargo.toml
  core/crates/admin/src/lib.rs
  core/crates/search/Cargo.toml
  core/crates/search/src/lib.rs
  core/crates/githost/Cargo.toml
  core/crates/githost/src/lib.rs
  core/crates/routines/Cargo.toml
  core/crates/routines/src/lib.rs
  core/crates/memory/Cargo.toml
  core/crates/memory/src/lib.rs
exit=0
#### 真写（--root 副本）
[命中 1 条] core/Cargo.toml: workspace 依赖表上方那句注释：放开四个骨架 crate 依赖 aite-gateway
[命中 1 条] core/Cargo.toml: workspace 依赖表：aite-evals 之后加五条 aite-*
[命中 1 条] core/crates/app/Cargo.toml: aite 的 [dependencies]：aite-evals 之后挂五个骨架 crate
[  合法] 五个 lib.rs：rustfmt 认、没有 doctest 形状
[  合法] 临时副本：cargo metadata 过、Cargo.lock 恰好多 5 个 aite-* 包、零删除行
写了 core/Cargo.toml
写了 core/crates/app/Cargo.toml
写了 core/crates/admin/Cargo.toml
写了 core/crates/admin/src/lib.rs
写了 core/crates/search/Cargo.toml
写了 core/crates/search/src/lib.rs
写了 core/crates/githost/Cargo.toml
写了 core/crates/githost/src/lib.rs
写了 core/crates/routines/Cargo.toml
写了 core/crates/routines/src/lib.rs
写了 core/crates/memory/Cargo.toml
写了 core/crates/memory/src/lib.rs
改了 core/Cargo.lock（恰好多 5 个 aite-* 包、零删除行）
[读回 OK] 三处锚点都换成了新内容

接着跑：scripts/check.sh（期望 cargo passed 不变，A4a clippy 绿）
exit=0
#### 再跑一次（应该拒：已打过）
[命中 0 条] core/Cargo.toml: workspace 依赖表上方那句注释：放开四个骨架 crate 依赖 aite-gateway —— 看起来已经打过了（新内容已在文件里）
[命中 1 条] core/Cargo.toml: workspace 依赖表：aite-evals 之后加五条 aite-*
[命中 0 条] core/crates/app/Cargo.toml: aite 的 [dependencies]：aite-evals 之后挂五个骨架 crate —— 看起来已经打过了（新内容已在文件里）

对不上的地方（一个字节都没写）：
  - core/Cargo.toml: 锚点命中 0 条（要求恰好 1 条）
  - core/crates/app/Cargo.toml: 锚点命中 0 条（要求恰好 1 条）
  - core/crates/admin/Cargo.toml 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/admin/src/lib.rs 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/search/Cargo.toml 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/search/src/lib.rs 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/githost/Cargo.toml 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/githost/src/lib.rs 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/routines/Cargo.toml 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/routines/src/lib.rs 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/memory/Cargo.toml 已经存在 —— 本补丁只新建，不覆盖
  - core/crates/memory/src/lib.rs 已经存在 —— 本补丁只新建，不覆盖
exit=1
#### git diff --numstat -- core/Cargo.lock
96	0	core/Cargo.lock
#### git diff -- core/Cargo.lock | grep '^+name = '
+name = "aite-admin"
+name = "aite-githost"
+name = "aite-memory"
+name = "aite-routines"
+name = "aite-search"
#### git diff -- core/Cargo.lock（全文）
diff --git a/core/Cargo.lock b/core/Cargo.lock
index b751d67..e571481 100644
--- a/core/Cargo.lock
+++ b/core/Cargo.lock
@@ -29,14 +29,19 @@ dependencies = [
 name = "aite"
 version = "0.0.1"
 dependencies = [
+ "aite-admin",
  "aite-contracts",
  "aite-control",
  "aite-edge-client",
  "aite-evals",
  "aite-evidence",
  "aite-gateway",
+ "aite-githost",
+ "aite-memory",
  "aite-models",
  "aite-proto",
+ "aite-routines",
+ "aite-search",
  "aite-store",
  "aite-testing",
  "aite-worker",
@@ -56,6 +61,21 @@ dependencies = [
  "tracing-subscriber",
 ]
 
+[[package]]
+name = "aite-admin"
+version = "0.0.1"
+dependencies = [
+ "aite-contracts",
+ "async-trait",
+ "chrono",
+ "serde",
+ "serde_json",
+ "tempfile",
+ "thiserror",
+ "tokio",
+ "tracing",
+]
+
 [[package]]
 name = "aite-contracts"
 version = "0.0.1"
@@ -153,6 +173,45 @@ dependencies = [
  "tracing",
 ]
 
+[[package]]
+name = "aite-githost"
+version = "0.0.1"
+dependencies = [
+ "aite-contracts",
+ "aite-gateway",
+ "aite-testing",
+ "async-trait",
+ "hex",
+ "reqwest",
+ "serde",
+ "serde_json",
+ "sha2",
+ "tempfile",
+ "thiserror",
+ "tokio",
+ "tracing",
+ "uuid",
+]
+
+[[package]]
+name = "aite-memory"
+version = "0.0.1"
+dependencies = [
+ "aite-contracts",
+ "aite-gateway",
+ "aite-testing",
+ "async-trait",
+ "chrono",
+ "rusqlite",
+ "serde",
+ "serde_json",
+ "tempfile",
+ "thiserror",
+ "tokio",
+ "tracing",
+ "uuid",
+]
+
 [[package]]
 name = "aite-models"
 version = "0.0.1"
@@ -183,6 +242,43 @@ dependencies = [
  "tonic-prost-build",
 ]
 
+[[package]]
+name = "aite-routines"
+version = "0.0.1"
+dependencies = [
+ "aite-contracts",
+ "aite-gateway",
+ "aite-testing",
+ "async-trait",
+ "chrono",
+ "rusqlite",
+ "serde",
+ "serde_json",
+ "tempfile",
+ "thiserror",
+ "tokio",
+ "tracing",
+ "uuid",
+]
+
+[[package]]
+name = "aite-search"
+version = "0.0.1"
+dependencies = [
+ "aite-contracts",
+ "aite-gateway",
+ "aite-testing",
+ "async-trait",
+ "reqwest",
+ "rusqlite",
+ "serde",
+ "serde_json",
+ "tempfile",
+ "thiserror",
+ "tokio",
+ "tracing",
+]
+
 [[package]]
 name = "aite-store"
 version = "0.0.1"
#### cargo metadata 包名
17 ['aite', 'aite-admin', 'aite-contracts', 'aite-control', 'aite-edge-client', 'aite-evals', 'aite-evidence', 'aite-gateway', 'aite-githost', 'aite-memory', 'aite-models', 'aite-proto', 'aite-routines', 'aite-search', 'aite-store', 'aite-testing', 'aite-worker']
#### scripts/check.sh（全量，含 clippy）

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
   Compiling aite-routines v0.0.1 (/tmp/cc1-selftest/core/crates/routines)
   Compiling aite-memory v0.0.1 (/tmp/cc1-selftest/core/crates/memory)
   Compiling aite-evidence v0.0.1 (/tmp/cc1-selftest/core/crates/evidence)
   Compiling aite-control v0.0.1 (/tmp/cc1-selftest/core/crates/control)
   Compiling aite-store v0.0.1 (/tmp/cc1-selftest/core/crates/store)
   Compiling aite-worker v0.0.1 (/tmp/cc1-selftest/core/crates/worker)
   Compiling aite v0.0.1 (/tmp/cc1-selftest/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.52s
-> exit 0

=== A2 go build ./... ===
$ bash -c cd edge && go build ./...

-> exit 0

=== A3/C2 契约锁 --check ===
$ core/target/debug/aite contracts lock --check
OK 25 files
-> exit 0

=== A4a cargo clippy -D warnings ===
$ bash -c cd core && cargo clippy --workspace --all-targets -- -D warnings
    Checking aite-routines v0.0.1 (/tmp/cc1-selftest/core/crates/routines)
    Checking aite-githost v0.0.1 (/tmp/cc1-selftest/core/crates/githost)
    Checking aite-memory v0.0.1 (/tmp/cc1-selftest/core/crates/memory)
    Checking aite-search v0.0.1 (/tmp/cc1-selftest/core/crates/search)
    Checking aite-admin v0.0.1 (/tmp/cc1-selftest/core/crates/admin)
    Checking aite-edge-client v0.0.1 (/tmp/cc1-selftest/core/crates/edge-client)
    Checking aite v0.0.1 (/tmp/cc1-selftest/core/crates/app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.02s
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
  Executable tests/test_context.rs (target/debug/deps/test_context-f9e37917ca0f9199)
  Executable tests/test_final.rs (target/debug/deps/test_final-a7d6f2ccc8c655a8)
  Executable tests/test_in_flight.rs (target/debug/deps/test_in_flight-931a5c77157efb1e)
  Executable tests/test_limits.rs (target/debug/deps/test_limits-029e3eecbee11646)
  Executable tests/test_loop_fallbacks.rs (target/debug/deps/test_loop_fallbacks-13c22a3da37ccddb)
  Executable tests/test_prompts_checklist.rs (target/debug/deps/test_prompts_checklist-95800b50d62494fa)
  Executable tests/test_sandbox_handoff.rs (target/debug/deps/test_sandbox_handoff-18e25eacc1b8f353)
  Executable tests/test_steer.rs (target/debug/deps/test_steer-63c771c6cd3f40a3)
-> exit 0

=== C1 契约测试 ===
$ bash -c cd core && cargo test -p aite-contracts 2>&1 | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"contracts passed=\" p \" failed=\" f; exit (f>0)}"
contracts passed=25 failed=0
-> exit 0

=== B 全量 cargo test ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^error(: test failed|: could not compile|\[E[0-9]+\])" | sort -u | head -n 5; printf "%s\n" "$o" | grep -E "^test result" | awk -v c="$c" "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p+0 \" failed=\" f+0; exit (c != 0 || f > 0 || NR == 0)}"
cargo passed=897 failed=0
-> exit 0

=== B 全量 go test（-race） ===
$ setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search -- bash -c cd edge && o=$(go test -race ./... -count=1 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^FAIL[[:space:]]+aite/edge/" | head -n 5; ok=$(printf "%s\n" "$o" | grep -cE "^(ok|\?)[[:space:]]"); fail=$(printf "%s\n" "$o" | grep -cE "^FAIL[[:space:]]+aite/edge/"); echo "go packages ok=${ok} fail=${fail}"; exit "$c"
go packages ok=9 fail=0
-> exit 0

=== B8 评测（passed 10/10） ===
$ bash -c o=$(core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | grep -qx "passed 10/10"
}
passed 10/10
-> exit 0

=== B9 评测 evals/p1 ===
B9 skip：evals/p1 尚无场景

全部通过

real	2m28.102s
user	5m28.207s
sys	1m13.819s
check exit=0
exit=0
```

- `git diff --numstat -- core/Cargo.lock` → `96	0	core/Cargo.lock`：**零删除**；`+name` 恰好 5 行、全是 `aite-*`；新包的依赖名全是锁里已有的。
- `cargo metadata --no-deps` → 17 个包，含 `aite-admin`、`aite-githost`、`aite-memory`、`aite-routines`、`aite-search`。
- 副本上的全量 check.sh：全部通过，`cargo passed=897 failed=0`，A4a clippy 绿、A4b fmt 绿（`//!` 里没有代码块、没有 4 空格缩进行、列表续行不懒）。
- 补丁脚本是「只加不删」的锚点：`aite-evals = { path = "crates/evals" }` 这条锚点在打过之后仍命中 1 次（新内容以它开头），
  但另外两条锚点与「五个 crate 已存在」会让重跑整份被拒（见上面「再跑一次」那段），不会重复插入。

逐 crate 的依赖与理由（全部取自 `[workspace.dependencies]`、都已在锁里，一律 `xxx.workspace = true`、不加 feature）：

| crate | 消费轨 | `[dependencies]` | `[dev-dependencies]` | 在派单建议之外的增删与理由 |
|---|---|---|---|---|
| aite-admin | DD12 | aite-contracts, serde, serde_json, tokio, async-trait, thiserror, tracing, chrono | tokio, tempfile | 照建议，不增不减：DD12 本来要加 axum 并改锁（D4 / D12），缺什么届时一起加；不声明 aite-gateway |
| aite-memory | EE1 | aite-contracts, **aite-gateway**, serde, serde_json, tokio, async-trait, thiserror, tracing, chrono, **rusqlite**, **uuid** | tokio, tempfile, aite-testing | +rusqlite：计划 CT21「每群一份记忆文件（SQLite）」；+uuid：记忆条目编号（W3 不许改锁，宁多勿漏） |
| aite-routines | EE2 | aite-contracts, **aite-gateway**, chrono, serde, serde_json, tokio, async-trait, thiserror, tracing, **rusqlite**, **uuid** | tokio, **tempfile**, aite-testing | +rusqlite：CT22 的 routines 表若落在本 crate 自己的库里；+uuid：例程编号；+tempfile（dev）：落盘测试 |
| aite-search | EE4 | aite-contracts, **aite-gateway**, reqwest, serde, serde_json, tokio, async-trait, thiserror, tracing, **rusqlite** | tokio, **tempfile**, aite-testing | +rusqlite：工作区搜索的本地消息索引（派单点名的候选）；+tempfile（dev） |
| aite-githost | EE5 | aite-contracts, **aite-gateway**, reqwest, serde, serde_json, tokio, async-trait, thiserror, tracing, sha2, hex, **uuid** | tokio, tempfile, aite-testing | +uuid：分支名去重；sha2 / hex 照建议。**缺口**：「服务端拉归档进 `/work/repo`」若是 tar.gz，解包要的 crate（flate2 / tar 之类）不在 workspace 依赖表里 = 新第三方依赖，记账见第 8 节 |

`aite-gateway` 那一列按原卡定死：memory / routines / search / githost 声明、admin 不声明。`core/Cargo.toml` 那句注释改成
「各轨只许依赖 contracts / proto / testing；memory / routines / search / githost 四个骨架 crate 另许依赖 aite-gateway；其余跨轨依赖 → 停下报告」。
app 代码里没有 `use` 新 crate、没有建 `features/*.rs`。

## 7. 被拦过的命令与拦截原文（第 2 步那条除外）

1. **守卫（误拦，已知形态）**：开场自检第 3 步把工具链 / 环境 / 可达性写成一条多命令的 Bash（含 `docker info --format '{{.ServerVersion}}'` 与 `for … "$h exit=$?"`）：

   ```
   PreToolUse:Bash hook error: [d=$(git rev-parse --show-toplevel 2>/dev/null); [ -f "$d/.claude/hooks/guard_bash.py" ] || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"]: blocked: 该操作触碰受保护面 <命令无法解析: No closing quotation>（解析失败）。停止当前工作并向人类报告。
   ```

   命令本身不碰任何受保护面；照 `CLAUDE.md`「已知误拦与绕法」把同样的命令写成 scratchpad 里的脚本文件再 `bash` 它跑（内容逐字相同），没有换写法绕保护面。
2. **会话权限规则（不是守卫）**：Edit 工具改 `core/Cargo.toml` 两次都被拒：

   ```
   File is in a directory that is denied by your permission settings.
   ```

   `core/Cargo.toml` 在派单可写面里，`core/crates/app/Cargo.toml` 同样的 Edit 却能过。问过总管：先说放开权限再试，重试仍被拒；
   第二次选「工作项 6 转成补丁脚本」。已写进 `app/Cargo.toml` 的改动与五个 crate 目录随即撤回（`git checkout` + `rm -r`），主树上工作项 6 一个字节都没留。
   没用 Bash / 脚本在主树上写 `core/Cargo.toml`（那等于绕权限）。

## 8. 记账转出去的

| 事项 | 为什么不在本轨 | 建议归哪轨 |
|---|---|---|
| **打 `review/p1/cc1-crates-patch.py`**（五个骨架 crate + 三个 Cargo 文件 + 锁），命令链见脚本头 | 云端会话的权限规则拒写 `core/Cargo.toml`（第 7 节） | 总管本机，**W2 派 DD12 之前**（DD12 要写 `core/crates/admin/**`，W3 的 EE1/2/4/5 同理） |
| **其余第 1 波各轨的开场自检在云端必是 890/7**，直到本 PR 合并 | 它们从 main clone，main 上的 check.sh 还没降权 | 总管：派单 / 计划 §4.4 加一句「本 PR 合并前，云端自检第 4 步认 `890/7`，7 条名单见 CC1.md 1.5；或自己在 check.sh 那格前加 `setpriv …` 单跑」；或先合本 PR 再派 |
| `README.md:104-117` BB3 的「手敲 `--build-arg GOPROXY`」说明因 `make compose-build` 透传而过时 | `README.md` 不在可写面 | EE14（W3 的部署文档 / Makefile 主人） |
| `PIP_INDEX_URL` 的 Makefile / compose 透传 | CC12 的参数，不在原卡五个名字里；`BASE_REGISTRY` / `APT_MIRROR` 已靠全局 `--build-arg` 送到 `sandbox-image`，只差它 | EE14 |
| `scripts/cloud-setup.sh` 第 ⑤ 步 WARN 文案「多半是 deb.debian.org / pypi 不通」在云端误导：真实原因是 Docker Hub 429，其次是容器里 TLS 验不过 | 改它要贴回环境设置（缓存失效）；不是 bug，没改 | 总管下次改 setup 时顺手（或 EE14） |
| 骨架 crate 的反向依赖：worker / control 要调用记忆、例程时（如 worker 注入记忆上下文、control 的调度入口）需要 `aite-worker → aite-memory`、`aite-control → aite-routines` 之类的边；今天只有 `aite`（app）依赖五个骨架 crate | 派单：本轨不做；W3 又不许改 `Cargo.toml` / 锁 | 总管在 W2 末（W3 开派前）按 EE1 / EE2 的设计判一次，要就由 W2 的某轨（或一个补丁脚本）预先加边 |
| EE5 解 tar.gz 归档需要的 crate 不在 workspace 依赖表里 | 新第三方依赖要先批 | H1 类审批（总管）；或 EE5 改走 GitLab 的 zip / 逐文件 API 绕开 |
| 云端建不出沙箱镜像（TLS 拦截代理 + Docker Hub 429）；要在云端跑 docker 组，得让 docker 构建信任代理 CA（例如 cloud-setup 里给 dockerd 配 CA、或沙箱 Dockerfile 支持可选的额外 CA） | 改沙箱 Dockerfile 是 CC12 的面；改 setup 要贴回环境 | CC12（可选 CA 参数）/ 总管（环境设置）；否则一律以 CI `sandbox-docker` 为准 |
| `apt-get update` 抓取失败不退非 0，`APT_MIRROR` 写错时 core builder 那层不红（整次构建仍红在运行层） | 改错误模式会改变默认路径的行为 | EE14 评估（离线包 / 多架构时一起） |
| **R7 的竞态**：`reconnect_replay` 的 `a_root_and_its_thread_followup_replayed_together` 约 1/5 红（4.1），修法见下面的 diff（副本上 40/40 绿） | `core/crates/control/**` 不在本轨可写面；W1 归 CC2 | **CC2**（或总管另开一个小修复）；合并之前任何 PR 的 `checks` 都可能被它随机弄红 |

R7 竞态的修法（在仓库副本上验过，未入库）：

```diff
--- a/core/crates/control/src/plane.rs
+++ b/core/crates/control/src/plane.rs
@@ -1040,7 +1040,17 @@ impl InProcessControlPlane {
             last_active_at: now,
             archived_at: None,
         };
-        self.store.create_session(&session).await?;
+        // 建会话与写 root 那条 turn 在同一段 seq 临界区里：会话一落库，同话题并发到达的
+        // 追问就能经 find_session_by_thread 命中 R6；root 的 turn 若还没写，追问先拿到
+        // seq=0，transcript 顺序就反了。ack 挪到临界区之外，别让平台往返攥着全局的锁。
+        {
+            let _seq_guard = self.turn_seq_lock.lock().await;
+            self.store.create_session(&session).await?;
+            if !text.is_empty() {
+                self.append_turn_locked(&session, ev, TurnRole::User, text)
+                    .await?;
+            }
+        }
         if react {
             // ack 失败不影响建任务（Python 的 contextlib.suppress）
             if let Err(e) = self
@@ -1052,7 +1062,6 @@ impl InProcessControlPlane {
             }
         }
         if !text.is_empty() {
-            self.append_turn(&session, ev, TurnRole::User, text).await?;
             self.start_task(&session, ev, Some(text)).await?;
         }
         Ok(session)
@@ -1327,6 +1336,17 @@ impl InProcessControlPlane {
         content: &str,
     ) -> Result<(), IngressError> {
         let _seq_guard = self.turn_seq_lock.lock().await;
+        self.append_turn_locked(session, ev, role, content).await
+    }
+
+    /// 同 [`Self::append_turn`]，但调用方已经持有 `turn_seq_lock`。
+    async fn append_turn_locked(
+        &self,
+        session: &Session,
+        ev: &NormalizedEvent,
+        role: TurnRole,
+        content: &str,
+    ) -> Result<(), IngressError> {
         let recent = self.store.list_turns(&session.id, 1).await?;
         let seq = recent.last().map(|t| t.seq + 1).unwrap_or(0);
         self.store
```

## 9. 没做的与原因

- **五个骨架 crate 没进主树**：改成补丁脚本（第 7、8 节）。所以派单验收里 `cargo metadata` 17 个名字、`Cargo.lock` 的 `numstat` / `+name` 这几条
  在本 PR 的 diff 上**不成立**；它们在仓库副本上跑补丁之后成立（第 6 节原样输出），总管本机打补丁后应得到同样的结果。
- **云端正常跑一次 `--docker` / `make docker-test` 没绿**：红在沙箱镜像的 pip（TLS 拦截代理）。测试段与断言本身用注入 CA 的临时镜像验过（3.4），
  真判据交 CI 的 `sandbox-docker`（本 PR 上绿）。
- **镜像参数的「默认值建绿」在云端用的是注入 CA 的副本**，不是仓库那份原样；仓库那份原样的默认路径由 CI 的 `compose-smoke` 判（本 PR 上绿）。
  国内镜像站（tuna / aliyun / ustc / goproxy.cn）在云端全被网络策略拒，**没有一个国内取值是实测过的**，runbook 里的国内取值都是文档值，待大陆实测。
- 派单 §7 的 `git diff --name-only origin/main...HEAD`「全部落在 §3 可写面内」**差一个文件**：`review/p1/cc1-crates-patch.py` 不在原卡可写面里。
  它是总管在会话里选「工作项 6 转成补丁脚本」之后才有的，位置照 `CLAUDE.md`「守卫」一节「在 `review/` 下写补丁脚本」的惯例；其余 9 个文件都在可写面内。
- 派单 §7 的 `gh pr checks`：云端没有 `gh`，用 GitHub MCP 读 check runs：`checks`、`compose-smoke`、`sandbox-docker` 在推送过的 head 上都是 `success`
  （收尾推送之后的那次见 PR 页面）。

## 10. 要总管贴回环境设置的改动

无。`scripts/cloud-setup.sh` 与计划 §4.1 逐字相同（两次跑都 `exit=0`、末行 `cloud-setup 完成`）。

## 11. 契约缺口

无。骨架 crate 只预声明现有 workspace 依赖，没有逼出任何需要契约的形状。

## 12. 追记：工作项 6 打进主树（总管放开 `core/Cargo.toml` 写权限之后）

总管在会话里放开了 `core/Cargo.toml` 的写权限，让 CC1 直接跑补丁脚本。

```
$ python3 review/p1/cc1-crates-patch.py --check
[命中 1 条] core/Cargo.toml: workspace 依赖表上方那句注释：放开四个骨架 crate 依赖 aite-gateway
[命中 1 条] core/Cargo.toml: workspace 依赖表：aite-evals 之后加五条 aite-*
[命中 1 条] core/crates/app/Cargo.toml: aite 的 [dependencies]：aite-evals 之后挂五个骨架 crate
[  合法] 五个 lib.rs：rustfmt 认、没有 doctest 形状
[  合法] 临时副本：cargo metadata 过、Cargo.lock 恰好多 5 个 aite-* 包、零删除行
$ python3 review/p1/cc1-crates-patch.py
改了 core/Cargo.lock（恰好多 5 个 aite-* 包、零删除行）
[读回 OK] 三处锚点都换成了新内容
$ git diff --numstat -- core/Cargo.lock
96	0	core/Cargo.lock
$ git diff -- core/Cargo.lock | grep '^+name = '
+name = "aite-admin"
+name = "aite-githost"
+name = "aite-memory"
+name = "aite-routines"
+name = "aite-search"
$ cargo metadata --no-deps（包名）
17 ['aite', 'aite-admin', 'aite-contracts', 'aite-control', 'aite-edge-client', 'aite-evals', 'aite-evidence', 'aite-gateway', 'aite-githost', 'aite-memory', 'aite-models', 'aite-proto', 'aite-routines', 'aite-search', 'aite-store', 'aite-testing', 'aite-worker']
```

之后的 `scripts/check.sh`（全量，含 clippy）：A 各格、`OK 25 files`、`contracts passed=25 failed=0`、`go packages ok=9 fail=0`、`passed 10/10`、B9 skip 都对；
**B 那格两次是 `cargo passed=896 failed=1`**，失败 target 是 `-p aite-evals --test evals_runner`（check.sh 只打 target 名，没抓到测试名）。

- 这不是补丁带来的：补丁只加了五个空 crate 与三个 Cargo 文件，evals 一行没动；`cargo passed` 仍是 897 条（Δ=0）。
- 复现不出来：单跑 `evals_runner` 18 次、全量 `cargo test --workspace` 4 次、按 check.sh 的顺序（A1 → clippy → A5 → C1 → B）全量 3 次，
  加上一份把 B 那格输出落盘的 check.sh 副本跑 3 次、`scripts/check.sh --quick` 1 次——**全是 897/0**。
- 怀疑对象：`evals_runner.rs` 里带墙钟的那条 `a_plane_that_never_settles_times_out_with_a_reason`（`timeout_sec = 0.3`、5ms 轮询），冷编之后 CPU 紧时最可能踩线。
  没抓到现场，只是推断；记进 `CLAUDE.md`「已知时序抖动」要由总管定（不在本轨可写面里）。
