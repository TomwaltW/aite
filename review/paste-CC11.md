# 派单 CC11：出网代理包（不接线）（第 1 波 · Claude Code 云端）

> 启动：`cd ~/Documents/Projects/Aite && claude --cloud "读 review/paste-CC11.md 并照做"`（CC1 先单独派；其余在 CC1 自检通过后派）
> 总计划：`review/plan-2026-09-25-claude-tag-parity.md` §6.1 · 英文原卡：`review/p1/tracks-2026-09-25.json`（id=CC11）· 生成 2026-09-25 · 代码基线 `98e4460`（+ 总管的 D0 文档提交）
> 文中所有指向总计划的行号（`plan:NNN`、「计划第 N 行」之类）都是生成时的；计划此后又改过，行号已经漂了——一律按 § 编号或关键词在计划里找，不按行号。仓库代码的 `文件:行号` 以 `98e4460` 为准，照常可用。
> 你是一个 Claude Code 云端会话。总管不在线：独立干完、开 draft PR、写回执；拿不准的写进回执，不猜、不扩范围。

## 1. 背景

**对应 Claude Tag 的哪几条**（总计划 §2 对齐矩阵，行号是 D0 前的实测，计划再改就以节名为准）：CT17（Agent Proxy：默认拒绝、只放 HTTP(S)、被拦请求可审计，§2 CT17 行，plan:151）、
CT18（网络级别 none / trusted / full，CT18 行，plan:152）、CT27（每小时网络事件导出，CT27 行，plan:161）、NEW11（Domains：放行主机但不带凭证，NEW11 行，plan:180）、
NEW12（`*` 放行任意主机、默认关，NEW12 行，plan:181）。
中国化取舍：Trusted 档不用海外包源，换成国内镜像档 `china-trusted`；代理上线前默认仍是 none（§2.D「Trusted access」条，plan:234）。
凭证注入要做 TLS 终结，那是 EE10 的事（§2.C「CT17 凭证注入」条，plan:214；§6.4 EE10 行，plan:571）。

**Aite 今天的样子**（全仓库没有任何出网代理代码；§1.2 第 5 条，plan:104）：

- 沙箱安全只靠「完全不出网」：`edge/internal/sandbox/docker.go:5-7` 的包注释写死 network=none；`resolveSpec` 默认 `network: "none"`（:524），
  但 `spec.GetNetwork()` 非空时**原样照传**（:537-539），再原样塞进 `container.NetworkMode`（:188-189）—— edge 端不校验。
- 契约只有一个档位：`core/crates/contracts/src/sandbox.rs:6-9` `SandboxNetwork { None => "none" }`；`proto/aite/v1/sandbox.proto:13` 注释「P0 只允许 "none"」；
  core 侧 `core/crates/proto/src/convert.rs:500-507` 遇到非 none 直接报 `Invalid`。
- T0 的 p1.0 契约（§5.2「沙箱」一条，plan:473）会带来 `SandboxNetwork += Trusted, Custom, Full`、`EgressPolicy {level, allow_hosts, credentials, audit_tag}`、
  `CredentialBinding {host, header, scheme, secret_ref}`、`AcquireRequest.egress`，以及 `domain.rs` 里的 `NetworkEvent`（「Go 代理写的 JSONL 形状」）。
  **这些都在锁定面、本轨不碰**；本轨做的是一个不依赖契约、今天就能编过测过的纯 Go 包。

**本轨交付**：新包 `edge/internal/egress`（只用标准库）—— HTTP CONNECT + absolute-URI 正向代理，按沙箱令牌识别客户端、按档位放行、默认拒绝、
每请求一行 JSONL 审计、按小时分文件。**不接线**：`main.go`、`sandbox/docker.go`、`docker-compose.yml` 一个字都不动。

**谁接你的东西**：
- **DD8**（W2，Go 侧 R0 主人，§6.3 DD8 行，plan:547）：在 `edge/cmd/aite-edge/main.go` 里起你的代理（接在 `run()` 的 :96 `sandbox.NewDocker` / :102 `server.ListenUnix` 附近）、
  建沙箱内网、每个沙箱发一个令牌 `Register`、给容器注入 `HTTP(S)_PROXY`、置 `EdgeStatus.egress_ok`；docker 测试里「放行的本地测试主机能到、没放行的得 403」直接打你的代理。
  所以 **`New` / `Serve` / `Register` / `Unregister` 的签名与语义由本轨定死，DD8 照着接**。
- **DD6**（W2，§6.3 DD6 行，plan:545）：把 settings + bundle 域名 + 连接拼成 `EgressPolicy`；任务结束「只吊销令牌」= 调你的 `Unregister`。
- **EE10**（W3，§6.4 EE10 行，plan:571）：在你的包里加「只对带凭证的主机做 TLS 终结 + 注入」，`NetworkEvent.injected=true`；IM / API 主机只能这样到达。
- **EE12**（W3，§6.4 EE12 行，plan:573）：`aite audit export` 把你的小时 JSONL 与 AuditLog 合并导出、网络日志留存 ≥190 天（附录 B「网络安全法」条，plan:767）；
  **FF7**（W4，§6.5 FF7 行，plan:586）做网络事件检索。
