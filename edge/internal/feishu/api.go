// 对应 aite/adapters/feishu/api.py。
//
// 飞书开放平台 HTTP 客户端（net/http 直打）。
//
// 为什么不用 SDK 自带的 API client：判据要求断言 HTTP 方法与路径（httptest 拦的是
// http.Client），而 SDK 把请求包了好几层。所以出站走 net/http，长连接才用 SDK
// （ws.Client，见 connection.go）。
//
// 路径不是猜的：逐条对过 SDK 里生成的请求类（它们就是官方 OpenAPI 描述文件的产物）。
//
// 失败面：
//   - 429 / 5xx / 传输层错误 → 退避重试 3 次（0.5s / 1s / 2s），仍失败抛 retryable=true
//   - 4xx（非 429）          → 不重试，抛 retryable=false
package feishu

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"mime/multipart"
	"net/http"
	"net/textproto"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"aite/edge/internal/aiteerr"
)

// DefaultDomain 是飞书开放平台的默认域名。
const DefaultDomain = "https://open.feishu.cn"

// RetryDelays 是 429/5xx 的三次退避间隔（合计 4 次请求）。
var RetryDelays = []time.Duration{500 * time.Millisecond, time.Second, 2 * time.Second}

// 各接口路径 —— 与 SDK 生成的请求类逐字对齐。
const (
	PathTenantToken     = "/open-apis/auth/v3/tenant_access_token/internal"
	PathMessages        = "/open-apis/im/v1/messages"          // CreateMessageRequest / ListMessageRequest
	PathMessage         = "/open-apis/im/v1/messages/%s"       // PatchMessageRequest（PATCH）
	PathMessageReply    = "/open-apis/im/v1/messages/%s/reply" //
	PathMessageReaction = "/open-apis/im/v1/messages/%s/reactions"
	PathMessageResource = "/open-apis/im/v1/messages/%s/resources/%s"
	PathFiles           = "/open-apis/im/v1/files"
	PathImages          = "/open-apis/im/v1/images"
	PathDocxDocument    = "/open-apis/docx/v1/documents/%s"
	PathDocxRawContent  = "/open-apis/docx/v1/documents/%s/raw_content"
	PathWikiNode        = "/open-apis/wiki/v2/spaces/get_node"
)

// tokenSafetySec 是 token 过期前多久就提前换新的。
const tokenSafetySec = 60

// uploadFile 是 multipart 里的那一个文件部件。
type uploadFile struct {
	field    string
	filename string
	mime     string
	data     []byte
}

// apiRequest 是一次飞书 REST 调用的全部参数。
type apiRequest struct {
	method string
	path   string
	params map[string]string
	body   any               // JSON 请求体
	file   *uploadFile       // multipart 文件部件
	form   map[string]string // multipart 表单字段
	// rateLimited=true 的调用先过令牌桶 —— 出站消息类接口才受 outbound_rate_per_min
	// 约束，读接口与上传接口不占这个额度。
	rateLimited bool
	// binary=true 直接返回响应字节，完全跳过业务码检查。
	binary bool
}

// apiClient 是带鉴权、限速、退避重试的飞书 REST 客户端。
type apiClient struct {
	appID     string
	appSecret string
	domain    string

	http        *http.Client
	retryDelays []time.Duration
	sleep       sleeperFunc
	clock       clockFunc
	bucket      *TokenBucket
	logger      *slog.Logger

	tokenMu       sync.Mutex
	token         string
	tokenExpireAt time.Time
}

type apiOptions struct {
	appID       string
	appSecret   string
	domain      string
	ratePerMin  int
	retryDelays []time.Duration
	sleep       sleeperFunc
	clock       clockFunc
	httpClient  *http.Client
	logger      *slog.Logger
	timeout     time.Duration
}

func newAPIClient(opts apiOptions) (*apiClient, error) {
	if opts.domain == "" {
		opts.domain = DefaultDomain
	}
	if opts.ratePerMin <= 0 {
		opts.ratePerMin = 60
	}
	if opts.retryDelays == nil {
		opts.retryDelays = RetryDelays
	}
	if opts.clock == nil {
		opts.clock = time.Now
	}
	if opts.sleep == nil {
		opts.sleep = realSleep
	}
	if opts.logger == nil {
		opts.logger = slog.Default()
	}
	if opts.timeout <= 0 {
		opts.timeout = 30 * time.Second
	}
	if opts.httpClient == nil {
		opts.httpClient = &http.Client{Timeout: opts.timeout}
	}
	bucket, err := NewTokenBucket(opts.ratePerMin, 0, opts.clock, opts.sleep)
	if err != nil {
		return nil, err
	}
	return &apiClient{
		appID:       opts.appID,
		appSecret:   opts.appSecret,
		domain:      strings.TrimRight(opts.domain, "/"),
		http:        opts.httpClient,
		retryDelays: opts.retryDelays,
		sleep:       opts.sleep,
		clock:       opts.clock,
		bucket:      bucket,
		logger:      opts.logger,
	}, nil
}

// -- 鉴权 -------------------------------------------------------------------

