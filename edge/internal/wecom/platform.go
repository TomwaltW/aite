// Package wecom 是 PlatformPort 的企业微信智能机器人（aibot，API 模式长连接）实现。
//
// 本轨（CC10）只交包、不接线：没有任何现有文件 import 它。平台工厂、配置映射、
// 环境变量读取都在 DD8 的 cmd/aite-edge/main.go 里做（照 feishu.New 的样子）。
//
// 与飞书的四处本质差异（plan §1.3 / §2C）：
//
//   - 一个 bot 只许一条长连接，新连接踢旧连接 → 收到 disconnected_event 进备用，不再重连。
//   - 没有话题、引用不带 msgid → 锚点只能靠可见文本 #A..，填进 Anchor.task_no。
//   - 回复是流式的（aibot_respond_msg），同一个 stream.id 10 分钟内必须收尾 → 9 分钟自动收尾。
//   - 主动发送只能发给来过消息的会话，每会话 30 条/分钟。
//
// adapter 不去重（与飞书同口径）：重连后平台重推的同一条照样往上送，core 用 event_id 判。
package wecom

import (
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"google.golang.org/protobuf/proto"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
	"aite/edge/internal/server"
)

// 编译期断言：本包实现完整的 PlatformPort。
var _ server.PlatformPort = (*Platform)(nil)

const platformName = "wecom"

// DefaultWSURL 是企微 aibot 长连接地址。测试一律显式覆盖，永不拨它。
const DefaultWSURL = "wss://openws.work.weixin.qq.com"

// onEventBudget：HandleEvent 必须在 1s 内返回。超了不拦（拦了会丢事件），但要吼一声。
const onEventBudget = time.Second

// 契约字段的包内常量：T0 的 wecom_v1() 新字段今天 pb 里还没有，名字贴着契约字段，
// DD11 逐个对到 wecom_v1()。
const (
	StreamMaxSec          = 600              // stream_max_sec：平台 10 分钟上限
	CardActionDeadlineMs  = 5000             // card_action_deadline_ms：模板卡片回调 5 秒内更新
	MaxUploadBytes        = 20 * 1024 * 1024 // max_upload_bytes：20 MB（本轨 SendFile 不拦它，见 media.go）
	ReactionsIn           = true             // reactions_in：feedback_event 点赞 / 点踩
	RequiresVisibleAnchor = true             // requires_visible_anchor：引用无 msgid，靠可见 #A..
)

// 包内有界表的容量。
const (
	replyTableCap = 4096
	cardEventCap  = 1024
)

// Config 是 wecom 配置段的本地镜像，字段名照 T0 的 WecomConfig（DD8 机械映射）。
// config 里只放环境变量名，不放凭证；本包不读环境变量。
type Config struct {
	BotIDEnv     string `yaml:"bot_id_env"`
	BotSecretEnv string `yaml:"bot_secret_env"`
	WSURL        string `yaml:"ws_url"`
	// CorpIDEnv / AppSecretEnv 是「另配自建应用把加密 userid 换明文」的占位，本轨不实现换取。
	CorpIDEnv    string `yaml:"corp_id_env"`
	AppSecretEnv string `yaml:"app_secret_env"`
	BotName      string `yaml:"bot_name"`
}

// DefaultConfig 返回 wecom 配置段的默认值。
func DefaultConfig() Config {
	return Config{
		BotIDEnv:     "WECOM_BOT_ID",
		BotSecretEnv: "WECOM_BOT_SECRET",
		WSURL:        DefaultWSURL,
		CorpIDEnv:    "",
		AppSecretEnv: "",
		BotName:      "Aite",
	}
}

// Options 是构造时从环境变量解析好的凭证（DD8 在 main.go 里读）。
type Options struct {
	BotID       string
	BotSecret   string
	TenantID    string // 空 → "default"
	WSURL       string // 空 → 用 Config.WSURL
	WelcomeText string // 空 → enter_chat 不回欢迎语
}

// EventSink 是 edge 把归一化事件送进 core 的出口（internal/ingress 实现）。
type EventSink interface {
	HandleEvent(ctx context.Context, ev *pb.NormalizedEvent) error
}

