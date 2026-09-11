// Package feishu 是 PlatformPort 的飞书实现（对应旧 aite/adapters/feishu/**）。
//
// 本文件对应 aite/adapters/feishu/platform.py。
//
// 长连接收事件 → 归一化成 pb.NormalizedEvent → 出站（文本 / 卡片 / 文件 / 表情）
// → 群历史与云文档读取。
//
// 三条不许走偏的：
//
//   - UpdateCard 一定是 PATCH 同一条 message_id，绝不新发消息。
//   - adapter 不做去重 —— 去重是 core 靠 event_id 干的。
//     重连后平台重推的重复事件在这里照单全收往上送。
//   - ReadHistory 按时间正序返回，不做 sender_kind 过滤 —— 过滤归 core 的
//     read_group_history 工具。
//
// 构造函数签名不许改（cmd/aite-edge 归 R2，两边并行时靠这个签名对接）。
package feishu

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
	"aite/edge/internal/config"
)

// onEventBudget：HandleEvent 必须在 1s 内返回。超了不拦（拦了会丢事件），但要吼一声。
const onEventBudget = time.Second

// KnownEmojiTypes 是「表情文案说明」里确实存在的 emoji_type，只列本包可能用到的几个。
// 测试拿它兜住「别再往 ReactionEmoji 里写一个清单外的 key」。
var KnownEmojiTypes = map[string]bool{
	"OnIt": true, "DONE": true, "CRY": true,
	"GLANCE": true, "THUMBSUP": true, "MUSCLE": true, "OK": true,
}

// ReactionEmoji 是 ReactionKind → 飞书表情 key（「添加消息表情回复」的 reaction_type.emoji_type）。
//
// 取值必须落在官方那份固定清单里，不在清单里的会被打回 231001 表情类型不合法。
// T16 拿清单逐个核过：DONE / CRY 在清单里；早期写的 EYES 不在 —— 有 eyes 的是云文档
// 高亮块那套小写枚举，跟消息表情回复不是一套，照原样发上去每一次 ack 都会 400。
// 换成清单里的 OnIt，语义正好是「收到，正在处理」。
var ReactionEmoji = map[pb.ReactionKind]string{
	pb.ReactionKind_REACTION_KIND_ACK:  "OnIt",
	pb.ReactionKind_REACTION_KIND_DONE: "DONE",
	pb.ReactionKind_REACTION_KIND_FAIL: "CRY",
}

// fileTypeByExt 是「上传文件」接口的 file_type 取值，按扩展名挑；认不出就 stream。
var fileTypeByExt = map[string]string{
	".opus": "opus", ".mp4": "mp4", ".pdf": "pdf",
	".doc": "doc", ".docx": "doc",
	".xls": "xls", ".xlsx": "xls",
	".ppt": "ppt", ".pptx": "ppt",
}

// historyPageSize 是一页历史消息的上限（飞书 page_size 上限 50）。
const historyPageSize = 50

// docURLRe 是云文档链接里 token 的位置：/docx/<token>、/docs/<token>、/wiki/<token>。
var docURLRe = regexp.MustCompile(`/(docx|docs|wiki)/([A-Za-z0-9]+)`)

// EventSink 是 edge 把归一化事件送进 core 的出口（internal/ingress 实现）。
type EventSink interface {
	HandleEvent(ctx context.Context, ev *pb.NormalizedEvent) error
}

// Options 是构造时从环境变量解析好的凭证（config 里只有变量名）。
type Options struct {
	AppID     string
	AppSecret string
	BotOpenID string
	TenantID  string
	Domain    string // 默认 https://open.feishu.cn
}

// Platform 实现 server.PlatformPort。
type Platform struct {
	cfg  config.Feishu
	opts Options
	sink EventSink

	api     *apiClient
	caps    *pb.PlatformCapabilities
	factory ConnectionFactory
	sleep   sleeperFunc
	clock   clockFunc
	logger  *slog.Logger
	budget  time.Duration

	// historyWindow 是 config 里的 feishu.history_window，只存不用：
	// ReadHistory 的条数由调用方按 ports.go 的签名给（Python 版同样只存着）。
	historyWindow int

	connected      atomic.Bool
	reconnectCount atomic.Int64
}

