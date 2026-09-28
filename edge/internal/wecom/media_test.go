package wecom

import (
	"bytes"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// AES 向量：云端用 openssl 生成一次后写死（测试运行时不调任何外部命令）。
//
//	key = 00 01 … 1f（32 字节），IV = key 前 16 字节 = 00 01 … 0f
//	明文 = "企微 aibot 媒体解密向量 CC10"（36 字节 UTF-8）按 PKCS#7 填充到 32 的倍数（+28 个 0x1c = 64 字节）
//	openssl enc -aes-256-cbc -nopad -K 000102…1f -iv 000102…0f -in plain.bin -out cipher.bin
const (
	vectorAESKey   = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=" // base64(00..1f)
	vectorPlain    = "企微 aibot 媒体解密向量 CC10"
	vectorCipherHx = "6d161d33553aa3b2b39e1353432c8cd3f3897a07de8aabd8811e2f244803c3c5" +
		"acb37ad5afe1acee6dc6ea4ebf00713c391852b2a83a6dbfdb249a54dee2d1be"
)

func vectorCipher(t *testing.T) []byte {
	t.Helper()
	c, err := hex.DecodeString(vectorCipherHx)
	if err != nil {
		t.Fatalf("向量 hex 坏了：%v", err)
	}
	return c
}

func TestAES256CBCDecryptVector(t *testing.T) {
	cipherText := vectorCipher(t)

	got, err := decryptMedia(vectorAESKey, cipherText)
	if err != nil || string(got) != vectorPlain {
		t.Fatalf("openssl 向量解不出来：%q, %v", got, err)
	}

	// 企微传统的 43 字符、无 = 填充的 aeskey 也要能解出 32 字节。
	raw43 := vectorAESKey[:43]
	if len(raw43) != 43 {
		t.Fatalf("夹具不对")
	}
	key, err := decodeAESKey(raw43)
	if err != nil || len(key) != 32 {
		t.Fatalf("43 字符 aeskey 解不出 32 字节：%d, %v", len(key), err)
	}
	if got, err := decryptMedia(raw43, cipherText); err != nil || string(got) != vectorPlain {
		t.Fatalf("43 字符 aeskey 解密失败：%q, %v", got, err)
	}

	// IV = key 前 16 字节。
	if iv := cbcIV(key); !bytes.Equal(iv, key[:16]) || len(iv) != 16 {
		t.Fatalf("IV 推导不对：%x", iv)
	}

	// 按 32 块去 PKCS#7：填充 28（> 16）要认；33 / 0 / 不一致都要拒。
	padded := append([]byte("abcd"), bytes.Repeat([]byte{28}, 28)...)
	if out, err := pkcs7Unpad(padded, 32); err != nil || string(out) != "abcd" {
		t.Fatalf("32 块填充去不掉：%q, %v", out, err)
	}
	for name, bad := range map[string][]byte{
		"pad=0":   append(bytes.Repeat([]byte{'x'}, 31), 0),
		"pad=33":  append(bytes.Repeat([]byte{'x'}, 31), 33),
		"不一致":     append(bytes.Repeat([]byte{'x'}, 30), 1, 2),
		"pad>len": {5, 5},
	} {
		if _, err := pkcs7Unpad(bad, 32); err == nil {
			t.Fatalf("%s 的坏填充没报错", name)
		}
	}

	// 错 key：解不出原文（坏填充报错，或者出来是乱码）。
	wrongKey := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x42}, 32))
	if got, err := decryptMedia(wrongKey, cipherText); err == nil && string(got) == vectorPlain {
		t.Fatalf("错 key 居然解出了原文")
	}
	// key 不是 32 字节 / 不是 base64 / 密文长度不对 → 报错，绝不 panic。
	for name, tc := range map[string]struct {
		key  string
		data []byte
	}{
		"16 字节 key": {base64.StdEncoding.EncodeToString(make([]byte, 16)), cipherText},
		"非 base64":  {"!!!not-base64!!!", cipherText},
		"密文 15 字节":  {vectorAESKey, cipherText[:15]},
		"密文为空":      {vectorAESKey, nil},
	} {
		if _, err := decryptMedia(tc.key, tc.data); err == nil {
			t.Fatalf("%s 没报错", name)
		}
	}
}

