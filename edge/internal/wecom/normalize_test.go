package wecom

import (
	"encoding/json"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"

	"google.golang.org/protobuf/encoding/protojson"

	pb "aite/edge/gen/aitepb"
)

var testNow = time.Date(2026, 9, 25, 9, 0, 0, 0, time.UTC)

// frameOf 把一份 body 包成回调帧。
func frameOf(t *testing.T, cmd, reqID string, body any) *inFrame {
	t.Helper()
	raw, err := json.Marshal(body)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	return &inFrame{Cmd: cmd, Headers: frameHeaders{ReqID: reqID}, Body: raw}
}

// normalizeBody 走一遍 JSON → msgCallback → normalizeMessage。
func normalizeBody(t *testing.T, reqID string, body map[string]any) *normalized {
	t.Helper()
	f := frameOf(t, cmdMsgCallback, reqID, body)
	var cb msgCallback
	if err := json.Unmarshal(f.Body, &cb); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	return normalizeMessage(f, &cb, Options{BotID: "bot-opt", TenantID: "default"}, "Aite", testNow)
}

func withQuote(body map[string]any, quote map[string]any) map[string]any {
	body["quote"] = quote
	return body
}

func mediaMsg(msgID, chatType, msgType string) map[string]any {
	return map[string]any{
		"msgid": msgID, "aibotid": "bot-test", "chatid": "chat-1", "chattype": chatType,
		"from": map[string]any{"userid": "u-1"}, "msgtype": msgType,
		msgType: map[string]any{"url": "http://127.0.0.1:1/dl/" + msgID, "aeskey": "SECRET-AESKEY-" + msgID},
	}
}

func TestGroupOnlyTextMixedQuote(t *testing.T) {
	// 群 text。
	n := normalizeBody(t, "rq-1", textMsg("m-text", "chat-1", "group", "  @Aite  查一下昨天的报错 "))
	if n == nil {
		t.Fatalf("群 text 被丢了")
	}
	ev := n.event
	if ev.GetKind() != pb.EventKind_EVENT_KIND_MESSAGE || ev.GetPlatform() != "wecom" ||
		ev.GetChatType() != pb.ChatType_CHAT_TYPE_GROUP || !ev.GetMentioned() ||
		ev.GetText() != "查一下昨天的报错" || ev.GetRawText() != "  @Aite  查一下昨天的报错 " ||
		ev.GetSenderId() != "u-1" || ev.GetSenderKind() != pb.SenderKind_SENDER_KIND_HUMAN ||
		ev.GetWorkspaceId() != "bot-test" || ev.GetEventId() != "m-text" || ev.GetTenantId() != "default" {
		t.Fatalf("群 text 归一化不对：%v", ev)
	}
	a := ev.GetAnchor()
	if a == nil || a.GetPlatform() != "wecom" || a.GetChatId() != "chat-1" || a.GetMessageId() != "m-text" || a.ThreadId != nil {
		t.Fatalf("Anchor 不对：%v", a)
	}
	if !ev.GetOccurredAt().AsTime().Equal(testNow) {
		t.Fatalf("OccurredAt 要用收到时刻")
	}

	// 群 mixed：文字按顺序拼接、图片进附件。
	mixed := map[string]any{
		"msgid": "m-mixed", "aibotid": "bot-test", "chatid": "chat-1", "chattype": "group",
		"from": map[string]any{"userid": "u-1"}, "msgtype": "mixed",
		"mixed": map[string]any{"msg_item": []any{
			map[string]any{"msgtype": "text", "text": map[string]any{"content": "@Aite 看图"}},
			map[string]any{"msgtype": "image", "image": map[string]any{"url": "http://127.0.0.1:1/i", "aeskey": "K"}},
			map[string]any{"msgtype": "text", "text": map[string]any{"content": "，哪里不对"}},
		}},
	}
	n = normalizeBody(t, "rq-2", mixed)
	if n == nil || n.event.GetText() != "看图，哪里不对" || len(n.event.GetAttachments()) != 1 ||
		n.event.GetAttachments()[0].GetKind() != pb.AttachmentKind_ATTACHMENT_KIND_IMAGE {
		t.Fatalf("群 mixed 不对：%v", n)
	}

	// 群里带 quote 的 text。
	n = normalizeBody(t, "rq-3", withQuote(textMsg("m-quote", "chat-1", "group", "@Aite 继续"),
		map[string]any{"msgtype": "text", "text": map[string]any{"content": "上一条回复"}}))
	if n == nil || n.event.GetText() != "继续" {
		t.Fatalf("群 quote text 不对：%v", n)
	}

	// 群 image / file / voice → nil。
	for _, mt := range []string{"image", "file", "voice"} {
		if n := normalizeBody(t, "rq-g-"+mt, mediaMsg("m-g-"+mt, "group", mt)); n != nil {
			t.Fatalf("群 %s 应该丢掉，得到 %v", mt, n.event)
		}
	}

	// 单聊 image → 带附件的事件，Mentioned=false，FileKey 不含密钥。
	n = normalizeBody(t, "rq-s", mediaMsg("m-s-img", "single", "image"))
	if n == nil {
		t.Fatalf("单聊 image 被丢了")
	}
	if n.event.GetChatType() != pb.ChatType_CHAT_TYPE_P2P || n.event.GetMentioned() {
		t.Fatalf("单聊口径不对：%v", n.event)
	}
	atts := n.event.GetAttachments()
	if len(atts) != 1 || atts[0].GetKind() != pb.AttachmentKind_ATTACHMENT_KIND_IMAGE || atts[0].GetMessageId() != "m-s-img" {
		t.Fatalf("单聊 image 附件不对：%v", atts)
	}
	if strings.Contains(atts[0].GetFileKey(), "SECRET") || strings.Contains(atts[0].GetFileKey(), "http") {
		t.Fatalf("FileKey 带了密钥或链接：%q", atts[0].GetFileKey())
	}
	if len(n.media) != 1 || n.media[0].aesKey != "SECRET-AESKEY-m-s-img" || n.media[0].fileKey != atts[0].GetFileKey() {
		t.Fatalf("下载表登记不对：%+v", n.media)
	}

	// msgid 空 → EventId / Anchor.MessageId 都用回调 req_id；aibotid 空 → Options.BotID。
	noID := textMsg("", "chat-1", "single", "hi")
	delete(noID, "aibotid")
	n = normalizeBody(t, "rq-noid", noID)
	if n.event.GetEventId() != "rq-noid" || n.event.GetAnchor().GetMessageId() != "rq-noid" || n.event.GetWorkspaceId() != "bot-opt" {
		t.Fatalf("回退口径不对：%v", n.event)
	}
}

