// 入站归一化：aibot_msg_callback / template_card_event → pb.NormalizedEvent，
// feedback_event → 包内 Feedback（今天不送 core）。字段口径照 feishu/normalize.go。
package wecom

import (
	"encoding/json"
	"regexp"
	"strconv"
	"strings"
	"time"

	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"

	pb "aite/edge/gen/aitepb"
)

// taskNoRe 是可见锚点 #A..：Crockford base32（没有 I L O U），大小写都认，取到后转大写。
var taskNoRe = regexp.MustCompile(`(?i)#A([0-9A-HJKMNP-TV-Z]+)\b`)

// findTaskNo 在一段文字里找第一个 #A..，找不到返回 ""。
func findTaskNo(s string) string {
	m := taskNoRe.FindStringSubmatch(s)
	if m == nil {
		return ""
	}
	return "#A" + strings.ToUpper(m[1])
}

// inboundMedia 是一条消息里的一个可下载资源（url + aeskey 只进包内表，不进事件）。
type inboundMedia struct {
	fileKey string
	url     string
	aesKey  string
}

// normalized 是消息归一化的产物：事件 + 要登记进下载表的媒体。
type normalized struct {
	event *pb.NormalizedEvent
	media []inboundMedia
}

// mixedText 把图文混排里的文字按顺序拼起来。
func mixedText(m *mixedBody) string {
	if m == nil {
		return ""
	}
	var sb strings.Builder
	for _, item := range m.MsgItem {
		if item.MsgType == msgTypeText && item.Text != nil {
			sb.WriteString(item.Text.Content)
		}
	}
	return sb.String()
}

// quoteText 是引用消息里的文字（text / mixed）；引用的图片文件没有文字。
func quoteText(q *quoteBody) string {
	if q == nil {
		return ""
	}
	switch q.MsgType {
	case msgTypeText:
		if q.Text != nil {
			return q.Text.Content
		}
	case msgTypeMixed:
		return mixedText(q.Mixed)
	}
	// msgtype 缺省时两样都看一眼。
	if q.Text != nil {
		return q.Text.Content
	}
	return mixedText(q.Mixed)
}

// stripMention 去掉开头的 @<BotName> 再 trim。
func stripMention(text, botName string) string {
	t := strings.TrimSpace(text)
	if botName != "" {
		t = strings.TrimPrefix(t, "@"+botName)
	}
	return strings.TrimSpace(t)
}

// groupAccepts / singleAccepts：群里只认 @ 的 text / mixed（引用挂在它们身上）；
// 单聊认 text / mixed / image / file。
func groupAccepts(msgType string) bool {
	return msgType == msgTypeText || msgType == msgTypeMixed
}

func singleAccepts(msgType string) bool {
	switch msgType {
	case msgTypeText, msgTypeMixed, msgTypeImage, msgTypeFile:
		return true
	}
	return false
}

