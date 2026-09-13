// Package ingress 是 edge → core 的 IngressService 客户端（proto/aite/v1/edge.proto）。
//
// owner: R2。语义（spec §2.1 / §2.2）：
//   - 懒连接：core 先起还是 edge 先起都行；连不上就按 1→2→4→…→30s 封顶退避无限重连，
//     进程不退出。退避交给 gRPC 自己的 ConnectParams 执行，本包只把状态迁移翻成
//     ingress.reconnecting / ingress.reconnected 两行日志与计数。
//   - HandleEvent 带 deadline（配置项 edge.handle_event_deadline_ms，默认 1s）：
//     core 必须 1s 内返回（只做去重/入库/入队）。
//   - INVALID_ARGUMENT 是「core 说这条事件非法」：计 ingress.invalid，**不重推**
//     （返回 nil 让平台 SDK 确认掉，重推多少次都还是非法）。
//   - 其余失败计 ingress.errors 并把 error 返回去，让平台重推；重复由 core 靠
//     event_id 去重。去重只在 core，edge 不做。
package ingress

import (
	"context"
	"errors"
	"log/slog"
	"net"
	"sync"
	"sync/atomic"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/backoff"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/connectivity"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/status"

	pb "aite/edge/gen/aitepb"
)

// 退避：1→2→4→…→30s 封顶，无限重试（spec §2.1）。
const (
	defaultBaseDelay = 1 * time.Second
	defaultMaxDelay  = 30 * time.Second
)

// Counters 是给运维看的快照。键名与 spec §2.2 的计数器名一致。
//
// **`!status` 看不到这些。** 原注释写的是「给 !status / 运维看的」，而 `Counters()` 在产品
// 代码里只有一个调用方：`cmd/aite-edge/main.go` 收尾时打的那行 `edge.counters` 日志。
// 这四个数一个都不过线（`EdgeStatus` 里没有它们），而 `!status` 由 core 的
// `control::cmd_status` 答，只从 store 列活跃任务；它唯一会多报的计数是 core 自己的
// `events.dropped`（`plane.rs` 的 `dropped_note`），与 edge 无关。
// 与本文件 `noteUp` / `HandleEvent` 里那两句、`main.go` 的 `platform_connected` 那句同源。
type Counters struct {
	Sent       int64 `json:"events.sent"`
	Invalid    int64 `json:"ingress.invalid"`
	Errors     int64 `json:"ingress.errors"`
	Reconnects int64 `json:"ingress.reconnects"`
}

type Client struct {
	socket   string
	deadline time.Duration
	maxMsg   int

	// 退避参数：生产是 1s/30s，测试里调小（同包可写）。
	baseDelay time.Duration
	maxDelay  time.Duration

	mu     sync.Mutex
	conn   *grpc.ClientConn
	cancel context.CancelFunc
	// Close() 之后就是终态：再调 HandleEvent 不许把连接和 watch goroutine 复活。
	// 收尾序列是「停投递 → 关连接」，复活一条就等于收尾没收干净。
	closed bool

	connected atomic.Bool
	everUp    atomic.Bool
	// 「当前认为断着」。watch 与 HandleEvent 两头都会写它：谁先看出来算谁的，
	// 另一头就是空转。做成字段而不是 watch 的局部变量，是因为一次成功的 RPC
	// 也是「通了」的证据，而它发生在 watch 之外。
	down       atomic.Bool
	sent       atomic.Int64
	invalid    atomic.Int64
	errs       atomic.Int64
	reconnects atomic.Int64
}

func New(coreSocket string, deadline time.Duration, maxMessageMB int) *Client {
	return &Client{
		socket:    coreSocket,
		deadline:  deadline,
		maxMsg:    maxMessageMB * 1024 * 1024,
		baseDelay: defaultBaseDelay,
		maxDelay:  defaultMaxDelay,
	}
}

// ErrClosed 是 Close() 之后再投事件的答复。返回 error（而不是静默成功）是因为
// 这条事件确实没送到 core —— 让平台重推，重复交给 core 靠 event_id 去重。
var ErrClosed = errors.New("ingress 客户端已关闭")

func (c *Client) client() (pb.IngressServiceClient, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return nil, ErrClosed
	}
	if c.conn == nil {
		sock := c.socket
		conn, err := grpc.NewClient("unix:"+sock,
			grpc.WithTransportCredentials(insecure.NewCredentials()),
			grpc.WithDefaultCallOptions(grpc.MaxCallRecvMsgSize(c.maxMsg), grpc.MaxCallSendMsgSize(c.maxMsg)),
			grpc.WithConnectParams(grpc.ConnectParams{
				Backoff: backoff.Config{
					BaseDelay:  c.baseDelay,
					Multiplier: 2,
					Jitter:     0,
					MaxDelay:   c.maxDelay,
				},
				MinConnectTimeout: c.baseDelay,
			}),
			grpc.WithContextDialer(func(ctx context.Context, _ string) (net.Conn, error) {
				var d net.Dialer
				return d.DialContext(ctx, "unix", sock)
			}))
		if err != nil {
			return nil, err
		}
		ctx, cancel := context.WithCancel(context.Background())
		c.conn, c.cancel = conn, cancel
		go c.watch(ctx, conn)
		conn.Connect() // 别等第一条事件才拨号：core 没起来时要立刻开始退避重连
	}
	return pb.NewIngressServiceClient(c.conn), nil
}

