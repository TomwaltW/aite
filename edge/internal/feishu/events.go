// 对应 aite/adapters/feishu/connection.py。
//
// 这里只干两件事：
//
//  1. 定义 adapter 眼里的连接长什么样（Connection）—— 三个方法，够 Platform 跑重连
//     循环即可。测试拿假连接对象顶上去，不用起真 websocket。
//  2. 把 lark oapi-sdk-go 的 ws.Client 包成这个形状。
//
// 重连退避不在这里做，在 Platform 的循环里（backoffDelay 是它用的）：
// 退避策略是 adapter 的行为契约（判据要断言 [1,2,4,8,16,30,30]），
// 不该被真连接实现绑架。SDK 自己的 autoReconnect 一律关掉 —— 它用的是服务端下发的
// 固定间隔，跟指数退避不是一回事。
package feishu

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"sync"
	"time"

	larkcore "github.com/larksuite/oapi-sdk-go/v3/core"
	larkevent "github.com/larksuite/oapi-sdk-go/v3/event"
	"github.com/larksuite/oapi-sdk-go/v3/event/dispatcher"
	"github.com/larksuite/oapi-sdk-go/v3/event/dispatcher/callback"
	larkws "github.com/larksuite/oapi-sdk-go/v3/ws"

	pb "aite/edge/gen/aitepb"
)

const (
	eventMessageReceive = "im.message.receive_v1"
	eventCardAction     = "card.action.trigger"
)

// ---------------------------------------------------------------------------
// 事件分发表
// ---------------------------------------------------------------------------

// normalizeFunc 把一份事件信封归一化；返回 nil = 认识这类事件但这一条不上送。
type normalizeFunc func(raw map[string]any, botOpenID, workspaceID, tenantID string) *pb.NormalizedEvent

// eventEntry 是分发表里的一项：一个事件类型怎么归一化、在 SDK 的哪张表上注册。
//
// 表里有的类型 buildDispatcher 都会注册 —— 控制台订阅了、代码里没注册的事件，
// SDK 的 dispatcher.Do 返回 NotFoundEventHandlerErr，长连接回 500，平台就会一直重推。
type eventEntry struct {
	normalize normalizeFunc
	// callback=true 走 OnP2CardActionTrigger：卡片回传只能注册在 SDK 的 callback 表上
	// （callback 表优先于 event 表，OnCustomizedEvent 注册不到它）。目前只有 card.action.trigger。
	callback bool
	// onDropped 在 normalize 返回 nil 时由 dispatchRaw 调用，打这一类事件自己的日志；
	// nil = 照旧打 feishu.event_ignored。
	onDropped func(logger *slog.Logger, raw map[string]any)
}

// eventTable 是包级可变状态：各文件在 init() 里登记，测试运行期也会临时登记 / 撤销，
// 而 SDK 的 websocket goroutine 同时在经 buildDispatcher / Normalize 读它 —— 所以要锁。
var (
	eventTableMu sync.RWMutex
	eventTable   = map[string]eventEntry{}
)

// registerEvent 往分发表里登记一个事件类型。重复登记 panic：两个文件抢同一个类型
// 一定是写错了，启动时炸出来比静默覆盖好。
func registerEvent(eventType string, entry eventEntry) {
	if entry.normalize == nil {
		panic(fmt.Sprintf("feishu: 事件 %s 登记时没给 normalize", eventType))
	}
	if entry.callback && eventType != eventCardAction {
		panic(fmt.Sprintf("feishu: SDK 只有 %s 一条 callback 注册面，%s 登记不上", eventCardAction, eventType))
	}
	eventTableMu.Lock()
	defer eventTableMu.Unlock()
	if _, dup := eventTable[eventType]; dup {
		panic(fmt.Sprintf("feishu: 事件 %s 重复登记", eventType))
	}
	eventTable[eventType] = entry
}

// lookupEvent 查一个事件类型的登记项。
func lookupEvent(eventType string) (eventEntry, bool) {
	eventTableMu.RLock()
	defer eventTableMu.RUnlock()
	entry, ok := eventTable[eventType]
	return entry, ok
}

// eventTableSnapshot 返回分发表的快照，读侧拿着快照用，不在锁里调 SDK。
func eventTableSnapshot() map[string]eventEntry {
	eventTableMu.RLock()
	defer eventTableMu.RUnlock()
	out := make(map[string]eventEntry, len(eventTable))
	for k, v := range eventTable {
		out[k] = v
	}
	return out
}

// RawEventHandler 是原始事件（信封 map）的消费者。
type RawEventHandler func(ctx context.Context, raw map[string]any) error

