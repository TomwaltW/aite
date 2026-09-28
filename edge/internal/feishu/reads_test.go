// 对应 tests/adapters/feishu/test_feishu_read.py。
//
// 读取面：群历史、云文档、消息附件下载。
//
// ReadHistory 有两条硬要求，都在这里钉住：
// 按时间正序返回；不做 sender_kind 过滤（过滤是 core 的 read_group_history 的活）。
package feishu

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"testing"

	pb "aite/edge/gen/aitepb"
)

// T0 是 2026-09-09T01:02:00Z；每条 +1 分钟。
const historyT0 = 1788915720000

type historyOpt func(map[string]any)

func withSenderType(t string) historyOpt {
	return func(m map[string]any) { asMap(m["sender"])["sender_type"] = t }
}

func withSenderName(n string) historyOpt {
	return func(m map[string]any) { asMap(m["sender"])["sender_name"] = n }
}

func withRootID(id string) historyOpt {
	return func(m map[string]any) { m["root_id"] = id }
}

func historyItem(messageID, text string, createdAt int64, opts ...historyOpt) map[string]any {
	content, _ := json.Marshal(map[string]any{"text": text})
	item := map[string]any{
		"message_id":  messageID,
		"msg_type":    "text",
		"create_time": strconv.FormatInt(createdAt, 10),
		"chat_id":     testChatID,
		"sender": map[string]any{
			"id":          "ou_" + messageID,
			"id_type":     "open_id",
			"sender_type": "user",
			"sender_name": "张三",
		},
		"body": map[string]any{"content": string(content)},
	}
	for _, o := range opts {
		o(item)
	}
	return item
}

func historyPage(items []map[string]any, hasMore bool, pageToken string) func(http.ResponseWriter) {
	list := make([]any, 0, len(items))
	for _, i := range items {
		list = append(list, i)
	}
	return jsonResponse(200, map[string]any{
		"code": 0,
		"data": map[string]any{"items": list, "has_more": hasMore, "page_token": pageToken},
	})
}

func readPlatform(t *testing.T, f *fakeFeishu) *Platform {
	t.Helper()
	p, _ := outboundPlatform(t, f)
	return p
}

// ---------------------------------------------------------------------------
// ReadHistory
// ---------------------------------------------------------------------------

// TestHistoryIsReturnedOldestFirst 对应 test_history_is_returned_oldest_first。
//
// 按时间正序返回。平台给的是倒序，adapter 负责翻回来。
func TestHistoryIsReturnedOldestFirst(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	// 平台按 ByCreateTimeDesc 返回：最新的在最前面。
	f.on(http.MethodGet, PathMessages, historyPage([]map[string]any{
		historyItem("om_c", "第三条", historyT0+120_000),
		historyItem("om_b", "第二条", historyT0+60_000),
		historyItem("om_a", "第一条", historyT0),
	}, false, ""))

	messages, err := readPlatform(t, f).ReadHistory(context.Background(), testChatID, 3, nil)
	if err != nil {
		t.Fatalf("ReadHistory 失败：%v", err)
	}
	var texts []string
	for _, m := range messages {
		texts = append(texts, m.GetText())
	}
	if fmt.Sprint(texts) != "[第一条 第二条 第三条]" {
		t.Errorf("顺序 = %v", texts)
	}
	for i := 1; i < len(messages); i++ {
		if !messages[i-1].GetCreatedAt().AsTime().Before(messages[i].GetCreatedAt().AsTime()) {
			t.Errorf("第 %d 条的时间没有递增", i)
		}
	}
}

// TestHistoryAsksForTheMostRecentWindow 对应 test_history_asks_for_the_most_recent_window。
func TestHistoryAsksForTheMostRecentWindow(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	route := f.on(http.MethodGet, PathMessages, historyPage(nil, false, ""))

	if _, err := readPlatform(t, f).ReadHistory(context.Background(), testChatID, 50, nil); err != nil {
		t.Fatalf("ReadHistory 失败：%v", err)
	}

	q := route.last(t).query
	for key, want := range map[string]string{
		"container_id_type": "chat",
		"container_id":      testChatID,
		"sort_type":         "ByCreateTimeDesc",
		// with_sender_name 传的是字符串 "true"（文档没有、SDK 有）
		"with_sender_name": "true",
		"page_size":        "50",
	} {
		if got := q.Get(key); got != want {
			t.Errorf("query[%s] = %q，要 %q", key, got, want)
		}
	}
	if q.Has("page_token") {
		t.Error("第一页不该带 page_token")
	}
}

