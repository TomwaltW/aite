# 派单 CC12：沙箱镜像中国版 + AIGC 隐式标识器（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC12.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC12，含 `[REVISION 2026-09-25]` 的 `BASE_REGISTRY` 一条）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

对齐项（总计划 §2 A 表）：**CT10**（沙箱「国内源烘进镜像」——多架构与离线包是 EE14 的事）、**CT18**（网络档 none / china-trusted / custom / full；
「镜像内置 pip.conf / .npmrc / GOPROXY」这一格归本轨）、**CT11**（「生成文件加 AIGC 隐式标识」）。合规依据见附录 B：文件隐式元数据
`AIGC{Label, ContentProducer, ProduceID, ContentPropagator, PropagateID…}`（CC12 标识器、DD5 调用），图片可见水印（CC12 可选）。
依赖审批是 §3 的 **D12**：apt `git curl jq unzip` + Python `pypdf`。

**今天的样子（`98e4460`）**：

- `docker/sandbox/Dockerfile:15` 是写死的 `FROM python:3.11-slim`，没有任何 `ARG`；`:20-28` apt 只装 `fontconfig` + 两套文泉驿字体，
  并当场自检 `fc-list :lang=zh` 非空、有 `timeout`；**没有 git / curl / jq / unzip**（CT12 行写着「镜像无 git」）。
- `:31-33` pip 直接走默认 PyPI；`docker/sandbox/requirements.txt:9-13` 钉了 5 个包（pandas / matplotlib / openpyxl / python-docx / pillow==11.0.0），没有 pypdf。
- `:42-44` 建 uid 1000 的 `aite` 用户——`edge/internal/sandbox/docker.go:117-119`、`:138-139` 的 PutFile tar 头写死 1000，两边必须一致；
  `:46-49` 设 `HOME=/home/aite`、`PYTHONDONTWRITEBYTECODE=1`；`:51` `USER aite`；`:56-57` 出厂自检（四个 import + 中文预热）。
- `core` / `edge` / `docker` 下**没有任何 AIGC 标识代码**：`git grep -n 'AIGC\|aite_label' -- core edge docker` 零命中（不带 pathspec 会命中 D0 后入库的 `CLAUDE.md` 与计划，那不是代码）。
- 沙箱里只能跑 Python：`docker.go:249-252` 的 Exec 永远是 `timeout -k 2 <N> python -u /tmp/aite_exec_*.py`（`ExecLanguage::Bash` 要等 T0 / DD6）。
  所以标识器必须**既是 CLI 又能 import**——DD5 会在一段 Python 代码里调它。
- 网络：`docker.go:524` 默认 `none`，`:537-539` 把 spec 里的串原样交给 Docker。本波只有 `none`，所以你烘进去的四份运行时镜像配置**在本波不起作用**；
  等 T0 加档位、DD6 / DD8 接线、CC11 的 china-trusted 预设放行这些主机、EE10 把默认档翻成 trusted 之后才生效。
- `.github/workflows/ci.yml:134-137`：CI 刻意不建沙箱镜像——本轨 PR 的 CI 验不到镜像（CC1 若先合并了它的 `sandbox-docker` job，PR 的合并引用会跑到）。
- `edge/internal/sandbox/docker_test.go:605-650` 是 P0 镜像硬要求那三条（数据栈 / 中文字体 / `TestImageHasNoNetworkEgress`）。

**在计划里的位置**：W1，可写面与其余 12 轨两两不交；R0 归属 `docker/sandbox/**`：CC12（W1）→ EE10（W3：CA 信任）→ FF8（W4：厂商 CLI）（§7）。
**谁接着用你的东西**：

- **DD5**（W2）：原卡原话 "run python /opt/aite/aite_label.py <path> in the task sandbox before fetch_artifact (CC12 image)"——你定的 CLI / 函数签名、退出码、输出行就是它的接口。
- **EE12**（W3）：把标识默认翻开，并在 `cold_start_to_delivery.rs` 钉「fetch 之前先 exec 标识」。
- **EE10**（W3）：往 `docker/sandbox` 加 Aite CA 信任（镜像默认路径）；它会在你的 Dockerfile 上继续加层。**FF8**（W4）：往镜像里加厂商 CLI。
- **CC11**（W1 同波）：它的 china-trusted 预设要覆盖你的镜像主机，CC11 回执会拿「CC12 四份运行时配置」逐主机对照。
- **EE14**（W3）：多架构 buildx + 离线包，并在 deploy.md 写 `BASE_REGISTRY` / 镜像参数的取值。**FF6**（W4）：买 GB 45438 原文核对字段布局（§10：现在的布局来自第三方解读）。

## 2. 必读（按顺序）

1. `CLAUDE.md` 全文（尤其「守卫」「验收」里的「真容器那组」与「已知时序抖动」「规则」）。
2. 总计划：§3 D12 行与 §3 首句「不回就按推荐走」、§4.1 网络白名单（Trusted + `deb.debian.org` / `security.debian.org` / Docker Hub 两个域名；**清华 / 阿里 / npmmirror 等国内镜像不在里面**）、
   §4.1 安装脚本第 ⑤ 步（它可能已经用 B0 的 Dockerfile 建过 `aite-sandbox:p0`）、§4.4、§6 规则、§6.1 CC12 行、§7、§9、§10 GB 45438 那行、附录 B。
