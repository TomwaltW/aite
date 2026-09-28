// 媒体：入站下载 + AES-256-CBC 解密；出站分片上传。
//
// 下载链接 5 分钟有效、要用消息里的 aeskey 解密（plan §2C）。url / aeskey 只进包内有界表，
// Attachment.FileKey 只放不含密钥的短 id；Raw 里两者都脱敏（normalize.go）。
//
// 解密算法是参考资料整理的，TODO(真机核对)：key = base64(aeskey)（企微传统的 EncodingAESKey
// 是 43 个字符、不带 = 填充，严格解码会拒 → 先 StdEncoding 再 RawStdEncoding），必须恰好 32 字节；
// IV = key 前 16 字节；AES-256-CBC；PKCS#7 填充到 32 的倍数（去填充接受 1..32）。
package wecom

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// mediaURLTTL 是平台下载链接的有效期。
const mediaURLTTL = 5 * time.Minute

// mediaTableCap 是下载表容量。
const mediaTableCap = 4096

// maxDownloadBytes 是单个下载的读取上限（防一个坏链接把 edge 内存吃光）。
const maxDownloadBytes = 200 * 1024 * 1024

// pkcs7BlockSize 是企微惯例的填充块长（不是 AES 的 16）。
const pkcs7BlockSize = 32

// mediaEntry 是一条可下载资源；密钥只在 edge 内存里。
type mediaEntry struct {
	url       string
	aesKey    string
	expiresAt time.Time
}

func mediaKey(messageID, fileKey string) string { return messageID + "\x00" + fileKey }

// decodeAESKey 解出 32 字节 key：先带填充的标准 base64，失败再试无填充的。
func decodeAESKey(s string) ([]byte, error) {
	key, err := base64.StdEncoding.DecodeString(s)
	if err != nil {
		key, err = base64.RawStdEncoding.DecodeString(s)
	}
	if err != nil {
		return nil, errors.New("aeskey 不是合法的 base64")
	}
	if len(key) != 32 {
		return nil, fmt.Errorf("aeskey 解出 %d 字节，要恰好 32", len(key))
	}
	return key, nil
}

// cbcIV 是企微的 IV 推导：key 前 16 字节。
func cbcIV(key []byte) []byte { return key[:aes.BlockSize] }

// pkcs7Unpad 去掉 PKCS#7 填充（填充长度 1..blockSize），非法就报错。
func pkcs7Unpad(b []byte, blockSize int) ([]byte, error) {
	n := len(b)
	if n == 0 {
		return nil, errors.New("明文为空，没有填充可去")
	}
	pad := int(b[n-1])
	if pad < 1 || pad > blockSize || pad > n {
		return nil, fmt.Errorf("填充长度 %d 非法", pad)
	}
	for _, c := range b[n-pad:] {
		if int(c) != pad {
			return nil, errors.New("填充字节不一致")
		}
	}
	return b[:n-pad], nil
}

// decryptMedia 解密下载到的密文。任何非法输入都返回错误，绝不 panic。
func decryptMedia(aesKey string, data []byte) ([]byte, error) {
	key, err := decodeAESKey(aesKey)
	if err != nil {
		return nil, err
	}
	if len(data) == 0 || len(data)%aes.BlockSize != 0 {
		return nil, fmt.Errorf("密文长度 %d 不是 %d 的正整数倍", len(data), aes.BlockSize)
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	out := make([]byte, len(data))
	cipher.NewCBCDecrypter(block, cbcIV(key)).CryptBlocks(out, data)
	return pkcs7Unpad(out, pkcs7BlockSize)
}

// DownloadFile 查表 → HTTP GET → 解密。表里没有或过期 → file_expired（404，不可重试）。
func (p *Platform) DownloadFile(ctx context.Context, messageID, fileKey string) ([]byte, error) {
	p.mu.Lock()
	e, ok := p.media.get(mediaKey(messageID, fileKey))
	p.mu.Unlock()
	if !ok || !p.clock().Before(e.expiresAt) {
		return nil, &aiteerr.PlatformError{
			Code: "file_expired", HTTPStatus: http.StatusNotFound, Retryable: false,
			Msg: "企微下载链接不存在或已过 5 分钟有效期",
		}
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, e.url, nil)
	if err != nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Msg: "下载链接非法"}
	}
	resp, err := p.httpClient.Do(req)
	if err != nil {
		// 错误里可能带完整 url（5 分钟有效的下载链接），别往上抛原文。
		return nil, &aiteerr.PlatformError{Code: "network", Retryable: true, Msg: "下载失败"}
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, &aiteerr.PlatformError{
			Code: "http_" + strconv.Itoa(resp.StatusCode), HTTPStatus: resp.StatusCode,
			Retryable: resp.StatusCode >= 500, Msg: "下载返回非 200",
		}
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxDownloadBytes))
	if err != nil {
		return nil, &aiteerr.PlatformError{Code: "network", Retryable: true, Msg: "读取下载内容失败"}
	}
	plain, err := decryptMedia(e.aesKey, data)
	if err != nil {
		return nil, &aiteerr.PlatformError{Code: "decrypt_failed", Retryable: false, Msg: err.Error()}
	}
	return plain, nil
}

