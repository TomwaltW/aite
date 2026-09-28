// 测试公共件：假时钟、日志捕获、假钉钉（REST 路由 + Stream 网关 + 假 ws 服务端）、记录型 sink。
//
// 全部只连 httptest 起的本地假服务，永不拨真实钉钉域名。
package dingtalk

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gorilla/websocket"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/server"
)

// Platform 必须满足 R0 定下的 PlatformPort。
var _ server.PlatformPort = (*Platform)(nil)

// 假凭证（永远是假值）。
const (
	testClientID     = "ding_fake_client_id"
	testClientSecret = "FAKE-CLIENT-SECRET-0000"
	testRobotCode    = "ding_fake_robot"
	testTemplateID   = "fake-template.schema"
	testAccessToken  = "FAKE-ACCESS-TOKEN-0000"
	testGroupChat    = "cidGROUP0001=="
	testP2PChat      = "cidP2P0001=="
	testStaffID      = "staff_0001"
	testCorpID       = "ding_corp_0001"
	testWSPath       = "/gateway/ws"
)

// ---------------------------------------------------------------------------
// 假时钟：手动推进；sleep 只记账，真睡 1ms 让出 CPU（重连失败时别空转）
// ---------------------------------------------------------------------------

type fakeClock struct {
	mu    sync.Mutex
	now   time.Time
	slept []time.Duration
}

func newFakeClock() *fakeClock { return &fakeClock{now: time.Unix(1_700_000_000, 0).UTC()} }

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

func (c *fakeClock) Sleep(ctx context.Context, d time.Duration) error {
	c.mu.Lock()
	c.slept = append(c.slept, d)
	c.now = c.now.Add(d)
	c.mu.Unlock()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-time.After(time.Millisecond):
		return nil
	}
}

func (c *fakeClock) Slept() []time.Duration {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]time.Duration(nil), c.slept...)
}

// ---------------------------------------------------------------------------
// 日志捕获
// ---------------------------------------------------------------------------

type logCapture struct {
	mu      sync.Mutex
	records []slog.Record
}

func newLogCapture() (*logCapture, *slog.Logger) {
	h := &logCapture{}
	return h, slog.New(h)
}

func (h *logCapture) Enabled(context.Context, slog.Level) bool { return true }

func (h *logCapture) Handle(_ context.Context, r slog.Record) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.records = append(h.records, r.Clone())
	return nil
}

func (h *logCapture) WithAttrs([]slog.Attr) slog.Handler { return h }
func (h *logCapture) WithGroup(string) slog.Handler      { return h }

// dump 把全部记录（消息 + 属性）拼成一段文本，用来断言「日志里不含某串」。
func (h *logCapture) dump() string {
	h.mu.Lock()
	defer h.mu.Unlock()
	var b strings.Builder
	for _, r := range h.records {
		b.WriteString(r.Message)
		r.Attrs(func(a slog.Attr) bool {
			fmt.Fprintf(&b, " %s=%v", a.Key, a.Value.Any())
			return true
		})
		b.WriteString("\n")
	}
	return b.String()
}

// ---------------------------------------------------------------------------
// 假钉钉：REST 路由 + Stream 网关 open + ws
// ---------------------------------------------------------------------------

type recordedRequest struct {
	method string
	path   string
	query  url.Values
	body   []byte
	header http.Header
}

func (r recordedRequest) jsonBody(t *testing.T) map[string]any {
	t.Helper()
	var out map[string]any
	if err := json.Unmarshal(r.body, &out); err != nil {
		t.Fatalf("请求体不是 JSON 对象：%v（%s）", err, r.body)
	}
	return out
}

type route struct {
	mu        *sync.Mutex
	responses []func(w http.ResponseWriter, req *http.Request)
	calls     []recordedRequest
}

func (r *route) count() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.calls)
}

func (r *route) at(t *testing.T, i int) recordedRequest {
	t.Helper()
	r.mu.Lock()
	defer r.mu.Unlock()
	if i >= len(r.calls) {
		t.Fatalf("这条路由只被调用了 %d 次，取不到第 %d 次", len(r.calls), i)
	}
	return r.calls[i]
}