3. `docker/sandbox/Dockerfile`（60 行）、`requirements.txt`（13 行）、`matplotlibrc`（18 行）全文。
4. `edge/internal/sandbox/docker_test.go` 全文（650 行），尤其 `:33` `testImage`、`:58-66` `requireDocker`（**缺 daemon 或缺镜像时整组 `t.Skipf`，`go test` 照样打 `ok`**）、
   `:605-650` 三条镜像硬要求、`:626` 用 `d.execRun` 跑非 Python 命令的写法。
5. `edge/internal/sandbox/docker.go:161-221`（Acquire：`CapDrop ALL`、`no-new-privileges`、network 照传）、`:223-293`（Exec）、`:604-636`（execRun：不指定 `User`，即镜像的 `USER aite`）。
6. 钉着镜像的别处（只读）：`core/crates/app/src/preflight.rs:1601-1655`（preflight 第 6 组跑那四个 import，修复提示在 `:1650-1651`）、
   `core/crates/contracts/tests/layout.rs:46`（`docker/sandbox/Dockerfile` 必须存在——别改名、别挪；用 Read 工具读，Bash 里别点这个路径）、`docker-compose.yml:205-217`（`sandbox-image` service，无 `args`）、
   `Makefile:50-51`、`config/aite.example.yaml:27` 与 `edge/internal/config/config.go:59`（镜像名 `aite-sandbox:p0`）。
7. `docs/dev-spec-2026-09-11-rustgo.md:302`（R2 验收「镜像四条硬要求」）——用 Read 工具读（Bash 里别点这个文件名）。

## 3. 工作区

- **分支**：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC12: 沙箱镜像中国版 + AIGC 隐式标识器」（正文用 `--body-file`，见 §5 第 0 项 / §6 守卫）。
- **代码基线**：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**：
  - `docker/sandbox/**`：改 `Dockerfile` / `requirements.txt`（`matplotlibrc` 不需要动）；新建建议 `docker/sandbox/aite/aite_label.py`、
    `docker/sandbox/aite/tests/test_label.py`、`docker/sandbox/aite/tests/test_image_env.py`、`docker/sandbox/mirrors/{pip.conf,npmrc,cargo-config.toml}`，
    可选 `docker/sandbox/.dockerignore`（排除 `__pycache__`）。目录名你定，回执写实。
  - `edge/internal/sandbox/docker_test.go`：**本轨唯一能动的 Go 文件**（首行 `//go:build docker`）。
  - `review/p1/ledger/CC12.md`（新建）。
- **只读面**：其余一切。特别点名：
  - P0-CLOSE 的 14 个路径（总管在本波期间本机打，见总计划 §4.4 / §5.1）= 本文 §4 第 1 步情形 B 逐个列出的那 14 个；`.claude/**` 整个目录同样只读（守卫面）。
  - T0 补丁文件：`config/aite.example.yaml`、`edge/internal/server/server.go`、`core/crates/proto/**`（外加锁定面 `proto/**`、`core/crates/contracts/**`）。
  - 同包但不归你：`edge/internal/sandbox/{docker.go,docker_pure_test.go,workdir.go,workdir_test.go}`（network 校验、代理 / CA 环境变量注入是 DD8 / EE10 的）。
  - 离你最近的别轨面：CC1 的 `docker-compose.yml`、`Makefile`、`.github/**`、`scripts/cloud-setup.sh`、`scripts/check.sh`、`docker/core/**`、`docker/edge/**`（CC1 在那两个 Dockerfile 里加同名
    `APT_MIRROR` 与 `BASE_REGISTRY`）；CC11 的 `edge/internal/egress/**`；Go 模块文件与 `edge/internal/pin/**`（**没人**，不加 Go 依赖）。
- **本轨解冻的冻结项**：无（§9 没给 CC12 列）。P0 镜像四条硬要求（`Dockerfile:5-12`，dev-spec-rustgo:302）**仍然有效，一条都不许弱化**；
  镜像名 `aite-sandbox:p0`、uid 1000、`WORKDIR /work`、`CMD sleep infinity`、不写 `--platform`（`Dockerfile:13-14`：云端 amd64、总管本机 aarch64）都不变。

## 4. 开场自检（全部对上才开工；对不上就写回执停下）

分级：**第 1–4 步对不上 → 停**，写回执；**第 5 步对不上 → 原文进回执、docker 验收标「人工本机量」、照样写代码**；
基线 docker 组 `PASS≠50` 或 `SKIP≠0` → 按测试名逐条解释（进回执），能解释清楚就继续。

