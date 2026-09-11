// 对应 tests/adapters/feishu/test_feishu_outbound.py。
//
// 出站：文本 / 卡片 / 文件 / 表情。
//
// 最关键的一条是 UpdateCard 必须走「更新消息」而不是「发送消息」——
// 用 httptest 直接断言 HTTP 方法与路径，顺带断言「发送消息」的两条路由一次都没被碰过。
// 「过程中卡片至少更新 3 次且不新增消息」就压在这上面。
package feishu

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
)

var (
	pathReply     = fmt.Sprintf(PathMessageReply, testRootMsgID)
	pathPatchCard = fmt.Sprintf(PathMessage, testCardMsgID)
	pathReactions = fmt.Sprintf(PathMessageReaction, testRootMsgID)
)

func okSend(messageID string) func(http.ResponseWriter) {
	return jsonResponse(200, map[string]any{
		"code": 0, "msg": "ok", "data": map[string]any{"message_id": messageID},
	})
}

func okEmpty() func(http.ResponseWriter) {
	return jsonResponse(200, map[string]any{"code": 0, "msg": "ok", "data": map[string]any{}})
}

// multipartOf 解析记录下来的 multipart 请求体。
func (r recordedRequest) multipartOf(t *testing.T) (map[string]string, map[string][]byte, map[string]string) {
	t.Helper()
	_, params, err := mime.ParseMediaType(r.header.Get("Content-Type"))
	if err != nil {
		t.Fatalf("Content-Type 不是 multipart：%v", err)
	}
	reader := multipart.NewReader(bytes.NewReader(r.body), params["boundary"])
	fields := map[string]string{}
	files := map[string][]byte{}
	filenames := map[string]string{}
	for {
		part, err := reader.NextPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatalf("解析 multipart 失败：%v", err)
		}
		data, _ := io.ReadAll(part)
		if part.FileName() != "" {
			files[part.FormName()] = data
			filenames[part.FormName()] = part.FileName()
		} else {
			fields[part.FormName()] = string(data)
		}
	}
	return fields, files, filenames
}

// ---------------------------------------------------------------------------
// UpdateCard：更新消息 ≠ 发送消息
// ---------------------------------------------------------------------------

// TestUpdateCardUsesPatchNotSend
// 对应 test_update_card_patches_the_same_message_and_never_sends_a_new_one。
func TestUpdateCardUsesPatchNotSend(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	patch := f.on(http.MethodPatch, pathPatchCard, okEmpty())
	create := f.on(http.MethodPost, PathMessages, okSend("om_sent_0001"))
	reply := f.on(http.MethodPost, pathReply, okSend("om_sent_0001"))

	p, clock := outboundPlatform(t, f)
	if err := p.UpdateCard(context.Background(), testCardMsgID, sampleCard()); err != nil {
		t.Fatalf("UpdateCard 失败：%v", err)
	}

	// —— 判据：方法是 PATCH，路径是那条卡片自己 ——
	if !patch.called() {
		t.Fatal("PATCH 路由没被调用")
	}
	req := patch.last(t)
	if req.method != http.MethodPatch {
		t.Errorf("方法 = %s，要 PATCH", req.method)
	}
	if req.path != pathPatchCard {
		t.Errorf("路径 = %s，要 %s", req.path, pathPatchCard)
	}
	// —— 判据：两条「发送消息」的路由一次都没碰 ——
	if create.called() {
		t.Error("UpdateCard 退化成发新消息了")
	}
	if reply.called() {
		t.Error("UpdateCard 退化成回复新消息了")
	}

	body := req.jsonBodyOf(t)
	if len(body) != 1 {
		t.Errorf("更新卡片只带 content，别把 receive_id 之类捎上，得到 %v", body)
	}
	if _, ok := body["content"]; !ok {
		t.Error("请求体里没有 content")
	}
	var card map[string]any
	if err := json.Unmarshal([]byte(mapStr(body, "content")), &card); err != nil {
		t.Fatalf("content 不是合法 JSON：%v", err)
	}
	if updateMulti, _ := asMap(card["config"])["update_multi"].(bool); !updateMulti {
		t.Error("update_multi 必须为 true")
	}
	if got := clock.Slept(); len(got) != 0 {
		t.Errorf("出站路径上不该有重试等待，得到 %v", got)
	}
}

