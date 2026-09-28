package wecom

import (
	"strings"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
)

func workingCard(taskNo string, status pb.CardStatus, doneItems int) *pb.ChecklistCard {
	items := []*pb.ChecklistItemView{
		{Id: "1", Text: "读代码", State: pb.ChecklistState_CHECKLIST_STATE_DOING},
		{Id: "2", Text: "写报告", State: pb.ChecklistState_CHECKLIST_STATE_TODO},
	}
	for i := 0; i < doneItems && i < len(items); i++ {
		items[i].State = pb.ChecklistState_CHECKLIST_STATE_DONE
	}
	return &pb.ChecklistCard{
		TaskId: "task-1", TaskNo: taskNo, Title: "查昨天的报错", Initiator: "张三", StartedAt: "9:00",
		Status: status, Items: items, Actions: []pb.CardActionKind{pb.CardActionKind_CARD_ACTION_KIND_STOP},
	}
}

func streamOf(f recvFrame) map[string]any {
	s, _ := f.Body["stream"].(map[string]any)
	return s
}

func TestStreamAutoFinishesAt9MinWithPointer(t *testing.T) {
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, nil)
	h.waitConnected(t)
	ctx := t.Context()

	ev := h.inbound(t, "rq-cb", textMsg("m-1", "chat-1", "group", "@Aite 查一下"))
	replyTo := ev.GetAnchor().GetMessageId()
	res, err := h.p.SendCard(ctx, "chat-1", &replyTo, workingCard("#AH", pb.CardStatus_CARD_STATUS_WORKING, 0))
	if err != nil {
		t.Fatalf("SendCard: %v", err)
	}
	streamID := res.GetMessageId()

	h.clock.Advance(8*time.Minute + 59*time.Second)
	if err := h.p.UpdateCard(ctx, res.GetCardId(), workingCard("#AH", pb.CardStatus_CARD_STATUS_WORKING, 1)); err != nil {
		t.Fatalf("UpdateCard@8m59s: %v", err)
	}
	frames := fs.framesWith(cmdRespondMsg)
	if len(frames) != 2 || streamOf(frames[1])["finish"] != false {
		t.Fatalf("8m59s 的更新不该收尾：%+v", frames)
	}

	// 9 分钟：后台 sweep 收尾（任务不再更新也得收）。
	h.clock.Advance(time.Second)
	h.tick <- h.clock.Now()
	waitFor(t, "9 分钟收尾帧", func() bool { return len(fs.framesWith(cmdRespondMsg)) == 3 })
	last := fs.framesWith(cmdRespondMsg)[2]
	s := streamOf(last)
	if last.ReqID != "rq-cb" || s["id"] != streamID || s["finish"] != true {
		t.Fatalf("收尾帧不对：%+v", last)
	}
	content, _ := s["content"].(string)
	if !strings.HasSuffix(content, "进度见 #AH") {
		t.Fatalf("收尾内容要以「进度见 #AH」结尾：%q", content)
	}
	if !strings.Contains(content, "✅ 读代码") {
		t.Fatalf("收尾内容要是最后一次内容 + 指针：%q", content)
	}

	// 收尾后的更新一帧不发、返回 nil。
	if err := h.p.UpdateCard(ctx, res.GetCardId(), workingCard("#AH", pb.CardStatus_CARD_STATUS_WORKING, 2)); err != nil {
		t.Fatalf("收尾后 UpdateCard 要返回 nil：%v", err)
	}
	h.tick <- h.clock.Now()
	h.tick <- h.clock.Now() // 两次 tick：第二次送得进去说明第一次 sweep 已跑完
	if n := len(fs.framesWith(cmdRespondMsg)); n != 3 {
		t.Fatalf("收尾后又发了帧：共 %d 帧", n)
	}
}

