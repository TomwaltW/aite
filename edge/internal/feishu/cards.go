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

	// 按钮**当前不渲染**（RΩ 的处置，见 review-findings §二 G1）。
	//
	// 事实（BB5 2026-09-15 在 lark-oapi-go v3.12.0 源码上一手复核。模块 zip 的
	// h1 与 sum.golang.org 逐字一致，读的就是官方发布件，不是本机改过的副本）：
	//   ws/client_message.go:79  `if MessageType(messageType) != MessageTypeEvent || c.eventHandler == nil { return }`
	//                            —— 判据是帧的 `type` 头（const.go:10 的注释写着「Event/Card」）。
	//                            帧已经收全、拼好、打完 debug 日志，然后在这里 return：
	//                            `card.action.trigger` 到不了任何 handler，也不留计数器；
	//   ws/client.go:56-60       唯一的 `WithCardHandler` 钩子是**注释掉的**；
	//   ws/client.go:24          `cardHandler` 字段还在，但全 SDK **没人写也没人读**；
	//   ws/const.go:27           `MessageTypeCard` 定义了，全 SDK 仅此一处，无人引用。
	// Python 靠 monkeypatch 私有方法绕过（T16），Go 没有这条路：丢弃发生在任何 handler
	// 被调用**之前**，`handleDataFrame` 是私有的，`c.eventHandler` 又是具体类型
	// （`*dispatcher.EventDispatcher`，不是接口），包不住也换不掉。
	//
	// 于是渲染出来的按钮点了**一定没反应**。渲染一个点不动的按钮比不渲染更糟：
	// 用户会以为自己点了、以为任务在停。所以这里只留一行提示，告诉他怎么真的停下来。
	// `!stop` / `!status` 走的是普通消息事件，完全不受这个缺陷影响。
	//
	// **渲染那条路没删**（`buildActions` 还在，往返也还被测着）。但「SDK 哪天放开钩子
	// 就改回来」这句话不够准，改回来之前有两件事要先算清楚：
	//
	//  1. 上游要改的是**两处**，不是一处。只把 `WithCardHandler` 取消注释没有用 ——
	//     :79 那道 `type` 闸门也得放行，否则帧根本走不到 `cardHandler`（它现在是个
	//     死字段）。2026-09-15 查：v3.12.0 已是 proxy.golang.org 上的最新版
	//     （tag 于 2026-09-10），开发主干 v3_main 里那五行仍然是注释，上游自己的
	//     card 例子走的是 HTTP webhook —— **当前没有可升的版本**。
	//  2. core 侧**从来不往 `actions` 里写 EVIDENCE**：`worker/src/card.rs:83` 与
	//     `control/src/card.rs:83` 都硬编码 `vec![Stop]`。所以就算闸门放开、这里也
	//     接回来了，渲染出来的仍然只有「停止」一个按钮，「证据」一个都不会有。
	//     **「看证据」的缺口不在 SDK 上**，见 README §已知边界。
	//
	// 闸门这件事有 `card_frames_test.go` 的真长连接实测钉着：那条测试红了就说明
	// SDK 修好了（或平台改用 event 帧发卡片回传），那时才轮到重新算第 2 条。
	if hint := actionHint(card); hint != "" {
		elements = append(elements, map[string]any{
			"tag":      "note",
			"elements": []any{plainText(hint)},
		})
	}

	return elements
}

// buildActions 按 card.actions 原样渲染按钮：什么时候还该留「停止」是 worker 的决定，
// adapter 不替它判断。不认识的名字静默丢弃。
//
// 现在没有调用方把它拼进卡片（见 `cardElements` 里那段注释），但它与
// `NormalizeCardAction` 是一对：按钮 value 的形状与读回来的口径必须始终对得上，
// 所以留着并继续测。
func buildActions(card *pb.ChecklistCard) []any {
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
	return actions
}

// actionHint 是按钮的替代品：本来会有「停止」按钮的卡片上，改成告诉用户发什么命令。
// 终态卡片（没有 actions）不加这一行 —— 已经结束的任务没什么可停的。
//
// 光说「发 !stop」不够，**必须连投递条件一起说**。控制面的 R5 是
// `text.starts_with('!') && (ev.mentioned || 话题里已有会话)`（core plane.rs），
// 两个条件都不满足的一条群消息接着往下走：R6 要 thread 命中会话、R7 要 mentioned，
// 全不命中 → R8「其余丢弃」，只 bump `events.ignored`，用户那边零回复、零反应。
// 所以「请在群里发 !stop」这种说法本身就是又一个「点了没反应的按钮」。
//
// 两条真的走得通的路：
//  1. 在这条话题里回复 —— 卡片是 SendCard 发的，reply_to = 话题 root、
//     reply_in_thread 恒为 true（platform.go 的 SendCard / sendMessage），
//     所以卡片一定在任务话题内；话题里的回复带 root_id，Normalize 取它当 thread_id，
//     find_session_by_thread 命中 → R5 收下，不需要 @。
//  2. 在群里 @ 机器人再发 —— mentioned=true，同样命中 R5。
func actionHint(card *pb.ChecklistCard) string {
	for _, kind := range card.GetActions() {
		if kind == pb.CardActionKind_CARD_ACTION_KIND_STOP {
			no := card.GetTaskNo()
			return "要停这个任务：在本话题里回复 !stop " + no +
				"，或在群里发「@我 !stop " + no +
				"」。（既不 @ 我、也不在本话题里的命令会被丢弃，不会有任何回应。）"
		}
	}
	return ""
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