// noteUp 记一次「通了」。
//
// **第一次连上不算重连**，哪怕之前已经 TransientFailure 过一轮：core 晚起来是常态
// （§2.1 启动顺序无关），那一路必然先 TF 再 Ready —— 照「断过就 +1」算的话
// `ingress.reconnects` 会报「重连 1 次」，而它从没断过。
//
// 原注释这里写的是「`!status` 会说」，那是同一句谎话的又一个说法：这个计数只出现在
// 收尾那行 `edge.counters` 日志里，`!status` 看不到（见 `Counters` 的注释）。
func (c *Client) noteUp() {
	c.connected.Store(true)
	wasDown := c.down.Swap(false)
	if !c.everUp.Swap(true) {
		slog.Info("ingress.connected", "socket", c.socket)
		return
	}
	if wasDown {
		c.reconnects.Add(1)
		slog.Info("ingress.reconnected", "socket", c.socket, "reconnects", c.reconnects.Load())
	}
}

// watch 把 gRPC 的连接状态迁移翻成日志与计数。退避本身由 ConnectParams 执行。
func (c *Client) watch(ctx context.Context, conn *grpc.ClientConn) {
	for {
		state := conn.GetState()
		switch state {
		case connectivity.Ready:
			c.noteUp()
		case connectivity.TransientFailure:
			// 只在「刚掉下来」时喊一声：一次外网故障里 gRPC 会反复 TF→Connecting→TF，
			// 每轮都打就把日志刷爆了。
			if !c.down.Swap(true) {
				slog.Warn("ingress.reconnecting", "socket", c.socket,
					"base_delay", c.baseDelay.String(), "max_delay", c.maxDelay.String())
			}
			c.connected.Store(false)
		case connectivity.Idle:
			// 空闲回落也要接着拨，否则「无限重连」就断在这儿了。
			c.connected.Store(false)
			conn.Connect()
		case connectivity.Shutdown:
			c.connected.Store(false)
			return
		default: // Connecting
			c.connected.Store(false)
		}
		if !conn.WaitForStateChange(ctx, state) {
			return
		}
	}
}

// HandleEvent 把一条归一化事件送给 core；超过 deadline 即失败（core 必须 1s 内返回）。
// 返回 nil = 平台可以确认这条事件；返回 error = 让平台重推。
func (c *Client) HandleEvent(ctx context.Context, ev *pb.NormalizedEvent) error {
	cl, err := c.client()
	if err != nil {
		c.errs.Add(1)
		slog.Error("ingress.handle_failed", "event", ev.GetEventId(), "kind", ev.GetKind().String(), "err", err)
		return err
	}
	cctx, cancel := context.WithTimeout(ctx, c.deadline)
	defer cancel()

	if _, err := cl.HandleEvent(cctx, ev); err != nil {
		if status.Code(err) == codes.InvalidArgument {
			// core 判这条事件非法（anchor 缺失、枚举 UNSPECIFIED…）：重推也还是非法。
			c.invalid.Add(1)
			slog.Warn("ingress.invalid", "event", ev.GetEventId(), "kind", ev.GetKind().String(), "err", err)
			return nil
		}
		c.errs.Add(1)
		slog.Error("ingress.handle_failed", "event", ev.GetEventId(), "kind", ev.GetKind().String(), "err", err)
		return err
	}
	c.sent.Add(1)
	// 一次成功的 RPC 就是「现在连得上」的最强证据 —— 比 watch 的状态迁移新鲜。
	// 不加这条的话：watch 那边要等 gRPC 自己迁移到 Ready 才翻 true，这中间
	// Connected() 会报「没连上」，而 RPC 其实已经跑通了。
	//
	// 原注释说那段空窗里「`!status` 的健康行会报没连上」—— 那条健康行**全仓不存在**
	// （`!status` 由 core 的 `control::cmd_status` 答，只从 store 列活跃任务、不碰 edge）。
	// 这是 core 侧同一句谎话抄过来的副本：`link.note_ok()` 的注释原本也这么写，
	// 那边已经改掉（W2/X1/Y2/Z1 共六处），这边补齐 —— 修法同源这半句是对的，
	// 连病一起抄过来的是括注。
	//
	// 这个标志现在谁在看：`Connected()` 在**产品代码里零调用方**，只有
	// `client_test.go` / `client_lifecycle_test.go` 拿它当判据（`EdgeStatus.platform_connected`
	// 填的是 `feishu.Platform.Connected()`，不是这个）。真有人拿它出健康行的那天，
	// 有这一句答案才是对的 —— 所以行留着，话说准。
	c.noteUp()
	return nil
}

// Connected 报告到 core 的连接当前是否可用（EdgeStatus 用）。
func (c *Client) Connected() bool { return c.connected.Load() }

// Counters 返回计数快照。
func (c *Client) Counters() Counters {
	return Counters{
		Sent:       c.sent.Load(),
		Invalid:    c.invalid.Load(),
		Errors:     c.errs.Load(),
		Reconnects: c.reconnects.Load(),
	}
}

func (c *Client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.closed = true
	if c.cancel != nil {
		c.cancel()
		c.cancel = nil
	}
	c.connected.Store(false)
	if c.conn != nil {
		err := c.conn.Close()
		c.conn = nil
		return err
	}
	return nil
}
