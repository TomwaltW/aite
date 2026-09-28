package dingtalk

import (
	"reflect"
	"testing"
	"time"
)

// 钉：open 体里的三条订阅；ws 请求带 ticket；机器人消息 CALLBACK → sink 收到归一化事件、
// 假服务端收到 code 200 + 同一 messageId 的 ACK；SYSTEM ping 同样回显（data 原样）。
func TestStreamOpenConnectCallbackAckEchoesMessageID(t *testing.T) {
	f := newFakeDingtalk(t)
	sink := newRecordingSink()
	p := mustPlatform(t, platformBuild{apiBase: f.URL, sink: sink})
	startPlatform(t, p)

	conn := f.nextConn(t)
	eventually(t, "Connected()=true", p.Connected)

	opens := f.openBodies()
	if len(opens) != 1 {
		t.Fatalf("open 调了 %d 次，期望 1", len(opens))
	}
	open := opens[0]
	if open["clientId"] != testClientID || open["clientSecret"] != testClientSecret {
		t.Errorf("open 凭证不对：%v / %v", open["clientId"], open["clientSecret"])
	}
	wantSubs := []any{
		map[string]any{"type": "EVENT", "topic": "*"},
		map[string]any{"type": "CALLBACK", "topic": "/v1.0/im/bot/messages/get"},
		map[string]any{"type": "CALLBACK", "topic": "/v1.0/card/instances/callback"},
	}
	if !reflect.DeepEqual(open["subscriptions"], wantSubs) {
		t.Errorf("subscriptions = %#v\n期望 %#v", open["subscriptions"], wantSubs)
	}
	if conn.ticket != "ticket-1" || !reflect.DeepEqual(f.handshakeTickets(), []string{"ticket-1"}) {
		t.Errorf("ws 握手 ticket = %v，期望 [ticket-1]", f.handshakeTickets())
	}

	// 机器人消息 CALLBACK。
	conn.push(t, "CALLBACK", TopicBotMessage, "frame-msg-001", loadFixture(t, "text_group.json"))
	ev := sink.next(t)
	if ev.GetEventId() != "msgGROUPTEXT01" || ev.GetText() != "把 Q3 销售数据画成趋势图" {
		t.Errorf("sink 收到的事件不对：event_id=%q text=%q", ev.GetEventId(), ev.GetText())
	}
	ack := conn.waitAck(t, "frame-msg-001", 5*time.Second)
	if code, _ := ack["code"].(float64); code != 200 {
		t.Errorf("ACK code = %v，期望 200", ack["code"])
	}
	if ack["message"] != "OK" {
		t.Errorf("ACK message = %v", ack["message"])
	}
	headers, _ := ack["headers"].(map[string]any)
	if headers["contentType"] != "application/json" {
		t.Errorf("ACK contentType = %v", headers["contentType"])
	}

	// SYSTEM ping：当场 ACK，data 原样回显。
	pingData := `{"opaque":"ping-payload-42"}`
	conn.push(t, "SYSTEM", "ping", "frame-ping-001", pingData)
	pong := conn.waitAck(t, "frame-ping-001", 5*time.Second)
	if code, _ := pong["code"].(float64); code != 200 {
		t.Errorf("ping ACK code = %v", pong["code"])
	}
	if pong["data"] != pingData {
		t.Errorf("ping ACK data = %v，期望原样 %s", pong["data"], pingData)
	}

	// EVENT：ACK SUCCESS，不上送。
	conn.push(t, "EVENT", "chat_update_title", "frame-event-001", `{"x":1}`)
	evAck := conn.waitAck(t, "frame-event-001", 5*time.Second)
	if evAck["data"] != ackDataEventSuccess {
		t.Errorf("EVENT ACK data = %v", evAck["data"])
	}
	select {
	case extra := <-sink.got:
		t.Errorf("EVENT 帧不该上送：%v", extra)
	case <-time.After(50 * time.Millisecond):
	}

	// sink 出错 → 非 200 ACK（让平台重推）。
	sink.mu.Lock()
	sink.err = errSinkDown
	sink.mu.Unlock()
	conn.push(t, "CALLBACK", TopicBotMessage, "frame-msg-002", loadFixture(t, "text_p2p.json"))
	sink.next(t)
	failed := conn.waitAck(t, "frame-msg-002", 5*time.Second)
	if code, _ := failed["code"].(float64); code == 200 {
		t.Errorf("sink 出错时 ACK 仍是 200")
	}
}

// 钉：假网关每个 ticket 只放行一次、复用即 401；服务端发 disconnect 或直接断开后，
// 客户端重新 open、用新 ticket；Connected / ReconnectCount 跟着变；退避走注入的 sleep。
func TestReconnectReopensWithFreshTicket(t *testing.T) {
	f := newFakeDingtalk(t)
	clock := newFakeClock()
	p := mustPlatform(t, platformBuild{apiBase: f.URL, sink: newRecordingSink(), clock: clock})
	startPlatform(t, p)

	first := f.nextConn(t)
	eventually(t, "首连 Connected()=true", p.Connected)
	if p.ReconnectCount() != 0 {
		t.Fatalf("首连后 ReconnectCount = %d", p.ReconnectCount())
	}

	// 1) 服务端下发 disconnect：客户端 ACK 后关本连接，下一轮重新 open。
	first.push(t, "SYSTEM", "disconnect", "frame-disc-001", `{"reason":"rebalance"}`)
	first.waitAck(t, "frame-disc-001", 5*time.Second)
	second := f.nextConn(t)
	if second.ticket != "ticket-2" {
		t.Fatalf("disconnect 后用的 ticket = %q，期望新的 ticket-2", second.ticket)
	}
	eventually(t, "disconnect 后 ReconnectCount=1", func() bool { return p.ReconnectCount() == 1 })
	eventually(t, "重连后 Connected()=true", p.Connected)

	// 2) 服务端直接断开：Connected 变 false，退避后再 open 一次、用 ticket-3。
	_ = second.ws.Close()
	third := f.nextConn(t)
	if third.ticket != "ticket-3" {
		t.Fatalf("断开后用的 ticket = %q，期望新的 ticket-3", third.ticket)
	}
	eventually(t, "断开后 ReconnectCount=2", func() bool { return p.ReconnectCount() == 2 })
	eventually(t, "再次重连后 Connected()=true", p.Connected)

	if got := len(f.openBodies()); got != 3 {
		t.Errorf("open 调了 %d 次，期望 3（每轮一次）", got)
	}
	if got := f.handshakeTickets(); !reflect.DeepEqual(got, []string{"ticket-1", "ticket-2", "ticket-3"}) {
		t.Errorf("握手 ticket 序列 = %v", got)
	}
	f.mu.Lock()
	rejected := f.rejected
	f.mu.Unlock()
	if rejected != 0 {
		t.Errorf("有 %d 次握手被拒（复用了旧 ticket）", rejected)
	}
	// 两次断开各退避 1 秒（注入的 sleep，不吃墙钟）。
	if slept := clock.Slept(); !reflect.DeepEqual(slept, []time.Duration{time.Second, time.Second}) {
		t.Errorf("退避序列 = %v，期望 [1s 1s]", slept)
	}
}

// 钉：退避序列 1,2,4,8,16,30,30…（30 秒封顶）。
func TestBackoffDelaySequence(t *testing.T) {
	var got []time.Duration
	for i := 1; i <= 8; i++ {
		got = append(got, backoffDelay(i))
	}
	want := []time.Duration{1, 2, 4, 8, 16, 30, 30, 30}
	for i := range want {
		want[i] *= time.Second
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("backoff = %v，期望 %v", got, want)
	}
}
