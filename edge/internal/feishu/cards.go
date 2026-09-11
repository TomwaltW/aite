// 对应 aite/adapters/feishu/cards.py。
//
// pb.ChecklistCard → 飞书消息卡片 JSON。
//
// 用 v1 卡片（config / header / elements）。两个点是硬要求：
//
//   - config.update_multi = true —— 不开这个，「更新应用发送的消息卡片」只对第一个
//     看到卡片的人生效，群里其他人看到的还是旧卡片，「原地更新」就废了。
//   - 按钮 value 里带 {"action": ..., "task_id": ...} —— 这正是 NormalizeCardAction
//     读的字段，卡片发出去和点回来是同一套口径。
//
// 卡片 ≤30KB。这里在序列化后兜一道，超了就从尾部丢待办项，
// 宁可少显示几行，也不要整条 UpdateCard 被平台打回来。
package feishu

import (
	"bytes"
	"encoding/json"
	"fmt"
	"strings"

	pb "aite/edge/gen/aitepb"
)

// CardMaxBytes 是卡片大小上限。留点余量给平台自己包的信封。
const CardMaxBytes = 30_000

// statusTemplate 是任务状态 → 卡片头部配色。
var statusTemplate = map[pb.CardStatus]string{
	pb.CardStatus_CARD_STATUS_WORKING:   "blue",
	pb.CardStatus_CARD_STATUS_DELIVERED: "green",
	pb.CardStatus_CARD_STATUS_FAILED:    "red",
	pb.CardStatus_CARD_STATUS_CANCELLED: "grey",
}

var statusLabel = map[pb.CardStatus]string{
	pb.CardStatus_CARD_STATUS_WORKING:   "进行中",
	pb.CardStatus_CARD_STATUS_DELIVERED: "已交付",
	pb.CardStatus_CARD_STATUS_FAILED:    "失败",
	pb.CardStatus_CARD_STATUS_CANCELLED: "已取消",
}

// stateIcon 是 checklist 单项状态 → 行首图标。
var stateIcon = map[pb.ChecklistState]string{
	pb.ChecklistState_CHECKLIST_STATE_TODO:   "⬜",
	pb.ChecklistState_CHECKLIST_STATE_DOING:  "🔄",
	pb.ChecklistState_CHECKLIST_STATE_DONE:   "✅",
	pb.ChecklistState_CHECKLIST_STATE_FAILED: "❌",
}

// actionButton 是卡片按钮：action 名 → (按钮文案, 按钮样式)。
var actionButton = map[pb.CardActionKind]struct{ text, style string }{
	pb.CardActionKind_CARD_ACTION_KIND_STOP:     {"停止", "danger"},
	pb.CardActionKind_CARD_ACTION_KIND_EVIDENCE: {"证据", "default"},
}

// actionName 是按钮 value 里写的 action 名，与 NormalizeCardAction 读的口径一致。
var actionName = map[pb.CardActionKind]string{
	pb.CardActionKind_CARD_ACTION_KIND_STOP:     "stop",
	pb.CardActionKind_CARD_ACTION_KIND_EVIDENCE: "evidence",
}

func plainText(content string) map[string]any {
	return map[string]any{"tag": "plain_text", "content": content}
}

func larkMD(content string) map[string]any {
	return map[string]any{"tag": "lark_md", "content": content}
}

func itemLines(card *pb.ChecklistCard) []string {
	lines := make([]string, 0, len(card.GetItems()))
	for _, item := range card.GetItems() {
		icon, ok := stateIcon[item.GetState()]
		if !ok {
			icon = "⬜"
		}
		line := icon + " " + item.GetText()
		if note := item.GetNote(); note != "" {
			line += "　—— " + note
		}
		lines = append(lines, line)
	}
	return lines
}