// SendFile 分片上传（每片恰好 512 KB、最后一片可小，按序发、每片等应答），拿到媒体 id
// 再走回复或主动发送。
//
// 本轨只执行协议上限（≤ 100 片 × 512 KB），超了在发出第一帧之前就报错。T0 的
// max_upload_bytes（20 MB，= 40 片）只是包内常量 MaxUploadBytes，**不在这里拦** ——
// 否则 20 MB 先拦住，片数上限那道检查永远走不到。20 MB 在哪里拦由 DD11 对到契约后定。
func (p *Platform) SendFile(ctx context.Context, msg *pb.OutboundFile) (*pb.SendResult, error) {
	if msg == nil {
		return nil, &aiteerr.PlatformError{Code: "bad_request", Msg: "SendFile 收到空消息"}
	}
	data := msg.GetData()
	if len(data) == 0 {
		return nil, &aiteerr.PlatformError{Code: "bad_request", HTTPStatus: http.StatusBadRequest, Msg: "SendFile 收到空文件"}
	}
	chunks := (len(data) + uploadChunkSize - 1) / uploadChunkSize
	if chunks > uploadMaxChunks {
		return nil, &aiteerr.PlatformError{
			Code: "file_too_large", HTTPStatus: http.StatusBadRequest, Retryable: false,
			Msg: fmt.Sprintf("文件 %d 字节要切 %d 片，超过企微分片上传上限 %d 片 × 512 KB", len(data), chunks, uploadMaxChunks),
		}
	}
	conn, err := p.currentConn()
	if err != nil {
		return nil, err
	}

	uploadID := newReqID()
	var mediaID string
	for i := 0; i < chunks; i++ {
		end := min((i+1)*uploadChunkSize, len(data))
		ack, err := conn.request(ctx, cmdUploadChunk, newReqID(), uploadChunkBody{
			UploadID: uploadID, FileName: msg.GetName(), ChunkIndex: i, TotalChunks: chunks,
			TotalSize: len(data), Data: data[i*uploadChunkSize : end],
		})
		if err != nil {
			return nil, err
		}
		if i == chunks-1 {
			var ref mediaRef
			if len(ack) > 0 {
				_ = json.Unmarshal(ack, &ref)
			}
			mediaID = ref.MediaID
		}
	}
	if mediaID == "" {
		return nil, &aiteerr.PlatformError{Code: "upload_no_media_id", Retryable: true, Msg: "分片上传完成但没拿到 media_id"}
	}

	if rc, ok := p.lookupReply(msg.ReplyTo); ok {
		if _, err := conn.request(ctx, cmdRespondMsg, rc.reqID, respondFileBody{
			MsgType: msgTypeFile, File: mediaRef{MediaID: mediaID},
		}); err != nil {
			return nil, err
		}
		return &pb.SendResult{MessageId: noStreamPrefix + uploadID}, nil
	}
	id, err := p.sendProactive(ctx, msg.GetChatId(), sendFileBody{
		ChatID: msg.GetChatId(), MsgType: msgTypeFile, File: mediaRef{MediaID: mediaID},
	})
	if err != nil {
		return nil, err
	}
	return &pb.SendResult{MessageId: id}, nil
}
