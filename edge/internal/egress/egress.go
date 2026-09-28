// Package egress 是沙箱的出网代理（CT17 / CT18 / CT27 / NEW11 / NEW12）。
//
// owner: CC11（只建包、不接线；DD8 在 cmd/aite-edge 里起它）。要点：
//
//   - HTTP CONNECT + absolute-URI（http://）正向代理；别的请求形状一律 400。
//   - 客户端按 Proxy-Authorization 里的沙箱令牌识别（一个沙箱一个令牌，
//     DD8 注入 HTTPS_PROXY=http://aite:<token>@<代理地址>）。令牌原文永不进日志、永不进审计行。
//   - 默认拒绝：档位 none / trusted（china-trusted 预设 ∪ AllowHosts）/ custom（只 AllowHosts）/
//     full（任意主机的 80 / 443 ∪ AllowHosts）；IM / API 保留主机任何档位都到不了（EE10 之前）。
//   - 每个请求一行 NetworkEvent JSONL，按 UTC 小时分文件；写审计失败只打日志、不拦请求。
//   - 上游连接一律走 Config.Dial；不碰 http.DefaultTransport（它读 HTTP(S)_PROXY 环境变量）。
//
// 日志事件名：`egress.denied` / `egress.upstream_error` / `egress.audit_failed`。
package egress

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// Config 的零值字段在 New 里取缺省。
type Config struct {
	AuditDir string                                                            // 缺省 "data/audit/network"
	Now      func() time.Time                                                  // 缺省 time.Now；测试注入假时钟
	Dial     func(ctx context.Context, network, addr string) (net.Conn, error) // 缺省 net.Dialer{Timeout: 10s}.DialContext
	Logger   *slog.Logger                                                      // 缺省 slog.Default()
}

// DefaultAuditDir 是 Config.AuditDir 的缺省值。
const DefaultAuditDir = "data/audit/network"

// HeaderReason 是拒绝响应里携带原因码的头。
const HeaderReason = "X-Aite-Egress-Reason"

// 原因码（docs/p1/egress.md 的原因码表；测试按它断言）。
const (
	ReasonMissingToken   = "missing_token"    // 407
	ReasonUnknownToken   = "unknown_token"    // 407
	ReasonLevelNone      = "level_none"       // 403
	ReasonIMAPIHost      = "im_api_host"      // 403
	ReasonHostNotAllowed = "host_not_allowed" // 403
	ReasonPortNotAllowed = "port_not_allowed" // 403
	ReasonBadRequest     = "bad_request"      // 400
	ReasonUpstreamError  = "upstream_error"   // 502（审计 decision 仍记 allow）
)

// Proxy 是出网代理。Register / Unregister 可与 Serve 并发调用；Serve 每个 Proxy 只调一次。
type Proxy struct {
	now       func() time.Time
	dial      func(ctx context.Context, network, addr string) (net.Conn, error)
	log       *slog.Logger
	audit     *auditWriter
	transport *http.Transport

	polMu    sync.RWMutex
	policies map[string]*compiledPolicy

	// 在途请求与隧道的记账：http.Server.Close 不管 hijack 过的连接，得自己关。
	trackMu  sync.Mutex
	closed   bool
	conns    map[net.Conn]struct{}
	inflight sync.WaitGroup
	copiers  atomic.Int64 // 活着的隧道拷贝 goroutine 数（测试断言 Serve 返回后归零）

	onAudit func(NetworkEvent) // 测试钩子：审计行写完之后调
}

// New 建一个代理；不监听，监听交给 Serve。
func New(cfg Config) *Proxy {
	if cfg.AuditDir == "" {
		cfg.AuditDir = DefaultAuditDir
	}
	if cfg.Now == nil {
		cfg.Now = time.Now
	}
	if cfg.Dial == nil {
		cfg.Dial = (&net.Dialer{Timeout: 10 * time.Second}).DialContext
	}
	if cfg.Logger == nil {
		cfg.Logger = slog.Default()
	}
	return &Proxy{
		now:   cfg.Now,
		dial:  cfg.Dial,
		log:   cfg.Logger,
		audit: &auditWriter{dir: cfg.AuditDir},
		transport: &http.Transport{
			Proxy:                 nil, // 显式：永不读 HTTP(S)_PROXY 环境变量
			DialContext:           cfg.Dial,
			DisableCompression:    true, // 原样透传 Content-Encoding
			MaxIdleConnsPerHost:   4,
			IdleConnTimeout:       90 * time.Second,
			ResponseHeaderTimeout: 60 * time.Second,
		},
		policies: make(map[string]*compiledPolicy),
		conns:    make(map[net.Conn]struct{}),
	}
}

