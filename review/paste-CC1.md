# 派单 CC1：云端基建 · 基线 · 基建 R0 主人 · 镜像构建参数 · 骨架 crate（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC1.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC1）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。
>
> **你是第 1 波的闸门。** T0 与 CC2–CC12 要等你的开场自检贴出「`check.sh` 全部通过 + Read 守卫被拦 + 工具链版本」才派（计划 §4.3）。
> 所以自检一做完就**先推回执第一节、开 draft PR**（§5 工作项 0），再干别的。自检没过也照样推，写清哪一步没对上，然后停。

## 1. 背景

对齐项：CT10（沙箱镜像国内可建、离线可建）、CT18（网络级别里「国内镜像档」的构建侧：`BASE_REGISTRY` 让大陆构建期不直连 Docker Hub，外加 apt / crates / Go / protoc 的镜像参数）、CT25（为扩展类功能预建 crate）。
CC1 不做产品功能，做的是之后每一轨都要站的地基，并且是本波这些文件的唯一 R0 主人（计划 §7）：
`scripts/check.sh`、`scripts/cloud-setup.sh`、`Makefile`、`.github/**`、`docker-compose.yml`、`docker/core|edge/**`、
`core/Cargo.toml`、`core/Cargo.lock`、`core/crates/app/Cargo.toml`、`CLAUDE.md` 的「云端基线」。

今天的样子（都在 `98e4460` 上核过）：

1. **安装脚本没入库**：全文只在计划 §4.1（计划第 288–340 行）；`CLAUDE.md`「## 环境」写着「CC1 落地前以总计划 §4.1 里那份为准」。
2. **Go 那一格看不全**：`scripts/check.sh:15-21` 的 `run()` 只留最后 8 行（`:19`）。`:41` 的 `go test -race ./...` 打 9 行：
   7 个 `ok` + 2 个 `? … [no test files]`（`gen/aitepb`、`internal/pin`），排第一的 `aite/edge/cmd/aite-edge` 永远被截掉，
   红了也看不到名字。`:11-12` 只认 `$1 == --quick`；没有 `evals/p1` 的门，也没有 docker 那组。
3. **真容器那组哪都不跑**：`edge/internal/sandbox/docker_test.go:1` 是 `//go:build docker`；`ci.yml:55` 写明「不含 docker tag」，
   `ci.yml:133-135` 刻意不建沙箱镜像；`Makefile` 只有 `docker-image`（`:50-51`），没有 `docker-test` / `evals-p1` / `cloud-setup`。
4. **镜像构建全走海外上游**：四个 `FROM`（`docker/core/Dockerfile:16`、`:75`，`docker/edge/Dockerfile:11`、`:54`）直连 Docker Hub；`docker/core/Dockerfile:34-49` 从 GitHub release 拉 protoc（URL 在 `:45-46`，架构由 `:35-41` 的 TARGETARCH 映射）；
   apt 在 `:42-44`（builder）与 `:85-87`（运行层）、`docker/edge/Dockerfile:59-61`；`docker/core/Dockerfile:68-72` 是 `cargo build --workspace --locked`（crates.io）。
   `docker/edge/Dockerfile:13-31` 有 BB3 留的 `GOPROXY` / `GOSUMDB` ARG，但 `Makefile:62-63` 的 `compose-build` 不透传，只能手敲（`README.md:104-117`）。
5. **新 crate 没地方放**：W2/W3 要 5 个新 crate；各轨自己建的话 `core/Cargo.toml:13-25`、`core/crates/app/Cargo.toml:19-30`、
   `core/Cargo.lock`（`:28-57` 的 `aite` 包）会被五六轨同时改。计划 §6.4 规定 W3 **谁都不改 `Cargo.toml` / `Cargo.lock`**，靠你预先声明。
6. `CLAUDE.md`「## 云端基线」一节是空的；「## 验收」里那句「check.sh 那一格只显示 8 行」会被你的改动变成过时描述。

谁吃你的产出：总管（你的自检 = 第 1 波闸门）；之后每份派单的开场自检第 4 步改认 `go packages ok=N fail=M`（计划 §4.4 末句）；
T0c 期望 `B9 passed 3/3`、`go packages ok=12 fail=0`（CC7 的 3 个 p1 场景 + CC9/CC10/CC11 各一个新 Go 包）且 PR CI 含 `sandbox-docker`；
DD12 在 `aite-admin` 上加 axum（那时它改 `Cargo.toml`/`Cargo.lock`）；EE1 / EE2 / EE4 / EE5 只写 `core/crates/{memory,routines,search,githost}/**`；
EE14 接手 `docker/core|edge`、`Makefile`、`.github`，并按你留下的镜像参数（含 `BASE_REGISTRY` 的取值）写部署文档。

## 2. 必读（按顺序）

1. `CLAUDE.md` 全文（「## 守卫」「## 验收」「## 规则」「## 云端基线」四节；它未入库、D0 才提交，行号可能漂，按节名找）。
2. 计划 §4.1（第 275–344 行，安装脚本原文在 288–340）、§4.3–§4.5（第 355–407 行）、§5.1（第 431–445 行，P0-CLOSE 文件）、
   §6 开头规则（第 500–512 行）、§6.1 CC1 行（第 521 行）、§7（第 604–631 行）。行号是生成时的，D0 可能再挪：以 `grep -n '^### 4.4' review/plan-2026-09-25-claude-tag-parity.md` 这类按标题找为准。