// platformOptions 是 Platform 的全部可注入面；New 用默认值填满它。
type platformOptions struct {
	cfg     config.Feishu
	opts    Options
	sink    EventSink
	api     *apiClient
	caps    *pb.PlatformCapabilities
	factory ConnectionFactory
	sleep   sleeperFunc
	clock   clockFunc
	logger  *slog.Logger
	budget  time.Duration
}

func newPlatform(po platformOptions) (*Platform, error) {
	if po.opts.Domain == "" {
		po.opts.Domain = DefaultDomain
	}
	if po.opts.TenantID == "" {
		po.opts.TenantID = "default"
	}
	if po.clock == nil {
		po.clock = time.Now
	}
	if po.sleep == nil {
		po.sleep = realSleep
	}
	if po.logger == nil {
		po.logger = slog.Default()
	}
	if po.budget <= 0 {
		po.budget = onEventBudget
	}
	// FeishuP0() 每次返回新对象；注入的也拷一份，别把调用方的对象攥在手里。
	caps := po.caps
	if caps == nil {
		caps = FeishuP0()
	} else {
		caps = proto.Clone(caps).(*pb.PlatformCapabilities)
	}

	historyWindow := po.cfg.HistoryWindow
	if historyWindow <= 0 {
		historyWindow = 50
	}

	p := &Platform{
		cfg:           po.cfg,
		opts:          po.opts,
		sink:          po.sink,
		caps:          caps,
		sleep:         po.sleep,
		clock:         po.clock,
		logger:        po.logger,
		budget:        po.budget,
		historyWindow: historyWindow,
	}

	p.api = po.api
	if p.api == nil {
		api, err := newAPIClient(apiOptions{
			appID:      po.opts.AppID,
			appSecret:  po.opts.AppSecret,
			domain:     po.opts.Domain,
			ratePerMin: int(caps.GetOutboundRatePerMin()),
			sleep:      po.sleep,
			clock:      po.clock,
			logger:     po.logger,
		})
		if err != nil {
			return nil, err
		}
		p.api = api
	}

	p.factory = po.factory
	if p.factory == nil {
		p.factory = func(onRaw RawEventHandler) Connection {
			return newLarkConnection(po.opts.AppID, po.opts.AppSecret, po.opts.Domain, onRaw, po.logger)
		}
	}
	return p, nil
}

// New 只做装配，不建连接；Start 才起长连接。
func New(cfg config.Feishu, opts Options, sink EventSink) (*Platform, error) {
	return newPlatform(platformOptions{cfg: cfg, opts: opts, sink: sink})
}

// FeishuP0 是契约里的 FEISHU_P0 常量；每次调用返回新对象，
// 调用方可安全改 supports_passive_listen。
func FeishuP0() *pb.PlatformCapabilities {
	return &pb.PlatformCapabilities{
		Platform:                      "feishu",
		SupportsThread:                true,
		SupportsHistory:               true,
		SupportsPassiveListen:         false,
		SupportsCardEdit:              true,
		CardEditWindowSec:             1209600,
		InboundFileInGroup:            true,
		ProactiveRequiresPriorMessage: false,
		OutboundRatePerMin:            60,
	}
}

// Capabilities 返回本实例的能力副本。
func (p *Platform) Capabilities() *pb.PlatformCapabilities { return p.caps }

// SetPassiveListen 在权限核实后调这个，改的是本实例而不是 FeishuP0() 的返回值。
func (p *Platform) SetPassiveListen(value bool) {
	p.caps.SupportsPassiveListen = value
}

// ------------------------------------------------------------------
// 长连接
// ------------------------------------------------------------------

