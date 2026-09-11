// 对应 tests/adapters/feishu/test_normalize.py。
//
// 飞书原始事件 → NormalizedEvent，x.json 与 x.expected.json 逐字节一致。
//
// 两层判据，缺一不可：
//
//  1. 黄金文件：x.json 跑一遍归一化，protojson 的产物与 x.expected.json 逐字节比。
//     它锁的是「以后别悄悄漂」，锁不住「今天就写错了」—— expected 是实现生成的。
//  2. 手写断言：点名的 6 个 fixture，每个把关键字段用字面量写死在下面。
//     这一层才是真判据；黄金文件只是它的全字段兜底。
//
// 重新生成 expected：go test ./internal/feishu/ -run TestFixture -update
package feishu

import (
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"

	pb "aite/edge/gen/aitepb"
)

var updateGolden = flag.Bool("update", false, "重新生成 testdata/feishu/*.expected.json")

// requiredFixtures 是点名必须有的 6 个。多的可以有，少一个就不算数。
var requiredFixtures = []string{
	"message_at_bot_toplevel",
	"message_in_thread_no_at",
	"message_in_thread_with_at",
	"message_from_bot",
	"message_with_file",
	"card_action_stop",
}

// goldenMarshal 是 expected 文件的口径：protoNames + 不输出零值。
var goldenMarshal = protojson.MarshalOptions{UseProtoNames: true, EmitUnpopulated: false}

func normalized(t *testing.T, name string) *pb.NormalizedEvent {
	t.Helper()
	event := Normalize(loadFixture(t, name, ".json"), testBotOpenID, testAppID, "default")
	if event == nil {
		t.Fatalf("%s 应该能被归一化", name)
	}
	return event
}

func goldenBytes(t *testing.T, event *pb.NormalizedEvent) []byte {
	t.Helper()
	raw, err := goldenMarshal.Marshal(event)
	if err != nil {
		t.Fatalf("protojson 序列化失败：%v", err)
	}
	out, err := canonicalJSON(raw)
	if err != nil {
		t.Fatalf("规范化 JSON 失败：%v", err)
	}
	return out
}

// ---------------------------------------------------------------------------
// fixture 清单本身
// ---------------------------------------------------------------------------

// TestRequiredFixtureExists 对应 test_required_fixture_exists。
func TestRequiredFixtureExists(t *testing.T) {
	for _, name := range requiredFixtures {
		t.Run(name, func(t *testing.T) {
			if _, err := os.Stat(filepath.Join(testFixturesDir, name+".json")); err != nil {
				t.Fatalf("缺 fixture：%s.json", name)
			}
		})
	}
}

