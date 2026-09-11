// 对应 tests/adapters/feishu/test_feishu_errors.py。
//
// 失败面：
//
//	| 场景          | 期望                                                        |
//	|---------------|-------------------------------------------------------------|
//	| 429 / 5xx     | 退避重试 3 次（0.5s / 1s / 2s），仍失败抛 retryable=true       |
//	| 4xx（非 429） | 不重试，抛 retryable=false                                    |
package feishu

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// expectedRetryDelays 是写死的三个间隔。
var expectedRetryDelays = []time.Duration{500 * time.Millisecond, time.Second, 2 * time.Second}

func errorPlatform(t *testing.T, f *fakeFeishu, clock *fakeClock) *Platform {
	t.Helper()
	_, logger := newLogCapture()
	return mustPlatform(t, platformBuild{
		domain: f.URL, clock: clock.Now, sleep: clock.Sleep, logger: logger,
	})
}

func sendText(ctx context.Context, p *Platform) error {
	_, err := p.SendText(ctx, &pb.OutboundText{ChatId: testChatID, Text: "hi"})
	return err
}

func asPlatformError(t *testing.T, err error) *aiteerr.PlatformError {
	t.Helper()
	var pe *aiteerr.PlatformError
	if !errors.As(err, &pe) {
		t.Fatalf("要 *aiteerr.PlatformError，得到 %T：%v", err, err)
	}
	return pe
}

func sameDelays(got []time.Duration, want []time.Duration) bool {
	if len(got) != len(want) {
		return false
	}
	for i := range got {
		if got[i] != want[i] {
			return false
		}
	}
	return true
}

// ---------------------------------------------------------------------------
// 退避间隔本身
// ---------------------------------------------------------------------------

// TestRetryDelaysAreFrozenBySpec 对应 test_retry_delays_are_frozen_by_the_spec。
func TestRetryDelaysAreFrozenBySpec(t *testing.T) {
	if !sameDelays(RetryDelays, expectedRetryDelays) {
		t.Errorf("RetryDelays = %v，要 %v", RetryDelays, expectedRetryDelays)
	}
}

// ---------------------------------------------------------------------------
// 429 / 5xx：重试 3 次后 retryable=true
// ---------------------------------------------------------------------------

// TestRetryableStatusRetriesThreeTimesThenRaises
// 对应 test_retryable_status_retries_three_times_then_raises（4 个参数）。
func TestRetryableStatusRetriesThreeTimesThenRaises(t *testing.T) {
	for _, status := range []int{429, 500, 502, 503} {
		t.Run(fmt.Sprint(status), func(t *testing.T) {
			f := newFakeFeishu(t)
			f.mockToken()
			route := f.onJSON(http.MethodPost, PathMessages, status,
				map[string]any{"code": 99991400, "msg": "too fast"})

			clock := newFakeClock()
			err := sendText(context.Background(), errorPlatform(t, f, clock))
			if err == nil {
				t.Fatal("该抛 PlatformError")
			}

			if route.count() != 4 {
				t.Errorf("请求次数 = %d，要 4（一次原始请求 + 三次重试）", route.count())
			}
			if got := clock.Slept(); !sameDelays(got, expectedRetryDelays) {
				t.Errorf("slept = %v，要 %v", got, expectedRetryDelays)
			}
			pe := asPlatformError(t, err)
			if !pe.Retryable {
				t.Error("retryable 要是 true")
			}
			if pe.HTTPStatus != status {
				t.Errorf("http_status = %d，要 %d", pe.HTTPStatus, status)
			}
			if pe.Code != "99991400" {
				t.Errorf("code = %q", pe.Code)
			}
		})
	}
}

// TestRetryLogsEachAttempt 是 Go 侧补的：重试前打 feishu.retry。
func TestRetryLogsEachAttempt(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.onJSON(http.MethodPost, PathMessages, 503, map[string]any{"code": 1, "msg": "busy"})

	clock := newFakeClock()
	capture, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{
		domain: f.URL, clock: clock.Now, sleep: clock.Sleep, logger: logger,
	})
	_ = sendText(context.Background(), p)

	records := capture.find("feishu.retry")
	if len(records) != 3 {
		t.Fatalf("feishu.retry 该打 3 条，得到 %d 条", len(records))
	}
	for i, r := range records {
		for key, want := range map[string]any{
			"method": http.MethodPost, "path": PathMessages,
			"status": int64(503), "attempt": int64(i + 1),
		} {
			v, ok := attr(r, key)
			if !ok {
				t.Fatalf("第 %d 条缺 %s", i+1, key)
			}
			if fmt.Sprint(v.Any()) != fmt.Sprint(want) {
				t.Errorf("第 %d 条 %s = %v，要 %v", i+1, key, v.Any(), want)
			}
		}
	}
}