func TestChunkedUpload512KB(t *testing.T) {
	fs := newFakeServer(t, func(f recvFrame) (int, any) {
		if f.Cmd == cmdUploadChunk {
			idx, _ := f.Body["chunk_index"].(float64)
			total, _ := f.Body["total_chunks"].(float64)
			if idx == total-1 {
				return 0, map[string]any{"media_id": "media-1"}
			}
		}
		return 0, nil
	})
	h := startHarness(t, fs, nil)
	h.waitConnected(t)
	ctx := t.Context()
	h.inbound(t, "rq-1", textMsg("m-1", "chat-1", "single", "发我报告"))

	size := 1200 * 1024 // 1.2 MB → 3 片
	data := bytes.Repeat([]byte{0xAB}, size)
	if _, err := h.p.SendFile(ctx, &pb.OutboundFile{ChatId: "chat-1", Name: "report.pdf", Mime: "application/pdf", Data: data}); err != nil {
		t.Fatalf("SendFile: %v", err)
	}
	chunks := fs.framesWith(cmdUploadChunk)
	if len(chunks) != 3 {
		t.Fatalf("1.2 MB 要切 3 片，得到 %d", len(chunks))
	}
	var total int
	for i, c := range chunks {
		if idx, _ := c.Body["chunk_index"].(float64); int(idx) != i {
			t.Fatalf("第 %d 片序号 = %v，要连续", i, c.Body["chunk_index"])
		}
		b64, _ := c.Body["data"].(string)
		raw, err := base64.StdEncoding.DecodeString(b64)
		if err != nil {
			t.Fatalf("第 %d 片 data 不是 base64：%v", i, err)
		}
		if i < 2 && len(raw) != 524288 {
			t.Fatalf("第 %d 片 = %d 字节，要恰好 524288", i, len(raw))
		}
		total += len(raw)
	}
	if total != size {
		t.Fatalf("分片合计 %d 字节，要 %d", total, size)
	}
	sends := fs.framesWith(cmdSendMsg)
	if len(sends) != 1 {
		t.Fatalf("上传后要恰好一帧主动发送：%+v", sends)
	}
	if file, _ := sends[0].Body["file"].(map[string]any); file["media_id"] != "media-1" {
		t.Fatalf("发送帧没带 media_id：%v", sends[0].Body)
	}

	// 100 × 512 KB + 1 字节 → 101 片，发第一帧之前就报错。
	before := len(fs.snapshot())
	big := make([]byte, 100*uploadChunkSize+1)
	_, err := h.p.SendFile(ctx, &pb.OutboundFile{ChatId: "chat-1", Name: "big.bin", Data: big})
	if platformCode(err) != "file_too_large" {
		t.Fatalf("101 片要报 file_too_large：%v", err)
	}
	if n := len(fs.snapshot()); n != before {
		t.Fatalf("超限还发了帧：%d → %d", before, n)
	}
	// 恰好 100 片不在上限外（只数帧、不看结果）。
	if uploadMaxChunks != 100 || uploadChunkSize != 512*1024 {
		t.Fatalf("协议上限常量被改了")
	}
}

func TestDownloadFileDecryptsAndExpires(t *testing.T) {
	cipherText := vectorCipher(t)
	var hits atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		if r.URL.Path != "/media/img-1" {
			http.NotFound(w, r)
			return
		}
		_, _ = w.Write(cipherText)
	}))
	t.Cleanup(srv.Close)

	clock := newFakeClock()
	sink := &recordingSink{}
	p, err := newPlatform(platformOptions{
		cfg:   DefaultConfig(),
		opts:  Options{BotID: "bot-test", WSURL: "ws://127.0.0.1:1"},
		sink:  sink,
		clock: clock.Now,
	})
	if err != nil {
		t.Fatalf("newPlatform: %v", err)
	}
	body := map[string]any{
		"msgid": "m-img", "aibotid": "bot-test", "chatid": "u-1", "chattype": "single",
		"from": map[string]any{"userid": "u-1"}, "msgtype": "image",
		"image": map[string]any{"url": srv.URL + "/media/img-1", "aeskey": vectorAESKey},
	}
	f := frameOf(t, cmdMsgCallback, "rq-img", body)
	var cb msgCallback
	if err := json.Unmarshal(f.Body, &cb); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	p.handleMessage(t.Context(), f, &cb, clock.Now())
	evs := sink.all()
	if len(evs) != 1 || len(evs[0].GetAttachments()) != 1 {
		t.Fatalf("单聊图片要出一个带附件的事件：%v", evs)
	}
	att := evs[0].GetAttachments()[0]

	got, err := p.DownloadFile(t.Context(), att.GetMessageId(), att.GetFileKey())
	if err != nil || string(got) != vectorPlain {
		t.Fatalf("下载解密不对：%q, %v", got, err)
	}

	clock.Advance(5 * time.Minute)
	_, err = p.DownloadFile(t.Context(), att.GetMessageId(), att.GetFileKey())
	pe, ok := err.(*aiteerr.PlatformError)
	if !ok || pe.Code != "file_expired" || pe.HTTPStatus != 404 || pe.Retryable {
		t.Fatalf("5 分钟后要 file_expired：%v", err)
	}
	if hits.Load() != 1 {
		t.Fatalf("过期后不该再发 HTTP 请求：共 %d 次", hits.Load())
	}
	if _, err := p.DownloadFile(t.Context(), "m-none", "img-0"); platformCode(err) != "file_expired" {
		t.Fatalf("表里没有也要 file_expired：%v", err)
	}
}
