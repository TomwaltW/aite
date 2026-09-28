package feishu

import (
	"context"
	"crypto/rand"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// KnownEmojiTypes 是「表情文案说明」里确实存在的 emoji_type，只列本包可能用到的几个。
// 测试拿它兜住「别再往 ReactionEmoji 里写一个清单外的 key」。
var KnownEmojiTypes = map[string]bool{
	"OnIt": true, "DONE": true, "CRY": true,
	"GLANCE": true, "THUMBSUP": true, "MUSCLE": true, "OK": true,
}

// ReactionEmoji 是 ReactionKind → 飞书表情 key（「添加消息表情回复」的 reaction_type.emoji_type）。
//
// 取值必须落在官方那份固定清单里，不在清单里的会被打回 231001 表情类型不合法。
// T16 拿清单逐个核过：DONE / CRY 在清单里；早期写的 EYES 不在 —— 有 eyes 的是云文档
// 高亮块那套小写枚举，跟消息表情回复不是一套，照原样发上去每一次 ack 都会 400。
// 换成清单里的 OnIt，语义正好是「收到，正在处理」。
var ReactionEmoji = map[pb.ReactionKind]string{
	pb.ReactionKind_REACTION_KIND_ACK:  "OnIt",
	pb.ReactionKind_REACTION_KIND_DONE: "DONE",
	pb.ReactionKind_REACTION_KIND_FAIL: "CRY",
}

// fileTypeByExt 是「上传文件」接口的 file_type 取值，按扩展名挑；认不出就 stream。
var fileTypeByExt = map[string]string{
	".opus": "opus", ".mp4": "mp4", ".pdf": "pdf",
	".doc": "doc", ".docx": "doc",
	".xls": "xls", ".xlsx": "xls",
	".ppt": "ppt", ".pptx": "ppt",
}

// ------------------------------------------------------------------
// 出站
// ------------------------------------------------------------------

// sendMessage 发一条消息，返回 message_id。
//
// 有 replyTo 就走「回复消息」接口（带 reply_in_thread 才进话题），否则走「发送消息」。
//
// 先过该群的桶（每群 5 QPS，见 chatPacer），再由 api.request 过全局桶（outbound_rate_per_min）。
// 请求体带 uuid（飞书按它对「发送 / 回复」去重）：每次 sendMessage 生成一次，
// rawRequest 的重试复用同一个请求体、也就复用同一个 uuid —— 5xx 之后重试不会重复发出同一条。
// dedupeKey 非空时由它当 uuid（留给 DD10 接 OutboundText.dedupe_key；飞书要求 ≤50 字符）。
func (p *Platform) sendMessage(ctx context.Context, chatID string, replyTo *string, msgType, content string, inThread bool, dedupeKey string) (string, error) {
	if err := p.pacer.acquire(ctx, chatID); err != nil {
		return "", &aiteerr.PlatformError{Code: "transport_error", Retryable: true, Msg: err.Error()}
	}
	uuid := dedupeKey
	if uuid == "" {
		uuid = newMessageUUID()
	}

	var (
		data map[string]any
		err  error
	)
	if replyTo != nil && *replyTo != "" {
		data, _, err = p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   fmt.Sprintf(PathMessageReply, *replyTo),
			body: map[string]any{
				"content": content, "msg_type": msgType, "reply_in_thread": inThread, "uuid": uuid,
			},
			rateLimited: true,
		})
	} else {
		data, _, err = p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   PathMessages,
			params: map[string]string{"receive_id_type": "chat_id"},
			body: map[string]any{
				"receive_id": chatID, "msg_type": msgType, "content": content, "uuid": uuid,
			},
			rateLimited: true,
		})
	}
	if err != nil {
		return "", err
	}
	return mapStr(data, "message_id"), nil
}

// newMessageUUID 生成发送请求的去重 uuid：crypto/rand 的 26 字符 base32（≤50 字符）。
func newMessageUUID() string { return rand.Text() }

// ------------------------------------------------------------------
// 每群限速
// ------------------------------------------------------------------

