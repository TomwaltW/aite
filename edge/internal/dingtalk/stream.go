// Stream 模式客户端（协议形状核对过官方 SDK open-dingtalk/dingtalk-stream-sdk-go 的源码）。
//
//	开连接：POST {api_base}/v1.0/gateway/connections/open → {endpoint, ticket}
//	建 ws ：拨 endpoint?ticket=<ticket>；ticket 一次性，每次重连整套重走
//	下行帧：{"specVersion","type","time","headers":{"messageId","topic",…},"data":"<JSON 字符串>"}
//	ACK   ：每一帧都回 {"code":200,"headers":{"contentType","messageId":<原样回显>},"message":"OK","data":"…"}
//
// 读循环只解码、分发，永不在本 goroutine 里调 sink：
//
//   - SYSTEM ping / EVENT / 卡片回调：读循环里当场 ACK（卡片回调的 sink 投递另起 goroutine）；
//   - 机器人消息：投给一个串行 dispatcher goroutine（保序），sink 返回后再经写锁 ACK。
//
// 这样慢 sink 拖不住后面的 ping / 卡片回调帧（卡片回调时限 2 秒）。
package dingtalk

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

// Stream 订阅与帧常量。
const (
	PathGatewayOpen = "/v1.0/gateway/connections/open"

	TopicBotMessage   = "/v1.0/im/bot/messages/get"
	TopicCardCallback = "/v1.0/card/instances/callback"

	frameTypeSystem   = "SYSTEM"
	frameTypeEvent    = "EVENT"
	frameTypeCallback = "CALLBACK"

	topicPing       = "ping"
	topicDisconnect = "disconnect"

	streamUA = "aite-edge/dingtalk"
)

// subscriptions 是 open 请求体里的三条订阅（逐字）。
func subscriptions() []map[string]string {
	return []map[string]string{
		{"type": frameTypeEvent, "topic": "*"},
		{"type": frameTypeCallback, "topic": TopicBotMessage},
		{"type": frameTypeCallback, "topic": TopicCardCallback},
	}
}

// streamFrame 是下行帧。data 是 JSON 字符串。
type streamFrame struct {
	SpecVersion string            `json:"specVersion"`
	Type        string            `json:"type"`
	Time        int64             `json:"time"`
	Headers     map[string]string `json:"headers"`
	Data        string            `json:"data"`
}

func (f *streamFrame) messageID() string { return f.Headers["messageId"] }
func (f *streamFrame) topic() string     { return f.Headers["topic"] }

// ackFrame 是上行 ACK。
type ackFrame struct {
	Code    int               `json:"code"`
	Headers map[string]string `json:"headers"`
	Message string            `json:"message"`
	Data    string            `json:"data"`
}

// ACK 的 data。EVENT 回 SUCCESS；回调类回一个空 response（推断，待 H9 核实）。
const (
	ackDataEventSuccess = `{"status":"SUCCESS","message":"success"}`
	ackDataCallbackOK   = `{"response":{}}`
)

