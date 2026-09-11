// 对应 tests/adapters/conftest.py。
//
// 假时钟、假连接、日志捕获、假飞书服务器都在测试包里自建（不依赖别的轨）。
package feishu

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/config"
	"aite/edge/internal/server"
)

// Platform 必须满足 R0 定下的 PlatformPort。
var _ server.PlatformPort = (*Platform)(nil)

// 与 edge/testdata/feishu/*.json 里写死的是同一套 id。
const (
	testBotOpenID   = "ou_aite_bot_0000000000000000000001"
	testAppID       = "cli_a1b2c3d4e5f60123"
	testChatID      = "oc_chat_p0_demo_0001"
	testRootMsgID   = "om_toplevel_0001"
	testCardMsgID   = "om_checklist_card_0001"
	testFixturesDir = "../../testdata/feishu"
)

// ---------------------------------------------------------------------------
// 假时钟：手动推进；sleep 只记账、不真睡
// ---------------------------------------------------------------------------

type fakeClock struct {
	mu    sync.Mutex
	now   time.Time
	slept []time.Duration
}

func newFakeClock() *fakeClock {
	return &fakeClock{now: time.Unix(1_700_000_000, 0).UTC()}
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

func (c *fakeClock) Sleep(_ context.Context, d time.Duration) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.slept = append(c.slept, d)
	c.now = c.now.Add(d)
	return nil
}

func (c *fakeClock) Slept() []time.Duration {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]time.Duration, len(c.slept))
	copy(out, c.slept)
	return out
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

// find 返回 msg 逐字相等的所有记录。
func (h *logCapture) find(msg string) []slog.Record {
	h.mu.Lock()
	defer h.mu.Unlock()
	var out []slog.Record
	for _, r := range h.records {
		if r.Message == msg {
			out = append(out, r)
		}
	}
	return out
}

func (h *logCapture) has(msg string) bool { return len(h.find(msg)) > 0 }

// attr 取一条记录里某个属性的值。
func attr(r slog.Record, key string) (slog.Value, bool) {
	var out slog.Value
	found := false
	r.Attrs(func(a slog.Attr) bool {
		if a.Key == key {
			out, found = a.Value, true
			return false
		}
		return true
	})
	return out, found
}

// ---------------------------------------------------------------------------
// 假飞书服务器：记录方法、路径、query、body
// ---------------------------------------------------------------------------

type recordedRequest struct {
	method string
	path   string
	query  url.Values
	body   []byte
	header http.Header
}

// jsonBodyOf 把记录下来的请求体解析成 map。
func (r recordedRequest) jsonBodyOf(t *testing.T) map[string]any {
	t.Helper()
	var out map[string]any
	if err := json.Unmarshal(r.body, &out); err != nil {
		t.Fatalf("请求体不是 JSON 对象：%v（%s）", err, r.body)
	}
	return out
}

type route struct {
	// mu 是 fakeFeishu 那把锁的指针：写入侧（serve）持它，读取侧（count/called/last/at）
	// 也必须持。不持的话 -race 会在服务端 goroutine 还没退出时抓现行 ——
	// TestTransportErrorIsRetryable 就是这种形状：连接被掐断，断言先跑到了。
	mu        *sync.Mutex
	responses []func(w http.ResponseWriter)
	calls     []recordedRequest
}

type fakeFeishu struct {
	*httptest.Server
	mu     sync.Mutex
	routes map[string]*route
	// unmatched 记下没有注册路由的请求，测试里可以断言「这条路由一次都没碰」。
	unmatched []recordedRequest
}

func newFakeFeishu(t *testing.T) *fakeFeishu {
	t.Helper()
	f := &fakeFeishu{routes: map[string]*route{}}
	f.Server = httptest.NewServer(http.HandlerFunc(f.serve))
	t.Cleanup(f.Close)
	return f
}

func routeKey(method, path string) string { return method + " " + path }

// on 注册一条路由，每次调用按顺序取一个响应；用完后重复最后一个。
func (f *fakeFeishu) on(method, path string, responses ...func(w http.ResponseWriter)) *route {
	f.mu.Lock()
	defer f.mu.Unlock()
	r := &route{mu: &f.mu, responses: responses}
	f.routes[routeKey(method, path)] = r
	return r
}

// onJSON 注册一条固定返回同一份 JSON 的路由。
func (f *fakeFeishu) onJSON(method, path string, status int, body any) *route {
	return f.on(method, path, jsonResponse(status, body))
}

func jsonResponse(status int, body any) func(http.ResponseWriter) {
	encoded, err := json.Marshal(body)
	if err != nil {
		panic(err)
	}
	return func(w http.ResponseWriter) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		_, _ = w.Write(encoded)
	}
}

func rawResponseBody(status int, contentType string, body []byte) func(http.ResponseWriter) {
	return func(w http.ResponseWriter) {
		w.Header().Set("Content-Type", contentType)
		w.WriteHeader(status)
		_, _ = w.Write(body)
	}
}

// mockToken 注册 tenant_access_token 的默认成功响应。
func (f *fakeFeishu) mockToken() *route {
	return f.onJSON(http.MethodPost, PathTenantToken, 200, map[string]any{
		"code": 0, "msg": "ok", "tenant_access_token": "t-fake", "expire": 7200,
	})
}

