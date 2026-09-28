// 出站文本与文件下载。
//
// SendText 路由：
//   - 会话缓存里有没过期的 sessionWebhook（到期前留 60 秒余量）→ POST 它（markdown）；
//     这条路不给消息 id，SendResult.MessageId 留空（core 今天不读 send_text 的返回值）。
//   - 否则走机器人 API：群 → groupMessages/send；单聊 → oToMessages/batchSend（userIds = 缓存的 senderStaffId）。
//
// ReplyTo / InThread 钉钉没有对应物，忽略（docs 写明）。
package dingtalk

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// webhookSafety 是 sessionWebhook 到期前留的余量。
const webhookSafety = 60 * time.Second

// markdownTitleRunes 是 markdown 消息 title（会话列表里的摘要）最多取多少字。
const markdownTitleRunes = 20

func (p *Platform) SendText(ctx context.Context, msg *pb.OutboundText) (*pb.SendResult, error) {
	if msg == nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Msg: "SendText 收到空消息", Retryable: false}
	}
	chatID := msg.GetChatId()
	text := msg.GetText()
	title := markdownTitle(text)

	info, known := p.sessions.get(chatID)
	if known && info.webhook != "" && p.clock().Add(webhookSafety).Before(info.webhookExpiresAt) {
		if err := p.sendViaWebhook(ctx, info.webhook, title, text); err != nil {
			return nil, err
		}
		return &pb.SendResult{MessageId: ""}, nil
	}

	param, err := json.Marshal(map[string]string{"title": title, "text": text})
	if err != nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Retryable: false, Msg: err.Error()}
	}
	body := map[string]any{
		"robotCode": p.opts.RobotCode,
		"msgKey":    "sampleMarkdown",
		"msgParam":  string(param),
	}
	// 缓存里没有这个会话（没收过它的消息）→ 按群处理：主动发只有群接口能用 chat_id 寻址。
	path := PathGroupSend
	if known && info.conversationType == conversationP2P {
		if info.staffID == "" {
			return nil, &aiteerr.PlatformError{Code: "no_recipient", Retryable: false, Msg: "单聊会话缺 senderStaffId，无法走 oToMessages"}
		}
		path = PathOToBatchSend
		body["userIds"] = []string{info.staffID}
	} else {
		body["openConversationId"] = chatID
	}
	resp, err := p.api.call(ctx, http.MethodPost, path, body)
	if err != nil {
		return nil, err
	}
	return &pb.SendResult{MessageId: mapStr(resp, "processQueryKey")}, nil
}

// sendViaWebhook POST 到 sessionWebhook。URL 带会话令牌：错误与日志里只出现去掉 query 的形式。
func (p *Platform) sendViaWebhook(ctx context.Context, webhook, title, text string) error {
	status, data, err := p.api.send(ctx, http.MethodPost, webhook, map[string]any{
		"msgtype":  "markdown",
		"markdown": map[string]string{"title": title, "text": text},
	}, "")
	if err != nil {
		return err
	}
	if status >= 400 {
		return errorFromResponse(status, data)
	}
	parsed := jsonBody(data)
	if code := mapStr(parsed, "errcode"); code != "" && code != "0" {
		return &aiteerr.PlatformError{Code: code, HTTPStatus: status, Retryable: false, Msg: mapStr(parsed, "errmsg")}
	}
	return nil
}

// markdownTitle 取正文第一行非空文本的前若干字；全空用 "Aite"。
func markdownTitle(text string) string {
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(strings.TrimLeft(strings.TrimSpace(line), "#*> -"))
		if line != "" {
			return truncateRunes(line, markdownTitleRunes)
		}
	}
	return "Aite"
}

// DownloadFile 用 downloadCode 换临时下载地址，再 GET 取字节（不带 token 头）。
//
// 单聊才收得到文件；群里只有图片走这条。messageID 钉钉用不上（downloadCode 自带定位）。
func (p *Platform) DownloadFile(ctx context.Context, _ string, fileKey string) ([]byte, error) {
	resp, err := p.api.call(ctx, http.MethodPost, PathMessageFileDL, map[string]any{
		"downloadCode": fileKey,
		"robotCode":    p.opts.RobotCode,
	})
	if err != nil {
		return nil, err
	}
	downloadURL := mapStr(resp, "downloadUrl")
	if downloadURL == "" {
		return nil, &aiteerr.PlatformError{Code: "no_download_url", Retryable: false, Msg: "响应里没有 downloadUrl"}
	}
	status, data, err := p.api.send(ctx, http.MethodGet, downloadURL, nil, "")
	if err != nil {
		return nil, err
	}
	if status >= 400 {
		return nil, errorFromResponse(status, data)
	}
	return data, nil
}