// TestHistoryDoesNotFilterBySenderKind 对应 test_history_does_not_filter_by_sender_kind。
//
// 不做 sender_kind 过滤 —— 过滤归 core 的 read_group_history 工具。
func TestHistoryDoesNotFilterBySenderKind(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.on(http.MethodGet, PathMessages, historyPage([]map[string]any{
		historyItem("om_bot", "日报机器人播报", historyT0+60_000,
			withSenderType("app"), withSenderName("日报机器人")),
		historyItem("om_human", "收到", historyT0),
	}, false, ""))

	messages, err := readPlatform(t, f).ReadHistory(context.Background(), testChatID, 10, nil)
	if err != nil {
		t.Fatalf("ReadHistory 失败：%v", err)
	}
	var kinds []string
	for _, m := range messages {
		kinds = append(kinds, m.GetSenderKind())
	}
	if fmt.Sprint(kinds) != "[human app]" {
		t.Errorf("sender_kind = %v，要 [human app]", kinds)
	}
	if len(messages) != 2 {
		t.Errorf("机器人消息也要原样返回，得到 %d 条", len(messages))
	}
}

// TestHistoryCarriesSenderNameAndThreadID 对应 test_history_carries_sender_name_and_thread_id。
func TestHistoryCarriesSenderNameAndThreadID(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.on(http.MethodGet, PathMessages, historyPage([]map[string]any{
		historyItem("om_x", "在话题里说的", historyT0, withRootID(testRootMsgID)),
	}, false, ""))

	messages, err := readPlatform(t, f).ReadHistory(context.Background(), testChatID, 10, nil)
	if err != nil {
		t.Fatalf("ReadHistory 失败：%v", err)
	}
	m := messages[0]
	if m.GetSenderName() != "张三" {
		t.Errorf("sender_name = %q", m.GetSenderName())
	}
	if m.GetThreadId() != testRootMsgID {
		t.Errorf("thread_id = %q", m.GetThreadId())
	}
	if m.GetMessageId() != "om_x" {
		t.Errorf("message_id = %q", m.GetMessageId())
	}
	// 历史 API 形状：sender.id 而不是 sender.sender_id.open_id
	if m.GetSenderId() != "ou_om_x" {
		t.Errorf("sender_id = %q", m.GetSenderId())
	}
}

// TestHistoryCanBeNarrowedToOneThread 对应 test_history_can_be_narrowed_to_one_thread。
//
// 飞书的 container_id_type=thread 收的是 omt_ 话题 id，而锚点里存的是话题 root
// 消息 id，两者不是一个 id 空间，所以在客户端筛。
//
// CC8 起带 threadID 先查 root 的 thread_id、走 thread 容器；这条注册的 root 不带
// thread_id，**有意地**钉住回落 ①（非话题消息 → 整群拉取、客户端筛），期望不变。
func TestHistoryCanBeNarrowedToOneThread(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.on(http.MethodGet, fmt.Sprintf(PathMessage, testRootMsgID), messageGet(
		historyItem(testRootMsgID, "话题根消息", historyT0)))
	f.on(http.MethodGet, PathMessages, historyPage([]map[string]any{
		historyItem("om_other", "别的话题", historyT0+120_000),
		historyItem("om_in", "本话题里的", historyT0+60_000, withRootID(testRootMsgID)),
		historyItem(testRootMsgID, "话题根消息", historyT0),
	}, false, ""))

	messages, err := readPlatform(t, f).ReadHistory(
		context.Background(), testChatID, 10, strptr(testRootMsgID))
	if err != nil {
		t.Fatalf("ReadHistory 失败：%v", err)
	}
	var ids []string
	for _, m := range messages {
		ids = append(ids, m.GetMessageId())
	}
	if fmt.Sprint(ids) != fmt.Sprintf("[%s om_in]", testRootMsgID) {
		t.Errorf("message_id = %v", ids)
	}
}

