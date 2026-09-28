package feishu

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"regexp"
	"strconv"
	"strings"

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
