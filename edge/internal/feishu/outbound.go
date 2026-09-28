package feishu

import (
	"context"
	"fmt"
	"net/http"
	"strings"

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
func (p *Platform) sendMessage(ctx context.Context, chatID string, replyTo *string, msgType, content string, inThread bool) (string, error) {
	var (
		data map[string]any
		err  error
	)
	if replyTo != nil && *replyTo != "" {
		data, _, err = p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   fmt.Sprintf(PathMessageReply, *replyTo),
			body: map[string]any{
				"content": content, "msg_type": msgType, "reply_in_thread": inThread,
			},
			rateLimited: true,
		})
	} else {
		data, _, err = p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   PathMessages,
			params: map[string]string{"receive_id_type": "chat_id"},
			body: map[string]any{
				"receive_id": chatID, "msg_type": msgType, "content": content,
			},
			rateLimited: true,
		})
	}
	if err != nil {
		return "", err
	}
	return mapStr(data, "message_id"), nil
}

func (p *Platform) SendText(ctx context.Context, msg *pb.OutboundText) (*pb.SendResult, error) {
	// pb 的 getter 是 nil-safe 的，但 msg.ReplyTo 是裸字段 —— gRPC 请求里没塞 msg
	// 就会 nil 解引用，把整个 edge 进程带走。
	if msg == nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Msg: "SendText 收到空消息", Retryable: false}
	}
	content := DumpsCard(BuildMarkdownCard(msg.GetText()))
	messageID, err := p.sendMessage(ctx, msg.GetChatId(), msg.ReplyTo, "interactive", content, msg.GetInThread())
	if err != nil {
		return nil, err
	}
	return &pb.SendResult{MessageId: messageID}, nil
}

func (p *Platform) SendCard(ctx context.Context, chatID string, replyTo *string, card *pb.ChecklistCard) (*pb.SendResult, error) {
	content := DumpsCard(BuildChecklistCard(card))
	// send_card 的 in_thread 写死 true，只有 SendText 透传。
	messageID, err := p.sendMessage(ctx, chatID, replyTo, "interactive", content, true)
	if err != nil {
		return nil, err
	}
	cardID := messageID
	return &pb.SendResult{MessageId: messageID, CardId: &cardID}, nil
}

// UpdateCard 原地更新同一条卡片消息。
//
// 必须是 PATCH /open-apis/im/v1/messages/{card_id}（「更新应用发送的消息卡片」）。
// 任何时候都不许退化成再发一条 —— 「过程中卡片至少更新 3 次且不新增消息」就靠这条。
func (p *Platform) UpdateCard(ctx context.Context, cardID string, card *pb.ChecklistCard) error {
	_, _, err := p.api.request(ctx, apiRequest{
		method:      http.MethodPatch,
		path:        fmt.Sprintf(PathMessage, cardID),
		body:        map[string]any{"content": DumpsCard(BuildChecklistCard(card))},
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

	messageID, err := p.sendMessage(ctx, msg.GetChatId(), msg.ReplyTo, msgType, content, true)
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