// messageGet 是「获取指定消息的内容」的响应：data.items 是数组（SDK GetMessageRespData）。
func messageGet(item map[string]any) func(http.ResponseWriter) {
	return jsonResponse(200, map[string]any{"code": 0, "data": map[string]any{"items": []any{item}}})
}

func withThreadID(id string) historyOpt {
	return func(m map[string]any) { m["thread_id"] = id }
}

func historyIDs(messages []*pb.HistoryMessage) string {
	var ids []string
	for _, m := range messages {
		ids = append(ids, m.GetMessageId())
	}
	return fmt.Sprint(ids)
}

// TestThreadHistoryUsesThreadContainer 钉住话题历史走 thread 容器，以及两条回落。
func TestThreadHistoryUsesThreadContainer(t *testing.T) {
	const omt = "omt_x"
	rootItem := historyItem(testRootMsgID, "话题根消息", historyT0, withThreadID(omt))
	// 整群那一页：回落时客户端筛出 [root om_in]。
	chatPage := historyPage([]map[string]any{
		historyItem("om_other", "别的话题", historyT0+120_000),
		historyItem("om_in", "本话题里的", historyT0+60_000, withRootID(testRootMsgID)),
		historyItem(testRootMsgID, "话题根消息", historyT0),
	}, false, "")
	permissionDenied := jsonResponse(400, map[string]any{"code": 230002, "msg": "bot not in chat"})

	read := func(t *testing.T, f *fakeFeishu) ([]*pb.HistoryMessage, error) {
		t.Helper()
		return readPlatform(t, f).ReadHistory(context.Background(), testChatID, 10, strptr(testRootMsgID))
	}
	containers := func(r *route) []string {
		r.mu.Lock()
		defer r.mu.Unlock()
		var out []string
		for _, c := range r.calls {
			out = append(out, c.query.Get("container_id_type")+":"+c.query.Get("container_id"))
		}
		return out
	}

	t.Run("thread 容器", func(t *testing.T) {
		f := newFakeFeishu(t)
		f.mockToken()
		f.on(http.MethodGet, fmt.Sprintf(PathMessage, testRootMsgID), messageGet(rootItem))
		// thread 列表里没有 root（倒序）：结果要把 root 补上、按时间归位。
		list := f.on(http.MethodGet, PathMessages, historyPage([]map[string]any{
			historyItem("om_r2", "第二条回复", historyT0+120_000, withRootID(testRootMsgID)),
			historyItem("om_r1", "第一条回复", historyT0+60_000, withRootID(testRootMsgID)),
		}, false, ""))

		messages, err := read(t, f)
		if err != nil {
			t.Fatalf("ReadHistory 失败：%v", err)
		}
		if got := fmt.Sprint(containers(list)); got != "[thread:omt_x]" {
			t.Errorf("列表请求 = %s，要恰好一次 thread:omt_x、没有 chat 容器请求", got)
		}
		q := list.last(t).query
		for key, want := range map[string]string{
			"sort_type": "ByCreateTimeDesc", "with_sender_name": "true", "page_size": "10",
		} {
			if got := q.Get(key); got != want {
				t.Errorf("query[%s] = %q，要 %q（预算不再 ×4）", key, got, want)
			}
		}
		if got := historyIDs(messages); got != fmt.Sprintf("[%s om_r1 om_r2]", testRootMsgID) {
			t.Errorf("message_id = %s，要含 root 且正序", got)
		}
	})

	t.Run("thread 列表权限错误 → 回落整群筛法", func(t *testing.T) {
		for name, denied := range map[string]func(http.ResponseWriter){
			"业务码 230002": permissionDenied,
			"HTTP 403":   jsonResponse(403, map[string]any{"code": 99999, "msg": "forbidden"}),
		} {
			t.Run(name, func(t *testing.T) {
				f := newFakeFeishu(t)
				f.mockToken()
				f.on(http.MethodGet, fmt.Sprintf(PathMessage, testRootMsgID), messageGet(rootItem))
				list := f.on(http.MethodGet, PathMessages, denied, chatPage)

				messages, err := read(t, f)
				if err != nil {
					t.Fatalf("权限错误该回落而不是上抛：%v", err)
				}
				want := fmt.Sprintf("[thread:omt_x chat:%s]", testChatID)
				if got := fmt.Sprint(containers(list)); got != want {
					t.Errorf("列表请求 = %s，要 %s（恰好一次 chat 容器请求）", got, want)
				}
				if got := historyIDs(messages); got != fmt.Sprintf("[%s om_in]", testRootMsgID) {
					t.Errorf("message_id = %s，要与整群筛法一致", got)
				}
			})
		}
	})

	t.Run("非权限错误上抛；root 查询失败回落", func(t *testing.T) {
		f := newFakeFeishu(t)
		f.mockToken()
		f.on(http.MethodGet, fmt.Sprintf(PathMessage, testRootMsgID), messageGet(rootItem))
		list := f.on(http.MethodGet, PathMessages,
			jsonResponse(400, map[string]any{"code": 230001, "msg": "invalid params"}))
		if _, err := read(t, f); err == nil {
			t.Fatal("thread 列表回非权限错误该上抛")
		}
		if got := fmt.Sprint(containers(list)); got != "[thread:omt_x]" {
			t.Errorf("非权限错误不该回落，列表请求 = %s", got)
		}

		// root 查询失败（生产里 root 被撤回就是这样）→ 回落、不上抛。
		f2 := newFakeFeishu(t)
		f2.mockToken()
		f2.on(http.MethodGet, fmt.Sprintf(PathMessage, testRootMsgID),
			jsonResponse(400, map[string]any{"code": 230011, "msg": "message recalled"}))
		list2 := f2.on(http.MethodGet, PathMessages, chatPage)
		messages, err := read(t, f2)
		if err != nil {
			t.Fatalf("root 查询失败该回落：%v", err)
		}
		if got := fmt.Sprint(containers(list2)); got != fmt.Sprintf("[chat:%s]", testChatID) {
			t.Errorf("列表请求 = %s", got)
		}
		if got := historyIDs(messages); got != fmt.Sprintf("[%s om_in]", testRootMsgID) {
			t.Errorf("message_id = %s", got)
		}
	})
}

