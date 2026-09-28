// 对应 aite/adapters/feishu/normalize.py。
//
// 飞书原始事件 → pb.NormalizedEvent（proto/aite/v1/events.proto）。
//
// 认的事件都登记在分发表（events.go）里：P0 的 im.message.receive_v1 / card.action.trigger，
// CC8 加的撤回（→ MESSAGE_DELETED）、机器人入群（→ BOT_ADDED）、成员进出（→ MEMBER_CHANGED），
// 以及只解析、暂不上送的表情回复（等契约里的 EventKind::Reaction）。
// 表里没有的事件返回 nil —— 没订阅的东西凭空映射成别的 EventKind 就是在发明契约。
//
// 归一化全程只读原始 map，不经过 SDK 的类型对象：raw 要原样进 NormalizedEvent.raw
// 供审计（events.proto「任何逻辑不得依赖 raw」），过一遍 SDK 模型再吐回来只会丢字段。
package feishu

import (
	"encoding/json"
	"log/slog"
	"math"
	"regexp"
	"strconv"
	"strings"
	"time"

	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"

	pb "aite/edge/gen/aitepb"
)

const platformName = "feishu"

// CC8 订阅的新事件。字段路径以 lark-oapi-go v3.12.0 生成的结构体 json tag 为准
// （service/im/v1/model.go 的 P2*V1Data），见各归一化函数的注释。
const (
	eventMessageRecalled = "im.message.recalled_v1"
	eventBotAdded        = "im.chat.member.bot.added_v1"
	eventUserAdded       = "im.chat.member.user.added_v1"
	eventUserDeleted     = "im.chat.member.user.deleted_v1"
	eventReactionCreated = "im.message.reaction.created_v1"
	eventReactionDeleted = "im.message.reaction.deleted_v1"
)

// init 把本文件归一化的事件登记进分发表（events.go）。
func init() {
	registerEvent(eventMessageReceive, eventEntry{normalize: NormalizeMessage})
	registerEvent(eventCardAction, eventEntry{
		normalize: func(raw map[string]any, _, workspaceID, tenantID string) *pb.NormalizedEvent {
			return NormalizeCardAction(raw, workspaceID, tenantID)
		},
		callback: true,
	})

	registerEvent(eventMessageRecalled, eventEntry{normalize: normalizeRecalled, onDropped: logMalformed})
	registerEvent(eventBotAdded, eventEntry{normalize: normalizeBotAdded, onDropped: logMalformed})
	registerEvent(eventUserAdded, eventEntry{normalize: normalizeMemberChanged, onDropped: logMalformed})
	registerEvent(eventUserDeleted, eventEntry{normalize: normalizeMemberChanged, onDropped: logMalformed})
	// 表情事件只解析、不上送：H5 会在控制台订阅它们，不注册的话 SDK 回 NotFound → 平台重推。
	registerEvent(eventReactionCreated, eventEntry{normalize: dropReaction, onDropped: logReactionDropped})
	registerEvent(eventReactionDeleted, eventEntry{normalize: dropReaction, onDropped: logReactionDropped})
}

// senderKindByType 是飞书 sender.sender_type → 契约 SenderKind。
//
// 飞书有两套 sender_type 枚举，取值不一样，这张表要同时吃下（T16 核过）：
//   - 事件面 im.message.receive_v1 的 event.sender.sender_type：user / bot
//   - 消息 API 面（获取消息 / 会话历史）items[].sender.sender_type：user / app / anonymous / unknown
//
// anonymous / unknown / 日后新增的值，跟非字符串一起落到 app 的兜底上：
// 一律不映射到 system，把未知值猜成 system 反而会让审计看不出它其实是个应用。
var senderKindByType = map[string]pb.SenderKind{
	"user":   pb.SenderKind_SENDER_KIND_HUMAN,
	"bot":    pb.SenderKind_SENDER_KIND_BOT, // 事件面：其他机器人
	"app":    pb.SenderKind_SENDER_KIND_APP, // 消息 API 面：应用
	"system": pb.SenderKind_SENDER_KIND_SYSTEM,
}