1. **代码基线**：先 `git cat-file -e 98e4460 || git fetch -q --unshallow origin`（浅 clone 时补历史），再
   `git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'`
   - 输出为空 → 情形 A：基线行 = `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok；
   - 恰好列出下面 14 个 P0-CLOSE 路径、一个不多 → 情形 B：基线行 = `cargo passed=901 failed=0`、`contracts passed=27 failed=0`、`OK 25 files`、`passed 10/10`（以实测为准，差异逐条解释）。
     14 个路径：AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
     BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
     `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
     `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
     BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；
   - 别的 → 停下，写回执。（D0 的改动全是删除或落在排除的路径里，所以情形 A 输出为空；`--no-renames` 让「挪文件」也现形。）回执里写明是 A 还是 B。
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（期望形如「blocked: 该操作触碰受保护面 …」；拦截原文逐字贴进回执）。
   拦截原文里的「停止当前工作并向人类报告」**这一次不适用**：被拦是期望结果，记下原文继续下一步。
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可——本轨不跑 codegen，照跑、照记进回执，不因此停）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**（冷编译 10–15 分钟）。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认 → `ok`。
   CC1 合并之后 check.sh 会多打一行 `go packages ok=N fail=M`，从那以后以这行为准。
5. **本轨附加（Docker 基线）**：
   - `docker info --format '{{.ServerVersion}}'` 有版本号；`docker compose version` 有输出。daemon 不在 → 写回执，docker 验收全部标「人工本机量」，照样把代码写完。
   - `docker run --rm python:3.11-slim sh -c 'ls /etc/apt/sources.list.d/; cat /etc/apt/sources.list.d/debian.sources'` → 原样贴进回执
     （决定 `APT_MIRROR` 的替换怎么写：deb822 还是老 `sources.list`、是 trixie 还是 bookworm、`debian-security` 走哪个主机）。
   - `docker image inspect aite-sandbox:p0 --format '{{.Id}} {{.Size}}'` → 记下（安装脚本 ⑤ 可能已用 B0 的 Dockerfile 建过）；没有就
     `docker build -t aite-sandbox:p0 docker/sandbox` 现建，记耗时与大小。**建不出来**：错误原文进回执，再试 `docker pull python:3.11-slim` 分辨是 Docker Hub 还是
     deb / PyPI 不通；标「docker 验收：人工本机量」，**不降低标准**，照样写完代码与测试。
   - 基线 docker 组：`cd edge && go test -tags docker ./internal/sandbox/... -count=1 -v > /tmp/cc12-base.txt 2>&1; echo "exit=$?"` → `exit=0`；
     `grep -c '^--- PASS' /tmp/cc12-base.txt` → **50**（6 + 23 + 21 条顶层测试，按源码数出，以实测为准）；`grep -c '^--- SKIP' /tmp/cc12-base.txt` → **0**；
     `docker ps -a --filter label=aite.task -q | wc -l` → `0`。
   - **D12 判定**（决定装不装 pypdf）：`grep -n 'D12' review/plan-2026-09-25-claude-tag-parity.md CLAUDE.md`。§3 首句是「不回就按推荐走」，D12 推荐「批」——
     除非 D0 版的计划或 `CLAUDE.md` 写明 pypdf 不批，否则装。判定结果与依据（引原文）写进回执第一节。

## 5. 工作项

**总纪律**：Dockerfile、Python、配置文件一律用 Write / Edit 工具写，**不用 heredoc / 多行 `python3 -c`**（守卫扫 heredoc 正文、跨行引号判「无法解析」）。
联网只在镜像构建、拉基底镜像与查 pypdf 版本时；测试全部在 `--network none` 的容器里跑，不 `pip install` / `git clone` / `curl` 任何外部主机。

0. **自检完先开 draft PR**：第一次提交 = 回执第一节（开场自检原文，用 Write 工具写进 `review/p1/ledger/CC12.md`）→ 推 →
   `gh pr create --draft --title "CC12: 沙箱镜像中国版 + AIGC 隐式标识器" --body-file review/p1/ledger/CC12.md`（此时回执文件已存在；非交互下不带正文 `gh` 会直接报错）；
   收尾时 `gh pr edit --body-file review/p1/ledger/CC12.md`（或先 Write 一份摘要文件再 `--body-file` 它）。**永远别**把多行正文塞进 `--body "…"` 或 heredoc（见 §6 守卫）。

1. **构建参数（原卡 + REVISION）**。
   - `FROM` 之前 `ARG BASE_REGISTRY=docker.io`，`FROM ${BASE_REGISTRY}/library/python:3.11-slim`（默认值下与今天是同一个镜像）。
     与 CC1 同形：若 D0 版 `review/paste-CC1.md` 规定了别的写法就照它，回执写明；EE14 在 deploy.md 统一写取值（ACR / TCR / SWR / 客户 Harbor）。
   - `FROM` 之后声明 `ARG APT_MIRROR=`（**空 = 用上游**，不动 sources；非空 = 镜像主机名，把 `deb.debian.org` 换掉，`debian-security` 那条也覆盖）——
     与 CC1 在 `docker/core`、`docker/edge` 里的同名参数同语义（CC1 的派单写的是「非空才替换」）。
   - `ARG PIP_INDEX_URL=https://pypi.org/simple`，`pip install` 显式写 `--index-url "${PIP_INDEX_URL:-https://pypi.org/simple}"`（空串也回落上游，
     防 compose 的 `${X:-}` 把默认值盖成空串）。
   - 国内取值**只写进注释**（Dockerfile 头部）：apt `mirrors.tuna.tsinghua.edu.cn` / `mirrors.aliyun.com`；pip `https://pypi.tuna.tsinghua.edu.cn/simple` /
     `https://mirrors.aliyun.com/pypi/simple/`；`BASE_REGISTRY` 的示例留给 EE14。
   - CC1 的 `make compose-build` 用全局 `--build-arg` 把 `BASE_REGISTRY` / `APT_MIRROR` **也发给 `sandbox-image`**（`docker-compose.yml:205-217` 建的就是这份 Dockerfile），
     所以这两个参数的名字、默认值（`docker.io` / 空）与语义必须和 CC1 逐字一致，否则一次 `make compose-build APT_MIRROR=…` 会弄坏其中一边。
   - 验证（云端能做的）：默认值建一次绿；`--build-arg BASE_REGISTRY=invalid.example` 建一次必须红在 `FROM`（与 CC1 同一条检查）、
     `--build-arg APT_MIRROR=invalid.example` 建一次必须红在 apt 那层、`--build-arg PIP_INDEX_URL=https://invalid.example/simple`
     必须红在 pip 那层（证明参数真被消费），三段报错原文进回执。**国内取值的真实构建**云端做不了（§4.1 白名单里没有这些主机）→ 标「人工本机量」。