// TestRepeatedUpdatesNeverGrowTheMessageCount
// 对应 test_repeated_updates_never_grow_the_message_count。
func TestRepeatedUpdatesNeverGrowTheMessageCount(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	patch := f.on(http.MethodPatch, pathPatchCard, okEmpty())
	create := f.on(http.MethodPost, PathMessages, okSend("om_sent_0001"))

	p, _ := outboundPlatform(t, f)
	card := sampleCard()
	for _, state := range []pb.ChecklistState{
		pb.ChecklistState_CHECKLIST_STATE_TODO,
		pb.ChecklistState_CHECKLIST_STATE_DOING,
		pb.ChecklistState_CHECKLIST_STATE_DONE,
	} {
		card.Items[0].State = state
		if err := p.UpdateCard(context.Background(), testCardMsgID, card); err != nil {
			t.Fatalf("UpdateCard 失败：%v", err)
		}
	}

	if patch.count() != 3 {
		t.Errorf("PATCH 次数 = %d，要 3", patch.count())
	}
	if create.count() != 0 {
		t.Errorf("发送消息次数 = %d，要 0", create.count())
	}
	for _, call := range patch.calls {
		if call.method != http.MethodPatch {
			t.Errorf("出现了非 PATCH 的调用：%s", call.method)
		}
	}
}

// ---------------------------------------------------------------------------
// SendCard / SendText
// ---------------------------------------------------------------------------

// TestSendCardIntoThreadUsesReplyEndpoint 对应 test_send_card_into_thread_uses_reply_endpoint。
func TestSendCardIntoThreadUsesReplyEndpoint(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	reply := f.on(http.MethodPost, pathReply, okSend(testCardMsgID))

	p, _ := outboundPlatform(t, f)
	result, err := p.SendCard(context.Background(), testChatID, strptr(testRootMsgID), sampleCard())
	if err != nil {
		t.Fatalf("SendCard 失败：%v", err)
	}

	req := reply.last(t)
	if req.method != http.MethodPost {
		t.Errorf("方法 = %s", req.method)
	}
	if req.path != pathReply {
		t.Errorf("路径 = %s，要 %s", req.path, pathReply)
	}
	body := req.jsonBodyOf(t)
	if mapStr(body, "msg_type") != "interactive" {
		t.Errorf("msg_type = %q", mapStr(body, "msg_type"))
	}
	if inThread, _ := body["reply_in_thread"].(bool); !inThread {
		t.Error("SendCard 的 reply_in_thread 恒为 true")
	}
	// card_id 要能直接喂给 UpdateCard
	if result.GetMessageId() != testCardMsgID {
		t.Errorf("message_id = %q", result.GetMessageId())
	}
	if result.GetCardId() != testCardMsgID {
		t.Errorf("card_id = %q", result.GetCardId())
	}
}

// TestSendCardWithoutReplyToUsesCreateEndpoint
// 对应 test_send_card_without_reply_to_uses_create_endpoint。
func TestSendCardWithoutReplyToUsesCreateEndpoint(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	create := f.on(http.MethodPost, PathMessages, okSend(testCardMsgID))

	p, _ := outboundPlatform(t, f)
	if _, err := p.SendCard(context.Background(), testChatID, nil, sampleCard()); err != nil {
		t.Fatalf("SendCard 失败：%v", err)
	}

	req := create.last(t)
	if req.method != http.MethodPost {
		t.Errorf("方法 = %s", req.method)
	}
	if req.path != PathMessages {
		t.Errorf("路径 = %s", req.path)
	}
	if got := req.query.Get("receive_id_type"); got != "chat_id" {
		t.Errorf("receive_id_type = %q", got)
	}
	body := req.jsonBodyOf(t)
	if mapStr(body, "receive_id") != testChatID {
		t.Errorf("receive_id = %q", mapStr(body, "receive_id"))
	}
	if mapStr(body, "msg_type") != "interactive" {
		t.Errorf("msg_type = %q", mapStr(body, "msg_type"))
	}
}