const senderKindFallback = pb.SenderKind_SENDER_KIND_APP

// attachmentByMsgType 是消息类型 → (附件 kind, content 里装 key 的字段名)。
var attachmentByMsgType = map[string]struct {
	kind     pb.AttachmentKind
	keyField string
}{
	"image":   {pb.AttachmentKind_ATTACHMENT_KIND_IMAGE, "image_key"},
	"sticker": {pb.AttachmentKind_ATTACHMENT_KIND_IMAGE, "file_key"},
	"file":    {pb.AttachmentKind_ATTACHMENT_KIND_FILE, "file_key"},
	"audio":   {pb.AttachmentKind_ATTACHMENT_KIND_FILE, "file_key"},
	"media":   {pb.AttachmentKind_ATTACHMENT_KIND_FILE, "file_key"},
}

// placeholderRe 兜底扫剩余的 @_user_N / @_all_N 占位符；
// 后跟的空白一并吃掉，U+00A0 是飞书客户端在 @ 后面塞的不换行空格。
var placeholderRe = regexp.MustCompile("@_(?:user|all)_\\d+[ \t ]*")

// microsecondFloor 是时间戳按微秒解释的下界（见 toTime）。1e14 落在毫秒上界与微秒下界之间。
const microsecondFloor = int64(100_000_000_000_000)

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

// firstNonEmpty 返回第一个非空字符串；全空返回 ""。
func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

func asMap(value any) map[string]any {
	if m, ok := value.(map[string]any); ok {
		return m
	}
	return map[string]any{}
}

func asList(value any) []any {
	if l, ok := value.([]any); ok {
		return l
	}
	return nil
}

// mapStr 取 m[key] 的字符串值；不是字符串（含缺失）返回 ""。
func mapStr(m map[string]any, key string) string {
	if s, ok := m[key].(string); ok {
		return s
	}
	return ""
}

// toTicks 把飞书的时间戳（字符串或数字）转成整数 ticks。
func toTicks(value any) (int64, bool) {
	switch v := value.(type) {
	case nil:
		return 0, false
	case string:
		if v == "" {
			return 0, false
		}
		n, err := strconv.ParseInt(v, 10, 64)
		if err != nil {
			return 0, false
		}
		return n, true
	case float64:
		// json.Unmarshal 把数字解成 float64；Python 的 int(value) 是截断。
		if math.IsNaN(v) || math.IsInf(v, 0) || math.Abs(v) >= math.MaxInt64 {
			return 0, false
		}
		return int64(v), true
	case json.Number:
		n, err := v.Int64()
		if err != nil {
			return 0, false
		}
		return n, true
	case int:
		return int64(v), true
	case int64:
		return v, true
	default:
		return 0, false
	}
}

// floorDiv / floorMod 复刻 Python 的 // 与 %（向下取整），Go 的 / 是向零截断。
func floorDivMod(n, d int64) (int64, int64) {
	q, r := n/d, n%d
	if r != 0 && (r < 0) != (d < 0) {
		q--
		r += d
	}
	return q, r
}

// toTime 把飞书的毫秒 / 微秒时间戳（字符串或整数）转成 UTC 时间。
//
// 单位得按量级判，因为官方文档自己就不是一个口径：事件订阅概述与
// card.action.trigger 回调的例子是 16 位微秒，而 im.message.receive_v1 自己那份
// 例子是 13 位毫秒、消息体里的 message.create_time 更是明写「毫秒」。
// 两者隔着两个数量级，按 1e14 分界不会误判。不判的话，微秒值除以 1000 得到的是
// 五万年后的秒数，Timestamp 当场非法，卡片回传事件（!stop / 证据按钮）会整条丢掉。
//
// 解析不出来返回 ok=false，让调用方回退到 now()：宁可时间不准，
// 也不要为了一个时间戳把整条事件丢掉。
func toTime(value any) (time.Time, bool) {
	ticks, ok := toTicks(value)
	if !ok {
		return time.Time{}, false
	}
	var unit int64 = 1000 // 毫秒
	if ticks >= microsecondFloor || ticks <= -microsecondFloor {
		unit = 1_000_000
	}
	sec, rem := floorDivMod(ticks, unit)
	nsec := rem * (1_000_000_000 / unit)
	t := time.Unix(sec, nsec).UTC()
	if err := timestamppb.New(t).CheckValid(); err != nil {
		// 平台给了个两种单位都解释不通的值。
		return time.Time{}, false
	}
	return t, true
}