2. **apt 层加 `git curl jq unzip ca-certificates bash`**（D12 批了前四个；后两个 slim 基底本来就有，按原卡照列，预期无变化）。
   保留 `fontconfig fonts-wqy-microhei fonts-wqy-zenhei` 与 `:26-28` 的 `fc-cache` / `fc-list :lang=zh` / `timeout` 自检，另在同一层末尾加 `git --version && jq --version`。
   记镜像大小前后对比（EE14 要）。

3. **pypdf（按开场自检第 5 步的 D12 判定）**：批 → `requirements.txt` 加 `pypdf==<版本>`（`docker run --rm python:3.11-slim pip index versions pypdf` 在镜像的 3.11 里查最新稳定版并钉死——宿主不一定有 `pip`；纯 Python wheel，aarch64 不受影响；
   文件头那段「为什么钉版本」的注释照旧成立）；不批 → 不加，标识器走「跳过 PDF」分支。**已钉的 5 个版本一个不改。**
   `Dockerfile:56` 的 `import pandas, matplotlib, openpyxl, docx` **逐字保留**（preflight `:1650-1651` 与 `docker_test.go:612` 钉的是这四个）；
   `PIL` / `pypdf` / 标识器的 import 检查另起一行。

4. **四份运行时镜像配置（`--network none` 下不起作用）**：
   - `/etc/pip.conf`：`index-url` = 清华，`extra-index-url` = 阿里（或反过来，写明理由）；
   - `/home/aite/.npmrc`：`registry=https://registry.npmmirror.com`（镜像里没有 node，文件本身惰性）；
   - `ENV GOPROXY=https://goproxy.cn,direct`（镜像里没有 go）；
   - `/home/aite/.cargo/config.toml`：**指向 ustc**（总管 2026-09-25 修订，覆盖原卡的 rsproxy）→ `[source.crates-io] replace-with = 'ustc-sparse'`，
     `[source.ustc-sparse] registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"`；同文件用注释写好 rsproxy 备选
     （`sparse+https://rsproxy.cn/index/`，大陆实测通过后才可能切，归 EE10）。这样与 CC11 的 china-trusted 预设（只列 ustc crates）一致。
   - **顺序陷阱**：`/etc/pip.conf` 必须在 `pip install` 那层**之后**才 COPY，否则构建期的 pip 会先读到清华、构建参数形同虚设；在 Dockerfile 里写一句注释提醒
     后来的 EE10 / FF8：新的 `pip install` 要么放在这层之前，要么显式带 `--index-url "$PIP_INDEX_URL"`。
   - `/home/aite` 下的文件放在 `useradd --create-home`（`:42-44`）**之后** COPY，属主必须是 aite（`chown` 或 `COPY --chown=1000:1000`）。**不设任何 `HTTP(S)_PROXY` / `ALL_PROXY`**——那会改变
     `TestImageHasNoNetworkEgress`（`docker_test.go:639-650`）看到的报错形态，也不是「惰性」。