func cardElements(card *pb.ChecklistCard, lines []string, dropped int) []any {
	label, ok := statusLabel[card.GetStatus()]
	if !ok {
		label = card.GetStatus().String()
	}
	elements := []any{
		map[string]any{
			"tag": "div",
			"fields": []any{
				map[string]any{"is_short": true, "text": larkMD("**发起人**\n" + card.GetInitiator())},
				map[string]any{"is_short": true, "text": larkMD("**开始于**\n" + card.GetStartedAt())},
				map[string]any{"is_short": true, "text": larkMD("**状态**\n" + label)},
			},
		},
		map[string]any{"tag": "hr"},
	}

	body := strings.Join(lines, "\n")
	if len(lines) == 0 {
		body = "_（还没有待办项）_"
	}
	if dropped > 0 {
		body += fmt.Sprintf("\n…… 另有 %d 项未显示", dropped)
	}
	elements = append(elements, map[string]any{"tag": "div", "text": larkMD(body)})

	if footer := card.GetFooter(); footer != "" {
		elements = append(elements, map[string]any{
			"tag":      "note",
			"elements": []any{plainText(footer)},
		})
	}

	// 按 card.actions 原样渲染：什么时候还该留「停止」按钮是 worker 的决定，
	// adapter 不替它判断。不认识的名字静默丢弃。
	var actions []any
	for _, kind := range card.GetActions() {
		button, ok := actionButton[kind]
		if !ok {
			continue
		}
		actions = append(actions, map[string]any{
			"tag":   "button",
			"text":  plainText(button.text),
			"type":  button.style,
			"value": map[string]any{"action": actionName[kind], "task_id": card.GetTaskId()},
		})
	}
	if len(actions) > 0 {
		elements = append(elements, map[string]any{"tag": "action", "actions": actions})
	}

	return elements
}

// BuildChecklistCard 把 ChecklistCard 渲染成飞书卡片 JSON。
func BuildChecklistCard(card *pb.ChecklistCard) map[string]any {
	title := strings.TrimSpace(card.GetTaskNo() + " " + card.GetTitle())
	lines := itemLines(card)
	dropped := 0

	for {
		template, ok := statusTemplate[card.GetStatus()]
		if !ok {
			template = "blue"
		}
		payload := map[string]any{
			// update_multi 必须为 true，否则 UpdateCard 只对单个用户生效。
			"config": map[string]any{"wide_screen_mode": true, "update_multi": true},
			"header": map[string]any{
				"template": template,
				"title":    plainText(title),
			},
			"elements": cardElements(card, lines, dropped),
		}
		if len(DumpsCard(payload)) <= CardMaxBytes || len(lines) == 0 {
			return payload
		}
		lines = lines[:len(lines)-1]
		dropped++
	}
}

// DumpsCard 把卡片 JSON 序列化成发送用的字符串。飞书的 content 收的是字符串而不是对象。
//
// 对齐 Python 的 json.dumps(payload, ensure_ascii=False, separators=(",", ":"))：
// 紧凑、非 ASCII 不转义，所以必须关掉 encoding/json 默认的 HTML 转义
// （否则 & < > 会变成 & 之类，白白撑大字节数还改了文案）。
func DumpsCard(payload map[string]any) string {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(payload); err != nil {
		return ""
	}
	// Encode 会在末尾补一个换行，去掉它才与 json.dumps 逐字节一致。
	return strings.TrimSuffix(buf.String(), "\n")
}

// BuildMarkdownCard 把一段 markdown 包成只有一个 markdown 元素的卡片。
//
// OutboundText.text 的契约注释写的是「markdown（飞书 post/markdown 由 adapter 转）」。
// 飞书里唯一真能渲染 markdown 的载体就是卡片的 markdown 元素 —— msg_type=text
// 会把 **粗体**、列表、链接原样当字面量吐出来。所以文本出站统一走这个卡片。
func BuildMarkdownCard(text string) map[string]any {
	return map[string]any{
		"config":   map[string]any{"wide_screen_mode": true, "update_multi": true},
		"elements": []any{map[string]any{"tag": "markdown", "content": text}},
	}
}
