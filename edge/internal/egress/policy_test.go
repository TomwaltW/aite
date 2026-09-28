package egress

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// upstream 起一个 httptest 上游，回 "ok <host>"。
func upstream(t *testing.T) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("ok " + r.Host))
	}))
	t.Cleanup(srv.Close)
	return srv
}

func TestAllowSuffixWildcard(t *testing.T) {
	h := newHarness(t)
	srv := upstream(t)
	h.route("a.mirror.test:80", srv.Listener.Addr().String())
	h.route("a.b.mirror.test:80", srv.Listener.Addr().String())
	h.register("tok", Policy{Level: LevelCustom, AllowHosts: []string{"*.mirror.test"}})

	for _, host := range []string{"a.mirror.test", "a.b.mirror.test", "A.Mirror.Test."} {
		status, _, body := h.get("tok", "http://"+host+"/", nil)
		if status != http.StatusOK || !strings.HasPrefix(body, "ok ") {
			t.Errorf("%s：status=%d body=%q，想要 200", host, status, body)
		}
		if ev := h.event(); ev.Decision != decisionAllow {
			t.Errorf("%s：审计 decision=%q", host, ev.Decision)
		}
	}
	// 顶级后缀本身与「后缀前不是 .」的主机都不中；表外地址不在 routes 里，真拨了会当场红。
	for _, host := range []string{"mirror.test", "badmirror.test"} {
		status, reason, _ := h.get("tok", "http://"+host+"/", nil)
		if status != http.StatusForbidden || reason != ReasonHostNotAllowed {
			t.Errorf("%s：status=%d reason=%q，想要 403 %s", host, status, reason, ReasonHostNotAllowed)
		}
		h.event()
	}
}

func TestIMAPIHostsNotInTrustedPreset(t *testing.T) {
	// 逐个过匹配器（不是字符串比较）：预设在 80 / 443 上都不中。
	for _, host := range ReservedHosts() {
		for _, port := range []int{80, 443} {
			if presetAllows(host, port) {
				t.Errorf("china-trusted 预设放行了保留主机 %s:%d", host, port)
			}
		}
	}

	h := newHarness(t)
	h.register("trusted", Policy{Level: LevelTrusted})
	h.register("full", Policy{Level: LevelFull})
	for _, tok := range []string{"trusted", "full"} {
		for _, host := range ReservedHosts() {
			status, reason, _ := h.get(tok, "http://"+host+"/", nil)
			if status != http.StatusForbidden || reason != ReasonIMAPIHost {
				t.Errorf("%s 档 GET %s：status=%d reason=%q，想要 403 %s", tok, host, status, reason, ReasonIMAPIHost)
			}
			h.event()
			c, _, resp := h.connect(basic(tok), host+":443")
			if resp.StatusCode != http.StatusForbidden || resp.Header.Get(HeaderReason) != ReasonIMAPIHost {
				t.Errorf("%s 档 CONNECT %s:443：status=%d reason=%q", tok, host, resp.StatusCode, resp.Header.Get(HeaderReason))
			}
			c.Close()
			h.event()
		}
	}
	if n := h.dials.Load(); n != 0 {
		t.Errorf("保留主机被拨了 %d 次", n)
	}

	// custom 的 AllowHosts 写保留主机或能匹配它的通配 → Register 拒。
	for _, pat := range append(ReservedHosts(), "*.dingtalk.com", "*.feishu.cn", "OPEN.FEISHU.CN.:443") {
		for _, lvl := range []Level{LevelCustom, LevelTrusted, LevelFull} {
			err := h.p.Register("x", Policy{Level: lvl, AllowHosts: []string{pat}})
			if !errors.Is(err, ErrInvalidPolicy) {
				t.Errorf("%s 档 AllowHosts=%q：err=%v，想要 ErrInvalidPolicy", lvl, pat, err)
			}
		}
	}
}