3. 要改的文件：`scripts/check.sh` 全文（49 行）；`Makefile:11-18`（变量与 `.PHONY`）、`:47-63`、`:142-143`；
   `.github/workflows/ci.yml:14-60`（checks）、`:72-137`（compose-smoke 前半）、`:277-281`（收尾断言）；
   `docker/core/Dockerfile:14-49`、`:67-87`；`docker/edge/Dockerfile:9-31`、`:54-61`；`docker-compose.yml:57-63`、`:155-160`、`:205-217`；
   `core/Cargo.toml:3-27`；`core/crates/app/Cargo.toml:19-47`；`core/Cargo.lock:28-57`（`aite` 包）与 `:157-169`（`aite-models`，当样板）；`.gitignore` 全文。
4. 钉行为的东西：`core/crates/evals/src/scenario.rs:478-489`（场景 `name` 必须等于文件名）、`:509-535`（目录不存在 `:510-515`、
   没有 yaml `:527-532` 都**报错**；`:520-525` 同时认 `.yaml` / `.yml`）；`core/crates/evals/src/cli.rs:488`（汇总行 `passed {}/{}`）；
   `edge/internal/sandbox/docker_test.go:1-12`（docker tag、断言按 task_id 过滤）；`core/crates/contracts/tests/layout.rs:11-50`（只查存在，不挡新 crate）；
   crate 写法样板 `core/crates/models/Cargo.toml` 与 `core/crates/models/src/lib.rs:1-18`。

## 3. 工作区

- 分支：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC1: 云端基建、基线与骨架 crate」。
- 代码基线：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。CC1 最先派，预期情形 A。
- **可写面**（逐字来自原卡）：`scripts/cloud-setup.sh`（新建）· `scripts/check.sh` · `Makefile` · `.github/**` · `.gitignore` ·
  `docker/core/**` · `docker/edge/**` · `docker-compose.yml` · `docs/p1/cloud-runbook.md`（新建；`docs/p1/` 目录也是新的）· `CLAUDE.md` ·
  `core/Cargo.toml` · `core/Cargo.lock` · `core/crates/app/Cargo.toml` · `core/crates/{admin,search,githost,routines,memory}/**`（五个都新建）·
  `review/p1/ledger/CC1.md`（新建；目录 `review/p1/ledger/` 由 D0 带着 `.gitkeep` 提交，clone 里已有）。
