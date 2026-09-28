package egress

import (
	"bufio"
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// fakeClock 是可推进的假时钟；全程不睡。
type fakeClock struct {
	mu sync.Mutex
	t  time.Time
}

func (c *fakeClock) Now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.t
}

func (c *fakeClock) Set(t time.Time) {
	c.mu.Lock()
	c.t = t
	c.mu.Unlock()
}

// lockedBuffer 是加了互斥锁的 io.Writer：请求 goroutine 会并发写日志，裸 bytes.Buffer 在 -race 下会红。
type lockedBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (l *lockedBuffer) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.Write(p)
}

func (l *lockedBuffer) String() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.String()
}

// harness 起一个代理：审计写 t.TempDir()、假时钟、日志进 lockedBuffer（JSON，保留 With 属性）、
// Dial 只认 routes 表里的「主机:端口 → httptest 地址」，表外地址当场 t.Errorf 并返回错误。
type harness struct {
	t      *testing.T
	p      *Proxy
	clock  *fakeClock
	logs   *lockedBuffer
	dir    string
	addr   string // 代理监听地址
	dials  atomic.Int64
	events chan NetworkEvent
	cancel context.CancelFunc
	served chan error

	mu     sync.Mutex
	routes map[string]string
}

var t0 = time.Date(2026, 9, 25, 10, 0, 0, 0, time.UTC)

func newHarness(t *testing.T) *harness {
	t.Helper()
	h := &harness{
		t:      t,
		clock:  &fakeClock{t: t0},
		logs:   &lockedBuffer{},
		dir:    filepath.Join(t.TempDir(), "audit", "network"),
		events: make(chan NetworkEvent, 64),
		routes: make(map[string]string),
		served: make(chan error, 1),
	}
	h.p = New(Config{
		AuditDir: h.dir,
		Now:      h.clock.Now,
		Dial:     h.dial,
		Logger:   slog.New(slog.NewJSONHandler(h.logs, &slog.HandlerOptions{Level: slog.LevelDebug})),
	})
	h.p.onAudit = func(ev NetworkEvent) { h.events <- ev }
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	h.addr = l.Addr().String()
	ctx, cancel := context.WithCancel(context.Background())
	h.cancel = cancel
	go func() { h.served <- h.p.Serve(ctx, l) }()
	t.Cleanup(h.stop)
	return h
}

// stop 取消 ctx 并等 Serve 返回（最多 5 秒）；可重复调。
func (h *harness) stop() {
	h.cancel()
	select {
	case err, ok := <-h.served:
		if ok && err != nil {
			h.t.Errorf("Serve 返回 %v，想要 nil", err)
		}
		if ok {
			close(h.served)
		}
	case <-time.After(5 * time.Second):
		h.t.Errorf("Serve 5 秒内没返回")
	}
}

func (h *harness) dial(ctx context.Context, network, addr string) (net.Conn, error) {
	h.dials.Add(1)
	h.mu.Lock()
	real, ok := h.routes[addr]
	h.mu.Unlock()
	if !ok {
		h.t.Errorf("Dial 了表外地址 %s（测试永不连真主机）", addr)
		return nil, fmt.Errorf("test dial: %s 不在映射表里", addr)
	}
	var d net.Dialer
	return d.DialContext(ctx, network, real)
}

// route 把 hostport 映射到一个 httptest 监听地址。
func (h *harness) route(hostport, real string) {
	h.mu.Lock()
	h.routes[hostport] = real
	h.mu.Unlock()
}

func (h *harness) register(token string, pol Policy) {
	h.t.Helper()
	if err := h.p.Register(token, pol); err != nil {
		h.t.Fatalf("Register(%v): %v", pol, err)
	}
}

// client 返回一个显式走代理的 http.Client（不读环境变量）；token 为空时不带凭证。
func (h *harness) client(token string) *http.Client {
	u := &url.URL{Scheme: "http", Host: h.addr}
	if token != "" {
		u.User = url.User(token) // Basic base64("<token>:")：密码为空时取用户名
	}
	tr := &http.Transport{Proxy: http.ProxyURL(u)}
	h.t.Cleanup(tr.CloseIdleConnections)
	return &http.Client{Transport: tr}
}

