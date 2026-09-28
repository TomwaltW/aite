package wecom

import (
	"errors"
	"fmt"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

func TestProactiveSendRequiresPriorMessage(t *testing.T) {
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, nil)
	h.waitConnected(t)
	ctx := t.Context()

	_, err := h.p.SendText(ctx, &pb.OutboundText{ChatId: "chat-never", Text: "早上好"})
	var pe *aiteerr.PlatformError
	if !errors.As(err, &pe) || pe.Code != "proactive_not_allowed" || pe.Retryable {
		t.Fatalf("没来过消息的会话要拒：%v", err)
	}
	if n := len(fs.framesWith(cmdSendMsg)); n != 0 {
		t.Fatalf("被拒了还发了 %d 帧", n)
	}

	h.inbound(t, "rq-1", textMsg("m-1", "chat-1", "group", "@Aite hi"))
	res, err := h.p.SendText(ctx, &pb.OutboundText{ChatId: "chat-1", Text: "早上好"})
	if err != nil {
		t.Fatalf("来过消息的会话要放行：%v", err)
	}
	sends := fs.framesWith(cmdSendMsg)
	if len(sends) != 1 || sends[0].Body["chatid"] != "chat-1" || res.GetMessageId() != noStreamPrefix+sends[0].ReqID {
		t.Fatalf("主动发送帧不对：%+v / %v", sends, res)
	}
}

func TestProactiveSend30PerMinPerConversation(t *testing.T) {
	fs := newFakeServer(t, nil)
	h := startHarness(t, fs, nil)
	h.waitConnected(t)
	ctx := t.Context()
	h.inbound(t, "rq-1", textMsg("m-1", "chat-1", "group", "@Aite hi"))
	h.inbound(t, "rq-2", textMsg("m-2", "chat-2", "group", "@Aite hi"))

	for i := 0; i < 30; i++ {
		if _, err := h.p.SendText(ctx, &pb.OutboundText{ChatId: "chat-1", Text: fmt.Sprintf("第 %d 条", i+1)}); err != nil {
			t.Fatalf("第 %d 条就被拒了：%v", i+1, err)
		}
		h.clock.Advance(time.Second)
	}
	// 第 31 条：最早那条在 30 s 前，窗口内已满 30 条。
	_, err := h.p.SendText(ctx, &pb.OutboundText{ChatId: "chat-1", Text: "第 31 条"})
	var pe *aiteerr.PlatformError
	if !errors.As(err, &pe) || pe.Code != "rate_limited" || !pe.Retryable {
		t.Fatalf("第 31 条要被限速（可重试）：%v", err)
	}
	if n := len(fs.framesWith(cmdSendMsg)); n != 30 {
		t.Fatalf("限速后帧数 = %d，要 30", n)
	}

	// 换一个会话不受影响。
	if _, err := h.p.SendText(ctx, &pb.OutboundText{ChatId: "chat-2", Text: "hi"}); err != nil {
		t.Fatalf("另一个会话不该被连坐：%v", err)
	}

	// 推 30 s：第一条已在 60 s 前，腾出一个位置。
	h.clock.Advance(30 * time.Second)
	if _, err := h.p.SendText(ctx, &pb.OutboundText{ChatId: "chat-1", Text: "恢复"}); err != nil {
		t.Fatalf("60 s 后要恢复：%v", err)
	}
	// 但只腾出一个：紧接着再发，最早那条（第 2 条）距今 < 60 s。
	if _, err := h.p.SendText(ctx, &pb.OutboundText{ChatId: "chat-1", Text: "再来"}); platformCode(err) != "rate_limited" {
		t.Fatalf("滑动窗口只腾出一个位置：%v", err)
	}
}
