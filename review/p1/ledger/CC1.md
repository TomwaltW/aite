# CC1 回执：云端基建 · 基线 · 基建 R0 主人 · 镜像构建参数 · 骨架 crate

> 状态：**开场自检没过（第 3 步工具链 + 第 4 步 check.sh），按派单 §4「protoc / go 缺了不要自己装——写回执、推 PR、停」停在工作项 0。**
> 工作项 1–8 一个没动。环境（H3）补好之后，需要重新派一个 CC1 会话（本会话的 clone 里没有 protoc，接着干也过不了闸门）。

## 1. 开场自检原文

顶上五行：

- check.sh：**没过**，`exit=1`，末行 `以下没过：A1 cargo build --workspace A3/C2 契约锁 --check A4a cargo clippy -D warnings A5 cargo test --no-run（全部测试可编译） B8 评测（passed 10/10）`
  （根因只有一个：`aite-proto` 的 build.rs 找不到 protoc → 凡要编 core 全量的格都红，A3 / B8 没有 `core/target/debug/aite`；逐行全文在下面「第 4 步」）。
  另：**「B 全量 cargo test」那格假绿**（`cargo passed= failed=`、`-> exit 0`），见第 4 步末尾——这是 check.sh 的缺陷，归本轨工作项 2 修。
- 守卫：Read `.claude/hooks/guard_bash.py` **被拦**（原文：`blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。`）
- 工具链：**protoc 缺失**（`protoc: command not found`，exit 127）/ rustc 1.98.1 (48a229cea 2026-09-01) / go1.27.1 linux/amd64（GOTOOLCHAIN 自动拉取，镜像自带 go1.24.7）/ protoc-gen-go **缺失**（exit 127）/ protoc-gen-go-grpc **缺失**（exit 127）
- 情形：A，B0=`18b75fc06485290dd3bf492dca13311ee0fd29bb`（`18b75fc docs(p1): 对齐 Claude Tag 总计划 + 第 1 波派单 + 仓库卫生`）
- 可达性：deb.debian.org exit=56 / security.debian.org exit=56 / auth.docker.io exit=0 / production.cloudflare.docker.com exit=0 / pypi.org exit=0 / goproxy.cn exit=56（56 = 代理对 CONNECT 回 403，网络策略拒绝）

### 判断：H3 的环境设置没生效

证据（都是本会话实测）：

1. `protoc`、`protoc-gen-go`、`protoc-gen-go-grpc` 都不在 PATH 上；`/usr/local/bin` 下没有 protoc。§4.1 脚本第 ① 步会装 protoc 并在失败时 `exit 1`。
2. `rustup toolchain list` 是 `stable-x86_64-unknown-linux-gnu (active, default)` + `1.98.1`；1.98.1 是本会话第一次 `rustc --version` 时 rustup 现拉的
   （输出里有 `info: syncing channel updates for 1.98.1` / `downloading 5 components`）。§4.1 第 ② 步会预装 1.98.1。
3. `~/.cargo/registry` 不存在、`core/target/debug` 不存在：第 ④ 步的 `cargo fetch` / 预编译都没发生。
4. `env | grep GO` 为空：环境变量 `GOTOOLCHAIN=auto` 没设（go 仍拉到 1.27.1，是 go1.24.7 的默认 `GOTOOLCHAIN=auto` + go.mod 的 go 指令在起作用）。
5. 网络：`deb.debian.org`、`security.debian.org` 被代理 403 拒绝——§4.1 要求在 Custom 网络里加这两个（外加 `auth.docker.io`、
   `production.cloudflare.docker.com`，这两个是通的）。`goproxy.cn` 不在 §4.1 的清单里，403 是预期内的，只记录。
6. docker：`docker` / `dockerd` / `containerd` 二进制都在，但 daemon 没起（见下）。

要总管做的（claude.ai/code → 会话标题栏的云环境菜单 → Edit）：

