// Package dingtalk 是 PlatformPort 的钉钉实现（CC9：独立包，未接线）。
//
// 形状照抄 internal/feishu（只抄形状，不 import）：
//
//   - Stream 长连接收事件（stream.go）→ 归一化成 pb.NormalizedEvent（normalize.go / anchor.go）
//     → 出站（outbound.go 文本 / 文件下载，card.go AI 卡片）→ 卡片回调（callback.go）。
//   - adapter 不做去重 —— 去重是 core 靠 event_id（= 钉钉 msgId）干的。
//   - UpdateCard 一定是更新同一张卡片实例，绝不新发。
//
// 接线（平台工厂、config 放行 dingtalk、EdgeStatus 读 Connected / ReconnectCount）归 DD8；
// 本包只保证 New / Start / Connected / ReconnectCount 与 *feishu.Platform 同形。
package dingtalk

import (
	"context"
	"log/slog"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gorilla/websocket"
	"google.golang.org/protobuf/proto"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// platformName 是 NormalizedEvent.platform / Anchor.platform / Capabilities.platform 的取值。
const platformName = "dingtalk"

// DefaultAPIBase 是钉钉开放平台新版 API 的默认域名（Stream 开连接与 REST 共用）。
const DefaultAPIBase = "https://api.dingtalk.com"

// 1s → 2s → … → 30s 封顶，无限重试（与 feishu 同一条退避契约）。
const (
	ReconnectBaseSec = 1
	ReconnectMaxSec  = 30
)

// Config 是包内对 T0 契约 DingtalkConfig 的镜像（config 里只放环境变量名，不放值）。
//
// edge/internal/config 是 R0，本轨不碰；DD8 接线时把 config.Dingtalk 映射过来。
type Config struct {
	ClientIDEnv       string `yaml:"client_id_env"`
	ClientSecretEnv   string `yaml:"client_secret_env"`
	RobotCodeEnv      string `yaml:"robot_code_env"`
	CardTemplateIDEnv string `yaml:"card_template_id_env"`
	APIBase           string `yaml:"api_base"`
	BotName           string `yaml:"bot_name"`
}

// DefaultConfig 与 T0 的 DingtalkConfig 默认值逐字一致。
func DefaultConfig() Config {
	return Config{
		ClientIDEnv:       "DINGTALK_CLIENT_ID",
		ClientSecretEnv:   "DINGTALK_CLIENT_SECRET",
		RobotCodeEnv:      "DINGTALK_ROBOT_CODE",
		CardTemplateIDEnv: "DINGTALK_CARD_TEMPLATE_ID",
		APIBase:           DefaultAPIBase,
		BotName:           "Aite",
	}
}

// Options 是构造时从环境变量解析好的凭证（照 feishu.Options）。
type Options struct {
	ClientID       string // = appKey
	ClientSecret   string // = appSecret
	RobotCode      string
	CardTemplateID string
	TenantID       string // 空 → "default"
	APIBase        string // 空 → Config.APIBase → DefaultAPIBase
}

// EventSink 是 edge 把归一化事件送进 core 的出口（internal/ingress 实现），与 feishu 同形。
type EventSink interface {
	HandleEvent(ctx context.Context, ev *pb.NormalizedEvent) error
}

type (
	clockFunc   func() time.Time
	sleeperFunc func(ctx context.Context, d time.Duration) error
)

// realSleep 是可被 ctx 打断的 sleep。
func realSleep(ctx context.Context, d time.Duration) error {
	t := time.NewTimer(d)
	defer t.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-t.C:
		return nil
	}
}

// Platform 实现 server.PlatformPort。
type Platform struct {
	cfg  Config
	opts Options
	sink EventSink

	api    *apiClient
	dialer *websocket.Dialer
	sleep  sleeperFunc
	clock  clockFunc
	logger *slog.Logger

	capsMu sync.RWMutex
	caps   *pb.PlatformCapabilities

	sessions *sessionCache
	cards    *cardRoutes

	// inflight 记着卡片回调投递 goroutine；Start 返回前等它们退出。
	inflight sync.WaitGroup

	connected      atomic.Bool
	reconnectCount atomic.Int64
}

// platformOptions 是 Platform 的全部可注入面；New 用默认值填满它。
type platformOptions struct {
	cfg        Config
	opts       Options
	sink       EventSink
	httpClient *http.Client
	dialer     *websocket.Dialer
	sleep      sleeperFunc
	clock      clockFunc
	logger     *slog.Logger
}

func newPlatform(po platformOptions) (*Platform, error) {
	if po.cfg.BotName == "" {
		po.cfg.BotName = DefaultConfig().BotName
	}
	if po.opts.APIBase == "" {
		po.opts.APIBase = po.cfg.APIBase
	}
	if po.opts.APIBase == "" {
		po.opts.APIBase = DefaultAPIBase
	}
	if po.opts.TenantID == "" {
		po.opts.TenantID = "default"
	}
	if po.clock == nil {
		po.clock = time.Now
	}
	if po.sleep == nil {
		po.sleep = realSleep
	}
	if po.logger == nil {
		po.logger = slog.Default()
	}
	if po.dialer == nil {
		po.dialer = &websocket.Dialer{
			Proxy:            http.ProxyFromEnvironment,
			HandshakeTimeout: 10 * time.Second,
		}
	}
	p := &Platform{
		cfg:      po.cfg,
		opts:     po.opts,
		sink:     po.sink,
		dialer:   po.dialer,
		sleep:    po.sleep,
		clock:    po.clock,
		logger:   po.logger,
		caps:     defaultCapabilities(),
		sessions: newSessionCache(),
		cards:    newCardRoutes(),
	}
	p.api = newAPIClient(apiOptions{
		appKey:     po.opts.ClientID,
		appSecret:  po.opts.ClientSecret,
		base:       po.opts.APIBase,
		httpClient: po.httpClient,
		clock:      po.clock,
		logger:     po.logger,
	})
	return p, nil
}

