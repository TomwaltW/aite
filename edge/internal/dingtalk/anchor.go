// 可见 #A 锚点。钉钉没有话题：续接一条任务靠正文里的 #A 任务号，或引用回复里被引用内容的 #A。
//
// H9 探针（总计划 §8 H9）确认前，#A 文本是主锚点；Anchor.thread_id 里放的 repliedMsg.msgId
// 只是提示 —— 它能不能对上 core 存的会话 id 要等 H9 真机核实，对不上 core 走 R7，与不填一样。
package dingtalk

import (
	"regexp"
	"strings"

	pb "aite/edge/gen/aitepb"
)

// taskNoRe 匹配 #A + Crockford base32（字母表去掉了 I L O U），不分大小写。
// 「只认 #A\d+」是错的：17 → #AH、1000 → #AZ8。
var taskNoRe = regexp.MustCompile(`(?i)#A([0-9A-HJKMNP-TV-Z]+)`)

// TaskNoOf 从一段文本里取第一个 #A 任务号，转大写。
//
// 紧跟在号码后面的若是 ASCII 字母或数字就不算（"#Awesome" 不是锚点）；
// 中文等非 ASCII 字符可以紧跟（"#AH进展如何" → "#AH"）。
func TaskNoOf(text string) (string, bool) {
	for _, loc := range taskNoRe.FindAllStringSubmatchIndex(text, -1) {
		end := loc[1]
		if end < len(text) && isASCIIAlnum(text[end]) {
			continue
		}
		return "#A" + strings.ToUpper(text[loc[2]:loc[3]]), true
	}
	return "", false
}

func isASCIIAlnum(b byte) bool {
	return (b >= '0' && b <= '9') || (b >= 'a' && b <= 'z') || (b >= 'A' && b <= 'Z')
}

// anchorOf 组装机器人消息的 Anchor：先看本条正文，找不到再看引用内容；thread_id 只作提示。
func anchorOf(data map[string]any, chatID, msgID, text string) *pb.Anchor {
	anchor := &pb.Anchor{
		Platform:  platformName,
		ChatId:    chatID,
		MessageId: msgID,
	}
	quote := ParseQuote(data)
	if no, ok := TaskNoOf(text); ok {
		anchor.TaskNo = &no
	} else if quote != nil {
		if no, ok := TaskNoOf(quote.Text); ok {
			anchor.TaskNo = &no
		}
	}
	if quote != nil && quote.MessageID != "" {
		tid := quote.MessageID
		anchor.ThreadId = &tid
	}
	return anchor
}
