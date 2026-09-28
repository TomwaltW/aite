// 企微智能机器人（aibot）长连接协议的全部假设都集中在这个文件里。
//
// 命令名 / 事件名来自 CC10 原卡（权威）；其余字段名是派单作者按公开资料整理的参考，
// 云端打不开 developer.work.weixin.qq.com（文档 101463），**未对真机核实** ——
// 每一处都在 docs/p1/wecom.md 里标了 TODO(真机核对)，H9 真机冒烟时逐条核。
// 改协议形状只改这里，别的文件只认这里的常量与结构体。
package wecom

import (
	"encoding/json"
	"strconv"
)

// 命令名（原卡给定）。
const (
	cmdSubscribe      = "aibot_subscribe"
	cmdPing           = "ping"
	cmdMsgCallback    = "aibot_msg_callback"
	cmdEventCallback  = "aibot_event_callback"
	cmdRespondMsg     = "aibot_respond_msg"
	cmdRespondWelcome = "aibot_respond_welcome_msg"
	cmdRespondUpdate  = "aibot_respond_update_msg"
	cmdSendMsg        = "aibot_send_msg"
	// cmdUploadChunk 是分片上传的命令名。TODO(真机核对)：原卡没给命令名，这是占位，
	// 真机核对前别当真；测试只引用这个常量、不钉字面值。
	cmdUploadChunk = "aibot_upload_media_chunk"
)

// 事件回调里的 event.eventtype（原卡给定）。
const (
	eventEnterChat         = "enter_chat"
	eventTemplateCardEvent = "template_card_event"
	eventFeedbackEvent     = "feedback_event"
	eventDisconnected      = "disconnected_event"
)

// 消息回调的 chattype / msgtype。TODO(真机核对)。
const (
	chatTypeSingle = "single"
	chatTypeGroup  = "group"

	msgTypeText   = "text"
	msgTypeMixed  = "mixed"
	msgTypeImage  = "image"
	msgTypeFile   = "file"
	msgTypeStream = "stream"
	msgTypeMD     = "markdown"
)

// feedback_event.type 的取值。TODO(真机核对)：1 = 点赞（准确）、2 = 点踩（不准确）、3 = 取消。
const (
	feedbackTypeLike    = 1
	feedbackTypeDislike = 2
	feedbackTypeCancel  = 3
)

// 分片上传的协议上限（原卡给定）：每片 512 KB、最多 100 片。
const (
	uploadChunkSize = 512 * 1024
	uploadMaxChunks = 100
)

// retryableErrcodes 是可重试的平台 errcode。TODO(真机核对)：-1 系统繁忙、45009 调用频率超限，
// 其余一律按不可重试处理（core 不会重发一条注定失败的消息）。
var retryableErrcodes = map[int]bool{-1: true, 45009: true}

// frameHeaders 是帧头；目前只用 req_id。
type frameHeaders struct {
	ReqID string `json:"req_id"`
}

// outFrame 是客户端发出的帧：{"cmd", "headers": {"req_id"}, "body"}。
type outFrame struct {
	Cmd     string       `json:"cmd"`
	Headers frameHeaders `json:"headers"`
	Body    any          `json:"body,omitempty"`
}

// inFrame 是服务端推来的帧。回调帧带 cmd + body；应答帧不带 cmd、带 errcode / errmsg
// （{"headers": {"req_id"}, "errcode": 0, "errmsg": "ok"}）。
type inFrame struct {
	Cmd     string          `json:"cmd"`
	Headers frameHeaders    `json:"headers"`
	Body    json.RawMessage `json:"body"`
	ErrCode *int            `json:"errcode"`
	ErrMsg  string          `json:"errmsg"`
}

// isAck：没有 cmd、有 errcode 的就是应答帧。
func (f *inFrame) isAck() bool { return f.Cmd == "" && f.ErrCode != nil }

// subscribeBody 是 aibot_subscribe 的 body。secret 只在这一帧里出现，永不进日志。
type subscribeBody struct {
	BotID  string `json:"bot_id"`
	Secret string `json:"secret"`
}

// fromUser 是消息 / 事件的发送者。userid 除非 bot 由超管创建否则是加密的（plan §1.3）。
type fromUser struct {
	UserID string `json:"userid"`
}

type textBody struct {
	Content string `json:"content"`
}

// mediaBody 是 image / file 消息体：下载链接 5 分钟有效、要用 aeskey 解密。
type mediaBody struct {
	URL      string `json:"url"`
	AESKey   string `json:"aeskey"`
	FileName string `json:"filename,omitempty"` // TODO(真机核对)：文件名字段
}

// mixedItem 是图文混排里的一项。
type mixedItem struct {
	MsgType string     `json:"msgtype"`
	Text    *textBody  `json:"text,omitempty"`
	Image   *mediaBody `json:"image,omitempty"`
}

type mixedBody struct {
	MsgItem []mixedItem `json:"msg_item"`
}

