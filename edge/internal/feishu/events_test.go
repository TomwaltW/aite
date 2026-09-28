// 对应 tests/adapters/feishu/test_feishu_card_frames.py（T16 专项）。
//
// 卡片回传帧在长连接上到底能不能到我们手里 —— Go SDK 的结论要实测，不能照抄
// Python 那一份（Python SDK 1.7.3 的 ws/client.py 把 MessageType.CARD 那一支写成
// 一句 return，帧收到了然后被丢掉，!stop / 证据按钮点了没反应）。
//
// 这里不造假、不打桩 SDK：起一个真的 websocket 服务端，让 larkws.Client 走完
// bootstrap → 握手 → 收帧的全程，分别喂 type=card 与 type=event 两种帧，
// 量 handler 到底被调了几次。结论见回执。
package feishu

import (
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gorilla/websocket"
	larkevent "github.com/larksuite/oapi-sdk-go/v3/event"
	"github.com/larksuite/oapi-sdk-go/v3/event/dispatcher"
	larkws "github.com/larksuite/oapi-sdk-go/v3/ws"

	pb "aite/edge/gen/aitepb"
)

var cardActionPayload = map[string]any{
	"schema": "2.0",
	"header": map[string]any{
		"event_id":    "evt_card_frame_0001",
		"create_time": "1788916020000000",
		"event_type":  "card.action.trigger",
		"tenant_key":  "tk_p0_demo",
		"app_id":      testAppID,
		// 校验令牌：不该跟着信封进审计。
		"token": "v-header-verification-token",
	},
	"event": map[string]any{
		"operator": map[string]any{"open_id": "ou_zhang_san_00000000000000000001"},
		"action": map[string]any{
			"tag":   "button",
			"value": map[string]any{"action": "stop", "task_id": "t-1"},
		},
		"context": map[string]any{
			"open_message_id": testCardMsgID,
			"open_chat_id":    testChatID,
		},
		"token": "c-3f0a1b2c3d4e5f60718293a4b5c6d7e8",
	},
}

var messagePayload = map[string]any{
	"schema": "2.0",
	"header": map[string]any{
		"event_id":    "evt_msg_0001",
		"create_time": "1788915720000",
		"event_type":  "im.message.receive_v1",
		"app_id":      testAppID,
	},
	"event": map[string]any{
		"message": map[string]any{"message_id": "om_x", "chat_id": "oc_x"},
	},
}

// ---------------------------------------------------------------------------
// 假飞书长连接服务端
// ---------------------------------------------------------------------------

type fakeWS struct {
	*httptest.Server
	t     *testing.T
	up    websocket.Upgrader
	mu    sync.Mutex
	conn  *websocket.Conn
	ready chan struct{}
	once  sync.Once
}

func newFakeWS(t *testing.T) *fakeWS {
	t.Helper()
	s := &fakeWS{t: t, ready: make(chan struct{})}
	mux := http.NewServeMux()
	mux.HandleFunc("/callback/ws/endpoint", s.bootstrap)
	mux.HandleFunc("/ws", s.upgrade)
	s.Server = httptest.NewServer(mux)
	t.Cleanup(func() {
		s.mu.Lock()
		if s.conn != nil {
			_ = s.conn.Close()
		}
		s.mu.Unlock()
		s.Close()
	})
	return s
}

// bootstrap 是 SDK 建连第一步：拿 websocket 地址与客户端配置。
func (s *fakeWS) bootstrap(w http.ResponseWriter, _ *http.Request) {
	wsURL := strings.Replace(s.Server.URL, "http://", "ws://", 1) + "/ws?device_id=dev-1&service_id=1"
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(larkws.EndpointResp{
		Code: 0,
		Data: &larkws.Endpoint{
			Url: wsURL,
			ClientConfig: &larkws.ClientConfig{
				ReconnectCount: -1, ReconnectInterval: 120, ReconnectNonce: 30, PingInterval: 120,
			},
		},
	})
}

