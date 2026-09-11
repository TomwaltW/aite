package config

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func repoRoot(t *testing.T) string {
	t.Helper()
	_, file, _, _ := runtime.Caller(0)
	return filepath.Clean(filepath.Join(filepath.Dir(file), "..", "..", ".."))
}

// 样例文件里写的全是契约默认值 → 解析结果必须等于 Default()。
func TestExampleEqualsDefault(t *testing.T) {
	cfg, err := Load(filepath.Join(repoRoot(t), "config", "aite.example.yaml"))
	if err != nil {
		t.Fatal(err)
	}
	if cfg != Default() {
		t.Fatalf("example != default\n got %+v\nwant %+v", cfg, Default())
	}
}

func TestMissingEdgeSectionUsesDefaults(t *testing.T) {
	cfg, err := Parse([]byte("platform: fake\nsandbox:\n  image: x:1\n"))
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Platform != "fake" || cfg.Sandbox.Image != "x:1" {
		t.Fatalf("override lost: %+v", cfg)
	}
	if cfg.Edge != Default().Edge || cfg.Sandbox.MemMB != 1024 {
		t.Fatalf("defaults lost: %+v", cfg)
	}
}

func TestUnknownKeysIgnored(t *testing.T) {
	if _, err := Parse([]byte("model:\n  provider: scripted\nworker:\n  max_steps: 3\n")); err != nil {
		t.Fatal(err)
	}
}

func TestValidate(t *testing.T) {
	if _, err := Parse([]byte("platform: dingtalk\n")); err == nil {
		t.Error("bad platform must fail")
	}
	if _, err := Parse([]byte("edge:\n  handle_event_deadline_ms: 0\n")); err == nil {
		t.Error("zero deadline must fail")
	}
	if _, err := Load(filepath.Join(os.TempDir(), "definitely-missing-aite.yaml")); err == nil {
		t.Error("missing file must fail")
	}
}