type fakeDingtalk struct {
	*httptest.Server
	t *testing.T

	mu        sync.Mutex
	routes    map[string]*route
	unmatched []recordedRequest

	// Stream 网关
	opens      []map[string]any
	ticketSeq  int
	tickets    map[string]bool // ticket → 已用
	wsTickets  []string        // 每次 ws 握手带来的 ticket（含被拒的）
	rejected   int
	conns      chan *gwConn
	upgrader   websocket.Upgrader
	openStatus int // 非 0 → open 直接回这个状态码
}

func newFakeDingtalk(t *testing.T) *fakeDingtalk {
	t.Helper()
	f := &fakeDingtalk{
		t:       t,
		routes:  map[string]*route{},
		tickets: map[string]bool{},
		conns:   make(chan *gwConn, 16),
	}
	f.Server = httptest.NewServer(http.HandlerFunc(f.serve))
	t.Cleanup(f.Close)
	return f
}

func routeKey(method, path string) string { return method + " " + path }

// on 注册一条路由，每次调用按顺序取一个响应；用完后重复最后一个。
func (f *fakeDingtalk) on(method, path string, responses ...func(http.ResponseWriter, *http.Request)) *route {
	f.mu.Lock()
	defer f.mu.Unlock()
	r := &route{mu: &f.mu, responses: responses}
	f.routes[routeKey(method, path)] = r
	return r
}

func (f *fakeDingtalk) onJSON(method, path string, status int, body any) *route {
	return f.on(method, path, jsonResponse(status, body))
}

func jsonResponse(status int, body any) func(http.ResponseWriter, *http.Request) {
	encoded, err := json.Marshal(body)
	if err != nil {
		panic(err)
	}
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		_, _ = w.Write(encoded)
	}
}

// mockToken 注册 accessToken 的默认成功响应。
func (f *fakeDingtalk) mockToken() *route {
	return f.onJSON(http.MethodPost, PathAccessToken, 200, map[string]any{
		"accessToken": testAccessToken, "expireIn": 7200,
	})
}

func (f *fakeDingtalk) serve(w http.ResponseWriter, req *http.Request) {
	switch req.URL.Path {
	case PathGatewayOpen:
		f.serveOpen(w, req)
		return
	case testWSPath:
		f.serveWS(w, req)
		return
	}
	body, _ := io.ReadAll(req.Body)
	rec := recordedRequest{
		method: req.Method, path: req.URL.Path, query: req.URL.Query(),
		body: body, header: req.Header.Clone(),
	}
	f.mu.Lock()
	r, ok := f.routes[routeKey(req.Method, req.URL.Path)]
	if !ok {
		f.unmatched = append(f.unmatched, rec)
		f.mu.Unlock()
		w.WriteHeader(http.StatusNotFound)
		_, _ = w.Write([]byte(`{"code":"NotFound","message":"no route in fake dingtalk"}`))
		return
	}
	r.calls = append(r.calls, rec)
	idx := min(len(r.calls)-1, len(r.responses)-1)
	respond := r.responses[idx]
	f.mu.Unlock()
	respond(w, req)
}

func (f *fakeDingtalk) serveOpen(w http.ResponseWriter, req *http.Request) {
	var body map[string]any
	_ = json.NewDecoder(req.Body).Decode(&body)
	f.mu.Lock()
	f.opens = append(f.opens, body)
	status := f.openStatus
	f.ticketSeq++
	ticket := fmt.Sprintf("ticket-%d", f.ticketSeq)
	f.tickets[ticket] = false
	f.mu.Unlock()
	if status != 0 {
		w.WriteHeader(status)
		return
	}
	endpoint := "ws" + strings.TrimPrefix(f.URL, "http") + testWSPath
	jsonResponse(200, map[string]any{"endpoint": endpoint, "ticket": ticket})(w, req)
}

