package dingtalk

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// 钉：两次调用只打一次 token 端点；时钟推过期后再打一次；日志里没有 secret / token。
func TestAccessTokenIsCachedUntilExpiry(t *testing.T) {
	f := newFakeDingtalk(t)
	token := f.mockToken()
	group := f.onJSON(http.MethodPost, PathGroupSend, 200, map[string]any{"processQueryKey": "pqk-1"})
	clock := newFakeClock()
	logs, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{apiBase: f.URL, clock: clock, logger: logger})
	ctx := context.Background()

	for i := 0; i < 2; i++ {
		if _, err := p.SendText(ctx, &pb.OutboundText{ChatId: testGroupChat, Text: "hi"}); err != nil {
			t.Fatalf("第 %d 次 SendText 失败：%v", i+1, err)
		}
	}
	if token.count() != 1 {
		t.Fatalf("两次调用打了 %d 次 token 端点，期望 1", token.count())
	}
	req := token.at(t, 0).jsonBody(t)
	if req["appKey"] != testClientID || req["appSecret"] != testClientSecret {
		t.Errorf("token 请求体 = %v", req)
	}
	if got := group.at(t, 1).header.Get(HeaderAccessToken); got != testAccessToken {
		t.Errorf("%s 头 = %q", HeaderAccessToken, got)
	}

	// 过期前 60 秒就换新：7200 - 60 = 7140 秒。
	clock.Advance(7139 * time.Second)
	if _, err := p.SendText(ctx, &pb.OutboundText{ChatId: testGroupChat, Text: "hi"}); err != nil {
		t.Fatal(err)
	}
	if token.count() != 1 {
		t.Fatalf("还没到换新点就重取了 token（%d 次）", token.count())
	}
	clock.Advance(2 * time.Second)
	if _, err := p.SendText(ctx, &pb.OutboundText{ChatId: testGroupChat, Text: "hi"}); err != nil {
		t.Fatal(err)
	}
	if token.count() != 2 {
		t.Fatalf("过期后打了 %d 次 token 端点，期望 2", token.count())
	}

	// 401 → 作废重取一次。
	f.on(http.MethodPost, PathGroupSend,
		jsonResponse(401, map[string]any{"code": "InvalidAuthentication", "message": "token expired"}),
		jsonResponse(200, map[string]any{"processQueryKey": "pqk-2"}))
	res, err := p.SendText(ctx, &pb.OutboundText{ChatId: testGroupChat, Text: "hi"})
	if err != nil || res.GetMessageId() != "pqk-2" {
		t.Fatalf("401 后重取 token 再发：res=%v err=%v", res, err)
	}
	if token.count() != 3 {
		t.Fatalf("401 后 token 端点共 %d 次，期望 3", token.count())
	}

	dump := logs.dump()
	for _, secret := range []string{testClientSecret, testAccessToken} {
		if strings.Contains(dump, secret) {
			t.Errorf("日志里出现了 %q：\n%s", secret, dump)
		}
	}
}

// fakeWebhook 是 sessionWebhook 的假服务端。
type fakeWebhook struct {
	*httptest.Server
	mu    sync.Mutex
	calls []map[string]any
}

func newFakeWebhook(t *testing.T) *fakeWebhook {
	w := &fakeWebhook{}
	w.Server = httptest.NewServer(http.HandlerFunc(func(rw http.ResponseWriter, req *http.Request) {
		body, _ := io.ReadAll(req.Body)
		var m map[string]any
		_ = json.Unmarshal(body, &m)
		m["_query_session"] = req.URL.Query().Get("session")
		m["_token_header"] = req.Header.Get(HeaderAccessToken)
		w.mu.Lock()
		w.calls = append(w.calls, m)
		w.mu.Unlock()
		_, _ = rw.Write([]byte(`{"errcode":0,"errmsg":"ok"}`))
	}))
	t.Cleanup(w.Close)
	return w
}

func (w *fakeWebhook) count() int {
	w.mu.Lock()
	defer w.mu.Unlock()
	return len(w.calls)
}

