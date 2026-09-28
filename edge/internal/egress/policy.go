package egress

import (
	"errors"
	"fmt"
	"net"
	"strconv"
	"strings"
)

// Level 是沙箱的出网档位。四个字符串与 T0 契约的 SandboxNetwork 逐一相同。
type Level string

const (
	LevelNone    Level = "none"
	LevelTrusted Level = "trusted"
	LevelCustom  Level = "custom"
	LevelFull    Level = "full"
)

// Policy 是一个沙箱令牌的放行策略；形状对齐 T0 的 EgressPolicy
// {level, allow_hosts, credentials, audit_tag}。
type Policy struct {
	Level       Level
	AllowHosts  []string
	AuditTag    string
	Credentials []CredentialBinding
}

// CredentialBinding 形状照 T0 契约。EE10 之前本包只用它来「拒收」：
// 非空的 Policy.Credentials 让 Register 返回 ErrCredentialsUnsupported。
type CredentialBinding struct {
	Host, Header, Scheme, SecretRef string
}

var (
	// ErrInvalidPolicy：空令牌、未知档位、AllowHosts 里有 `*` / 语法不对的模式 /
	// 能匹配到保留主机（IM / API）的模式。
	ErrInvalidPolicy = errors.New("egress: 策略不合法")
	// ErrCredentialsUnsupported：Policy.Credentials 非空。凭证注入要 TLS 终结，归 EE10。
	ErrCredentialsUnsupported = errors.New("egress: EE10 之前不支持凭证注入（Policy.Credentials 必须为空）")
)

// chinaTrusted 是 trusted 档的匿名预设（国内镜像）。包级常量表：只读、永不改写，
// 对外只经 ChinaTrustedPreset 给副本。每条的用途见 docs/p1/egress.md。
// 禁止宽通配（*.aliyuncs.com、*.cn …）：会把模型 API 端点与 IM 主机一起放出去。
var chinaTrusted = []string{
	"pypi.tuna.tsinghua.edu.cn",
	"mirrors.tuna.tsinghua.edu.cn",
	"mirrors.aliyun.com",
	"mirrors.cloud.tencent.com",
	"repo.huaweicloud.com",
	"mirrors.huaweicloud.com",
	"registry.npmmirror.com",
	"cdn.npmmirror.com",
	"goproxy.cn",
	"mirrors.ustc.edu.cn",
	"modelscope.cn",
	"www.modelscope.cn",
	"hf-mirror.com",
}

// reservedHosts 是 IM / API 主机：永不进任何匿名预设，只能作为带凭证的连接主机
// 到达（EE10）。本轨拒收凭证，所以它们在任何档位都到不了。
var reservedHosts = []string{
	"open.feishu.cn",
	"api.dingtalk.com",
	"mcp.dingtalk.com",
	"qyapi.weixin.qq.com",
}

// presetPatterns 是 chinaTrusted 编译后的模式；init 时编译失败直接 panic（常量表写错是程序错）。
var presetPatterns = mustCompile(chinaTrusted)

// ChinaTrustedPreset 返回 china-trusted 预设的副本。
func ChinaTrustedPreset() []string { return append([]string(nil), chinaTrusted...) }

// ReservedHosts 返回保留主机表的副本。
func ReservedHosts() []string { return append([]string(nil), reservedHosts...) }

// pattern 是一条编译后的放行模式：精确主机名或 `*.后缀`，可带端口；port==0 表示只放 80 / 443。
type pattern struct {
	host     string // 规范化后；通配时是后缀（不含开头的 "*."）
	wildcard bool
	port     int
}

// normalizeHost：转小写、去掉结尾的 `.`。
func normalizeHost(h string) string {
	return strings.TrimSuffix(strings.ToLower(h), ".")
}

func parsePattern(s string) (pattern, error) {
	raw := strings.TrimSpace(s)
	host, port := raw, 0
	if strings.Contains(raw, ":") {
		h, ps, err := net.SplitHostPort(raw)
		if err != nil {
			return pattern{}, fmt.Errorf("%w: 模式 %q：%v", ErrInvalidPolicy, s, err)
		}
		n, err := strconv.Atoi(ps)
		if err != nil || n < 1 || n > 65535 {
			return pattern{}, fmt.Errorf("%w: 模式 %q：端口不合法", ErrInvalidPolicy, s)
		}
		host, port = h, n
	}
	host = normalizeHost(host)
	p := pattern{host: host, port: port}
	if strings.HasPrefix(host, "*.") {
		p.wildcard, p.host = true, host[2:]
	}
	if !validHostname(p.host) {
		// 包括单独的 `*`：放行任意主机是 level=full（NEW12 由 DD6 映射），不是一条模式。
		return pattern{}, fmt.Errorf("%w: 模式 %q：只接受精确主机名或 `*.后缀`", ErrInvalidPolicy, s)
	}
	return p, nil
}

