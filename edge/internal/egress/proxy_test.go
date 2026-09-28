package egress

import (
	"bufio"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestAllowExact(t *testing.T) {
	h := newHarness(t)
	var mu sync.Mutex
	var got http.Header
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		got = r.Header.Clone()
		mu.Unlock()
		io.WriteString(w, "hello from "+r.URL.Path)
	}))
	defer srv.Close()
	h.route("mirror.test:80", srv.Listener.Addr().String())
	h.register("tok-exact", Policy{Level: LevelCustom, AllowHosts: []string{"mirror.test"}, AuditTag: "task-1"})

	hdr := http.Header{
		"Proxy-Connection": {"keep-alive"},
		"Connection":       {"X-Hop"},
		"X-Hop":            {"1"},
		"Keep-Alive":       {"timeout=5"},
		"X-End":            {"kept"},
	}
	status, _, body := h.get("tok-exact", "http://mirror.test/x", hdr)
	if status != http.StatusOK || body != "hello from /x" {
		t.Fatalf("status=%d body=%q，想要 200 %q", status, body, "hello from /x")
	}
	mu.Lock()
	defer mu.Unlock()
	for _, k := range []string{"Proxy-Authorization", "Proxy-Connection", "X-Hop", "Keep-Alive"} {
		if v := got.Get(k); v != "" {
			t.Errorf("上游收到了逐跳头 %s=%q", k, v)
		}
	}
	if got.Get("X-End") != "kept" {
		t.Errorf("端到端头 X-End 丢了：%v", got)
	}
	ev := h.event()
	if ev.Decision != decisionAllow || ev.Status != http.StatusOK || ev.Host != "mirror.test" || ev.Port != 80 ||
		ev.AuditTag != "task-1" || ev.Level != "custom" || ev.Method != http.MethodGet || ev.BytesDown != int64(len(body)) {
		t.Errorf("审计行不对：%+v", ev)
	}
}

func TestDenyDefault403(t *testing.T) {
	h := newHarness(t)
	h.register("tok", Policy{Level: LevelCustom, AllowHosts: []string{"mirror.test"}})
	h.register("off", Policy{Level: LevelNone, AllowHosts: []string{"mirror.test"}})

	check := func(token, url string, wantStatus int, wantReason, wantHostPort string) {
		t.Helper()
		status, reason, body := h.get(token, url, nil)
		if status != wantStatus || reason != wantReason {
			t.Errorf("%s：status=%d reason=%q，想要 %d %q", url, status, reason, wantStatus, wantReason)
		}
		if !strings.Contains(body, wantReason) || !strings.Contains(body, wantHostPort) {
			t.Errorf("%s：正文 %q 没有原因码 / host:port", url, body)
		}
		ev := h.event()
		if ev.Decision != decisionDeny || ev.Reason != wantReason || ev.Status != wantStatus {
			t.Errorf("%s：审计 %+v", url, ev)
		}
	}
	check("tok", "http://other.test/", http.StatusForbidden, ReasonHostNotAllowed, "other.test:80")
	check("off", "http://mirror.test/", http.StatusForbidden, ReasonLevelNone, "mirror.test:80")

	c, _, resp := h.connect(basic("tok"), "other.test:443")
	c.Close()
	if resp.StatusCode != http.StatusForbidden || resp.Header.Get(HeaderReason) != ReasonHostNotAllowed {
		t.Errorf("CONNECT other.test:443：status=%d reason=%q", resp.StatusCode, resp.Header.Get(HeaderReason))
	}
	h.event()

	t.Run("bad_request", func(t *testing.T) {
		for _, raw := range []string{
			"GET https://mirror.test/ HTTP/1.1\r\nHost: mirror.test\r\n",
			"GET /x HTTP/1.1\r\nHost: mirror.test\r\n",
			"CONNECT mirror.test HTTP/1.1\r\nHost: mirror.test\r\n",
		} {
			c, _, resp := h.raw(raw + "Proxy-Authorization: " + basic("tok") + "\r\n\r\n")
			c.Close()
			if resp.StatusCode != http.StatusBadRequest || resp.Header.Get(HeaderReason) != ReasonBadRequest {
				t.Errorf("%q：status=%d reason=%q，想要 400 %s", raw, resp.StatusCode, resp.Header.Get(HeaderReason), ReasonBadRequest)
			}
			if ev := h.event(); ev.Reason != ReasonBadRequest {
				t.Errorf("%q：审计 %+v", raw, ev)
			}
		}
	})

	if n := h.dials.Load(); n != 0 {
		t.Errorf("拒绝的请求拨了上游 %d 次，想要 0", n)
	}
}