// TestRetryStopsAsSoonAsItSucceeds 对应 test_retry_stops_as_soon_as_it_succeeds。
func TestRetryStopsAsSoonAsItSucceeds(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	route := f.on(http.MethodPost, PathMessages,
		jsonResponse(503, map[string]any{"code": 1, "msg": "busy"}),
		jsonResponse(200, map[string]any{"code": 0, "data": map[string]any{"message_id": "om_ok"}}),
	)

	clock := newFakeClock()
	result, err := errorPlatform(t, f, clock).SendText(
		context.Background(), &pb.OutboundText{ChatId: testChatID, Text: "hi"})
	if err != nil {
		t.Fatalf("第二次该成功：%v", err)
	}
	if result.GetMessageId() != "om_ok" {
		t.Errorf("message_id = %q", result.GetMessageId())
	}
	if route.count() != 2 {
		t.Errorf("请求次数 = %d，要 2", route.count())
	}
	if got := clock.Slept(); !sameDelays(got, expectedRetryDelays[:1]) {
		t.Errorf("slept = %v，要 [500ms]", got)
	}
}

// TestTransportErrorIsRetryable 对应 test_transport_error_is_retryable。
//
// 连不上和对面挂了，对调用方是一回事。
func TestTransportErrorIsRetryable(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	// panic(http.ErrAbortHandler) 让 httptest 直接掐断连接，客户端看到传输层错误。
	route := f.on(http.MethodPost, PathMessages, func(http.ResponseWriter) {
		panic(http.ErrAbortHandler)
	})

	clock := newFakeClock()
	err := sendText(context.Background(), errorPlatform(t, f, clock))
	if err == nil {
		t.Fatal("该抛 PlatformError")
	}

	if route.count() != 4 {
		t.Errorf("请求次数 = %d，要 4", route.count())
	}
	if got := clock.Slept(); !sameDelays(got, expectedRetryDelays) {
		t.Errorf("slept = %v，要 %v", got, expectedRetryDelays)
	}
	pe := asPlatformError(t, err)
	if !pe.Retryable {
		t.Error("retryable 要是 true")
	}
	if pe.Code != "transport_error" {
		t.Errorf("code = %q，要 transport_error", pe.Code)
	}
	if pe.HTTPStatus != 0 {
		t.Errorf("传输层错误没有 http_status，得到 %d", pe.HTTPStatus)
	}
}

// TestTimeoutGetsItsOwnCode 是 Go 侧补的一条。
//
// 与 Python 的一处有意差异：Python 把超时也归进 transport_error；Go 侧单独给
// code="timeout"，好让 aiteerr 的冻结映射把它翻成 DEADLINE_EXCEEDED
// （spec §2.2「超时 → DEADLINE_EXCEEDED」）。两者 retryable 都是 true。
func TestTimeoutGetsItsOwnCode(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	f.on(http.MethodPost, PathMessages, func(w http.ResponseWriter) {
		time.Sleep(200 * time.Millisecond)
		_, _ = w.Write([]byte(`{"code":0,"data":{}}`))
	})

	clock := newFakeClock()
	_, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{
		domain: f.URL, clock: clock.Now, sleep: clock.Sleep, logger: logger,
		httpClient: &http.Client{Timeout: 20 * time.Millisecond},
	})

	err := sendText(context.Background(), p)
	if err == nil {
		t.Fatal("该抛 PlatformError")
	}
	pe := asPlatformError(t, err)
	if pe.Code != "timeout" {
		t.Errorf("code = %q，要 timeout", pe.Code)
	}
	if !pe.Retryable {
		t.Error("retryable 要是 true")
	}
}

// ---------------------------------------------------------------------------
// 4xx（非 429）：不重试，retryable=false
// ---------------------------------------------------------------------------