// TestHistoryPaginatesUntilTheLimitIsFilled
// 对应 test_history_paginates_until_the_limit_is_filled。
func TestHistoryPaginatesUntilTheLimitIsFilled(t *testing.T) {
	var first, second []map[string]any
	for i := 60; i > 10; i-- {
		first = append(first, historyItem(fmt.Sprintf("om_%d", i), fmt.Sprintf("第 %d 条", i), historyT0+int64(i)*1000))
	}
	for i := 10; i > 0; i-- {
		second = append(second, historyItem(fmt.Sprintf("om_%d", i), fmt.Sprintf("第 %d 条", i), historyT0+int64(i)*1000))
	}

	f := newFakeFeishu(t)
	f.mockToken()
	route := f.on(http.MethodGet, PathMessages,
		historyPage(first, true, "pt2"),
		historyPage(second, false, ""),
	)

	messages, err := readPlatform(t, f).ReadHistory(context.Background(), testChatID, 55, nil)
	if err != nil {
		t.Fatalf("ReadHistory 失败：%v", err)
	}
	if route.count() != 2 {
		t.Errorf("请求次数 = %d，要 2", route.count())
	}
	if got := route.at(t, 1).query.Get("page_token"); got != "pt2" {
		t.Errorf("第二次的 page_token = %q，要 pt2", got)
	}
	if len(messages) != 55 {
		t.Errorf("条数 = %d，要 55", len(messages))
	}
	if !messages[0].GetCreatedAt().AsTime().Before(messages[len(messages)-1].GetCreatedAt().AsTime()) {
		t.Error("结果要是正序")
	}
	// page_size 随已收数量收缩：第二页只该再要 5 条。
	if got := route.at(t, 1).query.Get("page_size"); got != "5" {
		t.Errorf("第二次的 page_size = %q，要 5", got)
	}
}