// 钉：有效 webhook → 只打 webhook；过期 → 群走 groupMessages、单聊走 oToMessages 且 userIds 对；
// MessageId 分别是空与 processQueryKey；webhook 服务挂掉后错误与日志里不含会话令牌。
func TestSendTextUsesSessionWebhookThenRobotAPI(t *testing.T) {
	const sessionToken = "SESSION-SECRET-TOKEN-777"
	f := newFakeDingtalk(t)
	f.mockToken()
	group := f.onJSON(http.MethodPost, PathGroupSend, 200, map[string]any{"processQueryKey": "pqk-group"})
	oto := f.onJSON(http.MethodPost, PathOToBatchSend, 200, map[string]any{"processQueryKey": "pqk-oto"})
	hook := newFakeWebhook(t)
	clock := newFakeClock()
	logs, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{apiBase: f.URL, clock: clock, logger: logger})
	ctx := context.Background()

	webhookURL := hook.URL + "/robot/sendBySession?session=" + sessionToken
	expire := float64(clock.Now().Add(10 * time.Minute).UnixMilli())
	p.sessions.remember(map[string]any{
		"conversationId": testGroupChat, "conversationType": "2", "senderStaffId": testStaffID,
		"chatbotCorpId": testCorpID, "sessionWebhook": webhookURL, "sessionWebhookExpiredTime": expire,
	})
	p.sessions.remember(map[string]any{
		"conversationId": testP2PChat, "conversationType": "1", "senderStaffId": testStaffID,
		"sessionWebhook": webhookURL, "sessionWebhookExpiredTime": expire,
	})

	// 1) webhook 有效 → 只打 webhook，MessageId 空。
	res, err := p.SendText(ctx, &pb.OutboundText{ChatId: testGroupChat, Text: "## 进展\n已完成 2/3", ReplyTo: strptr("m"), InThread: true})
	if err != nil {
		t.Fatalf("webhook 发送失败：%v", err)
	}
	if res.GetMessageId() != "" {
		t.Errorf("webhook 路的 MessageId = %q，期望空", res.GetMessageId())
	}
	if hook.count() != 1 || group.count() != 0 || oto.count() != 0 {
		t.Fatalf("webhook=%d group=%d oto=%d，期望 1/0/0", hook.count(), group.count(), oto.count())
	}
	hook.mu.Lock()
	call := hook.calls[0]
	hook.mu.Unlock()
	if call["msgtype"] != "markdown" || call["_query_session"] != sessionToken || call["_token_header"] != "" {
		t.Errorf("webhook 请求不对：%v", call)
	}
	if md, _ := call["markdown"].(map[string]any); md["text"] != "## 进展\n已完成 2/3" || md["title"] != "进展" {
		t.Errorf("webhook markdown = %v", call["markdown"])
	}

	// 2) 推到过期前 60 秒以内 → 走机器人 API。
	clock.Advance(9*time.Minute + 1*time.Second)
	res, err = p.SendText(ctx, &pb.OutboundText{ChatId: testGroupChat, Text: "群里的话"})
	if err != nil {
		t.Fatalf("groupMessages 发送失败：%v", err)
	}
	if res.GetMessageId() != "pqk-group" || hook.count() != 1 || group.count() != 1 {
		t.Fatalf("过期后群消息：MessageId=%q webhook=%d group=%d", res.GetMessageId(), hook.count(), group.count())
	}
	gb := group.at(t, 0).jsonBody(t)
	if gb["openConversationId"] != testGroupChat || gb["robotCode"] != testRobotCode || gb["msgKey"] != "sampleMarkdown" {
		t.Errorf("groupMessages 请求体 = %v", gb)
	}
	var param map[string]string
	if err := json.Unmarshal([]byte(gb["msgParam"].(string)), &param); err != nil || param["text"] != "群里的话" {
		t.Errorf("msgParam = %v（%v）", gb["msgParam"], err)
	}

	res, err = p.SendText(ctx, &pb.OutboundText{ChatId: testP2PChat, Text: "单聊的话"})
	if err != nil {
		t.Fatalf("oToMessages 发送失败：%v", err)
	}
	if res.GetMessageId() != "pqk-oto" || oto.count() != 1 {
		t.Fatalf("单聊：MessageId=%q oto=%d", res.GetMessageId(), oto.count())
	}
	ob := oto.at(t, 0).jsonBody(t)
	if !reflect.DeepEqual(ob["userIds"], []any{testStaffID}) || ob["robotCode"] != testRobotCode {
		t.Errorf("oToMessages 请求体 = %v", ob)
	}
	if _, ok := ob["openConversationId"]; ok {
		t.Errorf("oToMessages 不该带 openConversationId：%v", ob)
	}

	// 3) 单聊缓存里没 staffId → 不可重试的 PlatformError。
	p.sessions.put("cidNOSTAFF", sessionInfo{conversationType: "1"})
	_, err = p.SendText(ctx, &pb.OutboundText{ChatId: "cidNOSTAFF", Text: "x"})
	var pe *aiteerr.PlatformError
	if !errors.As(err, &pe) || pe.Retryable {
		t.Errorf("缺 staffId 应为不可重试 PlatformError，得到 %v", err)
	}

	// 4) msg == nil → bad_request。
	if _, err := p.SendText(ctx, nil); !errors.As(err, &pe) || pe.Code != "bad_request" {
		t.Errorf("nil msg 应为 bad_request，得到 %v", err)
	}

	// 5) webhook 假服务关掉后再发：错误与日志里都不含会话令牌。
	p.sessions.remember(map[string]any{
		"conversationId": testGroupChat, "conversationType": "2",
		"sessionWebhook": webhookURL, "sessionWebhookExpiredTime": float64(clock.Now().Add(time.Hour).UnixMilli()),
	})
	hook.Close()
	_, err = p.SendText(ctx, &pb.OutboundText{ChatId: testGroupChat, Text: "x"})
	if !errors.As(err, &pe) {
		t.Fatalf("webhook 挂掉应返回 PlatformError，得到 %T %v", err, err)
	}
	if !pe.Retryable || (pe.Code != "transport_error" && pe.Code != "timeout") {
		t.Errorf("传输失败应可重试：%+v", pe)
	}
	if strings.Contains(pe.Msg, sessionToken) || strings.Contains(pe.Error(), sessionToken) {
		t.Errorf("错误文本里有会话令牌：%s", pe.Msg)
	}
	if !strings.Contains(pe.Msg, "/robot/sendBySession") {
		t.Errorf("错误文本应保留去掉 query 的 URL 便于排障：%s", pe.Msg)
	}
	if dump := logs.dump(); strings.Contains(dump, sessionToken) {
		t.Errorf("日志里有会话令牌：\n%s", dump)
	}
}

