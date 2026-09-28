package egress

import (
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"
)

func TestAuditLineSchema(t *testing.T) {
	const secret = "tok-SECRET-cc11-7f3a"
	h := newHarness(t)
	srv := upstream(t)
	h.route("mirror.test:80", srv.Listener.Addr().String())
	h.register(secret, Policy{Level: LevelCustom, AllowHosts: []string{"mirror.test"}, AuditTag: "task-schema"})

	// 一次放行、一次拒绝、一次未知令牌（令牌原文带在请求里），日志与审计都要干净。
	h.get(secret, "http://mirror.test/p?sig="+secret, nil)
	h.event()
	h.get(secret, "http://other.test/?sig="+secret, nil)
	h.event()
	h.get(secret+"-wrong", "http://mirror.test/", nil)
	h.event()

	want := []string{"audit_tag", "bytes_down", "bytes_up", "decision", "duration_ms", "host", "injected",
		"level", "method", "port", "reason", "status", "ts"}
	files := h.auditFiles()
	var raw strings.Builder
	n := 0
	for name, lines := range files {
		for _, line := range lines {
			raw.WriteString(line)
			n++
			var m map[string]any
			if err := json.Unmarshal([]byte(line), &m); err != nil {
				t.Fatalf("%s：%q 不是 JSON 对象：%v", name, line, err)
			}
			keys := make([]string, 0, len(m))
			for k := range m {
				keys = append(keys, k)
			}
			sort.Strings(keys)
			if strings.Join(keys, ",") != strings.Join(want, ",") {
				t.Errorf("键集合 = %v，想要恰好 %v", keys, want)
			}
			for _, k := range []string{"ts", "audit_tag", "level", "method", "host", "decision", "reason"} {
				if _, ok := m[k].(string); !ok {
					t.Errorf("%s 应是 string：%T", k, m[k])
				}
			}
			for _, k := range []string{"port", "status", "bytes_up", "bytes_down", "duration_ms"} {
				f, ok := m[k].(float64)
				if !ok || f != float64(int64(f)) {
					t.Errorf("%s 应是整数：%v", k, m[k])
				}
			}
			if inj, ok := m["injected"].(bool); !ok || inj {
				t.Errorf("injected 应是 false：%v", m["injected"])
			}
			ts, _ := m["ts"].(string)
			parsed, err := time.Parse(tsLayout, ts)
			if err != nil || len(ts) != len("2006-01-02T15:04:05.000000Z") || parsed.Location() != time.UTC {
				t.Errorf("ts %q 不是 UTC 定长 6 位小数 + Z：%v", ts, err)
			}
			if name != hourFile(parsed) {
				t.Errorf("ts %q 写进了 %s", ts, name)
			}
		}
	}
	if n != 3 {
		t.Errorf("审计行 %d 条，想要 3", n)
	}
	if strings.Contains(raw.String(), "tok-SECRET") || strings.Contains(raw.String(), "sig=") {
		t.Errorf("审计文件里有令牌原文或 query：%s", raw.String())
	}
	if logs := h.logs.String(); strings.Contains(logs, "tok-SECRET") || strings.Contains(logs, "sig=") {
		t.Errorf("日志里有令牌原文或 query：%s", logs)
	} else if !strings.Contains(logs, "egress.denied") {
		t.Errorf("没捕获到 egress.denied 日志：%s", logs)
	}
}

func TestHourlyRotationWithInjectedClock(t *testing.T) {
	h := newHarness(t)
	srv := upstream(t)
	h.route("mirror.test:80", srv.Listener.Addr().String())
	h.register("tok", Policy{Level: LevelCustom, AllowHosts: []string{"mirror.test"}})

	h.clock.Set(time.Date(2026, 9, 25, 10, 59, 59, 500_000_000, time.UTC))
	if status, _, _ := h.get("tok", "http://mirror.test/", nil); status != http.StatusOK {
		t.Fatalf("status=%d", status)
	}
	h.event()
	h.clock.Set(time.Date(2026, 9, 25, 11, 0, 0, 500_000_000, time.UTC))
	if status, _, _ := h.get("tok", "http://mirror.test/", nil); status != http.StatusOK {
		t.Fatalf("status=%d", status)
	}
	h.event()

	files := h.auditFiles()
	want := map[string]string{
		"2026092510.jsonl": "2026-09-25T10:59:59.500000Z",
		"2026092511.jsonl": "2026-09-25T11:00:00.500000Z",
	}
	if len(files) != len(want) {
		t.Fatalf("审计文件 = %v，想要 %v", files, want)
	}
	for name, ts := range want {
		lines := files[name]
		if len(lines) != 1 {
			t.Errorf("%s 有 %d 行，想要 1", name, len(lines))
			continue
		}
		var ev NetworkEvent
		if err := json.Unmarshal([]byte(lines[0]), &ev); err != nil {
			t.Fatal(err)
		}
		if ev.TS != ts {
			t.Errorf("%s 的 ts = %q，想要 %q", name, ev.TS, ts)
		}
	}
	// 权限位显式写：文件 0644、目录 0755（umask 022 下）。
	if st, err := os.Stat(filepath.Join(h.dir, "2026092510.jsonl")); err == nil && st.Mode().Perm()&0o044 != 0o044 {
		t.Errorf("审计文件权限 %v，宿主机要读得动", st.Mode().Perm())
	}
}
