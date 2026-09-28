# 出网代理包 `edge/internal/egress`（CC11，不接线）

> 对应 Claude Tag：CT17（Agent Proxy：默认拒绝、只放 HTTP(S)、被拦请求可审计）、CT18（网络档位）、
> CT27（每小时网络事件导出）、NEW11（Domains：放行主机但不带凭证）、NEW12（`*` 放行任意主机、默认关）。
> 本包**只建不接**：`edge/cmd/aite-edge/main.go`、`edge/internal/sandbox/docker.go`、`docker-compose.yml` 一个字都没动。
> 接线归 DD8（起代理、沙箱内网、注入 `HTTP(S)_PROXY`、`EdgeStatus.egress_ok`），策略拼装归 DD6，TLS 终结 + 凭证注入归 EE10，
> 导出与留存归 EE12，检索归 FF7。代理上线前沙箱默认仍是 `network=none`。

只用 Go 标准库；上游连接一律走 `Config.Dial`，给上游用的 `http.Transport` 显式 `Proxy: nil`，
不碰 `http.DefaultTransport` / `http.DefaultClient`（它们读 `HTTP(S)_PROXY` 环境变量）。

## 1. 公开 API（DD8 照这个接，签名定死）

```go
func New(cfg Config) *Proxy
func (p *Proxy) Serve(ctx context.Context, l net.Listener) error
func (p *Proxy) Register(token string, pol Policy) error
func (p *Proxy) Unregister(token string)

type Config struct {
	AuditDir string                                                           // 缺省 "data/audit/network"
	Now      func() time.Time                                                 // 缺省 time.Now；测试注入假时钟
	Dial     func(ctx context.Context, network, addr string) (net.Conn, error) // 缺省 net.Dialer{Timeout: 10s}.DialContext
	Logger   *slog.Logger                                                     // 缺省 slog.Default()
}

type Level string // "none" | "trusted" | "custom" | "full"
type Policy struct {
	Level       Level
	AllowHosts  []string
	AuditTag    string
	Credentials []CredentialBinding
}
type CredentialBinding struct{ Host, Header, Scheme, SecretRef string }

var ErrInvalidPolicy, ErrCredentialsUnsupported error // 用 errors.Is 判
func ChinaTrustedPreset() []string // 预设副本
func ReservedHosts() []string      // 保留主机副本
```

语义：

- `New` 只建对象、不监听；零值字段取缺省。
- `Serve` 每个 `Proxy` 只调一次。ctx 取消 → 停止接受、关监听、**关掉所有在途隧道**（hijack 过的连接自己记账）、
  等在途请求收尾，然后返回 `nil`；监听本身出错则同样收尾后返回该错误。
- `Register`：同一令牌再调一次 = 覆盖策略（DD6 换策略用）。**先整份校验、再动表**：被拒时该令牌已有的登记原样保留，
  之前没登记的仍是 `unknown_token`。拒绝：
  - `ErrInvalidPolicy`：空令牌；未知档位（含空串）；`AllowHosts` 里有 `*`、语法不对的模式、或能匹配到保留主机的模式。
  - `ErrCredentialsUnsupported`：`Credentials` 非空（错误文本：「EE10 之前不支持凭证注入」）。
- `Unregister`：之后的新请求得 407 `unknown_token`；**已建立的隧道不掐**（已知限制）。
- 原卡写的是 `Register(token, Policy)`、没写返回值；本包定为返回 `error`（要能拒收凭证与坏模式），DD8 接线时要处理。

## 2. 令牌

一个沙箱一个令牌，客户端由 `Proxy-Authorization` 识别。DD8 注入：

```
HTTPS_PROXY=http://aite:<token>@<代理地址>
HTTP_PROXY=http://aite:<token>@<代理地址>
```

认两种形状：`Basic base64(<用户名>:<令牌>)`（密码为空时取用户名，所以 `http://<token>@<代理地址>` 也行）与 `Bearer <令牌>`。
缺头 / 解不出 → 407 `missing_token`；没登记（含 `Unregister` 之后）→ 407 `unknown_token`。两者都带
`Proxy-Authenticate: Basic realm="aite-egress"`、都不拨上游、都写审计行（`audit_tag` / `level` 为空）。
**令牌原文永不进日志、永不进审计行**；往上游转发前剥掉 `Proxy-Authorization`。

## 3. 档位

| `Level` | 放行集 | 说明 |
|---|---|---|
| `none` | 无 | 一律 403 `level_none`（代理上线前的默认） |
| `trusted` | `china-trusted` 预设 ∪ `AllowHosts` | 中国化：不用海外包源，换国内镜像 |
| `custom` | 只有 `AllowHosts` | NEW11：放行主机但不带凭证 |
| `full` | 任意主机的 80 / 443 ∪ `AllowHosts` | NEW12；「要 `sandbox.allow_full` 才能开」是 DD8 / EE10 的配置闸门，不在本包 |