// 钉：两步请求体与返回字节逐字；第二步不带 token 头。
func TestDownloadFileViaDownloadCode(t *testing.T) {
	f := newFakeDingtalk(t)
	f.mockToken()
	payload := []byte("\x89PNG\r\n\x1a\nfake-bytes-中文")
	dl := f.onJSON(http.MethodPost, PathMessageFileDL, 200, map[string]any{
		"downloadUrl": f.URL + "/files/abc.png?Signature=FAKE-SIG",
	})
	get := f.on(http.MethodGet, "/files/abc.png", func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write(payload)
	})
	p := mustPlatform(t, platformBuild{apiBase: f.URL})

	got, err := p.DownloadFile(context.Background(), "msgPIC01", "pdc_pic_01")
	if err != nil {
		t.Fatalf("DownloadFile 失败：%v", err)
	}
	if string(got) != string(payload) {
		t.Errorf("字节不对：%q", got)
	}
	body := dl.at(t, 0).jsonBody(t)
	if !reflect.DeepEqual(body, map[string]any{"downloadCode": "pdc_pic_01", "robotCode": testRobotCode}) {
		t.Errorf("download 请求体 = %v", body)
	}
	if dl.at(t, 0).header.Get(HeaderAccessToken) != testAccessToken {
		t.Error("第一步应带 token 头")
	}
	second := get.at(t, 0)
	if second.header.Get(HeaderAccessToken) != "" {
		t.Error("第二步 GET 不该带 token 头")
	}
	if second.query.Get("Signature") != "FAKE-SIG" {
		t.Errorf("第二步 query = %v", second.query)
	}
}
