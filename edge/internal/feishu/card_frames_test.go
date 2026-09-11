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

	// 反证：只注册自定义事件的 dispatcher 认不出卡片回传。
	bare := dispatcher.NewEventDispatcher("", "")
	bare.OnCustomizedEvent(eventCardAction, func(context.Context, *larkevent.EventReq) error { return nil })
	if _, err := bare.Do(context.Background(), payload); err != nil {
		t.Fatalf("OnCustomizedEvent 也能接住卡片回传？那就有第二条路了：%v", err)
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