// Start 建连并持续投递事件；断线按 1,2,4,8,16,30,30… 秒退避无限重连。
// 只有 ctx 取消能让它返回。
func (p *Platform) Start(ctx context.Context) error {
	attempt := 0

	for {
		if ctx.Err() != nil {
			return nil
		}
		if attempt > 0 {
			delay := backoffDelay(attempt)
			p.logger.Warn("feishu.reconnecting", "attempt", attempt, "delay_sec", delay.Seconds())
			if err := p.sleep(ctx, delay); err != nil {
				return nil
			}
			if ctx.Err() != nil {
				return nil
			}
		}

		conn := p.factory(p.dispatchRaw)
		if err := conn.Connect(ctx); err != nil {
			if ctx.Err() != nil {
				return nil
			}
			attempt++
			p.logger.Warn("feishu.connect_failed", "attempt", attempt, "err", err)
			continue
		}

		if attempt > 0 {
			p.logger.Info("feishu.reconnected", "after", attempt)
			p.reconnectCount.Add(1)
		}
		attempt = 0
		p.connected.Store(true)

		err := conn.WaitClosed(ctx)
		p.connected.Store(false)
		if err != nil && ctx.Err() == nil {
			p.logger.Warn("feishu.connection_lost", "err", err)
		}
		p.safeClose(conn)

		if ctx.Err() != nil {
			return nil
		}
		// 断开了：下一轮从 1s 起退避重连。
		attempt = 1
	}
}

func (p *Platform) safeClose(c Connection) {
	if c == nil {
		return
	}
	if err := c.Close(); err != nil {
		p.logger.Warn("feishu.close_failed", "err", err)
	}
}

// Connected 报告长连接当前是否在线（EdgeStatus 用）。
func (p *Platform) Connected() bool { return p.connected.Load() }

// ReconnectCount 报告累计重连成功次数（EdgeStatus 用）。
func (p *Platform) ReconnectCount() int64 { return p.reconnectCount.Load() }

// dispatchRaw 把原始事件归一化后同步交给 sink。
//
// adapter 不去重：重连后平台重推的同一条也照样往上送，由 core 用 event_id 判。
//
// 与 Python 版的一处有意差异：Python 把 handler 的异常吞掉（只打日志），
// Go 版把 error 一路返回给 SDK，让平台重推（spec §2.1「失败 → 向平台返回错误让其重推」）。
func (p *Platform) dispatchRaw(ctx context.Context, raw map[string]any) error {
	event := Normalize(raw, p.opts.BotOpenID, p.opts.AppID, p.opts.TenantID)
	if event == nil {
		p.logger.Debug("feishu.event_ignored", "type", mapStr(asMap(raw["header"]), "event_type"))
		return nil
	}

	started := p.clock()
	err := p.sink.HandleEvent(ctx, event)
	elapsed := p.clock().Sub(started)
	if elapsed > p.budget {
		p.logger.Warn("feishu.on_event_slow",
			"event_id", event.GetEventId(),
			"elapsed_sec", elapsed.Seconds(),
			"budget_sec", p.budget.Seconds())
	}
	if err != nil {
		p.logger.Error("feishu.on_event_failed", "event_id", event.GetEventId(), "err", err)
		return err
	}
	return nil
}

// ------------------------------------------------------------------
// 出站
// ------------------------------------------------------------------

// sendMessage 发一条消息，返回 message_id。
//
// 有 replyTo 就走「回复消息」接口（带 reply_in_thread 才进话题），否则走「发送消息」。
func (p *Platform) sendMessage(ctx context.Context, chatID string, replyTo *string, msgType, content string, inThread bool) (string, error) {
	var (
		data map[string]any
		err  error
	)
	if replyTo != nil && *replyTo != "" {
		data, _, err = p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   fmt.Sprintf(PathMessageReply, *replyTo),
			body: map[string]any{
				"content": content, "msg_type": msgType, "reply_in_thread": inThread,
			},
			rateLimited: true,
		})
	} else {
		data, _, err = p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   PathMessages,
			params: map[string]string{"receive_id_type": "chat_id"},
			body: map[string]any{
				"receive_id": chatID, "msg_type": msgType, "content": content,
			},
			rateLimited: true,
		})
	}
	if err != nil {
		return "", err
	}
	return mapStr(data, "message_id"), nil
}

