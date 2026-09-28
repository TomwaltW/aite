// Package feishu 是 PlatformPort 的飞书实现（对应旧 aite/adapters/feishu/**）。
//
// 本文件对应 aite/adapters/feishu/platform.py。
//
// 长连接收事件 → 归一化成 pb.NormalizedEvent → 出站（文本 / 卡片 / 文件 / 表情）
// → 群历史与云文档读取。
//
// 三条不许走偏的：
//
//   - UpdateCard 一定是 PATCH 同一条 message_id，绝不新发消息。
//   - adapter 不做去重 —— 去重是 core 靠 event_id 干的。
//     重连后平台重推的重复事件在这里照单全收往上送。
//   - ReadHistory 按时间正序返回，不做 sender_kind 过滤 —— 过滤归 core 的
//     read_group_history 工具。
//
// 构造函数签名不许改（cmd/aite-edge 归 R2，两边并行时靠这个签名对接）。
package feishu

import (
	"context"
	"log/slog"
	"os"
	"sync"
	"sync/atomic"
	"time"

	"google.golang.org/protobuf/proto"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/config"
)

// onEventBudget：HandleEvent 必须在 1s 内返回。超了不拦（拦了会丢事件），但要吼一声。
const onEventBudget = time.Second

// 1s → 2s → … → 30s 封顶，无限重试。
const (
	ReconnectBaseSec = 1
	ReconnectMaxSec  = 30
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

	api *apiClient
	// capsMu 护 caps：gRPC server 并发读、SetPassiveListen 写。
	capsMu  sync.RWMutex
	caps    *pb.PlatformCapabilities
	factory ConnectionFactory
	sleep   sleeperFunc
	clock   clockFunc
	logger  *slog.Logger
	budget  time.Duration

	// cardButtons=true 时 SendCard / UpdateCard 的卡片带按钮（AITE_FEISHU_CARD_BUTTONS=1，默认关）。
	cardButtons bool
	// senderNames 非 nil 时 dispatchRaw 给事件补发言人姓名（通讯录查询，见 reads.go）。
	senderNames *senderNameLookup

	// historyWindow 是 config 里的 feishu.history_window，只存不用：
	// ReadHistory 的条数由调用方按 ports.go 的签名给（Python 版同样只存着）。
	historyWindow int

	connected      atomic.Bool
	reconnectCount atomic.Int64
}

// platformOptions 是 Platform 的全部可注入面；New 用默认值填满它。
type platformOptions struct {
	cfg     config.Feishu
	opts    Options
	sink    EventSink
	api     *apiClient
	caps    *pb.PlatformCapabilities
	factory ConnectionFactory
	sleep   sleeperFunc
	clock   clockFunc
	logger  *slog.Logger
	budget  time.Duration

	// cardButtons 见 Platform.cardButtons。New() 从环境变量填，newPlatform 只认字段、不读环境。
	cardButtons bool
	// senderNames=true 时装配发言人姓名查询（走 api 的通讯录接口）。只有 New() 设 true：
	// 测试的 dispatchPlatform 不给 api，默认 api 指向真的 open.feishu.cn，不能默认开。
	senderNames bool
}

