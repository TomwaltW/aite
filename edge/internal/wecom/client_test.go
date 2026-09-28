package wecom

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gorilla/websocket"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// ---------------------------------------------------------------------------
// 假企微长连接服务端（只监听 127.0.0.1，永不拨 openws.work.weixin.qq.com）
// ---------------------------------------------------------------------------

// recvFrame 是假服务端收到的一帧。
type recvFrame struct {
	Conn  int
	Cmd   string
	ReqID string
	Body  map[string]any
}

type serverConn struct {
	idx int
	c   *websocket.Conn
	wmu sync.Mutex
}

// push 往这条连接推一帧回调。
func (sc *serverConn) push(cmd, reqID string, body any) error {
	data, err := json.Marshal(map[string]any{"cmd": cmd, "headers": map[string]any{"req_id": reqID}, "body": body})
	if err != nil {
		return err
	}
	sc.wmu.Lock()
	defer sc.wmu.Unlock()
	return sc.c.WriteMessage(websocket.TextMessage, data)
}

func (sc *serverConn) ack(reqID string, code int, body any) error {
	m := map[string]any{"headers": map[string]any{"req_id": reqID}, "errcode": code, "errmsg": "ok"}
	if code != 0 {
		m["errmsg"] = "rejected"
	}
	if body != nil {
		m["body"] = body
	}
	data, err := json.Marshal(m)
	if err != nil {
		return err
	}
	sc.wmu.Lock()
	defer sc.wmu.Unlock()
	return sc.c.WriteMessage(websocket.TextMessage, data)
}

func (sc *serverConn) close() { _ = sc.c.Close() }

// ackFunc 决定怎么应答一帧：返回 errcode 与应答 body。
type ackFunc func(f recvFrame) (int, any)

type fakeServer struct {
	*httptest.Server
	t  *testing.T
	up websocket.Upgrader

	mu     sync.Mutex
	conns  []*serverConn
	frames []recvFrame
	ackFn  ackFunc
	notify chan struct{}
}

func newFakeServer(t *testing.T, ack ackFunc) *fakeServer {
	t.Helper()
	fs := &fakeServer{t: t, ackFn: ack, notify: make(chan struct{}, 1)}
	fs.Server = httptest.NewServer(http.HandlerFunc(fs.handle))
	t.Cleanup(func() {
		fs.mu.Lock()
		for _, c := range fs.conns {
			c.close()
		}
		fs.mu.Unlock()
		fs.Close()
	})
	return fs
}

func (fs *fakeServer) wsURL() string { return strings.Replace(fs.URL, "http://", "ws://", 1) }

func (fs *fakeServer) handle(w http.ResponseWriter, r *http.Request) {
	c, err := fs.up.Upgrade(w, r, nil)
	if err != nil {
		fs.t.Errorf("websocket 升级失败：%v", err)
		return
	}
	fs.mu.Lock()
	sc := &serverConn{idx: len(fs.conns), c: c}
	fs.conns = append(fs.conns, sc)
	fs.mu.Unlock()
	fs.wake()

	for {
		_, data, err := c.ReadMessage()
		if err != nil {
			return
		}
		var raw struct {
			Cmd     string         `json:"cmd"`
			Headers frameHeaders   `json:"headers"`
			Body    map[string]any `json:"body"`
		}
		if err := json.Unmarshal(data, &raw); err != nil {
			fs.t.Errorf("客户端发了非 JSON 帧：%v", err)
			return
		}
		f := recvFrame{Conn: sc.idx, Cmd: raw.Cmd, ReqID: raw.Headers.ReqID, Body: raw.Body}
		// 先记账再应答：客户端的请求返回时，这一帧一定已经在 frames 里。
		fs.mu.Lock()
		fs.frames = append(fs.frames, f)
		fs.mu.Unlock()
		fs.wake()
		code, body := 0, any(nil)
		if fs.ackFn != nil {
			code, body = fs.ackFn(f)
		}
		if code >= 0 {
			_ = sc.ack(f.ReqID, code, body)
		}
	}
}

func (fs *fakeServer) wake() {
	select {
	case fs.notify <- struct{}{}:
	default:
	}
}

