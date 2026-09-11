// 对应 tests/adapters/feishu/test_feishu_cards.py。
//
// ChecklistCard → 飞书卡片 JSON。两头都验：
//
//   - Schema —— 卡片结构逐层查（config / header / elements / 按钮 value），
//     外加「卡片 ≤30KB」。
//   - SDK —— 把卡片塞进 oapi-sdk-go 生成的请求对象里构造一遍，顺带用 SDK 自己的
//     请求体类型反证「更新消息」和「发送消息」是两个接口。
package feishu

import (
	"encoding/json"
	"strings"
	"testing"

	larkim "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"

	pb "aite/edge/gen/aitepb"
)

var validTags = map[string]bool{
	"div": true, "hr": true, "note": true, "action": true, "markdown": true, "img": true,
}

// checkCardSchema 是飞书 v1 卡片的最小 schema 检查。字段错了平台只会回一句 invalid params。
func checkCardSchema(t *testing.T, card map[string]any) {
	t.Helper()
	allowed := map[string]bool{
		"config": true, "header": true, "elements": true, "i18n_elements": true, "card_link": true,
	}
	for key := range card {
		if !allowed[key] {
			t.Errorf("卡片顶层出现未知键 %q", key)
		}
	}
	elements, ok := card["elements"].([]any)
	if !ok || len(elements) == 0 {
		t.Fatalf("elements 要是非空数组，得到 %v", card["elements"])
	}

	config := asMap(card["config"])
	// 不开 update_multi，UpdateCard 只对第一个看到卡片的人生效。
	if updateMulti, _ := config["update_multi"].(bool); !updateMulti {
		t.Error("config.update_multi 必须为 true")
	}

	if header, ok := card["header"].(map[string]any); ok {
		title := asMap(header["title"])
		if mapStr(title, "tag") != "plain_text" {
			t.Errorf("header.title.tag = %q", mapStr(title, "tag"))
		}
		if _, ok := title["content"].(string); !ok {
			t.Error("header.title.content 要是字符串")
		}
		tpl := mapStr(header, "template")
		switch tpl {
		case "blue", "green", "red", "grey", "orange", "turquoise":
		default:
			t.Errorf("header.template = %q 不是已知配色", tpl)
		}
	}

	for _, raw := range elements {
		element, ok := raw.(map[string]any)
		if !ok {
			t.Fatalf("元素不是对象：%v", raw)
		}
		tag := mapStr(element, "tag")
		if !validTags[tag] {
			t.Errorf("未知元素 tag：%q", tag)
		}
		switch tag {
		case "div":
			_, hasText := element["text"]
			_, hasFields := element["fields"]
			if !hasText && !hasFields {
				t.Error("div 要有 text 或 fields")
			}
			for _, f := range asList(element["fields"]) {
				field := asMap(f)
				if _, ok := field["is_short"].(bool); !ok {
					t.Error("field.is_short 要是 bool")
				}
				ft := asMap(field["text"])
				if mapStr(ft, "tag") != "plain_text" && mapStr(ft, "tag") != "lark_md" {
					t.Errorf("field.text.tag = %q", mapStr(ft, "tag"))
				}
				if _, ok := ft["content"].(string); !ok {
					t.Error("field.text.content 要是字符串")
				}
			}
			if hasText {
				tt := asMap(element["text"])
				if mapStr(tt, "tag") != "plain_text" && mapStr(tt, "tag") != "lark_md" {
					t.Errorf("div.text.tag = %q", mapStr(tt, "tag"))
				}
			}
		case "note":
			inner := asList(element["elements"])
			if len(inner) == 0 {
				t.Error("note.elements 不能为空")
			}
			for _, e := range inner {
				if mapStr(asMap(e), "tag") != "plain_text" {
					t.Error("note 的子元素只能是 plain_text")
				}
			}
		case "action":
			actions := asList(element["actions"])
			if len(actions) == 0 {
				t.Error("action.actions 不能为空")
			}
			for _, a := range actions {
				action := asMap(a)
				if mapStr(action, "tag") != "button" {
					t.Errorf("action.tag = %q", mapStr(action, "tag"))
				}
				if mapStr(asMap(action["text"]), "tag") != "plain_text" {
					t.Error("按钮 text.tag 要是 plain_text")
				}
				switch mapStr(action, "type") {
				case "default", "primary", "danger":
				default:
					t.Errorf("按钮 type = %q", mapStr(action, "type"))
				}
				if _, ok := action["value"].(map[string]any); !ok {
					t.Error("按钮 value 要是对象")
				}
			}
		case "markdown":
			if _, ok := element["content"].(string); !ok {
				t.Error("markdown.content 要是字符串")
			}
		}
	}
}