5. **标识器 `/opt/aite/aite_label.py`（给 DD5 的接口，形状定死）**。以 root 身份 COPY 到 `/opt/aite/`，文件 0644、目录 0755，放在 `USER aite`（`:51`）之前。
   - **CLI**：`python /opt/aite/aite_label.py [--producer 名] [--produce-id ID] [--propagator 名] [--propagate-id ID] [--watermark] <路径>...`。
     DD5 原卡的调用不带参数，所以默认值必须能用：`Label="1"`、`ContentProducer="Aite"`、`ProduceID=uuid4().hex`、`ContentPropagator=""`、`PropagateID=""`。
   - **输出**：每个路径 stdout 一行 JSON：`{"path", "format", "status": "labeled"|"skipped"|"error", "reason"}`。
     **退出码**：0 = 全部已标；3 = 有跳过、无错误（不支持的类型 / 缺 pypdf）；1 = 有文件出错；2 = 用法错（argparse 默认）。
     **调用方约定（写进脚本 docstring 与回执）：0 与 3 都算成功，只有 1 / 2 算失败**——DD5 不能拿 `returncode != 0` 判失败。
   - **可 import**：`sys.path.insert(0, "/opt/aite"); import aite_label`；导出 `label_file(path, fields=None, watermark=False) -> dict`（返回同一个 dict）与
     `read_label(path) -> dict | None`（测试和 EE12 读回用）。只用标准库 + 镜像里已有的 pillow / openpyxl / python-docx（+ D12 批了才有的 pypdf，**懒 import**）。
   - **载荷**：键恰好是原卡那 5 个：`{"Label","ContentProducer","ProduceID","ContentPropagator","PropagateID"}`，值全是字符串；5 个键名、`Label` 取值、
     XMP 命名空间都集中在文件顶部一处常量里，注明「布局来自第三方解读，FF6 买标准原文后核对」（§10）。不自行加 `ReservedCode1/2` 等别的键——觉得该加写进回执。
   - **各格式写在哪**（回执要原样列这张表给 DD5 / EE12 / FF6）：
     - PNG：`iTXt` 块，keyword `AIGC`，UTF-8 JSON（pillow `PngInfo.add_itxt`）；读回时 `tEXt` / `iTXt` 都认。
     - JPEG：XMP（APP1）里的 AIGC 属性（建议命名空间 `http://www.tc260.org.cn/ns/AIGC/1.0/`，前缀 `TC260`，属性 `AIGC`，值为 JSON——**第三方解读**）
       + EXIF `UserComment`（0x9286，Exif IFD），`ASCII\0\0\0` 前缀 + `ensure_ascii=True` 的 JSON。先在镜像里实测 pillow==11.0.0 的 JPEG `save()` 支不支持 `xmp=`
       与 `quality="keep"`：不支持 `xmp=` 就按字节插 APP1 段或只写 EXIF，二选一写进回执；**不许为此升级 pillow**。尽量不重压缩像素（重压缩了就在回执写明代价）。
     - DOCX / XLSX / PPTX：用 `zipfile` 整包重写——`docProps/custom.xml` 里 `name="AIGC"` 的 `vt:lpwstr`；没有 `custom.xml` 就新建，同时给 `[Content_Types].xml`
       加 `/docProps/custom.xml` 的 Override、给 `_rels/.rels` 加 custom-properties 关系；已有就合并（保留别的属性，`pid` 从现有最大值往上接）。
     - Markdown（`.md` / `.markdown`）：文件头 YAML front-matter 的 `AIGC:` 键（单行 JSON，单引号包住）；没有 front-matter 就加一个，已有就只增 / 换这一个键。
     - PDF：文档信息字典 `/AIGC`（pypdf：`PdfWriter(clone_from=…)` + `add_metadata`）；**pypdf 不在** → `status=skipped`，`reason` 写明
       「镜像里没有 pypdf（D12 未批），PDF 不打隐式标识」，退出码 3。
     - 其它扩展名：`skipped`，退出码 3，不报错（DD5 在 fetch 前批量调，不能因一个 `.csv` 让任务失败）。
   - **写法**：同目录临时文件 + `os.replace`，保留原权限位；同一文件标两次 → 替换，不重复。标完文件的 size / mtime 会变，
     所以它会出现在那次 Exec 的 `files_out` 里（`docker.go:291` 按快照差集算）——回执写一句给 DD5。

6. **可选可见水印**：`--watermark` 只对 PNG / JPEG 生效，右下角画 `AI生成`，字体用镜像里的文泉驿（路径先 `fc-list | grep -i wqy` 实测，别猜）；**默认关**。

7. **unittest 套件 `/opt/aite/tests`**（原卡没给测试名；下面是本派单定的名字，照用；要增删写进回执）。测试文件自己 `sys.path.insert(0, <tests 的上一级>)`
   （原卡的 discover 命令不设 `PYTHONPATH`）；夹具全部在 `tempfile` 里现造（pillow 造图、python-docx / openpyxl 造文档；**PPTX 手搓最小 OPC zip**——镜像里没有 python-pptx，也不许加）。
   - `test_label.py`：`test_png_itxt_roundtrip`、`test_png_relabel_replaces_not_duplicates`、`test_jpeg_exif_and_xmp_roundtrip`、`test_docx_custom_props_roundtrip`、
     `test_xlsx_keeps_existing_custom_props`、`test_pptx_minimal_package_labelled`（断言 Content_Types Override 与 rels 关系都在）、`test_markdown_front_matter_added`、
     `test_markdown_existing_front_matter_kept`、`test_pdf_info_roundtrip`（`skipUnless` 装了 pypdf）、`test_pdf_skipped_when_pypdf_missing`（用
     `mock.patch.dict(sys.modules, {"pypdf": None})` 模拟缺包，**装没装 pypdf 都会跑**）、`test_unsupported_type_skipped_exit_3`、`test_cli_prints_one_json_line_per_path`、
     `test_payload_has_exactly_five_fields`、`test_watermark_off_by_default`（像素逐字节不变）、`test_watermark_opt_in_changes_pixels`。
   - `test_image_env.py`：`test_pip_conf_points_to_mainland_index`、`test_npmrc_points_to_npmmirror`、`test_goproxy_env_is_goproxy_cn`、
     `test_cargo_config_points_to_ustc`、`test_no_proxy_env_in_image`（六个 `*_proxy` / `*_PROXY` 变量都没设）。
   - 验证与变异：不用每次重建镜像，把工作树挂进去跑：
     `docker run --rm --network none -v "$PWD/docker/sandbox/aite:/opt/aite:ro" aite-sandbox:p0 python -m unittest discover -s /opt/aite/tests -v`（在仓库根跑，路径按你的实际目录）。
     先确认输出里是 `Ran 20 tests`：若是 `Ran 0 tests` 或 `No such file`（云端 daemon 看不到会话的文件系统，挂载是空的），就别用挂载，每个 Python 变异改完重建镜像再跑，回执写明用的哪种。
   - 若 `docker run`（CLI）下 `test_no_proxy_env_in_image` 红、而 §5 第 8 项经 SDK 跑的同一套件绿：多半是 CLI 从 `~/.docker/config.json` 的 `proxies` 注入了代理变量——
     先 `docker image inspect aite-sandbox:p0 --format '{{.Config.Env}}'` 看镜像本身有没有，再决定改不改 Dockerfile，诊断写进回执。