func (fs *fakeServer) dials() int {
	fs.mu.Lock()
	defer fs.mu.Unlock()
	return len(fs.conns)
}

func (fs *fakeServer) conn(i int) *serverConn {
	fs.mu.Lock()
	defer fs.mu.Unlock()
	return fs.conns[i]
}

func (fs *fakeServer) snapshot() []recvFrame {
	fs.mu.Lock()
	defer fs.mu.Unlock()
	return append([]recvFrame(nil), fs.frames...)
}

func (fs *fakeServer) framesWith(cmd string) []recvFrame {
	var out []recvFrame
	for _, f := range fs.snapshot() {
		if f.Cmd == cmd {
			out = append(out, f)
		}
	}
	return out
}

// waitFor 等到 cond 成立；5 秒兜底只防挂死，不是计时断言。
func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("等不到：%s", what)
		}
		time.Sleep(2 * time.Millisecond)
	}
}

// ---------------------------------------------------------------------------
// 注入件：时钟、sleep、tick、sink
// ---------------------------------------------------------------------------

type fakeClock struct {
	mu  sync.Mutex
	now time.Time
}

func newFakeClock() *fakeClock {
	return &fakeClock{now: time.Date(2026, 9, 25, 9, 0, 0, 0, time.UTC)}
}

func (c *fakeClock) Now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.now
}

func (c *fakeClock) Advance(d time.Duration) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.now = c.now.Add(d)
}

// recordingSleep 记下每次退避时长并立即返回（时间「飞快流逝」）。
type recordingSleep struct {
	mu    sync.Mutex
	calls []time.Duration
}

func (s *recordingSleep) sleep(ctx context.Context, d time.Duration) error {
	s.mu.Lock()
	s.calls = append(s.calls, d)
	s.mu.Unlock()
	return ctx.Err()
}

func (s *recordingSleep) recorded() []time.Duration {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]time.Duration(nil), s.calls...)
}

type recordingSink struct {
	mu     sync.Mutex
	events []*pb.NormalizedEvent
}

func (s *recordingSink) HandleEvent(_ context.Context, ev *pb.NormalizedEvent) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.events = append(s.events, ev)
	return nil
}

func (s *recordingSink) all() []*pb.NormalizedEvent {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]*pb.NormalizedEvent(nil), s.events...)
}

// harness 是一台连着假服务端的 Platform。
type harness struct {
	p     *Platform
	fs    *fakeServer
	sink  *recordingSink
	clock *fakeClock
	tick  chan time.Time
	done  chan error
	stop  context.CancelFunc
}

// startHarness 起一台 Platform 连到 fs；tweak 可以改注入面。默认心跳 1 小时（不打扰）。
func startHarness(t *testing.T, fs *fakeServer, tweak func(po *platformOptions)) *harness {
	t.Helper()
	h := &harness{fs: fs, sink: &recordingSink{}, clock: newFakeClock(), tick: make(chan time.Time)}
	po := platformOptions{
		cfg:       DefaultConfig(),
		opts:      Options{BotID: "bot-test", BotSecret: "secret-test", WSURL: fs.wsURL()},
		sink:      h.sink,
		clock:     h.clock.Now,
		sleep:     func(ctx context.Context, _ time.Duration) error { return ctx.Err() },
		ticker:    func(time.Duration) (<-chan time.Time, func()) { return h.tick, func() {} },
		pingEvery: time.Hour,
	}
	if tweak != nil {
		tweak(&po)
	}
	p, err := newPlatform(po)
	if err != nil {
		t.Fatalf("newPlatform: %v", err)
	}
	h.p = p
	ctx, cancel := context.WithCancel(context.Background())
	h.stop = cancel
	h.done = make(chan error, 1)
	go func() { h.done <- p.Start(ctx) }()
	t.Cleanup(func() {
		cancel()
		select {
		case <-h.done:
		case <-time.After(5 * time.Second):
			t.Errorf("ctx 取消后 Start 没返回")
		}
	})
	return h
}

// waitConnected 等到订阅应答回来、Connected() 为真。
func (h *harness) waitConnected(t *testing.T) {
	t.Helper()
	waitFor(t, "Connected()", h.p.Connected)
}

