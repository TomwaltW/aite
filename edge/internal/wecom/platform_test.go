package wecom

import (
	"context"
	"errors"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
	"aite/edge/internal/feishu"
)

// lifecycle 是 DD8 的平台工厂要的生命周期形状：飞书与企微都得赋得进去。
type lifecycle interface {
	Start(context.Context) error
	Connected() bool
	ReconnectCount() int64
}

func TestPlatformShapeMatchesFeishu(t *testing.T) {
	var _ lifecycle = (*feishu.Platform)(nil)
	var _ lifecycle = (*Platform)(nil)

	cfg := DefaultConfig()
	want := Config{
		BotIDEnv: "WECOM_BOT_ID", BotSecretEnv: "WECOM_BOT_SECRET", WSURL: "wss://openws.work.weixin.qq.com",
		CorpIDEnv: "", AppSecretEnv: "", BotName: "Aite",
	}
	if cfg != want {
		t.Fatalf("DefaultConfig() = %+v，要 %+v", cfg, want)
	}

	p, err := New(cfg, Options{BotID: "bot-test", BotSecret: "secret-test", WSURL: "ws://127.0.0.1:1"}, nil)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	if p.opts.TenantID != "default" {
		t.Fatalf("TenantID 空时要落成 default，得到 %q", p.opts.TenantID)
	}
	caps := p.Capabilities()
	if caps.GetPlatform() != "wecom" ||
		caps.GetSupportsThread() || caps.GetSupportsHistory() || caps.GetSupportsPassiveListen() ||
		caps.GetSupportsCardEdit() || caps.GetCardEditWindowSec() != 0 || caps.GetInboundFileInGroup() ||
		!caps.GetProactiveRequiresPriorMessage() || caps.GetOutboundRatePerMin() != 30 {
		t.Fatalf("能力值不对：%v", caps)
	}

	// 改返回值改不到本体。
	caps.Platform = "mutated"
	caps.SupportsThread = true
	caps.OutboundRatePerMin = 999
	again := p.Capabilities()
	if again.GetPlatform() != "wecom" || again.GetSupportsThread() || again.GetOutboundRatePerMin() != 30 {
		t.Fatalf("Capabilities() 返回的不是副本：%v", again)
	}

	if StreamMaxSec != 600 || CardActionDeadlineMs != 5000 || MaxUploadBytes != 20*1024*1024 || !ReactionsIn || !RequiresVisibleAnchor {
		t.Fatalf("契约常量不对")
	}
}

func TestUnsupportedPortMethodsAreUnimplemented(t *testing.T) {
	p, err := New(DefaultConfig(), Options{WSURL: "ws://127.0.0.1:1"}, nil)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	ctx := context.Background()
	if _, err := p.ReadHistory(ctx, "chat", 10, nil); !errors.Is(err, aiteerr.ErrNotImplemented) {
		t.Fatalf("ReadHistory: %v", err)
	}
	if _, err := p.ReadDocument(ctx, "doc"); !errors.Is(err, aiteerr.ErrNotImplemented) {
		t.Fatalf("ReadDocument: %v", err)
	}
	if err := p.AddReaction(ctx, "m", pb.ReactionKind_REACTION_KIND_ACK); !errors.Is(err, aiteerr.ErrNotImplemented) {
		t.Fatalf("AddReaction: %v", err)
	}
}

func TestDisconnectedEventEntersStandby(t *testing.T) {
	fs := newFakeServer(t, nil)
	rs := &recordingSleep{}
	h := startHarness(t, fs, func(po *platformOptions) { po.sleep = rs.sleep })
	h.waitConnected(t)

	sc := fs.conn(0)
	if err := sc.push(cmdEventCallback, "rq-dis", map[string]any{
		"aibotid": "bot-test", "event": map[string]any{"eventtype": "disconnected_event"},
	}); err != nil {
		t.Fatalf("push: %v", err)
	}
	sc.close()

	waitFor(t, "进备用", h.p.Standby)
	if h.p.Connected() {
		t.Fatalf("备用状态下 Connected() 必须为 false")
	}
	// 注入的 sleep 立即返回：要是走了普通断线分支，退避「瞬间」就过完，远超 30s 封顶，
	// 新一轮拨号马上就会到。给它充足的真实时间去犯错。
	time.Sleep(200 * time.Millisecond)
	if n := fs.dials(); n != 1 {
		t.Fatalf("备用后又拨了号：共 %d 次（主备会互踢）", n)
	}
	if calls := rs.recorded(); len(calls) != 0 {
		t.Fatalf("备用后不该进退避：%v", calls)
	}
	select {
	case err := <-h.done:
		t.Fatalf("备用时 Start 提前返回了：%v", err)
	default:
	}

	h.stop()
	select {
	case err := <-h.done:
		if err != nil {
			t.Fatalf("ctx 取消后 Start 要返回 nil，得到 %v", err)
		}
		h.done <- nil // 让 Cleanup 里的等待也拿得到
	case <-time.After(5 * time.Second):
		t.Fatalf("ctx 取消后 Start 没返回")
	}
}