// serveWS 每个 ticket 只放行一次，复用或不认识即 401。
func (f *fakeDingtalk) serveWS(w http.ResponseWriter, req *http.Request) {
	ticket := req.URL.Query().Get("ticket")
	f.mu.Lock()
	f.wsTickets = append(f.wsTickets, ticket)
	used, known := f.tickets[ticket]
	if !known || used {
		f.rejected++
		f.mu.Unlock()
		w.WriteHeader(http.StatusUnauthorized)
		return
	}
	f.tickets[ticket] = true
	f.mu.Unlock()

	ws, err := f.upgrader.Upgrade(w, req, nil)
	if err != nil {
		return
	}
	c := &gwConn{ws: ws, ticket: ticket, acks: make(chan map[string]any, 64), closed: make(chan struct{})}
	go c.readAcks()
	f.t.Cleanup(func() { _ = ws.Close() })
	f.conns <- c
}

func (f *fakeDingtalk) openBodies() []map[string]any {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]map[string]any(nil), f.opens...)
}

func (f *fakeDingtalk) handshakeTickets() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.wsTickets...)
}

// nextConn 等下一条建好的 ws 连接。
func (f *fakeDingtalk) nextConn(t *testing.T) *gwConn {
	t.Helper()
	select {
	case c := <-f.conns:
		return c
	case <-time.After(5 * time.Second):
		t.Fatal("5 秒内没等到客户端建 ws 连接")
		return nil
	}
}

// gwConn 是假网关这一侧的一条 ws 连接。
type gwConn struct {
	ws      *websocket.Conn
	ticket  string
	writeMu sync.Mutex
	acks    chan map[string]any
	closed  chan struct{}
}

func (c *gwConn) readAcks() {
	defer close(c.closed)
	for {
		_, payload, err := c.ws.ReadMessage()
		if err != nil {
			return
		}
		var m map[string]any
		if json.Unmarshal(payload, &m) == nil {
			c.acks <- m
		}
	}
}

// push 下发一帧；data 会被编码成 JSON 字符串。
func (c *gwConn) push(t *testing.T, frameType, topic, messageID string, data any) {
	t.Helper()
	var dataStr string
	switch d := data.(type) {
	case string:
		dataStr = d
	default:
		encoded, err := json.Marshal(d)
		if err != nil {
			t.Fatalf("编码 data 失败：%v", err)
		}
		dataStr = string(encoded)
	}
	frame := map[string]any{
		"specVersion": "1.0",
		"type":        frameType,
		"time":        time.Now().UnixMilli(),
		"headers": map[string]any{
			"messageId": messageID, "topic": topic, "contentType": "application/json",
			"appId": "fake-app", "time": "0",
		},
		"data": dataStr,
	}
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	if err := c.ws.WriteJSON(frame); err != nil {
		t.Fatalf("下发帧失败：%v", err)
	}
}

// waitAck 等 messageId 对得上的那条 ACK（别的 ACK 丢掉）。
func (c *gwConn) waitAck(t *testing.T, messageID string, within time.Duration) map[string]any {
	t.Helper()
	deadline := time.After(within)
	for {
		select {
		case ack := <-c.acks:
			if headers, _ := ack["headers"].(map[string]any); headers["messageId"] == messageID {
				return ack
			}
		case <-deadline:
			t.Fatalf("%v 内没等到 messageId=%q 的 ACK", within, messageID)
			return nil
		}
	}
}

// ---------------------------------------------------------------------------
// sink
// ---------------------------------------------------------------------------

var errSinkDown = errors.New("sink down")

type recordingSink struct {
	mu     sync.Mutex
	events []*pb.NormalizedEvent
	got    chan *pb.NormalizedEvent
	err    error
}

func newRecordingSink() *recordingSink {
	return &recordingSink{got: make(chan *pb.NormalizedEvent, 64)}
}

func (s *recordingSink) HandleEvent(_ context.Context, ev *pb.NormalizedEvent) error {
	s.mu.Lock()
	s.events = append(s.events, ev)
	err := s.err
	s.mu.Unlock()
	s.got <- ev
	return err
}

