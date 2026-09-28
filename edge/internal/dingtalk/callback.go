// 卡片回调（CALLBACK /v1.0/card/instances/callback）：Stop → CARD_ACTION。
//
// 钉钉卡片回调 2 秒内必须 ACK：帧解析完立刻在读循环里 ACK，再另起 goroutine 把事件交给 sink
// （慢 sink 不许拖住 ACK；交付失败只打 Error 日志 —— 已经 ACK 过，平台不会重推）。
// 别的动作 → 不产生事件，照样 ACK。
//
// data 形状（推断，待 H9 核实）：{"outTrackId","userId","content":"<JSON 字符串>"}，
// content 里是 {"cardPrivateData":{"params":{"action":"stop","task_id":"…"}}}。
package dingtalk

import (
	"context"
	"encoding/json"

	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"

	pb "aite/edge/gen/aitepb"
)

// handleCardCallback 在读循环里被调：先 ACK，再异步投递。
func (p *Platform) handleCardCallback(ctx context.Context, c *streamConn, f *streamFrame) {
	var data map[string]any
	if err := json.Unmarshal([]byte(f.Data), &data); err != nil {
		p.logger.Warn("dingtalk.bad_card_callback", "err", err)
		data = nil
	}
	c.ack(f, 200, ackDataCallbackOK)

	if data == nil {
		return
	}
	ev := p.normalizeCardAction(data, f.messageID())
	if ev == nil {
		p.logger.Debug("dingtalk.card_action_ignored", "out_track_id", mapStr(data, "outTrackId"))
		return
	}
	if p.sink == nil {
		p.logger.Warn("dingtalk.no_sink", "note", "没接 EventSink，事件无处可送")
		return
	}
	p.inflight.Add(1)
	go func() {
		defer p.inflight.Done()
		if err := p.sink.HandleEvent(ctx, ev); err != nil {
			p.logger.Error("dingtalk.card_action_failed", "event_id", ev.GetEventId(), "err", err)
		}
	}()
}

// cardParams 取回调里的按钮参数 content.cardPrivateData.params。
func cardParams(data map[string]any) map[string]any {
	content, err := decodeJSONField(data["content"])
	if err != nil {
		return map[string]any{}
	}
	return asMap(asMap(content["cardPrivateData"])["params"])
}

// normalizeCardAction 把卡片回调归一化；只认 stop，其它返回 nil。
func (p *Platform) normalizeCardAction(data map[string]any, frameMessageID string) *pb.NormalizedEvent {
	params := cardParams(data)
	var kind pb.CardActionKind
	switch mapStr(params, "action") {
	case "stop":
		kind = pb.CardActionKind_CARD_ACTION_KIND_STOP
	default:
		return nil
	}

	outTrackID := mapStr(data, "outTrackId")
	// 取不到留零值：core R3 按 task_id 查（同 feishu 卡片回调）。
	route, _ := p.cards.get(outTrackID)

	var taskID *string
	if tid := mapStr(params, "task_id"); tid != "" {
		taskID = &tid
	}
	var value *structpb.Struct
	if s, err := structpb.NewStruct(params); err == nil {
		value = s
	}
	tenantID := p.opts.TenantID
	if tenantID == "" {
		tenantID = "default"
	}

	return &pb.NormalizedEvent{
		EventId:     frameMessageID,
		Kind:        pb.EventKind_EVENT_KIND_CARD_ACTION,
		Platform:    platformName,
		TenantId:    tenantID,
		WorkspaceId: route.corpID,
		ChatId:      route.chatID,
		ChatType:    route.chatType,
		SenderId:    mapStr(data, "userId"),
		// 点按钮的一定是真人；点的是我们自己发的卡片，等价于「冲着 Aite 来的」。
		SenderKind: pb.SenderKind_SENDER_KIND_HUMAN,
		Mentioned:  true,
		// Anchor 必填（缺失 core 拒收）；卡片回调只给得到卡片自己，话题归属由 core 按 task_id 查。
		Anchor: &pb.Anchor{
			Platform:  platformName,
			ChatId:    route.chatID,
			MessageId: outTrackID,
			ThreadId:  nil,
		},
		CardAction: &pb.CardAction{
			CardId: outTrackID,
			Action: kind,
			TaskId: taskID,
			Value:  value,
		},
		OccurredAt: timestamppb.New(p.clock()),
		Raw:        rawStruct(data),
	}
}
