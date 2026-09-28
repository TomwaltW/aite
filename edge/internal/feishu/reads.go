package feishu

import (
	"context"
	"fmt"
	"net/http"
	"regexp"
	"strconv"
	"strings"

	"google.golang.org/protobuf/types/known/timestamppb"

	pb "aite/edge/gen/aitepb"
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