// clockFunc 返回当前时刻。
type clockFunc func() time.Time

// sleeperFunc 是可注入的 sleep。返回非 nil 表示被 ctx 取消。
type sleeperFunc func(ctx context.Context, d time.Duration) error

// tickerFunc 起一个周期 tick 源；返回 tick 通道与停止函数。测试注入一个自己控制的通道。
type tickerFunc func(d time.Duration) (<-chan time.Time, func())

func realSleep(ctx context.Context, d time.Duration) error {
	if d <= 0 {
		return ctx.Err()
	}
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

func realTicker(d time.Duration) (<-chan time.Time, func()) {
	t := time.NewTicker(d)
	return t.C, t.Stop
}

// 1s → 2s → … → 30s 封顶，无限重试（从 feishu/connection.go 抄一份，非测试代码不 import feishu）。
const (
	reconnectBaseSec = 1
	reconnectMaxSec  = 30
)

// backoffDelay 返回第 attempt 次重连前该等多久（attempt 从 1 起）：1, 2, 4, 8, 16, 30, 30, …
func backoffDelay(attempt int) time.Duration {
	if attempt < 1 {
		return 0
	}
	if attempt >= 6 {
		return reconnectMaxSec * time.Second
	}
	secs := reconnectBaseSec << (attempt - 1)
	if secs > reconnectMaxSec {
		secs = reconnectMaxSec
	}
	return time.Duration(secs) * time.Second
}

// replyCtx 是一条入站回调留下的「可回复上下文」：流式回复要带回调帧的 req_id。
type replyCtx struct {
	reqID  string
	chatID string
}

// cardEventCtx 记模板卡片回调的 req_id 与收到时刻（5 秒内才能更新）。
type cardEventCtx struct {
	chatID     string
	receivedAt time.Time
}

// Platform 实现 server.PlatformPort。
type Platform struct {
	cfg  Config
	opts Options
	sink EventSink
	caps *pb.PlatformCapabilities

	dial       dialFunc
	sleep      sleeperFunc
	clock      clockFunc
	ticker     tickerFunc
	logger     *slog.Logger
	budget     time.Duration
	pingEvery  time.Duration
	ackTimeout time.Duration
	sweepEvery time.Duration
	httpClient *http.Client
	onFeedback func(Feedback)

	connected      atomic.Bool
	standby        atomic.Bool
	reconnectCount atomic.Int64
	cur            atomic.Pointer[wsConn]

	// mu 护下面几张包内有界表。
	mu         sync.Mutex
	replies    *boundedMap[replyCtx]
	cardEvents *boundedMap[cardEventCtx]
	streams    *boundedMap[*streamState]
	media      *boundedMap[mediaEntry]
	chats      *chatBook
}

// platformOptions 是 Platform 的全部可注入面；New 用默认值填满它。
type platformOptions struct {
	cfg        Config
	opts       Options
	sink       EventSink
	dial       dialFunc
	sleep      sleeperFunc
	clock      clockFunc
	ticker     tickerFunc
	logger     *slog.Logger
	budget     time.Duration
	pingEvery  time.Duration
	ackTimeout time.Duration
	sweepEvery time.Duration
	httpClient *http.Client
	onFeedback func(Feedback)
}

func newPlatform(po platformOptions) (*Platform, error) {
	if po.cfg.BotName == "" {
		po.cfg.BotName = DefaultConfig().BotName
	}
	if po.cfg.WSURL == "" {
		po.cfg.WSURL = DefaultWSURL
	}
	if po.opts.TenantID == "" {
		po.opts.TenantID = "default"
	}
	if po.opts.WSURL == "" {
		po.opts.WSURL = po.cfg.WSURL
	}
	if po.dial == nil {
		po.dial = defaultDial
	}
	if po.sleep == nil {
		po.sleep = realSleep
	}
	if po.clock == nil {
		po.clock = time.Now
	}
	if po.ticker == nil {
		po.ticker = realTicker
	}
	if po.logger == nil {
		po.logger = slog.Default()
	}
	if po.budget <= 0 {
		po.budget = onEventBudget
	}
	if po.pingEvery <= 0 {
		po.pingEvery = pingInterval
	}
	if po.ackTimeout <= 0 {
		po.ackTimeout = defaultAckTimeout
	}
	if po.sweepEvery <= 0 {
		po.sweepEvery = streamSweepEvery
	}
	if po.httpClient == nil {
		po.httpClient = &http.Client{Timeout: 30 * time.Second}
	}
	return &Platform{
		cfg:        po.cfg,
		opts:       po.opts,
		sink:       po.sink,
		caps:       WecomCapabilities(),
		dial:       po.dial,
		sleep:      po.sleep,
		clock:      po.clock,
		ticker:     po.ticker,
		logger:     po.logger,
		budget:     po.budget,
		pingEvery:  po.pingEvery,
		ackTimeout: po.ackTimeout,
		sweepEvery: po.sweepEvery,
		httpClient: po.httpClient,
		onFeedback: po.onFeedback,
		replies:    newBoundedMap[replyCtx](replyTableCap),
		cardEvents: newBoundedMap[cardEventCtx](cardEventCap),
		streams:    newBoundedMap[*streamState](streamTableCap),
		media:      newBoundedMap[mediaEntry](mediaTableCap),
		chats:      newChatBook(chatBookCap),
	}, nil
}

// New 只做装配，不建连接；Start 才起长连接。
func New(cfg Config, opts Options, sink EventSink) (*Platform, error) {
	return newPlatform(platformOptions{cfg: cfg, opts: opts, sink: sink})
}

// WecomCapabilities 是企微的能力值；每次调用返回新对象。
//
// 来源三类（docs/p1/wecom.md 有表）：supports_card_edit / proactive_requires_prior_message
// 与 T0 wecom_v1() 一致；platform / supports_thread / supports_history / supports_passive_listen /
// card_edit_window_sec / inbound_file_in_group 是本轨按 plan §1.3 / §2C 取值；
// outbound_rate_per_min 取原卡「每会话 30 条/分钟」（契约缺口，给 T0 对齐）。
func WecomCapabilities() *pb.PlatformCapabilities {
	return &pb.PlatformCapabilities{
		Platform:                      platformName,
		SupportsThread:                false,
		SupportsHistory:               false,
		SupportsPassiveListen:         false,
		SupportsCardEdit:              false,
		CardEditWindowSec:             0,
		InboundFileInGroup:            false,
		ProactiveRequiresPriorMessage: true,
		OutboundRatePerMin:            30,
	}
}

// Capabilities 返回本实例能力的**副本**：gRPC server 在别的 goroutine 上 marshal，
// 调用方改了也改不到本体。
func (p *Platform) Capabilities() *pb.PlatformCapabilities {
	return proto.Clone(p.caps).(*pb.PlatformCapabilities)
}

// ------------------------------------------------------------------
// 长连接
// ------------------------------------------------------------------

// Start 建连 → 订阅 → 读循环；断线按 1,2,4,8,16,30,30… 秒退避无限重连。
// 只有 ctx 取消能让它返回（返回 nil）。收到 disconnected_event 进备用：不再重连，
// 但照样阻塞到 ctx 取消 —— 否则 main.go 会把「Start 提前返回」当组件故障。
func (p *Platform) Start(ctx context.Context) error {
	sweepCtx, stopSweep := context.WithCancel(ctx)
	defer stopSweep()
	go p.sweepLoop(sweepCtx)

	attempt := 0
	for {
		if ctx.Err() != nil {
			return nil
		}
		if p.standby.Load() {
			<-ctx.Done()
			return nil
		}
		if attempt > 0 {
			delay := backoffDelay(attempt)
			p.logger.Warn("wecom.reconnecting", "attempt", attempt, "delay_sec", delay.Seconds())
			if err := p.sleep(ctx, delay); err != nil {
				return nil
			}
			if ctx.Err() != nil {
				return nil
			}
		}

		conn, dispatchDone, err := p.connectOnce(ctx)
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			attempt++
			p.logger.Warn("wecom.connect_failed", "attempt", attempt, "err", err)
			continue
		}

		if attempt > 0 {
			p.logger.Info("wecom.reconnected", "after", attempt)
			p.reconnectCount.Add(1)
		}
		attempt = 0
		p.cur.Store(conn)
		p.connected.Store(true)
		go conn.pingLoop()

		select {
		case <-conn.readDone:
		case <-ctx.Done():
			conn.close()
			<-conn.readDone
		}
		p.connected.Store(false)
		p.cur.CompareAndSwap(conn, nil)
		// 等分发 goroutine 处理完已读到的回调：disconnected_event 就在其中，
		// 下一轮要先看到 standby 再决定重不重连。
		<-dispatchDone
		if conn.readErr != nil && ctx.Err() == nil && !p.standby.Load() {
			p.logger.Warn("wecom.connection_lost", "err", conn.readErr)
		}
		if ctx.Err() != nil {
			return nil
		}
		attempt = 1
	}
}