// TestClientErrorIsNotRetried 对应 test_client_error_is_not_retried（4 个参数）。
func TestClientErrorIsNotRetried(t *testing.T) {
	for _, status := range []int{400, 403, 404, 422} {
		t.Run(fmt.Sprint(status), func(t *testing.T) {
			f := newFakeFeishu(t)
			f.mockToken()
			route := f.onJSON(http.MethodPost, PathMessages, status,
				map[string]any{"code": 230001, "msg": "invalid params"})

			clock := newFakeClock()
			err := sendText(context.Background(), errorPlatform(t, f, clock))
			if err == nil {
				t.Fatal("该抛 PlatformError")
			}

			if route.count() != 1 {
				t.Errorf("请求次数 = %d，要 1（4xx 重试多少次都是这个结果）", route.count())
			}
			if got := clock.Slept(); len(got) != 0 {
				t.Errorf("不该有任何退避等待，得到 %v", got)
			}
			pe := asPlatformError(t, err)
			if pe.Retryable {
				t.Error("retryable 要是 false")
			}
			if pe.Code != "230001" {
				t.Errorf("code = %q", pe.Code)
			}
			if pe.HTTPStatus != status {
				t.Errorf("http_status = %d，要 %d", pe.HTTPStatus, status)
			}
		})
	}
}

// TestBusinessErrorOnHTTP200IsNotRetryable
// 对应 test_business_error_on_http_200_is_not_retryable。
//
// 飞书常在 HTTP 200 里回业务错误码；那也是参数/权限问题，重试没意义。
func TestBusinessErrorOnHTTP200IsNotRetryable(t *testing.T) {
	f := newFakeFeishu(t)
	f.mockToken()
	route := f.onJSON(http.MethodPost, PathMessages, 200,
		map[string]any{"code": 230002, "msg": "bot not in chat"})

	clock := newFakeClock()
	err := sendText(context.Background(), errorPlatform(t, f, clock))
	if err == nil {
		t.Fatal("该抛 PlatformError")
	}
	if route.count() != 1 {
		t.Errorf("请求次数 = %d，要 1", route.count())
	}
	pe := asPlatformError(t, err)
	if pe.Retryable {
		t.Error("retryable 要是 false")
	}
	if pe.Code != "230002" {
		t.Errorf("code = %q", pe.Code)
	}
	if pe.HTTPStatus != 200 {
		t.Errorf("http_status = %d，要 200", pe.HTTPStatus)
	}
	if !contains(err.Error(), "bot not in chat") {
		t.Errorf("错误信息里要带 msg，得到 %q", err.Error())
	}
}

func contains(s, sub string) bool {
	return len(s) >= len(sub) && (len(sub) == 0 || indexOf(s, sub) >= 0)
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}

// TestBadCredentialsDoNotRetry 对应 test_bad_credentials_do_not_retry。
func TestBadCredentialsDoNotRetry(t *testing.T) {
	f := newFakeFeishu(t)
	tokenRoute := f.onJSON(http.MethodPost, PathTenantToken, 200,
		map[string]any{"code": 10003, "msg": "invalid app_secret"})

	clock := newFakeClock()
	err := sendText(context.Background(), errorPlatform(t, f, clock))
	if err == nil {
		t.Fatal("该抛 PlatformError")
	}
	pe := asPlatformError(t, err)
	if pe.Retryable {
		t.Error("应用凭证不对，重试 3 次也还是不对")
	}
	if pe.Code != "10003" {
		t.Errorf("code = %q", pe.Code)
	}
	if tokenRoute.count() != 1 {
		t.Errorf("token 请求次数 = %d，要 1", tokenRoute.count())
	}
}

// TestTokenResponseWithoutTokenIsRejected 是 Go 侧补的：code=0 但没 token。
func TestTokenResponseWithoutTokenIsRejected(t *testing.T) {
	f := newFakeFeishu(t)
	f.onJSON(http.MethodPost, PathTenantToken, 200, map[string]any{"code": 0, "msg": "ok"})

	clock := newFakeClock()
	err := sendText(context.Background(), errorPlatform(t, f, clock))
	if err == nil {
		t.Fatal("该抛 PlatformError")
	}
	pe := asPlatformError(t, err)
	if pe.Code != "no_token" {
		t.Errorf("code = %q，要 no_token", pe.Code)
	}
	if pe.Retryable {
		t.Error("retryable 要是 false")
	}
}

// ---------------------------------------------------------------------------
// token 过期
// ---------------------------------------------------------------------------