// Connection 是 adapter 需要的连接能力。
type Connection interface {
	// Connect 建连。失败返回 error —— 由 Platform 决定退避多久再来。
	Connect(ctx context.Context) error
	// WaitClosed 一直等到连接断掉才返回；异常断开返回非 nil。
	WaitClosed(ctx context.Context) error
	// Close 主动断开，幂等。
	Close() error
}

// ConnectionFactory 每轮新建一个连接对象（真实现也是这样：连接对象不复用）。
type ConnectionFactory func(onRaw RawEventHandler) Connection

// envelopeHeaderKeys 是信封里保留的 header 字段。
//
// 故意不含 token：它是应用的校验令牌，而信封会原样进 NormalizedEvent.raw
// 落到审计里，不该跟着躺进证据文件。
var envelopeHeaderKeys = []string{"event_id", "create_time", "event_type", "tenant_key", "app_id"}

// envelopeOf 把 SDK 收到的原始 payload 整理成事件信封。
func envelopeOf(payload map[string]any) map[string]any {
	header := asMap(payload["header"])
	out := make(map[string]any, len(envelopeHeaderKeys))
	for _, key := range envelopeHeaderKeys {
		out[key] = header[key]
	}
	schema := mapStr(payload, "schema")
	if schema == "" {
		schema = "2.0"
	}
	return map[string]any{
		"schema": schema,
		"header": out,
		"event":  asMap(payload["event"]),
	}
}

// errConnectClosedBeforeReady 是 Start 在连上之前就退出了。
var errConnectClosedBeforeReady = errors.New("feishu: 长连接在就绪前就退出了")

// larkConnection 把 lark oapi-sdk-go 的 ws.Client 包成 Connection。
//
// 与 Python 版的三处 SDK 私有面（对齐 event loop、轮询 _conn 判断断线、
// route_card_frames_as_events）相比，Go SDK 有公开的 Start/Close 与 OnReady 回调，
// 前两处自然消失；第三处见 cards.go 里 `cardElements` 那段注释与 card_frames_test.go
// 的实测（注册链路本身是通的，缺的只是 SDK 那道 type 闸门）。
type larkConnection struct {
	appID     string
	appSecret string
	domain    string
	onRaw     RawEventHandler
	logger    *slog.Logger

	mu     sync.Mutex
	client *larkws.Client
	cancel context.CancelFunc
	ready  chan struct{}
	done   chan struct{}
	runErr error

	readyOnce sync.Once
}

func newLarkConnection(appID, appSecret, domain string, onRaw RawEventHandler, logger *slog.Logger) *larkConnection {
	if logger == nil {
		logger = slog.Default()
	}
	return &larkConnection{
		appID:     appID,
		appSecret: appSecret,
		domain:    domain,
		onRaw:     onRaw,
		logger:    logger,
		ready:     make(chan struct{}),
		done:      make(chan struct{}),
	}
}

// buildDispatcher 把分发表里的每一类事件注册到 SDK 的 dispatcher 上。
//
// 普通事件走 OnCustomizedEvent：我们要的是原始信封而不是 SDK 的
// 类型对象（raw 要原样进审计，过一遍 SDK 模型再吐回来只会丢字段）。
// card.action.trigger 走 OnP2CardActionTrigger：它是 SDK 里唯一一条卡片回传的注册面
// （dispatcher 内部分两张表，callback 表优先于 event 表，自定义事件注册不到它）。
func (c *larkConnection) buildDispatcher() *dispatcher.EventDispatcher {
	d := dispatcher.NewEventDispatcher("", "")
	for eventType, entry := range eventTableSnapshot() {
		if entry.callback {
			d.OnP2CardActionTrigger(func(ctx context.Context, event *callback.CardActionTriggerEvent) (*callback.CardActionTriggerResponse, error) {
				if event == nil || event.EventReq == nil {
					return nil, nil
				}
				return nil, c.deliver(ctx, event.EventReq.Body)
			})
			continue
		}
		d.OnCustomizedEvent(eventType, func(ctx context.Context, event *larkevent.EventReq) error {
			return c.deliver(ctx, event.Body)
		})
	}
	return d
}

// deliver 把一份原始 payload 整理成信封投给 onRaw。
func (c *larkConnection) deliver(ctx context.Context, body []byte) error {
	var payload any
	if err := json.Unmarshal(body, &payload); err != nil {
		c.logger.Warn("feishu.frame_decode_failed", "err", err)
		return nil
	}
	m, ok := payload.(map[string]any)
	if !ok {
		c.logger.Warn("feishu.frame_decode_failed", "err", "payload 不是 JSON 对象")
		return nil
	}
	return c.onRaw(ctx, envelopeOf(m))
}