func (p *Platform) SendText(ctx context.Context, msg *pb.OutboundText) (*pb.SendResult, error) {
	content := DumpsCard(BuildMarkdownCard(msg.GetText()))
	messageID, err := p.sendMessage(ctx, msg.GetChatId(), msg.ReplyTo, "interactive", content, msg.GetInThread())
	if err != nil {
		return nil, err
	}
	return &pb.SendResult{MessageId: messageID}, nil
}

func (p *Platform) SendCard(ctx context.Context, chatID string, replyTo *string, card *pb.ChecklistCard) (*pb.SendResult, error) {
	content := DumpsCard(BuildChecklistCard(card))
	// send_card 的 in_thread 写死 true，只有 SendText 透传。
	messageID, err := p.sendMessage(ctx, chatID, replyTo, "interactive", content, true)
	if err != nil {
		return nil, err
	}
	cardID := messageID
	return &pb.SendResult{MessageId: messageID, CardId: &cardID}, nil
}

// UpdateCard 原地更新同一条卡片消息。
//
// 必须是 PATCH /open-apis/im/v1/messages/{card_id}（「更新应用发送的消息卡片」）。
// 任何时候都不许退化成再发一条 —— 「过程中卡片至少更新 3 次且不新增消息」就靠这条。
func (p *Platform) UpdateCard(ctx context.Context, cardID string, card *pb.ChecklistCard) error {
	_, _, err := p.api.request(ctx, apiRequest{
		method:      http.MethodPatch,
		path:        fmt.Sprintf(PathMessage, cardID),
		body:        map[string]any{"content": DumpsCard(BuildChecklistCard(card))},
		rateLimited: true,
	})
	return err
}

// SendFile 先上传再发送。图片走 images 接口，其余走 files 接口。上传不过令牌桶。
func (p *Platform) SendFile(ctx context.Context, msg *pb.OutboundFile) (*pb.SendResult, error) {
	var (
		content string
		msgType string
	)
	if strings.HasPrefix(msg.GetMime(), "image/") {
		data, _, err := p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   PathImages,
			file:   &uploadFile{field: "image", filename: msg.GetName(), mime: msg.GetMime(), data: msg.GetData()},
			form:   map[string]string{"image_type": "message"},
		})
		if err != nil {
			return nil, err
		}
		content = DumpsCard(map[string]any{"image_key": data["image_key"]})
		msgType = "image"
	} else {
		data, _, err := p.api.request(ctx, apiRequest{
			method: http.MethodPost,
			path:   PathFiles,
			file:   &uploadFile{field: "file", filename: msg.GetName(), mime: msg.GetMime(), data: msg.GetData()},
			form:   map[string]string{"file_type": fileTypeOf(msg.GetName()), "file_name": msg.GetName()},
		})
		if err != nil {
			return nil, err
		}
		content = DumpsCard(map[string]any{"file_key": data["file_key"]})
		msgType = "file"
	}

	messageID, err := p.sendMessage(ctx, msg.GetChatId(), msg.ReplyTo, msgType, content, true)
	if err != nil {
		return nil, err
	}
	return &pb.SendResult{MessageId: messageID}, nil
}

func (p *Platform) AddReaction(ctx context.Context, messageID string, kind pb.ReactionKind) error {
	emoji, ok := ReactionEmoji[kind]
	if !ok {
		return &aiteerr.PlatformError{
			Code: "bad_reaction", Retryable: false, Msg: fmt.Sprintf("未知表情类型 %s", kind),
		}
	}
	_, _, err := p.api.request(ctx, apiRequest{
		method:      http.MethodPost,
		path:        fmt.Sprintf(PathMessageReaction, messageID),
		body:        map[string]any{"reaction_type": map[string]any{"emoji_type": emoji}},
		rateLimited: true,
	})
	return err
}

// ------------------------------------------------------------------
// 读取
// ------------------------------------------------------------------

