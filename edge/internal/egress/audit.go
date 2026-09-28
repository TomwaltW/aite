package egress

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// NetworkEvent 是每个请求写出的一行审计（JSONL）。13 个键逐字对齐 T0 的
// domain.rs NetworkEvent；键名、个数、类型改动都要同步那边。
// 不记：令牌、任何请求 / 响应头、path、query、正文（URL 里常带签名与令牌）。
type NetworkEvent struct {
	TS         string `json:"ts"` // 请求开始时刻，UTC，固定 6 位小数 + Z
	AuditTag   string `json:"audit_tag"`
	Level      string `json:"level"`
	Method     string `json:"method"`
	Host       string `json:"host"` // 规范化后、不含端口
	Port       int    `json:"port"`
	Decision   string `json:"decision"` // allow / deny
	Reason     string `json:"reason"`   // 放行为 ""
	Status     int    `json:"status"`   // 回给沙箱的状态码
	BytesUp    int64  `json:"bytes_up"`
	BytesDown  int64  `json:"bytes_down"`
	DurationMS int64  `json:"duration_ms"`
	Injected   bool   `json:"injected"` // 本轨恒 false；EE10 起注入凭证时 true
}

const (
	decisionAllow = "allow"
	decisionDeny  = "deny"

	// tsLayout 与 store 的 created_at 同形（store/src/lib.rs 的 stamp）：定长，字典序 = 时间序。
	tsLayout   = "2006-01-02T15:04:05.000000Z"
	hourLayout = "2006010215"
)

func stamp(t time.Time) string { return t.UTC().Format(tsLayout) }

// hourFile 是 t 所在 UTC 小时的文件名。一行只进它 ts 所在小时的文件
// （跨小时的长隧道仍写开始那个小时），文件名 ⇔ ts 范围一一对应。
func hourFile(t time.Time) string { return t.UTC().Format(hourLayout) + ".jsonl" }

// auditWriter 把 NetworkEvent 追加进 <dir>/YYYYMMDDHH.jsonl。每行一次 Write，并发写由锁串行。
type auditWriter struct {
	dir string
	mu  sync.Mutex
}

func (a *auditWriter) write(start time.Time, ev NetworkEvent) error {
	line, err := json.Marshal(ev)
	if err != nil {
		return err
	}
	line = append(line, '\n')
	a.mu.Lock()
	defer a.mu.Unlock()
	if err := os.MkdirAll(a.dir, 0o755); err != nil {
		return err
	}
	f, err := os.OpenFile(filepath.Join(a.dir, hourFile(start)), os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	if _, err := f.Write(line); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}