func TestMissingOrUnknownTokenDenied(t *testing.T) {
	h := newHarness(t)
	h.register("tok-live", Policy{Level: LevelFull})

	check := func(name, token string, wantReason string) {
		t.Helper()
		req, _ := http.NewRequest(http.MethodGet, "http://mirror.test/", nil)
		resp, err := h.client(token).Do(req)
		if err != nil {
			t.Fatalf("%s：%v", name, err)
		}
		io.Copy(io.Discard, resp.Body)
		resp.Body.Close()
		if resp.StatusCode != http.StatusProxyAuthRequired || resp.Header.Get(HeaderReason) != wantReason {
			t.Errorf("%s：status=%d reason=%q，想要 407 %s", name, resp.StatusCode, resp.Header.Get(HeaderReason), wantReason)
		}
		if got := resp.Header.Get("Proxy-Authenticate"); got != `Basic realm="aite-egress"` {
			t.Errorf("%s：Proxy-Authenticate=%q", name, got)
		}
		ev := h.event()
		if ev.Decision != decisionDeny || ev.Reason != wantReason || ev.Status != 407 || ev.AuditTag != "" || ev.Level != "" {
			t.Errorf("%s：审计 %+v", name, ev)
		}
	}
	check("无头", "", ReasonMissingToken)
	check("错令牌", "tok-wrong", ReasonUnknownToken)
	h.p.Unregister("tok-live")
	check("Unregister 之后", "tok-live", ReasonUnknownToken)

	// CONNECT 的 Bearer 形状与解不出的头。
	c, _, resp := h.connect("Bearer tok-wrong", "mirror.test:443")
	c.Close()
	if resp.StatusCode != http.StatusProxyAuthRequired || resp.Header.Get(HeaderReason) != ReasonUnknownToken {
		t.Errorf("CONNECT Bearer 错令牌：status=%d reason=%q", resp.StatusCode, resp.Header.Get(HeaderReason))
	}
	h.event()
	c, _, resp = h.connect("Basic !!!", "mirror.test:443")
	c.Close()
	if resp.StatusCode != http.StatusProxyAuthRequired || resp.Header.Get(HeaderReason) != ReasonMissingToken {
		t.Errorf("CONNECT 坏 Basic：status=%d reason=%q", resp.StatusCode, resp.Header.Get(HeaderReason))
	}
	h.event()

	if n := h.dials.Load(); n != 0 {
		t.Errorf("令牌不对的请求拨了上游 %d 次，想要 0", n)
	}
	if n := len(h.auditEvents()); n != 5 {
		t.Errorf("审计行 %d 条，想要 5", n)
	}
}

func TestConnectTunnelToTLSServerMovesBytes(t *testing.T) {
	h := newHarness(t)
	srv := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, "secret-body over tls")
	}))
	defer srv.Close()
	h.route("mirror.test:443", srv.Listener.Addr().String())
	h.register("tok", Policy{Level: LevelTrusted, AllowHosts: []string{"mirror.test"}, AuditTag: "t-tls"})

	c, br, resp := h.connect(basic("tok"), "mirror.test:443")
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("CONNECT status=%d", resp.StatusCode)
	}
	if br.Buffered() != 0 {
		t.Fatalf("200 之后还有 %d 字节缓冲", br.Buffered())
	}
	c.SetDeadline(time.Now().Add(10 * time.Second)) // 隧道坏了（比如只通一个方向）就红，不挂到测试超时
	pool := x509.NewCertPool()
	pool.AddCert(srv.Certificate())
	// ServerName 用 httptest 证书签的名，只用于校验，不产生任何对外连接。
	tc := tls.Client(c, &tls.Config{RootCAs: pool, ServerName: "example.com"})
	if _, err := io.WriteString(tc, "GET / HTTP/1.1\r\nHost: mirror.test\r\nConnection: close\r\n\r\n"); err != nil {
		t.Fatal(err)
	}
	r, err := http.ReadResponse(bufio.NewReader(tc), nil)
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(r.Body)
	r.Body.Close()
	if string(body) != "secret-body over tls" {
		t.Fatalf("隧道里读到 %q", body)
	}
	tc.Close()

	ev := h.event()
	if ev.Status != http.StatusOK || ev.Decision != decisionAllow || ev.BytesUp <= 0 || ev.BytesDown <= 0 ||
		ev.Method != http.MethodConnect || ev.Port != 443 || ev.AuditTag != "t-tls" {
		t.Errorf("审计行不对：%+v", ev)
	}
}

func TestServeReturnsOnContextCancel(t *testing.T) {
	h := newHarness(t)
	srv := upstream(t)
	h.route("mirror.test:80", srv.Listener.Addr().String())
	h.register("tok", Policy{Level: LevelCustom, AllowHosts: []string{"mirror.test"}})

	c, br, resp := h.connect(basic("tok"), "mirror.test:80")
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("CONNECT status=%d", resp.StatusCode)
	}
	// 隧道上走一个 keep-alive 往返：之后两个拷贝 goroutine 都停在读上。
	io.WriteString(c, "GET / HTTP/1.1\r\nHost: mirror.test\r\n\r\n")
	r, err := http.ReadResponse(br, nil)
	if err != nil {
		t.Fatal(err)
	}
	io.Copy(io.Discard, r.Body)
	r.Body.Close()
	if n := h.p.copiers.Load(); n != 2 {
		t.Fatalf("取消前活着的拷贝 goroutine = %d，想要 2", n)
	}

	h.cancel()
	select {
	case err := <-h.served:
		if err != nil {
			t.Errorf("Serve 返回 %v，想要 nil", err)
		}
		close(h.served)
	case <-time.After(2 * time.Second):
		t.Fatal("ctx 取消后 Serve 2 秒内没返回（在途隧道没关？）")
	}
	if n := h.p.copiers.Load(); n != 0 {
		t.Errorf("Serve 返回后还有 %d 个拷贝 goroutine", n)
	}
	c.SetReadDeadline(time.Now().Add(2 * time.Second))
	if _, err := br.ReadByte(); !errors.Is(err, io.EOF) {
		t.Errorf("隧道客户端那头读到 %v，想要 EOF", err)
	}
	if ev := h.event(); ev.Status != http.StatusOK || ev.BytesUp <= 0 || ev.BytesDown <= 0 {
		t.Errorf("隧道审计行：%+v", ev)
	}
}