// openConnection 调 open 拿 {endpoint, ticket}。错误文本里不出现 clientSecret / ticket。
func (p *Platform) openConnection(ctx context.Context) (string, string, error) {
	body, err := json.Marshal(map[string]any{
		"clientId":      p.opts.ClientID,
		"clientSecret":  p.opts.ClientSecret,
		"subscriptions": subscriptions(),
		"ua":            streamUA,
		"localIp":       "",
	})
	if err != nil {
		return "", "", err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, p.api.base+PathGatewayOpen, bytes.NewReader(body))
	if err != nil {
		return "", "", err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")
	resp, err := p.api.http.Do(req)
	if err != nil {
		return "", "", transportError(sanitizeURLError(err))
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", "", transportError(err)
	}
	if resp.StatusCode >= 400 {
		return "", "", errorFromResponse(resp.StatusCode, data)
	}
	parsed := jsonBody(data)
	endpoint, ticket := mapStr(parsed, "endpoint"), mapStr(parsed, "ticket")
	if endpoint == "" || ticket == "" {
		return "", "", errors.New("open 响应里缺 endpoint 或 ticket")
	}
	return endpoint, ticket, nil
}

// connect 一轮建连：先 open 拿新 ticket，再拨 endpoint?ticket=。
func (p *Platform) connect(ctx context.Context) (*streamConn, error) {
	endpoint, ticket, err := p.openConnection(ctx)
	if err != nil {
		return nil, fmt.Errorf("open: %w", err)
	}
	target := endpoint
	if strings.Contains(target, "?") {
		target += "&ticket=" + url.QueryEscape(ticket)
	} else {
		target += "?ticket=" + url.QueryEscape(ticket)
	}
	ws, resp, err := p.dialer.DialContext(ctx, target, nil)
	if resp != nil && resp.Body != nil {
		_ = resp.Body.Close()
	}
	if err != nil {
		// 只报状态码，不带 URL（query 里有 ticket）。
		if resp != nil {
			return nil, fmt.Errorf("dial: HTTP %d", resp.StatusCode)
		}
		return nil, fmt.Errorf("dial: %w", sanitizeURLError(err))
	}
	return newStreamConn(p, ws), nil
}

// streamConn 是一轮连接。gorilla 的 Conn 同一时刻只许一个写者：所有写都走 writeMu。
type streamConn struct {
	p  *Platform
	ws *websocket.Conn

	writeMu sync.Mutex

	queueMu sync.Mutex
	queue   []*streamFrame
	wake    chan struct{}

	done      chan struct{}
	closeOnce sync.Once
	wg        sync.WaitGroup
}

func newStreamConn(p *Platform, ws *websocket.Conn) *streamConn {
	return &streamConn{
		p:    p,
		ws:   ws,
		wake: make(chan struct{}, 1),
		done: make(chan struct{}),
	}
}

// Close 主动断开，幂等；等 dispatcher 退出。
func (c *streamConn) Close() {
	c.closeOnce.Do(func() {
		close(c.done)
		_ = c.ws.Close()
	})
	c.wg.Wait()
}

// run 跑读循环直到连接断开。disconnect 帧返回 nil；服务端断开返回读错误。
func (c *streamConn) run(ctx context.Context) error {
	c.wg.Add(2)
	go c.dispatchLoop(ctx)
	go func() {
		defer c.wg.Done()
		select {
		case <-ctx.Done():
			c.closeOnce.Do(func() {
				close(c.done)
				_ = c.ws.Close()
			})
		case <-c.done:
		}
	}()

	for {
		_, payload, err := c.ws.ReadMessage()
		if err != nil {
			return err
		}
		var frame streamFrame
		if err := json.Unmarshal(payload, &frame); err != nil {
			c.p.logger.Warn("dingtalk.bad_frame", "err", err)
			continue
		}
		if stop := c.route(ctx, &frame); stop {
			return nil
		}
	}
}

// route 分发一帧；返回 true 表示要关掉本连接（disconnect）。
func (c *streamConn) route(ctx context.Context, f *streamFrame) bool {
	switch f.Type {
	case frameTypeSystem:
		switch f.topic() {
		case topicPing:
			c.ack(f, 200, f.Data)
		case topicDisconnect:
			c.p.logger.Info("dingtalk.server_disconnect")
			c.ack(f, 200, f.Data)
			return true
		default:
			c.ack(f, 200, f.Data)
		}
	case frameTypeEvent:
		c.ack(f, 200, ackDataEventSuccess)
		c.p.logger.Debug("dingtalk.event_ignored", "topic", f.topic())
	case frameTypeCallback:
		switch f.topic() {
		case TopicBotMessage:
			c.enqueue(f)
		case TopicCardCallback:
			c.p.handleCardCallback(ctx, c, f)
		default:
			c.ack(f, 200, ackDataCallbackOK)
			c.p.logger.Debug("dingtalk.callback_ignored", "topic", f.topic())
		}
	default:
		c.ack(f, 200, "")
		c.p.logger.Debug("dingtalk.frame_ignored", "type", f.Type)
	}
	return false
}

// ack 回一帧 ACK，原样回显 headers.messageId。
func (c *streamConn) ack(f *streamFrame, code int, data string) {
	message := "OK"
	if code != 200 {
		message = "handler failed"
	}
	payload, err := json.Marshal(ackFrame{
		Code: code,
		Headers: map[string]string{
			"contentType": "application/json",
			"messageId":   f.messageID(),
		},
		Message: message,
		Data:    data,
	})
	if err != nil {
		return
	}
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	_ = c.ws.SetWriteDeadline(time.Now().Add(5 * time.Second))
	if err := c.ws.WriteMessage(websocket.TextMessage, payload); err != nil {
		c.p.logger.Warn("dingtalk.ack_failed", "message_id", f.messageID(), "err", err)
	}
}

// enqueue 把机器人消息帧交给串行 dispatcher（无界队列：读循环永不因慢 sink 阻塞）。
func (c *streamConn) enqueue(f *streamFrame) {
	c.queueMu.Lock()
	c.queue = append(c.queue, f)
	c.queueMu.Unlock()
	select {
	case c.wake <- struct{}{}:
	default:
	}
}

func (c *streamConn) dequeue() *streamFrame {
	c.queueMu.Lock()
	defer c.queueMu.Unlock()
	if len(c.queue) == 0 {
		return nil
	}
	f := c.queue[0]
	c.queue[0] = nil
	c.queue = c.queue[1:]
	return f
}

// dispatchLoop 串行处理机器人消息：归一化 → sink → ACK（sink 出错回非 200，让平台重推）。
//
// 连接关掉后剩下没处理的帧不再 ACK —— 平台没收到 ACK 会重推，core 靠 event_id 去重。
func (c *streamConn) dispatchLoop(ctx context.Context) {
	defer c.wg.Done()
	for {
		select {
		case <-c.done:
			return
		default:
		}
		f := c.dequeue()
		if f == nil {
			select {
			case <-c.done:
				return
			case <-c.wake:
			}
			continue
		}
		if err := c.p.handleBotMessage(ctx, f.Data); err != nil {
			c.ack(f, 500, "")
			continue
		}
		c.ack(f, 200, ackDataCallbackOK)
	}
}