// Register 登记（或覆盖）一个沙箱令牌的策略。先整份校验、再动表：被拒时该令牌
// 已有的登记原样保留（之前没登记的仍是 unknown_token）。拒绝用 ErrInvalidPolicy /
// ErrCredentialsUnsupported，调用方用 errors.Is 判。
func (p *Proxy) Register(token string, pol Policy) error {
	if token == "" {
		return fmt.Errorf("%w: 令牌为空", ErrInvalidPolicy)
	}
	c, err := compile(pol)
	if err != nil {
		return err
	}
	p.polMu.Lock()
	p.policies[token] = c
	p.polMu.Unlock()
	return nil
}

// Unregister 吊销令牌：之后的新请求得 407 unknown_token。已建立的隧道不掐（已知限制）。
func (p *Proxy) Unregister(token string) {
	p.polMu.Lock()
	delete(p.policies, token)
	p.polMu.Unlock()
}

func (p *Proxy) lookup(token string) *compiledPolicy {
	p.polMu.RLock()
	defer p.polMu.RUnlock()
	return p.policies[token]
}

// Serve 在 l 上提供代理服务，直到 ctx 取消：停止接受、关掉监听、关掉所有在途隧道，
// 等在途请求收尾后返回 nil。监听本身出错则同样收尾后返回该错误。
func (p *Proxy) Serve(ctx context.Context, l net.Listener) error {
	srv := &http.Server{
		Handler:           p,
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       120 * time.Second,
		ErrorLog:          slog.NewLogLogger(p.log.Handler(), slog.LevelWarn),
	}
	errc := make(chan error, 1)
	go func() { errc <- srv.Serve(l) }()

	var serveErr error
	select {
	case <-ctx.Done():
	case serveErr = <-errc:
	}
	_ = srv.Close() // 关监听 + 没 hijack 的连接
	p.closeTunnels()
	p.inflight.Wait()
	p.transport.CloseIdleConnections()
	if serveErr == nil {
		serveErr = <-errc
	}
	if errors.Is(serveErr, http.ErrServerClosed) {
		return nil
	}
	return serveErr
}

// enter 给一个请求记账；Serve 收尾之后返回 false。
func (p *Proxy) enter() bool {
	p.trackMu.Lock()
	defer p.trackMu.Unlock()
	if p.closed {
		return false
	}
	p.inflight.Add(1)
	return true
}

// trackConn 登记一条隧道连接，Serve 收尾时统一关掉；已在收尾则返回 false。
func (p *Proxy) trackConn(c net.Conn) bool {
	p.trackMu.Lock()
	defer p.trackMu.Unlock()
	if p.closed {
		return false
	}
	p.conns[c] = struct{}{}
	return true
}

func (p *Proxy) untrackConn(c net.Conn) {
	p.trackMu.Lock()
	delete(p.conns, c)
	p.trackMu.Unlock()
}

func (p *Proxy) closeTunnels() {
	p.trackMu.Lock()
	defer p.trackMu.Unlock()
	p.closed = true
	for c := range p.conns {
		_ = c.Close()
	}
}

// target 是请求要去的上游（已规范化）。
type target struct {
	host string
	port int
}

func (t target) String() string { return net.JoinHostPort(t.host, strconv.Itoa(t.port)) }

// parseTarget：CONNECT 要 host:port；其余只收 absolute-URI 的 http://。
func parseTarget(r *http.Request) (target, bool) {
	if r.Method == http.MethodConnect {
		h, ps, err := net.SplitHostPort(r.Host)
		if err != nil || h == "" {
			return target{}, false
		}
		n, err := strconv.Atoi(ps)
		if err != nil || n < 1 || n > 65535 {
			return target{}, false
		}
		return target{host: normalizeHost(h), port: n}, true
	}
	if !r.URL.IsAbs() || r.URL.Scheme != "http" || r.URL.Hostname() == "" {
		return target{}, false
	}
	port := 80
	if ps := r.URL.Port(); ps != "" {
		n, err := strconv.Atoi(ps)
		if err != nil || n < 1 || n > 65535 {
			return target{}, false
		}
		port = n
	}
	return target{host: normalizeHost(r.URL.Hostname()), port: port}, true
}