func (c *larkConnection) Connect(ctx context.Context) error {
	client := larkws.NewClient(c.appID, c.appSecret,
		larkws.WithEventHandler(c.buildDispatcher()),
		larkws.WithDomain(c.domain),
		// 重连归 Platform 管，SDK 自己那套固定间隔不要掺和进来。
		larkws.WithAutoReconnect(false),
		larkws.WithLogLevel(larkcore.LogLevelWarn),
		larkws.WithOnReady(func() { c.readyOnce.Do(func() { close(c.ready) }) }),
	)

	runCtx, cancel := context.WithCancel(ctx)
	c.mu.Lock()
	c.client = client
	c.cancel = cancel
	c.mu.Unlock()

	go func() {
		err := client.Start(runCtx)
		c.mu.Lock()
		c.runErr = err
		c.mu.Unlock()
		close(c.done)
	}()

	select {
	case <-c.ready:
		return nil
	case <-c.done:
		c.mu.Lock()
		err := c.runErr
		c.mu.Unlock()
		if err == nil {
			err = errConnectClosedBeforeReady
		}
		// 建连失败也要摘掉这棵 cancelCtx：它挂在 Start 那个跑一整条命的 ctx 上，
		// 不摘就是每失败一次多一个永不释放的 child。退避封顶 30s ≈ 每小时 120 次，
		// 断网一夜数千个。go vet 的 lostcancel 抓不到（cancel 赋给了 c.cancel，算用过）。
		cancel()
		return fmt.Errorf("feishu: 建连失败: %w", err)
	case <-ctx.Done():
		cancel()
		return ctx.Err()
	}
}

func (c *larkConnection) WaitClosed(ctx context.Context) error {
	select {
	case <-c.done:
		c.mu.Lock()
		err := c.runErr
		c.mu.Unlock()
		return err
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (c *larkConnection) Close() error {
	c.mu.Lock()
	client, cancel := c.client, c.cancel
	c.mu.Unlock()
	if client == nil {
		return nil // 从未 Connect 过，没有要等的 goroutine。
	}
	if cancel != nil {
		cancel()
	}
	client.Close()
	select {
	case <-c.done:
	case <-time.After(5 * time.Second):
		c.logger.Warn("feishu.close_timeout", "note", "长连接 5s 内没退干净，放手")
	}
	return nil
}

// dispatchRaw 把原始事件归一化后同步交给 sink。
//
// adapter 不去重：重连后平台重推的同一条也照样往上送，由 core 用 event_id 判。
//
// 与 Python 版的一处有意差异：Python 把 handler 的异常吞掉（只打日志），
// Go 版把 error 一路返回给 SDK，让平台重推（spec §2.1「失败 → 向平台返回错误让其重推」）。
func (p *Platform) dispatchRaw(ctx context.Context, raw map[string]any) error {
	if p.sink == nil {
		// New(cfg, opts, nil) 是合法构造（测试就这么用），但真收到事件时不能裸调。
		p.logger.Warn("feishu.no_sink", "note", "没接 EventSink，事件无处可送")
		return nil
	}
	event := Normalize(raw, p.opts.BotOpenID, p.opts.AppID, p.opts.TenantID)
	if event == nil {
		eventType := mapStr(asMap(raw["header"]), "event_type")
		if entry, ok := lookupEvent(eventType); ok && entry.onDropped != nil {
			entry.onDropped(p.logger, raw)
			return nil
		}
		p.logger.Debug("feishu.event_ignored", "type", eventType)
		return nil
	}

	started := p.clock()
	err := p.sink.HandleEvent(ctx, event)
	elapsed := p.clock().Sub(started)
	if elapsed > p.budget {
		p.logger.Warn("feishu.on_event_slow",
			"event_id", event.GetEventId(),
			"elapsed_sec", elapsed.Seconds(),
			"budget_sec", p.budget.Seconds())
	}
	if err != nil {
		p.logger.Error("feishu.on_event_failed", "event_id", event.GetEventId(), "err", err)
		return err
	}
	return nil
}

// Normalize 是事件总入口：查分发表，认识就归一化，不认识返回 nil。
func Normalize(raw map[string]any, botOpenID, workspaceID, tenantID string) *pb.NormalizedEvent {
	entry, ok := lookupEvent(mapStr(asMap(raw["header"]), "event_type"))
	if !ok {
		return nil
	}
	return entry.normalize(raw, botOpenID, workspaceID, tenantID)
}