// tenantAccessToken 拿 tenant_access_token，带过期缓存。
func (c *apiClient) tenantAccessToken(ctx context.Context) (string, error) {
	c.tokenMu.Lock()
	defer c.tokenMu.Unlock()
	if c.token != "" && c.clock().Before(c.tokenExpireAt) {
		return c.token, nil
	}
	resp, err := c.rawRequest(ctx, apiRequest{
		method: http.MethodPost,
		path:   PathTenantToken,
		body:   map[string]any{"app_id": c.appID, "app_secret": c.appSecret},
	}, false)
	if err != nil {
		return "", err
	}
	body := jsonBody(resp.body)
	if code := codeOf(body); code != "0" {
		// 应用凭证不对，重试多少次都是这个结果。
		return "", &aiteerr.PlatformError{
			Code:       code,
			HTTPStatus: resp.status,
			Retryable:  false,
			Msg:        mapStr(body, "msg"),
		}
	}
	token := mapStr(body, "tenant_access_token")
	if token == "" {
		return "", &aiteerr.PlatformError{
			Code:      "no_token",
			Retryable: false,
			Msg:       "响应里没有 tenant_access_token",
		}
	}
	expire := int64(7200)
	if n, ok := toTicks(body["expire"]); ok && n != 0 {
		expire = n
	}
	safe := expire - tokenSafetySec
	if safe < 60 {
		safe = 60
	}
	c.token = token
	c.tokenExpireAt = c.clock().Add(time.Duration(safe) * time.Second)
	return token, nil
}

func (c *apiClient) invalidateToken() {
	c.tokenMu.Lock()
	defer c.tokenMu.Unlock()
	c.token = ""
	c.tokenExpireAt = time.Time{}
}

// -- 请求 -------------------------------------------------------------------

type rawResponse struct {
	status int
	body   []byte
}

// request 发一次请求，返回飞书响应体里的 data（binary=true 时返回原始字节）。
func (c *apiClient) request(ctx context.Context, req apiRequest) (map[string]any, []byte, error) {
	if req.rateLimited {
		if _, err := c.bucket.Acquire(ctx, 1); err != nil {
			return nil, nil, &aiteerr.PlatformError{
				Code: "transport_error", Retryable: true, Msg: err.Error(),
			}
		}
	}

	resp, err := c.rawRequest(ctx, req, true)
	if err != nil {
		return nil, nil, err
	}

	// token 过期：换一张再打一次，不算进三次退避里。
	if resp.status == http.StatusUnauthorized {
		c.invalidateToken()
		if resp, err = c.rawRequest(ctx, req, true); err != nil {
			return nil, nil, err
		}
	}

	if resp.status >= 400 {
		return nil, nil, errorFromResponse(resp, nil)
	}

	if req.binary {
		return nil, resp.body, nil
	}

	body := jsonBody(resp.body)
	if code := codeOf(body); code != "0" {
		// HTTP 2xx 但业务码非 0：参数/权限问题，重试没意义。
		return nil, nil, &aiteerr.PlatformError{
			Code:       code,
			HTTPStatus: resp.status,
			Retryable:  false,
			Msg:        mapStr(body, "msg"),
		}
	}
	return asMap(body["data"]), nil, nil
}

// rawRequest 是带退避重试的一次 HTTP 往返。4xx（非 429）直接返回给调用方判。
func (c *apiClient) rawRequest(ctx context.Context, req apiRequest, authed bool) (rawResponse, error) {
	attempts := len(c.retryDelays) + 1
	var lastErr error

	for attempt := 0; attempt < attempts; attempt++ {
		httpReq, err := c.buildRequest(ctx, req)
		if err != nil {
			return rawResponse{}, &aiteerr.PlatformError{
				Code: "bad_request", Retryable: false, Msg: err.Error(),
			}
		}
		if authed {
			token, err := c.tenantAccessToken(ctx)
			if err != nil {
				return rawResponse{}, err
			}
			httpReq.Header.Set("Authorization", "Bearer "+token)
		}

		resp, err := c.http.Do(httpReq)
		if err != nil {
			// 传输层错误按 5xx 一档处理：连不上和对面挂了，对调用方是一回事。
			// Python 版在传输层失败重试前不打 feishu.retry，只在 429/5xx 时打；照搬。
			lastErr = err
			if attempt < len(c.retryDelays) {
				if serr := c.sleep(ctx, c.retryDelays[attempt]); serr != nil {
					return rawResponse{}, transportError(serr)
				}
				continue
			}
			return rawResponse{}, transportError(err)
		}

		data, readErr := io.ReadAll(resp.Body)
		_ = resp.Body.Close()
		if readErr != nil {
			lastErr = readErr
			if attempt < len(c.retryDelays) {
				if serr := c.sleep(ctx, c.retryDelays[attempt]); serr != nil {
					return rawResponse{}, transportError(serr)
				}
				continue
			}
			return rawResponse{}, transportError(readErr)
		}
		out := rawResponse{status: resp.StatusCode, body: data}

		if out.status == http.StatusTooManyRequests || out.status >= 500 {
			if attempt < len(c.retryDelays) {
				c.logger.Warn("feishu.retry",
					"method", req.method, "path", req.path, "status", out.status, "attempt", attempt+1)
				if serr := c.sleep(ctx, c.retryDelays[attempt]); serr != nil {
					return rawResponse{}, transportError(serr)
				}
				continue
			}
			retryable := true
			return rawResponse{}, errorFromResponse(out, &retryable)
		}

		return out, nil
	}

	return rawResponse{}, &aiteerr.PlatformError{
		Code: "unreachable", Retryable: true, Msg: fmt.Sprint(lastErr),
	}
}