// asAnyMap 把 BuildChecklistCard 的产物过一遍 JSON，得到与平台看到的一致的形状。
func asAnyMap(t *testing.T, card map[string]any) map[string]any {
	t.Helper()
	var out map[string]any
	if err := json.Unmarshal([]byte(DumpsCard(card)), &out); err != nil {
		t.Fatalf("卡片不是合法 JSON：%v", err)
	}
	return out
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

// TestChecklistCardPassesSchema 对应 test_checklist_card_passes_schema。
func TestChecklistCardPassesSchema(t *testing.T) {
	checkCardSchema(t, asAnyMap(t, BuildChecklistCard(sampleCard())))
}

// TestEveryStatusBuilds 对应 test_every_status_builds（4 个参数）。
func TestEveryStatusBuilds(t *testing.T) {
	statuses := map[string]pb.CardStatus{
		"working":   pb.CardStatus_CARD_STATUS_WORKING,
		"delivered": pb.CardStatus_CARD_STATUS_DELIVERED,
		"failed":    pb.CardStatus_CARD_STATUS_FAILED,
		"cancelled": pb.CardStatus_CARD_STATUS_CANCELLED,
	}
	for name, status := range statuses {
		t.Run(name, func(t *testing.T) {
			card := sampleCard()
			card.Status = status
			built := asAnyMap(t, BuildChecklistCard(card))
			checkCardSchema(t, built)
			if mapStr(asMap(built["header"]), "template") == "" {
				t.Error("header.template 不能为空")
			}
		})
	}
}

// TestStatusTemplatesAreDistinct 对应 test_status_templates_are_distinct。
func TestStatusTemplatesAreDistinct(t *testing.T) {
	seen := map[string]bool{}
	for _, status := range []pb.CardStatus{
		pb.CardStatus_CARD_STATUS_WORKING,
		pb.CardStatus_CARD_STATUS_DELIVERED,
		pb.CardStatus_CARD_STATUS_FAILED,
		pb.CardStatus_CARD_STATUS_CANCELLED,
	} {
		card := sampleCard()
		card.Status = status
		seen[mapStr(asMap(BuildChecklistCard(card)["header"]), "template")] = true
	}
	if len(seen) != 4 {
		t.Errorf("四种状态的配色要能一眼分开，实际只有 %d 种：%v", len(seen), seen)
	}
}

// TestEveryItemStateRenders 对应 test_every_item_state_renders（4 个参数）。
func TestEveryItemStateRenders(t *testing.T) {
	states := map[string]pb.ChecklistState{
		"todo":   pb.ChecklistState_CHECKLIST_STATE_TODO,
		"doing":  pb.ChecklistState_CHECKLIST_STATE_DOING,
		"done":   pb.ChecklistState_CHECKLIST_STATE_DONE,
		"failed": pb.ChecklistState_CHECKLIST_STATE_FAILED,
	}
	for name, state := range states {
		t.Run(name, func(t *testing.T) {
			card := sampleCard()
			card.Items = []*pb.ChecklistItemView{{Id: "c1", Text: "读取 CSV", State: state}}
			built := BuildChecklistCard(card)
			checkCardSchema(t, asAnyMap(t, built))
			if !strings.Contains(DumpsCard(built), "读取 CSV") {
				t.Error("待办正文没进卡片")
			}
			if icon := stateIcon[state]; !strings.Contains(DumpsCard(built), icon) {
				t.Errorf("状态图标 %q 没进卡片", icon)
			}
		})
	}
}

// TestEmptyChecklistStillBuilds 对应 test_empty_checklist_still_builds。
//
// 卡片先发、待办后加：items 为空时也不能构造失败。
func TestEmptyChecklistStillBuilds(t *testing.T) {
	card := sampleCard()
	card.Items = nil
	built := BuildChecklistCard(card)
	checkCardSchema(t, asAnyMap(t, built))
	if !strings.Contains(DumpsCard(built), "（还没有待办项）") {
		t.Error("空 items 要有兜底文案")
	}
}

// TestItemNoteIsRendered 对应 test_item_note_is_rendered。
func TestItemNoteIsRendered(t *testing.T) {
	body := DumpsCard(BuildChecklistCard(sampleCard()))
	if !strings.Contains(body, "共 3 个 sheet") {
		t.Error("item.note 没进卡片")
	}
	if !strings.Contains(body, "　—— 共 3 个 sheet") {
		t.Error("note 的前缀要是 U+3000 加双破折号")
	}
}

// TestTaskNoAndTitleAreInTheHeader 对应 test_task_no_and_title_are_in_the_header。
func TestTaskNoAndTitleAreInTheHeader(t *testing.T) {
	card := sampleCard()
	title := mapStr(asMap(asMap(BuildChecklistCard(card)["header"])["title"]), "content")
	if !strings.Contains(title, card.GetTaskNo()) {
		t.Errorf("标题里没有 task_no：%q", title)
	}
	if !strings.Contains(title, card.GetTitle()) {
		t.Errorf("标题里没有 title：%q", title)
	}
}

// TestFooterBecomesANote 对应 test_footer_becomes_a_note。
func TestFooterBecomesANote(t *testing.T) {
	card := sampleCard()
	var notes []map[string]any
	for _, e := range BuildChecklistCard(card)["elements"].([]any) {
		if m := asMap(e); mapStr(m, "tag") == "note" {
			notes = append(notes, m)
		}
	}
	if len(notes) == 0 {
		t.Fatal("footer 要渲染成 note 元素")
	}
	got := mapStr(asMap(asList(notes[0]["elements"])[0]), "content")
	if got != card.GetFooter() {
		t.Errorf("note 内容 = %q，要 %q", got, card.GetFooter())
	}
}

// ---------------------------------------------------------------------------
// 按钮：发出去的 value 与点回来的 card_action 要对得上
// ---------------------------------------------------------------------------

// TestButtonValueRoundTripsIntoCardAction 对应 test_button_value_round_trips_into_card_action。
func TestButtonValueRoundTripsIntoCardAction(t *testing.T) {
	card := sampleCard()
	built := asAnyMap(t, BuildChecklistCard(card))
	var value map[string]any
	for _, e := range asList(built["elements"]) {
		if m := asMap(e); mapStr(m, "tag") == "action" {
			value = asMap(asMap(asList(m["actions"])[0])["value"])
		}
	}
	if value == nil {
		t.Fatal("卡片里没有按钮")
	}

	event := NormalizeCardAction(map[string]any{
		"header": map[string]any{
			"event_id": "evt_x", "create_time": "1788915720000",
			"event_type": "card.action.trigger", "app_id": "cli_x",
		},
		"event": map[string]any{
			"operator": map[string]any{"open_id": "ou_zhang_san"},
			"action":   map[string]any{"value": value, "tag": "button"},
			"context":  map[string]any{"open_message_id": "om_card", "open_chat_id": "oc_chat"},
		},
	}, "", "default")

	if event == nil || event.GetCardAction() == nil {
		t.Fatal("按钮 value 该能被 NormalizeCardAction 读回来")
	}
	if event.GetCardAction().GetAction() != pb.CardActionKind_CARD_ACTION_KIND_STOP {
		t.Errorf("action = %v", event.GetCardAction().GetAction())
	}
	if event.GetCardAction().GetTaskId() != card.GetTaskId() {
		t.Errorf("task_id = %q，要 %q", event.GetCardAction().GetTaskId(), card.GetTaskId())
	}
}

// TestActionsAreRenderedAsGiven 对应 test_actions_are_rendered_as_given。
//
// 按 card.actions 原样渲染 —— 留不留「停止」是 worker 的决定，adapter 不替它判。
func TestActionsAreRenderedAsGiven(t *testing.T) {
	card := sampleCard()
	card.Actions = []pb.CardActionKind{
		pb.CardActionKind_CARD_ACTION_KIND_STOP,
		pb.CardActionKind_CARD_ACTION_KIND_EVIDENCE,
	}
	var names []string
	for _, e := range asList(asAnyMap(t, BuildChecklistCard(card))["elements"]) {
		if m := asMap(e); mapStr(m, "tag") == "action" {
			for _, a := range asList(m["actions"]) {
				names = append(names, mapStr(asMap(asMap(a)["value"]), "action"))
			}
		}
	}
	if strings.Join(names, ",") != "stop,evidence" {
		t.Errorf("按钮顺序 = %v，要 [stop evidence]", names)
	}

	card.Actions = nil
	for _, e := range asList(asAnyMap(t, BuildChecklistCard(card))["elements"]) {
		if mapStr(asMap(e), "tag") == "action" {
			t.Error("actions 为空时不该有 action 元素")
		}
	}

	// 不在 ACTION_BUTTON 里的名字静默丢弃。
	card.Actions = []pb.CardActionKind{
		pb.CardActionKind_CARD_ACTION_KIND_UNSPECIFIED,
		pb.CardActionKind_CARD_ACTION_KIND_STOP,
	}
	names = nil
	for _, e := range asList(asAnyMap(t, BuildChecklistCard(card))["elements"]) {
		if m := asMap(e); mapStr(m, "tag") == "action" {
			for _, a := range asList(m["actions"]) {
				names = append(names, mapStr(asMap(asMap(a)["value"]), "action"))
			}
		}
	}
	if strings.Join(names, ",") != "stop" {
		t.Errorf("未知按钮该静默丢弃，得到 %v", names)
	}
}

// ---------------------------------------------------------------------------
// 卡片 ≤30KB
// ---------------------------------------------------------------------------

// TestOversizedCardIsTrimmedUnder30KB 对应 test_oversized_card_is_trimmed_under_30kb。
//
// 待办项被模型灌爆时，宁可少显示几行，也不要整条 UpdateCard 被平台打回来。
func TestOversizedCardIsTrimmedUnder30KB(t *testing.T) {
	card := sampleCard()
	card.Items = nil
	for i := 0; i < 80; i++ {
		card.Items = append(card.Items, &pb.ChecklistItemView{
			Id:    "c" + strings.Repeat("x", i%3),
			Text:  strings.Repeat("很长的一项", 200),
			State: pb.ChecklistState_CHECKLIST_STATE_TODO,
		})
	}
	built := BuildChecklistCard(card)
	checkCardSchema(t, asAnyMap(t, built))

	body := DumpsCard(built)
	if size := len(body); size > CardMaxBytes {
		t.Errorf("卡片 %d 字节，超过 %d", size, CardMaxBytes)
	}
	if !strings.Contains(body, "未显示") {
		t.Error("被裁掉的项要在卡片上说一声")
	}
}

// TestNormalCardIsNowhereNearTheLimit 对应 test_normal_card_is_nowhere_near_the_limit。
func TestNormalCardIsNowhereNearTheLimit(t *testing.T) {
	if size := len(DumpsCard(BuildChecklistCard(sampleCard()))); size >= 2000 {
		t.Errorf("普通卡片 %d 字节，不该接近上限", size)
	}
}

// ---------------------------------------------------------------------------
// markdown 文本卡片
// ---------------------------------------------------------------------------

// TestMarkdownCardPassesSchema 对应 test_markdown_card_passes_schema。
func TestMarkdownCardPassesSchema(t *testing.T) {
	checkCardSchema(t, asAnyMap(t, BuildMarkdownCard("**已完成**\n- 见附件")))
}

// TestMarkdownCardKeepsTheSourceVerbatim 对应 test_markdown_card_keeps_the_source_verbatim。
func TestMarkdownCardKeepsTheSourceVerbatim(t *testing.T) {
	text := "| a | b |\n| - | - |\n| 1 | 2 |"
	got := mapStr(asMap(asList(asAnyMap(t, BuildMarkdownCard(text))["elements"])[0]), "content")
	if got != text {
		t.Errorf("markdown 内容 = %q，要 %q", got, text)
	}
}

// TestDumpsCardDoesNotEscapeHTMLOrNonASCII 是 Go 侧补的一条。
//
// encoding/json 默认把 & < > 转成 & 之类，而 Python 的 json.dumps 不会 ——
// 不关掉 HTML 转义，回帖里的「A & B」会以转义形态发出去，字节数也白白撑大。
func TestDumpsCardDoesNotEscapeHTMLOrNonASCII(t *testing.T) {
	got := DumpsCard(BuildMarkdownCard("A & B <c> 中文"))
	if !strings.Contains(got, "A & B <c> 中文") {
		t.Errorf("DumpsCard 转义了本不该转义的字符：%s", got)
	}
	if strings.Contains(got, `\u`) {
		t.Errorf("DumpsCard 输出里不该有 \\u 转义：%s", got)
	}
	// 紧凑分隔符：没有 ", " 也没有 ": "。
	if strings.Contains(got, ", ") || strings.Contains(got, `": "`) {
		t.Errorf("DumpsCard 要用紧凑分隔符：%s", got)
	}
}

// ---------------------------------------------------------------------------
// SDK 构造
// ---------------------------------------------------------------------------

// TestCardIsConstructibleThroughTheSDK 对应 test_card_is_constructible_through_the_sdk。
func TestCardIsConstructibleThroughTheSDK(t *testing.T) {
	content := DumpsCard(BuildChecklistCard(sampleCard()))

	body := larkim.NewCreateMessageReqBodyBuilder().
		ReceiveId(testChatID).
		MsgType("interactive").
		Content(content).
		Build()
	req := larkim.NewCreateMessageReqBuilder().
		ReceiveIdType("chat_id").
		Body(body).
		Build()

	if req == nil {
		t.Fatal("SDK 构造请求对象失败")
	}
	if body.Content == nil || *body.Content != content {
		t.Error("SDK 没把卡片 content 收进去")
	}
	var card map[string]any
	if err := json.Unmarshal([]byte(*body.Content), &card); err != nil {
		t.Fatalf("content 不是合法 JSON：%v", err)
	}
	if mapStr(asMap(asMap(card["header"])["title"]), "tag") != "plain_text" {
		t.Error("卡片头部没过 SDK")
	}
}

// TestSDKAgreesThatUpdateIsNotSend 对应 test_sdk_agrees_that_update_is_not_send。
//
// 与 Python SDK 的差异：Go SDK 的请求对象把 uri / http_method 藏在私有 apiReq 里、
// 且只在 service 方法里填，外部读不到。改为用 SDK 自己的请求体类型反证：
// 更新只收 content，发送还要 receive_id / msg_type —— 两个接口不是一回事。
// 「UpdateCard 走 PATCH 而不是 POST」由 TestUpdateCardUsesPatchNotSend 在真实
// HTTP 层钉住。
func TestSDKAgreesThatUpdateIsNotSend(t *testing.T) {
	content := DumpsCard(BuildChecklistCard(sampleCard()))

	patchBody := larkim.NewPatchMessageReqBodyBuilder().Content(content).Build()
	createBody := larkim.NewCreateMessageReqBodyBuilder().
		ReceiveId(testChatID).MsgType("interactive").Content(content).Build()

	patchKeys := jsonKeys(t, patchBody)
	createKeys := jsonKeys(t, createBody)

	if len(patchKeys) != 1 || !patchKeys["content"] {
		t.Errorf("更新卡片的请求体只该有 content，得到 %v", patchKeys)
	}
	if !createKeys["receive_id"] || !createKeys["msg_type"] {
		t.Errorf("发送消息的请求体要有 receive_id / msg_type，得到 %v", createKeys)
	}
	if len(patchKeys) == len(createKeys) {
		t.Error("SDK 自己也认：更新卡片与发送消息不是一个接口")
	}
	if PathMessage == PathMessages {
		t.Error("更新与发送的路径不能相同")
	}
}

func jsonKeys(t *testing.T, v any) map[string]bool {
	t.Helper()
	encoded, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("序列化失败：%v", err)
	}
	var m map[string]any
	if err := json.Unmarshal(encoded, &m); err != nil {
		t.Fatalf("反序列化失败：%v", err)
	}
	out := map[string]bool{}
	for k := range m {
		out[k] = true
	}
	return out
}