// connectOnce 拨号、起读循环与分发、发订阅帧并等 errcode == 0。
func (p *Platform) connectOnce(ctx context.Context) (*wsConn, <-chan struct{}, error) {
	ws, err := p.dial(ctx, p.opts.WSURL)
	if err != nil {
		return nil, nil, err
	}
	conn := newWSConn(ws, p.logger, p.pingEvery, p.ackTimeout)
	dispatchDone := make(chan struct{})
	go conn.readLoop()
	go func() {
		defer close(dispatchDone)
		conn.dispatchLoop(ctx, func(ctx context.Context, f *inFrame) { p.handleFrame(ctx, conn, f) })
	}()

	if err := conn.subscribe(ctx, p.opts.BotID, p.opts.BotSecret); err != nil {
		conn.close()
		<-dispatchDone
		return nil, nil, err
	}
	return conn, dispatchDone, nil
}

// Connected 报告长连接当前是否在线（EdgeStatus 用）。备用状态下恒为 false。
func (p *Platform) Connected() bool { return p.connected.Load() }

// ReconnectCount 报告累计重连成功次数（EdgeStatus 用）。
func (p *Platform) ReconnectCount() int64 { return p.reconnectCount.Load() }

// Standby 报告本实例是否因 disconnected_event 进了备用（另一台连上了同一个 bot）。
func (p *Platform) Standby() bool { return p.standby.Load() }

