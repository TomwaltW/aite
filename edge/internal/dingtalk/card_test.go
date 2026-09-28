package dingtalk

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"testing"
	"unicode/utf8"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// 钉：群 / 单聊两种 openSpaceId；UpdateCard 只打 PUT /v1.0/card/instances、从不打 createAndDeliver。
func TestSendCardCreateAndDeliverThenUpdateInPlace(t *testing.T) {
	f := newFakeDingtalk(t)
	f.mockToken()
	create := f.onJSON(http.MethodPost, PathCardCreate, 200, map[string]any{"success": true, "result": map[string]any{}})
	update := f.onJSON(http.MethodPut, PathCardInstances, 200, map[string]any{"success": true})
	p := mustPlatform(t, platformBuild{apiBase: f.URL, cardTemplateID: testTemplateID})
	ctx := context.Background()
	p.sessions.put(testGroupChat, sessionInfo{conversationType: "2", staffID: testStaffID, corpID: testCorpID})
	p.sessions.put(testP2PChat, sessionInfo{conversationType: "1", staffID: testStaffID, corpID: testCorpID})

	// 群。
	res, err := p.SendCard(ctx, testGroupChat, strptr("msgROOT"), sampleCard())
	if err != nil {
		t.Fatalf("群 SendCard 失败：%v", err)
	}
	outTrack := res.GetMessageId()
	if !strings.HasPrefix(outTrack, "aite-") || len(outTrack) != len("aite-")+16 {
		t.Errorf("outTrackId = %q，期望 aite- + 16 位 hex", outTrack)
	}
	if res.CardId == nil || *res.CardId != outTrack {
		t.Errorf("CardId = %v，期望等于 outTrackId", deref(res.CardId))
	}
	gb := create.at(t, 0).jsonBody(t)
	if gb["openSpaceId"] != "dtv1.card//IM_GROUP."+testGroupChat {
		t.Errorf("群 openSpaceId = %v", gb["openSpaceId"])
	}
	if dm, _ := gb["imGroupOpenDeliverModel"].(map[string]any); dm["robotCode"] != testRobotCode {
		t.Errorf("imGroupOpenDeliverModel = %v", gb["imGroupOpenDeliverModel"])
	}
	if gb["cardTemplateId"] != testTemplateID || gb["outTrackId"] != outTrack || gb["callbackType"] != "STREAM" {
		t.Errorf("createAndDeliver 基本字段不对：%v", gb)
	}
	params, _ := gb["cardData"].(map[string]any)["cardParamMap"].(map[string]any)
	if params["task_id"] != sampleCard().GetTaskId() || params["task_no"] != "#AH" || params["status"] != "working" {
		t.Errorf("cardParamMap = %v", params)
	}
	content, _ := params["content"].(string)
	for _, want := range []string{"**#AH 把 Q3 销售数据画成趋势图**", "发起人 张三 · 9:02 开始 · 进行中", "- ✅ 读取 CSV", "- ⏳ 按月汇总（共 3 个 sheet）", "- ⬜ 出图并回传", "预计 2 分钟 · 已用 ¥0.12"} {
		if !strings.Contains(content, want) {
			t.Errorf("content 缺 %q：\n%s", want, content)
		}
	}
	if route, ok := p.cards.get(outTrack); !ok || route.chatID != testGroupChat || route.corpID != testCorpID {
		t.Errorf("outTrackId 映射 = %+v,%v", route, ok)
	}

	// 单聊。
	res2, err := p.SendCard(ctx, testP2PChat, nil, sampleCard())
	if err != nil {
		t.Fatalf("单聊 SendCard 失败：%v", err)
	}
	pb2 := create.at(t, 1).jsonBody(t)
	if pb2["openSpaceId"] != "dtv1.card//IM_ROBOT."+testStaffID {
		t.Errorf("单聊 openSpaceId = %v", pb2["openSpaceId"])
	}
	if dm, _ := pb2["imRobotOpenDeliverModel"].(map[string]any); dm["spaceType"] != "IM_ROBOT" {
		t.Errorf("imRobotOpenDeliverModel = %v", pb2["imRobotOpenDeliverModel"])
	}
	if res2.GetMessageId() == outTrack {
		t.Error("两张卡片的 outTrackId 重复")
	}

	// UpdateCard：只打 PUT /v1.0/card/instances。
	card := sampleCard()
	card.Status = pb.CardStatus_CARD_STATUS_DELIVERED
	for i := 0; i < 3; i++ {
		if err := p.UpdateCard(ctx, outTrack, card); err != nil {
			t.Fatalf("UpdateCard 失败：%v", err)
		}
	}
	if create.count() != 2 {
		t.Fatalf("UpdateCard 打了 createAndDeliver（共 %d 次，期望仍是 2）", create.count())
	}
	if update.count() != 3 {
		t.Fatalf("PUT /v1.0/card/instances %d 次，期望 3", update.count())
	}
	ub := update.at(t, 0).jsonBody(t)
	if ub["outTrackId"] != outTrack {
		t.Errorf("更新的 outTrackId = %v", ub["outTrackId"])
	}
	if opts, _ := ub["cardUpdateOptions"].(map[string]any); opts["updateCardDataByKey"] != true {
		t.Errorf("cardUpdateOptions = %v", ub["cardUpdateOptions"])
	}
	if up, _ := ub["cardData"].(map[string]any)["cardParamMap"].(map[string]any); up["status"] != "delivered" {
		t.Errorf("更新的 cardParamMap = %v", up)
	}

	// 没配模板 → 不可重试错误，且不发请求。
	noTpl := mustPlatform(t, platformBuild{apiBase: f.URL})
	_, err = noTpl.SendCard(ctx, testGroupChat, nil, sampleCard())
	var pe *aiteerr.PlatformError
	if !errors.As(err, &pe) || pe.Retryable {
		t.Errorf("没模板应为不可重试 PlatformError，得到 %v", err)
	}
	if create.count() != 2 {
		t.Error("没模板时不该打 createAndDeliver")
	}
}

