// 对应 tests/adapters/feishu/test_feishu_capabilities.py。
//
// 能力副本、配置装配、令牌桶。
//
// supports_passive_listen 保持契约默认值 false，但必须运行时可改，且改的是实例上的
// 副本 —— FeishuP0() 每次返回新对象，谁也污染不了谁。
package feishu

import (
	"context"
	"testing"
	"time"

	"google.golang.org/protobuf/proto"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/config"
)

func newTestPlatform(t *testing.T) *Platform {
	t.Helper()
	p, err := New(defaultFeishuConfig(), Options{BotOpenID: "ou_bot"}, nil)
	if err != nil {
		t.Fatalf("New 失败：%v", err)
	}
	return p
}

// ---------------------------------------------------------------------------
// 能力副本
// ---------------------------------------------------------------------------

// TestCapabilitiesDefaultToTheFrozenFeishuP0
// 对应 test_capabilities_default_to_the_frozen_feishu_p0。
func TestCapabilitiesDefaultToTheFrozenFeishuP0(t *testing.T) {
	p := newTestPlatform(t)
	if !proto.Equal(p.Capabilities(), FeishuP0()) {
		t.Errorf("默认能力 = %v，要等于 FeishuP0()", p.Capabilities())
	}
	if p.Capabilities().GetSupportsPassiveListen() {
		t.Error("supports_passive_listen 要保持契约默认值 false")
	}
}

// TestCapabilitiesAreAPerInstanceCopy 对应 test_capabilities_are_a_per_instance_copy。
func TestCapabilitiesAreAPerInstanceCopy(t *testing.T) {
	a, b := newTestPlatform(t), newTestPlatform(t)
	if a.Capabilities() == b.Capabilities() {
		t.Error("两个实例不该共享同一个 capabilities 对象")
	}
	if a.Capabilities() == FeishuP0() {
		t.Error("实例的 capabilities 不该是 FeishuP0() 的返回值本身")
	}
}

// TestSetPassiveListenDoesNotPolluteTheContractSingleton
// 对应 test_set_passive_listen_does_not_pollute_the_contract_singleton。
//
// 权限核实后要改的是 adapter 实例，不是契约常量。
func TestSetPassiveListenDoesNotPolluteTheContractSingleton(t *testing.T) {
	before := FeishuP0()

	p := newTestPlatform(t)
	p.SetPassiveListen(true)

	if !p.Capabilities().GetSupportsPassiveListen() {
		t.Error("实例上的 supports_passive_listen 没改成 true")
	}
	if FeishuP0().GetSupportsPassiveListen() {
		t.Error("FeishuP0() 被就地改了")
	}
	if !proto.Equal(FeishuP0(), before) {
		t.Error("契约常量被就地改了")
	}
	// 新建的实例仍然拿到干净的默认值。
	if newTestPlatform(t).Capabilities().GetSupportsPassiveListen() {
		t.Error("新实例拿到了被污染的默认值")
	}
}

// TestCapabilitiesCanBeInjected 对应 test_capabilities_can_be_injected。
func TestCapabilitiesCanBeInjected(t *testing.T) {
	custom := FeishuP0()
	custom.OutboundRatePerMin = 10

	p, err := newPlatform(platformOptions{
		opts: Options{BotOpenID: "ou_bot"}, caps: custom,
	})
	if err != nil {
		t.Fatalf("newPlatform 失败：%v", err)
	}
	if p.Capabilities().GetOutboundRatePerMin() != 10 {
		t.Errorf("outbound_rate_per_min = %d", p.Capabilities().GetOutboundRatePerMin())
	}
	if p.Capabilities() == custom {
		t.Error("注入的也要拷一份，别把调用方的对象攥在手里")
	}
	// 改实例不影响调用方手上的那份。
	p.SetPassiveListen(true)
	if custom.GetSupportsPassiveListen() {
		t.Error("改实例污染了调用方注入的对象")
	}
}

// TestOutboundRateComesFromTheCapabilities
// 对应 test_outbound_rate_comes_from_the_capabilities。
func TestOutboundRateComesFromTheCapabilities(t *testing.T) {
	p := newTestPlatform(t)
	if got := p.api.bucket.RatePerMin; got != 60 {
		t.Errorf("桶速率 = %d，要 60", got)
	}
	if int32(p.api.bucket.RatePerMin) != FeishuP0().GetOutboundRatePerMin() {
		t.Error("桶速率要取 outbound_rate_per_min，不是另写一个魔数")
	}
}