// TestHistoryLimitZeroMakesNoRequest 对应 test_history_limit_zero_makes_no_request。
func TestHistoryLimitZeroMakesNoRequest(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	route := f.on(http.MethodGet, PathMessages, historyPage(nil, false, ""))

	messages, err := readPlatform(t, f).ReadHistory(context.Background(), testChatID, 0, nil)
	if err != nil {
		t.Fatalf("ReadHistory 失败：%v", err)
	}
	if len(messages) != 0 {
		t.Errorf("limit=0 要返回空，得到 %d 条", len(messages))
	}
	if route.count() != 0 {
		t.Errorf("limit=0 不该发请求，发了 %d 次", route.count())
	}
}

// TestHistoryNonTextMessagesHaveEmptyText
// 对应 test_history_non_text_messages_have_empty_text。
func TestHistoryNonTextMessagesHaveEmptyText(t *testing.T) {
	item := historyItem("om_file", "", historyT0)
	item["msg_type"] = "file"
	item["body"] = map[string]any{"content": `{"file_key":"file_v2_x","file_name":"a.csv"}`}

	f := newFakeFeishu(t)
	f.mockToken()
	f.on(http.MethodGet, PathMessages, historyPage([]map[string]any{item}, false, ""))

	messages, err := readPlatform(t, f).ReadHistory(context.Background(), testChatID, 5, nil)
	if err != nil {
		t.Fatalf("ReadHistory 失败：%v", err)
	}
	if messages[0].GetText() != "" {
		t.Errorf("非文本消息 text 要为空，得到 %q", messages[0].GetText())
	}
}

// ---------------------------------------------------------------------------
// ReadDocument
// ---------------------------------------------------------------------------

// TestReadDocxDocument 对应 test_read_docx_document。
func TestReadDocxDocument(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	meta := f.onJSON(http.MethodGet, fmt.Sprintf(PathDocxDocument, "DocTokenAbc123"), 200,
		map[string]any{"code": 0, "data": map[string]any{"document": map[string]any{"title": "指标字典"}}})
	raw := f.onJSON(http.MethodGet, fmt.Sprintf(PathDocxRawContent, "DocTokenAbc123"), 200,
		map[string]any{"code": 0, "data": map[string]any{"content": "GMV 口径：…"}})

	doc, err := readPlatform(t, f).ReadDocument(
		context.Background(), "https://example.feishu.cn/docx/DocTokenAbc123")
	if err != nil {
		t.Fatalf("ReadDocument 失败：%v", err)
	}
	if !meta.called() || !raw.called() {
		t.Fatal("元信息与正文两条路都要打")
	}
	if doc.GetTitle() != "指标字典" {
		t.Errorf("title = %q", doc.GetTitle())
	}
	if doc.GetText() != "GMV 口径：…" {
		t.Errorf("text = %q", doc.GetText())
	}
	if doc.GetUrl() != "https://example.feishu.cn/docx/DocTokenAbc123" {
		t.Errorf("url = %q", doc.GetUrl())
	}
	if got := raw.last(t).query.Get("lang"); got != "0" {
		t.Errorf("正文要带 lang=0，得到 %q", got)
	}
}

// TestReadWikiDocumentResolvesToItsDocx
// 对应 test_read_wiki_document_resolves_to_its_docx。
func TestReadWikiDocumentResolvesToItsDocx(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	node := f.onJSON(http.MethodGet, PathWikiNode, 200, map[string]any{
		"code": 0,
		"data": map[string]any{"node": map[string]any{"obj_token": "DocReal999", "obj_type": "docx"}},
	})
	f.onJSON(http.MethodGet, fmt.Sprintf(PathDocxDocument, "DocReal999"), 200,
		map[string]any{"code": 0, "data": map[string]any{"document": map[string]any{"title": "周会纪要"}}})
	f.onJSON(http.MethodGet, fmt.Sprintf(PathDocxRawContent, "DocReal999"), 200,
		map[string]any{"code": 0, "data": map[string]any{"content": "正文"}})

	doc, err := readPlatform(t, f).ReadDocument(
		context.Background(), "https://example.feishu.cn/wiki/WikiToken777")
	if err != nil {
		t.Fatalf("ReadDocument 失败：%v", err)
	}
	if got := node.last(t).query.Get("token"); got != "WikiToken777" {
		t.Errorf("wiki token = %q", got)
	}
	if got := node.last(t).query.Get("obj_type"); got != "wiki" {
		t.Errorf("obj_type = %q", got)
	}
	if doc.GetTitle() != "周会纪要" {
		t.Errorf("title = %q", doc.GetTitle())
	}
}