// TestSendTextCarriesMarkdown 对应 test_send_text_carries_markdown。
func TestSendTextCarriesMarkdown(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	reply := f.on(http.MethodPost, pathReply, okSend("om_sent_0001"))

	p, _ := outboundPlatform(t, f)
	_, err := p.SendText(context.Background(), &pb.OutboundText{
		ChatId: testChatID, Text: "**已完成**\n- 图见附件",
		ReplyTo: strptr(testRootMsgID), InThread: true,
	})
	if err != nil {
		t.Fatalf("SendText 失败：%v", err)
	}

	body := reply.last(t).jsonBodyOf(t)
	var card map[string]any
	if err := json.Unmarshal([]byte(mapStr(body, "content")), &card); err != nil {
		t.Fatalf("content 不是合法 JSON：%v", err)
	}
	elements := asList(card["elements"])
	if len(elements) != 1 {
		t.Fatalf("markdown 卡片该只有一个元素，得到 %v", elements)
	}
	element := asMap(elements[0])
	if mapStr(element, "tag") != "markdown" {
		t.Errorf("tag = %q，要 markdown", mapStr(element, "tag"))
	}
	if mapStr(element, "content") != "**已完成**\n- 图见附件" {
		t.Errorf("content = %q", mapStr(element, "content"))
	}
	// 所有文本出站都是 interactive 卡片，不走 msg_type=text。
	if mapStr(body, "msg_type") != "interactive" {
		t.Errorf("msg_type = %q，要 interactive", mapStr(body, "msg_type"))
	}
}

// TestSendTextRespectsInThreadFalse 对应 test_send_text_respects_in_thread_false。
func TestSendTextRespectsInThreadFalse(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	reply := f.on(http.MethodPost, pathReply, okSend("om_sent_0001"))

	p, _ := outboundPlatform(t, f)
	_, err := p.SendText(context.Background(), &pb.OutboundText{
		ChatId: testChatID, Text: "hi", ReplyTo: strptr(testRootMsgID), InThread: false,
	})
	if err != nil {
		t.Fatalf("SendText 失败：%v", err)
	}
	if inThread, _ := reply.last(t).jsonBodyOf(t)["reply_in_thread"].(bool); inThread {
		t.Error("in_thread=false 时 reply_in_thread 要是 false")
	}
}

// ---------------------------------------------------------------------------
// SendFile
// ---------------------------------------------------------------------------

// TestSendFileUploadsImageThenSendsIt 对应 test_send_file_uploads_image_then_sends_it。
func TestSendFileUploadsImageThenSendsIt(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	upload := f.onJSON(http.MethodPost, PathImages, 200, map[string]any{
		"code": 0, "data": map[string]any{"image_key": "img_v3_new"},
	})
	files := f.onJSON(http.MethodPost, PathFiles, 200, map[string]any{"code": 0, "data": map[string]any{}})
	reply := f.on(http.MethodPost, pathReply, okSend("om_sent_0001"))

	p, _ := outboundPlatform(t, f)
	_, err := p.SendFile(context.Background(), &pb.OutboundFile{
		ChatId: testChatID, ReplyTo: strptr(testRootMsgID),
		Name: "trend.png", Mime: "image/png", Data: []byte("\x89PNG\r\n\x1a\n"),
	})
	if err != nil {
		t.Fatalf("SendFile 失败：%v", err)
	}

	if !upload.called() {
		t.Fatal("图片没走「上传图片」接口")
	}
	if files.called() {
		t.Error("图片不该走「上传文件」接口")
	}
	if got := upload.last(t).path; got != PathImages {
		t.Errorf("上传路径 = %s", got)
	}
	fields, parts, names := upload.last(t).multipartOf(t)
	if fields["image_type"] != "message" {
		t.Errorf("image_type = %q", fields["image_type"])
	}
	if string(parts["image"]) != "\x89PNG\r\n\x1a\n" {
		t.Errorf("图片字节没原样上传：%q", parts["image"])
	}
	if names["image"] != "trend.png" {
		t.Errorf("文件名 = %q", names["image"])
	}

	body := reply.last(t).jsonBodyOf(t)
	if mapStr(body, "msg_type") != "image" {
		t.Errorf("msg_type = %q，要 image", mapStr(body, "msg_type"))
	}
	var content map[string]any
	if err := json.Unmarshal([]byte(mapStr(body, "content")), &content); err != nil {
		t.Fatal(err)
	}
	if mapStr(content, "image_key") != "img_v3_new" || len(content) != 1 {
		t.Errorf("content = %v", content)
	}
}

