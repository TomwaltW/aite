// 流式回复（aibot_respond_msg，msgtype = stream）。FF3 接手这个文件，文件名别改。
//
// 一次回复 = 一个 stream.id：SendCard 开流，UpdateCard 在同一个 id 上推全量内容，
// 终态（DELIVERED / FAILED / CANCELLED）才 finish。平台要求 10 分钟内收尾 ——
// 到 9 分钟由后台 sweep 发 finish=true，内容尾巴加「进度见 #A..」指针；收尾后的更新
// 一帧不发（之后的里程碑消息是 FF3 的）。
//
// 没有可用的回调 req_id（replyTo 为空、被挤出有界表、进程重启过）就退回主动发送
// 一帧 markdown，返回 "nostream-" 前缀的 id，UpdateCard 认得它、对它零帧。
package wecom

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// streamAutoFinishAfter：平台 10 分钟上限留 1 分钟余量（plan:201）。
const streamAutoFinishAfter = 9 * time.Minute

// streamSweepEvery 是后台收尾 sweep 的周期。
const streamSweepEvery = 10 * time.Second

// streamTableCap 是在途流表的容量。
const streamTableCap = 4096

// noStreamPrefix 标出「非流式」回退出去的消息 id。
const noStreamPrefix = "nostream-"

// streamState 是一条流。mu 串行化同一条流上的「判定 + 发帧」，
// 否则 sweep 的 finish 帧与一次 UpdateCard 的普通帧可能在线上交错成「收尾后又推一帧」。
type streamState struct {
	mu       sync.Mutex
	id       string
	reqID    string
	chatID   string
	taskNo   string
	content  string // 最后一次推出去的内容（不含指针）
	openedAt time.Time
	finished bool
}

// isTerminal：这三个终态要 finish。
func isTerminal(s pb.CardStatus) bool {
	switch s {
	case pb.CardStatus_CARD_STATUS_DELIVERED, pb.CardStatus_CARD_STATUS_FAILED, pb.CardStatus_CARD_STATUS_CANCELLED:
		return true
	}
	return false
}

// progressPointer 是 9 分钟收尾时贴在内容尾巴上的指针；TaskNo 空就不加。
func progressPointer(taskNo string) string {
	if taskNo == "" {
		return ""
	}
	return "\n\n进度见 " + taskNo
}

var checklistMark = map[pb.ChecklistState]string{
	pb.ChecklistState_CHECKLIST_STATE_TODO:   "⬜",
	pb.ChecklistState_CHECKLIST_STATE_DOING:  "⏳",
	pb.ChecklistState_CHECKLIST_STATE_DONE:   "✅",
	pb.ChecklistState_CHECKLIST_STATE_FAILED: "❌",
}

var cardStatusText = map[pb.CardStatus]string{
	pb.CardStatus_CARD_STATUS_WORKING:   "进行中",
	pb.CardStatus_CARD_STATUS_DELIVERED: "已交付",
	pb.CardStatus_CARD_STATUS_FAILED:    "失败",
	pb.CardStatus_CARD_STATUS_CANCELLED: "已停止",
}

// renderChecklist 把清单卡渲染成 markdown（含 TaskNo，让群里看得到锚点）。
func renderChecklist(card *pb.ChecklistCard) string {
	var sb strings.Builder
	title := strings.TrimSpace(card.GetTaskNo() + " " + card.GetTitle())
	fmt.Fprintf(&sb, "**%s**", title)
	if st, ok := cardStatusText[card.GetStatus()]; ok {
		fmt.Fprintf(&sb, " · %s", st)
	}
	var meta []string
	if card.GetInitiator() != "" {
		meta = append(meta, card.GetInitiator())
	}
	if card.GetStartedAt() != "" {
		meta = append(meta, card.GetStartedAt()+" 开始")
	}
	if len(meta) > 0 {
		sb.WriteString("\n" + strings.Join(meta, " · "))
	}
	for _, item := range card.GetItems() {
		mark := checklistMark[item.GetState()]
		if mark == "" {
			mark = "⬜"
		}
		fmt.Fprintf(&sb, "\n%s %s", mark, item.GetText())
		if item.GetNote() != "" {
			fmt.Fprintf(&sb, "（%s）", item.GetNote())
		}
	}
	if card.GetFooter() != "" {
		sb.WriteString("\n\n" + card.GetFooter())
	}
	return sb.String()
}

