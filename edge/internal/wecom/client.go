// 企微 aibot 长连接的 ws 客户端：一条连接 = 一个 wsConn。
//
// 三条纪律：
//
//   - gorilla 的 Conn 只允许一个并发写者 → 所有写走 writeMu。
//   - 读循环从不阻塞在回调处理上：自己发出去的请求的应答也从同一条连接读回来，
//     而 enter_chat 欢迎语、模板卡片更新都是在回调处理里发请求的 —— 回调交给
//     独立的分发 goroutine，读循环只管读帧、对应答、续读超时。
//   - 帧日志只打 cmd + req_id；secret / aeskey 永不进日志。
package wecom

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"sync"
	"time"

	"github.com/gorilla/websocket"

	"aite/edge/internal/aiteerr"
)

// pingInterval 是心跳周期（原卡：每 30 秒一帧）；两个周期收不到任何帧判断线。
const pingInterval = 30 * time.Second

// defaultAckTimeout 是出站命令等应答的上限（ctx 更早到期以 ctx 为准）。
const defaultAckTimeout = 10 * time.Second

// callbackQueue 是分发队列长度；满了读循环才会等（背压到平台，而不是丢事件）。
const callbackQueue = 256

// dialFunc 建一条 ws 连接；测试注入它来连 httptest。
type dialFunc func(ctx context.Context, url string) (*websocket.Conn, error)

func defaultDial(ctx context.Context, url string) (*websocket.Conn, error) {
	conn, _, err := websocket.DefaultDialer.DialContext(ctx, url, nil)
	return conn, err
}

// callbackHandler 处理一帧回调（在分发 goroutine 上跑）。
type callbackHandler func(ctx context.Context, f *inFrame)

// ackReply 是一帧应答。
type ackReply struct {
	code int
	msg  string
	body json.RawMessage
}

// errConnClosed 是连接已断、请求等不到应答。
var errConnClosed = errors.New("wecom: 长连接已断开")

// wsConn 是一条已建立的长连接。
type wsConn struct {
	conn       *websocket.Conn
	logger     *slog.Logger
	pingEvery  time.Duration
	ackTimeout time.Duration

	writeMu sync.Mutex

	// pendingMu 护 pending：同一 req_id 上可能有多帧在途（流式刷新），按发送顺序 FIFO 对应答。
	pendingMu sync.Mutex
	pending   map[string][]chan ackReply

	callbacks chan *inFrame
	closed    chan struct{}
	closeOnce sync.Once
	readDone  chan struct{} // 读循环退出后关闭
	readErr   error         // 读循环退出原因；readDone 关闭后才可读
}

func newWSConn(conn *websocket.Conn, logger *slog.Logger, pingEvery, ackTimeout time.Duration) *wsConn {
	return &wsConn{
		conn:       conn,
		logger:     logger,
		pingEvery:  pingEvery,
		ackTimeout: ackTimeout,
		pending:    map[string][]chan ackReply{},
		callbacks:  make(chan *inFrame, callbackQueue),
		closed:     make(chan struct{}),
		readDone:   make(chan struct{}),
	}
}

// newReqID 生成本地 req_id / stream id：16 字节随机数的十六进制。
func newReqID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		// crypto/rand 不会失败（Go 1.24 起失败直接 fatal）；留个兜底别返回空串。
		return fmt.Sprintf("%x", time.Now().UnixNano())
	}
	return hex.EncodeToString(b[:])
}

// close 断开连接，幂等；所有在途请求以 errConnClosed 返回。
func (c *wsConn) close() {
	c.closeOnce.Do(func() {
		close(c.closed)
		_ = c.conn.Close()
	})
}

// write 发一帧（持写锁）。
func (c *wsConn) write(f outFrame) error {
	data, err := json.Marshal(f)
	if err != nil {
		return &aiteerr.PlatformError{Code: "bad_request", Msg: "帧序列化失败：" + err.Error()}
	}
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	if err := c.conn.WriteMessage(websocket.TextMessage, data); err != nil {
		return &aiteerr.PlatformError{Code: "network", Retryable: true, Msg: "写帧失败：" + err.Error()}
	}
	c.logger.Debug("wecom.frame_out", "cmd", f.Cmd, "req_id", f.Headers.ReqID)
	return nil
}

