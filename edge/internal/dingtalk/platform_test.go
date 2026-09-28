package dingtalk

import (
	"context"
	"sync"
	"testing"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"aite/edge/internal/aiteerr"
	"aite/edge/internal/server"
)

// 钉：接口断言 + DefaultConfig 的 6 个默认值逐字 + Capabilities 的 9 个字段逐个。
func TestPlatformImplementsPort(t *testing.T) {
	var port server.PlatformPort = mustPlatform(t, platformBuild{})
	_ = port

	cfg := DefaultConfig()
	for _, c := range []struct{ name, got, want string }{
		{"ClientIDEnv", cfg.ClientIDEnv, "DINGTALK_CLIENT_ID"},
		{"ClientSecretEnv", cfg.ClientSecretEnv, "DINGTALK_CLIENT_SECRET"},
		{"RobotCodeEnv", cfg.RobotCodeEnv, "DINGTALK_ROBOT_CODE"},
		{"CardTemplateIDEnv", cfg.CardTemplateIDEnv, "DINGTALK_CARD_TEMPLATE_ID"},
		{"APIBase", cfg.APIBase, "https://api.dingtalk.com"},
		{"BotName", cfg.BotName, "Aite"},
	} {
		if c.got != c.want {
			t.Errorf("DefaultConfig().%s = %q，期望 %q", c.name, c.got, c.want)
		}
	}

	// New 不建连：没有网络也能装配成功，默认 APIBase 落到真实域名常量（只断言、不拨）。
	p, err := New(DefaultConfig(), Options{}, nil)
	if err != nil {
		t.Fatalf("New 失败：%v", err)
	}
	if p.api.base != "https://api.dingtalk.com" {
		t.Errorf("默认 api base = %q", p.api.base)
	}
	if p.opts.TenantID != "default" {
		t.Errorf("默认 TenantID = %q，期望 default", p.opts.TenantID)
	}
	if p.Connected() || p.ReconnectCount() != 0 {
		t.Errorf("新建的 Platform 不该在线：Connected=%v ReconnectCount=%d", p.Connected(), p.ReconnectCount())
	}

	caps := p.Capabilities()
	if caps.GetPlatform() != "dingtalk" {
		t.Errorf("Platform = %q", caps.GetPlatform())
	}
	if caps.GetSupportsThread() {
		t.Error("SupportsThread 应为 false")
	}
	if caps.GetSupportsHistory() {
		t.Error("SupportsHistory 应为 false")
	}
	if caps.GetSupportsPassiveListen() {
		t.Error("SupportsPassiveListen 应为 false")
	}
	if !caps.GetSupportsCardEdit() {
		t.Error("SupportsCardEdit 应为 true")
	}
	if caps.GetCardEditWindowSec() != 0 {
		t.Errorf("CardEditWindowSec = %d，期望 0（待 H9）", caps.GetCardEditWindowSec())
	}
	if caps.GetInboundFileInGroup() {
		t.Error("InboundFileInGroup 应为 false")
	}
	if caps.GetProactiveRequiresPriorMessage() {
		t.Error("ProactiveRequiresPriorMessage 应为 false")
	}
	if caps.GetOutboundRatePerMin() != 20 {
		t.Errorf("OutboundRatePerMin = %d，期望 20", caps.GetOutboundRatePerMin())
	}
}

func TestCapabilitiesReturnsACopy(t *testing.T) {
	p := mustPlatform(t, platformBuild{})
	first := p.Capabilities()
	first.SupportsThread = true
	first.Platform = "tampered"
	first.OutboundRatePerMin = 999

	second := p.Capabilities()
	if second.GetSupportsThread() || second.GetPlatform() != "dingtalk" || second.GetOutboundRatePerMin() != 20 {
		t.Fatalf("改返回值影响了下一次读：%v", second)
	}

	// -race 下并发读、并发改各自的副本。
	var wg sync.WaitGroup
	for i := 0; i < 16; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			c := p.Capabilities()
			c.SupportsHistory = true
		}()
	}
	wg.Wait()
	if p.Capabilities().GetSupportsHistory() {
		t.Fatal("并发改副本改到了本体")
	}
}

func TestUnimplementedMethodsMapToUnimplemented(t *testing.T) {
	p := mustPlatform(t, platformBuild{})
	ctx := context.Background()
	_, errHistory := p.ReadHistory(ctx, testGroupChat, 10, nil)
	_, errDoc := p.ReadDocument(ctx, "https://alidocs.dingtalk.com/i/nodes/x")
	_, errFile := p.SendFile(ctx, nil)
	errReaction := p.AddReaction(ctx, "msg", 0)

	for name, err := range map[string]error{
		"ReadHistory": errHistory, "ReadDocument": errDoc, "SendFile": errFile, "AddReaction": errReaction,
	} {
		if err != aiteerr.ErrNotImplemented {
			t.Errorf("%s 返回 %v，期望 aiteerr.ErrNotImplemented", name, err)
			continue
		}
		if code := status.Code(aiteerr.ToStatus(err)); code != codes.Unimplemented {
			t.Errorf("%s 经 ToStatus 是 %v，期望 Unimplemented", name, code)
		}
	}
}
