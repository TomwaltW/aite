// AI 卡片：createAndDeliver 投放、实例更新（UpdateCard）、流式更新（StreamCard）。
//
// 模板变量（H9 建模板须知，见 docs/p1/dingtalk.md）：content / task_id / task_no / status；
// Stop 按钮回传参数 {"action":"stop","task_id":"${task_id}"}。
//
// 本文件不叫 stream_card.go —— 那个名字留给 FF3 的增量流式。
package dingtalk

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"unicode/utf8"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// streamChunkBytes 是 PUT /v1.0/card/streaming 每次 content 的上限（≤ 1 KB）。
const streamChunkBytes = 1024

// cardRoute 是 outTrackId → 卡片所在会话，给卡片回调填 ChatId / ChatType / WorkspaceId。
type cardRoute struct {
	chatID   string
	chatType pb.ChatType
	corpID   string
}

type cardRoutes struct {
	mu   sync.Mutex
	byID map[string]cardRoute
}

func newCardRoutes() *cardRoutes { return &cardRoutes{byID: map[string]cardRoute{}} }

func (r *cardRoutes) put(id string, route cardRoute) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.byID[id] = route
}

func (r *cardRoutes) get(id string) (cardRoute, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	route, ok := r.byID[id]
	return route, ok
}

// randomHex 返回 n 字节的随机 hex（2n 个字符）。
func randomHex(n int) string {
	buf := make([]byte, n)
	if _, err := rand.Read(buf); err != nil {
		// crypto/rand 在受支持平台上不会失败；真失败了宁可 panic 也别发重复 id。
		panic(fmt.Sprintf("crypto/rand: %v", err))
	}
	return hex.EncodeToString(buf)
}

func newOutTrackID() string { return "aite-" + randomHex(8) }

// SendCard 投放一张 AI 卡片。replyTo 钉钉没有对应物，忽略。
func (p *Platform) SendCard(ctx context.Context, chatID string, _ *string, card *pb.ChecklistCard) (*pb.SendResult, error) {
	if p.opts.CardTemplateID == "" {
		return nil, &aiteerr.PlatformError{Code: "no_card_template", Retryable: false, Msg: "没配 AI 卡片模板 id，发不了卡片"}
	}
	info, _ := p.sessions.get(chatID)
	outTrackID := newOutTrackID()
	body := map[string]any{
		"cardTemplateId": p.opts.CardTemplateID,
		"outTrackId":     outTrackID,
		"callbackType":   "STREAM",
		"cardData":       map[string]any{"cardParamMap": cardParamMap(card)},
	}
	route := cardRoute{chatID: chatID, corpID: info.corpID}
	if info.conversationType == conversationP2P {
		if info.staffID == "" {
			return nil, &aiteerr.PlatformError{Code: "no_recipient", Retryable: false, Msg: "单聊会话缺 senderStaffId，无法投放卡片"}
		}
		route.chatType = pb.ChatType_CHAT_TYPE_P2P
		body["openSpaceId"] = OpenSpaceRobotPrefix + info.staffID
		body["imRobotOpenSpaceModel"] = map[string]any{"supportForward": true}
		body["imRobotOpenDeliverModel"] = map[string]any{"spaceType": "IM_ROBOT"}
	} else {
		if info.conversationType == conversationGroup {
			route.chatType = pb.ChatType_CHAT_TYPE_GROUP
		}
		body["openSpaceId"] = OpenSpaceGroupPrefix + chatID
		body["imGroupOpenSpaceModel"] = map[string]any{"supportForward": true}
		body["imGroupOpenDeliverModel"] = map[string]any{"robotCode": p.opts.RobotCode}
	}
	if _, err := p.api.call(ctx, http.MethodPost, PathCardCreate, body); err != nil {
		return nil, err
	}
	p.cards.put(outTrackID, route)
	cardID := outTrackID
	return &pb.SendResult{MessageId: outTrackID, CardId: &cardID}, nil
}

// UpdateCard 原地更新同一张卡片实例（PUT /v1.0/card/instances），绝不新发一张。
func (p *Platform) UpdateCard(ctx context.Context, cardID string, card *pb.ChecklistCard) error {
	_, err := p.api.call(ctx, http.MethodPut, PathCardInstances, map[string]any{
		"outTrackId":        cardID,
		"cardData":          map[string]any{"cardParamMap": cardParamMap(card)},
		"cardUpdateOptions": map[string]any{"updateCardDataByKey": true},
	})
	return err
}