- **T0 / T0c**：`domain.rs` 的 `NetworkEvent` 要和你写出的 JSON 行逐字段一致（见 §5 第 7 项与回执「契约缺口」）。
- **CC12**（同波，§6.1 CC12 行，plan:532）把 pip / npm / Go / cargo 的国内镜像配置烘进沙箱镜像 —— 那些主机必须都在你的 `china-trusted` 预设里（cargo 已按总管修订指向 ustc，本就在预设里；`rsproxy.cn` 待大陆实测，本轨不列，见 §5 第 4 项）。

## 2. 必读（按顺序）

1. 仓库根 `CLAUDE.md`（云端唯一能读到的约定；与本派单冲突时以本派单为准）。重点：守卫（:27-42）、验收（:44-59）、规则（:61-75）。
2. 总计划（以节名为准，括号里是 D0 前实测行号）：§4.4 开场自检（plan:376-394）、§4.5 受保护面人工回路（plan:396-407）、§5.1 P0-CLOSE（plan:431-445）、
   §5.2「沙箱」一条（plan:473）、§6 每波规则（plan:502-512）、§6.1 CC11 行（plan:531）、§7 R0 归属里 `edge/internal/pin/**`、`edge/cmd/aite-edge/main.go` 两行（plan:621-622）、
   §9 解冻清单（plan:717-725）、§10 MITM 风险行（plan:743）。
3. 英文原卡：`review/p1/tracks-2026-09-25.json` 里 `waves[0].tracks` 的 `id == "CC11"`；`contract_batches` 里 T0-p1.0 的 sandbox 一条与 domain.rs 一条（`NetworkEvent`）。
4. 现有代码（只读，看形状、别改）：
   - `edge/internal/sandbox/docker.go:1-23`（包注释 + 日志事件名风格 `sandbox.acquired` …，:20-22）、:188-189、:513-544。
   - `edge/internal/pin/pin.go:1-15`（已钉的三方库；本轨一个都不用 —— 卡片要求纯标准库）。
   - `edge/internal/feishu/ratelimit.go:15-19`、:59-61（可注入时钟 `clockFunc`、缺省 `time.Now` 的写法）。
   - `edge/internal/feishu/helpers_test.go:43-70`（假时钟）、:85-133（`slog` 日志捕获 + `attr`）—— 那是 feishu 包内的测试件，**不能 import**，照形状在 egress 包里自建。
     注意它的 `WithAttrs` / `WithGroup` 直接 `return h`，会把 `logger.With(...)` 带的属性丢掉 —— 「令牌不进日志」的断言就看不到它们。
     egress 的捕获 handler 要在锁内保留 `WithAttrs` / `WithGroup` 的属性；或者把 `Config.Logger` 设成 `slog.NewJSONHandler(w, nil)`，
     `w` 是**加了互斥锁的** `io.Writer`（裸 `bytes.Buffer` 会被请求 goroutine 并发写，`-race` 下报 `DATA RACE`），断言输出里不含令牌原文。
   - `core/crates/store/src/lib.rs:79-83`（时间戳定长：UTC、6 位小数、`Z` 结尾 —— 你的 `ts` 照这个形状）。
   - `scripts/check.sh:15-19`（`run()` 只留最后 8 行）、:30-31（go vet / gofmt）、:41（`go test -race ./...`）；`.github/workflows/ci.yml:45`、:56（CI 同样两步）。

## 3. 工作区

- **分支**：会话自带的 `claude/*` 分支（只能推这一条）；第一次提交后立刻开 draft PR，标题「CC11: 出网代理包（不接线）」（正文用 `--body-file`，见 §6 守卫；收尾时 `gh pr edit --body-file review/p1/ledger/CC11.md` 或它的摘要文件）。
- **代码基线**：`98e4460`；判定方法见开场自检第 1 步（情形 A / 情形 B）。
- **可写面**（全部新建，基线上都不存在）：
  - `edge/internal/egress/**` —— 建议文件：`egress.go`（`Config` / `Proxy` / `New` / `Serve`）、`policy.go`（`Level` / `Policy` / 主机匹配 / 预设 / 保留主机）、
    `audit.go`（`NetworkEvent` / 小时文件写入）、`tunnel.go`（CONNECT 与转发），测试 `policy_test.go`、`proxy_test.go`、`audit_test.go`、`helpers_test.go`。文件怎么切你定。
  - `docs/p1/egress.md`（新建；`docs/p1/` 目录在基线上不存在，同波 CC1 / T0 / CC9 / CC10 也各自往里加**不同的**新文件，不会冲突）。
  - `review/p1/ledger/CC11.md`（新建）。
