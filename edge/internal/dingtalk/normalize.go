// 机器人消息（CALLBACK /v1.0/im/bot/messages/get）的归一化。
//
// data 一律按 map[string]any 解析：text.isReplyMsg / text.repliedMsg 没有文档，
// 官方 Go SDK 的结构体会把它们丢掉，只能从原始 JSON 里取。
package dingtalk

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"

	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"

	pb "aite/edge/gen/aitepb"
)

// 钉钉 conversationType 取值。
const (
	conversationP2P   = "1"
	conversationGroup = "2"
)

// Quote 是引用回复里被引用的那条消息（T0 之后由 DD11 用它填 NormalizedEvent.quote）。
type Quote struct {
	MessageID string
	SenderID  string
	Text      string
}

// ParseQuote 从机器人消息 data 里取引用内容。
//
// 只在 text.isReplyMsg == true 或 text.repliedMsg 存在时非 nil。
// repliedMsg.content 是对象取 .text、是字符串直接用，其它形状留空，不 panic。
func ParseQuote(data map[string]any) *Quote {
	text := asMap(data["text"])
	isReply, _ := text["isReplyMsg"].(bool)
	replied, hasReplied := text["repliedMsg"].(map[string]any)
	if !isReply && !hasReplied {
		return nil
	}
	q := &Quote{
		MessageID: mapStr(replied, "msgId"),
		SenderID:  mapStr(replied, "senderId"),
	}
	switch c := replied["content"].(type) {
	case map[string]any:
		q.Text = mapStr(c, "text")
	case string:
		q.Text = c
	}
	return q
}

// normalizeOptions 是归一化需要的外部量。
type normalizeOptions struct {
	botName  string
	tenantID string
	now      clockFunc
}

// NormalizeMessage 把机器人消息 data 归一化成 NormalizedEvent。
// 不支持的 msgtype（file / audio / video …）返回 nil。
func NormalizeMessage(data map[string]any, o normalizeOptions) *pb.NormalizedEvent {
	msgID := mapStr(data, "msgId")
	text, rawText, attachments, ok := extractContent(data, msgID, o.botName)
	if !ok {
		return nil
	}

	chatType := pb.ChatType_CHAT_TYPE_UNSPECIFIED
	switch mapStr(data, "conversationType") {
	case conversationP2P:
		chatType = pb.ChatType_CHAT_TYPE_P2P
	case conversationGroup:
		chatType = pb.ChatType_CHAT_TYPE_GROUP
	}

	occurredAt, ok := msTime(data["createAt"])
	if !ok {
		occurredAt = o.now()
	}

	var senderName *string
	if nick := mapStr(data, "senderNick"); nick != "" {
		senderName = &nick
	}

	tenantID := o.tenantID
	if tenantID == "" {
		tenantID = "default"
	}

	chatID := mapStr(data, "conversationId")
	mentioned, _ := data["isInAtList"].(bool)
	return &pb.NormalizedEvent{
		// 平台重推同一条消息 msgId 不变，core 靠它去重；adapter 自己不去重。
		EventId:     msgID,
		Kind:        pb.EventKind_EVENT_KIND_MESSAGE,
		Platform:    platformName,
		TenantId:    tenantID,
		WorkspaceId: firstNonEmpty(mapStr(data, "chatbotCorpId"), mapStr(data, "senderCorpId")),
		ChatId:      chatID,
		ChatType:    chatType,
		SenderId:    firstNonEmpty(mapStr(data, "senderStaffId"), mapStr(data, "senderId")),
		SenderKind:  pb.SenderKind_SENDER_KIND_HUMAN,
		SenderName:  senderName,
		Text:        text,
		RawText:     rawText,
		// 单聊不强行置 true：DM 路由是 DD3 的。
		Mentioned:   mentioned,
		Anchor:      anchorOf(data, chatID, msgID, text),
		Attachments: attachments,
		OccurredAt:  timestamppb.New(occurredAt),
		Raw:         rawStruct(data),
	}
}

// extractContent 按 msgtype 取正文与附件；ok=false 表示不支持的消息类型。
func extractContent(data map[string]any, msgID, botName string) (string, *string, []*pb.Attachment, bool) {
	switch mapStr(data, "msgtype") {
	case "text":
		raw := mapStr(asMap(data["text"]), "content")
		return stripBotMention(raw, botName), &raw, nil, true
	case "richText":
		var (
			parts       []string
			attachments []*pb.Attachment
		)
		for _, item := range asList(asMap(data["content"])["richText"]) {
			seg := asMap(item)
			if t, ok := seg["text"].(string); ok {
				parts = append(parts, t)
				continue
			}
			if key := firstNonEmpty(mapStr(seg, "downloadCode"), mapStr(seg, "pictureDownloadCode")); key != "" {
				attachments = append(attachments, imageAttachment(key, msgID))
			}
		}
		raw := strings.Join(parts, "")
		return stripBotMention(raw, botName), &raw, attachments, true
	case "picture":
		content := asMap(data["content"])
		key := firstNonEmpty(mapStr(content, "downloadCode"), mapStr(content, "pictureDownloadCode"))
		var attachments []*pb.Attachment
		if key != "" {
			attachments = append(attachments, imageAttachment(key, msgID))
		}
		return "", nil, attachments, true
	default:
		return "", nil, nil, false
	}
}

func imageAttachment(key, msgID string) *pb.Attachment {
	return &pb.Attachment{
		Kind:      pb.AttachmentKind_ATTACHMENT_KIND_IMAGE,
		FileKey:   key,
		MessageId: msgID,
	}
}