// toInt64 把 content 里的 file_size 之类转成 int64（平台给的是字符串 "20480"）。
func toInt64(value any) (int64, bool) {
	return toTicks(value)
}

// mentionOpenID 取一条 mention 的 open_id。
//
// 两种形状都要吃：事件里 mentions[].id 是对象（{"open_id": ...}），
// 而「获取会话历史消息」返回的 mentions[].id 是裸字符串。
func mentionOpenID(mention map[string]any) string {
	switch ident := mention["id"].(type) {
	case map[string]any:
		return mapStr(ident, "open_id")
	case string:
		return ident
	default:
		return ""
	}
}

// loadMessageContent 解析 message.content：它是一段 JSON 字符串，失败时按空 content 处理。
func loadMessageContent(message map[string]any) map[string]any {
	switch raw := message["content"].(type) {
	case map[string]any:
		return raw
	case string:
		if raw == "" {
			return map[string]any{}
		}
		var parsed any
		if err := json.Unmarshal([]byte(raw), &parsed); err != nil {
			return map[string]any{}
		}
		if m, ok := parsed.(map[string]any); ok {
			return m
		}
		return map[string]any{}
	default:
		return map[string]any{}
	}
}

// mentionsOf 从 message.mentions 里挑出 dict 形状的条目。
func mentionsOf(message map[string]any) []map[string]any {
	var out []map[string]any
	for _, item := range asList(message["mentions"]) {
		if m, ok := item.(map[string]any); ok {
			out = append(out, m)
		}
	}
	return out
}

// applyMentions 把 @_user_N 占位符换成人话，并去掉 @Aite。
//
//   - @ 到机器人自己的那条：连同紧跟的空白一起删掉，text 里不留 @Aite。
//   - @ 到别人的：换成 @姓名，模型看到的是人名而不是 @_user_2。
//   - 兜底再扫一遍剩余占位符 —— mentions 缺项时也不能把 @_user_N 漏给模型。
func applyMentions(text string, mentions []map[string]any, botOpenID string) string {
	for _, mention := range mentions {
		key := mapStr(mention, "key")
		if key == "" {
			continue
		}
		if botOpenID != "" && mentionOpenID(mention) == botOpenID {
			re := regexp.MustCompile(regexp.QuoteMeta(key) + "[ \t ]*")
			text = re.ReplaceAllString(text, "")
			continue
		}
		name := mapStr(mention, "name")
		if name != "" {
			text = strings.ReplaceAll(text, key, "@"+name)
		} else {
			text = strings.ReplaceAll(text, key, "")
		}
	}
	return strings.TrimSpace(placeholderRe.ReplaceAllString(text, ""))
}