NEW12 的 `*`（放行任意主机）由 DD6 映射成 `level=full`；本包对 `*` 模式报 `ErrInvalidPolicy`。
任何档位都到不了保留主机（§5）。

## 4. 主机匹配

- 模式 = 精确主机名，或 `*.后缀`；可带 `:端口`。**不带端口 = 只放 80 / 443**。
- 比较前主机名转小写、去掉结尾的 `.`（模式与请求两边都做）。
- `*.example.test` 匹配 `a.example.test`、`a.b.example.test`；**不匹配**顶级 `example.test`，也**不匹配** `badexample.test`（后缀前必须是 `.`）。
- 通配只能出现在最左边（`a.*.test` 非法）；单独的 `*`、`*.`、带 scheme 的 `http://x`、端口 0 / 超界都非法。IPv6 字面量模式不支持。
- 判定顺序：令牌 → `level_none` → 保留主机 → 主机匹配（`host_not_allowed`）→ 端口（`port_not_allowed`）。
- 实际拨号的就是判过的「规范化主机:端口」，不另解析一遍请求里的原串。

## 5. `china-trusted` 预设与保留主机

预设是包级常量表（切片字面量，永不改写；`ChinaTrustedPreset()` 给副本）。**只列精确主机，禁止宽通配**
（`*.aliyuncs.com`、`*.cn`、`*.com` 这类会把模型 API 端点、IM 主机一起放出去；要 CDN / OSS 子域就精确列）。

| 主机 | 用途 | 对应 CC12 哪份配置 |
|---|---|---|
| `pypi.tuna.tsinghua.edu.cn` | 清华 PyPI | `pip.conf`（主 index） |
| `mirrors.tuna.tsinghua.edu.cn` | 清华 apt / 其它镜像 | — |
| `mirrors.aliyun.com` | 阿里 PyPI / apt | `pip.conf`（extra / 备） |
| `mirrors.cloud.tencent.com` | 腾讯 PyPI / apt | — |
| `repo.huaweicloud.com` | 华为 PyPI / npm / Maven | — |
| `mirrors.huaweicloud.com` | 华为 apt 等 | — |
| `registry.npmmirror.com` | npmmirror 包索引 | `.npmrc` |
| `cdn.npmmirror.com` | npmmirror 二进制 / tarball | `.npmrc`（下载） |
| `goproxy.cn` | Go 模块代理 | `GOPROXY=https://goproxy.cn` |
| `mirrors.ustc.edu.cn` | 中科大 crates 索引 / apt | cargo（`config.toml` 指 ustc） |
| `modelscope.cn` | 模型下载（**主**） | — |
| `www.modelscope.cn` | 模型下载（**主**） | — |
| `hf-mirror.com` | HF 镜像（**辅**：志愿者维护，无 SLA） | — |

- `rsproxy.cn` **不列**：待大陆实测通过后再加（云端测不了）。
- 未核实、没加进预设的下载域（待总管本机核实）：ustc 的 crates 下载域（可能是 `crates-io.proxy.ustclug.org`）、
  modelscope / hf-mirror 的 CDN 域。缺它们时 `trusted` 档会在那一步得 403 `host_not_allowed`，审计里能看到具体主机。

**保留主机**（IM / API）：`open.feishu.cn`、`api.dingtalk.com`、`mcp.dingtalk.com`、`qyapi.weixin.qq.com`。
它们**永不**进任何匿名预设，只能作为带凭证的连接主机到达（EE10 的 TLS 终结 + 注入）。本包拒收凭证，所以任何档位都到不了：

- `Register` 拒收能匹配到它们的 `AllowHosts`（精确写或 `*.dingtalk.com` / `*.feishu.cn` 这类通配）→ `ErrInvalidPolicy`；
- 请求时再兜一道：`trusted` / `custom` / `full` 都回 403 `im_api_host`。
- 原因：沙箱里拿到平台主机的匿名通路 = 可以绕开 core 的审批与审计直接对外发消息。

## 6. 原因码

拒绝响应都带头 `X-Aite-Egress-Reason: <原因码>`，正文纯文本 `aite-egress: <原因码> <host:port>`。

| 原因码 | 状态码 | 何时 |
|---|---|---|
| `missing_token` | 407 | 没有 `Proxy-Authorization`，或解不出令牌 |
| `unknown_token` | 407 | 令牌没登记（含 `Unregister` 之后、被拒的 `Register`） |
| `level_none` | 403 | 档位 `none` |
| `im_api_host` | 403 | 目标是保留主机 |
| `host_not_allowed` | 403 | 主机不在放行集 |
| `port_not_allowed` | 403 | 主机中了、端口没中（无端口模式只放 80 / 443） |
| `bad_request` | 400 | CONNECT 目标不是 `host:port`；absolute-URI 不是 `http://`；origin-form 请求 |
| `upstream_error` | 502 | 放行了但连不上上游（审计 `decision` 仍记 `allow`） |

407 / 400 的判定在令牌之后：令牌不对的请求一律先得 407，不论目标长什么样。

## 7. 转发

