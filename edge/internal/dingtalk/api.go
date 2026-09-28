// 钉钉新版 REST 客户端（net/http 直打，照 feishu/api.go 的形状）。
//
// 路径来源：只有 Stream 的 open / 帧 / ACK 形状核对过官方 SDK 源码；下面 Path* 常量里的
// REST 路径、请求头与请求体键名来自总管 09-25 的调研记忆，没对过官方文档（云端打不开），
// 全部列进 docs/p1/dingtalk.md 的「待 H9 真机核实」。
//
// 失败面：HTTP ≥ 400 → PlatformError{Code: 响应体 code 或 HTTP 状态, Retryable: 429/5xx}；
// 传输失败 → timeout / transport_error，可重试。401 时作废 token 重取一次。
// 本客户端不做退避重试：重试由 core 按 retryable 决定。
package dingtalk

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"aite/edge/internal/aiteerr"
)

// 各接口路径（待 H9 真机核实）。
const (
	PathAccessToken      = "/v1.0/oauth2/accessToken"
	PathGroupSend        = "/v1.0/robot/groupMessages/send"
	PathOToBatchSend     = "/v1.0/robot/oToMessages/batchSend"
	PathMessageFileDL    = "/v1.0/robot/messageFiles/download"
	PathCardCreate       = "/v1.0/card/instances/createAndDeliver"
	PathCardInstances    = "/v1.0/card/instances"
	PathCardStreaming    = "/v1.0/card/streaming"
	HeaderAccessToken    = "x-acs-dingtalk-access-token"
	OpenSpaceGroupPrefix = "dtv1.card//IM_GROUP."
	OpenSpaceRobotPrefix = "dtv1.card//IM_ROBOT."
)

// tokenSafetySec 是 token 过期前多久就提前换新的。
const tokenSafetySec = 60

type apiClient struct {
	appKey    string
	appSecret string
	base      string

	http   *http.Client
	clock  clockFunc
	logger *slog.Logger

	tokenMu       sync.Mutex
	token         string
	tokenExpireAt time.Time
}

type apiOptions struct {
	appKey     string
	appSecret  string
	base       string
	httpClient *http.Client
	clock      clockFunc
	logger     *slog.Logger
}

func newAPIClient(o apiOptions) *apiClient {
	if o.base == "" {
		o.base = DefaultAPIBase
	}
	if o.httpClient == nil {
		o.httpClient = &http.Client{Timeout: 30 * time.Second}
	}
	if o.clock == nil {
		o.clock = time.Now
	}
	if o.logger == nil {
		o.logger = slog.Default()
	}
	return &apiClient{
		appKey:    o.appKey,
		appSecret: o.appSecret,
		base:      strings.TrimRight(o.base, "/"),
		http:      o.httpClient,
		clock:     o.clock,
		logger:    o.logger,
	}
}

// accessToken 拿企业内部应用 accessToken，带过期缓存（提前 60 秒换新）。
func (c *apiClient) accessToken(ctx context.Context) (string, error) {
	c.tokenMu.Lock()
	defer c.tokenMu.Unlock()
	if c.token != "" && c.clock().Before(c.tokenExpireAt) {
		return c.token, nil
	}
	status, body, err := c.send(ctx, http.MethodPost, c.base+PathAccessToken,
		map[string]any{"appKey": c.appKey, "appSecret": c.appSecret}, "")
	if err != nil {
		return "", err
	}
	if status >= 400 {
		pe := errorFromResponse(status, body)
		// 凭证不对重试多少次都是这个结果；429/5xx 保持可重试。
		return "", pe
	}
	parsed := jsonBody(body)
	token := mapStr(parsed, "accessToken")
	if token == "" {
		return "", &aiteerr.PlatformError{Code: "no_token", Retryable: false, Msg: "响应里没有 accessToken"}
	}
	expire := int64(7200)
	if n, err := strconv.ParseInt(mapStr(parsed, "expireIn"), 10, 64); err == nil && n > 0 {
		expire = n
	}
	safe := expire - tokenSafetySec
	if safe < 60 {
		safe = 60
	}
	c.token = token
	c.tokenExpireAt = c.clock().Add(time.Duration(safe) * time.Second)
	c.logger.Debug("dingtalk.token_refreshed", "expire_sec", expire)
	return token, nil
}

func (c *apiClient) invalidateToken() {
	c.tokenMu.Lock()
	defer c.tokenMu.Unlock()
	c.token = ""
	c.tokenExpireAt = time.Time{}
}