// flattenPost 把富文本（post）拍平成纯文本，顺带把内嵌图片的 image_key 收走。
//
// post 是「文本消息」的一种，所以给 text 填内容而不是留空 ——
// 群里真人发的排版消息不该在模型眼里变成一片空白。
//
// at 段原样吐占位符，剥 @Aite / 换人名一律交给 applyMentions：
// 官方文档写明 at 段的 user_id 是 @_user_N 序号而不是 open_id，
// 真实身份只能回 mentions 里查，而那正是 applyMentions 在干的事。
func flattenPost(content map[string]any, imageKeys *[]string) string {
	var lines []string
	if title, ok := content["title"].(string); ok && strings.TrimSpace(title) != "" {
		lines = append(lines, strings.TrimSpace(title))
	}

	for _, paragraph := range asList(content["content"]) {
		var parts []string
		for _, item := range asList(paragraph) {
			segment, ok := item.(map[string]any)
			if !ok {
				continue
			}
			switch mapStr(segment, "tag") {
			case "text", "md", "code_block":
				parts = append(parts, firstNonEmpty(mapStr(segment, "text"), mapStr(segment, "content")))
			case "a":
				label := mapStr(segment, "text")
				href := mapStr(segment, "href")
				if href != "" {
					parts = append(parts, "["+label+"]("+href+")")
				} else {
					parts = append(parts, label)
				}
			case "at":
				userID := mapStr(segment, "user_id")
				if strings.HasPrefix(userID, "@_") {
					parts = append(parts, userID) // 占位序号，交给 applyMentions
					break
				}
				// 文档说不该出现；真塞了个 open_id 进来也不能把它吐给模型看。
				if name := mapStr(segment, "user_name"); name != "" {
					parts = append(parts, "@"+name)
				} else {
					parts = append(parts, "")
				}
			case "img":
				if key := mapStr(segment, "image_key"); key != "" && imageKeys != nil {
					*imageKeys = append(*imageKeys, key)
				}
			case "emotion":
				parts = append(parts, mapStr(segment, "emoji_type"))
			}
		}
		if line := strings.TrimSpace(strings.Join(parts, "")); line != "" {
			lines = append(lines, line)
		}
	}

	return strings.Join(lines, "\n")
}

// senderKindOf 把 sender.sender_type 映射成 SenderKind。
func senderKindOf(senderType any) pb.SenderKind {
	s, ok := senderType.(string)
	if !ok {
		return senderKindFallback
	}
	if kind, ok := senderKindByType[strings.ToLower(s)]; ok {
		return kind
	}
	return senderKindFallback
}

// msgTypeOf 抹平事件面（message_type）与历史 API 面（msg_type）两种字段名。
func msgTypeOf(message map[string]any) string {
	return firstNonEmpty(mapStr(message, "message_type"), mapStr(message, "msg_type"))
}

// extractText 返回 (text, rawText, 内嵌图片的 image_key 列表)。
//
// text 是去掉 @Aite、strip 过的纯文本；rawText 保留 @ 之前的原样文本，
// 好让审计能看出用户到底打了什么。非文本消息 text=""、rawText=nil。
func extractText(message map[string]any, mentions []map[string]any, botOpenID string) (string, *string, []string) {
	msgType := msgTypeOf(message)
	if msgType != "text" && msgType != "post" {
		return "", nil, nil
	}

	content := loadMessageContent(message)
	if msgType == "text" {
		rawText := mapStr(content, "text")
		return applyMentions(rawText, mentions, botOpenID), &rawText, nil
	}

	// post：拍平一次做 rawText（占位符原样留着，跟 text 类消息的 rawText 同口径），
	// 再让 applyMentions 在它上面剥 @Aite、把别人的占位符换成人名，得到 text。
	var imageKeys []string
	rawText := flattenPost(content, &imageKeys)
	return applyMentions(rawText, mentions, botOpenID), &rawText, imageKeys
}

func attachmentsOf(message map[string]any, messageID string, imageKeys []string) []*pb.Attachment {
	var attachments []*pb.Attachment
	if mapping, ok := attachmentByMsgType[msgTypeOf(message)]; ok {
		content := loadMessageContent(message)
		if fileKey := mapStr(content, mapping.keyField); fileKey != "" {
			a := &pb.Attachment{
				Kind:      mapping.kind,
				FileKey:   fileKey,
				MessageId: messageID,
			}
			if name := mapStr(content, "file_name"); name != "" {
				a.Name = &name
			}
			// 平台给的是字符串 "20480"，转 int。
			if size, ok := toInt64(content["file_size"]); ok {
				a.Size = &size
			}
			if mime := mapStr(content, "mime_type"); mime != "" {
				a.Mime = &mime
			}
			attachments = append(attachments, a)
		}
	}
	for _, key := range imageKeys {
		attachments = append(attachments, &pb.Attachment{
			Kind:      pb.AttachmentKind_ATTACHMENT_KIND_IMAGE,
			FileKey:   key,
			MessageId: messageID,
		})
	}
	return attachments
}