func (f *fakeFeishu) serve(w http.ResponseWriter, req *http.Request) {
	body, _ := io.ReadAll(req.Body)
	rec := recordedRequest{
		method: req.Method,
		path:   req.URL.Path,
		query:  req.URL.Query(),
		body:   body,
		header: req.Header.Clone(),
	}

	f.mu.Lock()
	r, ok := f.routes[routeKey(req.Method, req.URL.Path)]
	if !ok {
		f.unmatched = append(f.unmatched, rec)
		f.mu.Unlock()
		w.WriteHeader(http.StatusNotFound)
		_, _ = w.Write([]byte(`{"code":404,"msg":"no route in fake feishu"}`))
		return
	}
	r.calls = append(r.calls, rec)
	idx := len(r.calls) - 1
	if idx >= len(r.responses) {
		idx = len(r.responses) - 1
	}
	respond := r.responses[idx]
	f.mu.Unlock()
	respond(w)
}

func (r *route) count() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.calls)
}

func (r *route) called() bool { return r.count() > 0 }

func (r *route) last(t *testing.T) recordedRequest {
	t.Helper()
	r.mu.Lock()
	defer r.mu.Unlock()
	if len(r.calls) == 0 {
		t.Fatal("这条路由一次都没被调用")
	}
	return r.calls[len(r.calls)-1]
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

// ---------------------------------------------------------------------------
// 造 Platform
// ---------------------------------------------------------------------------

type platformBuild struct {
	domain     string
	ratePerMin int
	clock      clockFunc
	sleep      sleeperFunc
	logger     *slog.Logger
	caps       *pb.PlatformCapabilities
	factory    ConnectionFactory
	budget     time.Duration
	sink       EventSink
	botOpenID  string
	appID      string
	httpClient *http.Client
}

func mustPlatform(t *testing.T, b platformBuild) *Platform {
	t.Helper()
	if b.botOpenID == "" {
		b.botOpenID = testBotOpenID
	}
	if b.ratePerMin == 0 {
		b.ratePerMin = 60
	}
	opts := Options{
		AppID:     b.appID,
		AppSecret: "secret",
		BotOpenID: b.botOpenID,
		TenantID:  "default",
		Domain:    b.domain,
	}
	po := platformOptions{
		opts:    opts,
		sink:    b.sink,
		caps:    b.caps,
		factory: b.factory,
		sleep:   b.sleep,
		clock:   b.clock,
		logger:  b.logger,
		budget:  b.budget,
	}
	if b.domain != "" {
		api, err := newAPIClient(apiOptions{
			appID:      opts.AppID,
			appSecret:  opts.AppSecret,
			domain:     b.domain,
			ratePerMin: b.ratePerMin,
			sleep:      b.sleep,
			clock:      b.clock,
			logger:     b.logger,
			httpClient: b.httpClient,
		})
		if err != nil {
			t.Fatalf("建 apiClient 失败：%v", err)
		}
		po.api = api
	}
	p, err := newPlatform(po)
	if err != nil {
		t.Fatalf("建 Platform 失败：%v", err)
	}
	return p
}

// outboundPlatform 是出站测试的标准装配：假服务器 + 不该被踩到的退避。
func outboundPlatform(t *testing.T, f *fakeFeishu) (*Platform, *fakeClock) {
	t.Helper()
	clock := newFakeClock()
	_, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{
		domain: f.URL, appID: testAppID,
		clock: clock.Now, sleep: clock.Sleep, logger: logger,
	})
	return p, clock
}

// ---------------------------------------------------------------------------
// fixture
// ---------------------------------------------------------------------------

func fixtureNames(t *testing.T) []string {
	t.Helper()
	entries, err := filepath.Glob(filepath.Join(testFixturesDir, "*.json"))
	if err != nil {
		t.Fatalf("列 fixture 失败：%v", err)
	}
	var names []string
	for _, p := range entries {
		base := filepath.Base(p)
		if len(base) > len(".expected.json") && base[len(base)-len(".expected.json"):] == ".expected.json" {
			continue
		}
		names = append(names, base[:len(base)-len(".json")])
	}
	return names
}

func loadFixture(t *testing.T, name, suffix string) map[string]any {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(testFixturesDir, name+suffix))
	if err != nil {
		t.Fatalf("读 fixture 失败：%v", err)
	}
	var out map[string]any
	if err := json.Unmarshal(data, &out); err != nil {
		t.Fatalf("解析 fixture 失败：%v", err)
	}
	return out
}

// canonicalJSON 把 protojson 的输出规范成稳定字节串。
//
// protojson 故意在输出里注入不稳定空白（防止调用方依赖字节级稳定），
// 先 Compact 抹掉它、再 Indent 成两格缩进，得到的才是能逐字节比对的形态。
func canonicalJSON(raw []byte) ([]byte, error) {
	var compact bytes.Buffer
	if err := json.Compact(&compact, raw); err != nil {
		return nil, err
	}
	var out bytes.Buffer
	if err := json.Indent(&out, compact.Bytes(), "", "  "); err != nil {
		return nil, err
	}
	out.WriteByte('\n')
	return out.Bytes(), nil
}

// ---------------------------------------------------------------------------
// 卡片样例（对应 conftest.sample_card）
// ---------------------------------------------------------------------------

func sampleCard() *pb.ChecklistCard {
	note := "共 3 个 sheet"
	return &pb.ChecklistCard{
		TaskId:    "b7c1e6f0-1111-4222-8333-444455556666",
		TaskNo:    "#A17",
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

// defaultFeishuConfig 是 config 契约的默认 feishu 段。
func defaultFeishuConfig() config.Feishu { return config.Default().Feishu }

// strptr 是 optional string 字段的取址助手。
func strptr(s string) *string { return &s }