8. **`docker_test.go` 新增 4 条**（放在 `:605` 那节「镜像的硬要求」里，照 `:621-636` 的写法；名字本派单定，照用）：
   - `TestImageHasShellTools`：`d.execRun` 跑 `sh -c 'git --version && curl --version && jq --version && unzip -v && bash --version'` → 退出码 0；
     再断言 `/etc/ssl/certs/ca-certificates.crt` 存在。
   - `TestImageRunsAsUID1000`：`id -u` 与 `id -g` 都是 `1000`（钉住 `docker.go:138-139`）。
   - `TestImageLabelerSuitePasses`：`mustAcquire` 拿真 DockerSandbox（network none、CapDrop ALL、镜像的 `USER aite`），然后照 `:626` **直接**
     `d.execRun(ctx, id, []string{"python", "-m", "unittest", "discover", "-s", "/opt/aite/tests"}, Workdir)`——**不走 `mustExec`**
     （Exec 把脚本以 0600、属主 1000 写进 `/tmp`，`docker.go:236-237` + `:698-704`；镜像 uid 一变它先红，就证明不了套件本身）→ 退出码 0，stderr 末行以 `OK` 开头；
     不满足时 `t.Fatalf` 带上 stderr 里全部 `FAIL:` / `ERROR:` 行与末尾几行（第 9 项变异要从这里读出是哪条 unittest 红）。
   - `TestImageLabelsPNGBeforeFetch`：`mustExec` 一段 Python——matplotlib 画一张带中文标题的图存 `/work/out.png`，再 `subprocess.run(["python", "/opt/aite/aite_label.py", "/work/out.png"])`；
     然后 `d.GetFile` 取回，用标准库逐块解析 PNG，找到 keyword 为 `AIGC` 的 `iTXt` / `tEXt` 块并 `json.Unmarshal` 出恰好 5 个键；找不到时
     `t.Fatalf` 的文案里带「找不到 AIGC 块」（第 9 项变异认这句）。这就是 DD5 将来走的路。
   - 只用标准库新 import（`encoding/json`、`encoding/binary`、`bytes` 等），不加第三方包；改完 `gofmt -w edge/internal/sandbox/docker_test.go`；
     文件头注释里「18 条 + 三条硬要求」的计数顺手改对。已有 21 条一条不改（`TestImageHasNoNetworkEgress` 必须仍绿）。

9. **镜像级变异（分两次建，按改不改 uid 分开）**。pip.conf 等四份配置只在镜像里，挂载式的 Python 变异碰不到它们，这里是它们唯一的证明，所以每条红必须红在**对的原因**上。
   两次都**仍打成 `aite-sandbox:p0`**：
   - **建 X（uid 不动）**：去掉 jq（同层的 `jq --version` 自检一并去掉，否则红在构建而不是测试）+ 不 COPY pip.conf + 注掉 `aite_label.py` 里写 PNG 块的那一行（注掉后仍要能 import，否则红在构建期自检）。
     跑 `cd edge && go test -tags docker ./internal/sandbox/... -count=1 -run 'TestImageHasShellTools|TestImageLabelerSuitePasses|TestImageLabelsPNGBeforeFetch' -v`
     → 三条都 `--- FAIL`，且贴回执的原文里分别看得到：`TestImageHasShellTools` 的 `jq` 找不到 / 退出码非 0；`TestImageLabelerSuitePasses` 的 `t.Fatalf` 里有
     `FAIL: test_pip_conf_points_to_mainland_index`（`test_png_itxt_roundtrip` 等 PNG 那几条同时红是预期）；`TestImageLabelsPNGBeforeFetch` 是「找不到 AIGC 块」那条断言。
   - **建 Y（只改 uid）**：`useradd --uid 1001`，别的不动。跑 `-run 'TestImageRunsAsUID1000'` → `--- FAIL`，断言原文里是 `1001`。
     （uid 1001 会让一切走 `mustExec` 的用例因读不到 0600 / 属主 1000 的脚本而红——所以 uid 不跟 X 混在一次里。）
   - 还原：`git checkout -- docker/sandbox/Dockerfile docker/sandbox/<你的标识器路径>`（或重新编辑，别用 `cp -p` / `copy2`）→ 重建 → 验收第 5 条全绿。

## 6. 规则

