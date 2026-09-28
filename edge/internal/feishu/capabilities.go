package feishu

import (
	"google.golang.org/protobuf/proto"

	pb "aite/edge/gen/aitepb"
)

// FeishuP0 是契约里的 FEISHU_P0 常量；每次调用返回新对象，
// 调用方可安全改 supports_passive_listen。
func FeishuP0() *pb.PlatformCapabilities {
	return &pb.PlatformCapabilities{
		Platform:                      "feishu",
		SupportsThread:                true,
		SupportsHistory:               true,
		SupportsPassiveListen:         false,
		SupportsCardEdit:              true,
		CardEditWindowSec:             1209600,
		InboundFileInGroup:            true,
		ProactiveRequiresPriorMessage: false,
		OutboundRatePerMin:            60,
	}
}

// Capabilities 返回本实例能力的**副本**。
//
// 必须是副本而不是本体：R2 的 gRPC server 会在别的 goroutine 上读它去 marshal，
// 而 SetPassiveListen 会写同一个对象（§3.7 权限核实后运行时改）——
// 返回本体既是 data race，调用方改一下还会改到 Platform 的状态。
func (p *Platform) Capabilities() *pb.PlatformCapabilities {
	p.capsMu.RLock()
	defer p.capsMu.RUnlock()
	return proto.Clone(p.caps).(*pb.PlatformCapabilities)
}

// SetPassiveListen 在权限核实后调这个，改的是本实例而不是 FeishuP0() 的返回值。
func (p *Platform) SetPassiveListen(value bool) {
	p.capsMu.Lock()
	defer p.capsMu.Unlock()
	p.caps.SupportsPassiveListen = value
}