// enterStandby：一个 bot 只许一条连接，新连接会踢旧连接。被踢的这台再重连就会把
// 新主机踢下线，两台互踢 —— 所以这里只置位、断开，不再重连。怎么接管归 DD8。
func (p *Platform) enterStandby(conn *wsConn) {
	p.standby.Store(true)
	p.connected.Store(false)
	p.logger.Warn("wecom.standby", "note", "收到 disconnected_event：同一 bot 的另一条连接已上线，本实例转备用、不再重连")
	conn.close()
}

// currentConn 返回当前可用连接；没有就给一个可重试的 network 错误。
func (p *Platform) currentConn() (*wsConn, error) {
	c := p.cur.Load()
	if c == nil || !p.connected.Load() {
		return nil, &aiteerr.PlatformError{Code: "network", Retryable: true, Msg: "企微长连接未就绪"}
	}
	return c, nil
}

// handleFrame 在分发 goroutine 上处理一帧回调。
func (p *Platform) handleFrame(ctx context.Context, conn *wsConn, f *inFrame) {
	receivedAt := p.clock()
	switch f.Cmd {
	case cmdMsgCallback:
		var body msgCallback
		if err := json.Unmarshal(f.Body, &body); err != nil {
			p.logger.Warn("wecom.bad_callback", "cmd", f.Cmd, "req_id", f.Headers.ReqID, "err", err)
			return
		}
		p.handleMessage(ctx, f, &body, receivedAt)
	case cmdEventCallback:
		var body eventCallback
		if err := json.Unmarshal(f.Body, &body); err != nil {
			p.logger.Warn("wecom.bad_callback", "cmd", f.Cmd, "req_id", f.Headers.ReqID, "err", err)
			return
		}
		p.handleEvent(ctx, conn, f, &body, receivedAt)
	default:
		p.logger.Debug("wecom.frame_ignored", "cmd", f.Cmd, "req_id", f.Headers.ReqID)
	}
}