- **Setup script**：贴计划 §4.1 那份脚本（本会话没跑它，脚本本身是否有 bug 未知）。
- **Network access**：Custom + 勾「同时包含默认 Trusted 列表」+ 加 `deb.debian.org`、`security.debian.org`、`auth.docker.io`、`production.cloudflare.docker.com`。
- **环境变量**：`GOTOOLCHAIN=auto`。
- 确认新会话用的是这个环境，然后重新派 CC1（环境设置只对新会话生效；新会话会起自己的 `claude/*` 分支和 draft PR，本 PR 到时关掉即可——它只有这份回执）。
- 第 1 波其余 12 轨**先别派**：它们的开场自检第 3 / 4 步会以同样的方式红。
- docker daemon 不在只影响后面的 `make docker-test` / `--docker`（工作项 2、3、5），不算闸门项；`dockerd` 二进制在，重派的 CC1 可以试着在会话里起它，起不来就照派单交 CI 的 `sandbox-docker` 判。

### 第 1 步：代码基线

```
$ git log --oneline -3
18b75fc docs(p1): 对齐 Claude Tag 总计划 + 第 1 波派单 + 仓库卫生
98e4460 merge(task-bb6): 并入 main
29433f4 merge(task-bb5): 并入 main
$ git cat-file -e 98e4460     # 在，不用 unshallow
$ git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'
（空）
```

→ 情形 A，基线行应为 `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok。

### 第 2 步：守卫

Read 工具读 `.claude/hooks/guard_bash.py`，被拦，原文：

```
PreToolUse:Read hook error: [d=$(git rev-parse --show-toplevel 2>/dev/null); [ -f "$d/.claude/hooks/guard_bash.py" ] || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"]: blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。
```

→ 通过（这一次被拦是期望结果）。

### 第 3 步：工具链

```
$ protoc --version
/bin/bash: line 1: protoc: command not found
protoc exit=127
$ (cd core && rustc --version)
info: syncing channel updates for 1.98.1-x86_64-unknown-linux-gnu
info: latest update on 2026-09-03 for version 1.98.1 (48a229cea 2026-09-01)
info: downloading 5 components
rustc 1.98.1 (48a229cea 2026-09-01)
$ (cd edge && go version)
go: downloading go1.27.1 (linux/amd64)
go version go1.27.1 linux/amd64
$ protoc-gen-go --version
/bin/bash: line 1: protoc-gen-go: command not found
pgg exit=127
$ protoc-gen-go-grpc --version
/bin/bash: line 1: protoc-gen-go-grpc: command not found
pggg exit=127
$ which protoc protoc-gen-go protoc-gen-go-grpc rustc cargo go
/root/.cargo/bin/rustc
/root/.cargo/bin/cargo
/usr/local/go/bin/go
$ env GOTOOLCHAIN=local go version      # 镜像自带的 go
go version go1.24.7 linux/amd64
$ rustup toolchain list
stable-x86_64-unknown-linux-gnu (active, default)
1.98.1-x86_64-unknown-linux-gnu
$ env | grep -iE '^(GO|CARGO|RUST|PROTOC|PATH)'
CARGO_HTTP_CAINFO=/root/.ccr/ca-bundle.crt
RUSTUP_HOME=/root/.rustup
PATH=/root/.local/bin:/root/.cargo/bin:/usr/local/go/bin:/opt/node22/bin:/opt/maven/bin:/opt/gradle/bin:/opt/rbenv/bin:/root/.bun/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
RUST_BACKTRACE=1
```

→ **不过**：protoc 缺失（判红项）；两个 codegen 插件缺失（只记录，但也说明环境脚本的插件那步没跑）。rustc / go 版本对。
`rustc` 在 PATH 上（`/root/.cargo/bin` 已在 PATH），`check.sh:9` 不需要补 PATH。

### 第 4 步：check.sh（冷启动）

冷编证据（跑 check.sh 之前）：

```
$ pwd; ls -d core/target/debug 2>&1; du -sh ~/.cargo/registry 2>&1
/home/user/aite
ls: cannot access 'core/target/debug': No such file or directory
du: cannot access '/root/.cargo/registry': No such file or directory
```

→ 会话首编是冷编译（连 registry 都没有，setup 的预热没发生——本来就没跑 setup）。

`{ time scripts/check.sh; } > /tmp/cc1-check-open.log 2>&1; echo "exit=$?" >> /tmp/cc1-check-open.log`，日志全文：

```

=== A1 cargo build --workspace ===
$ bash -c cd core && cargo build --workspace
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/events.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/outbound.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/sandbox.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/edge.proto

  --- stderr
  Error: Custom { kind: NotFound, error: "Could not find `protoc`. If `protoc` is installed, try setting the `PROTOC` environment variable to the path of the `protoc` binary. To install it on Debian, run `apt-get install protobuf-compiler`. It is also available at https://github.com/protocolbuffers/protobuf/releases  For more information: https://docs.rs/prost-build/#sourcing-protoc" }
warning: build failed, waiting for other jobs to finish...
-> exit 101  ✗

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
scripts/check.sh: line 18: core/target/debug/aite: No such file or directory
-> exit 127  ✗

=== A4a cargo clippy -D warnings ===
$ bash -c cd core && cargo clippy --workspace --all-targets -- -D warnings
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/events.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/outbound.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/sandbox.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/edge.proto

  --- stderr
  Error: Custom { kind: NotFound, error: "Could not find `protoc`. If `protoc` is installed, try setting the `PROTOC` environment variable to the path of the `protoc` binary. To install it on Debian, run `apt-get install protobuf-compiler`. It is also available at https://github.com/protocolbuffers/protobuf/releases  For more information: https://docs.rs/prost-build/#sourcing-protoc" }
warning: build failed, waiting for other jobs to finish...
-> exit 101  ✗

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
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/events.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/outbound.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/sandbox.proto
  cargo:rerun-if-changed=/home/user/aite/core/crates/proto/../../../proto/aite/v1/edge.proto

  --- stderr
  Error: Custom { kind: NotFound, error: "Could not find `protoc`. If `protoc` is installed, try setting the `PROTOC` environment variable to the path of the `protoc` binary. To install it on Debian, run `apt-get install protobuf-compiler`. It is also available at https://github.com/protocolbuffers/protobuf/releases  For more information: https://docs.rs/prost-build/#sourcing-protoc" }
warning: build failed, waiting for other jobs to finish...
-> exit 101  ✗

=== C1 契约测试 ===
$ bash -c cd core && cargo test -p aite-contracts 2>&1 | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"contracts passed=\" p \" failed=\" f; exit (f>0)}"
contracts passed=25 failed=0
-> exit 0

=== B 全量 cargo test ===
$ bash -c cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); printf "%s\n" "$o" | grep -E "^error: test failed" | sort -u | head -n 5; printf "%s\n" "$o" | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p \" failed=\" f; exit (f>0)}"
cargo passed= failed=
-> exit 0