// call 打一次带 token 的 REST 请求，返回解析后的 JSON 响应体。401 时作废 token 重取一次。
func (c *apiClient) call(ctx context.Context, method, path string, body any) (map[string]any, error) {
	status, data, err := c.callOnce(ctx, method, path, body)
	if err != nil {
		return nil, err
	}
	if status == http.StatusUnauthorized {
		c.invalidateToken()
		if status, data, err = c.callOnce(ctx, method, path, body); err != nil {
			return nil, err
		}
	}
	if status >= 400 {
		c.logger.Warn("dingtalk.api_failed", "method", method, "path", path, "status", status)
		return nil, errorFromResponse(status, data)
	}
	return jsonBody(data), nil
}

func (c *apiClient) callOnce(ctx context.Context, method, path string, body any) (int, []byte, error) {
	token, err := c.accessToken(ctx)
	if err != nil {
		return 0, nil, err
	}
	return c.send(ctx, method, c.base+path, body, token)
}

// send 是一次裸 HTTP 往返。target 可能带敏感 query（sessionWebhook / downloadUrl），
// 传输错误里的 URL 一律去掉 query 再往外交。
func (c *apiClient) send(ctx context.Context, method, target string, body any, token string) (int, []byte, error) {
	var reader io.Reader
	if body != nil {
		encoded, err := json.Marshal(body)
		if err != nil {
			return 0, nil, &aiteerr.PlatformError{Code: "bad_request", Retryable: false, Msg: err.Error()}
		}
		reader = bytes.NewReader(encoded)
	}
	req, err := http.NewRequestWithContext(ctx, method, target, reader)
	if err != nil {
		return 0, nil, &aiteerr.PlatformError{Code: "bad_request", Retryable: false, Msg: sanitizeURLError(err).Error()}
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	req.Header.Set("Accept", "application/json")
	if token != "" {
		req.Header.Set(HeaderAccessToken, token)
	}
	resp, err := c.http.Do(req)
	if err != nil {
		safe := sanitizeURLError(err)
		c.logger.Warn("dingtalk.transport_failed", "method", method, "err", safe)
		return 0, nil, transportError(safe)
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return 0, nil, transportError(sanitizeURLError(err))
	}
	return resp.StatusCode, data, nil
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

// sanitizeURLError 把 *url.Error 里的 URL 换成去掉 query 的形式。
//
// 不能照抄 feishu 的 transportError（它把 err.Error() 整个放进 Msg）：net/http 的
// *url.Error 文本带完整 URL，而 sessionWebhook 的 query 里就是会话令牌。
func sanitizeURLError(err error) error {
	var ue *url.Error
	if !errors.As(err, &ue) {
		return err
	}
	return &url.Error{Op: ue.Op, URL: stripQuery(ue.URL), Err: ue.Err}
}

// stripQuery 去掉 URL 的 query 与 fragment；解析不了就整段不要。
func stripQuery(raw string) string {
	u, err := url.Parse(raw)
	if err != nil {
		return "<url>"
	}
	u.RawQuery = ""
	u.Fragment = ""
	u.User = nil
	return u.String()
}

// transportError 把传输层失败包成 PlatformError；调用方先 sanitizeURLError。
func transportError(err error) *aiteerr.PlatformError {
	code := "transport_error"
	if isTimeout(err) {
		code = "timeout"
	}
	return &aiteerr.PlatformError{Code: code, Retryable: true, Msg: err.Error()}
}

func isTimeout(err error) bool {
	var t interface{ Timeout() bool }
	if errors.As(err, &t) && t.Timeout() {
		return true
	}
	return errors.Is(err, context.DeadlineExceeded)
}

// errorFromResponse 把 HTTP 失败翻成 PlatformError。钉钉新版错误体形如 {"code":"…","message":"…"}。
func errorFromResponse(status int, body []byte) *aiteerr.PlatformError {
	parsed := jsonBody(body)
	code := mapStr(parsed, "code")
	if code == "" {
		code = strconv.Itoa(status)
	}
	msg := mapStr(parsed, "message")
	if msg == "" {
		msg = truncateRunes(string(body), 200)
	}
	return &aiteerr.PlatformError{
		Code:       code,
		HTTPStatus: status,
		Retryable:  status == http.StatusTooManyRequests || status >= 500,
		Msg:        msg,
	}
}

func truncateRunes(s string, n int) string {
	runes := []rune(s)
	if len(runes) <= n {
		return s
	}
	return string(runes[:n])
}