func TestQuoteTaskNoGoesToAnchor(t *testing.T) {
	quoted := func(body, quote string) *pb.Anchor {
		t.Helper()
		msg := textMsg("m-q", "chat-1", "group", body)
		if quote != "" {
			msg = withQuote(msg, map[string]any{"msgtype": "text", "text": map[string]any{"content": quote}})
		}
		n := normalizeBody(t, "rq-q", msg)
		if n == nil {
			t.Fatalf("消息被丢了")
		}
		return n.event.GetAnchor()
	}

	a := quoted("@Aite 这个怎么样了", "已开始处理，进度见 #AH，稍后回复")
	if a.TaskNo == nil || a.GetTaskNo() != "#AH" {
		t.Fatalf("quote 里的 #AH 没进 task_no：%v", a)
	}
	if a.ThreadId != nil {
		t.Fatalf("企微没有话题，ThreadId 必须为 nil")
	}
	if a := quoted("@Aite 先看 #A1 再说", "进度见 #AH"); a.GetTaskNo() != "#A1" {
		t.Fatalf("正文优先于 quote：%v", a)
	}
	if a := quoted("@Aite #ah 呢", ""); a.GetTaskNo() != "#AH" {
		t.Fatalf("小写要转大写：%v", a)
	}
	if a := quoted("@Aite #AI 呢", ""); a.TaskNo != nil {
		t.Fatalf("I 不在 Crockford 字母表里，#AI 不该命中：%v", a)
	}

	// quote 是 mixed 时看它的文字。
	msg := withQuote(textMsg("m-q2", "chat-1", "group", "@Aite 继续"), map[string]any{
		"msgtype": "mixed",
		"mixed": map[string]any{"msg_item": []any{
			map[string]any{"msgtype": "text", "text": map[string]any{"content": "任务 #AZ8 已交付"}},
		}},
	})
	if n := normalizeBody(t, "rq-q2", msg); n.event.GetAnchor().GetTaskNo() != "#AZ8" {
		t.Fatalf("mixed quote 的 #AZ8 没取到：%v", n.event.GetAnchor())
	}
}