const (
	// perChatRatePerMin / perChatBurst：飞书对同一个群的发送限 5 QPS（全局额度另算）。
	perChatRatePerMin = 300
	perChatBurst      = 5
	// chatBucketsMax 是每群桶表的上限。超了先淘汰闲置满 chatBucketIdle 的（它们已经补满，
	// 丢掉再建一个等价），还超就淘汰最久没用的那个。
	chatBucketsMax = 1024
	// chatBucketIdle：5 个令牌按 5/s 补，闲 1s 就满了。
	chatBucketIdle = time.Second
	// cardChatsMax 是 card_id → chat_id 记账的上限（UpdateCard 靠它找群），先进先出。
	cardChatsMax = 4096
)

// chatPacer 是每群一个令牌桶的限速器，加上 card_id → chat_id 的有界记账。
type chatPacer struct {
	clock clockFunc
	sleep sleeperFunc

	mu        sync.Mutex
	buckets   map[string]*chatBucket
	cardChat  map[string]string
	cardOrder []string
}

type chatBucket struct {
	bucket   *TokenBucket
	lastUsed time.Time
}

func newChatPacer(clock clockFunc, sleep sleeperFunc) *chatPacer {
	return &chatPacer{
		clock: clock, sleep: sleep,
		buckets:  map[string]*chatBucket{},
		cardChat: map[string]string{},
	}
}

// acquire 过 chatID 那个群的桶；chatID 空就跳过。不在表锁里等（等的是桶自己的锁）。
func (c *chatPacer) acquire(ctx context.Context, chatID string) error {
	if chatID == "" {
		return nil
	}
	b, err := c.bucketFor(chatID)
	if err != nil {
		return err
	}
	_, err = b.Acquire(ctx, 1)
	return err
}

func (c *chatPacer) bucketFor(chatID string) (*TokenBucket, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	now := c.clock()
	if cb, ok := c.buckets[chatID]; ok {
		cb.lastUsed = now
		return cb.bucket, nil
	}
	if len(c.buckets) >= chatBucketsMax {
		c.evictLocked(now)
	}
	bucket, err := NewTokenBucket(perChatRatePerMin, perChatBurst, c.clock, c.sleep)
	if err != nil {
		return nil, err
	}
	c.buckets[chatID] = &chatBucket{bucket: bucket, lastUsed: now}
	return bucket, nil
}

func (c *chatPacer) evictLocked(now time.Time) {
	for id, cb := range c.buckets {
		if now.Sub(cb.lastUsed) >= chatBucketIdle {
			delete(c.buckets, id)
		}
	}
	for len(c.buckets) >= chatBucketsMax {
		oldestID := ""
		var oldest time.Time
		for id, cb := range c.buckets {
			if oldestID == "" || cb.lastUsed.Before(oldest) {
				oldestID, oldest = id, cb.lastUsed
			}
		}
		delete(c.buckets, oldestID)
	}
}

// rememberCard 记下卡片发在哪个群（SendCard 成功后调）。
func (c *chatPacer) rememberCard(cardID, chatID string) {
	if cardID == "" || chatID == "" {
		return
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if _, ok := c.cardChat[cardID]; !ok {
		c.cardOrder = append(c.cardOrder, cardID)
	}
	c.cardChat[cardID] = chatID
	for len(c.cardOrder) > cardChatsMax {
		delete(c.cardChat, c.cardOrder[0])
		c.cardOrder = c.cardOrder[1:]
	}
}

// chatOfCard 查卡片所在的群；查不到返回 ""（UpdateCard 就只过全局桶）。
func (c *chatPacer) chatOfCard(cardID string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.cardChat[cardID]
}

func (p *Platform) SendText(ctx context.Context, msg *pb.OutboundText) (*pb.SendResult, error) {
	// pb 的 getter 是 nil-safe 的，但 msg.ReplyTo 是裸字段 —— gRPC 请求里没塞 msg
	// 就会 nil 解引用，把整个 edge 进程带走。
	if msg == nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Msg: "SendText 收到空消息", Retryable: false}
	}
	content := DumpsCard(BuildMarkdownCard(msg.GetText()))
	messageID, err := p.sendMessage(ctx, msg.GetChatId(), msg.ReplyTo, "interactive", content, msg.GetInThread(), "")
	if err != nil {
		return nil, err
	}
	return &pb.SendResult{MessageId: messageID}, nil
}

