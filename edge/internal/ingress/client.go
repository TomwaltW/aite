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

// Counters 是给 !status / 运维看的快照。键名与 spec §2.2 的计数器名一致。
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

	connected  atomic.Bool
	everUp     atomic.Bool
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

func (c *Client) client() (pb.IngressServiceClient, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
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

// watch 把 gRPC 的连接状态迁移翻成日志与计数。退避本身由 ConnectParams 执行。
func (c *Client) watch(ctx context.Context, conn *grpc.ClientConn) {
	down := false
	for {
		state := conn.GetState()
		switch state {
		case connectivity.Ready:
			c.connected.Store(true)
			if down {
				c.reconnects.Add(1)
				slog.Info("ingress.reconnected", "socket", c.socket, "reconnects", c.reconnects.Load())
			} else if !c.everUp.Swap(true) {
				slog.Info("ingress.connected", "socket", c.socket)
			}
			down = false
		case connectivity.TransientFailure:
			// 只在「刚掉下来」时喊一声：一次外网故障里 gRPC 会反复 TF→Connecting→TF，
			// 每轮都打就把日志刷爆了。
			if !down {
				down = true
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