// StreamCard 把完整 markdown 流式写进卡片的 content 变量（不在 PlatformPort 上，FF3 接）。
//
// 口径：每次都重发完整 markdown（不做增量 diff，那是 FF3 的），按 rune 边界切成 ≤1024 字节的块；
// 第一块 isFull=true（覆盖），其余 isFull=false（追加）；finalize 时只有最后一块 isFinalize=true。
// 每个请求一个新 guid。
func (p *Platform) StreamCard(ctx context.Context, cardID, markdown string, finalize bool) error {
	chunks := chunkUTF8(markdown, streamChunkBytes)
	for i, chunk := range chunks {
		last := i == len(chunks)-1
		if _, err := p.api.call(ctx, http.MethodPut, PathCardStreaming, map[string]any{
			"outTrackId": cardID,
			"guid":       randomHex(16),
			"key":        "content",
			"content":    chunk,
			"isFull":     i == 0,
			"isFinalize": finalize && last,
			"isError":    false,
		}); err != nil {
			return err
		}
	}
	return nil
}

// chunkUTF8 按 rune 边界把 s 切成每块 ≤ limit 字节；空串返回一个空块。
func chunkUTF8(s string, limit int) []string {
	if s == "" {
		return []string{""}
	}
	var out []string
	for len(s) > 0 {
		if len(s) <= limit {
			out = append(out, s)
			break
		}
		cut := limit
		for cut > 0 && !utf8.RuneStart(s[cut]) {
			cut--
		}
		if cut == 0 {
			// limit 比一个 rune 还小；不会发生（limit=1024），兜底别死循环。
			_, size := utf8.DecodeRuneInString(s)
			cut = size
		}
		out = append(out, s[:cut])
		s = s[cut:]
	}
	return out
}

// cardParamMap 是模板变量；钉钉的 cardParamMap 值都是字符串。
func cardParamMap(card *pb.ChecklistCard) map[string]string {
	return map[string]string{
		"content": renderChecklistMarkdown(card),
		"task_id": card.GetTaskId(),
		"task_no": card.GetTaskNo(),
		"status":  statusToken(card.GetStatus()),
	}
}

func statusToken(s pb.CardStatus) string {
	switch s {
	case pb.CardStatus_CARD_STATUS_WORKING:
		return "working"
	case pb.CardStatus_CARD_STATUS_DELIVERED:
		return "delivered"
	case pb.CardStatus_CARD_STATUS_FAILED:
		return "failed"
	case pb.CardStatus_CARD_STATUS_CANCELLED:
		return "cancelled"
	default:
		return "unknown"
	}
}

func statusLabel(s pb.CardStatus) string {
	switch s {
	case pb.CardStatus_CARD_STATUS_WORKING:
		return "进行中"
	case pb.CardStatus_CARD_STATUS_DELIVERED:
		return "已完成"
	case pb.CardStatus_CARD_STATUS_FAILED:
		return "失败"
	case pb.CardStatus_CARD_STATUS_CANCELLED:
		return "已停止"
	default:
		return ""
	}
}

func itemMark(s pb.ChecklistState) string {
	switch s {
	case pb.ChecklistState_CHECKLIST_STATE_DONE:
		return "✅"
	case pb.ChecklistState_CHECKLIST_STATE_DOING:
		return "⏳"
	case pb.ChecklistState_CHECKLIST_STATE_FAILED:
		return "❌"
	default:
		return "⬜"
	}
}

// renderChecklistMarkdown 把清单卡渲染成 markdown（进模板的 content 变量）。
func renderChecklistMarkdown(card *pb.ChecklistCard) string {
	var b strings.Builder
	head := strings.TrimSpace(card.GetTaskNo() + " " + card.GetTitle())
	fmt.Fprintf(&b, "**%s**\n\n", head)
	var meta []string
	if card.GetInitiator() != "" {
		meta = append(meta, "发起人 "+card.GetInitiator())
	}
	if card.GetStartedAt() != "" {
		meta = append(meta, card.GetStartedAt()+" 开始")
	}
	if label := statusLabel(card.GetStatus()); label != "" {
		meta = append(meta, label)
	}
	if len(meta) > 0 {
		b.WriteString(strings.Join(meta, " · "))
		b.WriteString("\n\n")
	}
	for _, item := range card.GetItems() {
		fmt.Fprintf(&b, "- %s %s", itemMark(item.GetState()), item.GetText())
		if item.Note != nil && item.GetNote() != "" {
			fmt.Fprintf(&b, "（%s）", item.GetNote())
		}
		b.WriteString("\n")
	}
	if card.GetFooter() != "" {
		b.WriteString("\n")
		b.WriteString(card.GetFooter())
	}
	return strings.TrimRight(b.String(), "\n")
}