// rawStruct 把原始事件整份搬进 structpb.Struct 供审计，剥掉两处校验令牌。
//
// header.token 与 event.token 都是应用的校验令牌，而信封会原样进
// NormalizedEvent.raw 落到审计里，不该跟着躺进证据文件。
// （Python 版只在 _envelope 里剥了 header.token，event.token 一路进了审计 ——
// 移植清单 §8 第 10 条点名的遗留缺口，这里一并修掉。）
func rawStruct(raw map[string]any) *structpb.Struct {
	cleaned := make(map[string]any, len(raw))
	for k, v := range raw {
		switch k {
		case "header", "event":
			if m, ok := v.(map[string]any); ok {
				inner := make(map[string]any, len(m))
				for ik, iv := range m {
					if ik == "token" {
						continue
					}
					inner[ik] = iv
				}
				cleaned[k] = inner
				continue
			}
			cleaned[k] = v
		default:
			cleaned[k] = v
		}
	}
	s, err := structpb.NewStruct(cleaned)
	if err != nil {
		// 平台给了 structpb 装不下的值（非 JSON 类型）。审计少一份原文，
		// 也好过为此把整条事件丢掉。
		return nil
	}
	return s
}

func valueStruct(value map[string]any) *structpb.Struct {
	s, err := structpb.NewStruct(value)
	if err != nil {
		return nil
	}
	return s
}

// ---------------------------------------------------------------------------
// 两个事件的归一化
// ---------------------------------------------------------------------------

// NormalizeMessage 把 im.message.receive_v1 归一化成 NormalizedEvent。
func NormalizeMessage(raw map[string]any, botOpenID, workspaceID, tenantID string) *pb.NormalizedEvent {
	header := asMap(raw["header"])
	event := asMap(raw["event"])
	message := asMap(event["message"])
	sender := asMap(event["sender"])

	messageID := mapStr(message, "message_id")
	mentions := mentionsOf(message)
	text, rawText, imageKeys := extractText(message, mentions, botOpenID)

	mentioned := false
	if botOpenID != "" {
		for _, m := range mentions {
			if mentionOpenID(m) == botOpenID {
				mentioned = true
				break
			}
		}
		if !mentioned {
			mentioned = postMentionsBot(message, mentions, botOpenID)
		}
	}

	// 话题锚点：按 root_id → parent_id → thread_id 取第一个非空。
	// 契约把 thread_id 说成「话题 root 消息 id」，root_id 正是它；顶层消息三个都没有 → nil。
	var threadID *string
	if tid := firstNonEmpty(
		mapStr(message, "root_id"),
		mapStr(message, "parent_id"),
		mapStr(message, "thread_id"),
	); tid != "" {
		threadID = &tid
	}

	chatType := pb.ChatType_CHAT_TYPE_GROUP
	if mapStr(message, "chat_type") == "p2p" {
		chatType = pb.ChatType_CHAT_TYPE_P2P
	}

	occurredAt, ok := toTime(message["create_time"])
	if !ok {
		if occurredAt, ok = toTime(header["create_time"]); !ok {
			occurredAt = timeNow()
		}
	}

	chatID := mapStr(message, "chat_id")
	return &pb.NormalizedEvent{
		EventId:     firstNonEmpty(mapStr(header, "event_id"), messageID),
		Kind:        pb.EventKind_EVENT_KIND_MESSAGE,
		Platform:    platformName,
		TenantId:    tenantID,
		WorkspaceId: firstNonEmpty(mapStr(header, "app_id"), workspaceID),
		ChatId:      chatID,
		ChatType:    chatType,
		SenderId:    mapStr(asMap(sender["sender_id"]), "open_id"),
		SenderKind:  senderKindOf(sender["sender_type"]),
		// sender_name 恒为 nil：只有 read_history 会填。
		SenderName: nil,
		Text:       text,
		RawText:    rawText,
		Mentioned:  mentioned,
		Anchor: &pb.Anchor{
			Platform:  platformName,
			ChatId:    chatID,
			MessageId: messageID,
			ThreadId:  threadID,
		},
		Attachments: attachmentsOf(message, messageID, imageKeys),
		CardAction:  nil,
		OccurredAt:  timestamppb.New(occurredAt),
		Raw:         rawStruct(raw),
	}
}