func TestRawRedactsAESKey(t *testing.T) {
	const aesKey = "Zm9vYmFyYmF6cXV4LXRlc3QtYWVza2V5LWZvci1yYXc"
	body := map[string]any{
		"msgid": "m-raw", "aibotid": "bot-test", "chatid": "chat-1", "chattype": "single",
		"from": map[string]any{"userid": "u-1"}, "msgtype": "mixed",
		"mixed": map[string]any{"msg_item": []any{
			map[string]any{"msgtype": "text", "text": map[string]any{"content": "看看"}},
			map[string]any{"msgtype": "image", "image": map[string]any{
				"url": "http://127.0.0.1:1/dl/secret-link", "aeskey": aesKey,
			}},
		}},
		"quote": map[string]any{"msgtype": "file", "file": map[string]any{
			"url": "http://127.0.0.1:1/dl/quoted-link", "aeskey": aesKey,
		}},
	}
	n := normalizeBody(t, "rq-raw", body)
	if n == nil || n.event.GetRaw() == nil {
		t.Fatalf("Raw 缺失")
	}
	out, err := protojson.Marshal(n.event.GetRaw())
	if err != nil {
		t.Fatalf("marshal raw: %v", err)
	}
	s := string(out)
	if strings.Contains(s, aesKey) || strings.Contains(s, "aeskey") {
		t.Fatalf("Raw 里还有 aeskey：%s", s)
	}
	if strings.Contains(s, "secret-link") || strings.Contains(s, "quoted-link") {
		t.Fatalf("Raw 里还有下载链接：%s", s)
	}
	if !strings.Contains(s, "m-raw") || !strings.Contains(s, "rq-raw") {
		t.Fatalf("Raw 把该留的也删了：%s", s)
	}
}

func TestEnterChatWelcomeWithin5s(t *testing.T) {
	enter := map[string]any{
		"aibotid": "bot-test", "chatid": "u-new", "chattype": "single",
		"from": map[string]any{"userid": "u-new"}, "event": map[string]any{"eventtype": "enter_chat"},
	}

	// 有 WelcomeText → 回一帧欢迎语，req_id 对得上。
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, func(po *platformOptions) { po.opts.WelcomeText = "你好，我是 Aite" })
	h.waitConnected(t)
	if err := fs.conn(0).push(cmdEventCallback, "rq-enter-1", enter); err != nil {
		t.Fatalf("push: %v", err)
	}
	waitFor(t, "欢迎语帧", func() bool { return len(fs.framesWith(cmdRespondWelcome)) == 1 })
	w := fs.framesWith(cmdRespondWelcome)[0]
	if w.ReqID != "rq-enter-1" {
		t.Fatalf("欢迎语 req_id = %q，要回调的 rq-enter-1", w.ReqID)
	}
	if text, _ := w.Body["text"].(map[string]any); text["content"] != "你好，我是 Aite" {
		t.Fatalf("欢迎语内容不对：%v", w.Body)
	}
	// 屏障：再推一条消息，等它到 sink —— 回调是按序处理的，enter_chat 早处理完了。
	h.inbound(t, "rq-barrier-1", textMsg("m-b1", "u-new", "single", "hi"))
	if got := len(h.sink.all()); got != 1 {
		t.Fatalf("enter_chat 不该送 core：sink 收到 %d 个事件（含屏障 1 个）", got)
	}

	// 无 WelcomeText → 一帧不发。
	fs2 := newFakeServer(t, nil)
	h2 := startHarness(t, fs2, nil)
	h2.waitConnected(t)
	if err := fs2.conn(0).push(cmdEventCallback, "rq-enter-2", enter); err != nil {
		t.Fatalf("push: %v", err)
	}
	h2.inbound(t, "rq-barrier-2", textMsg("m-b2", "u-new", "single", "hi"))
	if n := len(fs2.framesWith(cmdRespondWelcome)); n != 0 {
		t.Fatalf("没配欢迎语却发了 %d 帧", n)
	}
	if got := len(h2.sink.all()); got != 1 {
		t.Fatalf("enter_chat 不该送 core：sink 收到 %d 个事件（含屏障 1 个）", got)
	}
}