func (p *Platform) SendCard(ctx context.Context, chatID string, replyTo *string, card *pb.ChecklistCard) (*pb.SendResult, error) {
	content := DumpsCard(buildChecklistCardWith(card, p.cardButtons))
	// send_card 的 in_thread 写死 true，只有 SendText 透传。
	messageID, err := p.sendMessage(ctx, chatID, replyTo, "interactive", content, true, "")
	if err != nil {
		return nil, err
	}
	cardID := messageID
	p.pacer.rememberCard(cardID, chatID)
	return &pb.SendResult{MessageId: messageID, CardId: &cardID}, nil
}

// UpdateCard 原地更新同一条卡片消息。
//
// 必须是 PATCH /open-apis/im/v1/messages/{card_id}（「更新应用发送的消息卡片」）。
// 任何时候都不许退化成再发一条 —— 「过程中卡片至少更新 3 次且不新增消息」就靠这条。
//
// 签名里没有 chat id：本进程 SendCard 过的卡片查得到它的群，过该群的桶；
// 查不到（进程重启前发的卡片）只过全局桶。PATCH 请求体不带 uuid（只有 content）。
func (p *Platform) UpdateCard(ctx context.Context, cardID string, card *pb.ChecklistCard) error {
	if err := p.pacer.acquire(ctx, p.pacer.chatOfCard(cardID)); err != nil {
		return &aiteerr.PlatformError{Code: "transport_error", Retryable: true, Msg: err.Error()}
	}
	_, _, err := p.api.request(ctx, apiRequest{
		method:      http.MethodPatch,
		path:        fmt.Sprintf(PathMessage, cardID),
		body:        map[string]any{"content": DumpsCard(buildChecklistCardWith(card, p.cardButtons))},
		rateLimited: true,
	})
	return err
}

// SendFile 先上传再发送。图片走 images 接口，其余走 files 接口。上传不过令牌桶。
func (p *Platform) SendFile(ctx context.Context, msg *pb.OutboundFile) (*pb.SendResult, error) {
	if msg == nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Msg: "SendFile 收到空消息", Retryable: false}
	}
	var (
		content string
		msgType string
	)
	if strings.HasPrefix(msg.GetMime(), "image/") {
		data, _, err := p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   PathImages,
			file:   &uploadFile{field: "image", filename: msg.GetName(), mime: msg.GetMime(), data: msg.GetData()},
			form:   map[string]string{"image_type": "message"},
		})
		if err != nil {
			return nil, err
		}
		content = DumpsCard(map[string]any{"image_key": data["image_key"]})
		msgType = "image"
	} else {
		data, _, err := p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   PathFiles,
			file:   &uploadFile{field: "file", filename: msg.GetName(), mime: msg.GetMime(), data: msg.GetData()},
			form:   map[string]string{"file_type": fileTypeOf(msg.GetName()), "file_name": msg.GetName()},
		})
		if err != nil {
			return nil, err
		}
		content = DumpsCard(map[string]any{"file_key": data["file_key"]})
		msgType = "file"
	}

	messageID, err := p.sendMessage(ctx, msg.GetChatId(), msg.ReplyTo, msgType, content, true, "")
	if err != nil {
		return nil, err
	}
	return &pb.SendResult{MessageId: messageID}, nil
}

func (p *Platform) AddReaction(ctx context.Context, messageID string, kind pb.ReactionKind) error {
	emoji, ok := ReactionEmoji[kind]
	if !ok {
		return &aiteerr.PlatformError{
			Code: "bad_reaction", Retryable: false, Msg: fmt.Sprintf("未知表情类型 %s", kind),
		}
	}
	_, _, err := p.api.request(ctx, apiRequest{
		method:      http.MethodPost,
		path:        fmt.Sprintf(PathMessageReaction, messageID),
		body:        map[string]any{"reaction_type": map[string]any{"emoji_type": emoji}},
		rateLimited: true,
	})
	return err
}

func fileTypeOf(name string) string {
	i := strings.LastIndex(name, ".")
	if i < 0 || i == len(name)-1 {
		return "stream"
	}
	if t, ok := fileTypeByExt[strings.ToLower(name[i:])]; ok {
		return t
	}
	return "stream"
}