func TestDefaultPorts80And443(t *testing.T) {
	h := newHarness(t)
	srv := upstream(t)
	for _, hp := range []string{"mirror.test:80", "mirror.test:443", "alt.test:8443"} {
		h.route(hp, srv.Listener.Addr().String())
	}
	h.register("tok", Policy{Level: LevelCustom, AllowHosts: []string{"mirror.test", "alt.test:8443"}})

	cases := []struct {
		hostport string
		status   int
		reason   string
	}{
		{"mirror.test:80", http.StatusOK, ""},
		{"mirror.test:443", http.StatusOK, ""},
		{"mirror.test:22", http.StatusForbidden, ReasonPortNotAllowed},
		{"mirror.test:8443", http.StatusForbidden, ReasonPortNotAllowed},
		{"alt.test:8443", http.StatusOK, ""},
		{"alt.test:443", http.StatusForbidden, ReasonPortNotAllowed},
	}
	for _, tc := range cases {
		c, _, resp := h.connect(basic("tok"), tc.hostport)
		c.Close()
		if resp.StatusCode != tc.status || resp.Header.Get(HeaderReason) != tc.reason {
			t.Errorf("CONNECT %s：status=%d reason=%q，想要 %d %q",
				tc.hostport, resp.StatusCode, resp.Header.Get(HeaderReason), tc.status, tc.reason)
		}
		if ev := h.event(); ev.Status != tc.status || ev.Reason != tc.reason {
			t.Errorf("CONNECT %s：审计 status=%d reason=%q", tc.hostport, ev.Status, ev.Reason)
		}
	}
	// absolute-URI 不写端口 = 80。
	if status, _, _ := h.get("tok", "http://mirror.test/", nil); status != http.StatusOK {
		t.Errorf("GET http://mirror.test/：status=%d", status)
	}
	h.event()
}

func TestCredentialBindingsRejectedUntilEE10(t *testing.T) {
	h := newHarness(t)
	srv := upstream(t)
	h.route("mirror.test:80", srv.Listener.Addr().String())
	creds := []CredentialBinding{{Host: "mirror.test", Header: "Authorization", Scheme: "Bearer", SecretRef: "vault:x"}}

	t.Run("新令牌带凭证被拒且不登记", func(t *testing.T) {
		err := h.p.Register("fresh", Policy{Level: LevelCustom, AllowHosts: []string{"mirror.test"}, Credentials: creds})
		if !errors.Is(err, ErrCredentialsUnsupported) {
			t.Fatalf("err=%v，想要 ErrCredentialsUnsupported", err)
		}
		if !strings.Contains(err.Error(), "EE10 之前不支持凭证注入") {
			t.Errorf("错误文本没点名 EE10：%q", err)
		}
		status, reason, _ := h.get("fresh", "http://mirror.test/", nil)
		if status != http.StatusProxyAuthRequired || reason != ReasonUnknownToken {
			t.Errorf("被拒的令牌：status=%d reason=%q，想要 407 %s", status, reason, ReasonUnknownToken)
		}
		h.event()
	})

	t.Run("被拒的重登记不动旧策略", func(t *testing.T) {
		h.register("old", Policy{Level: LevelCustom, AllowHosts: []string{"mirror.test"}})
		err := h.p.Register("old", Policy{Level: LevelNone, Credentials: creds})
		if !errors.Is(err, ErrCredentialsUnsupported) {
			t.Fatalf("err=%v，想要 ErrCredentialsUnsupported", err)
		}
		if err := h.p.Register("old", Policy{Level: LevelNone, AllowHosts: []string{"*"}}); !errors.Is(err, ErrInvalidPolicy) {
			t.Fatalf("AllowHosts=[*]：err=%v，想要 ErrInvalidPolicy", err)
		}
		status, _, body := h.get("old", "http://mirror.test/", nil)
		if status != http.StatusOK || !strings.HasPrefix(body, "ok ") {
			t.Errorf("旧策略应仍放行：status=%d body=%q", status, body)
		}
		h.event()
	})

	t.Run("其它坏策略", func(t *testing.T) {
		bad := []Policy{
			{Level: "open"},
			{Level: ""},
			{Level: LevelCustom, AllowHosts: []string{"*"}},
			{Level: LevelCustom, AllowHosts: []string{"a.*.test"}},
			{Level: LevelCustom, AllowHosts: []string{"*."}},
			{Level: LevelCustom, AllowHosts: []string{"mirror.test:0"}},
			{Level: LevelCustom, AllowHosts: []string{"http://mirror.test"}},
		}
		for _, pol := range bad {
			if err := h.p.Register("bad", pol); !errors.Is(err, ErrInvalidPolicy) {
				t.Errorf("%+v：err=%v，想要 ErrInvalidPolicy", pol, err)
			}
		}
		if err := h.p.Register("", Policy{Level: LevelNone}); !errors.Is(err, ErrInvalidPolicy) {
			t.Errorf("空令牌：err=%v，想要 ErrInvalidPolicy", err)
		}
	})
}