// quoteBody 是引用消息。企微的引用**不带 msgid**，只能从文字里找可见锚点 #A..。
type quoteBody struct {
	MsgType string     `json:"msgtype"`
	Text    *textBody  `json:"text,omitempty"`
	Mixed   *mixedBody `json:"mixed,omitempty"`
	Image   *mediaBody `json:"image,omitempty"`
	File    *mediaBody `json:"file,omitempty"`
}

// msgCallback 是 aibot_msg_callback 的 body。
type msgCallback struct {
	MsgID    string     `json:"msgid"`
	AibotID  string     `json:"aibotid"`
	ChatID   string     `json:"chatid"`
	ChatType string     `json:"chattype"`
	From     fromUser   `json:"from"`
	MsgType  string     `json:"msgtype"`
	Text     *textBody  `json:"text,omitempty"`
	Mixed    *mixedBody `json:"mixed,omitempty"`
	Image    *mediaBody `json:"image,omitempty"`
	File     *mediaBody `json:"file,omitempty"`
	Quote    *quoteBody `json:"quote,omitempty"`
}

// templateCardEvent 是模板卡片按钮回调。task_id 是模板卡片自带的任务 id，
// DD11 渲染卡片时放 Aite 的任务 id 进去。TODO(真机核对)。
type templateCardEvent struct {
	CardType string `json:"card_type"`
	EventKey string `json:"event_key"`
	TaskID   string `json:"task_id"`
}

// feedbackEventBody 是点赞 / 点踩回调。id = 开流时填的 stream.feedback.id。TODO(真机核对)。
type feedbackEventBody struct {
	ID                   string `json:"id"`
	Type                 int    `json:"type"`
	Content              string `json:"content"`
	InaccurateReasonList []int  `json:"inaccurate_reason_list"`
}

type eventInner struct {
	EventType         string             `json:"eventtype"`
	TemplateCardEvent *templateCardEvent `json:"template_card_event,omitempty"`
	FeedbackEvent     *feedbackEventBody `json:"feedback_event,omitempty"`
}

// eventCallback 是 aibot_event_callback 的 body。
type eventCallback struct {
	MsgID    string     `json:"msgid"`
	AibotID  string     `json:"aibotid"`
	ChatID   string     `json:"chatid"`
	ChatType string     `json:"chattype"`
	From     fromUser   `json:"from"`
	Event    eventInner `json:"event"`
}

// streamFeedback 让用户在流式回复上点赞 / 点踩；id 回到 feedback_event.id。
type streamFeedback struct {
	ID string `json:"id"`
}

// streamPayload 是流式回复：同一个 id 上推全量内容，finish=true 收尾（平台 10 分钟上限）。
type streamPayload struct {
	ID       string          `json:"id"`
	Finish   bool            `json:"finish"`
	Content  string          `json:"content"`
	Feedback *streamFeedback `json:"feedback,omitempty"`
}

// respondStreamBody 是 aibot_respond_msg 的 body（msgtype = stream）。
type respondStreamBody struct {
	MsgType string        `json:"msgtype"`
	Stream  streamPayload `json:"stream"`
}

// welcomeBody 是 aibot_respond_welcome_msg 的 body。
type welcomeBody struct {
	MsgType string   `json:"msgtype"`
	Text    textBody `json:"text"`
}

// updateCardBody 是 aibot_respond_update_msg 的 body。卡片形状归 DD11，这里原样透传。
type updateCardBody struct {
	ResponseType string         `json:"response_type"`
	TemplateCard map[string]any `json:"template_card"`
}

// sendMarkdownBody 是 aibot_send_msg（主动发送）的 markdown 体。
type sendMarkdownBody struct {
	ChatID   string   `json:"chatid"`
	MsgType  string   `json:"msgtype"`
	Markdown textBody `json:"markdown"`
}

// mediaRef 是上传后拿到的媒体 id。
type mediaRef struct {
	MediaID string `json:"media_id"`
}

// sendFileBody 是 aibot_send_msg 发文件；respondFileBody 是 aibot_respond_msg 回文件。TODO(真机核对)。
type sendFileBody struct {
	ChatID  string   `json:"chatid"`
	MsgType string   `json:"msgtype"`
	File    mediaRef `json:"file"`
}

type respondFileBody struct {
	MsgType string   `json:"msgtype"`
	File    mediaRef `json:"file"`
}

// uploadChunkBody 是一片上传。TODO(真机核对)：字段名全部是占位。
type uploadChunkBody struct {
	UploadID    string `json:"upload_id"`
	FileName    string `json:"filename"`
	ChunkIndex  int    `json:"chunk_index"`
	TotalChunks int    `json:"total_chunks"`
	TotalSize   int    `json:"total_size"`
	Data        []byte `json:"data"` // encoding/json 按 base64 编码
}

// errcodeString 把 errcode 转成 PlatformError.Code 用的十进制串。
func errcodeString(code int) string { return strconv.Itoa(code) }