// TestSendFileUploadsOtherFilesAsStream 对应 test_send_file_uploads_other_files_as_stream。
func TestSendFileUploadsOtherFilesAsStream(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	upload := f.onJSON(http.MethodPost, PathFiles, 200, map[string]any{
		"code": 0, "data": map[string]any{"file_key": "file_v2_new"},
	})
	images := f.onJSON(http.MethodPost, PathImages, 200, map[string]any{"code": 0, "data": map[string]any{}})
	reply := f.on(http.MethodPost, pathReply, okSend("om_sent_0001"))

	p, _ := outboundPlatform(t, f)
	_, err := p.SendFile(context.Background(), &pb.OutboundFile{
		ChatId: testChatID, ReplyTo: strptr(testRootMsgID),
		Name: "report.csv", Mime: "text/csv", Data: []byte("a,b\n1,2\n"),
	})
	if err != nil {
		t.Fatalf("SendFile 失败：%v", err)
	}

	if got := upload.last(t).path; got != PathFiles {
		t.Errorf("上传路径 = %s", got)
	}
	if images.called() {
		t.Error("非图片不该走「上传图片」接口")
	}
	fields, _, _ := upload.last(t).multipartOf(t)
	// .csv 不在 _FILE_TYPE_BY_EXT 里 → stream
	if fields["file_type"] != "stream" {
		t.Errorf("file_type = %q，要 stream", fields["file_type"])
	}
	if fields["file_name"] != "report.csv" {
		t.Errorf("file_name = %q", fields["file_name"])
	}

	body := reply.last(t).jsonBodyOf(t)
	if mapStr(body, "msg_type") != "file" {
		t.Errorf("msg_type = %q，要 file", mapStr(body, "msg_type"))
	}
	var content map[string]any
	if err := json.Unmarshal([]byte(mapStr(body, "content")), &content); err != nil {
		t.Fatal(err)
	}
	if mapStr(content, "file_key") != "file_v2_new" || len(content) != 1 {
		t.Errorf("content = %v", content)
	}
}

// TestFileTypeOfPicksTheDocumentedValues 是 Go 侧补的：_file_type_of 的映射表逐条。
func TestFileTypeOfPicksTheDocumentedValues(t *testing.T) {
	cases := map[string]string{
		"a.opus": "opus", "a.mp4": "mp4", "a.pdf": "pdf",
		"a.doc": "doc", "a.docx": "doc",
		"a.xls": "xls", "a.xlsx": "xls",
		"a.ppt": "ppt", "a.pptx": "ppt",
		"a.csv": "stream", "a.PDF": "pdf", "noext": "stream", "trailing.": "stream",
	}
	for name, want := range cases {
		if got := fileTypeOf(name); got != want {
			t.Errorf("fileTypeOf(%q) = %q，要 %q", name, got, want)
		}
	}
}

// ---------------------------------------------------------------------------
// AddReaction
// ---------------------------------------------------------------------------

// TestAddReactionPostsAnEmoji 对应 test_add_reaction_posts_an_emoji（3 个参数）。
func TestAddReactionPostsAnEmoji(t *testing.T) {
	kinds := map[string]pb.ReactionKind{
		"ack":  pb.ReactionKind_REACTION_KIND_ACK,
		"done": pb.ReactionKind_REACTION_KIND_DONE,
		"fail": pb.ReactionKind_REACTION_KIND_FAIL,
	}
	for name, kind := range kinds {
		t.Run(name, func(t *testing.T) {
			f := newFakeFeishu(t)
			f.mockToken()
			route := f.onJSON(http.MethodPost, pathReactions, 200,
				map[string]any{"code": 0, "data": map[string]any{}})

			p, _ := outboundPlatform(t, f)
			if err := p.AddReaction(context.Background(), testRootMsgID, kind); err != nil {
				t.Fatalf("AddReaction 失败：%v", err)
			}

			req := route.last(t)
			if req.method != http.MethodPost {
				t.Errorf("方法 = %s", req.method)
			}
			if req.path != pathReactions {
				t.Errorf("路径 = %s", req.path)
			}
			emoji := mapStr(asMap(req.jsonBodyOf(t)["reaction_type"]), "emoji_type")
			if !KnownEmojiTypes[emoji] {
				t.Errorf("emoji_type %q 不在官方清单里", emoji)
			}
		})
	}
}