- **可写面 / 只读面**见 §3；需要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。
- **B8 不变量**（`evals/p0` 钉死，弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；
  `!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 一个字都不许改。本轨不碰 core，B8 应当逐字不变。
- **守卫**：被拦就停（开场第 2 步那次除外：被拦是期望结果，记下原文继续）、拦截原文进回执、不许换写法绕。云端命令里永不出现：重锁变量赋值、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`；check.sh 不接 `| tail`。
  多行脚本、PR 描述、回执、多行提交信息**一律先用 Write 工具落文件再用**（`python3 <文件>` / `gh pr create --draft --title "CC12: 沙箱镜像中国版 + AIGC 隐式标识器" --body-file <文件>` /
  `gh pr edit --body-file <文件>` / `git commit -F <文件>`；命令行 `-m` 只写单行）；不走 Bash heredoc / `echo >`、不传多行 `--body "…"`（跨行引号会被判「无法解析」，
  回执与 PR 描述里有守卫拦截原文和受保护路径，heredoc 正文会被扫到拦下，拦了按规则就得停）。
- **每个行为改动**：回归测试 + 变异验证（撤回改动 → 测试红 → 贴输出）。至少：PNG 写块那行注掉；OOXML 不写 Content_Types Override；Markdown 不合并已有 front-matter；
  缺 pypdf 分支改成抛异常；水印默认改开；载荷多写一个键——各自对应的测试必须红（用 §5 第 7 项的挂载命令，不用重建）；外加 §5 第 9 项的镜像级变异。
  还原用 `git checkout -- <文件>` 或重新编辑，**别用 `cp -p` / `shutil.copy2`**。
- **格式化**：`gofmt -w <改过的 Go 文件>`；本轨没有 Rust 改动，不跑 rustfmt，更不跑 `cargo fmt --all`。
- **新依赖**：只许 D12 批的（apt `git curl jq unzip`；`pypdf` 按 D12 判定）与原卡点名的 `ca-certificates bash`。python-pptx、PyYAML、piexif、reportlab、
  任何别的 pip / apt 包 → 停下报告。R0 文件（不在你可写面里的）、锁定面 → 停下报告。