// ReadHistory 返回群历史，按时间正序，最近的 limit 条。
//
// 拉取用 ByCreateTimeDesc（最新的在前）再翻转：要的是「最近 N 条」，
// 用正序翻页只会从群成立那天开始拿，拿到的是最老的 N 条。
//
// threadID 给了就只留这条话题里的消息。飞书的 container_id_type=thread 收的是
// omt_ 开头的话题 id，而锚点里存的是话题 root 消息 id，两者不是一个 id 空间，
// 所以这里在客户端筛，root_id / parent_id / thread_id / message_id 命中任一即算。
//
// 不做 sender_kind 过滤 —— 那是 core 的 read_group_history 工具的活。
func (p *Platform) ReadHistory(ctx context.Context, chatID string, limit int, threadID *string) ([]*pb.HistoryMessage, error) {
	wanted := limit
	if wanted < 0 {
		wanted = 0
	}
	if wanted == 0 {
		return []*pb.HistoryMessage{}, nil
	}
	// 要按话题筛就得多捞几页，否则一页里可能一条都不属于这个话题。
	budget := wanted
	if threadID != nil && *threadID != "" {
		budget = wanted * 4
	}

	var collected []map[string]any
	pageToken := ""
	for len(collected) < budget {
		params := map[string]string{
			"container_id_type": "chat",
			"container_id":      chatID,
			"sort_type":         "ByCreateTimeDesc",
			"page_size":         strconv.Itoa(min(historyPageSize, budget-len(collected))),
			// with_sender_name 传字符串 "true"（文档没有、SDK 有）。
			"with_sender_name": "true",
		}
		if pageToken != "" {
			params["page_token"] = pageToken
		}
		data, _, err := p.api.request(ctx, apiRequest{
			method: http.MethodGet, path: PathMessages, params: params,
		})
		if err != nil {
			return nil, err
		}
		items := asList(data["items"])
		for _, item := range items {
			if m, ok := item.(map[string]any); ok {
				collected = append(collected, m)
			}
		}
		pageToken = ""
		if hasMore, _ := data["has_more"].(bool); hasMore {
			pageToken = mapStr(data, "page_token")
		}
		if pageToken == "" || len(items) == 0 {
			break
		}
	}

	if threadID != nil && *threadID != "" {
		filtered := collected[:0:0]
		for _, item := range collected {
			if inThread(item, *threadID) {
				filtered = append(filtered, item)
			}
		}
		collected = filtered
	}

	// collected 是倒序的；取最近 wanted 条后翻回正序。
	if len(collected) > wanted {
		collected = collected[:wanted]
	}
	out := make([]*pb.HistoryMessage, 0, len(collected))
	for i := len(collected) - 1; i >= 0; i-- {
		out = append(out, toHistoryMessage(collected[i]))
	}
	return out, nil
}

// ReadDocument 读一篇云文档，返回正文文本。
//
// wiki 链接先换成它挂的 docx token 再读。返回的是「获取文档纯文本内容」接口的产物 ——
// 是纯文本而不是带格式的 markdown。
func (p *Platform) ReadDocument(ctx context.Context, urlOrToken string) (*pb.DocumentContent, error) {
	kind, token := parseDocRef(urlOrToken)
	if kind == "wiki" {
		node, _, err := p.api.request(ctx, apiRequest{
			method: http.MethodGet, path: PathWikiNode,
			params: map[string]string{"token": token, "obj_type": "wiki"},
		})
		if err != nil {
			return nil, err
		}
		if objToken := mapStr(asMap(node["node"]), "obj_token"); objToken != "" {
			token = objToken
		}
	}

	meta, _, err := p.api.request(ctx, apiRequest{
		method: http.MethodGet, path: fmt.Sprintf(PathDocxDocument, token),
	})
	if err != nil {
		return nil, err
	}
	raw, _, err := p.api.request(ctx, apiRequest{
		method: http.MethodGet, path: fmt.Sprintf(PathDocxRawContent, token),
		params: map[string]string{"lang": "0"},
	})
	if err != nil {
		return nil, err
	}

	docURL := urlOrToken
	if !strings.HasPrefix(urlOrToken, "http") {
		docURL = "/docx/" + token
	}
	return &pb.DocumentContent{
		Title: mapStr(asMap(meta["document"]), "title"),
		Text:  mapStr(raw, "content"),
		Url:   docURL,
	}, nil
}