// TestEveryReactionEmojiIsOnTheOfficialList
// 对应 test_every_reaction_emoji_is_on_the_official_list。
//
// emoji_type 是一份固定清单，清单外的值平台回 231001 表情类型不合法。
// 早期写的 EYES 就不在清单上（那是云文档高亮块那套小写枚举）。
func TestEveryReactionEmojiIsOnTheOfficialList(t *testing.T) {
	want := map[pb.ReactionKind]bool{
		pb.ReactionKind_REACTION_KIND_ACK:  true,
		pb.ReactionKind_REACTION_KIND_DONE: true,
		pb.ReactionKind_REACTION_KIND_FAIL: true,
	}
	if len(ReactionEmoji) != len(want) {
		t.Errorf("ReactionEmoji 的键要恰好是 {ack, done, fail}，得到 %v", ReactionEmoji)
	}
	for kind, emoji := range ReactionEmoji {
		if !want[kind] {
			t.Errorf("ReactionEmoji 里多了 %v", kind)
		}
		if !KnownEmojiTypes[emoji] {
			t.Errorf("emoji_type %q 不在官方清单里", emoji)
		}
		if emoji == "EYES" {
			t.Error("EYES 不在消息表情回复的清单里，发上去会 231001")
		}
	}
}

// ---------------------------------------------------------------------------
// 出站限速
// ---------------------------------------------------------------------------

// TestOutboundIsRateLimitedByTheCapability
// 对应 test_outbound_is_rate_limited_by_the_capability。
func TestOutboundIsRateLimitedByTheCapability(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.on(http.MethodPatch, pathPatchCard, okEmpty())

	clock := newFakeClock()
	_, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{
		domain: f.URL, ratePerMin: 2,
		clock: clock.Now, sleep: clock.Sleep, logger: logger,
	})

	// 桶容量 = 2：前两次不等，第三次要等 60/2 = 30s。
	for i := 0; i < 3; i++ {
		if err := p.UpdateCard(context.Background(), testCardMsgID, sampleCard()); err != nil {
			t.Fatalf("第 %d 次 UpdateCard 失败：%v", i+1, err)
		}
	}

	got := clock.Slept()
	if len(got) != 1 || got[0] != 30*time.Second {
		t.Errorf("slept = %v，要 [30s]", got)
	}
}

// TestReadsDoNotConsumeTheOutboundQuota
// 对应 test_reads_do_not_consume_the_outbound_quota。
func TestReadsDoNotConsumeTheOutboundQuota(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.onJSON(http.MethodGet, PathMessages, 200, map[string]any{
		"code": 0, "data": map[string]any{"items": []any{}, "has_more": false},
	})

	clock := newFakeClock()
	_, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{
		domain: f.URL, ratePerMin: 1,
		clock: clock.Now, sleep: clock.Sleep, logger: logger,
	})

	for i := 0; i < 5; i++ {
		if _, err := p.ReadHistory(context.Background(), testChatID, 10, nil); err != nil {
			t.Fatalf("ReadHistory 失败：%v", err)
		}
	}
	if got := clock.Slept(); len(got) != 0 {
		t.Errorf("读接口不该占出站额度，slept = %v", got)
	}
}

// TestUploadsDoNotConsumeTheOutboundQuota 是 Go 侧补的：上传接口也不过桶。
func TestUploadsDoNotConsumeTheOutboundQuota(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.onJSON(http.MethodPost, PathImages, 200, map[string]any{
		"code": 0, "data": map[string]any{"image_key": "img_v3_new"},
	})
	f.on(http.MethodPost, PathMessages, okSend("om_sent_0001"))

	clock := newFakeClock()
	_, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{
		domain: f.URL, ratePerMin: 2,
		clock: clock.Now, sleep: clock.Sleep, logger: logger,
	})

	// 两次 SendFile = 2 次上传（不过桶）+ 2 次发消息（过桶，正好用光桶容量 2）。
	for i := 0; i < 2; i++ {
		_, err := p.SendFile(context.Background(), &pb.OutboundFile{
			ChatId: testChatID, Name: "a.png", Mime: "image/png", Data: []byte("x"),
		})
		if err != nil {
			t.Fatalf("SendFile 失败：%v", err)
		}
	}
	if got := clock.Slept(); len(got) != 0 {
		t.Errorf("上传不该占出站额度，slept = %v", got)
	}
}