func TestUpdateCardReusesStreamID(t *testing.T) {
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, nil)
	h.waitConnected(t)
	ctx := t.Context()

	ev := h.inbound(t, "rq-cb", textMsg("m-1", "chat-1", "group", "@Aite 查一下"))
	replyTo := ev.GetAnchor().GetMessageId()
	res, err := h.p.SendCard(ctx, "chat-1", &replyTo, workingCard("#A1", pb.CardStatus_CARD_STATUS_WORKING, 0))
	if err != nil {
		t.Fatalf("SendCard: %v", err)
	}
	if res.GetCardId() != res.GetMessageId() || res.GetMessageId() == "" {
		t.Fatalf("MessageId 与 CardId 都要是 stream.id：%v", res)
	}
	open := fs.framesWith(cmdRespondMsg)[0]
	os := streamOf(open)
	fb, _ := os["feedback"].(map[string]any)
	if open.ReqID != "rq-cb" || open.Body["msgtype"] != "stream" || os["id"] != res.GetMessageId() ||
		os["finish"] != false || fb["id"] != res.GetMessageId() {
		t.Fatalf("开流帧不对：%+v", open)
	}
	if c, _ := os["content"].(string); !strings.Contains(c, "#A1") {
		t.Fatalf("清单内容要带 TaskNo：%q", c)
	}

	for i, st := range []pb.CardStatus{pb.CardStatus_CARD_STATUS_WORKING, pb.CardStatus_CARD_STATUS_WORKING, pb.CardStatus_CARD_STATUS_DELIVERED} {
		if err := h.p.UpdateCard(ctx, res.GetCardId(), workingCard("#A1", st, i+1)); err != nil {
			t.Fatalf("UpdateCard #%d: %v", i, err)
		}
	}
	frames := fs.framesWith(cmdRespondMsg)
	if len(frames) != 4 {
		t.Fatalf("开流 + 3 次更新 = 4 帧，得到 %d", len(frames))
	}
	for i, f := range frames {
		s := streamOf(f)
		if s["id"] != res.GetMessageId() || f.ReqID != "rq-cb" {
			t.Fatalf("第 %d 帧换了 stream.id / req_id：%+v", i, f)
		}
		wantFinish := i == 3
		if s["finish"] != wantFinish {
			t.Fatalf("第 %d 帧 finish = %v，只有终态才收尾", i, s["finish"])
		}
	}
	if err := h.p.UpdateCard(ctx, res.GetCardId(), workingCard("#A1", pb.CardStatus_CARD_STATUS_DELIVERED, 2)); err != nil {
		t.Fatalf("收尾后 UpdateCard 要返回 nil：%v", err)
	}
	if n := len(fs.framesWith(cmdRespondMsg)); n != 4 {
		t.Fatalf("收尾后又发了帧：%d", n)
	}

	// replyTo 查不到（被挤出 / 重启过）：该会话来过消息 → 恰好一帧 aibot_send_msg，之后更新零帧。
	h.inbound(t, "rq-cb-2", textMsg("m-2", "chat-2", "group", "@Aite 另一件事"))
	lost := "m-evicted"
	res2, err := h.p.SendCard(ctx, "chat-2", &lost, workingCard("#A2", pb.CardStatus_CARD_STATUS_WORKING, 0))
	if err != nil {
		t.Fatalf("回退 SendCard: %v", err)
	}
	sends := fs.framesWith(cmdSendMsg)
	if len(sends) != 1 || sends[0].Body["chatid"] != "chat-2" || sends[0].Body["msgtype"] != "markdown" {
		t.Fatalf("要恰好一帧 aibot_send_msg：%+v", sends)
	}
	if res2.GetMessageId() != noStreamPrefix+sends[0].ReqID || res2.GetCardId() != res2.GetMessageId() {
		t.Fatalf("回退的 id 要是 nostream- + 那一帧的 req_id：%v", res2)
	}
	if n := len(fs.framesWith(cmdRespondMsg)); n != 4 {
		t.Fatalf("查不到 req_id 还发了 aibot_respond_msg：%d", n)
	}
	before := len(fs.snapshot())
	if err := h.p.UpdateCard(ctx, res2.GetCardId(), workingCard("#A2", pb.CardStatus_CARD_STATUS_DELIVERED, 2)); err != nil {
		t.Fatalf("非流式 UpdateCard 要返回 nil：%v", err)
	}
	if n := len(fs.snapshot()); n != before {
		t.Fatalf("非流式消息的更新发了帧：%d → %d", before, n)
	}
}