// ---------------------------------------------------------------------------
// 从 config.Feishu 装配
// ---------------------------------------------------------------------------

// TestConfigCarriesEnvVarNamesNotValues 对应 test_from_config_reads_credentials_by_env_var_name
// 的前半：契约里配置只存环境变量名不存值。
//
// Go 侧的差异：取环境变量这一步在 cmd/aite-edge（R2）里做，feishu.New 收的是
// 解析好的 Options —— 所以这里分成「config 里是变量名」与「New 按 Options 取值」两条。
func TestConfigCarriesEnvVarNamesNotValues(t *testing.T) {
	cfg := config.Default().Feishu
	for name, got := range map[string]string{
		"app_id_env":      cfg.AppIDEnv,
		"app_secret_env":  cfg.AppSecretEnv,
		"bot_open_id_env": cfg.BotOpenIDEnv,
	} {
		if got == "" {
			t.Errorf("%s 不能为空", name)
		}
	}
	if cfg.AppIDEnv != "FEISHU_APP_ID" || cfg.AppSecretEnv != "FEISHU_APP_SECRET" ||
		cfg.BotOpenIDEnv != "FEISHU_BOT_OPEN_ID" {
		t.Errorf("默认变量名跑偏了：%+v", cfg)
	}
	if cfg.HistoryWindow != 50 {
		t.Errorf("history_window = %d，要 50", cfg.HistoryWindow)
	}
}

// TestNewReadsCredentialsFromOptions 对应 test_from_config_reads_credentials_by_env_var_name
// 的后半与 test_from_config_honours_renamed_env_vars。
func TestNewReadsCredentialsFromOptions(t *testing.T) {
	// main 按 cfg 里记的变量名从环境变量取值，再传进来（改名也好、原名也好，
	// 到了 feishu 这一层只剩取好的值）。
	env := map[string]string{
		"MY_APP_ID":     "cli_from_env",
		"MY_APP_SECRET": "secret_from_env",
		"MY_BOT":        "ou_bot_from_env",
	}
	cfg := config.Feishu{
		AppIDEnv: "MY_APP_ID", AppSecretEnv: "MY_APP_SECRET",
		BotOpenIDEnv: "MY_BOT", BotName: "Aite", HistoryWindow: 30,
	}
	p, err := New(cfg, Options{
		AppID:     env[cfg.AppIDEnv],
		AppSecret: env[cfg.AppSecretEnv],
		BotOpenID: env[cfg.BotOpenIDEnv],
		TenantID:  "default",
	}, nil)
	if err != nil {
		t.Fatalf("New 失败：%v", err)
	}
	if p.opts.AppID != "cli_from_env" {
		t.Errorf("app_id = %q", p.opts.AppID)
	}
	if p.opts.BotOpenID != "ou_bot_from_env" {
		t.Errorf("bot_open_id = %q", p.opts.BotOpenID)
	}
	if p.api.appSecret != "secret_from_env" {
		t.Errorf("app_secret = %q", p.api.appSecret)
	}
	if p.historyWindow != cfg.HistoryWindow {
		t.Errorf("history_window = %d，要 %d", p.historyWindow, cfg.HistoryWindow)
	}
}

// TestNewWithMissingEnvDoesNotExplode 对应 test_from_config_with_missing_env_does_not_explode。
//
// 启动期缺变量不该在装配时炸；真正打不通是调 API 时的事。
func TestNewWithMissingEnvDoesNotExplode(t *testing.T) {
	p, err := New(config.Default().Feishu, Options{}, nil)
	if err != nil {
		t.Fatalf("缺变量不该在装配时炸：%v", err)
	}
	if p.opts.AppID != "" {
		t.Errorf("app_id = %q，要空", p.opts.AppID)
	}
	if p.opts.BotOpenID != "" {
		t.Errorf("bot_open_id = %q，要空", p.opts.BotOpenID)
	}
	// 缺 tenant_id / domain 时用默认值填上。
	if p.opts.TenantID != "default" {
		t.Errorf("tenant_id = %q，要 default", p.opts.TenantID)
	}
	if p.opts.Domain != DefaultDomain {
		t.Errorf("domain = %q，要 %s", p.opts.Domain, DefaultDomain)
	}
}

// ---------------------------------------------------------------------------
// 令牌桶
// ---------------------------------------------------------------------------

func mustBucket(t *testing.T, rate int, clock *fakeClock) *TokenBucket {
	t.Helper()
	b, err := NewTokenBucket(rate, 0, clock.Now, clock.Sleep)
	if err != nil {
		t.Fatalf("建桶失败：%v", err)
	}
	return b
}