// normalizeMessage 把 aibot_msg_callback 归一化；不收的返回 nil。
func normalizeMessage(f *inFrame, body *msgCallback, opts Options, botName string, receivedAt time.Time) *normalized {
	var chatType pb.ChatType
	switch body.ChatType {
	case chatTypeGroup:
		if !groupAccepts(body.MsgType) {
			return nil
		}
		chatType = pb.ChatType_CHAT_TYPE_GROUP
	case chatTypeSingle:
		if !singleAccepts(body.MsgType) {
			return nil
		}
		chatType = pb.ChatType_CHAT_TYPE_P2P
	default:
		return nil
	}

	// msgid 空则用回调 req_id：EventId 与 Anchor.MessageId 同值，也是 replyTo → req_id 表的键。
	messageID := firstNonEmpty(body.MsgID, f.Headers.ReqID)

	var rawText string
	var media []inboundMedia
	var attachments []*pb.Attachment
	addMedia := func(kind pb.AttachmentKind, prefix string, m *mediaBody) {
		if m == nil {
			return
		}
		key := prefix + strconv.Itoa(len(media))
		media = append(media, inboundMedia{fileKey: key, url: m.URL, aesKey: m.AESKey})
		att := &pb.Attachment{Kind: kind, FileKey: key, MessageId: messageID}
		if m.FileName != "" {
			name := m.FileName
			att.Name = &name
		}
		attachments = append(attachments, att)
	}

	switch body.MsgType {
	case msgTypeText:
		if body.Text != nil {
			rawText = body.Text.Content
		}
	case msgTypeMixed:
		rawText = mixedText(body.Mixed)
		if body.Mixed != nil {
			for i := range body.Mixed.MsgItem {
				if item := body.Mixed.MsgItem[i]; item.MsgType == msgTypeImage {
					addMedia(pb.AttachmentKind_ATTACHMENT_KIND_IMAGE, "img-", item.Image)
				}
			}
		}
	case msgTypeImage:
		addMedia(pb.AttachmentKind_ATTACHMENT_KIND_IMAGE, "img-", body.Image)
	case msgTypeFile:
		addMedia(pb.AttachmentKind_ATTACHMENT_KIND_FILE, "file-", body.File)
	}

	mentioned := chatType == pb.ChatType_CHAT_TYPE_GROUP
	text := strings.TrimSpace(rawText)
	if mentioned {
		text = stripMention(rawText, botName)
	}

	// 可见锚点：先看本条正文，再看引用的文字。引用没有 msgid，ThreadId 恒 nil。
	anchor := &pb.Anchor{Platform: platformName, ChatId: body.ChatID, MessageId: messageID}
	if no := firstNonEmpty(findTaskNo(rawText), findTaskNo(quoteText(body.Quote))); no != "" {
		anchor.TaskNo = &no
	}

	ev := &pb.NormalizedEvent{
		EventId:     messageID,
		Kind:        pb.EventKind_EVENT_KIND_MESSAGE,
		Platform:    platformName,
		TenantId:    opts.TenantID,
		WorkspaceId: firstNonEmpty(body.AibotID, opts.BotID),
		ChatId:      body.ChatID,
		ChatType:    chatType,
		// userid 除非 bot 由超管创建否则是加密的，原样填（plan:122）。
		SenderId:    body.From.UserID,
		SenderKind:  pb.SenderKind_SENDER_KIND_HUMAN,
		Text:        text,
		Mentioned:   mentioned,
		Anchor:      anchor,
		Attachments: attachments,
		OccurredAt:  timestamppb.New(receivedAt),
		Raw:         rawStruct(f),
	}
	if rawText != "" {
		rt := rawText
		ev.RawText = &rt
	}
	return &normalized{event: ev, media: media}
}

// normalizeTemplateCard 把模板卡片按钮回调归一化成 CARD_ACTION；认不出的按钮返回 nil
// （CardActionKind 今天只有 stop / evidence，塞别的值进去只会在下游炸掉）。
func normalizeTemplateCard(f *inFrame, body *eventCallback, opts Options, receivedAt time.Time) *pb.NormalizedEvent {
	tc := body.Event.TemplateCardEvent
	if tc == nil {
		return nil
	}
	var kind pb.CardActionKind
	switch tc.EventKey {
	case "stop":
		kind = pb.CardActionKind_CARD_ACTION_KIND_STOP
	case "evidence":
		kind = pb.CardActionKind_CARD_ACTION_KIND_EVIDENCE
	default:
		return nil
	}

	eventID := firstNonEmpty(body.MsgID, f.Headers.ReqID)
	var taskID *string
	if tc.TaskID != "" {
		tid := tc.TaskID
		taskID = &tid
	}
	chatType := pb.ChatType_CHAT_TYPE_GROUP
	if body.ChatType == chatTypeSingle {
		chatType = pb.ChatType_CHAT_TYPE_P2P
	}
	value, _ := structpb.NewStruct(map[string]any{
		"action": tc.EventKey, "task_id": tc.TaskID, "card_type": tc.CardType,
	})

	return &pb.NormalizedEvent{
		EventId:     eventID,
		Kind:        pb.EventKind_EVENT_KIND_CARD_ACTION,
		Platform:    platformName,
		TenantId:    opts.TenantID,
		WorkspaceId: firstNonEmpty(body.AibotID, opts.BotID),
		ChatId:      body.ChatID,
		ChatType:    chatType,
		SenderId:    body.From.UserID,
		// 点按钮的一定是真人。
		SenderKind: pb.SenderKind_SENDER_KIND_HUMAN,
		// 点的是我们自己发的卡片，等价于「冲着 Aite 来的」。
		Mentioned: true,
		Anchor: &pb.Anchor{
			Platform:  platformName,
			ChatId:    body.ChatID,
			MessageId: eventID,
		},
		CardAction: &pb.CardAction{
			// 回调不给卡片所在消息的 id；用本条回调的 id，话题归属由 core 按 task_id 查。
			CardId: eventID,
			Action: kind,
			TaskId: taskID,
			Value:  value,
		},
		OccurredAt: timestamppb.New(receivedAt),
		Raw:        rawStruct(f),
	}
}