func (s *fakeWS) upgrade(w http.ResponseWriter, r *http.Request) {
	conn, err := s.up.Upgrade(w, r, nil)
	if err != nil {
		s.t.Errorf("websocket 升级失败：%v", err)
		return
	}
	s.mu.Lock()
	s.conn = conn
	s.mu.Unlock()
	s.once.Do(func() { close(s.ready) })
	// 客户端处理完每一帧会回写一帧响应，读掉它，否则连接会堵住。
	for {
		if _, _, err := conn.ReadMessage(); err != nil {
			return
		}
	}
}

// send 往长连接上推一帧数据。
func (s *fakeWS) send(messageType string, payload map[string]any) error {
	body, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	frame := &larkws.Frame{
		SeqID: 0, LogID: 0, Service: 1,
		Method: int32(larkws.FrameTypeData),
		// 这五个头一个都不能少：SDK 会逐个读出来。
		Headers: larkws.Headers{
			{Key: larkws.HeaderType, Value: messageType},
			{Key: larkws.HeaderMessageID, Value: "msg-1"},
			{Key: larkws.HeaderTraceID, Value: "trace-1"},
			{Key: larkws.HeaderSum, Value: "1"},
			{Key: larkws.HeaderSeq, Value: "0"},
		},
		Payload: body,
	}
	data, err := frame.Marshal()
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.conn.WriteMessage(websocket.BinaryMessage, data)
}