func (c *apiClient) buildRequest(ctx context.Context, req apiRequest) (*http.Request, error) {
	target := c.domain + req.path
	if len(req.params) > 0 {
		q := url.Values{}
		for k, v := range req.params {
			q.Set(k, v)
		}
		target += "?" + q.Encode()
	}

	var body io.Reader
	contentType := ""
	switch {
	case req.file != nil:
		var buf bytes.Buffer
		w := multipart.NewWriter(&buf)
		for k, v := range req.form {
			if err := w.WriteField(k, v); err != nil {
				return nil, err
			}
		}
		h := textproto.MIMEHeader{}
		h.Set("Content-Disposition", fmt.Sprintf(`form-data; name="%s"; filename="%s"`,
			escapeQuotes(req.file.field), escapeQuotes(req.file.filename)))
		if req.file.mime != "" {
			h.Set("Content-Type", req.file.mime)
		}
		part, err := w.CreatePart(h)
		if err != nil {
			return nil, err
		}
		if _, err := part.Write(req.file.data); err != nil {
			return nil, err
		}
		if err := w.Close(); err != nil {
			return nil, err
		}
		body = &buf
		contentType = w.FormDataContentType()
	case req.body != nil:
		encoded, err := json.Marshal(req.body)
		if err != nil {
			return nil, err
		}
		body = bytes.NewReader(encoded)
		contentType = "application/json; charset=utf-8"
	}

	httpReq, err := http.NewRequestWithContext(ctx, req.method, target, body)
	if err != nil {
		return nil, err
	}
	if contentType != "" {
		httpReq.Header.Set("Content-Type", contentType)
	}
	return httpReq, nil
}

// -- 错误 -------------------------------------------------------------------

// transportError 把传输层失败包成 PlatformError。
//
// 超时单独给 code="timeout"：aiteerr 的冻结映射把它翻成 DEADLINE_EXCEEDED，
// 其余传输错误翻成 UNAVAILABLE（两者在 core 侧都是 retryable=true）。
func transportError(err error) *aiteerr.PlatformError {
	code := "transport_error"
	if isTimeout(err) {
		code = "timeout"
	}
	return &aiteerr.PlatformError{Code: code, Retryable: true, Msg: err.Error()}
}

func isTimeout(err error) bool {
	type timeouter interface{ Timeout() bool }
	for e := err; e != nil; {
		if t, ok := e.(timeouter); ok && t.Timeout() {
			return true
		}
		u, ok := e.(interface{ Unwrap() error })
		if !ok {
			return false
		}
		e = u.Unwrap()
	}
	return false
}

func jsonBody(data []byte) map[string]any {
	var parsed any
	if err := json.Unmarshal(data, &parsed); err != nil {
		return map[string]any{}
	}
	if m, ok := parsed.(map[string]any); ok {
		return m
	}
	return map[string]any{}
}

// codeOf 把响应体里的业务码规范成十进制字符串；缺失按 0（成功）处理。
func codeOf(body map[string]any) string {
	raw, ok := body["code"]
	if !ok {
		return "0"
	}
	return codeString(raw)
}

func codeString(raw any) string {
	switch v := raw.(type) {
	case string:
		return v
	case float64:
		return strconv.FormatInt(int64(v), 10)
	case int:
		return strconv.Itoa(v)
	case int64:
		return strconv.FormatInt(v, 10)
	case nil:
		return "0"
	default:
		return fmt.Sprint(v)
	}
}

// errorFromResponse 把 HTTP 响应翻成 PlatformError；retryable 为 nil 时按状态码决定。
func errorFromResponse(resp rawResponse, retryable *bool) *aiteerr.PlatformError {
	body := jsonBody(resp.body)
	code := strconv.Itoa(resp.status)
	if _, ok := body["code"]; ok {
		code = codeOf(body)
	}
	message := mapStr(body, "msg")
	if message == "" {
		message = truncateRunes(string(resp.body), 200)
	}
	retry := resp.status == http.StatusTooManyRequests || resp.status >= 500
	if retryable != nil {
		retry = *retryable
	}
	return &aiteerr.PlatformError{
		Code:       code,
		HTTPStatus: resp.status,
		Retryable:  retry,
		Msg:        message,
	}
}

// escapeQuotes 与标准库 mime/multipart 的同名私有函数一致。
var quoteEscaper = strings.NewReplacer("\\", "\\\\", `"`, "\\\"")

func escapeQuotes(s string) string { return quoteEscaper.Replace(s) }

func truncateRunes(s string, n int) string {
	runes := []rune(s)
	if len(runes) <= n {
		return s
	}
	return string(runes[:n])
}