func newPlatform(po platformOptions) (*Platform, error) {
	if po.opts.Domain == "" {
		po.opts.Domain = DefaultDomain
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
	if po.budget <= 0 {
		po.budget = onEventBudget
	}
	// FeishuP0() 每次返回新对象；注入的也拷一份，别把调用方的对象攥在手里。
	caps := po.caps
	if caps == nil {
		caps = FeishuP0()
	} else {
		caps = proto.Clone(caps).(*pb.PlatformCapabilities)
	}

	historyWindow := po.cfg.HistoryWindow
	if historyWindow <= 0 {
		historyWindow = 50
	}

	p := &Platform{
		cfg:           po.cfg,
		opts:          po.opts,
		sink:          po.sink,
		caps:          caps,
		sleep:         po.sleep,
		clock:         po.clock,
		logger:        po.logger,
		budget:        po.budget,
		cardButtons:   po.cardButtons,
		historyWindow: historyWindow,
	}

	p.api = po.api
	if p.api == nil {
		api, err := newAPIClient(apiOptions{
			appID:      po.opts.AppID,
			appSecret:  po.opts.AppSecret,
			domain:     po.opts.Domain,
			ratePerMin: int(caps.GetOutboundRatePerMin()),
			sleep:      po.sleep,
			clock:      po.clock,
			logger:     po.logger,
		})
		if err != nil {
			return nil, err
		}
		p.api = api
	}
	if po.senderNames {
		p.senderNames = newSenderNameLookup(p.api, p.clock, p.logger)
	}

	p.factory = po.factory
	if p.factory == nil {
		p.factory = func(onRaw RawEventHandler) Connection {
			conn := newLarkConnection(po.opts.AppID, po.opts.AppSecret, po.opts.Domain, onRaw, po.logger)
			conn.cardButtons = po.cardButtons
			return conn
		}
	}
	return p, nil
}

// 本包自己读的环境变量（只在 New() 里读；DD8 做 Go 配置镜像后换成 FeishuConfig 的字段）。
const (
	// EnvCardButtons 恰好等于 "1" 才在卡片上渲染按钮，别的值一律关。
	EnvCardButtons = "AITE_FEISHU_CARD_BUTTONS"
	// EnvPassiveListen 恰好等于 "1" 才 SetPassiveListen(true)（权限核实过的部署才开）。
	EnvPassiveListen = "AITE_FEISHU_PASSIVE_LISTEN"
	// EnvAPIBase 非空且 Options.Domain 为空时当域名用（REST 与长连接同一个域名）；
	// 显式的 Options.Domain 优先。
	EnvAPIBase = "AITE_FEISHU_API_BASE"
)

// New 只做装配，不建连接；Start 才起长连接。
//
// 环境变量只在这里读：newPlatform（测试都走它）只认 platformOptions 的字段。
func New(cfg config.Feishu, opts Options, sink EventSink) (*Platform, error) {
	cardButtons := os.Getenv(EnvCardButtons) == "1"
	passiveListen := os.Getenv(EnvPassiveListen) == "1"
	apiBase := os.Getenv(EnvAPIBase)
	if opts.Domain == "" && apiBase != "" {
		opts.Domain = apiBase
	}

	p, err := newPlatform(platformOptions{
		cfg: cfg, opts: opts, sink: sink,
		cardButtons: cardButtons,
		senderNames: true,
	})
	if err != nil {
		return nil, err
	}
	if passiveListen {
		p.SetPassiveListen(true)
	}
	p.logger.Info("feishu.env_flags",
		"card_buttons", cardButtons, "passive_listen", passiveListen, "api_base", apiBase)
	return p, nil
}

// ------------------------------------------------------------------
// 长连接
// ------------------------------------------------------------------

// Start 建连并持续投递事件；断线按 1,2,4,8,16,30,30… 秒退避无限重连。
// 只有 ctx 取消能让它返回。
func (p *Platform) Start(ctx context.Context) error {
	attempt := 0

	for {
		if ctx.Err() != nil {
			return nil
		}
		if attempt > 0 {
			delay := backoffDelay(attempt)
			p.logger.Warn("feishu.reconnecting", "attempt", attempt, "delay_sec", delay.Seconds())
			if err := p.sleep(ctx, delay); err != nil {
				return nil
			}
			if ctx.Err() != nil {
				return nil
			}
		}

		conn := p.factory(p.dispatchRaw)
		if err := conn.Connect(ctx); err != nil {
			if ctx.Err() != nil {
				return nil
			}
			attempt++
			p.logger.Warn("feishu.connect_failed", "attempt", attempt, "err", err)
			continue
		}

		if attempt > 0 {
			p.logger.Info("feishu.reconnected", "after", attempt)
			p.reconnectCount.Add(1)
		}
		attempt = 0
		p.connected.Store(true)

		err := conn.WaitClosed(ctx)
		p.connected.Store(false)
		if err != nil && ctx.Err() == nil {
			p.logger.Warn("feishu.connection_lost", "err", err)
		}
		p.safeClose(conn)

		if ctx.Err() != nil {
			return nil
		}
		// 断开了：下一轮从 1s 起退避重连。
		attempt = 1
	}
}

func (p *Platform) safeClose(c Connection) {
	if c == nil {
		return
	}
	if err := c.Close(); err != nil {
		p.logger.Warn("feishu.close_failed", "err", err)
	}
}

// Connected 报告长连接当前是否在线（EdgeStatus 用）。
func (p *Platform) Connected() bool { return p.connected.Load() }

// ReconnectCount 报告累计重连成功次数（EdgeStatus 用）。
func (p *Platform) ReconnectCount() int64 { return p.reconnectCount.Load() }

// backoffDelay 返回第 attempt 次重连前该等多久（attempt 从 1 起）。
//
// 1, 2, 4, 8, 16, 30, 30, …（32 会被 30s 的上限压回去）。
func backoffDelay(attempt int) time.Duration {
	if attempt < 1 {
		return 0
	}
	// attempt >= 6 时 2^(attempt-1) >= 32 已经越过上限；提前返回顺便躲开移位溢出。
	if attempt >= 6 {
		return ReconnectMaxSec * time.Second
	}
	secs := ReconnectBaseSec << (attempt - 1)
	if secs > ReconnectMaxSec {
		secs = ReconnectMaxSec
	}
	return time.Duration(secs) * time.Second
}