// inbound 从假服务端推一条消息回调，等 sink 收到（返回那个事件）。
func (h *harness) inbound(t *testing.T, reqID string, body map[string]any) *pb.NormalizedEvent {
	t.Helper()
	before := len(h.sink.all())
	if err := h.fs.conn(h.fs.dials()-1).push(cmdMsgCallback, reqID, body); err != nil {
		t.Fatalf("push: %v", err)
	}
	waitFor(t, "sink 收到事件", func() bool { return len(h.sink.all()) > before })
	return h.sink.all()[before]
}

func textMsg(msgID, chatID, chatType, content string) map[string]any {
	return map[string]any{
		"msgid": msgID, "aibotid": "bot-test", "chatid": chatID, "chattype": chatType,
		"from": map[string]any{"userid": "u-1"}, "msgtype": "text",
		"text": map[string]any{"content": content},
	}
}

func platformCode(err error) string {
	var pe *aiteerr.PlatformError
	if errors.As(err, &pe) {
		return pe.Code
	}
	return ""
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

func TestSubscribePingAndCallback(t *testing.T) {
	if pingInterval != 30*time.Second {
		t.Fatalf("默认心跳间隔 = %v，原卡要求 30s", pingInterval)
	}
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, func(po *platformOptions) { po.pingEvery = 100 * time.Millisecond })
	h.waitConnected(t)

	waitFor(t, "至少两帧 ping", func() bool { return len(fs.framesWith(cmdPing)) >= 2 })
	frames := fs.snapshot()
	first := frames[0]
	if first.Conn != 0 || first.Cmd != cmdSubscribe {
		t.Fatalf("第一帧 = %+v，必须是 %s", first, cmdSubscribe)
	}
	if first.Body["bot_id"] != "bot-test" || first.Body["secret"] != "secret-test" {
		t.Fatalf("订阅帧没带 bot_id / secret：%v", first.Body)
	}
	if first.ReqID == "" {
		t.Fatalf("订阅帧没有 req_id")
	}

	ev := h.inbound(t, "rq-cb-1", textMsg("m-1", "chat-1", "group", "@Aite 你好"))
	if got := len(h.sink.all()); got != 1 {
		t.Fatalf("sink 收到 %d 个事件，要 1 个", got)
	}
	if ev.GetKind() != pb.EventKind_EVENT_KIND_MESSAGE || ev.GetText() != "你好" {
		t.Fatalf("事件不对：kind=%v text=%q", ev.GetKind(), ev.GetText())
	}
}

func TestSubscribeRejectedBacksOff(t *testing.T) {
	fs := newFakeServer(t, func(f recvFrame) (int, any) {
		if f.Cmd == cmdSubscribe {
			return 40058, nil
		}
		return 0, nil
	})
	rs := &recordingSleep{}
	h := startHarness(t, fs, func(po *platformOptions) { po.sleep = rs.sleep })

	waitFor(t, "被拒后再拨一次", func() bool { return fs.dials() >= 2 })
	if h.p.Connected() {
		t.Fatalf("订阅被拒还报 Connected()")
	}
	calls := rs.recorded()
	if len(calls) == 0 || calls[0] != time.Second {
		t.Fatalf("第一次退避 = %v，要 1s", calls)
	}
	if h.p.ReconnectCount() != 0 {
		t.Fatalf("从没连上过，ReconnectCount = %d", h.p.ReconnectCount())
	}
}

func TestReconnectAfterDropCounts(t *testing.T) {
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, nil)
	h.waitConnected(t)
	if h.p.ReconnectCount() != 0 {
		t.Fatalf("首连不算重连：%d", h.p.ReconnectCount())
	}

	fs.conn(0).close()
	waitFor(t, "第二条连接", func() bool { return fs.dials() >= 2 })
	waitFor(t, "重连成功计数", func() bool { return h.p.Connected() && h.p.ReconnectCount() == 1 })
	subs := fs.framesWith(cmdSubscribe)
	if len(subs) != 2 || subs[1].Conn != 1 {
		t.Fatalf("重连后要在新连接上重新订阅：%+v", subs)
	}
}