func (s *recordingSink) next(t *testing.T) *pb.NormalizedEvent {
	t.Helper()
	select {
	case ev := <-s.got:
		return ev
	case <-time.After(5 * time.Second):
		t.Fatal("5 秒内 sink 没收到事件")
		return nil
	}
}

// blockingSink 每次调用先报到（entered），再阻塞到 release 关掉。
type blockingSink struct {
	entered chan *pb.NormalizedEvent
	release chan struct{}
}

func newBlockingSink() *blockingSink {
	return &blockingSink{entered: make(chan *pb.NormalizedEvent, 64), release: make(chan struct{})}
}

func (s *blockingSink) HandleEvent(ctx context.Context, ev *pb.NormalizedEvent) error {
	s.entered <- ev
	select {
	case <-s.release:
	case <-ctx.Done():
	}
	return nil
}

// ---------------------------------------------------------------------------
// 造 Platform
// ---------------------------------------------------------------------------

type platformBuild struct {
	apiBase        string
	sink           EventSink
	clock          *fakeClock
	logger         *slog.Logger
	cardTemplateID string
}

func mustPlatform(t *testing.T, b platformBuild) *Platform {
	t.Helper()
	if b.clock == nil {
		b.clock = newFakeClock()
	}
	if b.logger == nil {
		_, b.logger = newLogCapture()
	}
	p, err := newPlatform(platformOptions{
		cfg: DefaultConfig(),
		opts: Options{
			ClientID: testClientID, ClientSecret: testClientSecret, RobotCode: testRobotCode,
			CardTemplateID: b.cardTemplateID, APIBase: b.apiBase,
		},
		sink:       b.sink,
		httpClient: &http.Client{Timeout: 5 * time.Second},
		dialer:     &websocket.Dialer{HandshakeTimeout: 5 * time.Second},
		sleep:      b.clock.Sleep,
		clock:      b.clock.Now,
		logger:     b.logger,
	})
	if err != nil {
		t.Fatalf("建 Platform 失败：%v", err)
	}
	return p
}

// startPlatform 在后台跑 Start；测试结束时取消并等它返回。
func startPlatform(t *testing.T, p *Platform) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() {
		defer close(done)
		_ = p.Start(ctx)
	}()
	t.Cleanup(func() {
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Error("ctx 取消后 5 秒 Start 还没返回")
		}
	})
}

// eventually 轮询到条件成立或超时。
func eventually(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("5 秒内没等到：%s", what)
}

// ---------------------------------------------------------------------------
// fixture
// ---------------------------------------------------------------------------

func loadFixture(t *testing.T, name string) map[string]any {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join("testdata", name))
	if err != nil {
		t.Fatalf("读 fixture 失败：%v", err)
	}
	var out map[string]any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("解析 fixture 失败：%v", err)
	}
	return out
}

func sampleCard() *pb.ChecklistCard {
	note := "共 3 个 sheet"
	return &pb.ChecklistCard{
		TaskId:    "b7c1e6f0-1111-4222-8333-444455556666",
		TaskNo:    "#AH",
		Title:     "把 Q3 销售数据画成趋势图",
		Initiator: "张三",
		StartedAt: "9:02",
		Status:    pb.CardStatus_CARD_STATUS_WORKING,
		Items: []*pb.ChecklistItemView{
			{Id: "c1", Text: "读取 CSV", State: pb.ChecklistState_CHECKLIST_STATE_DONE},
			{Id: "c2", Text: "按月汇总", State: pb.ChecklistState_CHECKLIST_STATE_DOING, Note: &note},
			{Id: "c3", Text: "出图并回传", State: pb.ChecklistState_CHECKLIST_STATE_TODO},
		},
		Footer:  "预计 2 分钟 · 已用 ¥0.12",
		Actions: []pb.CardActionKind{pb.CardActionKind_CARD_ACTION_KIND_STOP},
	}
}

func strptr(s string) *string { return &s }