- **CONNECT**：判定放行 → `Config.Dial` 拨上游 → 回 `200 Connection Established` → hijack 后双向拷贝，分别计
  `bytes_up`（沙箱 → 上游）/ `bytes_down`（上游 → 沙箱）。一个方向读完就对另一端半关写；两个方向都结束后关连接、再写审计行。
  本包不看隧道里的内容（HTTPS 端到端，EE10 之前不做 TLS 终结）。
- **absolute-URI**（`GET http://host/path`）：`req.Clone` 后清 `RequestURI`、剥掉逐跳头（`Proxy-Authorization`、`Proxy-Authenticate`、
  `Proxy-Connection`、`Connection` 及其点名的头、`Keep-Alive`、`TE`、`Trailer`、`Upgrade`），用自建
  `http.Transport{Proxy: nil, DialContext: cfg.Dial, DisableCompression: true}` 发出去；响应头同样剥逐跳头后原样回。
- `https://` 的 absolute-URI 不收（客户端应走 CONNECT）。

## 8. 审计：`NetworkEvent` JSONL

- 路径 `<AuditDir>/YYYYMMDDHH.jsonl`，**小时按 UTC**（与 store 的 `created_at` 同为 UTC；无夏令时歧义；EE12 导出时再换算）。
  一行只属于它 `ts` 所在小时的文件：跨小时的长隧道仍写进**开始**那个小时的文件，文件名 ⇔ ts 范围一一对应。
- 目录按需建（`0o755`），文件 `O_APPEND|O_CREATE|O_WRONLY`（`0o644`），每行一次 `Write`，并发写由锁串行。
  权限位是显式传的，但仍受进程 umask 影响；umask 022 下就是 0755 / 0644（DD8 接线后宿主机要读得动 `./data` 下的文件）。
- 每请求一行，拒绝的也写（含 407 / 400）。

| 键 | 类型 | 含义 |
|---|---|---|
| `ts` | string | 请求开始时刻，UTC，定长 `2006-01-02T15:04:05.000000Z`（与 store `created_at` 同形） |
| `audit_tag` | string | `Policy.AuditTag`；令牌不对时为 `""` |
| `level` | string | 档位；令牌不对时为 `""` |
| `method` | string | `CONNECT` / `GET` / … |
| `host` | string | 规范化后的主机，不含端口 |
| `port` | int | 目标端口（absolute-URI 没写时为 80） |
| `decision` | string | `allow` / `deny` |
| `reason` | string | 原因码；放行为 `""`（`upstream_error` 时 decision=allow、reason=`upstream_error`） |
| `status` | int | 回给沙箱的状态码（隧道为 200） |
| `bytes_up` | int | 沙箱 → 上游字节数（隧道为隧道内全部字节；转发为请求体） |
| `bytes_down` | int | 上游 → 沙箱字节数（隧道为隧道内全部字节；转发为响应体） |
| `duration_ms` | int | 从请求开始到写审计行（隧道 = 隧道存活时长） |
| `injected` | bool | 是否注入了凭证；本轨恒 `false`，EE10 起注入时 `true` |

**不记**：令牌、任何请求 / 响应头、path、query、正文（URL 里常带签名与令牌）。

样例（文件 `2026092510.jsonl`）：

```
SAMPLE_LINE
```

- 写审计失败：打 `slog` 错误日志 `egress.audit_failed`，**不拦请求**（不 fail-closed）。这是派单替本轨定的默认，待总管拍板。

日志事件名：`egress.denied`（Info）、`egress.upstream_error`（Warn）、`egress.audit_failed`（Error）。都不带令牌、path、query。

## 9. 已知限制（本轨不做）

- `Unregister` 只挡新请求，不掐已建立的隧道（DD6 / DD8 若要「吊销即断」再加）。
- `full` 档对回环 / 内网 / 云元数据地址（`127.0.0.1`、`169.254.169.254` 等）没有额外防护（EE10）。
- 单进程内存表，重启即空（DD8 起代理时重新登记）。
- `Serve` 每个 `Proxy` 只能调一次（收尾后内部置 closed，不再接新请求）。
- 转发不做 HTTP/2、不转 trailer；`https://` absolute-URI 不收。

## 10. 与 T0 契约的对照

| 本包 | T0 p1.0 契约 | 说明 |
|---|---|---|
| `Level` 四个字符串 `none` / `trusted` / `custom` / `full` | `SandboxNetwork` `None` / `Trusted` / `Custom` / `Full` | 线上字符串逐一相同 |
| `Policy{Level, AllowHosts, AuditTag, Credentials}` | `EgressPolicy{level, allow_hosts, credentials, audit_tag}` | `AuditTag` ↔ `audit_tag` |
| `CredentialBinding{Host, Header, Scheme, SecretRef}` | `CredentialBinding{host, header, scheme, secret_ref}` | 本轨只用来拒收 |
| `NetworkEvent`（§8 的 13 个键） | `domain.rs` 的 `NetworkEvent` | 键名、个数、类型逐字对齐；要能反序列化本包写出的行 |
