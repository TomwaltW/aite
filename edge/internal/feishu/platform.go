// Package feishu 是 PlatformPort 的飞书实现（对应旧 aite/adapters/feishu/**）。
//
// owner: R1。R0 只放下骨架：构造函数签名 + 全部方法返回 ErrNotImplemented，
// 让 cmd/aite-edge 能在 R1 落地前就编译、起飞、报 UNIMPLEMENTED。
// R1 交付后本文件的 Capabilities/各方法都要变成真实现；构造函数签名不许改
// （main.go 归 R2，两边并行时靠这个签名对接）。
package feishu

import (
	"context"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
	"aite/edge/internal/config"
)

// EventSink 是 edge 把归一化事件送进 core 的出口（internal/ingress 实现）。
type EventSink interface {
	HandleEvent(ctx context.Context, ev *pb.NormalizedEvent) error
}

// Options 是构造时从环境变量解析好的凭证（config 里只有变量名）。
type Options struct {
	AppID     string
	AppSecret string
	BotOpenID string
	TenantID  string
	Domain    string // 默认 https://open.feishu.cn
}

// Platform 实现 server.PlatformPort。
type Platform struct {
	cfg  config.Feishu
	opts Options
	sink EventSink
}

// New 只做装配，不建连接；Start 才起长连接。
func New(cfg config.Feishu, opts Options, sink EventSink) (*Platform, error) {
	if opts.Domain == "" {
		opts.Domain = "https://open.feishu.cn"
	}
	return &Platform{cfg: cfg, opts: opts, sink: sink}, nil
}

// Start 建立长连接并阻塞到 ctx 取消；断线按 1,2,4,8,16,30,30… 秒退避无限重连。
func (p *Platform) Start(ctx context.Context) error { return aiteerr.ErrNotImplemented }

// Connected 报告长连接当前是否在线（EdgeStatus 用）。
func (p *Platform) Connected() bool { return false }

// ReconnectCount 报告累计重连次数（EdgeStatus 用）。
func (p *Platform) ReconnectCount() int64 { return 0 }

// FeishuP0 是契约里的 FEISHU_P0 常量；每次调用返回新对象，调用方可安全改 supports_passive_listen。
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

func (p *Platform) Capabilities() *pb.PlatformCapabilities { return FeishuP0() }

func (p *Platform) SendText(context.Context, *pb.OutboundText) (*pb.SendResult, error) {
	return nil, aiteerr.ErrNotImplemented
}

func (p *Platform) SendCard(context.Context, string, *string, *pb.ChecklistCard) (*pb.SendResult, error) {
	return nil, aiteerr.ErrNotImplemented
}

func (p *Platform) UpdateCard(context.Context, string, *pb.ChecklistCard) error {
	return aiteerr.ErrNotImplemented
}

func (p *Platform) SendFile(context.Context, *pb.OutboundFile) (*pb.SendResult, error) {
	return nil, aiteerr.ErrNotImplemented
}

func (p *Platform) AddReaction(context.Context, string, pb.ReactionKind) error {
	return aiteerr.ErrNotImplemented
}

func (p *Platform) ReadHistory(context.Context, string, int, *string) ([]*pb.HistoryMessage, error) {
	return nil, aiteerr.ErrNotImplemented
}

func (p *Platform) ReadDocument(context.Context, string) (*pb.DocumentContent, error) {
	return nil, aiteerr.ErrNotImplemented
}

func (p *Platform) DownloadFile(context.Context, string, string) ([]byte, error) {
	return nil, aiteerr.ErrNotImplemented
}
