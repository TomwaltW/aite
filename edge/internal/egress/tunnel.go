package egress

import (
	"io"
	"net"
	"net/http"
	"net/textproto"
	"strings"
	"sync"
	"time"
)

// hopHeaders 是逐跳头：只属于沙箱 ↔ 代理这一跳，绝不往上游透传（Proxy-Authorization 里是令牌）。
var hopHeaders = []string{
	"Proxy-Authorization",
	"Proxy-Authenticate",
	"Proxy-Connection",
	"Connection",
	"Keep-Alive",
	"Te",
	"Trailer",
	"Upgrade",
}

// removeHopHeaders 先删 Connection 点名的头，再删固定的逐跳头。
func removeHopHeaders(h http.Header) {
	for _, v := range h.Values("Connection") {
		for _, name := range strings.Split(v, ",") {
			if name = textproto.TrimString(name); name != "" {
				h.Del(name)
			}
		}
	}
	for _, k := range hopHeaders {
		h.Del(k)
	}
}

// tunnel 处理放行了的 CONNECT：拨上游 → 200 → hijack → 双向拷贝；隧道关了再写审计行。
func (p *Proxy) tunnel(w http.ResponseWriter, r *http.Request, start time.Time, ev *NetworkEvent, tgt target) {
	up, err := p.dial(r.Context(), "tcp", tgt.String())
	if err != nil {
		p.upstreamError(w, start, ev, tgt, err)
		return
	}
	hj, ok := w.(http.Hijacker)
	if !ok {
		_ = up.Close()
		p.upstreamError(w, start, ev, tgt, http.ErrNotSupported)
		return
	}
	client, brw, err := hj.Hijack()
	if err != nil {
		_ = up.Close()
		p.log.Warn("egress.upstream_error", "host", tgt.host, "port", tgt.port, "err", err)
		ev.Status = http.StatusInternalServerError
		p.finish(start, ev)
		return
	}
	if !p.trackConn(client) || !p.trackConn(up) {
		// Serve 已在收尾：两头都关掉，不再建隧道。
		_ = client.Close()
		_ = up.Close()
		p.untrackConn(client)
		p.untrackConn(up)
		ev.Status = http.StatusServiceUnavailable
		p.finish(start, ev)
		return
	}
	defer func() {
		_ = client.Close()
		_ = up.Close()
		p.untrackConn(client)
		p.untrackConn(up)
	}()

	ev.Status = http.StatusOK
	if _, err := io.WriteString(client, "HTTP/1.1 200 Connection Established\r\n\r\n"); err != nil {
		p.finish(start, ev)
		return
	}

	var wg sync.WaitGroup
	wg.Add(2)
	// 上行：先吐 hijack 时 bufio 里已缓冲的字节，再接着读连接（brw.Reader 会自己往下读）。
	go p.pipe(&wg, &ev.BytesUp, up, brw.Reader)
	go p.pipe(&wg, &ev.BytesDown, client, up)
	wg.Wait()
	p.finish(start, ev)
}

// pipe 把 src 拷进 dst、记字节数；src 读完后对 dst 半关写（不支持就整个关），
// 让另一头读到 EOF 自然收尾。
func (p *Proxy) pipe(wg *sync.WaitGroup, n *int64, dst net.Conn, src io.Reader) {
	p.copiers.Add(1)
	defer func() {
		p.copiers.Add(-1)
		wg.Done()
	}()
	*n, _ = io.Copy(dst, src)
	if cw, ok := dst.(interface{ CloseWrite() error }); ok {
		_ = cw.CloseWrite()
	} else {
		_ = dst.Close()
	}
}

// forward 处理放行了的 absolute-URI 请求：剥逐跳头后经自建 Transport 发往上游。
func (p *Proxy) forward(w http.ResponseWriter, r *http.Request, start time.Time, ev *NetworkEvent, tgt target) {
	out := r.Clone(r.Context())
	out.RequestURI = ""
	out.Close = false
	out.URL.Host = tgt.String() // 拨的就是判过的那个（规范化后的）主机:端口；Host 头原样

	removeHopHeaders(out.Header)
	var up *countingReader
	if out.Body != nil && out.Body != http.NoBody {
		up = &countingReader{ReadCloser: out.Body}
		out.Body = up
	}
	resp, err := p.transport.RoundTrip(out)
	if up != nil {
		ev.BytesUp = up.count()
	}
	if err != nil {
		p.upstreamError(w, start, ev, tgt, err)
		return
	}
	defer resp.Body.Close()

	removeHopHeaders(resp.Header)
	for k, vs := range resp.Header {
		for _, v := range vs {
			w.Header().Add(k, v)
		}
	}
	w.WriteHeader(resp.StatusCode)
	ev.Status = resp.StatusCode
	ev.BytesDown, _ = io.Copy(w, resp.Body)
	if up != nil {
		ev.BytesUp = up.count()
	}
	p.finish(start, ev)
}

// countingReader 数请求体字节（RoundTrip 在另一个 goroutine 读它，但读完才返回响应，
// 我们只在 RoundTrip 返回之后取 n —— 仍可能与 Transport 写请求体的 goroutine 并发，
// 所以 n 用锁护着）。
type countingReader struct {
	io.ReadCloser
	mu sync.Mutex
	n  int64
}

func (c *countingReader) Read(b []byte) (int, error) {
	k, err := c.ReadCloser.Read(b)
	c.mu.Lock()
	c.n += int64(k)
	c.mu.Unlock()
	return k, err
}

func (c *countingReader) count() int64 {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.n
}