// TestTokenBucketAllowsAFullBurstThenPaces
// 对应 test_token_bucket_allows_a_full_burst_then_paces。
func TestTokenBucketAllowsAFullBurstThenPaces(t *testing.T) {
	clock := newFakeClock()
	bucket := mustBucket(t, 60, clock)
	ctx := context.Background()

	for i := 0; i < 60; i++ {
		waited, err := bucket.Acquire(ctx, 1)
		if err != nil {
			t.Fatalf("第 %d 次取令牌失败：%v", i+1, err)
		}
		if waited != 0 {
			t.Fatalf("第 %d 次不该等，等了 %v", i+1, waited)
		}
	}
	if got := clock.Slept(); len(got) != 0 {
		t.Errorf("满桶 60 次不该 sleep，得到 %v", got)
	}

	// 第 61 次要等 1 秒（60/分钟 = 每秒 1 个）。
	waited, err := bucket.Acquire(ctx, 1)
	if err != nil {
		t.Fatal(err)
	}
	if waited != time.Second {
		t.Errorf("第 61 次等了 %v，要 1s", waited)
	}
	if got := clock.Slept(); len(got) != 1 || got[0] != time.Second {
		t.Errorf("slept = %v，要 [1s]", got)
	}
}

// TestTokenBucketRefillsOverTime 对应 test_token_bucket_refills_over_time。
func TestTokenBucketRefillsOverTime(t *testing.T) {
	clock := newFakeClock()
	bucket := mustBucket(t, 60, clock)
	ctx := context.Background()
	for i := 0; i < 60; i++ {
		if _, err := bucket.Acquire(ctx, 1); err != nil {
			t.Fatal(err)
		}
	}

	clock.Advance(10 * time.Second)
	if got := bucket.Tokens(); got < 9.999 || got > 10.001 {
		t.Errorf("补了 10 秒后 tokens = %v，要 10", got)
	}

	for i := 0; i < 10; i++ {
		waited, err := bucket.Acquire(ctx, 1)
		if err != nil {
			t.Fatal(err)
		}
		if waited != 0 {
			t.Errorf("补回来的 10 个不该等，第 %d 次等了 %v", i+1, waited)
		}
	}
	if got := clock.Slept(); len(got) != 0 {
		t.Errorf("slept = %v，要空", got)
	}
}

// TestTokenBucketNeverExceedsCapacity 对应 test_token_bucket_never_exceeds_capacity。
func TestTokenBucketNeverExceedsCapacity(t *testing.T) {
	clock := newFakeClock()
	bucket := mustBucket(t, 60, clock)
	clock.Advance(time.Hour)
	if got := bucket.Tokens(); got != 60 {
		t.Errorf("空闲一小时后 tokens = %v，要 60（不超过容量）", got)
	}
}

// TestTokenBucketRejectsANonPositiveRate
// 对应 test_token_bucket_rejects_a_non_positive_rate。
func TestTokenBucketRejectsANonPositiveRate(t *testing.T) {
	for _, rate := range []int{0, -1} {
		if _, err := NewTokenBucket(rate, 0, nil, nil); err == nil {
			t.Errorf("rate_per_min=%d 该报错", rate)
		}
	}
}

// TestTokenBucketRejectsMoreTokensThanCapacity 是 Go 侧补的一条。
func TestTokenBucketRejectsMoreTokensThanCapacity(t *testing.T) {
	clock := newFakeClock()
	bucket := mustBucket(t, 2, clock)
	if _, err := bucket.Acquire(context.Background(), 3); err == nil {
		t.Error("一次要 3 个令牌超过桶容量 2，该报错")
	}
}

// TestCapabilitiesEnumValuesMatchTheContract 是 Go 侧补的：逐字段钉住 FEISHU_P0。
func TestCapabilitiesEnumValuesMatchTheContract(t *testing.T) {
	caps := FeishuP0()
	want := &pb.PlatformCapabilities{
		Platform:                      "feishu",
		SupportsThread:                true,
		SupportsHistory:               true,
		SupportsPassiveListen:         false,
		SupportsCardEdit:              true,
		CardEditWindowSec:             1209600, // 14 天
		InboundFileInGroup:            true,
		ProactiveRequiresPriorMessage: false,
		OutboundRatePerMin:            60,
	}
	if !proto.Equal(caps, want) {
		t.Errorf("FeishuP0() = %v，要 %v", caps, want)
	}
}