- **只读面**：其余一切。特别点名：
  - P0-CLOSE 文件（总管在本波期间本机打）= 开场自检第 1 步列的 14 个路径：`proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/**`
    （P0-CLOSE 重生成 `edge/gen/aitepb/edge_grpc.pb.go`）、`core/crates/contracts/src/{evidence,lib}.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
    `core/crates/evidence/**`、`core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`、`.contracts.lock`、`.claude/**`、`core/crates/app/tests/guard.rs`。
  - T0 补丁文件：`config/aite.example.yaml`、`edge/internal/server/server.go`、`core/crates/proto/**`（外加锁定面 `proto/aite/v1/*.proto`、`core/crates/contracts/**`）。
  - 离你最近的别轨面：CC12 的 `docker/sandbox/**` 与 `edge/internal/sandbox/docker_test.go`（:639 `TestImageHasNoNetworkEgress` 要保持绿）；
    CC9 的 `edge/internal/dingtalk/**`、CC10 的 `edge/internal/wecom/**`、CC8 的 `edge/internal/feishu/**`；CC1 的 `docker-compose.yml`、`scripts/check.sh`、`.github/**`；
    CC4 会在 core 侧建一个空的 `core/crates/app/src/features/egress.rs` —— 那是 CC4 的文件，不是你的。
  - 本波没人拥有、但和你最相关的：`edge/internal/sandbox/docker.go`（network 串校验、内网、代理环境变量都是 DD8 的）、`edge/internal/config/**`（DD8）、
    `edge/internal/pin/**` 与 Go 模块文件（**没人**，§7 那一行，plan:621：不加 Go 依赖）。
- **本轨解冻的冻结项**：无（§9 没给 CC11 列解冻项）。「`SandboxNetwork` 只有 none」由 T0 / DD8 解冻，本轨的包不接线，所以不触碰这条冻结。

## 4. 开场自检（全部对上才开工；对不上就写回执停下）

1. **代码基线**：先 `git cat-file -e 98e4460 || git fetch -q --unshallow origin`（浅 clone 时补历史），再
   `git diff --stat --no-renames --diff-filter=AM 98e4460 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'`
   - 输出为空 → 情形 A：基线行 = `cargo passed=897 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok
     （= 7 行 ok + 2 行 no test files，见第 5 步）；
   - 恰好列出下面 14 个 P0-CLOSE 路径、一个不多 → 情形 B：基线行 = `cargo passed=901 failed=0`、`contracts passed=27 failed=0`、`OK 25 files`、`passed 10/10`（以实测为准，差异逐条解释）。
     14 个路径：AA4 的 `proto/aite/v1/edge.proto`、`edge/cmd/aite-edge/main.go`、`edge/gen/aitepb/edge_grpc.pb.go`；
     BB2 的 `core/crates/contracts/src/evidence.rs`、`core/crates/contracts/src/lib.rs`、`core/crates/contracts/tests/evidence_vectors.rs`、
     `core/crates/evidence/src/writer.rs`、`core/crates/evidence/src/cli.rs`、`core/crates/evidence/tests/chain.rs`、
     `core/crates/app/tests/evidence_on_disk.rs`、`core/crates/app/tests/cold_start_to_delivery.rs`；重锁的 `.contracts.lock`；
     BB4 的 `.claude/hooks/guard_bash.py`、`core/crates/app/tests/guard.rs`；
     （这张表只用来和 diff 输出逐行对照，别把这些路径写进任何命令。）
   - 别的 → 停下，写回执。（D0 的改动全是删除或落在排除的路径里，所以情形 A 输出为空；`--no-renames` 让「挪文件」也现形。）回执里写明是 A 还是 B。
2. **守卫挂上了**：用 Read 工具读 `.claude/hooks/guard_bash.py`，**必须被拦**（期望形如「blocked: 该操作触碰受保护面 …」；拦截原文逐字贴进回执）。
   **这一步被拦就是通过；拦截原文里「停止当前工作并向人类报告」对这一步不适用，贴进回执后继续第 3 步。**
   没被拦 = hook 没生效（云端只在单仓会话里加载项目 hook，而且 hook 失败是静默放行）→ 立刻停，什么都别改。
3. **工具链**：`protoc --version` → `libprotoc 31.x`；`cd core && rustc --version` → 1.98.1；`cd edge && go version` → ≥ go1.27；
   T0 另外要 `protoc-gen-go --version` → `v1.36.12`、`protoc-gen-go-grpc --version` → `1.6.2`（其它轨对不上记一笔即可 —— 本轨就是「其它轨」，照跑、照记，不因此停；本轨不跑 codegen）。
4. **`scripts/check.sh`**：末行「全部通过」，各行与第 1 步判定的基线行逐字一致；**别接 `| tail`**（冷编译 10–15 分钟，耐心等）。
   Go 那一格只显示 8 行是正常的（`cmd/aite-edge` 被截掉），单跑 `cd edge && go test -race ./cmd/... -count=1` 确认 → `ok`。
   若 check.sh 已经多打一行 `go packages ok=N fail=M`（CC1 合并后才有），以那行为准。
5. **本轨附加**：
   - `cd edge && go test -race ./... -count=1 2>&1 | grep -E '^(ok|\?|FAIL)'` → 恰好 9 行、没有 `FAIL`：7 行 `ok`（`cmd/aite-edge`、`internal/{aiteerr,config,feishu,ingress,sandbox,server}`）
     + 2 行 `?   … [no test files]`（`gen/aitepb`、`internal/pin`）。这是「Go 9 包」的数法，以实测为准。
   - `ls edge/internal/egress docs/p1/egress.md` → 两个都「No such file or directory」（确认没有别人的半成品）。
   - `grep -n NetworkEvent review/paste-T0.md` —— 若 T0 的派单（D0 里一起提交的）给了 `NetworkEvent` 字段表，以它为准实现，并在回执写明；
     没给（或文件不在）就按本派单 §5 第 7 项。只有出现逐字段表（键名 + 类型）才算给了；只点名 `NetworkEvent`、指回 CC11 的句子（如「W1 的接缝」一段、回执「契约缺口」一条）不算，按本派单 §5 第 7 项。
     T0 派单里若有这张表，它照抄的就是本派单 §5 第 7 项那 13 个键，两边应逐字一致；真有出入，按 paste-T0 实现，并在回执第 10 项逐项点名与 §5 第 7 项的差异。
     这一步只读 D0 里的派单，别去翻 T0 还在跑的分支。

## 5. 工作项

**总纪律**：只 import 标准库（连本仓库别的 `internal` 包也不需要）；**不跑 `go get` / `go mod …`**；不用 `http.DefaultTransport` / `http.DefaultClient` / `http.Get`
（它们读 `HTTP(S)_PROXY` 环境变量 —— 云端环境本身就可能设了代理，测试会莫名其妙地绕出去）。上游连接一律走 `Config.Dial`；给上游用的 `http.Transport` 显式 `Proxy: nil`。
日志用 `log/slog`，事件名照 `docker.go:20-22` 的风格：`egress.denied`、`egress.upstream_error`、`egress.audit_failed`。**令牌原文永不进日志、永不进审计行。**

1. **公开 API（DD8 照这个接，签名定死）**：
   ```go
   func New(cfg Config) *Proxy
   func (p *Proxy) Serve(ctx context.Context, l net.Listener) error
   func (p *Proxy) Register(token string, pol Policy) error
   func (p *Proxy) Unregister(token string)
   type Config struct {
       AuditDir string                                                     // 缺省 "data/audit/network"
       Now      func() time.Time                                           // 缺省 time.Now；测试注入假时钟
       Dial     func(ctx context.Context, network, addr string) (net.Conn, error) // 缺省 net.Dialer{Timeout: 10s}.DialContext；测试把 host:port 映射到 httptest
       Logger   *slog.Logger                                               // 缺省 slog.Default()
   }
   type Level string // "none" | "trusted" | "custom" | "full"（与 T0 的 SandboxNetwork 字符串逐一相同）
   type Policy struct { Level Level; AllowHosts []string; AuditTag string; Credentials []CredentialBinding }
   type CredentialBinding struct { Host, Header, Scheme, SecretRef string } // 形状照 T0 契约；本轨只用来「拒收」
   ```
   卡片写的是 `Register(token, Policy)`，没写返回值；本轨定为 **返回 `error`**（凭证绑定要能被拒，见第 8 项）。`Register` 同一令牌再调一次 = 覆盖策略（DD6 换策略用）。
   **被拒的 `Register` 不改动该令牌已有的登记**（先整份校验、再动表）：之前登记过的令牌仍按旧策略放行，之前没登记的仍是 `unknown_token`；
   这条由 `TestCredentialBindingsRejectedUntilEE10` 的子用例钉住（见测试表）。
   `Register` 的拒绝用导出的哨兵错误（如 `ErrInvalidPolicy`、`ErrCredentialsUnsupported`），调用方用 `errors.Is` 判：空令牌、未知档位、
   `AllowHosts` 里写 `*` 或语法不对的模式、`AllowHosts` 里有能匹配到保留主机的模式（第 4 项）→ `ErrInvalidPolicy`。
2. **令牌识别**：客户端由 `Proxy-Authorization` 里的令牌识别（一个沙箱一个）。DD8 会注入 `HTTPS_PROXY=http://aite:<token>@<代理地址>`，
   所以认 `Basic base64(<用户名>:<令牌>)`（密码为空时取用户名）与 `Bearer <令牌>`。缺头 / 解不出 → **407** `missing_token`；
   令牌没登记（含 `Unregister` 之后）→ **407** `unknown_token`；两者都带 `Proxy-Authenticate: Basic realm="aite-egress"`，**都不拨上游**，**都写审计行**（`audit_tag` / `level` 为空）。
3. **档位与主机匹配**：
   - 模式 = 精确主机名，或 `*.后缀`；可带 `:端口`；**不带端口 = 只放 80 / 443**。比较前主机名转小写、去掉结尾的 `.`。
   - `*.example.test` 匹配 `a.example.test`、`a.b.example.test`；**不匹配**顶级 `example.test`，也**不匹配** `badexample.test`（后缀前必须是 `.`）。
   - `none`：一律 403 `level_none`。`trusted`：`china-trusted` 预设 ∪ `AllowHosts`。`custom`：只有 `AllowHosts`。`full`：任意主机的 80 / 443 ∪ `AllowHosts`（NEW12；
     「full 要 `sandbox.allow_full` 才能开」是 DD8 / EE10 的配置闸门，不在本包）。
   - 判定顺序：令牌 → `level_none` → 保留主机（第 4 项）→ 主机匹配（`host_not_allowed`）→ 端口（`port_not_allowed`）。
4. **`china-trusted` 预设（常量表）+ 保留主机**：
   - 预设是包级常量表（切片字面量，永不改写；导出访问函数返回副本）。起点（每条在 `docs/p1/egress.md` 写清用途；增删在回执说理由）：
     `pypi.tuna.tsinghua.edu.cn`、`mirrors.tuna.tsinghua.edu.cn`（清华 PyPI / apt）；`mirrors.aliyun.com`（阿里 PyPI / apt）；`mirrors.cloud.tencent.com`（腾讯）；
     `repo.huaweicloud.com`、`mirrors.huaweicloud.com`（华为）；`registry.npmmirror.com`、`cdn.npmmirror.com`（npmmirror）；`goproxy.cn`；
     `mirrors.ustc.edu.cn`（中科大 crates 索引 / apt）；`modelscope.cn`、`www.modelscope.cn`（模型下载**主**）；`hf-mirror.com`（**辅**：志愿者维护，无 SLA）。
   - **`rsproxy.cn` 不列**（原卡修订：大陆实测通过后才加；云端测不了）。
   - **别从云端去探测镜像站**（云端不在大陆、可能没网，测试也永不拨真主机）。拿不准的下载域（ustc 的 dl 域可能是 `crates-io.proxy.ustclug.org`、
     hf-mirror / modelscope 的 CDN）**不自己加进预设**，写进回执「没做的与原因」待总管本机核实。
     **禁止宽通配**（`*.aliyuncs.com`、`*.cn`、`*.com` 这类）：宽通配会把模型 API 端点、IM 主机一起放出去。要 CDN / OSS 子域就精确列主机。
   - 回执里给一张对照表：CC12 卡片要烘进镜像的四份运行时配置（pip.conf → tuna / aliyun、.npmrc → npmmirror、GOPROXY=goproxy.cn、cargo → ustc）各指向哪个主机、是否在预设里；
     四行都应在预设里（cargo 已按总管 2026-09-25 修订指向 ustc，覆盖 CC12 原卡的 rsproxy）。你看不到 CC12 的分支，以本条为准。
   - **保留主机**（IM / API）：`open.feishu.cn`、`api.dingtalk.com`、`mcp.dingtalk.com`、`qyapi.weixin.qq.com`。它们**永不**进任何匿名预设；
     只能作为带凭证的连接主机到达（EE10）—— 而本轨拒收凭证绑定，所以在本轨**任何档位都到不了**：`Register` 拒收能匹配到它们的 `AllowHosts`（第 1 项），
     请求时再兜一道（`full` 档也拒，403 `im_api_host`）。想加 `oapi.dingtalk.com` 之类别的主机 → 写进回执「记账转出去的」，别自己扩表。
5. **默认拒绝 = 403 + 原因**：拒绝响应带头 `X-Aite-Egress-Reason: <原因码>`，正文纯文本含原因码与 `host:port`。原因码表（写进 `docs/p1/egress.md`，测试按它断言）：
   `missing_token` / `unknown_token`（407）、`level_none` / `im_api_host` / `host_not_allowed` / `port_not_allowed`（403）、
   `bad_request`（400：CONNECT 目标不是 `host:port`、absolute-URI 不是 `http://`、origin-form 请求）、`upstream_error`（502：放行了但连不上上游，审计里 decision 仍记 `allow`）。
6. **CONNECT 隧道 + absolute-URI 转发**：
   - CONNECT：判定放行 → `Config.Dial` 拨上游 → 回 `200 Connection Established` → hijack 后双向拷贝，分别计 `bytes_up` / `bytes_down`；隧道关了再写审计行。
   - absolute-URI（`GET http://host/path`）：`req.Clone` 后清 `RequestURI`、**剥掉逐跳头**（`Proxy-Authorization`、`Proxy-Connection`、`Connection` 及其点名的头、`Keep-Alive`、`TE`、`Trailer`、`Upgrade`），
     用自建 `http.Transport{Proxy: nil, DialContext: cfg.Dial}` 发出去。令牌若透传给上游 = 泄露，测试要断言上游收到的请求里没有 `Proxy-Authorization`。
   - `Serve`：用 `http.Server`（设 `ReadHeaderTimeout`）；ctx 取消 → 停止接受、关掉监听、**关掉所有在途隧道**（hijack 过的连接 `http.Server.Close` 不管，要自己记账），然后返回 `nil`。
7. **审计：每请求一行 `NetworkEvent` JSONL，按小时分文件**：
   - 路径 `<AuditDir>/YYYYMMDDHH.jsonl`；**小时按 UTC**（与 store 的 `created_at` 同为 UTC，store/src/lib.rs:79-83；无夏令时歧义；EE12 导出时再换算）。
     一行只属于它 `ts` 所在小时的文件（跨小时的长隧道仍写进开始那个小时的文件，文件名 ⇔ ts 范围一一对应，EE12 好合并）。
   - 目录按需建；以 `O_APPEND|O_CREATE|O_WRONLY` 追加，每行一次 `Write`，并发写有锁；权限位显式写：文件 `0o644`、目录 `0o755`（= evidence 用 Rust 默认值在 umask 022 下的结果；DD8 接线后 CI 的 ⑤ 要求 ./data 下文件宿主机读得动，ci.yml:206）。
   - 字段（13 个，键名逐字；T0 派单若已给字段表以它为准，见开场自检第 5 步）：
     `ts`（请求开始，UTC，固定 6 位小数 + `Z`）、`audit_tag`、`level`、`method`、`host`（规范化后、不含端口）、`port`（int）、`decision`（`allow` / `deny`）、
     `reason`（放行为 `""`）、`status`（回给沙箱的状态码）、`bytes_up`、`bytes_down`、`duration_ms`、`injected`（本轨恒 `false`，EE10 起注入时 `true`）。
     **不记**：令牌、任何请求 / 响应头、path、query、正文（URL 里常带签名与令牌）。
   - 写审计失败：`slog` 错误日志 `egress.audit_failed`，**不拦请求**（不 fail-closed）；这是本派单替你做的默认，写进 egress.md，并在回执「没做的与原因」请总管拍板。
8. **凭证绑定一律拒收（直到 EE10）**：`Policy.Credentials` 非空 → `Register` 返回 `ErrCredentialsUnsupported`（错误文本点名「EE10 之前不支持凭证注入」），该令牌**不登记**。
9. **`docs/p1/egress.md`（中文）**：定位（CT17 / CT18 / CT27 / NEW11 / NEW12，不接线）；公开 API 与语义；令牌形状与 DD8 的 `HTTPS_PROXY` 写法；档位表；匹配规则；
   预设表（主机 | 用途 | 对应 CC12 哪份配置；`modelscope.cn` / `www.modelscope.cn` 标「主」、`hf-mirror.com` 标「辅（志愿者，无 SLA）」；另注一句 `rsproxy.cn` 待大陆实测）；保留主机与原因；原因码表；`NetworkEvent` 字段表 + 一行样例 + 文件命名；已知限制（第 10 项）；
   与 T0 契约的对照（`Level` ↔ `SandboxNetwork`、`Policy` ↔ `EgressPolicy`、`CredentialBinding`、`NetworkEvent` ↔ `domain.rs`）。
10. **已知限制，只写进文档与回执、不做**：`Unregister` 只挡新请求、不掐已建立的隧道；`full` 档对回环 / 内网 / 云元数据地址（`127.0.0.1`、`169.254.169.254` 等）没有额外防护；
    单进程内存表、重启即空（DD8 起代理时重新登记）。

**新测试（名字逐字；每条后面引号里是英文原卡验收括号里的原话）**：

| 测试 | 原卡 | 钉什么 |
|---|---|---|
| `TestAllowExact` | "allow exact" | `custom` + 精确模式：absolute-URI `GET http://mirror.test/x` 经代理拿到 httptest 的正文；上游收到的请求**没有** `Proxy-Authorization` / `Proxy-Connection`；审计 decision=allow |
| `TestAllowSuffixWildcard` | "allow \*.suffix" | `*.mirror.test` 放 `a.mirror.test`、`a.b.mirror.test`；拒 `mirror.test` 顶级与 `badmirror.test`（403 `host_not_allowed`） |
| `TestDenyDefault403` | "deny default 403" | 不在放行集的主机 → 403、`X-Aite-Egress-Reason: host_not_allowed`、正文含原因码；**拨号次数 = 0**；审计 decision=deny；`none` 档 → `level_none` |
| `TestMissingOrUnknownTokenDenied` | "missing/unknown token denied" | 无头 → 407 `missing_token`；错令牌 → 407 `unknown_token`；`Unregister` 后原令牌 → `unknown_token`；三种都不拨号、都有审计行 |
| `TestIMAPIHostsNotInTrustedPreset` | "IM API hosts not in the trusted preset" | 四个保留主机逐个过**匹配器**（不是字符串比较），预设在 80 / 443 上都不中；`trusted` / `full` 档请求它们 → 403 `im_api_host`；`custom` 的 `AllowHosts` 写它们或 `*.dingtalk.com` → `Register` 报 `ErrInvalidPolicy` |
| `TestConnectTunnelToTLSServerMovesBytes` | "CONNECT tunnel to an httptest TLS server moves bytes" | `httptest.NewTLSServer`；CONNECT `mirror.test:443`（`Dial` 映射到它）→ 200 → 在隧道上做 TLS（`RootCAs` 用 `srv.Certificate()`，`ServerName: "example.com"` —— httptest 自带证书签的名，只用于校验、不产生任何对外连接）→ 读到正文；审计行 `bytes_up > 0`、`bytes_down > 0`、status 200 |
| `TestAuditLineSchema` | "audit line schema" | 解析成 `map[string]any`：键集合**恰好**是那 13 个、类型对、`ts` 能按固定格式解析且是 UTC、`injected == false`；令牌原文不在审计文件里，也不在任何捕获的日志记录里 |
| `TestHourlyRotationWithInjectedClock` | "hourly rotation with an injected clock" | 假时钟 `10:59:59.5Z` 发一个请求、推到 `11:00:00.5Z` 再发一个 → `…10.jsonl` 与 `…11.jsonl` 各 1 行，每行 `ts` 落在自己文件的小时里；全程不睡 |
| `TestCredentialBindingsRejectedUntilEE10` | 原卡 goal："rejected as unsupported until EE10" | `Credentials` 非空 → `errors.Is(err, ErrCredentialsUnsupported)`；该令牌随后的请求 → `unknown_token`；子用例：先正常登记 → 再带 `Credentials` 重登记被拒 → 旧策略仍放行 |
| `TestDefaultPorts80And443` | 原卡 goal："on ports 80/443 by default" | 无端口模式放 :80、:443，拒 :22 / :8443（`port_not_allowed`）；`mirror.test:8443` 模式放 :8443 |
| `TestServeReturnsOnContextCancel` | 原卡 goal：`Serve(ctx, net.Listener) error` | 建一条隧道后取消 ctx → `Serve` 2 秒内返回 `nil`，隧道那头读到 EOF；隧道两端的 goroutine 都已退出（用 WaitGroup / 计数器断言，`-race` 不报 goroutine 泄漏、证明不了这条） |

**测试封闭**：所有上游都是 `httptest`；测试的 `Dial` 只认一张「主机:端口 → httptest 地址」映射表，**表外地址直接 `t.Errorf` 并返回错误**（任何测试想连公网都会当场红）。
测试客户端显式设 `Transport.Proxy = http.ProxyURL(<代理地址，带令牌>)`，不依赖环境变量。主机名一律用 `.test` 后缀（RFC 6761），预设里的真实主机名只喂给匹配器、永不拨号。

## 6. 规则

- **可写面 / 只读面**见 §3；需要改可写面以外的文件 → 写进回执「记账转出去的」，不动手。
- **B8 不变量**（`evals/p0` 钉死，弄红就停下报告）：场景 01/03/04/05/06/10 的 `send_text` 恰好 1 条、02 恰好 2 条；顶层 @ 恰好建 1 个 Task 会话（01 `sessions_equals 1`）；
  `!stop` 仍立即释放沙箱（07 `release min:1`）；对 bot 不加表情（09 `add_reaction == 0`）；`evals/p0/*.yaml` 一个字都不许改。本轨不碰 core，B8 应当逐字不变。
- **守卫**：被拦就停（开场自检第 2 步那次除外：那次被拦就是通过）、拦截原文进回执、不许换写法绕。云端命令里永不出现：`AITE_RELOCK=1`、守卫点名的路径（`edge/go.mod`、`edge/go.sum`、`.contracts.lock`、
  `.claude/settings.json`、`guard_bash.py`、`proto/…` 路径、`core/crates/proto`）、包着它们的 `$(…)`；check.sh 不接 `| tail`。
  多行脚本、PR 描述、回执、多行提交信息**一律先用 Write 工具落文件再用**（`python3 <文件>` / `gh pr create --draft --title "CC11: 出网代理包（不接线）" --body-file <文件>` /
  `gh pr edit --body-file <文件>` / `git commit -F <文件>`；命令行 `-m` 只写单行）；不走 Bash heredoc / `echo >`、不传多行 `--body "…"`（跨行引号会被判「无法解析」，
  守卫也扫 heredoc 正文 —— 回执里贴的拦截原文本身就带受保护路径名）。
- **每个行为改动**：回归测试 + 变异验证（撤回改动 → 测试红 → 贴输出）。还原用 `git checkout -- <文件>` 或 `git stash pop`，**别用 `cp -p` / `shutil.copy2`**。
  建议的变异：匹配器改成不带 `.` 的 `HasSuffix`（→ `TestAllowSuffixWildcard` 红）；拒绝码改 200（→ `TestDenyDefault403` 红）；跳过令牌检查；预设里加 `*.feishu.cn`；
  去掉请求时的保留主机兜底（→ full 子用例红）；隧道只拷一个方向；删掉逐跳头剥离；改一个 json 键名；文件名改用固定名或 `time.Now()`；去掉凭证拒收；端口不判；ctx 取消不关隧道。每条至少一个。
- **格式化**：`gofmt -w <改过的文件>`。check.sh 的 A4c / A4d 跑 `go vet ./...` 与 gofmt 检查。
- **新第三方依赖、R0 文件（不在你可写面里的）、锁定面** → 停下报告。本轨**只用标准库**；`edge/internal/pin` 里钉着的库也不用。
- **Docker 测试封闭**（本轨用不到 Docker；DD8 的 docker 测试才会打你的代理）。云端 protoc 生成的 `edge/gen` 永不提交（本轨不跑 `make proto-gen`）。
- **已知时序抖动**（先单跑再下结论）：`aite --test graceful_shutdown`、`--test startup_recovery`、`--test reconnect_replay`、
  `aite-testing` 的 `hold_spends_scheduler_ticks_not_wall_clock`、Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）。你自己的测试不许靠 `time.Sleep` 等时序。

## 7. 验收（命令 + 期望输出）

带管道的命令（第 3、4、5、6 条）都放在下面的代码块里，照原样敲；表格里只放不带 `|` 的命令。

```
# 3 → exit 0，打印 0
cd edge && go vet ./... && gofmt -l . | wc -l

# 4 → 打印 0（本包及其测试的全部传递依赖都是标准库；-deps -test 连 package egress_test 的黑盒测试一起算）
cd edge && go list -deps -test -f '{{if not .Standard}}{{.ImportPath}}{{end}}' ./internal/egress | grep -v -e '^$' -e '^aite/edge/internal/egress' | wc -l

# 5 → 无输出（注释里别写这几个名字；若有命中，只允许是注释行，回执里逐行说明）
grep -rnE 'http\.(Get|Post|PostForm|Head)\(|http\.Default(Client|Transport)' edge/internal/egress --include='*.go'

# 6 → 恰好 10 行（基线 9 + ok aite/edge/internal/egress），没有 FAIL
cd edge && go test -race ./... -count=1 2>&1 | grep -E -e '^ok' -e '^\?' -e '^FAIL'
```

| # | 命令 | 期望 |
|---|---|---|
| 1 | `cd edge && go test -race ./internal/egress/... -count=1 -v` | 上表 11 条逐个 `--- PASS`，末行 `ok  	aite/edge/internal/egress`；没有 `FAIL`、没有 `DATA RACE` |
| 2 | `cd edge && go test -race ./internal/egress/... -count=5` | `ok`（连跑 5 遍不抖） |
| 3–6 | 见上面代码块 | 见代码块注释 |
| 7 | `scripts/check.sh` | 末行「全部通过」、exit 0；`cargo passed=<897 或 901> failed=0`（**Δ = 0**：本轨不加 Rust 测试）、`contracts passed=<25 或 27> failed=0`、`OK 25 files`、`passed 10/10`；A4c / A4d `-> exit 0`；Go 那格 8 行：7 行 `ok`（含 `ok  aite/edge/internal/egress`）+ 1 行 `?   aite/edge/internal/pin [no test files]`，没有 `FAIL`（现在是 10 个包，被截掉的是 `cmd/aite-edge` 与 `gen/aitepb`）；单跑 `cd edge && go test -race ./cmd/... -count=1` → `ok` |
| 8 | `git diff --name-only origin/main...HEAD` | 每一行都以 `edge/internal/egress/` 开头，或正好是 `docs/p1/egress.md`、`review/p1/ledger/CC11.md` |
| 9 | `git status --short` | 空（临时脚本、探针都清掉；测试只写 `t.TempDir()`，仓库里不留 `data/audit/…`） |

- 第 7 行的基线数按开场自检第 1 步判定的情形取（A：897 / 25；B：901 / 27）。**别接 `| tail`**。
- 原卡「reviewer 另外确认 Go 模块文件没被改」这一条由**总管审 PR 时**看；你别在命令里 grep 或 diff 那两个文件（守卫点名路径）。第 8 行本身已经保证它们不在 diff 里。
- 本机人工步骤：无（本轨不接线、不碰受保护面、不需要真网络）。

## 8. 回执（写 `review/p1/ledger/CC11.md`，PR 描述贴摘要）

1. **开场自检原文**（4 + 1 项）：第 1 步 diff 输出与判定（A / B）；守卫拦截原文（逐字）；三条工具链版本 + 两个 codegen 插件版本（对不上只记一笔）；check.sh 各行原样；第 5 步的 Go 9 行、`ls` 结果、`grep NetworkEvent review/paste-T0.md` 的结果与你据此选了哪份字段表。
2. **工作项逐条**：1–10 每项落在哪些 `文件:行`；公开 API 最终签名（原样贴）；预设表终稿 + 与 CC12 四份运行时配置的对照表；原因码表终稿。
3. **新增测试逐条 + 变异验证输出**：11 条每条钉什么；变异怎么做的；红的那段输出逐字贴（`--- FAIL` 那几行 + 断言信息）。
4. **check.sh 完整输出**（原样，不截）；外加单跑 `cmd/aite-edge` 的那行。
5. **`cargo passed` 增量逐条**：Δ = 0（写明「本轨不改 Rust」）；另列 **Go 测试增量**：`internal/egress` 新增 11 条（按文件列名），Go 包数 9 → 10。
6. **审计行样例**：贴一行真实写出的 JSONL（来自测试的 TempDir）+ 一个小时文件名样例，给 T0 / EE12 对照。
7. **被守卫拦过的命令与拦截原文**（没有就写「无」）。
8. **记账转出去的**（表格：事项 | 为什么不在本轨 | 建议归哪轨）。至少考虑：在 `main.go` 起代理、沙箱内网、注入 `HTTP(S)_PROXY`、`EdgeStatus.egress_ok`（DD8）；
   `docker.go:537-539` 不校验 network 串（DD8）；`full` 档的 `allow_full` 闸门（DD8 / EE10）；只对带凭证主机做 TLS 终结 + 注入 + Aite CA（EE10）；
   `full` 档对回环 / 内网 / 元数据地址的防护（EE10）；`Unregister` 掐在途隧道（DD6 / DD8）；网络日志 ≥190 天留存与小时导出（EE12）；
   保留主机表是否加 `oapi.dingtalk.com` 等（EE10）；网络事件检索（FF7）。另外这三行必须写（事项 → 为什么不在本轨 → 归哪轨）：
   `rsproxy.cn` 进预设 → 需大陆实测，云端测不了 → 总管 / EE10；
   `Register` 返回 `error`（偏离原卡签名），DD8 接线时要处理错误 → 原卡没写返回值，本轨要能拒收凭证与坏模式 → DD8；
   `*` 不进 `AllowHosts`，NEW12 的 `*` 由 DD6 映射为 `level=full` → 本包对 `*` 模式报 `ErrInvalidPolicy` → DD6。
9. **没做的与原因**：包括「写审计失败不拦请求」这条默认请总管拍板；预设里任何你拿不准用途或下载域的主机。
10. **契约缺口**（给 T0 / T0.1）：把 `NetworkEvent` 的 13 个字段（键名、类型、`ts` 格式、UTC 小时文件名）原样列出，请 T0 的 `domain.rs` `NetworkEvent` 逐字段对齐；
    `Level` 四个字符串与 `SandboxNetwork` 的对应；`Policy` 与 `EgressPolicy {level, allow_hosts, credentials, audit_tag}` 的对应（`AuditTag` ↔ `audit_tag`）。
    若 T0 的字段表（开场自检第 5 步看到的）与你实现的不同，逐项点名差异，**不自己去迁就一个还没合并的稿子**，总管合并前统一。
    除此之外的缺口，每条写清需要什么形状、为什么开放通道（`AuditEvent.detail` 等）绕不过去；没有就写「无」。