// DownloadFile 下载消息里的资源文件。
//
// type 只有 image / file 两种取值，按 key 前缀判：飞书的图片 key 是
// img_v2_… / img_v3_…，文件 key 是 file_v2_…。
func (p *Platform) DownloadFile(ctx context.Context, messageID, fileKey string) ([]byte, error) {
	resourceType := "file"
	if strings.HasPrefix(fileKey, "img_") {
		resourceType = "image"
	}
	_, body, err := p.api.request(ctx, apiRequest{
		method: http.MethodGet,
		path:   fmt.Sprintf(PathMessageResource, messageID, fileKey),
		params: map[string]string{"type": resourceType},
		binary: true,
	})
	if err != nil {
		return nil, err
	}
	return body, nil
}

// ---------------------------------------------------------------------------
// 私有小工具
// ---------------------------------------------------------------------------

// parseDocRef 把云文档链接或裸 token 解析成 (类型, token)。
//
// 认 /docx/<token>、/docs/<token>、/wiki/<token> 三种链接；
// 传进来的要是裸 token（没有 /），按 docx 处理。
func parseDocRef(urlOrToken string) (string, string) {
	if m := docURLRe.FindStringSubmatch(urlOrToken); m != nil {
		return m[1], m[2]
	}
	token := strings.TrimSpace(urlOrToken)
	token, _, _ = strings.Cut(token, "?")
	token = strings.TrimRight(token, "/")
	if i := strings.LastIndex(token, "/"); i >= 0 {
		token = token[i+1:]
	}
	return "docx", token
}

func fileTypeOf(name string) string {
	i := strings.LastIndex(name, ".")
	if i < 0 || i == len(name)-1 {
		return "stream"
	}
	if t, ok := fileTypeByExt[strings.ToLower(name[i:])]; ok {
		return t
	}
	return "stream"
}

func inThread(item map[string]any, threadID string) bool {
	for _, key := range []string{"root_id", "parent_id", "thread_id", "message_id"} {
		if mapStr(item, key) == threadID {
			return true
		}
	}
	return false
}

func toHistoryMessage(item map[string]any) *pb.HistoryMessage {
	sender := asMap(item["sender"])
	body := asMap(item["body"])
	// 历史接口把正文放在 body.content，事件里放在 message.content —— 抹平成一个形状再复用解析。
	message := map[string]any{"message_type": item["msg_type"], "content": body["content"]}
	// botOpenID 传 ""：历史里别人 @ 机器人的那句话，去掉 @ 反而看不懂上下文。
	text, _, _ := extractText(message, mentionsOf(item), "")

	createdAt, ok := toTime(item["create_time"])
	if !ok {
		createdAt = timeNow()
	}

	msg := &pb.HistoryMessage{
		MessageId: mapStr(item, "message_id"),
		// 历史 API 形状：sender.id 而不是 sender.sender_id.open_id。
		SenderId:   mapStr(sender, "id"),
		SenderKind: senderKindToken(senderKindOf(sender["sender_type"])),
		Text:       text,
		CreatedAt:  timestamppb.New(createdAt),
	}
	if name := mapStr(sender, "sender_name"); name != "" {
		msg.SenderName = &name
	}
	// HistoryMessage.thread_id 只看 root_id。
	if rootID := mapStr(item, "root_id"); rootID != "" {
		msg.ThreadId = &rootID
	}
	return msg
}

// senderKindToken 把枚举转回旧契约的字符串形态（HistoryMessage.sender_kind）。
func senderKindToken(kind pb.SenderKind) string {
	switch kind {
	case pb.SenderKind_SENDER_KIND_HUMAN:
		return "human"
	case pb.SenderKind_SENDER_KIND_BOT:
		return "bot"
	case pb.SenderKind_SENDER_KIND_SYSTEM:
		return "system"
	default:
		return "app"
	}
}