// postMentionsBot 判断 post 正文里的 at 段是不是 @ 到了机器人。
//
// at 段的 user_id 装的是 @_user_N 占位序号而不是 open_id（官方文档明写），
// 所以真实身份得拿这个序号回 mentions 里查 —— 直接跟 open_id 比永远不会相等。
//
// post 的 @ 正常也会出现在 message.mentions 里，调用方那条主判据已经覆盖了，
// 这里只是兜底：多花不了什么，而漏判一次 @ 就是一次没人应答的任务。
func postMentionsBot(message map[string]any, mentions []map[string]any, botOpenID string) bool {
	if msgTypeOf(message) != "post" {
		return false
	}
	botKeys := map[string]bool{}
	for _, m := range mentions {
		if key := mapStr(m, "key"); key != "" && mentionOpenID(m) == botOpenID {
			botKeys[key] = true
		}
	}
	content := loadMessageContent(message)
	for _, paragraph := range asList(content["content"]) {
		for _, item := range asList(paragraph) {
			segment, ok := item.(map[string]any)
			if !ok || mapStr(segment, "tag") != "at" {
				continue
			}
			userID := mapStr(segment, "user_id")
			// 占位序号回查 mentions；万一哪个客户端真塞了 open_id 进来，也一并认。
			if userID != "" && (botKeys[userID] || userID == botOpenID) {
				return true
			}
		}
	}
	return false
}

// NormalizeCardAction 把 card.action.trigger 归一化成 NormalizedEvent。
//
// action.value 不是 stop / evidence 的（比如别的按钮）返回 nil ——
// 契约的 CardAction.action 是两值枚举，塞别的值进去只会在下游炸掉。
func NormalizeCardAction(raw map[string]any, workspaceID, tenantID string) *pb.NormalizedEvent {
	header := asMap(raw["header"])
	event := asMap(raw["event"])
	context := asMap(event["context"])
	operator := asMap(event["operator"])
	action := asMap(event["action"])
	value := asMap(action["value"])

	var kind pb.CardActionKind
	switch mapStr(value, "action") {
	case "stop":
		kind = pb.CardActionKind_CARD_ACTION_KIND_STOP
	case "evidence":
		kind = pb.CardActionKind_CARD_ACTION_KIND_EVIDENCE
	default:
		return nil
	}

	cardID := mapStr(context, "open_message_id")
	chatID := mapStr(context, "open_chat_id")
	occurredAt, ok := toTime(header["create_time"])
	if !ok {
		occurredAt = timeNow()
	}

	var taskID *string
	// 空串 → nil（Python 的 value.get("task_id") or None）。
	if tid := mapStr(value, "task_id"); tid != "" {
		taskID = &tid
	}

	return &pb.NormalizedEvent{
		EventId:     firstNonEmpty(mapStr(header, "event_id"), cardID),
		Kind:        pb.EventKind_EVENT_KIND_CARD_ACTION,
		Platform:    platformName,
		TenantId:    tenantID,
		WorkspaceId: firstNonEmpty(mapStr(header, "app_id"), workspaceID),
		ChatId:      chatID,
		// 卡片回传事件不带 chat_type。P0 的卡片只发在群里，且路由规则 R3 先于任何
		// chat_type 判定命中，这里固定 group。
		ChatType: pb.ChatType_CHAT_TYPE_GROUP,
		SenderId: mapStr(operator, "open_id"),
		// 点按钮的一定是真人；机器人点不动卡片。
		SenderKind: pb.SenderKind_SENDER_KIND_HUMAN,
		SenderName: nil,
		Text:       "",
		RawText:    nil,
		// 点的是我们自己发的卡片，等价于「冲着 Aite 来的」。
		Mentioned: true,
		Anchor: &pb.Anchor{
			Platform:  platformName,
			ChatId:    chatID,
			MessageId: cardID,
			// 卡片回传只给得到卡片自己那条消息，话题归属由 core 按 task_id 查。
			ThreadId: nil,
		},
		Attachments: nil,
		CardAction: &pb.CardAction{
			CardId: cardID,
			Action: kind,
			TaskId: taskID,
			Value:  valueStruct(value),
		},
		OccurredAt: timestamppb.New(occurredAt),
		Raw:        rawStruct(raw),
	}
}

