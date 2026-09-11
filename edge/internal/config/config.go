// Package config 读 config/aite.yaml 里 edge 关心的那几段。
//
// 完整形状（冻结契约）在 core/crates/contracts/src/config.rs；这里是它的**子集镜像**，
// 字段名、默认值必须与之逐字一致 —— config_test.go 用 config/aite.example.yaml 钉住。
// owner: R0。加字段 → 先改 Rust 契约（需授权）再改这里。
package config

import (
	"fmt"
	"os"

	"gopkg.in/yaml.v3"
)

type Feishu struct {
	AppIDEnv      string `yaml:"app_id_env"`
	AppSecretEnv  string `yaml:"app_secret_env"`
	BotName       string `yaml:"bot_name"`
	BotOpenIDEnv  string `yaml:"bot_open_id_env"`
	HistoryWindow int    `yaml:"history_window"`
}

type Sandbox struct {
	Image          string  `yaml:"image"`
	CPU            float64 `yaml:"cpu"`
	MemMB          int     `yaml:"mem_mb"`
	IdleSec        int     `yaml:"idle_sec"`
	ExecTimeoutSec int     `yaml:"exec_timeout_sec"`
}

type Edge struct {
	EdgeSocket            string `yaml:"edge_socket"`
	CoreSocket            string `yaml:"core_socket"`
	HandleEventDeadlineMS int    `yaml:"handle_event_deadline_ms"`
	MaxMessageMB          int    `yaml:"max_message_mb"`
}

type Config struct {
	TenantID string  `yaml:"tenant_id"`
	Platform string  `yaml:"platform"` // feishu | fake
	Feishu   Feishu  `yaml:"feishu"`
	Sandbox  Sandbox `yaml:"sandbox"`
	Edge     Edge    `yaml:"edge"`
}

// Default 返回契约默认值（与 config.rs 的 Default impl 逐字一致）。
func Default() Config {
	return Config{
		TenantID: "default",
		Platform: "feishu",
		Feishu: Feishu{
			AppIDEnv:      "FEISHU_APP_ID",
			AppSecretEnv:  "FEISHU_APP_SECRET",
			BotName:       "Aite",
			BotOpenIDEnv:  "FEISHU_BOT_OPEN_ID",
			HistoryWindow: 50,
		},
		Sandbox: Sandbox{
			Image:          "aite-sandbox:p0",
			CPU:            1.0,
			MemMB:          1024,
			IdleSec:        300,
			ExecTimeoutSec: 120,
		},
		Edge: Edge{
			EdgeSocket:            "data/run/aite-edge.sock",
			CoreSocket:            "data/run/aite-core.sock",
			HandleEventDeadlineMS: 1000,
			MaxMessageMB:          64,
		},
	}
}

// Parse 在默认值之上叠加 YAML；未知键忽略（core 的字段 edge 不关心）。
func Parse(data []byte) (Config, error) {
	cfg := Default()
	if err := yaml.Unmarshal(data, &cfg); err != nil {
		return Config{}, fmt.Errorf("config: %w", err)
	}
	if err := cfg.Validate(); err != nil {
		return Config{}, err
	}
	return cfg, nil
}

func Load(path string) (Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Config{}, fmt.Errorf("config: %w", err)
	}
	return Parse(data)
}

func (c Config) Validate() error {
	switch c.Platform {
	case "feishu", "fake":
	default:
		return fmt.Errorf("config: platform 必须是 feishu 或 fake，得到 %q", c.Platform)
	}
	if c.Edge.HandleEventDeadlineMS <= 0 {
		return fmt.Errorf("config: edge.handle_event_deadline_ms 必须 > 0")
	}
	if c.Edge.MaxMessageMB <= 0 {
		return fmt.Errorf("config: edge.max_message_mb 必须 > 0")
	}
	if c.Edge.EdgeSocket == "" || c.Edge.CoreSocket == "" {
		return fmt.Errorf("config: edge.edge_socket / edge.core_socket 不能为空")
	}
	return nil
}