// validHostname：非空、各段非空、只含 [a-z0-9-_]（已转小写）。
func validHostname(h string) bool {
	if h == "" {
		return false
	}
	for _, label := range strings.Split(h, ".") {
		if label == "" || len(label) > 63 {
			return false
		}
		for _, c := range label {
			if !(c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-' || c == '_') {
				return false
			}
		}
	}
	return true
}

// matchHost：精确相等；通配要求 host 以 "."+后缀 结尾（所以不中顶级后缀本身、也不中 badsuffix）。
func (p pattern) matchHost(host string) bool {
	if p.wildcard {
		return strings.HasSuffix(host, "."+p.host)
	}
	return host == p.host
}

func (p pattern) matchPort(port int) bool {
	if p.port == 0 {
		return defaultPort(port)
	}
	return port == p.port
}

func defaultPort(port int) bool { return port == 80 || port == 443 }

func mustCompile(list []string) []pattern {
	out := make([]pattern, 0, len(list))
	for _, s := range list {
		p, err := parsePattern(s)
		if err != nil {
			panic(err)
		}
		out = append(out, p)
	}
	return out
}

func isReservedHost(host string) bool {
	for _, r := range reservedHosts {
		if host == r {
			return true
		}
	}
	return false
}

// compiledPolicy 是 Register 校验通过后存进表里的策略。
type compiledPolicy struct {
	level    Level
	auditTag string
	patterns []pattern // trusted 时已并上预设
}

// compile 整份校验策略；失败时不产生任何副作用（Register 先 compile、再动表）。
func compile(pol Policy) (*compiledPolicy, error) {
	if len(pol.Credentials) > 0 {
		return nil, ErrCredentialsUnsupported
	}
	switch pol.Level {
	case LevelNone, LevelTrusted, LevelCustom, LevelFull:
	default:
		return nil, fmt.Errorf("%w: 未知档位 %q", ErrInvalidPolicy, pol.Level)
	}
	c := &compiledPolicy{level: pol.Level, auditTag: pol.AuditTag}
	if pol.Level == LevelTrusted {
		c.patterns = append(c.patterns, presetPatterns...)
	}
	for _, s := range pol.AllowHosts {
		p, err := parsePattern(s)
		if err != nil {
			return nil, err
		}
		for _, r := range reservedHosts {
			if p.matchHost(r) {
				return nil, fmt.Errorf("%w: 模式 %q 能匹配到保留主机 %s（IM / API 主机只能经 EE10 的凭证注入到达）", ErrInvalidPolicy, s, r)
			}
		}
		c.patterns = append(c.patterns, p)
	}
	return c, nil
}

// decide 按「level_none → 保留主机 → 主机匹配 → 端口」的顺序判定；放行返回 ""。
// 令牌那一步在调用方。host 必须已规范化。
func (c *compiledPolicy) decide(host string, port int) string {
	if c.level == LevelNone {
		return ReasonLevelNone
	}
	if isReservedHost(host) {
		return ReasonIMAPIHost
	}
	hostOK, portOK := false, false
	if c.level == LevelFull {
		hostOK, portOK = true, defaultPort(port)
	}
	for _, p := range c.patterns {
		if p.matchHost(host) {
			hostOK = true
			if p.matchPort(port) {
				portOK = true
			}
		}
	}
	switch {
	case !hostOK:
		return ReasonHostNotAllowed
	case !portOK:
		return ReasonPortNotAllowed
	}
	return ""
}

// presetAllows 只用预设判定（不看档位与保留主机兜底）；测试用它把保留主机逐个过匹配器。
func presetAllows(host string, port int) bool {
	host = normalizeHost(host)
	for _, p := range presetPatterns {
		if p.matchHost(host) && p.matchPort(port) {
			return true
		}
	}
	return false
}