// New 只做装配，不建连接；Start 才起 Stream 长连接。
func New(cfg Config, opts Options, sink EventSink) (*Platform, error) {
	return newPlatform(platformOptions{cfg: cfg, opts: opts, sink: sink})
}

// defaultCapabilities 只用今天 pb 里的 9 个字段；每次返回新对象。
//
// CardEditWindowSec / ProactiveRequiresPriorMessage / OutboundRatePerMin 三个值云端查不到
// 官方文档，是保守占位（docs/p1/dingtalk.md 能力值表标「待核实」）；DD11 按契约 dingtalk_v1() 对齐。
func defaultCapabilities() *pb.PlatformCapabilities {
	return &pb.PlatformCapabilities{
		Platform:              platformName,
		SupportsThread:        false, // 钉钉没有话题：靠可见 #A 锚点 + 引用续接
		SupportsHistory:       false, // 机器人读不了群历史
		SupportsPassiveListen: false, // 群里只收 @ 机器人的消息
		SupportsCardEdit:      true,  // AI 卡片实例可按 outTrackId 更新
		// 能更新多久待 H9 探；core 今天没有这个值的消费者，0 只是被钉住的占位。
		CardEditWindowSec:             0,
		InboundFileInGroup:            false, // 群里 @ 机器人收不到文件，只有单聊能收
		ProactiveRequiresPriorMessage: false, // groupMessages/send 能主动发
		OutboundRatePerMin:            20,
	}
}

// Capabilities 返回本实例能力的**副本**（理由同 feishu：gRPC server 在别的 goroutine 上 marshal）。
func (p *Platform) Capabilities() *pb.PlatformCapabilities {
	p.capsMu.RLock()
	defer p.capsMu.RUnlock()
	return proto.Clone(p.caps).(*pb.PlatformCapabilities)
}

// ------------------------------------------------------------------
// 长连接生命周期
// ------------------------------------------------------------------

// backoffDelay 返回第 attempt 次重连前该等多久（attempt 从 1 起）：1, 2, 4, 8, 16, 30, 30, …
func backoffDelay(attempt int) time.Duration {
	if attempt < 1 {
		return 0
	}
	if attempt >= 6 {
		return ReconnectMaxSec * time.Second
	}
	secs := ReconnectBaseSec << (attempt - 1)
	if secs > ReconnectMaxSec {
		secs = ReconnectMaxSec
	}
	return time.Duration(secs) * time.Second
}

// Start 建连并持续投递事件；断线按 1,2,4,8,16,30,30… 秒退避无限重连。
// 只有 ctx 取消能让它返回。每一轮都重新 open 拿新 ticket（ticket 一次性）。
func (p *Platform) Start(ctx context.Context) error {
	defer p.inflight.Wait()
	attempt := 0

	for {
		if ctx.Err() != nil {
			return nil
		}
		if attempt > 0 {
			delay := backoffDelay(attempt)
			p.logger.Warn("dingtalk.reconnecting", "attempt", attempt, "delay_sec", delay.Seconds())
			if err := p.sleep(ctx, delay); err != nil {
				return nil
			}
			if ctx.Err() != nil {
				return nil
			}
		}

		conn, err := p.connect(ctx)
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			attempt++
			p.logger.Warn("dingtalk.connect_failed", "attempt", attempt, "err", err)
			continue
		}

		if attempt > 0 {
			p.logger.Info("dingtalk.reconnected", "after", attempt)
			p.reconnectCount.Add(1)
		}
		attempt = 0
		p.connected.Store(true)

		err = conn.run(ctx)
		p.connected.Store(false)
		if err != nil && ctx.Err() == nil {
			p.logger.Warn("dingtalk.connection_lost", "err", err)
		}
		conn.Close()

		if ctx.Err() != nil {
			return nil
		}
		// 断开了（服务端断开或下发 disconnect）：下一轮从 1s 起退避，重新 open。
		attempt = 1
	}
}

// Connected 报告长连接当前是否在线（EdgeStatus 用）。
func (p *Platform) Connected() bool { return p.connected.Load() }

// ReconnectCount 报告累计重连成功次数（EdgeStatus 用）。
func (p *Platform) ReconnectCount() int64 { return p.reconnectCount.Load() }

// ------------------------------------------------------------------
// 未实现
// ------------------------------------------------------------------

// ReadHistory：钉钉机器人读不了群历史（Capabilities.SupportsHistory=false）。
func (p *Platform) ReadHistory(context.Context, string, int, *string) ([]*pb.HistoryMessage, error) {
	return nil, aiteerr.ErrNotImplemented
}

// ReadDocument：钉钉文档读取不在 W1 范围。
func (p *Platform) ReadDocument(context.Context, string) (*pb.DocumentContent, error) {
	return nil, aiteerr.ErrNotImplemented
}

// SendFile：原卡没列，记账转给 DD11。
func (p *Platform) SendFile(context.Context, *pb.OutboundFile) (*pb.SendResult, error) {
	return nil, aiteerr.ErrNotImplemented
}

// AddReaction：钉钉机器人没有给消息加表情的接口；core 对 ack 表情失败只打 warn。记账转给 DD11。
func (p *Platform) AddReaction(context.Context, string, pb.ReactionKind) error {
	return aiteerr.ErrNotImplemented
}