// ---------------------------------------------------------------------------
// CC8 的新事件：撤回、入群、成员进出、表情
// ---------------------------------------------------------------------------

// chatEvent 是撤回 / 入群 / 成员事件共用的形状。
//
// 形状规则（违反就在 core 被判 INVALID_ARGUMENT，而 edge 的 ingress 对它是静默丢）：
//   - event_id 只取 header.event_id，缺了返回 nil（别拿 message_id 顶替：入群事件根本没有
//     message_id，拿别的 id 冒充会让 core 的去重把两条不同事件当成一条）；
//   - anchor 必须有；没有触发消息的事件 message_id 留空 —— 下游把 anchor.message_id
//     当 reply_to 用，塞 event_id 进去就是回复一条不存在的消息；
//   - 事件体里没有 chat_type，一律 GROUP（这几类事件只在群里发生）；
//   - occurred_at 取 header.create_time，解析不了回退 now。
func chatEvent(raw map[string]any, workspaceID, tenantID string, kind pb.EventKind, chatID, messageID, senderID string) *pb.NormalizedEvent {
	header := asMap(raw["header"])
	eventID := mapStr(header, "event_id")
	if eventID == "" {
		return nil
	}
	occurredAt, ok := toTime(header["create_time"])
	if !ok {
		occurredAt = timeNow()
	}
	return &pb.NormalizedEvent{
		EventId:     eventID,
		Kind:        kind,
		Platform:    platformName,
		TenantId:    tenantID,
		WorkspaceId: firstNonEmpty(mapStr(header, "app_id"), workspaceID),
		ChatId:      chatID,
		ChatType:    pb.ChatType_CHAT_TYPE_GROUP,
		SenderId:    senderID,
		// 非 HUMAN 在 core 的 R1 就被丢，到不了记这些事件的 R4。
		SenderKind: pb.SenderKind_SENDER_KIND_HUMAN,
		Text:       "",
		Mentioned:  false,
		Anchor: &pb.Anchor{
			Platform:  platformName,
			ChatId:    chatID,
			MessageId: messageID,
		},
		OccurredAt: timestamppb.New(occurredAt),
		Raw:        rawStruct(raw),
	}
}

// normalizeRecalled 把 im.message.recalled_v1 归一化成 MESSAGE_DELETED。
//
// 字段（P2MessageRecalledV1Data，model.go:16640）：event.message_id / event.chat_id。
// 事件体里没有操作者（只有 message_id / chat_id / recall_time / recall_type），撤回的人
// 是发送者本人或群主 / 管理员，一律记 HUMAN、sender_id 留空。
func normalizeRecalled(raw map[string]any, _, workspaceID, tenantID string) *pb.NormalizedEvent {
	event := asMap(raw["event"])
	return chatEvent(raw, workspaceID, tenantID, pb.EventKind_EVENT_KIND_MESSAGE_DELETED,
		mapStr(event, "chat_id"), mapStr(event, "message_id"), "")
}

