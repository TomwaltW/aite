// 主动发送（aibot_send_msg）的两道闸：只发给来过消息的会话、每会话任意 60 秒内最多 30 条。
//
// 不用 x/time/rate：它是令牌桶，按 0.5/s、突发 30 配，头一分钟能放过约 59 条，
// 守不住平台的「每分钟 30 条」。这里是滑动窗口：记最近 30 次发送时刻（环形），
// 第 31 次时若其中最早那次距今 < 60 s 就拒。capabilities.proto 的 outbound_rate_per_min
// 注释里写的「令牌桶」只是泛指，以本文件为准。
//
// 本轨只对主动发送计数；回复（流式）是否计入同一预算（plan:210）记账转给 DD11 / FF3。
package wecom

import (
	"context"
	"time"

	"aite/edge/internal/aiteerr"
)

// 每会话主动发送上限（原卡「每会话 30 条/分钟」）。
const (
	proactivePerWindow = 30
	proactiveWindow    = time.Minute
)

// chatBookCap 是「来过消息的会话」与发送窗口两张表的容量。
const chatBookCap = 10000

// sendWindow 是一个会话最近 proactivePerWindow 次发送时刻的环形缓冲。
type sendWindow struct {
	times [proactivePerWindow]time.Time
	n     int // 已记次数（封顶 proactivePerWindow）
	next  int // 下一个写入位置；满了之后它也是最早那次
}

// allow 判断此刻能不能再发一条；能就记下。
func (w *sendWindow) allow(now time.Time) bool {
	if w.n == proactivePerWindow && now.Sub(w.times[w.next]) < proactiveWindow {
		return false
	}
	w.times[w.next] = now
	w.next = (w.next + 1) % proactivePerWindow
	if w.n < proactivePerWindow {
		w.n++
	}
	return true
}

// chatBook 记「来过消息的会话」与每会话的发送窗口。调用方持 Platform.mu。
type chatBook struct {
	seen    *boundedMap[struct{}]
	windows *boundedMap[*sendWindow]
}

func newChatBook(capacity int) *chatBook {
	return &chatBook{
		seen:    newBoundedMap[struct{}](capacity),
		windows: newBoundedMap[*sendWindow](capacity),
	}
}

func (b *chatBook) markSeen(chatID string) {
	if chatID == "" {
		return
	}
	b.seen.put(chatID, struct{}{})
}

func (b *chatBook) hasSeen(chatID string) bool {
	_, ok := b.seen.get(chatID)
	return ok
}

func (b *chatBook) allow(chatID string, now time.Time) bool {
	w, ok := b.windows.get(chatID)
	if !ok {
		w = &sendWindow{}
		b.windows.put(chatID, w)
	}
	return w.allow(now)
}

// checkProactive 过两道闸；不过就原样返回错误（调用方一帧不发）。
func (p *Platform) checkProactive(chatID string) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.chats.hasSeen(chatID) {
		return &aiteerr.PlatformError{
			Code: "proactive_not_allowed", Retryable: false,
			Msg: "企微只允许主动发给给 bot 发过消息的会话：" + chatID,
		}
	}
	if !p.chats.allow(chatID, p.clock()) {
		// Retryable → UNAVAILABLE，core 会重试。
		return &aiteerr.PlatformError{
			Code: "rate_limited", Retryable: true,
			Msg: "企微每会话 60 秒内最多主动发送 30 条：" + chatID,
		}
	}
	return nil
}

// sendProactive 过闸后发一帧 aibot_send_msg，返回 "nostream-" + 本帧 req_id
// （本地生成，不依赖应答里有没有 msgid；前缀让 UpdateCard 认出它是非流式）。
func (p *Platform) sendProactive(ctx context.Context, chatID string, body any) (string, error) {
	conn, err := p.currentConn()
	if err != nil {
		return "", err
	}
	if err := p.checkProactive(chatID); err != nil {
		return "", err
	}
	reqID := newReqID()
	if _, err := conn.request(ctx, cmdSendMsg, reqID, body); err != nil {
		return "", err
	}
	return noStreamPrefix + reqID, nil
}

// sendProactiveMarkdown 主动发一帧 markdown。
func (p *Platform) sendProactiveMarkdown(ctx context.Context, chatID, content string) (string, error) {
	return p.sendProactive(ctx, chatID, sendMarkdownBody{
		ChatID: chatID, MsgType: msgTypeMD, Markdown: textBody{Content: content},
	})
}