- **Docker 测试封闭**：只在 `--network none` 的容器里跑，不连公网主机（`TestImageHasNoNetworkEgress` 是「确认连不上」，不算连）。云端 protoc 生成的 `edge/gen` 永不提交（本轨不跑 `make proto-gen`）。
- **已知时序抖动**（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、`aite-testing` 的
  `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。你的测试不许靠 `sleep` 等时序。

## 7. 验收（命令 + 期望输出）

基线数按开场自检第 1 步判定的情形取（A：897 / 25；B：901 / 27）。

1. `docker build -t aite-sandbox:p0 docker/sandbox` → 成功；`docker image inspect aite-sandbox:p0 --format '{{.Id}} {{.Size}}'` 的 Id **不同于**开场自检记下的那个
   （确认后面跑的不是 B0 的旧镜像），大小前后对比进回执。
2. `docker run --rm --network none aite-sandbox:p0 python -m unittest discover -s /opt/aite/tests -v` → `Ran 20 tests`（以你最终条数为准，回执逐条列），
   末行 `OK`（D12 批）或 `OK (skipped=1)`（D12 不批，只跳 `test_pdf_info_roundtrip`）；别的 skipped 数要逐条解释。
   若 `docker image inspect aite-sandbox:p0 --format '{{.Config.Env}}'` 里没有任何 `*_PROXY`、而 CLI 注入了代理（只有 `test_no_proxy_env_in_image` 红）：
   第 2 条以第 5 条（SDK 路径，`TestImageLabelerSuitePasses`）为准，CLI 结果与 `~/.docker/config.json` 的 `proxies` 原文进回执；**不许改测试放宽断言**。
3. `docker run --rm --network none aite-sandbox:p0 sh -c 'git --version && jq --version'` → exit 0（原卡原样）。
4. `docker run --rm --network none aite-sandbox:p0 id -u` → `1000`；`docker run --rm --network none aite-sandbox:p0 fc-list :lang=zh` → 非空。
5. `cd edge && go test -tags docker ./internal/sandbox/... -count=1 -v > /tmp/cc12-docker.txt 2>&1; echo "exit=$?"` → `exit=0`；
   `grep -c '^--- PASS' /tmp/cc12-docker.txt` → **54**（基线 50 + 4）；`grep -c '^--- SKIP' /tmp/cc12-docker.txt` → **0**（`requireDocker` 跳过 ≠ 通过）；
   `grep -E '^--- (PASS|FAIL): TestImage' /tmp/cc12-docker.txt` → 7 行全 PASS（旧 3 + 新 4）。
6. `docker ps -a --filter label=aite.task -q | wc -l` → `0`。
7. `cd edge && go vet -tags docker ./internal/sandbox/... && gofmt -l . | wc -l` → exit 0，打印 `0`（check.sh 的 `go vet ./...` 不带 tag，编不到 `docker_test.go`）。
8. `scripts/check.sh` → 末行「全部通过」、exit 0；`cargo passed=<897 或 901> failed=0`（**Δ = 0**：本轨不改 Rust）、`contracts passed=<25 或 27> failed=0`、
   `OK 25 files`、`passed 10/10`；A4c / A4d `-> exit 0`；Go 那格 8 行 = 6 行 `ok` + 2 行 `?`（`gen/aitepb`、`internal/pin` 无测试文件；`docker_test.go` 带 tag，不影响包数，仍 9 包），单跑 `cd edge && go test -race ./cmd/... -count=1` → `ok`。
9. `git diff --name-only origin/main...HEAD` → 每一行都以 `docker/sandbox/` 开头，或正好是 `edge/internal/sandbox/docker_test.go`、`review/p1/ledger/CC12.md`。
10. `git status --short` → 空（临时变异、探针都清掉）。

- 原卡「reviewer 另外确认 Go 模块文件没被改」由**总管审 PR 时**看；你别在命令里 grep 或 diff 那两个文件（守卫点名路径）。第 9 条本身已经保证它们不在 diff 里。
- 第 1–7 条在云端因网络 / daemon 做不成 → 错误原文进回执，逐条标「人工本机量」，**不许**把 SKIP 或「没跑」当通过。
- **人工本机量**（写进回执给总管，你不跑）：国内取值的真实构建，形如
  `cd ~/Documents/Projects/Aite && docker build --build-arg APT_MIRROR=mirrors.tuna.tsinghua.edu.cn --build-arg PIP_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple -t aite-sandbox:cn docker/sandbox`，
  然后对 `aite-sandbox:cn` 跑第 2、3 条；以及总管本机 aarch64 上默认参数建一次（`Dockerfile:13-14` 的理由）。

## 8. 回执（写 `review/p1/ledger/CC12.md`，PR 描述贴摘要；两者都先 Write 成文件，PR 用 `--body-file`，见 §5 第 0 项、§6）

回执**只用 Write 工具**写（它要逐字贴守卫拦截原文，里面有受保护路径，Bash heredoc / `echo` 会被拦）；PR 描述用 `gh pr edit --body-file review/p1/ledger/CC12.md`
（或先 Write 一份摘要文件再 `--body-file` 它）同步。

1. **开场自检原文**（4 + 1 项）：第 1 步 diff 输出与判定（A / B）；守卫拦截原文（逐字）；三条工具链版本 + 两个 codegen 插件版本（对不上也照记）；check.sh 各行原样；第 5 步的 docker 版本、
   基底 apt 源文件原文、旧镜像 Id / 大小、基线 docker 组的 PASS / SKIP 计数、D12 判定与依据。
2. **工作项逐条**：1–9 每项落在哪些 `文件:行`；最终 Dockerfile 的层顺序（一句话一层）；镜像大小前后；`BASE_REGISTRY` / `APT_MIRROR` / `PIP_INDEX_URL` 三次「坏值必红」的报错原文。
3. **给 DD5 / EE12 / FF6 的接口**：CLI 用法原样、退出码表（写明「0 / 3 算成功，1 / 2 算失败」）、输出行样例（真跑出来的）、`label_file` / `read_label` 签名、各格式写入位置表（§5 第 5 项那张的终稿）、
   pillow 11.0.0 实测结论（`xmp=` / `quality="keep"`）、标识后文件进 `files_out` 这一条。
4. **四份运行时配置对照表**（给 CC11 / DD6 / EE10）：文件路径 | 属主 | 指向的主机 | 与 CC11 china-trusted 预设是否一致（应全部一致）。
5. **新增测试逐条 + 变异验证输出**：Python 20 条 + Go 4 条各钉什么；每个变异怎么做的；红的那段输出逐字贴（`FAIL:` / `--- FAIL` 与断言信息）。
6. **check.sh 完整输出**（原样，不截）；外加单跑 `cmd/aite-edge` 那行。
7. **`cargo passed` 增量逐条**：Δ = 0（写明「本轨不改 Rust」）；另列 Go docker 组 50 → 54（按测试名）、镜像内 unittest 0 → 20。
8. **被守卫拦过的命令与拦截原文**（开场第 2 步那次也列上；除此之外没有就写「无」）。
9. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）。至少考虑：`PIP_INDEX_URL` 经 `docker-compose.yml:205-217` 的 `sandbox-image` 与 `Makefile:50-51` 透传
   （`BASE_REGISTRY` / `APT_MIRROR` 已由 CC1 的 `make compose-build` 全局 `--build-arg` 带到，只差这一个；不在 CC1 原卡五个名字里 → EE14）；CI 建沙箱镜像并跑 docker 组（CC1 的 `sandbox-docker` job）；network≠none 时注入代理与 CA 环境变量（DD8 / EE10）；
   fetch 前调用标识器（DD5）、默认翻开（EE12）；**「DD5 调用方必须把退出码 3 当成功 | CC12 接口约定（跳过不是失败）| DD5」这一行必写**；cargo 源 ustc → rsproxy 的切换条件（大陆实测后，EE10）；GB 45438 原文核对、要不要加 `ReservedCode1/2`（FF6）；
   arm64 构建验证与离线包（EE14）；`BASE_REGISTRY` 写法若与 CC1 不一致（EE14 统一）。
10. **没做的与原因**：云端做不成的 docker 验收逐条列并标「人工本机量」；你拿不准的字段布局 / 取值。
11. **契约缺口**（给 T0 / T0.1）：预期「无」——标识器经现有 `run_python` / Exec 调用，不需要新 RPC。若你认为需要（例如沙箱要带环境变量才能标识），写清需要什么形状、
    为什么开放通道（`ExecRequest` 的 Python 代码本身、`SandboxSpec` 现有字段）绕不过去。
