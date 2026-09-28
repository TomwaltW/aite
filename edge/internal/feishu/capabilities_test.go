// 对应 tests/adapters/feishu/test_feishu_capabilities.py。
//
// 能力副本、配置装配、令牌桶。
//
// supports_passive_listen 保持契约默认值 false，但必须运行时可改，且改的是实例上的
// 副本 —— FeishuP0() 每次返回新对象，谁也污染不了谁。
package feishu

import (
	"testing"

	"google.golang.org/protobuf/proto"

	pb "aite/edge/gen/aitepb"
)

func newTestPlatform(t *testing.T) *Platform {
	t.Helper()
	p, err := New(defaultFeishuConfig(), Options{BotOpenID: "ou_bot"}, nil)
	if err != nil {
		t.Fatalf("New 失败：%v", err)
	}
	return p
}

// ---------------------------------------------------------------------------
// 能力副本
// ---------------------------------------------------------------------------

// TestCapabilitiesDefaultToTheFrozenFeishuP0
// 对应 test_capabilities_default_to_the_frozen_feishu_p0。
func TestCapabilitiesDefaultToTheFrozenFeishuP0(t *testing.T) {
	p := newTestPlatform(t)
	if !proto.Equal(p.Capabilities(), FeishuP0()) {
		t.Errorf("默认能力 = %v，要等于 FeishuP0()", p.Capabilities())
	}
	if p.Capabilities().GetSupportsPassiveListen() {
		t.Error("supports_passive_listen 要保持契约默认值 false")
	}
}

// TestCapabilitiesAreAPerInstanceCopy 对应 test_capabilities_are_a_per_instance_copy。
func TestCapabilitiesAreAPerInstanceCopy(t *testing.T) {
	a, b := newTestPlatform(t), newTestPlatform(t)
	if a.Capabilities() == b.Capabilities() {
		t.Error("两个实例不该共享同一个 capabilities 对象")
	}
	if a.Capabilities() == FeishuP0() {
		t.Error("实例的 capabilities 不该是 FeishuP0() 的返回值本身")
	}
}

// TestSetPassiveListenDoesNotPolluteTheContractSingleton
// 对应 test_set_passive_listen_does_not_pollute_the_contract_singleton。
//
// 权限核实后要改的是 adapter 实例，不是契约常量。
func TestSetPassiveListenDoesNotPolluteTheContractSingleton(t *testing.T) {
	before := FeishuP0()

	p := newTestPlatform(t)
	p.SetPassiveListen(true)

	if !p.Capabilities().GetSupportsPassiveListen() {
		t.Error("实例上的 supports_passive_listen 没改成 true")
	}
	if FeishuP0().GetSupportsPassiveListen() {
		t.Error("FeishuP0() 被就地改了")
	}
	if !proto.Equal(FeishuP0(), before) {
		t.Error("契约常量被就地改了")
	}
	// 新建的实例仍然拿到干净的默认值。
	if newTestPlatform(t).Capabilities().GetSupportsPassiveListen() {
		t.Error("新实例拿到了被污染的默认值")
	}
}

// TestCapabilitiesCanBeInjected 对应 test_capabilities_can_be_injected。
func TestCapabilitiesCanBeInjected(t *testing.T) {
	custom := FeishuP0()
	custom.OutboundRatePerMin = 10

	p, err := newPlatform(platformOptions{
		opts: Options{BotOpenID: "ou_bot"}, caps: custom,
	})
	if err != nil {
		t.Fatalf("newPlatform 失败：%v", err)
	}
	if p.Capabilities().GetOutboundRatePerMin() != 10 {
		t.Errorf("outbound_rate_per_min = %d", p.Capabilities().GetOutboundRatePerMin())
	}
	if p.Capabilities() == custom {
		t.Error("注入的也要拷一份，别把调用方的对象攥在手里")
	}
	// 改实例不影响调用方手上的那份。
	p.SetPassiveListen(true)
	if custom.GetSupportsPassiveListen() {
		t.Error("改实例污染了调用方注入的对象")
	}
}

// TestOutboundRateComesFromTheCapabilities
// 对应 test_outbound_rate_comes_from_the_capabilities。
func TestOutboundRateComesFromTheCapabilities(t *testing.T) {
	p := newTestPlatform(t)
	if got := p.api.bucket.RatePerMin; got != 60 {
		t.Errorf("桶速率 = %d，要 60", got)
	}
	if int32(p.api.bucket.RatePerMin) != FeishuP0().GetOutboundRatePerMin() {
		t.Error("桶速率要取 outbound_rate_per_min，不是另写一个魔数")
	}
}

// TestCapabilitiesEnumValuesMatchTheContract 是 Go 侧补的：逐字段钉住 FEISHU_P0。
func TestCapabilitiesEnumValuesMatchTheContract(t *testing.T) {
	caps := FeishuP0()
	want := &pb.PlatformCapabilities{
		Platform:                      "feishu",
		SupportsThread:                true,
		SupportsHistory:               true,
		SupportsPassiveListen:         false,
		SupportsCardEdit:              true,
		CardEditWindowSec:             1209600, // 14 天
		InboundFileInGroup:            true,
		ProactiveRequiresPriorMessage: false,
		OutboundRatePerMin:            60,
	}
	if !proto.Equal(caps, want) {
		t.Errorf("FeishuP0() = %v，要 %v", caps, want)
	}
}
