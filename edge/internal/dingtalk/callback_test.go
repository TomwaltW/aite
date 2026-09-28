package dingtalk

import (
	"context"
	"net/http"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
)

// cardCallbackData 造一份卡片回调 data（content 是 JSON 字符串）。
func cardCallbackData(outTrackID, action, taskID string) map[string]any {
	return map[string]any{
		"outTrackId": outTrackID,
		"userId":     "staff_clicker",
		"corpId":     testCorpID,
		"type":       "actionCallback",
		"content":    `{"cardPrivateData":{"actionIds":["` + action + `"],"params":{"action":"` + action + `","task_id":"` + taskID + `"}}}`,
	}
}

// 钉：卡片回调在 sink 阻塞时 2 秒内 ACK；放开后事件字段逐字；未知动作有 ACK 无事件；
// 机器人消息的 sink 阻塞着时，卡片回调照样 2 秒内 ACK。
func TestCardCallbackAckedWithin2sWhileSinkBlocks(t *testing.T) {
	f := newFakeDingtalk(t)
	f.mockToken()
	f.onJSON(http.MethodPost, PathCardCreate, 200, map[string]any{"success": true})
	sink := newBlockingSink()
	clock := newFakeClock()
	p := mustPlatform(t, platformBuild{apiBase: f.URL, sink: sink, clock: clock, cardTemplateID: testTemplateID})
	p.sessions.put(testGroupChat, sessionInfo{conversationType: "2", staffID: testStaffID, corpID: testCorpID})
	res, err := p.SendCard(context.Background(), testGroupChat, nil, sampleCard())
	if err != nil {
		t.Fatalf("SendCard 失败：%v", err)
	}
	outTrack := res.GetMessageId()

	startPlatform(t, p)
	released := false
	release := func() {
		if !released {
			released = true
			close(sink.release)
		}
	}
	t.Cleanup(release)
	conn := f.nextConn(t)

	// 1) Stop：sink 阻塞着，ACK 先到。
	started := time.Now()
	conn.push(t, "CALLBACK", TopicCardCallback, "frame-card-001", cardCallbackData(outTrack, "stop", "task-123"))
	ack := conn.waitAck(t, "frame-card-001", 2*time.Second)
	if elapsed := time.Since(started); elapsed >= 2*time.Second {
		t.Fatalf("卡片回调 ACK 用了 %v，超过 2 秒", elapsed)
	}
	if code, _ := ack["code"].(float64); code != 200 {
		t.Errorf("卡片回调 ACK code = %v", ack["code"])
	}
	var ev *pb.NormalizedEvent
	select {
	case ev = <-sink.entered:
	case <-time.After(5 * time.Second):
		t.Fatal("sink 没收到卡片回调事件")
	}

	if ev.GetEventId() != "frame-card-001" || ev.GetKind() != pb.EventKind_EVENT_KIND_CARD_ACTION ||
		ev.GetPlatform() != "dingtalk" || ev.GetTenantId() != "default" {
		t.Errorf("事件头字段不对：%v", ev)
	}
	if ev.GetChatId() != testGroupChat || ev.GetChatType() != pb.ChatType_CHAT_TYPE_GROUP || ev.GetWorkspaceId() != testCorpID {
		t.Errorf("会话字段不对：chat=%q type=%v ws=%q", ev.GetChatId(), ev.GetChatType(), ev.GetWorkspaceId())
	}
	if ev.GetSenderId() != "staff_clicker" || ev.GetSenderKind() != pb.SenderKind_SENDER_KIND_HUMAN || !ev.GetMentioned() {
		t.Errorf("发送者字段不对：%v", ev)
	}
	ca := ev.GetCardAction()
	if ca.GetCardId() != outTrack || ca.GetAction() != pb.CardActionKind_CARD_ACTION_KIND_STOP || ca.TaskId == nil || *ca.TaskId != "task-123" {
		t.Errorf("CardAction = %v", ca)
	}
	a := ev.GetAnchor()
	if a == nil {
		t.Fatal("Anchor 为 nil（core 会拒收）")
	}
	if a.GetMessageId() != outTrack || a.GetPlatform() != "dingtalk" || a.GetChatId() != testGroupChat || a.ThreadId != nil {
		t.Errorf("Anchor = %v", a)
	}
	if !ev.GetOccurredAt().AsTime().Equal(clock.Now()) {
		t.Errorf("OccurredAt = %v，期望注入时钟 %v", ev.GetOccurredAt().AsTime(), clock.Now())
	}

	// 2) 未知动作：有 ACK、无事件。
	conn.push(t, "CALLBACK", TopicCardCallback, "frame-card-002", cardCallbackData(outTrack, "approve", "task-123"))
	conn.waitAck(t, "frame-card-002", 2*time.Second)
	select {
	case extra := <-sink.entered:
		t.Errorf("未知动作不该产生事件：%v", extra)
	case <-time.After(100 * time.Millisecond):
	}

	// 3) 先推一帧机器人消息、让它的 sink 阻塞住，再推卡片回调 → 卡片回调照样 < 2 s ACK。
	t.Run("card callback not blocked by slow bot message sink", func(t *testing.T) {
		conn.push(t, "CALLBACK", TopicBotMessage, "frame-msg-slow", loadFixture(t, "text_group.json"))
		select {
		case msg := <-sink.entered:
			if msg.GetKind() != pb.EventKind_EVENT_KIND_MESSAGE {
				t.Fatalf("期望先收到机器人消息，得到 %v", msg.GetKind())
			}
		case <-time.After(5 * time.Second):
			t.Fatal("sink 没收到机器人消息")
		}
		started := time.Now()
		conn.push(t, "CALLBACK", TopicCardCallback, "frame-card-003", cardCallbackData(outTrack, "stop", "task-456"))
		conn.waitAck(t, "frame-card-003", 2*time.Second)
		if elapsed := time.Since(started); elapsed >= 2*time.Second {
			t.Fatalf("机器人消息 sink 阻塞时卡片回调 ACK 用了 %v", elapsed)
		}
		// 机器人消息的 ACK 要等 sink 返回：放开前不该到。
		select {
		case early := <-conn.acks:
			if h, _ := early["headers"].(map[string]any); h["messageId"] == "frame-msg-slow" {
				t.Fatal("机器人消息在 sink 返回前就 ACK 了")
			}
		case <-time.After(50 * time.Millisecond):
		}
		release()
		conn.waitAck(t, "frame-msg-slow", 5*time.Second)
	})
}
