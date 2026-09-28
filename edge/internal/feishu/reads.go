package feishu

import (
	"container/list"
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"google.golang.org/protobuf/types/known/timestamppb"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// historyPageSize 是一页历史消息的上限（飞书 page_size 上限 50）。
const historyPageSize = 50

// docURLRe 是云文档链接里 token 的位置：/docx/<token>、/docs/<token>、/wiki/<token>。
var docURLRe = regexp.MustCompile(`/(docx|docs|wiki)/([A-Za-z0-9]+)`)

// ------------------------------------------------------------------
// 读取
// ------------------------------------------------------------------

// ReadHistory 返回群历史，按时间正序，最近的 limit 条。
//
// 拉取用 ByCreateTimeDesc（最新的在前）再翻转：要的是「最近 N 条」，
// 用正序翻页只会从群成立那天开始拿，拿到的是最老的 N 条。
//
// threadID 给了就只要这条话题里的消息。锚点里存的是话题 root 消息 id，而飞书的
// container_id_type=thread 收的是 omt_ 开头的话题 id，两者不是一个 id 空间 ——
// 所以先 GET root 消息取它的 thread_id，再按 thread 容器拉（见 readThreadHistory）。
// 取不到 thread_id（非话题消息、查询失败）或 thread 列表回权限错误时，回落到老办法：
// 整群拉取、在客户端筛，root_id / parent_id / thread_id / message_id 命中任一即算。
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
	if threadID != nil && *threadID != "" {
		out, handled, err := p.readThreadHistory(ctx, *threadID, wanted)
		if handled {
			return out, err
		}
	}

	// 要按话题筛就得多捞几页，否则一页里可能一条都不属于这个话题。
	budget := wanted
	if threadID != nil && *threadID != "" {
		budget = wanted * 4
	}

	collected, err := p.listMessages(ctx, "chat", chatID, budget)
	if err != nil {
		return nil, err
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

	return recentOldestFirst(collected, wanted), nil
}

// listMessages 按 ByCreateTimeDesc 分页拉一个容器（chat / thread）里的消息，最多 budget 条。
func (p *Platform) listMessages(ctx context.Context, containerType, containerID string, budget int) ([]map[string]any, error) {
	var collected []map[string]any
	pageToken := ""
	for len(collected) < budget {
		params := map[string]string{
			"container_id_type": containerType,
			"container_id":      containerID,
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
	return collected, nil
}

// recentOldestFirst 取倒序列表里最近的 wanted 条，翻回正序。
func recentOldestFirst(collected []map[string]any, wanted int) []*pb.HistoryMessage {
	if len(collected) > wanted {
		collected = collected[:wanted]
	}
	out := make([]*pb.HistoryMessage, 0, len(collected))
	for i := len(collected) - 1; i >= 0; i-- {
		out = append(out, toHistoryMessage(collected[i]))
	}
	return out
}

// readThreadHistory 按 thread 容器拉一条话题的历史。handled=false 表示该回落到整群筛法。
//
// 返回口径与整群筛法一致：正序、最近 wanted 条、含 root（thread 列表里没有 root 就用
// 第一步取到的补上、按时间归位）。预算不再 ×4 —— thread 容器里每一条都属于这个话题。
func (p *Platform) readThreadHistory(ctx context.Context, rootID string, wanted int) ([]*pb.HistoryMessage, bool, error) {
	root, err := p.getMessage(ctx, rootID)
	if err != nil {
		// 生产里 root 被撤回就是这种情形：回落，不上抛。
		p.logger.Debug("feishu.thread_history_fallback", "reason", "root_lookup_failed", "root_id", rootID, "err", err)
		return nil, false, nil
	}
	threadContainer := mapStr(root, "thread_id")
	if threadContainer == "" {
		p.logger.Debug("feishu.thread_history_fallback", "reason", "root_has_no_thread_id", "root_id", rootID)
		return nil, false, nil
	}

	collected, err := p.listMessages(ctx, "thread", threadContainer, wanted)
	if err != nil {
		if isPermissionError(err) {
			p.logger.Debug("feishu.thread_history_fallback", "reason", "thread_permission_denied", "root_id", rootID, "err", err)
			return nil, false, nil
		}
		return nil, true, err
	}
	return recentOldestFirst(withRoot(collected, root), wanted), true, nil
}

// getMessage 取一条消息（「获取指定消息的内容」，GET PathMessage）。
// 响应是 data.items[] 数组（SDK GetMessageRespData，service/im/v1/model.go:14134），取第一项。
func (p *Platform) getMessage(ctx context.Context, messageID string) (map[string]any, error) {
	data, _, err := p.api.request(ctx, apiRequest{
		method: http.MethodGet, path: fmt.Sprintf(PathMessage, messageID),
	})
	if err != nil {
		return nil, err
	}
	for _, item := range asList(data["items"]) {
		if m, ok := item.(map[string]any); ok {
			return m, nil
		}
	}
	return map[string]any{}, nil
}

// withRoot 把 root 按创建时间插进倒序列表（已在列表里就不动）。
func withRoot(collected []map[string]any, root map[string]any) []map[string]any {
	rootID := mapStr(root, "message_id")
	for _, item := range collected {
		if mapStr(item, "message_id") == rootID {
			return collected
		}
	}
	rootTicks, _ := toTicks(root["create_time"])
	out := make([]map[string]any, 0, len(collected)+1)
	inserted := false
	for _, item := range collected {
		if ticks, ok := toTicks(item["create_time"]); !inserted && ok && ticks < rootTicks {
			out = append(out, root)
			inserted = true
		}
		out = append(out, item)
	}
	if !inserted {
		out = append(out, root)
	}
	return out
}

// 权限错误的判据（thread 列表回它就回落整群筛法；通讯录回它就熔断）。
//
// 码值不是猜的，离线出处是 lark-oapi-go v3.12.0 的 channel/types/errors.go：
// :79-80 把业务码 99991400 / 99991401 / 230002 归为 ErrCodePermissionDenied，
// :96-97 把 HTTP 401 / 403 归为同一类。HTTP 401 在 apiClient.request 里已被「换 token 再打一次」
// 吃掉，这里只认 403。
//
// 疑点（待 H7 真机补）：99991400 在飞书通用错误码里可能是限流而不是权限；
// SDK 这张分类表是否可靠没有核实过。
const permissionDeniedHTTPStatus = http.StatusForbidden

var permissionDeniedCodes = map[string]bool{
	"99991400": true,
	"99991401": true,
	"230002":   true,
}

func isPermissionError(err error) bool {
	var pe *aiteerr.PlatformError
	if !errors.As(err, &pe) {
		return false
	}
	return pe.HTTPStatus == permissionDeniedHTTPStatus || permissionDeniedCodes[pe.Code]
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

// ------------------------------------------------------------------
// 发言人姓名（通讯录）
// ------------------------------------------------------------------

const (
	// senderNameCacheSize 是姓名缓存的条数上限。一个群里常说话的人是几十到几百，
	// 一个 edge 进程接的群有限；1024 条按每条百来字节算不到 1MB，又足够让热点发言人常驻。
	// 名字几乎不变，不设 TTL：改名要等被挤出或进程重启才生效（可接受，只是显示用）。
	senderNameCacheSize = 1024
	// senderNameTimeout 是每次查询的上限。事件链路要 1s 内交给 core（onEventBudget），
	// 查名字是锦上添花，超时就不要了；这个 ctx 也截断 apiClient 的退避重试。
	senderNameTimeout = 300 * time.Millisecond
	// senderNameTripFor 是权限错误后的熔断时长：没开 contact:user.base:readonly 的应用
	// 每条事件都会撞同一个错，熔断期内不再请求，每次熔断只打一条 WARN。
	senderNameTripFor = 10 * time.Minute
)

// senderNameLookup 按 open_id 查发言人姓名（GET /open-apis/contact/v3/users/{open_id}），
// 带有界 LRU 与权限熔断。只在 New() 里装配（newPlatform 收到 senderNames=false 就不查），
// Normalize 保持纯函数，老黄金文件不变。
type senderNameLookup struct {
	api    *apiClient
	clock  clockFunc
	logger *slog.Logger

	mu           sync.Mutex
	order        *list.List // 最近用过的在前；元素是 *senderNameEntry
	index        map[string]*list.Element
	trippedUntil time.Time
}

type senderNameEntry struct {
	openID string
	name   string
}

func newSenderNameLookup(api *apiClient, clock clockFunc, logger *slog.Logger) *senderNameLookup {
	return &senderNameLookup{
		api: api, clock: clock, logger: logger,
		order: list.New(), index: map[string]*list.Element{},
	}
}

// lookup 返回 open_id 的姓名；查不到（超时 / 出错 / 熔断中 / 响应里没有）返回 ok=false。
func (s *senderNameLookup) lookup(ctx context.Context, openID string) (string, bool) {
	s.mu.Lock()
	if el, ok := s.index[openID]; ok {
		s.order.MoveToFront(el)
		name := el.Value.(*senderNameEntry).name
		s.mu.Unlock()
		return name, true
	}
	if s.clock().Before(s.trippedUntil) {
		s.mu.Unlock()
		return "", false
	}
	s.mu.Unlock()

	ctx, cancel := context.WithTimeout(ctx, senderNameTimeout)
	defer cancel()
	// 字段形状照 SDK：GetUserRespData.user（contact/v3 model.go:12819）→ User.name（:4441）。
	data, _, err := s.api.request(ctx, apiRequest{
		method: http.MethodGet,
		path:   fmt.Sprintf(PathContactUser, openID),
		params: map[string]string{"user_id_type": "open_id"},
	})
	if err != nil {
		if isPermissionError(err) {
			s.mu.Lock()
			s.trippedUntil = s.clock().Add(senderNameTripFor)
			s.mu.Unlock()
			s.logger.Warn("feishu.sender_name_denied",
				"note", "通讯录没权限（要 contact:user.base:readonly），熔断后不再查",
				"trip_sec", senderNameTripFor.Seconds(), "err", err)
			return "", false
		}
		s.logger.Debug("feishu.sender_name_failed", "open_id", openID, "err", err)
		return "", false
	}
	name := mapStr(asMap(data["user"]), "name")
	if name == "" {
		s.logger.Debug("feishu.sender_name_failed", "open_id", openID, "err", "响应里没有 user.name")
		return "", false
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	if el, ok := s.index[openID]; ok { // 并发查同一个人，后到的覆盖
		el.Value.(*senderNameEntry).name = name
		s.order.MoveToFront(el)
		return name, true
	}
	s.index[openID] = s.order.PushFront(&senderNameEntry{openID: openID, name: name})
	for s.order.Len() > senderNameCacheSize {
		oldest := s.order.Back()
		s.order.Remove(oldest)
		delete(s.index, oldest.Value.(*senderNameEntry).openID)
	}
	return name, true
}

// fillSenderName 在事件交给 sink 之前补 sender_name：仅 MESSAGE / CARD_ACTION、
// sender_kind=HUMAN、sender_id 非空且 sender_name 为空时查。查不到就留 nil（core 退回 open_id）。
func (p *Platform) fillSenderName(ctx context.Context, event *pb.NormalizedEvent) {
	if p.senderNames == nil || event.SenderName != nil || event.GetSenderId() == "" ||
		event.GetSenderKind() != pb.SenderKind_SENDER_KIND_HUMAN {
		return
	}
	switch event.GetKind() {
	case pb.EventKind_EVENT_KIND_MESSAGE, pb.EventKind_EVENT_KIND_CARD_ACTION:
	default:
		return
	}
	if name, ok := p.senderNames.lookup(ctx, event.GetSenderId()); ok {
		event.SenderName = &name
	}
}