// wsHarness 起一条真长连接，返回收到的信封与关闭函数。
func wsHarness(t *testing.T) (*fakeWS, *[]map[string]any, *sync.Mutex, func()) {
	t.Helper()
	server := newFakeWS(t)

	var mu sync.Mutex
	received := []map[string]any{}
	_, logger := newLogCapture()
	conn := newLarkConnection("cli_test", "secret", server.Server.URL,
		func(_ context.Context, raw map[string]any) error {
			mu.Lock()
			received = append(received, raw)
			mu.Unlock()
			return nil
		}, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	if err := conn.Connect(ctx); err != nil {
		cancel()
		t.Fatalf("连不上假飞书长连接：%v", err)
	}
	select {
	case <-server.ready:
	case <-time.After(5 * time.Second):
		cancel()
		t.Fatal("服务端没等到握手")
	}
	return server, &received, &mu, func() { _ = conn.Close(); cancel() }
}

// settle 让投递跑完。收帧是在 SDK 自己的 goroutine 里做的，没有回调可等。
func settle(mu *sync.Mutex, got *[]map[string]any, want int) {
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		mu.Lock()
		n := len(*got)
		mu.Unlock()
		if n >= want {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	// 再多等一小会，好让「本不该来的那一帧」有机会露头。
	time.Sleep(200 * time.Millisecond)
}

// ---------------------------------------------------------------------------
// 实测：Go SDK 在长连接上怎么对待 card 帧
// ---------------------------------------------------------------------------

// TestGoSDKDropsCardFramesOnTheWire
// 对应 test_sdk_alone_drops_card_frames。
//
// 实测结论：oapi-sdk-go v3.12.0 的 ws/client_message.go::handleDataFrame 里那句
//
//	if MessageType(messageType) != MessageTypeEvent || c.eventHandler == nil { return }
//
// 把 type=card 的帧整条丢掉 —— 和 Python SDK 1.7.3 是同一个病。差别在于 Python
// 能 monkeypatch _handle_data_frame 把 CARD 帧改写成 EVENT 帧交回去，Go 这边
// handleDataFrame 与 eventHandler 都是私有的，没有同等的接法。
//
// 这条红了就说明 SDK 修好了（或平台改用 event 帧发卡片回传），那是好消息。
func TestGoSDKDropsCardFramesOnTheWire(t *testing.T) {
	server, received, mu, done := wsHarness(t)
	defer done()

	if err := server.send(string(larkws.MessageTypeCard), cardActionPayload); err != nil {
		t.Fatalf("发帧失败：%v", err)
	}
	settle(mu, received, 1)

	mu.Lock()
	got := len(*received)
	mu.Unlock()
	if got != 0 {
		t.Errorf("SDK 突然会转发 card 帧了？收到 %d 条 —— 那是好消息，"+
			"把回执里「卡片回传要真机复验」那条结论改掉", got)
	}
}

// TestCardActionTriggerReachesTheHandlerOnAnEventFrame
// 对应 test_card_frames_reach_the_handler。
//
// 注册链路本身是通的：只要帧的 type 头是 event，card.action.trigger 就能一路走到
// OnP2CardActionTrigger → 我们的 onRaw，内容原样、信封不带 token。
func TestCardActionTriggerReachesTheHandlerOnAnEventFrame(t *testing.T) {
	server, received, mu, done := wsHarness(t)
	defer done()

	if err := server.send(string(larkws.MessageTypeEvent), cardActionPayload); err != nil {
		t.Fatalf("发帧失败：%v", err)
	}
	settle(mu, received, 1)

	mu.Lock()
	defer mu.Unlock()
	if len(*received) != 1 {
		t.Fatalf("卡片回传该到 onRaw，收到 %d 条", len(*received))
	}
	envelope := (*received)[0]
	header := asMap(envelope["header"])
	if mapStr(header, "event_type") != "card.action.trigger" {
		t.Errorf("event_type = %q", mapStr(header, "event_type"))
	}
	if mapStr(header, "event_id") != "evt_card_frame_0001" {
		t.Errorf("event_id = %q", mapStr(header, "event_id"))
	}
	event := asMap(envelope["event"])
	if got := mapStr(asMap(asMap(event["action"])["value"]), "action"); got != "stop" {
		t.Errorf("action = %q", got)
	}
	if got := mapStr(asMap(event["context"]), "open_message_id"); got != testCardMsgID {
		t.Errorf("open_message_id = %q", got)
	}
	// 校验令牌不该跟着进审计。
	if _, ok := header["token"]; ok {
		t.Error("header.token 不该进信封")
	}
	if mapStr(envelope, "schema") != "2.0" {
		t.Errorf("schema = %q", mapStr(envelope, "schema"))
	}
}

// TestEventFramesAreDelivered 对应 test_event_frames_are_untouched_by_the_shim。
//
// 普通消息事件走原路，行为一个字不变。
func TestEventFramesAreDelivered(t *testing.T) {
	server, received, mu, done := wsHarness(t)
	defer done()

	if err := server.send(string(larkws.MessageTypeEvent), messagePayload); err != nil {
		t.Fatalf("发帧失败：%v", err)
	}
	settle(mu, received, 1)

	mu.Lock()
	defer mu.Unlock()
	if len(*received) != 1 {
		t.Fatalf("消息事件该到 onRaw，收到 %d 条", len(*received))
	}
	if got := mapStr(asMap((*received)[0]["header"]), "event_type"); got != "im.message.receive_v1" {
		t.Errorf("event_type = %q", got)
	}
}

// TestCardActionTriggerIsRegisteredOnTheDispatcher 不经过长连接，
// 单独钉住「注册的是 callback 表而不是 event 表」。
//
// dispatcher 内部分两张表，Do() 先查 callbackType2CallbackHandler 再查
// eventType2EventHandler；卡片回传只认前者，OnCustomizedEvent 注册不进去。
func TestCardActionTriggerIsRegisteredOnTheDispatcher(t *testing.T) {
	var got []map[string]any
	_, logger := newLogCapture()
	conn := newLarkConnection("cli_test", "secret", DefaultDomain,
		func(_ context.Context, raw map[string]any) error {
			got = append(got, raw)
			return nil
		}, logger)

	payload, err := json.Marshal(cardActionPayload)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := conn.buildDispatcher().Do(context.Background(), payload); err != nil {
		t.Fatalf("dispatcher 没认出 card.action.trigger：%v", err)
	}
	if len(got) != 1 {
		t.Fatalf("该投递 1 条，得到 %d 条", len(got))
	}
	if _, ok := asMap(got[0]["header"])["token"]; ok {
		t.Error("header.token 不该进信封")
	}

	// 原来这里写的是「反证：只注册自定义事件的 dispatcher 认不出卡片回传」，判据是
	// `if err != nil { t.Fatal }` —— 方向反了，而且永远不会失败：SDK 的 dispatcher.Do
	// 查不到 callback handler 会继续落到 eventType2EventHandler，而 OnCustomizedEvent
	// 正是往后者注册的，所以它返回的是 (nil, nil)。那条 if 是个空断言。
	//
	// 真正要钉的是**两条路拿到的东西不是一回事**：卡片回传只有走 callback 那条
	// 才能解析成 CardActionTriggerEvent，OnCustomizedEvent 拿到的是未解析的 EventReq。
	var bareGot []*larkevent.EventReq
	bare := dispatcher.NewEventDispatcher("", "")
	bare.OnCustomizedEvent(eventCardAction, func(_ context.Context, req *larkevent.EventReq) error {
		bareGot = append(bareGot, req)
		return nil
	})
	if _, err := bare.Do(context.Background(), payload); err != nil {
		t.Fatalf("OnCustomizedEvent 这条路本身应该跑得通：%v", err)
	}
	if len(bareGot) != 1 {
		t.Fatalf("OnCustomizedEvent 该收到 1 条原始 EventReq，得到 %d 条", len(bareGot))
	}
	if len(got) != 1 {
		t.Fatalf("callback 那条路仍该只有 1 条，得到 %d 条", len(got))
	}
}

// TestEnvelopeShapeMatchesPython 钉住信封形状照 Python：
// {schema, header{5 个字段}, event}，不多不少。
func TestEnvelopeShapeMatchesPython(t *testing.T) {
	envelope := envelopeOf(cardActionPayload)
	if len(envelope) != 3 {
		t.Errorf("信封顶层该只有 schema/header/event，得到 %v", keysOf(envelope))
	}
	for _, key := range []string{"schema", "header", "event"} {
		if _, ok := envelope[key]; !ok {
			t.Errorf("信封缺 %s", key)
		}
	}
	header := asMap(envelope["header"])
	want := map[string]bool{
		"event_id": true, "create_time": true, "event_type": true,
		"tenant_key": true, "app_id": true,
	}
	if len(header) != len(want) {
		t.Errorf("header 字段 = %v，要 %v", keysOf(header), keysOf(anyMap(want)))
	}
	for key := range want {
		if _, ok := header[key]; !ok {
			t.Errorf("header 缺 %s", key)
		}
	}

	// 缺 schema 时兜底成 "2.0"。
	noSchema := map[string]any{"header": map[string]any{}, "event": map[string]any{}}
	if got := mapStr(envelopeOf(noSchema), "schema"); got != "2.0" {
		t.Errorf("缺 schema 时要兜底成 2.0，得到 %q", got)
	}
	// header 里没有的字段留 nil（与 Python 的 getattr(..., None) 同口径）。
	if v, ok := asMap(envelopeOf(noSchema)["header"])["tenant_key"]; !ok || v != nil {
		t.Errorf("缺失的 header 字段要留 nil，得到 %v", v)
	}
}

func keysOf(m map[string]any) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	return out
}

func anyMap(m map[string]bool) map[string]any {
	out := make(map[string]any, len(m))
	for k, v := range m {
		out[k] = v
	}
	return out
}

// ---------------------------------------------------------------------------
// 事件投递
// ---------------------------------------------------------------------------

// TestRawEventsAreNormalizedAndDelivered
// 对应 test_raw_events_are_normalized_and_delivered。
func TestRawEventsAreNormalizedAndDelivered(t *testing.T) {
	sink := &recordingSink{}
	p, _ := dispatchPlatform(t, sink, 0, nil)

	if err := p.dispatchRaw(context.Background(),
		loadFixture(t, "message_at_bot_toplevel", ".json")); err != nil {
		t.Fatalf("dispatchRaw 失败：%v", err)
	}

	seen := sink.seen()
	if len(seen) != 1 {
		t.Fatalf("事件数 = %d，要 1", len(seen))
	}
	if got := seen[0].GetText(); got != "把这个季度的销售数据画成趋势图" {
		t.Errorf("text = %q", got)
	}
	if !seen[0].GetMentioned() {
		t.Error("mentioned 要是 true")
	}
	if got := seen[0].GetWorkspaceId(); got != testAppID {
		t.Errorf("workspace_id = %q", got)
	}
}

// TestAdapterDoesNotDeduplicateReplayedEvents
// 对应 test_adapter_does_not_deduplicate_replayed_events。
//
// 重连后平台重推的重复事件由 core 靠 event_id 去重，adapter 不管。
func TestAdapterDoesNotDeduplicateReplayedEvents(t *testing.T) {
	sink := &recordingSink{}
	p, _ := dispatchPlatform(t, sink, 0, nil)

	raw := loadFixture(t, "message_at_bot_toplevel", ".json")
	for i := 0; i < 2; i++ {
		if err := p.dispatchRaw(context.Background(), raw); err != nil {
			t.Fatalf("第 %d 次 dispatchRaw 失败：%v", i+1, err)
		}
	}

	seen := sink.seen()
	if len(seen) != 2 {
		t.Fatalf("事件数 = %d，要 2（adapter 私自去重的话 core 的 events.duplicate 就永远是 0）", len(seen))
	}
	if seen[0].GetEventId() != seen[1].GetEventId() {
		t.Error("两条的 event_id 该相同")
	}
}

// TestUnsubscribedEventIsDroppedWithoutCallingHandler
// 对应 test_unsubscribed_event_is_dropped_without_calling_handler。
func TestUnsubscribedEventIsDroppedWithoutCallingHandler(t *testing.T) {
	sink := &recordingSink{}
	p, capture := dispatchPlatform(t, sink, 0, nil)

	raw := loadFixture(t, "message_at_bot_toplevel", ".json")
	asMap(raw["header"])["event_type"] = "im.chat.member.user.added_v1"
	if err := p.dispatchRaw(context.Background(), raw); err != nil {
		t.Fatalf("没订阅的事件不该报错：%v", err)
	}

	if got := sink.seen(); len(got) != 0 {
		t.Errorf("没订阅的事件不该进 sink，得到 %d 条", len(got))
	}
	if !capture.has("feishu.event_ignored") {
		t.Error("该打一条 feishu.event_ignored")
	}
}

// TestHandlerFailureDoesNotKillTheConnection
// 对应 test_handler_exception_does_not_kill_the_connection。
//
// 与 Python 的一处有意差异：Python 把回调异常吞掉只打日志；Go 版把 error 返回给
// SDK 让平台重推（spec §2.1）。「不带走长连接」这条不变 —— Start 的循环照转。
func TestHandlerFailureDoesNotKillTheConnection(t *testing.T) {
	boom := errors.New("上游炸了")
	sink := &recordingSink{err: boom}
	p, capture := dispatchPlatform(t, sink, 0, nil)

	err := p.dispatchRaw(context.Background(), loadFixture(t, "message_at_bot_toplevel", ".json"))
	if !errors.Is(err, boom) {
		t.Errorf("HandleEvent 失败要一路返回给 SDK（让平台重推），得到 %v", err)
	}

	records := capture.find("feishu.on_event_failed")
	if len(records) != 1 {
		t.Fatalf("该打一条 feishu.on_event_failed，得到 %d 条", len(records))
	}
	if records[0].Level != slog.LevelError {
		t.Errorf("feishu.on_event_failed 的级别 = %v，要 ERROR", records[0].Level)
	}
	if v, ok := attr(records[0], "event_id"); !ok || v.String() != "evt_at_bot_toplevel_0001" {
		t.Errorf("event_id = %v", v.Any())
	}

	// 长连接不受影响：投递一路失败，Start 的重连循环照转到 ctx 取消为止。
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	sleep := &recordingSleep{stopAfter: 3, cancel: cancel}
	cs := &connectionScript{script: []string{"ok", "ok", "ok"}}
	_, logger := newLogCapture()
	loopP := mustPlatform(t, platformBuild{
		factory: cs.factory, sleep: sleep.Sleep, logger: logger, sink: sink, appID: testAppID,
	})
	raw := loadFixture(t, "message_at_bot_toplevel", ".json")
	sleep.onSleep = func(int) {
		for _, c := range cs.all() {
			if c.onRaw != nil {
				_ = c.onRaw(context.Background(), raw)
			}
		}
	}
	if err := loopP.Start(ctx); err != nil {
		t.Fatalf("投递一直失败也不该让 Start 抛出去：%v", err)
	}
	if len(sink.seen()) < 3 {
		t.Errorf("投递该一直在发生，得到 %d 条", len(sink.seen()))
	}
}

// TestSlowHandlerIsReported 对应 test_slow_handler_is_reported。
//
// HandleEvent 必须 1s 内返回。超了不拦（拦了会丢事件），但要吼一声。
func TestSlowHandlerIsReported(t *testing.T) {
	clock := newFakeClock()
	sink := &recordingSink{before: func() { clock.Advance(2 * time.Second) }}
	p, capture := dispatchPlatform(t, sink, time.Second, clock)

	if err := p.dispatchRaw(context.Background(),
		loadFixture(t, "message_at_bot_toplevel", ".json")); err != nil {
		t.Fatalf("慢不等于失败：%v", err)
	}

	records := capture.find("feishu.on_event_slow")
	if len(records) != 1 {
		t.Fatalf("该打一条 feishu.on_event_slow，得到 %d 条", len(records))
	}
	if records[0].Level != slog.LevelWarn {
		t.Errorf("feishu.on_event_slow 的级别 = %v，要 WARN", records[0].Level)
	}
	if v, ok := attr(records[0], "elapsed_sec"); !ok || v.Float64() != 2 {
		t.Errorf("elapsed_sec = %v，要 2", v.Any())
	}
	if v, ok := attr(records[0], "budget_sec"); !ok || v.Float64() != 1 {
		t.Errorf("budget_sec = %v，要 1", v.Any())
	}

	// 没超时就不打。
	clock2 := newFakeClock()
	sink2 := &recordingSink{}
	p2, capture2 := dispatchPlatform(t, sink2, time.Second, clock2)
	if err := p2.dispatchRaw(context.Background(),
		loadFixture(t, "message_at_bot_toplevel", ".json")); err != nil {
		t.Fatal(err)
	}
	if capture2.has("feishu.on_event_slow") {
		t.Error("没超预算不该打 feishu.on_event_slow")
	}
}

// ---------------------------------------------------------------------------
// 事件分发表
// ---------------------------------------------------------------------------

// sampleEnvelopes 给分发表里的每一类事件备一份样例信封。表里多了一类、这里没跟上，
// TestHandlerTableDrivesDispatcherAndNormalize 会红 —— 逼着新事件带着样例进来。
func sampleEnvelopes(t *testing.T) map[string]map[string]any {
	t.Helper()
	return map[string]map[string]any{
		eventMessageReceive: loadFixture(t, "message_at_bot_toplevel", ".json"),
		eventCardAction:     cardActionPayload,
	}
}

// droppedByDesign 是表里登记了、但归一化后故意不上送的事件类型。
var droppedByDesign = map[string]bool{}

// dispatchThroughSDK 把一份信封经 buildDispatcher().Do 投一遍，返回 onRaw 收到的信封。
func dispatchThroughSDK(t *testing.T, envelope map[string]any) ([]map[string]any, error) {
	t.Helper()
	var got []map[string]any
	_, logger := newLogCapture()
	conn := newLarkConnection("cli_test", "secret", DefaultDomain,
		func(_ context.Context, raw map[string]any) error {
			got = append(got, raw)
			return nil
		}, logger)
	payload, err := json.Marshal(envelope)
	if err != nil {
		t.Fatal(err)
	}
	_, err = conn.buildDispatcher().Do(context.Background(), payload)
	return got, err
}

// TestHandlerTableDrivesDispatcherAndNormalize 钉住分发表是唯一的注册面：
// 表里每一项都经 buildDispatcher().Do 到 onRaw、且被 Normalize 认；
// 临时登记一个假事件类型，不改 events.go 就能被投递和归一化（EE11 要的接口）。
func TestHandlerTableDrivesDispatcherAndNormalize(t *testing.T) {
	samples := sampleEnvelopes(t)
	for eventType := range eventTableSnapshot() {
		t.Run(eventType, func(t *testing.T) {
			envelope, ok := samples[eventType]
			if !ok {
				t.Fatalf("分发表里有 %s，但 sampleEnvelopes 没给样例", eventType)
			}
			got, err := dispatchThroughSDK(t, envelope)
			if err != nil {
				t.Fatalf("buildDispatcher().Do 没认出 %s：%v", eventType, err)
			}
			if len(got) != 1 {
				t.Fatalf("该投递 1 条，得到 %d 条", len(got))
			}
			event := Normalize(got[0], testBotOpenID, testAppID, "default")
			if droppedByDesign[eventType] {
				if event != nil {
					t.Errorf("%s 该被归一化成 nil（暂不上送），得到 %v", eventType, event)
				}
				return
			}
			if event == nil {
				t.Fatalf("Normalize 没认出 %s", eventType)
			}
		})
	}

	const fakeType = "test.fake_event_v1"
	fakeEnvelope := map[string]any{
		"schema": "2.0",
		"header": map[string]any{"event_id": "evt_fake_0001", "event_type": fakeType, "app_id": testAppID},
		"event":  map[string]any{"chat_id": testChatID},
	}
	if _, err := dispatchThroughSDK(t, fakeEnvelope); err == nil {
		t.Fatal("没登记的类型 Do 该报 NotFound —— 否则下面那半证明不了什么")
	}

	registerTestEvent(t, fakeType, eventEntry{
		normalize: func(raw map[string]any, _, workspaceID, tenantID string) *pb.NormalizedEvent {
			return &pb.NormalizedEvent{
				EventId: mapStr(asMap(raw["header"]), "event_id"),
				Kind:    pb.EventKind_EVENT_KIND_MESSAGE,
				ChatId:  mapStr(asMap(raw["event"]), "chat_id"),
			}
		},
	})
	got, err := dispatchThroughSDK(t, fakeEnvelope)
	if err != nil {
		t.Fatalf("登记之后 Do 该认出假事件：%v", err)
	}
	if len(got) != 1 {
		t.Fatalf("假事件该投递 1 条，得到 %d 条", len(got))
	}
	event := Normalize(got[0], testBotOpenID, testAppID, "default")
	if event.GetEventId() != "evt_fake_0001" || event.GetChatId() != testChatID {
		t.Errorf("假事件归一化结果 = %v", event)
	}

	sink := &recordingSink{}
	p, _ := dispatchPlatform(t, sink, 0, nil)
	if err := p.dispatchRaw(context.Background(), got[0]); err != nil {
		t.Fatalf("dispatchRaw 失败：%v", err)
	}
	if len(sink.seen()) != 1 {
		t.Errorf("假事件该进 sink，得到 %d 条", len(sink.seen()))
	}
}

// TestDuplicateEventRegistrationPanics 钉住重复登记会炸。
func TestDuplicateEventRegistrationPanics(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Error("重复登记 im.message.receive_v1 该 panic")
		}
	}()
	registerEvent(eventMessageReceive, eventEntry{normalize: NormalizeMessage})
}
