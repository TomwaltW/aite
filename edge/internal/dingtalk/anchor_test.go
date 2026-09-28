package dingtalk

import "testing"

// 钉：Crockford 向量；先本文、再引用；thread_id = repliedMsg.msgId。
func TestAnchorTaskNoFromOwnTextThenQuote(t *testing.T) {
	for _, c := range []struct {
		in   string
		want string // "" = 无锚点
	}{
		{"#A1", "#A1"},
		{"#AH", "#AH"},
		{"#A10", "#A10"},
		{"#AZ8", "#AZ8"},
		{"看看 #ah 呢", "#AH"},
		{"#AH进展如何", "#AH"},
		{"#Awesome", ""},
		{"#A", ""},
		{"没有锚点", ""},
		{"#Awesome 之后是 #A2", "#A2"},
	} {
		got, ok := TaskNoOf(c.in)
		if c.want == "" {
			if ok {
				t.Errorf("TaskNoOf(%q) = %q，期望无锚点", c.in, got)
			}
			continue
		}
		if !ok || got != c.want {
			t.Errorf("TaskNoOf(%q) = %q,%v，期望 %q", c.in, got, ok, c.want)
		}
	}

	quoted := func(own, quote string) map[string]any {
		return map[string]any{
			"msgId": "msgSELF",
			"text": map[string]any{
				"content":    own,
				"isReplyMsg": true,
				"repliedMsg": map[string]any{"msgId": "msgQUOTED", "content": map[string]any{"text": quote}},
			},
		}
	}

	// 本文与引用都有 → 取本文。
	a := anchorOf(quoted("@Aite #AZ8 呢", "#AH 已开始"), "cid", "msgSELF", "#AZ8 呢")
	if a.TaskNo == nil || *a.TaskNo != "#AZ8" {
		t.Errorf("本文与引用都有时 TaskNo = %v，期望 #AZ8", deref(a.TaskNo))
	}
	if a.ThreadId == nil || *a.ThreadId != "msgQUOTED" {
		t.Errorf("ThreadId = %v，期望 msgQUOTED", deref(a.ThreadId))
	}
	if a.GetMessageId() != "msgSELF" || a.GetChatId() != "cid" || a.GetPlatform() != "dingtalk" {
		t.Errorf("Anchor 基本字段不对：%v", a)
	}

	// 只在引用里 → 取引用（小写也转大写）。
	a = anchorOf(quoted("@Aite 继续", "#ah 已开始"), "cid", "msgSELF", "继续")
	if a.TaskNo == nil || *a.TaskNo != "#AH" {
		t.Errorf("只在引用里时 TaskNo = %v，期望 #AH", deref(a.TaskNo))
	}

	// 都没有 → nil；没有引用 → ThreadId nil。
	a = anchorOf(map[string]any{"msgId": "m", "text": map[string]any{"content": "hi"}}, "cid", "m", "hi")
	if a.TaskNo != nil || a.ThreadId != nil {
		t.Errorf("无锚点无引用时 TaskNo=%v ThreadId=%v，期望都 nil", deref(a.TaskNo), deref(a.ThreadId))
	}
}