// 钉：3 KB 含中文的 markdown → 每个 PUT 的 content ≤1024 字节且合法 UTF-8、拼起来逐字等于原文、
// 只有第一块 isFull=true、每块 guid 不同；finalize 只在最后一块。
func TestCardStreamingChunksAtMost1KB(t *testing.T) {
	f := newFakeDingtalk(t)
	f.mockToken()
	streaming := f.onJSON(http.MethodPut, PathCardStreaming, 200, map[string]any{"success": true})
	p := mustPlatform(t, platformBuild{apiBase: f.URL})

	var b strings.Builder
	for i := 0; b.Len() < 3*1024; i++ {
		b.WriteString("- 第")
		b.WriteString(strings.Repeat("进", i%7+1))
		b.WriteString("步 step ✅\n")
	}
	markdown := b.String()

	if err := p.StreamCard(context.Background(), "aite-0123456789abcdef", markdown, true); err != nil {
		t.Fatalf("StreamCard 失败：%v", err)
	}
	n := streaming.count()
	if n < 3 {
		t.Fatalf("3 KB 只打了 %d 次 PUT，期望 ≥3", n)
	}
	var joined strings.Builder
	guids := map[string]bool{}
	for i := 0; i < n; i++ {
		body := streaming.at(t, i).jsonBody(t)
		content, _ := body["content"].(string)
		if len(content) > 1024 {
			t.Errorf("第 %d 块 %d 字节，超过 1024", i, len(content))
		}
		if !utf8.ValidString(content) {
			t.Errorf("第 %d 块不是合法 UTF-8", i)
		}
		joined.WriteString(content)
		if body["isFull"] != (i == 0) {
			t.Errorf("第 %d 块 isFull = %v", i, body["isFull"])
		}
		if body["isFinalize"] != (i == n-1) {
			t.Errorf("第 %d 块 isFinalize = %v", i, body["isFinalize"])
		}
		if body["key"] != "content" || body["outTrackId"] != "aite-0123456789abcdef" || body["isError"] != false {
			t.Errorf("第 %d 块基本字段不对：%v", i, body)
		}
		guid, _ := body["guid"].(string)
		if guid == "" || guids[guid] {
			t.Errorf("第 %d 块 guid %q 为空或重复", i, guid)
		}
		guids[guid] = true
	}
	if joined.String() != markdown {
		t.Fatal("各块拼起来不等于原文")
	}

	// 不 finalize → 没有任何一块 isFinalize=true。
	before := streaming.count()
	if err := p.StreamCard(context.Background(), "aite-0123456789abcdef", "短", false); err != nil {
		t.Fatal(err)
	}
	last := streaming.at(t, before).jsonBody(t)
	if last["isFinalize"] != false || last["isFull"] != true || last["content"] != "短" {
		t.Errorf("短内容不 finalize：%v", last)
	}
}