// tokenFrom 从 Proxy-Authorization 取令牌：Basic base64(<用户名>:<令牌>)（密码为空时取用户名）
// 或 Bearer <令牌>。解不出返回 ""。
func tokenFrom(h string) string {
	scheme, rest, ok := strings.Cut(strings.TrimSpace(h), " ")
	if !ok {
		return ""
	}
	rest = strings.TrimSpace(rest)
	switch strings.ToLower(scheme) {
	case "basic":
		b, err := base64.StdEncoding.DecodeString(rest)
		if err != nil {
			return ""
		}
		user, pass, _ := strings.Cut(string(b), ":")
		if pass != "" {
			return pass
		}
		return user
	case "bearer":
		return rest
	}
	return ""
}

func (p *Proxy) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if !p.enter() {
		return
	}
	defer p.inflight.Done()

	start := p.now()
	ev := NetworkEvent{TS: stamp(start), Method: r.Method}
	tgt, targetOK := parseTarget(r)
	ev.Host, ev.Port = tgt.host, tgt.port

	token := tokenFrom(r.Header.Get("Proxy-Authorization"))
	if token == "" {
		p.deny(w, start, &ev, http.StatusProxyAuthRequired, ReasonMissingToken, tgt)
		return
	}
	pol := p.lookup(token)
	if pol == nil {
		p.deny(w, start, &ev, http.StatusProxyAuthRequired, ReasonUnknownToken, tgt)
		return
	}
	ev.AuditTag, ev.Level = pol.auditTag, string(pol.level)
	if !targetOK {
		p.deny(w, start, &ev, http.StatusBadRequest, ReasonBadRequest, tgt)
		return
	}
	if reason := pol.decide(tgt.host, tgt.port); reason != "" {
		p.deny(w, start, &ev, http.StatusForbidden, reason, tgt)
		return
	}
	ev.Decision = decisionAllow
	if r.Method == http.MethodConnect {
		p.tunnel(w, r, start, &ev, tgt)
	} else {
		p.forward(w, r, start, &ev, tgt)
	}
}

// deny 回拒绝响应（带原因码头与纯文本正文）并写审计。
func (p *Proxy) deny(w http.ResponseWriter, start time.Time, ev *NetworkEvent, status int, reason string, tgt target) {
	if status == http.StatusProxyAuthRequired {
		w.Header().Set("Proxy-Authenticate", `Basic realm="aite-egress"`)
	}
	w.Header().Set(HeaderReason, reason)
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.WriteHeader(status)
	fmt.Fprintf(w, "aite-egress: %s %s\n", reason, tgt)

	ev.Decision, ev.Reason, ev.Status = decisionDeny, reason, status
	p.log.Info("egress.denied", "reason", reason, "host", tgt.host, "port", tgt.port,
		"method", ev.Method, "audit_tag", ev.AuditTag, "level", ev.Level)
	p.finish(start, ev)
}

// upstreamError 是「放行了但连不上上游」：502，审计 decision 仍是 allow。
func (p *Proxy) upstreamError(w http.ResponseWriter, start time.Time, ev *NetworkEvent, tgt target, err error) {
	w.Header().Set(HeaderReason, ReasonUpstreamError)
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.WriteHeader(http.StatusBadGateway)
	fmt.Fprintf(w, "aite-egress: %s %s\n", ReasonUpstreamError, tgt)

	ev.Reason, ev.Status = ReasonUpstreamError, http.StatusBadGateway
	p.log.Warn("egress.upstream_error", "host", tgt.host, "port", tgt.port,
		"method", ev.Method, "audit_tag", ev.AuditTag, "err", err)
	p.finish(start, ev)
}

// finish 补上时长并写审计行；写失败只打日志、不拦请求。
func (p *Proxy) finish(start time.Time, ev *NetworkEvent) {
	ev.DurationMS = p.now().Sub(start).Milliseconds()
	if err := p.audit.write(start, *ev); err != nil {
		p.log.Error("egress.audit_failed", "host", ev.Host, "port", ev.Port, "err", err)
	}
	if p.onAudit != nil {
		p.onAudit(*ev)
	}
}