=== B 全量 go test（-race） ===
$ bash -c cd edge && go test -race ./... -count=1
?   	aite/edge/gen/aitepb	[no test files]
ok  	aite/edge/internal/aiteerr	1.016s
ok  	aite/edge/internal/config	1.027s
ok  	aite/edge/internal/feishu	4.610s
ok  	aite/edge/internal/ingress	1.309s
?   	aite/edge/internal/pin	[no test files]
ok  	aite/edge/internal/sandbox	1.025s
ok  	aite/edge/internal/server	1.034s
-> exit 0

=== B8 评测（passed 10/10） ===
$ bash -c o=$(core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | grep -qx "passed 10/10"
bash: line 1: core/target/debug/aite: No such file or directory
-> exit 1  ✗

以下没过：A1 cargo build --workspace A3/C2 契约锁 --check A4a cargo clippy -D warnings A5 cargo test --no-run（全部测试可编译） B8 评测（passed 10/10）

real	3m49.720s
user	10m3.462s
sys	2m5.103s
exit=1
```

冷启动墙钟 `real 3m49.720s`——不能当「冷编」数据点：A1 / A4a / A5 / B 都在 `aite-proto` 的 build.rs 处早早失败，全量编译根本没跑完。

对照基线行（情形 A）：

| 基线行 | 期望 | 实测 |
|---|---|---|
| `cargo passed=… failed=…` | `cargo passed=897 failed=0` | `cargo passed= failed=`（没编出来，空值） |
| `contracts passed=… failed=…` | `contracts passed=25 failed=0` | `contracts passed=25 failed=0` ✔（`aite-contracts` 不依赖 `aite-proto`，单独编得过） |
| 契约锁 `OK 25 files` | `OK 25 files` | 没有（`core/target/debug/aite` 不存在，exit 127） |
| B8 `passed 10/10` | `passed 10/10` | 没有（同上，exit 1） |
| Go 9 包全 ok | 9 | ✔，见下 |

Go 那一格被截掉的包单跑、以及包数：

```
$ (cd edge && go test -race ./cmd/... -count=1 2>&1)
ok  	aite/edge/cmd/aite-edge	5.531s
cmd exit=0
$ (cd edge && go test -race ./... -count=1 2>&1) | grep -cE '^(ok|\?)'
9
```

**顺手发现的 check.sh 缺陷（本轨工作项 2 的面，闸门过了之后由重派的 CC1 修）**：「B 全量 cargo test」那格在**整个工作区编不过**时是假绿——
`cargo test` 编译失败时没有任何 `^test result` 行，awk 的 `f` 是空串，`exit (f>0)` 得 0；`grep -E "^error: test failed"` 也只认测试失败、不认编译失败。
所以上面那格打出 `cargo passed= failed=` 却 `-> exit 0`。今天有 A1 / A5 兜着，整体仍是红的；但这格单独拿出来看（或 A1 / A5 哪天被拿掉、改动）就会放行一个编不过的工作区。
另一种漏法：只要**有一部分** crate 的测试编出来跑了，打出来的是部分计数、仍是 `-> exit 0`，只有逐字对照基线条数才看得出来。
建议修法：捕获 `cargo test` 自身的退出码（照 B8 那格 `c=$?`），并要求 `p` 非空（awk 里 `exit (f>0 || p=="")`）。

### 环境与网络（只记录）

```
$ uname -m; nproc; free -g; df -h .
x86_64
4
               total        used        free      shared  buff/cache   available
Mem:              15           0          13           0           1          15
Swap:              0           0           0
Filesystem      Size  Used Avail Use% Mounted on
/dev/vda        252G  8.1G   30G  22% /
$ docker info --format '{{.ServerVersion}}'

failed to connect to the docker API at unix:///var/run/docker.sock; check if the path is correct and if the daemon is running: dial unix /var/run/docker.sock: connect: no such file or directory
docker exit=1
$ which docker dockerd containerd
/usr/bin/docker
/usr/bin/dockerd
/usr/bin/containerd
$ for h in deb.debian.org security.debian.org auth.docker.io production.cloudflare.docker.com pypi.org goproxy.cn; do curl -sSI --max-time 8 "https://$h/" >/dev/null; echo "$h exit=$?"; done
curl: (56) CONNECT tunnel failed, response 403
deb.debian.org exit=56
curl: (56) CONNECT tunnel failed, response 403
security.debian.org exit=56
auth.docker.io exit=0
production.cloudflare.docker.com exit=0
pypi.org exit=0
curl: (56) CONNECT tunnel failed, response 403
goproxy.cn exit=56
```

代理状态（`$HTTPS_PROXY/__agentproxy/status` 的 `recentRelayFailures`）：三条都是 `connect_rejected`，
`gateway answered 403 to CONNECT (policy denial or upstream failure)`，主机 `deb.debian.org:443`、`security.debian.org:443`、`goproxy.cn:443`。

## 2–6. 工作项 / 验证 / check.sh 终版 / cargo 增量 / Cargo.lock diff

没做：闸门没过，按派单停在工作项 0。

## 7. 被守卫拦过的命令与拦截原文

无（第 2 步那条除外，已在第 1 节）。

## 8. 记账转出去的

无（没开工）。

## 9. 没做的与原因

工作项 1–8 全部没做：开场自检第 3 步 protoc 缺失、第 4 步 check.sh 没过，根因是 H3 的环境设置（Setup script、网络白名单、`GOTOOLCHAIN`）在这个会话的环境里没生效。
派单明令「protoc / go 缺了不要自己装」，所以本会话没有手动装 protoc、没有跑 `bash scripts/cloud-setup.sh`（那份脚本也还没入库）。

## 10. 要总管贴回环境设置的改动

无（脚本还没入库、也没跑过；上面「要总管做的」是把计划 §4.1 原样贴进环境，不是改动）。

## 11. 契约缺口

无。