// FeedbackKind 是点赞 / 点踩 / 取消。
type FeedbackKind string

const (
	FeedbackLike    FeedbackKind = "like"
	FeedbackDislike FeedbackKind = "dislike"
	FeedbackCancel  FeedbackKind = "cancel"
)

// Feedback 是 feedback_event 解析出的包内结构体（DD11 拿它映射 Reaction{thumbs_down}）。
// FeedbackID = 开流时填的 stream.feedback.id = 返回给 core 的 MessageId。
type Feedback struct {
	FeedbackID  string
	Kind        FeedbackKind
	Text        string
	ReasonCodes []int
	ChatID      string
	UserID      string
	At          time.Time
}

// parseFeedback 解析 feedback_event；缺负载或类型认不出返回 false。
func parseFeedback(body *eventCallback, receivedAt time.Time) (Feedback, bool) {
	fe := body.Event.FeedbackEvent
	if fe == nil {
		return Feedback{}, false
	}
	var kind FeedbackKind
	switch fe.Type {
	case feedbackTypeLike:
		kind = FeedbackLike
	case feedbackTypeDislike:
		kind = FeedbackDislike
	case feedbackTypeCancel:
		kind = FeedbackCancel
	default:
		return Feedback{}, false
	}
	var reasons []int
	if len(fe.InaccurateReasonList) > 0 {
		reasons = append([]int(nil), fe.InaccurateReasonList...)
	}
	return Feedback{
		FeedbackID:  fe.ID,
		Kind:        kind,
		Text:        fe.Content,
		ReasonCodes: reasons,
		ChatID:      body.ChatID,
		UserID:      body.From.UserID,
		At:          receivedAt,
	}, true
}

// redactedKeys 是进 Raw 之前必须删掉的键：aeskey 是解密密钥，url 是 5 分钟有效的下载链接。
// Raw 会落进审计 / 证据（照飞书信封不放 token 的理由）。
var redactedKeys = map[string]bool{"aeskey": true, "url": true}

// redact 递归删掉 redactedKeys。
func redact(v any) any {
	switch t := v.(type) {
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, iv := range t {
			if redactedKeys[k] {
				continue
			}
			out[k] = redact(iv)
		}
		return out
	case []any:
		out := make([]any, len(t))
		for i, iv := range t {
			out[i] = redact(iv)
		}
		return out
	default:
		return v
	}
}

// rawStruct 把回调帧（cmd + req_id + 脱敏后的 body）装进 structpb。
func rawStruct(f *inFrame) *structpb.Struct {
	var body any
	if len(f.Body) > 0 {
		if err := json.Unmarshal(f.Body, &body); err != nil {
			body = nil
		}
	}
	raw := map[string]any{
		"cmd":     f.Cmd,
		"headers": map[string]any{"req_id": f.Headers.ReqID},
	}
	if body != nil {
		raw["body"] = redact(body)
	}
	s, err := structpb.NewStruct(raw)
	if err != nil {
		// 审计少一份原文，也好过为此把整条事件丢掉。
		return nil
	}
	return s
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}