// TestEveryFixtureHasExpected 对应 test_every_fixture_has_expected。
func TestEveryFixtureHasExpected(t *testing.T) {
	for _, name := range fixtureNames(t) {
		t.Run(name, func(t *testing.T) {
			if _, err := os.Stat(filepath.Join(testFixturesDir, name+".expected.json")); err != nil {
				t.Fatalf("%s 缺 .expected.json", name)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// 黄金文件：逐字节比
// ---------------------------------------------------------------------------

// TestFixtureMatchesExpectedByteForByte 对应 test_fixture_matches_expected_field_by_field。
func TestFixtureMatchesExpectedByteForByte(t *testing.T) {
	for _, name := range fixtureNames(t) {
		t.Run(name, func(t *testing.T) {
			actual := goldenBytes(t, normalized(t, name))
			path := filepath.Join(testFixturesDir, name+".expected.json")
			if *updateGolden {
				if err := os.WriteFile(path, actual, 0o644); err != nil {
					t.Fatalf("写 expected 失败：%v", err)
				}
				t.Logf("已重新生成 %s", path)
				return
			}
			want, err := os.ReadFile(path)
			if err != nil {
				t.Fatalf("读 expected 失败：%v", err)
			}
			if string(actual) != string(want) {
				t.Fatalf("%s 与 expected 不一致\n--- 实际 ---\n%s\n--- 期望 ---\n%s", name, actual, want)
			}
		})
	}
}

// TestExpectedRoundTripsBackIntoTheContract 对应 test_expected_round_trips_back_into_the_contract。
func TestExpectedRoundTripsBackIntoTheContract(t *testing.T) {
	for _, name := range fixtureNames(t) {
		t.Run(name, func(t *testing.T) {
			data, err := os.ReadFile(filepath.Join(testFixturesDir, name+".expected.json"))
			if err != nil {
				t.Fatalf("读 expected 失败：%v", err)
			}
			var event pb.NormalizedEvent
			if err := protojson.Unmarshal(data, &event); err != nil {
				t.Fatalf("expected 不是合法的 NormalizedEvent：%v", err)
			}
			if !proto.Equal(&event, normalized(t, name)) {
				t.Fatal("expected 反序列化回来与实现的产物不等价")
			}
		})
	}
}

// TestRawIsKeptVerbatim 对应 test_raw_is_kept_verbatim。
//
// 与 Python 版的一处有意差异：raw 里剥掉 header.token 与 event.token
// （移植清单 §8 第 10 条点名的审计卫生缺口）。除这两个键外逐字段一致。
func TestRawIsKeptVerbatim(t *testing.T) {
	for _, name := range fixtureNames(t) {
		t.Run(name, func(t *testing.T) {
			raw := normalized(t, name).GetRaw().AsMap()
			want := loadFixture(t, name, ".json")
			stripTokens(want)
			if !jsonEqual(raw, want) {
				a, _ := json.Marshal(raw)
				b, _ := json.Marshal(want)
				t.Fatalf("raw 与原始事件（剥 token 后）不一致\n实际 %s\n期望 %s", a, b)
			}
		})
	}
}

func stripTokens(raw map[string]any) {
	for _, key := range []string{"header", "event"} {
		if m, ok := raw[key].(map[string]any); ok {
			delete(m, "token")
		}
	}
}

func jsonEqual(a, b any) bool {
	x, err1 := json.Marshal(a)
	y, err2 := json.Marshal(b)
	return err1 == nil && err2 == nil && string(x) == string(y)
}

// ---------------------------------------------------------------------------
// 6 个 fixture 各自验什么（手写字面量）
// ---------------------------------------------------------------------------

// TestMessageAtBotToplevel 对应 test_message_at_bot_toplevel。
//
// 群里顶层 @Aite。thread_id 必须是 nil：本条自己才是话题 root。
func TestMessageAtBotToplevel(t *testing.T) {
	event := normalized(t, "message_at_bot_toplevel")
	if event.GetKind() != pb.EventKind_EVENT_KIND_MESSAGE {
		t.Errorf("kind = %v，要 MESSAGE", event.GetKind())
	}
	if event.GetSenderKind() != pb.SenderKind_SENDER_KIND_HUMAN {
		t.Errorf("sender_kind = %v，要 HUMAN", event.GetSenderKind())
	}
	if !event.GetMentioned() {
		t.Error("mentioned 要是 true")
	}
	// @Aite 剥掉并 strip 过
	if got := event.GetText(); got != "把这个季度的销售数据画成趋势图" {
		t.Errorf("text = %q", got)
	}
	if got := event.GetRawText(); got != "@_user_1 把这个季度的销售数据画成趋势图" {
		t.Errorf("raw_text = %q", got)
	}
	if event.GetAnchor().ThreadId != nil {
		t.Errorf("anchor.thread_id 要是 nil，得到 %q", event.GetAnchor().GetThreadId())
	}
	if got := event.GetAnchor().GetMessageId(); got != testRootMsgID {
		t.Errorf("anchor.message_id = %q", got)
	}
	if event.GetChatType() != pb.ChatType_CHAT_TYPE_GROUP {
		t.Errorf("chat_type = %v，要 GROUP", event.GetChatType())
	}
	if got := event.GetWorkspaceId(); got != testAppID { // 飞书 app_id
		t.Errorf("workspace_id = %q", got)
	}
	if got := event.GetEventId(); got != "evt_at_bot_toplevel_0001" {
		t.Errorf("event_id = %q", got)
	}
}

// TestMessageInThreadNoAt 对应 test_message_in_thread_no_at。
//
// 话题里不带 @ 的追问 —— 靠 thread_id 续接，不要求 mentioned。
func TestMessageInThreadNoAt(t *testing.T) {
	event := normalized(t, "message_in_thread_no_at")
	if event.GetMentioned() {
		t.Error("mentioned 要是 false")
	}
	if got := event.GetAnchor().GetThreadId(); got != testRootMsgID {
		t.Errorf("anchor.thread_id = %q", got)
	}
	if got := event.GetText(); got != "再按季度画一张" {
		t.Errorf("text = %q", got)
	}
}

// TestMessageInThreadWithAt 对应 test_message_in_thread_with_at。
//
// 话题里带 @：root_id 优先于 parent_id / thread_id；别人的 @ 换成人名。
func TestMessageInThreadWithAt(t *testing.T) {
	message := asMap(asMap(loadFixture(t, "message_in_thread_with_at", ".json")["event"])["message"])
	root, parent, thread := mapStr(message, "root_id"), mapStr(message, "parent_id"), mapStr(message, "thread_id")
	if root == parent || parent == thread || root == thread {
		t.Fatal("这条 fixture 就是要三者都不同")
	}

	event := normalized(t, "message_in_thread_with_at")
	if !event.GetMentioned() {
		t.Error("mentioned 要是 true")
	}
	// 不是 parent_id，也不是 omt_
	if got := event.GetAnchor().GetThreadId(); got != testRootMsgID {
		t.Errorf("anchor.thread_id = %q", got)
	}
	if got := event.GetText(); got != "顺便把 @李四 上周给的口径也对一下" {
		t.Errorf("text = %q", got)
	}
	if strings.Contains(event.GetText(), "@_user_") {
		t.Error("占位符不能漏给模型")
	}
}

// TestMessageFromBot 对应 test_message_from_bot。
//
// 机器人发的消息 —— 即使 @ 了 Aite，sender_kind 也必须不是 human（路由靠它丢弃）。
func TestMessageFromBot(t *testing.T) {
	event := normalized(t, "message_from_bot")
	switch event.GetSenderKind() {
	case pb.SenderKind_SENDER_KIND_BOT, pb.SenderKind_SENDER_KIND_APP:
	default:
		t.Errorf("sender_kind = %v，要 BOT 或 APP", event.GetSenderKind())
	}
	if event.GetSenderKind() == pb.SenderKind_SENDER_KIND_HUMAN {
		t.Error("sender_kind 不能是 HUMAN")
	}
	if !event.GetMentioned() {
		t.Error("它确实 @ 了；路由该看 sender_kind 而不是 mentioned")
	}
}

// TestMessageWithFile 对应 test_message_with_file。
//
// 带文件的消息：text 为空，附件进 attachments。
func TestMessageWithFile(t *testing.T) {
	event := normalized(t, "message_with_file")
	if event.GetText() != "" {
		t.Errorf("非文本消息 text 要为空，得到 %q", event.GetText())
	}
	if event.RawText != nil {
		t.Errorf("raw_text 要是 nil，得到 %q", event.GetRawText())
	}
	if len(event.GetAttachments()) != 1 {
		t.Fatalf("附件数 = %d，要 1", len(event.GetAttachments()))
	}
	a := event.GetAttachments()[0]
	if a.GetKind() != pb.AttachmentKind_ATTACHMENT_KIND_FILE {
		t.Errorf("attachment.kind = %v，要 FILE", a.GetKind())
	}
	if !strings.HasPrefix(a.GetFileKey(), "file_v2_") {
		t.Errorf("file_key = %q", a.GetFileKey())
	}
	if a.GetName() != "2026Q3-sales.csv" {
		t.Errorf("name = %q", a.GetName())
	}
	// 平台给的是字符串，这里已转 int
	if a.GetSize() != 20480 {
		t.Errorf("size = %d，要 20480", a.GetSize())
	}
	// 配 message_id 才能下载
	if a.GetMessageId() != event.GetAnchor().GetMessageId() {
		t.Errorf("attachment.message_id = %q，要 %q", a.GetMessageId(), event.GetAnchor().GetMessageId())
	}
}

// TestCardActionStop 对应 test_card_action_stop。
//
// 卡片 stop 按钮回传。card_id 就是卡片所在消息的 message_id。
func TestCardActionStop(t *testing.T) {
	event := normalized(t, "card_action_stop")
	if event.GetKind() != pb.EventKind_EVENT_KIND_CARD_ACTION {
		t.Errorf("kind = %v，要 CARD_ACTION", event.GetKind())
	}
	ca := event.GetCardAction()
	if ca == nil {
		t.Fatal("card_action 不该为 nil")
	}
	if ca.GetAction() != pb.CardActionKind_CARD_ACTION_KIND_STOP {
		t.Errorf("action = %v，要 STOP", ca.GetAction())
	}
	if ca.GetCardId() != testCardMsgID {
		t.Errorf("card_id = %q", ca.GetCardId())
	}
	if ca.GetCardId() != event.GetAnchor().GetMessageId() {
		t.Error("card_id 要等于 anchor.message_id")
	}
	if ca.GetTaskId() != "b7c1e6f0-1111-4222-8333-444455556666" {
		t.Errorf("task_id = %q", ca.GetTaskId())
	}
	// 点按钮的一定是真人
	if event.GetSenderKind() != pb.SenderKind_SENDER_KIND_HUMAN {
		t.Errorf("sender_kind = %v，要 HUMAN", event.GetSenderKind())
	}
	if event.GetText() != "" {
		t.Errorf("text 要为空，得到 %q", event.GetText())
	}
	if event.GetChatId() != testChatID {
		t.Errorf("chat_id = %q", event.GetChatId())
	}
}

// ---------------------------------------------------------------------------
// 额外覆盖
// ---------------------------------------------------------------------------

// TestPostMessageIsFlattenedAndImageBecomesAttachment
// 对应 test_post_message_is_flattened_and_image_becomes_attachment。
func TestPostMessageIsFlattenedAndImageBecomesAttachment(t *testing.T) {
	event := normalized(t, "message_post_with_image")
	if !event.GetMentioned() { // at 段 @ 的是机器人
		t.Error("mentioned 要是 true")
	}
	if strings.Contains(event.GetText(), "@Aite") {
		t.Error("text 里不该留 @Aite")
	}
	if !strings.HasPrefix(event.GetText(), "本周复盘") {
		t.Errorf("text 要以标题开头，得到 %q", event.GetText())
	}
	if !strings.Contains(event.GetText(), "[指标字典](https://example.feishu.cn/docx/DocTokenAbc123)") {
		t.Errorf("a 段要变成 markdown 链接，得到 %q", event.GetText())
	}
	if len(event.GetAttachments()) != 1 ||
		event.GetAttachments()[0].GetKind() != pb.AttachmentKind_ATTACHMENT_KIND_IMAGE {
		t.Errorf("附件要恰好一张图，得到 %v", event.GetAttachments())
	}
}

// TestUnsubscribedEventTypeIsIgnored 对应 test_unsubscribed_event_type_is_ignored。
func TestUnsubscribedEventTypeIsIgnored(t *testing.T) {
	raw := loadFixture(t, "message_at_bot_toplevel", ".json")
	asMap(raw["header"])["event_type"] = "im.message.message_read_v1"
	if Normalize(raw, testBotOpenID, "", "default") != nil {
		t.Error("没订阅的事件类型要返回 nil，而不是硬凑一个 EventKind 出来")
	}
}

// TestCardActionWithUnknownActionIsIgnored 对应 test_card_action_with_unknown_action_is_ignored。
func TestCardActionWithUnknownActionIsIgnored(t *testing.T) {
	raw := loadFixture(t, "card_action_stop", ".json")
	asMap(asMap(asMap(raw["event"])["action"])["value"])["action"] = "rerun"
	if Normalize(raw, testBotOpenID, "", "default") != nil {
		t.Error("CardAction.action 只有 stop/evidence，别的按钮不该硬塞进契约")
	}
}

// TestAdapterDoesNotDeduplicate 对应 test_adapter_does_not_deduplicate。
func TestAdapterDoesNotDeduplicate(t *testing.T) {
	first := normalized(t, "message_at_bot_toplevel")
	second := normalized(t, "message_at_bot_toplevel")
	if first.GetEventId() != second.GetEventId() {
		t.Error("同一条投两次要给出同样的 event_id")
	}
	if !proto.Equal(first, second) {
		t.Error("同一条投两次要给出两个等价事件")
	}
}

// TestAtSomeoneElseOnlyIsNotMentioned 对应 test_at_someone_else_only_is_not_mentioned。
func TestAtSomeoneElseOnlyIsNotMentioned(t *testing.T) {
	raw := loadFixture(t, "message_at_bot_toplevel", ".json")
	message := asMap(asMap(raw["event"])["message"])
	message["content"] = `{"text": "@_user_2 你看下"}`
	message["mentions"] = []any{
		map[string]any{
			"key":  "@_user_2",
			"id":   map[string]any{"open_id": "ou_li_si_000000000000000000000002"},
			"name": "李四",
		},
	}
	event := Normalize(raw, testBotOpenID, "", "default")
	if event == nil {
		t.Fatal("应该能被归一化")
	}
	if event.GetMentioned() {
		t.Error("只 @ 了别人 → mentioned 要是 false")
	}
	if got := event.GetText(); got != "@李四 你看下" {
		t.Errorf("text = %q", got)
	}
}

// ---------------------------------------------------------------------------
// T16：拿官方文档核对出来的三条
// ---------------------------------------------------------------------------

// TestTimestampUnitIsDetectedByMagnitude 对应 test_timestamp_unit_is_detected_by_magnitude。
func TestTimestampUnitIsDetectedByMagnitude(t *testing.T) {
	cases := []struct {
		ticks any
		why   string
	}{
		{"1788916020000", "im.message.receive_v1 的例子给的是 13 位毫秒"},
		{"1788916020000000", "事件订阅概述与 card.action.trigger 的例子给的是 16 位微秒"},
		{float64(1788916020000), "整数形式的毫秒"},
		{float64(1788916020000000), "整数形式的微秒"},
	}
	for _, c := range cases {
		t.Run(c.why, func(t *testing.T) {
			parsed, ok := toTime(c.ticks)
			if !ok {
				t.Fatalf("解析失败：%s", c.why)
			}
			if got := parsed.Format(time.RFC3339); got != "2026-09-09T01:07:00Z" {
				t.Errorf("得到 %s，要 2026-09-09T01:07:00Z（%s）", got, c.why)
			}
		})
	}
}

// TestCardActionSurvivesAMicrosecondTimestamp
// 对应 test_card_action_survives_a_microsecond_timestamp。
//
// card.action.trigger 的 header.create_time 是微秒。当成毫秒除会得到五万年后的
// 秒数，Timestamp 当场非法 —— 这条就是钉住它不许回去。
func TestCardActionSurvivesAMicrosecondTimestamp(t *testing.T) {
	raw := loadFixture(t, "card_action_stop", ".json")
	if got := mapStr(asMap(raw["header"]), "create_time"); got != "1788916020000000" {
		t.Fatalf("fixture 该是 16 位微秒，得到 %q", got)
	}
	event := Normalize(raw, testBotOpenID, "", "default")
	if event == nil {
		t.Fatal("应该能被归一化")
	}
	at := event.GetOccurredAt().AsTime()
	if at.Year() != 2026 {
		t.Errorf("年份 = %d", at.Year())
	}
	if got := at.Format(time.RFC3339); got != "2026-09-09T01:07:00Z" {
		t.Errorf("occurred_at = %s", got)
	}
}

// TestPostAtSegmentCarriesAPlaceholderNotAnOpenID
// 对应 test_post_at_segment_carries_a_placeholder_not_an_open_id。
func TestPostAtSegmentCarriesAPlaceholderNotAnOpenID(t *testing.T) {
	raw := loadFixture(t, "message_post_with_image", ".json")
	message := asMap(asMap(raw["event"])["message"])
	content := loadMessageContent(message)
	at := asMap(asList(asList(content["content"])[0])[0])
	if mapStr(at, "tag") != "at" {
		t.Fatalf("第一段该是 at，得到 %v", at)
	}
	if mapStr(at, "user_id") != "@_user_1" {
		t.Fatalf("fixture 要按文档写成占位序号，得到 %q", mapStr(at, "user_id"))
	}

	event := Normalize(raw, testBotOpenID, "", "default")
	if event == nil {
		t.Fatal("应该能被归一化")
	}
	if !event.GetMentioned() { // 靠 mentions 认出来的
		t.Error("mentioned 要是 true")
	}
	if strings.Contains(event.GetText(), "@_user_") {
		t.Error("占位符不能漏给模型")
	}
	if !strings.Contains(event.GetRawText(), "@_user_1") {
		t.Error("raw_text 要与 text 类消息同口径，保留占位符")
	}
}

// TestPostAtPlaceholderWithoutMentionsCannotIdentifyAnyone
// 对应 test_post_at_placeholder_without_mentions_cannot_identify_anyone。
func TestPostAtPlaceholderWithoutMentionsCannotIdentifyAnyone(t *testing.T) {
	raw := loadFixture(t, "message_post_with_image", ".json")
	delete(asMap(asMap(raw["event"])["message"]), "mentions")

	event := Normalize(raw, testBotOpenID, "", "default")
	if event == nil {
		t.Fatal("应该能被归一化")
	}
	if event.GetMentioned() {
		t.Error("查无此人 → mentioned 要是 false")
	}
	if strings.Contains(event.GetText(), "@_user_") {
		t.Error("占位符不能漏给模型")
	}
}

// TestPostAtWithARawOpenIDIsStillRecognised
// 对应 test_post_at_with_a_raw_open_id_is_still_recognised。
//
// 兜底分支：某个客户端真在 at 段塞了 open_id（文档说不该有），也得认出来。
func TestPostAtWithARawOpenIDIsStillRecognised(t *testing.T) {
	raw := loadFixture(t, "message_post_with_image", ".json")
	message := asMap(asMap(raw["event"])["message"])
	delete(message, "mentions")
	content := loadMessageContent(message)
	asMap(asList(asList(content["content"])[0])[0])["user_id"] = testBotOpenID // 老形状：直接是 open_id
	encoded, err := json.Marshal(content)
	if err != nil {
		t.Fatal(err)
	}
	message["content"] = string(encoded)

	event := Normalize(raw, testBotOpenID, "", "default")
	if event == nil {
		t.Fatal("应该能被归一化")
	}
	if !event.GetMentioned() {
		t.Error("mentioned 要是 true")
	}
	if strings.Contains(event.GetText(), testBotOpenID) {
		t.Error("别把 open_id 吐给模型")
	}
	if !strings.HasPrefix(event.GetText(), "本周复盘") {
		t.Errorf("text = %q", event.GetText())
	}
}

// ---------------------------------------------------------------------------
// Go 侧补的：event.token 不进 raw（Python 版遗留的审计卫生缺口）
// ---------------------------------------------------------------------------

// TestRawStripsBothVerificationTokens 是 Go 侧新增的判据。
//
// Python 的 _envelope 只剥了 header.token；card.action.trigger 的 event.token
// 一路进了 NormalizedEvent.raw 落进审计。这条钉住两个都剥掉。
func TestRawStripsBothVerificationTokens(t *testing.T) {
	raw := loadFixture(t, "card_action_stop", ".json")
	if _, ok := asMap(raw["event"])["token"]; !ok {
		t.Fatal("fixture 里该有 event.token，否则这条测试没有意义")
	}
	asMap(raw["header"])["token"] = "v-header-verification-token"

	event := Normalize(raw, testBotOpenID, "", "default")
	if event == nil {
		t.Fatal("应该能被归一化")
	}
	got := event.GetRaw().AsMap()
	if _, ok := asMap(got["header"])["token"]; ok {
		t.Error("header.token 不该进 raw")
	}
	if _, ok := asMap(got["event"])["token"]; ok {
		t.Error("event.token 不该进 raw")
	}
	// 剥的只是 token，别的字段一个不少。
	if mapStr(asMap(got["event"]), "host") != "im_message" {
		t.Error("event 的其他字段被误删了")
	}
	// 调用方传进来的 map 不许被就地改（Python 版 raw 是同一引用，Go 版是拷贝）。
	if _, ok := asMap(raw["event"])["token"]; !ok {
		t.Error("归一化不该就地改调用方的 map")
	}
}