// stripBotMention 去首尾空白，开头若是 @<BotName> 再剥掉。
// 不剥的话 "@Aite !stop" 过不了 core R5 的 starts_with('!')。
func stripBotMention(text, botName string) string {
	text = strings.TrimSpace(text)
	if botName != "" {
		if rest, ok := strings.CutPrefix(text, "@"+botName); ok {
			text = strings.TrimSpace(rest)
		}
	}
	return text
}

// rawStruct 把整份 data 搬进 structpb.Struct 供审计，剥掉 sessionWebhook。
//
// sessionWebhook 的 URL 本身就能往会话里发消息（query 里带会话令牌），
// 与 feishu 剥校验令牌同理，不该跟着躺进证据文件。
func rawStruct(data map[string]any) *structpb.Struct {
	cleaned := make(map[string]any, len(data))
	for k, v := range data {
		if k == "sessionWebhook" {
			continue
		}
		cleaned[k] = v
	}
	s, err := structpb.NewStruct(cleaned)
	if err != nil {
		return nil
	}
	return s
}

// handleBotMessage 是 dispatcher 调的：解析 → 记会话缓存 → 归一化 → sink。
// 返回非 nil 时调用方回非 200 ACK（对齐 feishu「失败 → 让平台重推」的意图）。
func (p *Platform) handleBotMessage(ctx context.Context, raw string) error {
	var data map[string]any
	if err := json.Unmarshal([]byte(raw), &data); err != nil {
		p.logger.Warn("dingtalk.bad_message", "err", err)
		// 坏 JSON 重推也还是坏的：照样 ACK 成功。
		return nil
	}
	p.sessions.remember(data)

	if p.sink == nil {
		p.logger.Warn("dingtalk.no_sink", "note", "没接 EventSink，事件无处可送")
		return nil
	}
	ev := NormalizeMessage(data, normalizeOptions{
		botName: p.cfg.BotName, tenantID: p.opts.TenantID, now: p.clock,
	})
	if ev == nil {
		p.logger.Debug("dingtalk.message_ignored", "msgtype", mapStr(data, "msgtype"))
		return nil
	}
	if err := p.sink.HandleEvent(ctx, ev); err != nil {
		p.logger.Error("dingtalk.on_event_failed", "event_id", ev.GetEventId(), "err", err)
		return err
	}
	return nil
}

// ---------------------------------------------------------------------------
// 会话缓存：conversationId → 出站要用的会话信息
// ---------------------------------------------------------------------------

type sessionInfo struct {
	webhook          string
	webhookExpiresAt time.Time // 零值 = 不知道何时过期，按已过期处理
	conversationType string
	staffID          string
	corpID           string
}

type sessionCache struct {
	mu   sync.Mutex
	byID map[string]sessionInfo
}

func newSessionCache() *sessionCache { return &sessionCache{byID: map[string]sessionInfo{}} }

func (s *sessionCache) remember(data map[string]any) {
	chatID := mapStr(data, "conversationId")
	if chatID == "" {
		return
	}
	info := sessionInfo{
		webhook:          mapStr(data, "sessionWebhook"),
		conversationType: mapStr(data, "conversationType"),
		staffID:          mapStr(data, "senderStaffId"),
		corpID:           mapStr(data, "chatbotCorpId"),
	}
	if t, ok := msTime(data["sessionWebhookExpiredTime"]); ok {
		info.webhookExpiresAt = t
	}
	s.put(chatID, info)
}

func (s *sessionCache) put(chatID string, info sessionInfo) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.byID[chatID] = info
}

func (s *sessionCache) get(chatID string) (sessionInfo, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	info, ok := s.byID[chatID]
	return info, ok
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

func asMap(v any) map[string]any {
	if m, ok := v.(map[string]any); ok {
		return m
	}
	return map[string]any{}
}

func asList(v any) []any {
	if l, ok := v.([]any); ok {
		return l
	}
	return nil
}

func mapStr(m map[string]any, key string) string {
	switch v := m[key].(type) {
	case string:
		return v
	case float64:
		return strconv.FormatInt(int64(v), 10)
	case json.Number:
		return v.String()
	default:
		return ""
	}
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

// msTime 把毫秒时间戳（数字或数字字符串）转成 time.Time。
func msTime(v any) (time.Time, bool) {
	var ms int64
	switch x := v.(type) {
	case float64:
		ms = int64(x)
	case string:
		n, err := strconv.ParseInt(x, 10, 64)
		if err != nil {
			return time.Time{}, false
		}
		ms = n
	default:
		return time.Time{}, false
	}
	if ms <= 0 {
		return time.Time{}, false
	}
	return time.UnixMilli(ms).UTC(), true
}

func jsonBody(data []byte) map[string]any {
	var parsed map[string]any
	if err := json.Unmarshal(data, &parsed); err != nil || parsed == nil {
		return map[string]any{}
	}
	return parsed
}

// decodeJSONField 把「JSON 字符串或已是对象」的字段解成 map。
func decodeJSONField(v any) (map[string]any, error) {
	switch x := v.(type) {
	case map[string]any:
		return x, nil
	case string:
		var out map[string]any
		if err := json.Unmarshal([]byte(x), &out); err != nil {
			return nil, err
		}
		return out, nil
	case nil:
		return nil, errors.New("字段缺失")
	default:
		return nil, fmt.Errorf("意外的字段类型 %T", v)
	}
}