func TestTemplateCardEventAndUpdateDeadline5s(t *testing.T) {
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, nil)
	h.waitConnected(t)

	cardEvent := func(key, taskID string) map[string]any {
		return map[string]any{
			"msgid": "", "aibotid": "bot-test", "chatid": "chat-1", "chattype": "group",
			"from": map[string]any{"userid": "u-op"},
			"event": map[string]any{
				"eventtype":           "template_card_event",
				"template_card_event": map[string]any{"card_type": "button_interaction", "event_key": key, "task_id": taskID},
			},
		}
	}

	// stop 按钮 → CARD_ACTION。
	if err := fs.conn(0).push(cmdEventCallback, "rq-card-1", cardEvent("stop", "task-42")); err != nil {
		t.Fatalf("push: %v", err)
	}
	waitFor(t, "CARD_ACTION 事件", func() bool { return len(h.sink.all()) == 1 })
	ev := h.sink.all()[0]
	ca := ev.GetCardAction()
	if ev.GetKind() != pb.EventKind_EVENT_KIND_CARD_ACTION || ca == nil ||
		ca.GetAction() != pb.CardActionKind_CARD_ACTION_KIND_STOP || ca.GetTaskId() != "task-42" ||
		ev.GetAnchor() == nil || ev.GetSenderId() != "u-op" || !ev.GetMentioned() {
		t.Fatalf("CARD_ACTION 不对：%v", ev)
	}

	// 认不出的按钮 → 不送 core。
	if err := fs.conn(0).push(cmdEventCallback, "rq-card-x", cardEvent("approve", "")); err != nil {
		t.Fatalf("push: %v", err)
	}
	h.inbound(t, "rq-barrier", textMsg("m-b", "chat-1", "group", "@Aite hi"))
	if got := len(h.sink.all()); got != 2 {
		t.Fatalf("认不出的按钮不该送 core：sink 共 %d 个（要 CARD_ACTION + 屏障 = 2）", got)
	}

	ctx := t.Context()
	card := map[string]any{"card_type": "text_notice", "main_title": map[string]any{"title": "已停止"}}

	// 4.9 s：发得出。
	h.clock.Advance(4900 * time.Millisecond)
	if err := h.p.RespondTemplateCardUpdate(ctx, "rq-card-1", card); err != nil {
		t.Fatalf("4.9s 内要发得出：%v", err)
	}
	ups := fs.framesWith(cmdRespondUpdate)
	if len(ups) != 1 || ups[0].ReqID != "rq-card-1" {
		t.Fatalf("更新帧不对：%+v", ups)
	}

	// 另一条回调，5.1 s：返回错误且无帧。
	if err := fs.conn(0).push(cmdEventCallback, "rq-card-2", cardEvent("evidence", "task-43")); err != nil {
		t.Fatalf("push: %v", err)
	}
	waitFor(t, "第二个 CARD_ACTION", func() bool { return len(h.sink.all()) == 3 })
	if got := h.sink.all()[2].GetCardAction().GetAction(); got != pb.CardActionKind_CARD_ACTION_KIND_EVIDENCE {
		t.Fatalf("evidence 按钮映射不对：%v", got)
	}
	h.clock.Advance(5100 * time.Millisecond)
	err := h.p.RespondTemplateCardUpdate(ctx, "rq-card-2", card)
	if platformCode(err) != "deadline_exceeded" {
		t.Fatalf("5.1s 要返回 deadline_exceeded，得到 %v", err)
	}
	if n := len(fs.framesWith(cmdRespondUpdate)); n != 1 {
		t.Fatalf("超时后还发了帧：共 %d 帧", n)
	}
}

func TestFeedbackEventParsed(t *testing.T) {
	var (
		got  []Feedback
		gotM = make(chan struct{}, 8)
	)
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, func(po *platformOptions) {
		po.onFeedback = func(fb Feedback) { got = append(got, fb); gotM <- struct{}{} }
	})
	h.waitConnected(t)

	fixtures := []string{"feedback_dislike.json", "feedback_like.json", "feedback_cancel.json"}
	for i, name := range fixtures {
		data, err := os.ReadFile("testdata/" + name)
		if err != nil {
			t.Fatalf("读夹具 %s：%v", name, err)
		}
		var body map[string]any
		if err := json.Unmarshal(data, &body); err != nil {
			t.Fatalf("夹具 %s 不是 JSON：%v", name, err)
		}
		if err := fs.conn(0).push(cmdEventCallback, "rq-fb-"+string(rune('a'+i)), body); err != nil {
			t.Fatalf("push: %v", err)
		}
	}
	h.inbound(t, "rq-barrier", textMsg("m-b", "chat-fb", "group", "@Aite hi"))
	for range fixtures {
		<-gotM
	}

	at := h.clock.Now()
	want := []Feedback{
		{FeedbackID: "stream-abc", Kind: FeedbackDislike, Text: "答非所问", ReasonCodes: []int{2, 4}, ChatID: "chat-fb", UserID: "u-dislike", At: at},
		{FeedbackID: "stream-abc", Kind: FeedbackLike, ChatID: "chat-fb", UserID: "u-like", At: at},
		{FeedbackID: "stream-abc", Kind: FeedbackCancel, ChatID: "chat-fb", UserID: "u-dislike", At: at},
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("Feedback 解析不对：\n got  %+v\n want %+v", got, want)
	}
	if n := len(h.sink.all()); n != 1 {
		t.Fatalf("feedback_event 不该送 core：sink 共 %d 个（只该有屏障 1 个）", n)
	}
}