// handleMessage：记「来过消息」→ 归一化 → 记可回复上下文与媒体 → 送 core。
func (p *Platform) handleMessage(ctx context.Context, f *inFrame, body *msgCallback, receivedAt time.Time) {
	p.mu.Lock()
	p.chats.markSeen(body.ChatID)
	p.mu.Unlock()

	n := normalizeMessage(f, body, p.opts, p.cfg.BotName, receivedAt)
	if n == nil {
		p.logger.Debug("wecom.event_ignored", "msgtype", body.MsgType, "chattype", body.ChatType, "req_id", f.Headers.ReqID)
		return
	}
	messageID := n.event.GetAnchor().GetMessageId()
	p.mu.Lock()
	p.replies.put(messageID, replyCtx{reqID: f.Headers.ReqID, chatID: body.ChatID})
	for _, m := range n.media {
		p.media.put(mediaKey(messageID, m.fileKey), mediaEntry{
			url: m.url, aesKey: m.aesKey, expiresAt: receivedAt.Add(mediaURLTTL),
		})
	}
	p.mu.Unlock()
	p.deliver(ctx, n.event)
}

// handleEvent 处理 aibot_event_callback 的四种事件。
func (p *Platform) handleEvent(ctx context.Context, conn *wsConn, f *inFrame, body *eventCallback, receivedAt time.Time) {
	switch body.Event.EventType {
	case eventEnterChat:
		// 单聊欢迎（5 秒内），不是入群 —— 别映射成 BOT_ADDED（plan:191），不送 core。
		p.sendWelcome(ctx, conn, f.Headers.ReqID, receivedAt)
	case eventTemplateCardEvent:
		ev := normalizeTemplateCard(f, body, p.opts, receivedAt)
		if ev == nil {
			p.logger.Debug("wecom.card_action_ignored", "req_id", f.Headers.ReqID)
			return
		}
		p.mu.Lock()
		p.cardEvents.put(f.Headers.ReqID, cardEventCtx{chatID: body.ChatID, receivedAt: receivedAt})
		p.mu.Unlock()
		p.deliver(ctx, ev)
	case eventFeedbackEvent:
		fb, ok := parseFeedback(body, receivedAt)
		if !ok {
			p.logger.Debug("wecom.feedback_ignored", "req_id", f.Headers.ReqID)
			return
		}
		// 今天契约没有 EventKind::Reaction，不送 core；DD11 接 thumbs_down。
		p.logger.Debug("wecom.feedback", "feedback_id", fb.FeedbackID, "kind", string(fb.Kind))
		if p.onFeedback != nil {
			p.onFeedback(fb)
		}
	case eventDisconnected:
		p.enterStandby(conn)
	default:
		p.logger.Debug("wecom.event_ignored", "eventtype", body.Event.EventType, "req_id", f.Headers.ReqID)
	}
}

// sendWelcome：有 WelcomeText 且距回调 ≤ 5 秒才回；空就只打 debug。
func (p *Platform) sendWelcome(ctx context.Context, conn *wsConn, reqID string, receivedAt time.Time) {
	if p.opts.WelcomeText == "" {
		p.logger.Debug("wecom.enter_chat", "req_id", reqID, "note", "未配欢迎语")
		return
	}
	if p.clock().Sub(receivedAt) > cardActionDeadline {
		p.logger.Warn("wecom.welcome_too_late", "req_id", reqID)
		return
	}
	body := welcomeBody{MsgType: msgTypeText, Text: textBody{Content: p.opts.WelcomeText}}
	if _, err := conn.request(ctx, cmdRespondWelcome, reqID, body); err != nil {
		p.logger.Warn("wecom.welcome_failed", "req_id", reqID, "err", err)
	}
}