// request 发一帧并等同 req_id 的应答；errcode != 0 → *aiteerr.PlatformError。
func (c *wsConn) request(ctx context.Context, cmd, reqID string, body any) (json.RawMessage, error) {
	ch := make(chan ackReply, 1)
	c.pendingMu.Lock()
	c.pending[reqID] = append(c.pending[reqID], ch)
	c.pendingMu.Unlock()

	if err := c.write(outFrame{Cmd: cmd, Headers: frameHeaders{ReqID: reqID}, Body: body}); err != nil {
		c.dropWaiter(reqID, ch)
		return nil, err
	}

	timer := time.NewTimer(c.ackTimeout)
	defer timer.Stop()
	select {
	case r := <-ch:
		if r.code != 0 {
			return nil, &aiteerr.PlatformError{
				Code: errcodeString(r.code), Retryable: retryableErrcodes[r.code], Msg: r.msg,
			}
		}
		return r.body, nil
	case <-ctx.Done():
		c.dropWaiter(reqID, ch)
		return nil, &aiteerr.PlatformError{Code: "timeout", Retryable: true, Msg: cmd + " 等应答被取消：" + ctx.Err().Error()}
	case <-timer.C:
		c.dropWaiter(reqID, ch)
		return nil, &aiteerr.PlatformError{Code: "timeout", Retryable: true, Msg: cmd + " 等应答超时"}
	case <-c.closed:
		c.dropWaiter(reqID, ch)
		return nil, &aiteerr.PlatformError{Code: "network", Retryable: true, Msg: errConnClosed.Error()}
	}
}

// dropWaiter 把一个不再等的 waiter 从 FIFO 里摘掉（保持其余顺序）。
func (c *wsConn) dropWaiter(reqID string, ch chan ackReply) {
	c.pendingMu.Lock()
	defer c.pendingMu.Unlock()
	q := c.pending[reqID]
	for i, w := range q {
		if w == ch {
			q = append(q[:i:i], q[i+1:]...)
			break
		}
	}
	if len(q) == 0 {
		delete(c.pending, reqID)
	} else {
		c.pending[reqID] = q
	}
}

// resolve 把一帧应答交给同 req_id 上最早的那个 waiter；没人等（ping 的应答）就丢。
func (c *wsConn) resolve(reqID string, r ackReply) {
	c.pendingMu.Lock()
	defer c.pendingMu.Unlock()
	q := c.pending[reqID]
	if len(q) == 0 {
		return
	}
	ch := q[0]
	if len(q) == 1 {
		delete(c.pending, reqID)
	} else {
		c.pending[reqID] = q[1:]
	}
	ch <- r
}

// readLoop 一直读到连接断开：应答对给 waiter，回调塞进分发队列。
// 每收到一帧把读超时续到「两个心跳周期之后」—— 超了 ReadMessage 报错 = 判断线。
func (c *wsConn) readLoop() {
	defer close(c.readDone)
	defer close(c.callbacks)
	for {
		_ = c.conn.SetReadDeadline(time.Now().Add(2 * c.pingEvery))
		_, data, err := c.conn.ReadMessage()
		if err != nil {
			c.readErr = err
			c.close()
			return
		}
		var f inFrame
		if err := json.Unmarshal(data, &f); err != nil {
			c.logger.Warn("wecom.bad_frame", "err", err)
			continue
		}
		c.logger.Debug("wecom.frame_in", "cmd", f.Cmd, "req_id", f.Headers.ReqID)
		if f.isAck() {
			c.resolve(f.Headers.ReqID, ackReply{code: *f.ErrCode, msg: f.ErrMsg, body: f.Body})
			continue
		}
		if f.Cmd == "" {
			continue
		}
		select {
		case c.callbacks <- &f:
		case <-c.closed:
			return
		}
	}
}

// dispatchLoop 在独立 goroutine 上按到达顺序处理回调。
func (c *wsConn) dispatchLoop(ctx context.Context, handle callbackHandler) {
	for f := range c.callbacks {
		handle(ctx, f)
	}
}

// pingLoop 每 pingEvery 发一帧 ping，直到连接断开。ping 不等应答（读超时兜断线）。
func (c *wsConn) pingLoop() {
	ticker := time.NewTicker(c.pingEvery)
	defer ticker.Stop()
	for {
		select {
		case <-c.closed:
			return
		case <-ticker.C:
			if err := c.write(outFrame{Cmd: cmdPing, Headers: frameHeaders{ReqID: newReqID()}}); err != nil {
				c.logger.Warn("wecom.ping_failed", "err", err)
				c.close()
				return
			}
		}
	}
}

// subscribe 发订阅帧并等应答；errcode != 0 不算连上。
// 调用时读循环必须已经在跑（应答要靠它读回来），但 ping 还没起 —— 订阅帧必须是第一帧。
func (c *wsConn) subscribe(ctx context.Context, botID, secret string) error {
	_, err := c.request(ctx, cmdSubscribe, newReqID(), subscribeBody{BotID: botID, Secret: secret})
	return err
}