// get 经代理发 absolute-URI GET，返回状态、原因头与正文。
func (h *harness) get(token, rawURL string, hdr http.Header) (int, string, string) {
	h.t.Helper()
	req, err := http.NewRequest(http.MethodGet, rawURL, nil)
	if err != nil {
		h.t.Fatal(err)
	}
	for k, vs := range hdr {
		req.Header[k] = vs
	}
	resp, err := h.client(token).Do(req)
	if err != nil {
		h.t.Fatalf("GET %s: %v", rawURL, err)
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	return resp.StatusCode, resp.Header.Get(HeaderReason), string(b)
}

// connect 发一个 CONNECT，返回连接、它的 reader 与响应（响应没有正文时已读完）。
func (h *harness) connect(auth, hostport string) (net.Conn, *bufio.Reader, *http.Response) {
	h.t.Helper()
	req := "CONNECT " + hostport + " HTTP/1.1\r\nHost: " + hostport + "\r\n"
	if auth != "" {
		req += "Proxy-Authorization: " + auth + "\r\n"
	}
	return h.raw(req + "\r\n")
}

// raw 把一段原样的请求字节写给代理，读回响应头（非 200 时连正文一起读完）。
func (h *harness) raw(req string) (net.Conn, *bufio.Reader, *http.Response) {
	h.t.Helper()
	c, err := net.Dial("tcp", h.addr)
	if err != nil {
		h.t.Fatal(err)
	}
	h.t.Cleanup(func() { c.Close() })
	if _, err := io.WriteString(c, req); err != nil {
		h.t.Fatal(err)
	}
	br := bufio.NewReader(c)
	method, _, _ := strings.Cut(req, " ")
	resp, err := http.ReadResponse(br, &http.Request{Method: method})
	if err != nil {
		h.t.Fatalf("%q: %v", req, err)
	}
	if resp.StatusCode != http.StatusOK {
		io.Copy(io.Discard, resp.Body)
		resp.Body.Close()
	}
	return c, br, resp
}

func basic(token string) string {
	return "Basic " + base64.StdEncoding.EncodeToString([]byte("aite:"+token))
}

// event 等下一条审计事件（onAudit 钩子在写完文件之后才调）。
func (h *harness) event() NetworkEvent {
	h.t.Helper()
	select {
	case ev := <-h.events:
		return ev
	case <-time.After(5 * time.Second):
		h.t.Fatalf("5 秒内没有审计事件")
		return NetworkEvent{}
	}
}

// auditFiles 返回审计目录下的 文件名 → 行。
func (h *harness) auditFiles() map[string][]string {
	h.t.Helper()
	out := map[string][]string{}
	ents, err := os.ReadDir(h.dir)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return out
		}
		h.t.Fatal(err)
	}
	for _, e := range ents {
		b, err := os.ReadFile(filepath.Join(h.dir, e.Name()))
		if err != nil {
			h.t.Fatal(err)
		}
		for _, line := range strings.Split(strings.TrimRight(string(b), "\n"), "\n") {
			if line != "" {
				out[e.Name()] = append(out[e.Name()], line)
			}
		}
	}
	return out
}

// auditEvents 把所有审计行按文件名顺序解析出来。
func (h *harness) auditEvents() []NetworkEvent {
	h.t.Helper()
	files := h.auditFiles()
	names := make([]string, 0, len(files))
	for n := range files {
		names = append(names, n)
	}
	sort.Strings(names)
	var evs []NetworkEvent
	for _, n := range names {
		for _, line := range files[n] {
			var ev NetworkEvent
			if err := json.Unmarshal([]byte(line), &ev); err != nil {
				h.t.Fatalf("审计行不是 JSON：%q：%v", line, err)
			}
			evs = append(evs, ev)
		}
	}
	return evs
}