// normalizeBotAdded 把 im.chat.member.bot.added_v1 归一化成 BOT_ADDED。
//
// 字段（P2ChatMemberBotAddedV1Data，model.go:16498）：event.chat_id / event.operator_id.open_id。
func normalizeBotAdded(raw map[string]any, _, workspaceID, tenantID string) *pb.NormalizedEvent {
	event := asMap(raw["event"])
	return chatEvent(raw, workspaceID, tenantID, pb.EventKind_EVENT_KIND_BOT_ADDED,
		mapStr(event, "chat_id"), "", mapStr(asMap(event["operator_id"]), "open_id"))
}

// normalizeMemberChanged 把 im.chat.member.user.added_v1 / deleted_v1 归一化成 MEMBER_CHANGED。
//
// 字段（P2ChatMemberUserAddedV1Data model.go:16546、P2ChatMemberUserDeletedV1Data model.go:16572）：
// event.chat_id / event.operator_id.open_id。加入还是退出、谁进谁出（event.users[]）契约里
// 没有字段装，只留在 raw 里（契约缺口见回执）。
func normalizeMemberChanged(raw map[string]any, _, workspaceID, tenantID string) *pb.NormalizedEvent {
	event := asMap(raw["event"])
	return chatEvent(raw, workspaceID, tenantID, pb.EventKind_EVENT_KIND_MEMBER_CHANGED,
		mapStr(event, "chat_id"), "", mapStr(asMap(event["operator_id"]), "open_id"))
}

// logMalformed 是新事件归一化成 nil（缺 header.event_id）时的日志。
func logMalformed(logger *slog.Logger, raw map[string]any) {
	logger.Debug("feishu.event_malformed",
		"type", mapStr(asMap(raw["header"]), "event_type"), "reason", "缺 header.event_id")
}

// reactionEvent 是表情回复事件解析出来的样子。暂不上送：契约里还没有 EventKind::Reaction
// （T0 计划里加），DD9 接上之前只解析、打 Debug。
//
// 字段（P2MessageReactionCreatedV1Data model.go:16676、P2MessageReactionDeletedV1Data model.go:16700）：
// event.message_id、event.reaction_type.emoji_type（Emoji，model.go:2751）、event.operator_type
// （user / app）、event.user_id.open_id（UserId，model.go:8453；operator_type=user 时有）、
// event.app_id（operator_type=app 时有）、event.action_time（毫秒）。
type reactionEvent struct {
	EventID      string
	MessageID    string
	EmojiType    string
	OperatorType string
	// OperatorID 是 user_id.open_id（用户）或 app_id（应用）。
	OperatorID string
	// Added 区分增 / 删：created_v1 → true，deleted_v1 → false。
	Added bool
}

// parseReaction 解析表情回复事件；不是这两类事件返回 ok=false。
func parseReaction(raw map[string]any) (reactionEvent, bool) {
	header := asMap(raw["header"])
	var added bool
	switch mapStr(header, "event_type") {
	case eventReactionCreated:
		added = true
	case eventReactionDeleted:
		added = false
	default:
		return reactionEvent{}, false
	}
	event := asMap(raw["event"])
	return reactionEvent{
		EventID:      mapStr(header, "event_id"),
		MessageID:    mapStr(event, "message_id"),
		EmojiType:    mapStr(asMap(event["reaction_type"]), "emoji_type"),
		OperatorType: mapStr(event, "operator_type"),
		OperatorID:   firstNonEmpty(mapStr(asMap(event["user_id"]), "open_id"), mapStr(event, "app_id")),
		Added:        added,
	}, true
}

// dropReaction 是表情事件的归一化：恒为 nil（不上送、不报错、不重推）。
func dropReaction(map[string]any, string, string, string) *pb.NormalizedEvent { return nil }

func logReactionDropped(logger *slog.Logger, raw map[string]any) {
	r, _ := parseReaction(raw)
	logger.Debug("feishu.reaction_dropped",
		"event_id", r.EventID, "message_id", r.MessageID, "emoji_type", r.EmojiType,
		"operator_type", r.OperatorType, "added", r.Added)
}

// timeNow 是 time.Now 的可替换钩子（测试里钉住回退时间）。
var timeNow = func() time.Time { return time.Now().UTC() }