// TestExpiredTokenIsRefreshedOnce 对应 test_expired_token_is_refreshed_once。
//
// 401 换一张 token 再打一次；这一次不吃三次退避额度。
func TestExpiredTokenIsRefreshedOnce(t *testing.T) {
	f := newFakeFeishu(t)
	tokenRoute := f.on(http.MethodPost, PathTenantToken,
		jsonResponse(200, map[string]any{"code": 0, "tenant_access_token": "t-old", "expire": 7200}),
		jsonResponse(200, map[string]any{"code": 0, "tenant_access_token": "t-new", "expire": 7200}),
	)
	sendRoute := f.on(http.MethodPost, PathMessages,
		jsonResponse(401, map[string]any{"code": 99991663, "msg": "token expired"}),
		jsonResponse(200, map[string]any{"code": 0, "data": map[string]any{"message_id": "om_ok"}}),
	)

	clock := newFakeClock()
	result, err := errorPlatform(t, f, clock).SendText(
		context.Background(), &pb.OutboundText{ChatId: testChatID, Text: "hi"})
	if err != nil {
		t.Fatalf("换 token 后该成功：%v", err)
	}
	if result.GetMessageId() != "om_ok" {
		t.Errorf("message_id = %q", result.GetMessageId())
	}
	if tokenRoute.count() != 2 {
		t.Errorf("token 请求次数 = %d，要 2", tokenRoute.count())
	}
	if sendRoute.count() != 2 {
		t.Errorf("发送请求次数 = %d，要 2", sendRoute.count())
	}
	if got := sendRoute.at(t, 1).header.Get("Authorization"); got != "Bearer t-new" {
		t.Errorf("第二次的 Authorization = %q，要 Bearer t-new", got)
	}
	if got := clock.Slept(); len(got) != 0 {
		t.Errorf("401 换 token 不吃退避额度，slept = %v", got)
	}
}

// TestTokenIsCachedAcrossCalls 对应 test_token_is_cached_across_calls。
func TestTokenIsCachedAcrossCalls(t *testing.T) {
	f := newFakeFeishu(t)
	tokenRoute := f.mockToken()
	f.onJSON(http.MethodPost, PathMessages, 200,
		map[string]any{"code": 0, "data": map[string]any{"message_id": "om"}})

	clock := newFakeClock()
	p := errorPlatform(t, f, clock)
	for i := 0; i < 5; i++ {
		if err := sendText(context.Background(), p); err != nil {
			t.Fatalf("第 %d 次发送失败：%v", i+1, err)
		}
	}
	if tokenRoute.count() != 1 {
		t.Errorf("token 请求次数 = %d，要 1（跨调用缓存）", tokenRoute.count())
	}
}

// TestTokenIsRefreshedBeforeItExpires 是 Go 侧补的：提前 60s 换新的。
func TestTokenIsRefreshedBeforeItExpires(t *testing.T) {
	f := newFakeFeishu(t)
	tokenRoute := f.mockToken()
	f.onJSON(http.MethodPost, PathMessages, 200,
		map[string]any{"code": 0, "data": map[string]any{"message_id": "om"}})

	clock := newFakeClock()
	p := errorPlatform(t, f, clock)
	if err := sendText(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	// expire=7200，安全窗 60s → 7139s 时还在缓存里。
	clock.Advance(7139 * time.Second)
	if err := sendText(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	if tokenRoute.count() != 1 {
		t.Errorf("还没到换新时点，token 请求次数 = %d，要 1", tokenRoute.count())
	}
	// 再走 2s 就越过 7140s 的门槛。
	clock.Advance(2 * time.Second)
	if err := sendText(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	if tokenRoute.count() != 2 {
		t.Errorf("过了安全窗要换新，token 请求次数 = %d，要 2", tokenRoute.count())
	}
}

// ---------------------------------------------------------------------------
// PlatformError 本身
// ---------------------------------------------------------------------------

// TestPlatformErrorSignatureMatchesTheSpec
// 对应 test_platform_error_signature_matches_the_spec。
//
// 与 Python 的一处形状差异：aiteerr.PlatformError 的 Code 是 string（R0 冻结），
// 飞书的数字 code 一律转成十进制字符串。
func TestPlatformErrorSignatureMatchesTheSpec(t *testing.T) {
	err := &aiteerr.PlatformError{Code: "bad", Retryable: true}
	if err.Code != "bad" {
		t.Errorf("code = %q", err.Code)
	}
	if !err.Retryable {
		t.Error("retryable 要是 true")
	}
	if err.Msg != "" {
		t.Errorf("message = %q，要空", err.Msg)
	}
	var asErr error = err
	if asErr == nil {
		t.Error("PlatformError 要实现 error")
	}
}