// cardActionDeadline：模板卡片回调与欢迎语都要在回调收到后 5 秒内回。
const cardActionDeadline = CardActionDeadlineMs * time.Millisecond

// RespondTemplateCardUpdate 在模板卡片回调的 req_id 上发 aibot_respond_update_msg。
// 距回调收到 > 5 s（或没见过这个 req_id）就返回 deadline_exceeded，一帧不发。
// 卡片内容、Approve / Reject / Submit 与「谁来在 5 秒内调它」是 DD11 的。
func (p *Platform) RespondTemplateCardUpdate(ctx context.Context, reqID string, card map[string]any) error {
	p.mu.Lock()
	ce, ok := p.cardEvents.get(reqID)
	p.mu.Unlock()
	if !ok || p.clock().Sub(ce.receivedAt) > cardActionDeadline {
		return &aiteerr.PlatformError{
			Code: "deadline_exceeded", Retryable: false,
			Msg: "模板卡片回调已超过 5 秒更新窗口（或 req_id 未知）：" + reqID,
		}
	}
	conn, err := p.currentConn()
	if err != nil {
		return err
	}
	_, err = conn.request(ctx, cmdRespondUpdate, reqID, updateCardBody{ResponseType: "update_template_card", TemplateCard: card})
	return err
}

// deliver 把归一化事件同步交给 sink（1 s 预算告警、错误打日志）。
func (p *Platform) deliver(ctx context.Context, ev *pb.NormalizedEvent) {
	if p.sink == nil {
		p.logger.Warn("wecom.no_sink", "note", "没接 EventSink，事件无处可送")
		return
	}
	started := p.clock()
	err := p.sink.HandleEvent(ctx, ev)
	if elapsed := p.clock().Sub(started); elapsed > p.budget {
		p.logger.Warn("wecom.on_event_slow",
			"event_id", ev.GetEventId(), "elapsed_sec", elapsed.Seconds(), "budget_sec", p.budget.Seconds())
	}
	if err != nil {
		// 企微回调没有「返回错误让平台重推」的通道；只能记下来。
		p.logger.Error("wecom.on_event_failed", "event_id", ev.GetEventId(), "err", err)
	}
}

// ------------------------------------------------------------------
// 没有企微对应物的端口方法
// ------------------------------------------------------------------

// ReadHistory：企微 bot 读不到群历史。core 读历史失败记 error 后用空历史。
func (p *Platform) ReadHistory(context.Context, string, int, *string) ([]*pb.HistoryMessage, error) {
	return nil, aiteerr.ErrNotImplemented
}

// ReadDocument：没有对应的文档读取接口。
func (p *Platform) ReadDocument(context.Context, string) (*pb.DocumentContent, error) {
	return nil, aiteerr.ErrNotImplemented
}

// AddReaction：bot 不能给消息加表情。core 的 ack 失败只 warn。
func (p *Platform) AddReaction(context.Context, string, pb.ReactionKind) error {
	return aiteerr.ErrNotImplemented
}

// ------------------------------------------------------------------
// 有界表
// ------------------------------------------------------------------

// boundedMap 是按插入顺序淘汰的有界表（不是 LRU：更新已有键不挪位置）。调用方持锁。
type boundedMap[V any] struct {
	cap   int
	m     map[string]V
	order []string
}

func newBoundedMap[V any](capacity int) *boundedMap[V] {
	return &boundedMap[V]{cap: capacity, m: make(map[string]V)}
}

func (b *boundedMap[V]) get(key string) (V, bool) {
	v, ok := b.m[key]
	return v, ok
}

func (b *boundedMap[V]) put(key string, v V) {
	if _, ok := b.m[key]; !ok {
		b.order = append(b.order, key)
	}
	b.m[key] = v
	for len(b.m) > b.cap && len(b.order) > 0 {
		oldest := b.order[0]
		b.order = b.order[1:]
		delete(b.m, oldest)
	}
}

func (b *boundedMap[V]) each(fn func(key string, v V)) {
	for _, k := range b.order {
		if v, ok := b.m[k]; ok {
			fn(k, v)
		}
	}
}
