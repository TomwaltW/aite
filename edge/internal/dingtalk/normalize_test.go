package dingtalk

import (
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
)

type expectedEvent struct {
	eventID     string
	workspaceID string
	chatID      string
	chatType    pb.ChatType
	senderID    string
	senderName  string
	text        string
	rawText     *string
	mentioned   bool
	threadID    *string
	taskNo      *string
	attachments []string // FileKey 序列（都是 IMAGE、MessageId = eventID）
	occurredAt  time.Time
	quote       *Quote
}

// 钉：6 份夹具逐字段比期望；Raw 里没有 sessionWebhook。
func TestNormalizeFixtures(t *testing.T) {
	clock := newFakeClock()
	cases := map[string]expectedEvent{
		"text_group.json": {
			eventID: "msgGROUPTEXT01", workspaceID: "ding_corp_0001", chatID: "cidGROUP0001==",
			chatType: pb.ChatType_CHAT_TYPE_GROUP, senderID: "staff_0001", senderName: "张三",
			text: "把 Q3 销售数据画成趋势图", rawText: strptr("  @Aite 把 Q3 销售数据画成趋势图  "),
			mentioned: true, occurredAt: time.UnixMilli(1700000000123).UTC(),
		},
		"text_p2p.json": {
			// senderStaffId 空 → senderId；chatbotCorpId 空 → senderCorpId；createAt 缺 → 注入时钟。
			eventID: "msgP2PTEXT01", workspaceID: "ding_corp_0001", chatID: "cidP2P0001==",
			chatType: pb.ChatType_CHAT_TYPE_P2P, senderID: "$:LWCP_v1:$fakeSender02", senderName: "李四",
			text: "帮我看看 #ah 进展", rawText: strptr("帮我看看 #ah 进展"),
			mentioned: false, taskNo: strptr("#AH"), occurredAt: clock.Now(),
		},
		"rich_text.json": {
			eventID: "msgRICH01", workspaceID: "ding_corp_0001", chatID: "cidGROUP0001==",
			chatType: pb.ChatType_CHAT_TYPE_GROUP, senderID: "staff_0001", senderName: "张三",
			text: "看看这张图\n对不对", rawText: strptr("@Aite 看看这张图\n对不对"),
			mentioned: true, attachments: []string{"dlc_rich_01"}, occurredAt: time.UnixMilli(1700000100000).UTC(),
		},
		"picture.json": {
			// downloadCode 空 → pictureDownloadCode。
			eventID: "msgPIC01", workspaceID: "ding_corp_0001", chatID: "cidP2P0001==",
			chatType: pb.ChatType_CHAT_TYPE_P2P, senderID: "staff_0002", senderName: "李四",
			text: "", rawText: nil, mentioned: false, attachments: []string{"pdc_pic_01"},
			occurredAt: time.UnixMilli(1700000200000).UTC(),
		},
		"quote_reply.json": {
			eventID: "msgQUOTE01", workspaceID: "ding_corp_0001", chatID: "cidGROUP0001==",
			chatType: pb.ChatType_CHAT_TYPE_GROUP, senderID: "staff_0001", senderName: "张三",
			text: "继续，加上同比", rawText: strptr("@Aite 继续，加上同比"), mentioned: true,
			threadID: strptr("msgROOT0001"), taskNo: strptr("#AH"),
			occurredAt: time.UnixMilli(1700000300000).UTC(),
			quote:      &Quote{MessageID: "msgROOT0001", SenderID: "$:LWCP_v1:$fakeBot", Text: "#AH 已开始：把 Q3 销售数据画成趋势图"},
		},
		"quote_reply_string_content.json": {
			eventID: "msgQUOTE02", workspaceID: "ding_corp_0001", chatID: "cidGROUP0001==",
			chatType: pb.ChatType_CHAT_TYPE_GROUP, senderID: "staff_0001", senderName: "张三",
			text: "#az8 那个呢", rawText: strptr("@Aite #az8 那个呢"), mentioned: true,
			threadID: strptr("msgROOT0002"), taskNo: strptr("#AZ8"),
			occurredAt: time.UnixMilli(1700000400000).UTC(),
			quote:      &Quote{MessageID: "msgROOT0002", SenderID: "$:LWCP_v1:$fakeSender01", Text: "看 #a10 的结果"},
		},
	}

	for name, want := range cases {
		t.Run(name, func(t *testing.T) {
			data := loadFixture(t, name)
			ev := NormalizeMessage(data, normalizeOptions{botName: "Aite", tenantID: "", now: clock.Now})
			if ev == nil {
				t.Fatal("归一化返回 nil")
			}
			checkStr := func(field, got, exp string) {
				t.Helper()
				if got != exp {
					t.Errorf("%s = %q，期望 %q", field, got, exp)
				}
			}
			checkOpt := func(field string, got, exp *string) {
				t.Helper()
				switch {
				case got == nil && exp == nil:
				case got == nil || exp == nil:
					t.Errorf("%s = %v，期望 %v", field, deref(got), deref(exp))
				case *got != *exp:
					t.Errorf("%s = %q，期望 %q", field, *got, *exp)
				}
			}
			checkStr("EventId", ev.GetEventId(), want.eventID)
			if ev.GetKind() != pb.EventKind_EVENT_KIND_MESSAGE {
				t.Errorf("Kind = %v", ev.GetKind())
			}
			checkStr("Platform", ev.GetPlatform(), "dingtalk")
			checkStr("TenantId", ev.GetTenantId(), "default")
			checkStr("WorkspaceId", ev.GetWorkspaceId(), want.workspaceID)
			checkStr("ChatId", ev.GetChatId(), want.chatID)
			if ev.GetChatType() != want.chatType {
				t.Errorf("ChatType = %v，期望 %v", ev.GetChatType(), want.chatType)
			}
			checkStr("SenderId", ev.GetSenderId(), want.senderID)
			if ev.GetSenderKind() != pb.SenderKind_SENDER_KIND_HUMAN {
				t.Errorf("SenderKind = %v", ev.GetSenderKind())
			}
			checkOpt("SenderName", ev.SenderName, &want.senderName)
			checkStr("Text", ev.GetText(), want.text)
			checkOpt("RawText", ev.RawText, want.rawText)
			if ev.GetMentioned() != want.mentioned {
				t.Errorf("Mentioned = %v，期望 %v", ev.GetMentioned(), want.mentioned)
			}
			if !ev.GetOccurredAt().AsTime().Equal(want.occurredAt) {
				t.Errorf("OccurredAt = %v，期望 %v", ev.GetOccurredAt().AsTime(), want.occurredAt)
			}

			a := ev.GetAnchor()
			if a == nil {
				t.Fatal("Anchor 为 nil")
			}
			checkStr("Anchor.Platform", a.GetPlatform(), "dingtalk")
			checkStr("Anchor.ChatId", a.GetChatId(), want.chatID)
			checkStr("Anchor.MessageId", a.GetMessageId(), want.eventID)
			checkOpt("Anchor.ThreadId", a.ThreadId, want.threadID)
			checkOpt("Anchor.TaskNo", a.TaskNo, want.taskNo)

			if len(ev.GetAttachments()) != len(want.attachments) {
				t.Fatalf("附件 %d 个，期望 %d", len(ev.GetAttachments()), len(want.attachments))
			}
			for i, att := range ev.GetAttachments() {
				if att.GetKind() != pb.AttachmentKind_ATTACHMENT_KIND_IMAGE || att.GetFileKey() != want.attachments[i] || att.GetMessageId() != want.eventID {
					t.Errorf("附件[%d] = %v", i, att)
				}
			}
			if ev.CardAction != nil {
				t.Error("消息事件不该带 CardAction")
			}

			// Raw：整份 data 进审计，但 sessionWebhook 必须剥掉。
			raw := ev.GetRaw().AsMap()
			if _, ok := raw["sessionWebhook"]; ok {
				t.Error("Raw 里还留着 sessionWebhook")
			}
			if raw["msgId"] != want.eventID {
				t.Errorf("Raw.msgId = %v", raw["msgId"])
			}

			q := ParseQuote(data)
			switch {
			case want.quote == nil && q != nil:
				t.Errorf("ParseQuote = %+v，期望 nil", *q)
			case want.quote != nil && (q == nil || *q != *want.quote):
				t.Errorf("ParseQuote = %+v，期望 %+v", q, *want.quote)
			}
		})
	}

	t.Run("unsupported msgtype returns nil", func(t *testing.T) {
		for _, mt := range []string{"file", "audio", "video"} {
			if ev := NormalizeMessage(map[string]any{"msgtype": mt, "msgId": "m"}, normalizeOptions{now: clock.Now}); ev != nil {
				t.Errorf("msgtype=%s 应返回 nil", mt)
			}
		}
	})

	t.Run("quote with odd content shape does not panic", func(t *testing.T) {
		q := ParseQuote(map[string]any{"text": map[string]any{"isReplyMsg": true, "repliedMsg": map[string]any{"msgId": "m1", "content": 42.0}}})
		if q == nil || q.MessageID != "m1" || q.Text != "" {
			t.Errorf("ParseQuote = %+v", q)
		}
		if ParseQuote(map[string]any{"text": map[string]any{"isReplyMsg": true}}) == nil {
			t.Error("isReplyMsg=true 但没 repliedMsg 时也应非 nil")
		}
		if ParseQuote(map[string]any{"text": map[string]any{"content": "hi"}}) != nil {
			t.Error("普通消息 ParseQuote 应为 nil")
		}
	})
}

func deref(s *string) any {
	if s == nil {
		return nil
	}
	return *s
}