- **只读面**：其余一切。特别点名：
  - P0-CLOSE 的 14 个路径（总管在你跑的同时本机打；与开场自检第 1 步情形 B 那张清单是同一组）：
    AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
    BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
    `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
    `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
    BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`。（外加整片 `core/crates/evidence/**`、`.claude/**` 都不碰。）
  - T0 补丁文件（计划 §5.2 的穷举范围，只有总管本机打）：`proto/aite/v1/*.proto`、`core/crates/contracts/**`、`core/crates/proto/**`、
    `edge/internal/server/server.go`、`config/aite.example.yaml`（`ci.yml:97-101` 读它，你只读）；外加 `edge/gen/**`（只有总管本机 `make proto-gen`）。
  - 离你最近的别轨：CC12 的 `docker/sandbox/**` 与 `edge/internal/sandbox/docker_test.go`（你**建它、跑它，不改它**；CC12 在沙箱镜像里也加同名同语义的 `APT_MIRROR` 与 `BASE_REGISTRY`）；
    CC7 的 `core/crates/evals/**`、`core/crates/testing/**`、`evals/p1/**`（B9 只读它；§5 的临时探针用完即删、永不提交）；
    CC4 的 `core/crates/app/src/{app,wiring,lib}.rs` 与 `app/src/features/**`（你只动 `app/Cargo.toml`，不在 app 代码里 `use` 新 crate，不建 `features/*.rs`）；
    T0 的 `docs/p1/contract-p1.md`（同目录不同文件）；CC9/CC10/CC11 的 `edge/internal/{dingtalk,wecom,egress}/**`（它们合并后 Go 包数变 12）；
    还有 `README.md`、`.dockerignore`、`evals/p0/**`。
- **本轨解冻的冻结项**：无（计划 §9 没有 CC1 条目）。`scripts/check.sh:43-45` 的 B8 那一格**逐字不动**。

## 4. 开场自检（全部对上才开工；对不上就写回执停下）

照计划 §4.4 原样：

1. **代码基线**：先 `git cat-file -e 98e4460 || git fetch -q --unshallow origin`（浅 clone 时补历史），再
   `git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'`
   - 输出为空 → 情形 A：基线行 = `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok；
   - 恰好列出下面 14 个 P0-CLOSE 路径、一个不多 → 情形 B：基线行 = `cargo passed=901 failed=0`、`contracts passed=27 failed=0`、`OK 25 files`、`passed 10/10`（以实测为准，差异逐条解释）。
     14 个路径：AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
     BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
     `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
     `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
     BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；
   - 别的 → 停下，写回执。（D0 的改动全是删除或落在排除的路径里，所以情形 A 输出为空；`--no-renames` 让「挪文件」也现形。）
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（把拦截原文贴进回执）。
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认。
   CC1 合并之后 check.sh 会多打一行 `go packages ok=N fail=M`，从那以后以这行为准（这一行正是你要加的）。

CC1 补充（结果一起进回执第一节）：

- **Bash 的 cwd 会跨调用保留**：上面第 3 步那几条带 `cd` 的命令各用子 shell 跑（`(cd core && rustc --version)`、`(cd edge && go version)`），
  否则第二条会去找 `core/edge`、第 4 步找不到 `scripts/check.sh`。下面所有带 `cd` 的命令同理。
- 第 1 步前先 `git log --oneline -3`，记 B0 sha（只作记录，不钉）。
- 第 2 步：期望看到形如 `blocked: 该操作触碰受保护面 …` 的原文。**只有这一次被拦是期望结果**，拦截文里「停止当前工作」对这一条不适用；其它任何被拦都按 §6 停。
- 第 3 步的两个 codegen 插件 CC1 也要跑、也要记，但**只记录不判红**：对不上就在回执第一节与 PR 描述首段点名（T0 的 --codegen 自测要它们，
  那说明环境脚本的插件那步没装上）。
- 第 3 步若 `rustc: command not found` 但 `~/.cargo/bin/cargo` 存在：`export PATH="$HOME/.cargo/bin:$PATH"` 后重试并记下
  （`check.sh:9` 的 PATH 行归你，工作项 2 里补上）。protoc / go 缺了不要自己装——那说明 H3 的环境脚本没跑，写回执、推 PR、停。
- 第 4 步之前先取「会话首编是否仍是冷编」的证据（计划 §4.1 脚本第 ④ 步注释要你实测）：`pwd; ls -d core/target/debug 2>&1; du -sh ~/.cargo/registry 2>&1`
  （target 不在 = setup 的预编译没留到这份 checkout）；check.sh 跑完看 A1 那格：有 `Compiling` 行、`Finished … in` 是分钟级 = 冷编，
  只有 `Finished … in 0.xs` = 热（`run()` 只留 8 行，数不全 `Compiling`，按 `Finished` 行的耗时判）。
- 第 4 步冷编译约 10–15 分钟，超过 Bash 单条 600 秒上限，前台跑会被掐断（闸门判不了、墙钟也记错）：用 Bash 工具的 `run_in_background` 后台跑
  `{ time scripts/check.sh; } > /tmp/cc1-check-open.log 2>&1; echo "exit=$?" >> /tmp/cc1-check-open.log`
  （`time` 不是管道；`{ …; }` 让 `time` 的 real/user/sys 也进日志），跑完用 Read 工具读日志全文（仍不接 `| tail`）：
  有「全部通过」、其后是 `time` 的三行、末行 `exit=0`；`real` 那行记为「冷启动」墙钟数据点。另跑一次
  `(cd edge && go test -race ./... -count=1 2>&1) | grep -cE '^(ok|\?)'` → `9`（`ok` 行加 `[no test files]` 行）。
- 环境与网络（不判红，只记录）：`uname -m; nproc; free -g; df -h .`；`docker info --format '{{.ServerVersion}}'`（失败就逐字记错误）；
  可达性一行跑完：`for h in deb.debian.org security.debian.org auth.docker.io production.cloudflare.docker.com pypi.org goproxy.cn; do curl -sSI --max-time 8 "https://$h/" >/dev/null; echo "$h exit=$?"; done`

## 5. 工作项

**0. 自检先上墙（闸门，最先做）。** 用 Write 工具写 `review/p1/ledger/CC1.md`，只填「开场自检原文」一节，顶上放这 5 行：
`check.sh：全部通过（逐行贴）` / `守卫：Read .claude/hooks/guard_bash.py 被拦（原文：…）` / `工具链：libprotoc … / rustc … / go… / protoc-gen-go … / protoc-gen-go-grpc …` /
`情形：A（或 B），B0=<sha>` / `可达性：6 个主机各自 exit 码`。提交（中文信息，如 `cc1: 开场自检回执`）→ 推 → 开 draft PR，
PR 描述贴同样 5 行。**PR 描述先用 Write 写成 `/tmp/cc1-pr.md`**，再 `gh pr create --draft --title "CC1: 云端基建、基线与骨架 crate" --body-file /tmp/cc1-pr.md`：
描述里有守卫文件名，直接写进 `gh` 命令或 heredoc 会被守卫拦。收尾时同样用 `gh pr edit --body-file` 换成完整摘要。

**1. `scripts/cloud-setup.sh`（新建，mode 100755）。** 内容 = 计划 §4.1 代码块**逐字**（今天在计划第 288–340 行，围栏在 287 / 341；D0 可能让行号漂，所以按内容锚点核：
`sed -n '/^#!\/usr\/bin\/env bash$/,/^echo "cloud-setup 完成"$/p' review/plan-2026-09-25-claude-tag-parity.md | diff - scripts/cloud-setup.sh` → 无输出）。
照抄的是**修订后的** §4.1（原卡 [REVISION 2026-09-25]）：缺 `unzip` 先装、protoc 严格判 `libprotoc 31.` 且装完 `hash -r` 再判、`protoc-gen-go` / `protoc-gen-go-grpc`
软链进 `/usr/local/bin`、第 ④ 步注释要你实测冷编——你手里那份计划若没有这几处，说明 clone 到的不是 D0 之后的 main，停下写回执。用 Write 工具落文件，别用 heredoc；
Write 不带执行位，接着 `chmod +x scripts/cloud-setup.sh`，提交后 `git ls-files -s scripts/cloud-setup.sh` 以 `100755` 开头（同 `check.sh`）。
连跑两次。脚本第 ④ 步有 `cargo test --workspace --no-run`，冷跑同样 10–15 分钟、超过 Bash 单条 600 秒：两次都用 `run_in_background` 后台跑，一次跑完再起下一次——
`{ time bash scripts/cloud-setup.sh; } > /tmp/cc1-setup-1.log 2>&1; echo "exit=$?" >> /tmp/cc1-setup-1.log`，第二次写 `/tmp/cc1-setup-2.log`；
每次跑完用 Read 工具读日志全文（不接 `| tail`）→ 两次 `exit=0`、脚本输出末行 `cloud-setup 完成`（其后是 `time` 的三行与 `exit=` 行），记两次 `real` 墙钟与所有 `WARN:` 行。
真发现脚本有 bug 必须改：改仓库这份，回执单列「要总管贴回环境设置的改动」（贴回会让约 7 天的环境缓存失效）。

**2. `scripts/check.sh`。** 四处改动，B8 不动：
- **Go 汇总行**：改 `:41` 那一格，输出顺序照 `:34-37` cargo 行的理由——**先打失败包名（最多 5 个），末行打计数**：`go packages ok=N fail=M`。
  口径写进注释：N = `^ok` 行 + `^?` 行（`[no test files]` 也算，所以今天是 9；T0c 按同一口径期望 12），M = `^FAIL[[:space:]]+aite/edge/` 行（含 `[build failed]`）；
  逐包的 `ok` 行不再显示——与 cargo 行（`:37`）同样的取舍，9 行原文加汇总本来也塞不进 8 行，别去放宽 `run()` 的 tail；
  这一格的退出码必须保留 go test 的退出码（照 B8 那格，捕获输出后立刻 `c=$?`，最后按它退出）。
- **B9 门**：B8 之后加一格。`evals/p1` 下没有 `*.yaml` → 打一行 `B9 skip：evals/p1 尚无场景`，**不进 FAILED**；
  必须先在 shell 里判空（`ls evals/p1/*.yaml >/dev/null 2>&1`），因为 `load_suite` 对「目录不存在 / 没有 yaml」都报错（`scenario.rs:510-515`、`:527-532`）。
  有场景 → `core/target/debug/aite evals run evals/p1 --platform fake --model scripted`，退出码 0 **且**末行是 `passed k/k`（两数相等；用 awk
  `$1=="passed" && split($2,a,"/")==2 && a[1]==a[2]`，别用 `grep -E` 反向引用）。`.yml` 认不认，与 `scenario.rs:520-525` 对齐与否写进回执。
- **`--docker`**：`:11-12` 改成 `for a in "$@"` 循环，`--quick` 与 `--docker` 可同时给。`--docker` 时多跑一格：建 `aite-sandbox:p0` →
  `cd edge && go test -tags docker ./internal/sandbox/... -count=1` → 断言 `docker ps -a --filter label=aite.task -q | wc -l` 为 0。与工作项 3 的 `docker-test` 同口径。
- **PATH**：自检第 3 步若要补 `$HOME/.cargo/bin`，加在 `:9`。
- **验证（每格「绿 → 红 → 绿」三段输出进回执）**：
  - Go 行：临时建 `edge/cmd/aite-edge/zz_cc1_probe_test.go`（`package main`，一个 `t.Fatal("cc1 probe")` 的测试；写完先 `gofmt -l edge/cmd/aite-edge` 为空，
    否则 A4d gofmt 也红、「以下没过」会列两格）→ `scripts/check.sh --quick`
    必须出现 `aite/edge/cmd/aite-edge` 与 `go packages ok=8 fail=1`、末行「以下没过」只列 go test 那一格；删探针 → `ok=9 fail=0`。这正是今天被截掉的那个包。
  - B9：`mkdir -p evals/p1 && sed 's/^name: 01_simple_qa$/name: zz_cc1_probe/' evals/p0/01_simple_qa.yaml > evals/p1/zz_cc1_probe.yaml` → B9 `passed 1/1`；
    再把 `method: send_text, equals: 1}` 改成 `equals: 2}` → B9 红、check.sh 退 1；最后 `rm evals/p1/zz_cc1_probe.yaml && rmdir evals/p1`。
  - `--docker`：正常一次；再 `DOCKER_HOST=unix:///nonexistent scripts/check.sh --quick --docker` → docker 那格红。
  - 收尾 `git status --short` 里不许有探针。`git diff origin/main -- scripts/check.sh` 的 `-` 行里不许出现 B8 那三行（原 `:43-45`）。

**3. `Makefile`。** 新 target 带 `## ` 说明（`make help` 要列得出），补进 `:17-18` 的 `.PHONY`：
`cloud-setup`（`bash scripts/cloud-setup.sh`）、`docker-test`（建沙箱镜像 + `go test -tags docker ./internal/sandbox/... -count=1` + 遗留容器数为 0 的断言，
写法对齐 `ci.yml:281`，Makefile 里 `$` 要写成 `$$`）、`evals-p1`（依赖 `build`，跑 `evals/p1`，没场景就打 skip 退 0）。
`docker-test` 写成 `docker-test: docker-image`（复用现有 `:50-51` 那个 target 建镜像，不另写一遍 `docker build`）。
`compose-build`（`:62-63`）透传五个构建参数，见工作项 5。**别跑、别改 `proto-gen`（`:47-48`）。**

**4. CI（`.github/workflows/ci.yml`）。** `on:`（`:4-8`）不动。
- 新 job `sandbox-docker`（与 checks、compose-smoke 并列）：checkout → setup-go 照抄 `:28-31` → `docker build -t aite-sandbox:p0 docker/sandbox` →
  `cd edge && go test -tags docker ./internal/sandbox/... -count=1` → `if: always()` 的收尾步照 `:281` 断言没有 `aite.task` 容器。`timeout-minutes` 给 25。
- checks job 在 B8（`:59-60`）后加 B9 步：与 check.sh 同一个判空条件，没场景打 skip，有场景要求 `passed k/k`。
- 这份文件 `:31` 有 Go 模块文件的字面量：**只用 Edit 工具改**，Bash 里不要 grep / sed / cat 那个字面量。
  同样带受保护字面量的还有：`docker/edge/Dockerfile:47`（Go 模块文件）、`docker/core/Dockerfile:27` 与 `Makefile:6`（`core/crates/proto` 下的 build.rs）、
  `Makefile:47`（重锁变量赋值）、`CLAUDE.md`（多处）——这几份也一律只用 Edit 工具改。

**5. 国内镜像构建参数（`docker/core/Dockerfile`、`docker/edge/Dockerfile`，经 `make compose-build` 透传）。** 五个名字：`BASE_REGISTRY`、`GOPROXY`、`PROTOC_URL`、`CARGO_REGISTRY`、`APT_MIRROR`。
- **默认值 = 上游，且不传时与今天逐字等价**：CI 的 compose-smoke 直接 `docker compose build`（`ci.yml:137`），不经 make，那条路一个字节都不能变
  （纪律同 `docker/edge/Dockerfile:14-17`、`:28-29`）。国内值（私有 / 内网镜像仓库、goproxy.cn、tuna/aliyun 的 apt 镜像、ustc 的 sparse 源、内网制品库里的 protoc）
  **只写在注释与 `docs/p1/cloud-runbook.md` 里，不当默认值**。
- `BASE_REGISTRY`（默认 `docker.io`）：两份 Dockerfile 在第一个 `FROM` 之前声明全局 `ARG BASE_REGISTRY=docker.io`（core 挨着 `:15` 的 `ARG RUST_VERSION`，
  edge 挨着 `:10` 的 `ARG GO_VERSION`），四个 `FROM` 全改成 `${BASE_REGISTRY}/library/…`：core `:16` `rust:${RUST_VERSION}-trixie`、`:75` `debian:trixie-slim`；
  edge `:11` `golang:${GO_VERSION}-trixie`、`:54` `debian:trixie-slim`。写法与 CC12 的 `${BASE_REGISTRY}/library/python:3.11-slim` 一致；默认值下与今天是同一个镜像
  （CI 的 `docker compose build` 不变）。它只在 `FROM` 行里用，所以**只要这一条全局声明，不必也不该在各阶段里重新声明**（与下面 `APT_MIRROR` 不同）。
  大陆取值（私有或内网镜像仓库）写进 runbook，EE14 在 deploy.md 统一。
- `make -n compose-build` 必须**打印出**五个 `--build-arg`，所以 Makefile 要有 `GOPROXY ?= …` 之类的变量并总是传。注意 make 会把同名环境变量读进来
  （云端若设了 `GOPROXY` 会被带进去）——命名与取舍写进回执。若另在 `docker-compose.yml` 的 `build.args` 里写 `${X:-}`，空串会**盖掉** Dockerfile 默认值：
  三处（Makefile / compose / Dockerfile）默认值必须一致，或 Dockerfile 把空串当「用上游」处理。`BASE_REGISTRY` 三处默认都是 `docker.io`，`APT_MIRROR` 三处默认都是空。
  注意 `make compose-build` 是 `docker compose --profile images build`，全局 `--build-arg` **也会发给 `sandbox-image`**（`docker-compose.yml:210-217`，建的是 CC12 的
  Dockerfile），所以 `BASE_REGISTRY` / `APT_MIRROR` 的口径必须与 CC12 一致，否则一次 `make compose-build APT_MIRROR=…` 会弄坏其中一边。
- `APT_MIRROR`：取值口径与 CC12 一致：`ARG APT_MIRROR=`，**空 = 上游、不动 sources**；非空 = 镜像**主机名**（如 `mirrors.tuna.tsinghua.edu.cn`，不是 URL），
  把 `debian.sources` 里的 `deb.debian.org`（以及若出现的 `security.debian.org`）都换成它，`debian-security` 那条也要落到镜像上。Makefile 里 `APT_MIRROR ?=` 默认留空。
  trixie 镜像用的是 deb822 的 `/etc/apt/sources.list.d/debian.sources`，不是 `sources.list`。先
  `docker run --rm debian:trixie-slim cat /etc/apt/sources.list.d/debian.sources` 看清格式再写替换；三个 apt 点都要覆盖（core builder `:42-44`、core 运行层 `:85-87`、edge 运行层 `:59-61`），
  每个 `FROM` 之后重新声明 `ARG`（照 `docker/core/Dockerfile:15`/`:24` 的写法）。
- `PROTOC_URL`：定义成**前缀**（默认 `https://github.com/protocolbuffers/protobuf/releases/download`），版本与 `${arch}` 仍按 `:34-41` 拼，别把 arm64 弄坏（EE14 要多架构）。
- `CARGO_REGISTRY`：非空时在 builder 里写 cargo 配置做 `[source.crates-io] replace-with`（sparse 源）；这种源替换对 `:71` 的 `--locked` 透明，`Cargo.lock` 不变。
  国内值以 ustc 为准（`sparse+https://mirrors.ustc.edu.cn/crates.io-index/`，计划 CT18）；rsproxy 标「待大陆实测后再列」。
- `GOPROXY`：就是 BB3 那个 ARG（`docker/edge/Dockerfile:30`），这里只补透传。
- **验证**：`make -n compose-build` 看五个参数；每个参数给一个不存在的值单建一次（如 `docker compose build --build-arg CARGO_REGISTRY=sparse+https://invalid.example/index/ core`、
  `docker compose build --build-arg BASE_REGISTRY=invalid.example core`——后者必须红在 `FROM`），
  必须红在对应那一步（证明真被消费了），再用默认值建绿。要看 `docker compose config` 一律加 `Makefile:56` 的 `COMPOSE_NOSECRET` 前缀（完整输出带密钥）。
  云端 docker 不可用就把这组标「交 CI 判」，写进「没做的」。

**6. 五个空骨架 crate。** `core/crates/{admin,search,githost,routines,memory}`，包名 `aite-admin` 等，`version.workspace = true` 等四行照 `models/Cargo.toml` 抄。
- `src/lib.rs` 只有 `//!` 文档注释（这个 crate 以后装什么、归哪轨、依赖为什么预声明）。**注释里别放 ``` 代码块**——会变成 doctest，`cargo passed` 就不是 897 了；
  `//!` 里也别用 4 空格缩进的行（同样会成 doctest），列表续行别写成懒续行（clippy 的 `doc_lazy_continuation` 默认告警，`-D warnings` 下就是错）。
  `--quick` 跳过 clippy，所以写完 lib.rs 要跑一次**不带** `--quick` 的 check.sh，确认 A4a clippy 那格绿。
- 依赖**只从现有 `[workspace.dependencies]` 取**（它们都已在 `Cargo.lock` 里，不会引入第三方新包），一律 `xxx.workspace = true`、不额外加 feature；
  每个 `Cargo.toml` 照 `app/Cargo.toml:38-47` 的样子写一段理由注释。建议起点（依据是各消费轨原卡，你可以增删，逐 crate 在回执写理由）：

  | crate | 消费轨 | `[dependencies]` | `[dev-dependencies]` |
  |---|---|---|---|
  | admin | DD12（W2，届时它自己加 axum、改锁，所以这份宁少勿多） | aite-contracts, serde, serde_json, tokio, async-trait, thiserror, tracing, chrono | tokio, tempfile |
  | memory | EE1（记忆工具 = GatewayTool） | aite-contracts, aite-gateway, serde, serde_json, tokio, async-trait, thiserror, tracing, chrono | tokio, tempfile, aite-testing |
  | routines | EE2（next_run、调度循环、例程工具） | aite-contracts, aite-gateway, chrono, serde, serde_json, tokio, async-trait, thiserror, tracing | tokio, aite-testing |
  | search | EE4（服务端 web_search 走 HTTP） | aite-contracts, aite-gateway, reqwest, serde, serde_json, tokio, async-trait, thiserror, tracing | tokio, aite-testing |
  | githost | EE5（GitLab API、提交 API、Draft MR） | aite-contracts, aite-gateway, reqwest, serde, serde_json, tokio, async-trait, thiserror, tracing, sha2, hex | tokio, tempfile, aite-testing |

  **`aite-gateway` 这一列是定死的（原卡 [REVISION 2026-09-25]、计划 §6.1 CC1 行），不在「可增删」之内**：memory / routines / search / githost 四个都必须预声明它，
  admin 不声明——因为 CC4 把公开的 `GatewayTool` trait 放在 gateway crate 里、这四个 crate 都要注册工具，W3 又不许改锁。
  同一条修订要求改 `core/Cargo.toml:14` 那句注释「各轨只许依赖 contracts / proto / testing；跨轨依赖 → 停下报告」（P0 时代的口径）：
  改成「contracts / proto / testing 之外，memory / routines / search / githost 四个骨架 crate 还允许依赖 aite-gateway；其余跨轨依赖 → 停下报告」这个意思（措辞你定，
  中文、只改这一行注释；它挨着 `[workspace.dependencies]` 的 aite-* 条目，用 Edit 工具改）。dev-dependencies 也进 `Cargo.lock`，所以一并预声明。你判断某个 W3 轨还需要**别的 crate 反过来依赖骨架 crate**（例如 worker 依赖 aite-memory），
  不在本轨做，写进「记账转出去的」。
  原卡点名的 `rusqlite`（以及 `uuid`、`sha2` / `hex`，都在 `core/Cargo.toml:36-38`、`:46`）逐 crate 判一次要不要预声明——例如 EE4 的本地消息索引、
  EE1 的记忆文件都可能要它；W3 不许改锁（计划 §6.4），漏了就只能停下。判断与理由写进回执第 6 节。
- `core/Cargo.toml` 的 `[workspace.dependencies]`（`:15-25` 之后）加五条 `aite-xxx = { path = "crates/xxx" }`；`core/crates/app/Cargo.toml` 的 `[dependencies]`（`:20-30` 之后）加五条 `.workspace = true`。
  `members = ["crates/*"]`（`:5`）自动收新目录。
- **锁**：只让 `cargo build` 做最小插入，**禁止 `cargo update` / `cargo generate-lockfile`**。期望 diff 形状：5 个新 `[[package]]` 块（`version = "0.0.1"`，依赖名全是锁里已有的）
  + `aite` 包依赖表（`Cargo.lock:31-57`）里多 5 行，**零删除行**。`docker/core/Dockerfile:71` 是 `--locked`，锁没提交 compose-smoke 必红。
- 格式：`(cd core && rustfmt --edition 2024 crates/<x>/src/lib.rs)` 逐个跑——**必须在 `core/` 下**：工具链只由 `core/rust-toolchain.toml` 钉住，
  而 §4.1 第 ② 步装 rustup 用的是 `--default-toolchain none`，在仓库根跑 rustfmt 要么报没有默认工具链、要么用上别的 stable，与 A4b（`core/` 下的 `cargo fmt --check`）对不上。
  另在仓库根跑一次 `rustfmt --version`（不判红），有没有默认工具链逐字记进 `docs/p1/cloud-runbook.md`。未使用的预声明依赖不会触发 `clippy -D warnings`（`unused_crate_dependencies` 默认关）。

**7. D0 卫生与 `.gitignore`。** `git ls-files .venv aite.egg-info | wc -l` → `0`；`git ls-files '*.md' | grep -v /` → 只有 `CLAUDE.md`、`README.md`
（D0 移出索引的那份仓库根规格副本，**别把它的文件名敲进任何 Bash 命令**，守卫按名字拦）。`grep -n 'venv\|egg-info' .gitignore` 应有 D0 追加的两条；缺了由你补。
跑完 cloud-setup、check.sh、docker-test 之后 `git status --short` 只剩你的有意改动（没有构建产物、没有探针）。

**8. 云端基线。** 全部改完后 check.sh **连跑两次**，同样用 `run_in_background` 后台跑、一次跑完再起下一次：
`{ time scripts/check.sh; } > /tmp/cc1-check-final1.log 2>&1; echo "exit=$?" >> /tmp/cc1-check-final1.log`（第二次写 `/tmp/cc1-check-final2.log`），
跑完用 Read 工具读日志全文（不接 `| tail`）；记每次 `real` 墙钟、各行、哪条时序测试抖过（抖了先单跑，见 §6）。
- 新建 `docs/p1/cloud-runbook.md`：环境实测（`uname -m` / `nproc` / 内存 / 磁盘 / docker 版本 / 工具链）、6 个主机可达性表、cloud-setup 两次的墙钟与 WARN、
  check.sh 三次（自检冷启动 + 两次终版）的墙钟与各行、抖动记录、五个镜像参数的国内取值与用法（EE14 会引用）、「改 cloud-setup.sh 要贴回环境设置」这条人工步骤。
- `CLAUDE.md`：填「## 云端基线」一节——一张表：B0（云端实测，填 D0 sha 与各行、两次墙钟）一行；
  P0-CLOSE 之后一行写 `901 / 27 / OK 25 files`，**标「计划值（§5.1），待总管实测」**，别写成实测。
  「## 验收」里那句「check.sh 那一格只显示 8 行…单跑」改成新口径（`go packages` 行、B9 行、`--docker`）；别的节不动。
- runbook 里单写一行「会话首编是否仍是冷编译：是 / 否 + 证据」（证据 = 开场自检补充里那三条命令的输出 + A1 那格的 `Finished` 行）。

## 6. 规则

- 可写面 / 只读面见 §3；要改可写面以外的文件（例如 `README.md:104-117` 的 BB3 手敲说明因透传而过时）→ 写进回执「记账转出去的」，不动手。
- **B8 不变量**：`evals/p0` 场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；
  `!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 不许改（只读、可复制成临时探针）。B8 红了就停下报告。
- **守卫**：被拦就停、原文进回执、不许绕。云端命令里永不出现：`AITE_RELOCK=1`、守卫点名路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`。多行脚本、PR 描述、回执一律用 Write 工具落文件再用；
  `ci.yml`、`Makefile`、两份 Dockerfile、`CLAUDE.md` 里都有受保护字面量：一律只用 Edit 工具改，Bash 里不 grep / sed 那些字面量。`check.sh` 不接 `| tail`。
- 每个行为改动：回归验证 + 变异验证（§5 各项的「绿 → 红 → 绿」三段输出）；还原源码别用 `cp -p` / `shutil.copy2`。
- 格式化：`(cd core && rustfmt --edition 2024 <core 下的相对路径>)`（在 `core/` 下跑，理由见工作项 6；不要 `cargo fmt --all`，守卫也拦不带 `--check` 的 `cargo fmt`）/ `gofmt -w <文件>`（本轨不该有 Go 改动）。
- 新第三方依赖、R0 文件（不在你可写面里的）、锁定面 → 停下报告。axum 归 DD12（H1 批过之后），不在本轨。
- Docker 测试封闭（只连本地 daemon，沙箱 `network none`）。别跑 `make proto-gen`；云端 protoc 生成的 `edge/gen` 永不提交。
- 已知时序抖动测试先单跑再下结论：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、
  `aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。云端 4 vCPU，抖了照记进 runbook。
- 提交信息中文；一个工作项一个提交；`git add` 点名路径，别 `git add -A`（探针会被带进去）。

## 7. 验收（命令 + 期望输出）

```bash
bash scripts/cloud-setup.sh; echo "exit=$?"          # 连跑两次：都 exit=0，末行 cloud-setup 完成
protoc --version                                      # libprotoc 31.x
(cd core && rustc --version)                          # rustc 1.98.1
(cd edge && go version)                               # go1.27 或更新
scripts/check.sh                                      # 末行「全部通过」，退出码 0（不接 | tail）
```

上面 cloud-setup 与 check.sh 两条冷跑都可能超过 Bash 单条 600 秒：照工作项 1 / 8 的写法用 `run_in_background` 后台跑、落 `/tmp/cc1-*.log`、Read 读全文。

check.sh 各行（情形 A）：`cargo passed=897 failed=0`（Δ=0：五个空 crate 不加测试，每个 crate 只多两行 `0 passed`）、`contracts passed=25 failed=0`、
`OK 25 files`、`go packages ok=9 fail=0`、`passed 10/10`、B9 skip 行。情形 B 换成 `901` / `27`。

```bash
git ls-files .venv aite.egg-info | wc -l               # 0
git ls-files '*.md' | grep -v /                       # CLAUDE.md 与 README.md，别无他物
make docker-test; echo "exit=$?"                      # exit=0
docker ps -a --filter label=aite.task -q | wc -l      # 0
(cd core && cargo metadata --no-deps --format-version 1) | python3 -c "import json,sys;print(sorted(p['name'] for p in json.load(sys.stdin)['packages']))"
                                                      # 17 个名字，含 aite-admin、aite-githost、aite-memory、aite-routines、aite-search
git diff --numstat origin/main...HEAD -- core/Cargo.lock                  # 「<加行数> 0 core/Cargo.lock」：零删除
git diff origin/main...HEAD -- core/Cargo.lock | grep '^+name = '         # 恰好 5 行，全是 aite-*
make -n compose-build                                 # 命令行里出现 BASE_REGISTRY / GOPROXY / PROTOC_URL / CARGO_REGISTRY / APT_MIRROR 五个 --build-arg
make help                                             # 列出 cloud-setup、docker-test、evals-p1
gh pr checks                                          # checks、compose-smoke、sandbox-docker 全绿
git diff --name-only origin/main...HEAD               # 全部落在 §3 可写面内
git status --short                                    # 空（探针已删）
```

- `make docker-test` 若因沙箱镜像的 apt / pip 源不通建不出来：**不判红**，把错误原文与可达性表记进 runbook，这一条交 CI 的 `sandbox-docker` 判（原卡：一个坏镜像站不许弄坏环境）。
- 若 `sandbox-docker` / `make docker-test` 红在 `docker/sandbox/**` 或 `edge/internal/sandbox/docker_test.go` 本身（非网络原因）：**不改它们**（CC12 的面），
  原文进回执「记账转出去的」（建议归 CC12），job 与 target 本身保留；PR 描述写明这一格为何红。
- Go 模块文件有没有被改，由总管审 PR 时看；你的命令里不出现那两个文件名。

## 8. 回执（写 `review/p1/ledger/CC1.md`，PR 描述贴摘要）

1. **开场自检原文**（工作项 0 已先推）：4 步逐条原样输出 + 守卫拦截原文 + 工具链五行（含两个 codegen 插件）+ 情形 A/B 与 B0 sha + 环境四项 + docker info + 6 个主机的可达性。
2. **工作项逐条**：改了哪些文件:行（check.sh、Makefile、ci.yml、两个 Dockerfile、compose、三个 Cargo 文件、五个 crate、.gitignore、CLAUDE.md、runbook）。
3. **验证逐条**：Go 行 / B9 / `--docker` / 遗留容器断言 / 五个镜像参数（含 `BASE_REGISTRY` 坏值红在 `FROM`）的「绿 → 红 → 绿」原样输出；cloud-setup 两次的输出与墙钟。
4. **check.sh 完整输出**：自检冷启动一次 + 终版两次，逐字贴，附墙钟。
5. **cargo passed 增量**：期望 Δ=0；若不是 0，逐条列出来源（哪个文件哪几条）。
6. **Cargo.lock diff**：`--numstat` 与五个 `+name` 行原样贴；逐 crate 的依赖理由。
7. **被守卫拦过的命令与拦截原文**（第 2 步那条除外，已在第 1 节）。
8. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）——至少考虑 `README.md:104-117` 的 BB3 手敲说明、`PIP_INDEX_URL` 的 Makefile / compose 透传（CC12 的参数、不在原卡五个名字里；建议归 EE14——W3 的 Makefile / compose 主人，计划 §7；
   `BASE_REGISTRY` / `APT_MIRROR` 已经靠全局 `--build-arg` 送到 `sandbox-image`，只差它一个）、骨架 crate 的反向依赖。
9. **没做的与原因**：例如云端 docker 不可用、某个镜像参数只能交 CI 判。
10. **要总管贴回环境设置的改动**：`scripts/cloud-setup.sh` 若与计划 §4.1 不再逐字相同，贴 diff（没有就写「无」）。
11. **契约缺口**（给 T0 / T0.1）：本轨预期「无」；若骨架 crate 的预声明逼出了需要契约的形状，写清需要什么形状、为什么开放通道绕不过去。