// lookupReply 用 replyTo（= 入站事件的 Anchor.MessageId）查可回复上下文。
func (p *Platform) lookupReply(replyTo *string) (replyCtx, bool) {
	if replyTo == nil || *replyTo == "" {
		return replyCtx{}, false
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.replies.get(*replyTo)
}

// sendStreamFrame 在回调 req_id 上推一帧流式内容。
func (p *Platform) sendStreamFrame(ctx context.Context, reqID string, payload streamPayload) error {
	conn, err := p.currentConn()
	if err != nil {
		return err
	}
	_, err = conn.request(ctx, cmdRespondMsg, reqID, respondStreamBody{MsgType: msgTypeStream, Stream: payload})
	return err
}

// openStream 开一条新流；finish=true 就是一帧了事（SendText）。
func (p *Platform) openStream(ctx context.Context, rc replyCtx, taskNo, content string, finish bool) (string, error) {
	st := &streamState{
		id: newReqID(), reqID: rc.reqID, chatID: rc.chatID, taskNo: taskNo,
		content: content, openedAt: p.clock(), finished: finish,
	}
	// FeedbackID 能对回 Aite 的消息：feedback.id = stream.id = 返回给 core 的 MessageId。
	err := p.sendStreamFrame(ctx, rc.reqID, streamPayload{
		ID: st.id, Finish: finish, Content: content, Feedback: &streamFeedback{ID: st.id},
	})
	if err != nil {
		return "", err
	}
	p.mu.Lock()
	p.streams.put(st.id, st)
	p.mu.Unlock()
	return st.id, nil
}

// SendText：带可回复的 ReplyTo → 新开一个流、一帧 finish=true；否则走主动发送。
func (p *Platform) SendText(ctx context.Context, msg *pb.OutboundText) (*pb.SendResult, error) {
	if msg == nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Msg: "SendText 收到空消息"}
	}
	if rc, ok := p.lookupReply(msg.ReplyTo); ok {
		id, err := p.openStream(ctx, rc, "", msg.GetText(), true)
		if err != nil {
			return nil, err
		}
		return &pb.SendResult{MessageId: id}, nil
	}
	id, err := p.sendProactiveMarkdown(ctx, msg.GetChatId(), msg.GetText())
	if err != nil {
		return nil, err
	}
	return &pb.SendResult{MessageId: id}, nil
}

// SendCard 开流推清单卡；查不到 req_id 就退回主动发送一帧 markdown。
// 返回 MessageId = CardId = stream.id（或 "nostream-" + req_id）。
func (p *Platform) SendCard(ctx context.Context, chatID string, replyTo *string, card *pb.ChecklistCard) (*pb.SendResult, error) {
	if card == nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Msg: "SendCard 收到空卡片"}
	}
	content := renderChecklist(card)
	var (
		id  string
		err error
	)
	if rc, ok := p.lookupReply(replyTo); ok {
		id, err = p.openStream(ctx, rc, card.GetTaskNo(), content, isTerminal(card.GetStatus()))
	} else {
		id, err = p.sendProactiveMarkdown(ctx, chatID, content)
	}
	if err != nil {
		return nil, err
	}
	cardID := id
	return &pb.SendResult{MessageId: id, CardId: &cardID}, nil
}

// UpdateCard 在同一个 stream.id 上推全量内容；终态 finish。已收尾 / 非流式 / 查不到 → 零帧、nil。
func (p *Platform) UpdateCard(ctx context.Context, cardID string, card *pb.ChecklistCard) error {
	if strings.HasPrefix(cardID, noStreamPrefix) {
		p.logger.Info("wecom.update_skipped", "card_id", cardID, "reason", "非流式消息无法更新")
		return nil
	}
	p.mu.Lock()
	st, ok := p.streams.get(cardID)
	p.mu.Unlock()
	if !ok {
		// 被挤出表或进程重启过：流已经没法续了。与收尾后同口径，不让 core 反复重试。
		p.logger.Warn("wecom.update_skipped", "card_id", cardID, "reason", "流不在表里")
		return nil
	}

	st.mu.Lock()
	defer st.mu.Unlock()
	if st.finished {
		p.logger.Info("wecom.update_skipped", "card_id", cardID, "reason", "流已收尾")
		return nil
	}
	content := renderChecklist(card)
	if card.GetTaskNo() != "" {
		st.taskNo = card.GetTaskNo()
	}
	finish := isTerminal(card.GetStatus())
	sent := content
	if !finish && p.clock().Sub(st.openedAt) >= streamAutoFinishAfter {
		// sweep 还没来得及：这一帧就收尾，别冲过平台的 10 分钟。
		finish = true
		sent = content + progressPointer(st.taskNo)
	}
	if err := p.sendStreamFrame(ctx, st.reqID, streamPayload{ID: st.id, Finish: finish, Content: sent}); err != nil {
		return err
	}
	st.content = content
	st.finished = finish
	return nil
}

// sweepLoop 周期性收尾快到 10 分钟的流，直到 ctx 取消。
func (p *Platform) sweepLoop(ctx context.Context) {
	tick, stop := p.ticker(p.sweepEvery)
	defer stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick:
			p.sweepStreams(ctx)
		}
	}
}

// sweepStreams 给开了 ≥ 9 分钟还没收尾的流发 finish=true（内容 = 最后一次内容 + 指针）。
// 不能只靠下一次 UpdateCard：任务第 4 分钟起不再更新，流就会冲过 10 分钟。
func (p *Platform) sweepStreams(ctx context.Context) {
	now := p.clock()
	var due []*streamState
	p.mu.Lock()
	p.streams.each(func(_ string, st *streamState) { due = append(due, st) })
	p.mu.Unlock()

	for _, st := range due {
		st.mu.Lock()
		if !st.finished && now.Sub(st.openedAt) >= streamAutoFinishAfter {
			err := p.sendStreamFrame(ctx, st.reqID, streamPayload{
				ID: st.id, Finish: true, Content: st.content + progressPointer(st.taskNo),
			})
			if err != nil {
				// 下一轮再试。
				p.logger.Warn("wecom.stream_auto_finish_failed", "stream_id", st.id, "err", err)
			} else {
				st.finished = true
				p.logger.Info("wecom.stream_auto_finished", "stream_id", st.id, "task_no", st.taskNo)
			}
		}
		st.mu.Unlock()
	}
}