// TestReadDocumentAcceptsABareToken 对应 test_read_document_accepts_a_bare_token。
func TestReadDocumentAcceptsABareToken(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.onJSON(http.MethodGet, fmt.Sprintf(PathDocxDocument, "DocTokenAbc123"), 200,
		map[string]any{"code": 0, "data": map[string]any{"document": map[string]any{"title": "T"}}})
	f.onJSON(http.MethodGet, fmt.Sprintf(PathDocxRawContent, "DocTokenAbc123"), 200,
		map[string]any{"code": 0, "data": map[string]any{"content": "c"}})

	doc, err := readPlatform(t, f).ReadDocument(context.Background(), "DocTokenAbc123")
	if err != nil {
		t.Fatalf("ReadDocument 失败：%v", err)
	}
	if doc.GetTitle() != "T" {
		t.Errorf("title = %q", doc.GetTitle())
	}
	// 非 http 输入时 url 合成成相对路径。
	if doc.GetUrl() != "/docx/DocTokenAbc123" {
		t.Errorf("url = %q", doc.GetUrl())
	}
}

// TestParseDocRefHandlesEveryShape 是 Go 侧补的：裸 token 的三步清洗逐条。
func TestParseDocRefHandlesEveryShape(t *testing.T) {
	cases := []struct{ in, kind, token string }{
		{"https://example.feishu.cn/docx/DocTokenAbc123", "docx", "DocTokenAbc123"},
		{"https://example.feishu.cn/docs/OldStyleToken1", "docs", "OldStyleToken1"},
		{"https://example.feishu.cn/wiki/WikiToken777?from=x", "wiki", "WikiToken777"},
		{"DocTokenAbc123", "docx", "DocTokenAbc123"},
		{"  DocTokenAbc123  ", "docx", "DocTokenAbc123"},
		{"DocTokenAbc123?sheet=1", "docx", "DocTokenAbc123"},
	}
	for _, c := range cases {
		kind, token := parseDocRef(c.in)
		if kind != c.kind || token != c.token {
			t.Errorf("parseDocRef(%q) = (%q, %q)，要 (%q, %q)", c.in, kind, token, c.kind, c.token)
		}
	}
}

// ---------------------------------------------------------------------------
// DownloadFile
// ---------------------------------------------------------------------------

// TestDownloadFileUsesTypeFile 对应 test_download_file_uses_type_file。
func TestDownloadFileUsesTypeFile(t *testing.T) {
	key := "file_v2_0a1b2c3d"
	f := newFakeFeishu(t)
	f.mockToken()
	route := f.on(http.MethodGet, fmt.Sprintf(PathMessageResource, "om_with_file_0005", key),
		rawResponseBody(200, "text/csv", []byte("a,b\n1,2\n")))

	data, err := readPlatform(t, f).DownloadFile(context.Background(), "om_with_file_0005", key)
	if err != nil {
		t.Fatalf("DownloadFile 失败：%v", err)
	}
	if string(data) != "a,b\n1,2\n" {
		t.Errorf("下载内容 = %q", data)
	}
	if got := route.last(t).query.Get("type"); got != "file" {
		t.Errorf("type = %q，要 file", got)
	}
}

// TestDownloadImageUsesTypeImage 对应 test_download_image_uses_type_image。
func TestDownloadImageUsesTypeImage(t *testing.T) {
	key := "img_v3_0a1b2c3d"
	png := []byte("\x89PNG\r\n\x1a\n")
	f := newFakeFeishu(t)
	f.mockToken()
	route := f.on(http.MethodGet, fmt.Sprintf(PathMessageResource, "om_post_0007", key),
		rawResponseBody(200, "image/png", png))

	data, err := readPlatform(t, f).DownloadFile(context.Background(), "om_post_0007", key)
	if err != nil {
		t.Fatalf("DownloadFile 失败：%v", err)
	}
	if string(data[:4]) != "\x89PNG" {
		t.Errorf("下载内容 = %q", data)
	}
	if got := route.last(t).query.Get("type"); got != "image" {
		t.Errorf("type = %q，要 image", got)
	}
}
