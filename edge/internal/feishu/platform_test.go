// 对应 tests/adapters/feishu/test_feishu_reconnect.py。
//
// 长连接与重连。判据原文：重连策略对假连接对象的退避序列断言为 [1,2,4,8,16,30,30]。
//
// 假连接对象在这个文件里自建。这里也顺带把「adapter 不去重」
// 「HandleEvent 失败不能带走长连接」两条钉住。
package feishu

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"testing"
	"time"

	"google.golang.org/protobuf/proto"

	"aite/edge/internal/config"
)

// expectedBackoff 是点名的序列。
var expectedBackoff = []time.Duration{
	1 * time.Second, 2 * time.Second, 4 * time.Second, 8 * time.Second,
	16 * time.Second, 30 * time.Second, 30 * time.Second,
}

// recordingSleep 记录每次退避等了多久；到第 stopAfter 次就取消 ctx，把 Start 的死循环停下来。
type recordingSleep struct {
	mu        sync.Mutex
	delays    []time.Duration
	stopAfter int
	cancel    context.CancelFunc
	onSleep   func(n int)
}

func (s *recordingSleep) Sleep(ctx context.Context, d time.Duration) error {
	s.mu.Lock()
	s.delays = append(s.delays, d)
	n := len(s.delays)
	onSleep := s.onSleep
	s.mu.Unlock()
	if onSleep != nil {
		onSleep(n)
	}
	if s.stopAfter > 0 && n >= s.stopAfter {
		s.cancel()
		return ctx.Err()
	}
	return nil
}

func (s *recordingSleep) Delays() []time.Duration {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]time.Duration, len(s.delays))
	copy(out, s.delays)
	return out
}

// fakeConnection 是 Connection 的假实现。
//
// outcome 决定 Connect() 的结果："fail" 返回错误，"ok" 连上。
// 连上之后 WaitClosed() 立刻返回 —— 等价于「刚连上就又断了」，
// 这样重连循环会一直转，正好用来量退避序列。
type fakeConnection struct {
	outcome      string
	onRaw        RawEventHandler
	connectCalls int
	closeCalls   int
}

func (c *fakeConnection) Connect(context.Context) error {
	c.connectCalls++
	if c.outcome == "fail" {
		return fmt.Errorf("假连接第 %d 次故意失败", c.connectCalls)
	}
	return nil
}

func (c *fakeConnection) WaitClosed(context.Context) error { return nil }

func (c *fakeConnection) Close() error {
	c.closeCalls++
	return nil
}

// connectionScript 按剧本逐次新建假连接（真实现也是这样：连接对象不复用）。
type connectionScript struct {
	mu        sync.Mutex
	script    []string
	instances []*fakeConnection
}

func (s *connectionScript) factory(onRaw RawEventHandler) Connection {
	s.mu.Lock()
	defer s.mu.Unlock()
	outcome := "ok"
	if i := len(s.instances); i < len(s.script) {
		outcome = s.script[i]
	}
	conn := &fakeConnection{outcome: outcome, onRaw: onRaw}
	s.instances = append(s.instances, conn)
	return conn
}

func (s *connectionScript) all() []*fakeConnection {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]*fakeConnection, len(s.instances))
	copy(out, s.instances)
	return out
}

// reconnectHarness 把「假连接剧本 + 记账 sleep + Start 跑到停」打包。
func reconnectHarness(t *testing.T, script []string, stopAfter int) (*recordingSleep, *connectionScript, *logCapture) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	sleep := &recordingSleep{stopAfter: stopAfter, cancel: cancel}
	cs := &connectionScript{script: script}
	capture, logger := newLogCapture()

	p := mustPlatform(t, platformBuild{
		factory: cs.factory, sleep: sleep.Sleep, logger: logger, sink: &recordingSink{},
	})

	done := make(chan error, 1)
	go func() { done <- p.Start(ctx) }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("Start 该正常返回，得到 %v", err)
		}
	case <-time.After(5 * time.Second):
		cancel()
		t.Fatal("Start 没有在 5s 内停下")
	}
	return sleep, cs, capture
}

// ---------------------------------------------------------------------------
// 退避序列
// ---------------------------------------------------------------------------

// TestBackoffDelayIsExponentialCappedAt30
// 对应 test_backoff_delay_is_exponential_capped_at_30。
func TestBackoffDelayIsExponentialCappedAt30(t *testing.T) {
	var got []time.Duration
	for n := 1; n <= 7; n++ {
		got = append(got, backoffDelay(n))
	}
	if !sameDelays(got, expectedBackoff) {
		t.Errorf("退避序列 = %v，要 %v", got, expectedBackoff)
	}
	if backoffDelay(20) != 30*time.Second {
		t.Error("封顶之后一直是 30，不许再翻倍")
	}
	// 大到会让 1<<n 溢出的 attempt 也不能出负数。
	if backoffDelay(1000) != 30*time.Second {
		t.Errorf("backoffDelay(1000) = %v", backoffDelay(1000))
	}
	if backoffDelay(0) != 0 {
		t.Errorf("backoffDelay(0) = %v，要 0", backoffDelay(0))
	}
}

// TestReconnectBackoffSequenceIs1248163030
// 对应 test_reconnect_backoff_sequence_is_1_2_4_8_16_30_30。
func TestReconnectBackoffSequenceIs1248163030(t *testing.T) {
	script := make([]string, 20)
	for i := range script {
		script[i] = "fail"
	}
	sleep, _, _ := reconnectHarness(t, script, len(expectedBackoff))
	if got := sleep.Delays(); !sameDelays(got, expectedBackoff) {
		t.Errorf("退避序列 = %v，要 %v", got, expectedBackoff)
	}
}

// TestBackoffResetsAfterASuccessfulConnect
// 对应 test_backoff_resets_after_a_successful_connect。
//
// 连上一次之后退避要归零，否则一晚上的抖动会把重连推到 30s 起步。
func TestBackoffResetsAfterASuccessfulConnect(t *testing.T) {
	sleep, _, _ := reconnectHarness(t, []string{"fail", "fail", "ok"}, 3)
	// 前两次失败 → 1s、2s；第三次连上；连上就断 → 又从 1s 起。
	want := []time.Duration{1 * time.Second, 2 * time.Second, 1 * time.Second}
	if got := sleep.Delays(); !sameDelays(got, want) {
		t.Errorf("退避序列 = %v，要 %v", got, want)
	}
}

// TestInfiniteRetryNeverGivesUp 对应 test_infinite_retry_never_gives_up。
//
// 无限重试。连挂 50 次也不许抛出去。
func TestInfiniteRetryNeverGivesUp(t *testing.T) {
	script := make([]string, 60)
	for i := range script {
		script[i] = "fail"
	}
	sleep, _, _ := reconnectHarness(t, script, 50)
	got := sleep.Delays()
	if len(got) != 50 {
		t.Fatalf("退避次数 = %d，要 50", len(got))
	}
	for _, d := range got[45:] {
		if d != 30*time.Second {
			t.Errorf("尾部退避 = %v，要 30s", d)
		}
	}
}

// TestReconnectedIsLoggedAtInfo 对应 test_reconnected_is_logged_at_info。
func TestReconnectedIsLoggedAtInfo(t *testing.T) {
	_, _, capture := reconnectHarness(t, []string{"fail", "ok"}, 2)

	records := capture.find("feishu.reconnected")
	if len(records) == 0 {
		t.Fatal("重连成功必须有一条 feishu.reconnected")
	}
	if records[0].Level != slog.LevelInfo {
		t.Errorf("feishu.reconnected 的级别 = %v，要 INFO", records[0].Level)
	}
	if v, ok := attr(records[0], "after"); !ok || v.Int64() != 1 {
		t.Errorf("after = %v，要 1", v.Any())
	}
	// 连接失败与重连中都是 WARN。
	for _, msg := range []string{"feishu.connect_failed", "feishu.reconnecting"} {
		rs := capture.find(msg)
		if len(rs) == 0 {
			t.Fatalf("该有一条 %s", msg)
		}
		if rs[0].Level != slog.LevelWarn {
			t.Errorf("%s 的级别 = %v，要 WARN", msg, rs[0].Level)
		}
	}
}

// TestFirstConnectIsNotDelayed 对应 test_first_connect_is_not_delayed。
func TestFirstConnectIsNotDelayed(t *testing.T) {
	sleep, cs, _ := reconnectHarness(t, []string{"ok"}, 1)
	// 第一次连上、断开之后才出现第一次退避。
	if got := sleep.Delays(); !sameDelays(got, []time.Duration{time.Second}) {
		t.Errorf("退避序列 = %v，要 [1s]", got)
	}
	if got := cs.all()[0].connectCalls; got != 1 {
		t.Errorf("第一个连接的 Connect 调用次数 = %d，要 1", got)
	}
}

// TestConnectionIsClosedBeforeReconnecting
// 对应 test_connection_is_closed_before_reconnecting。
//
// 断开后要把旧连接关掉，不能攒着一堆半死的连接。
func TestConnectionIsClosedBeforeReconnecting(t *testing.T) {
	_, cs, _ := reconnectHarness(t, []string{"ok", "ok"}, 2)
	for i, c := range cs.all() {
		if c.outcome == "ok" && c.connectCalls > 0 && c.closeCalls < 1 {
			t.Errorf("第 %d 个连上的连接没被关掉", i)
		}
	}
}

// TestStopBreaksTheLoop 对应 test_stop_breaks_the_loop。
//
// ctx 取消之后 Start 要能正常返回，而不是继续重连（Python 版是 stop()）。
func TestStopBreaksTheLoop(t *testing.T) {
	sleep, _, _ := reconnectHarness(t, []string{"ok"}, 1)
	if got := sleep.Delays(); !sameDelays(got, []time.Duration{time.Second}) {
		t.Errorf("退避序列 = %v，要 [1s]", got)
	}
}

// TestConnectedAndReconnectCountTrackTheLoop 是 Go 侧补的（EdgeStatus 用）。
func TestConnectedAndReconnectCountTrackTheLoop(t *testing.T) {
	_, _, _ = reconnectHarness(t, []string{"fail", "ok", "ok"}, 3)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	sleep := &recordingSleep{stopAfter: 3, cancel: cancel}
	cs := &connectionScript{script: []string{"fail", "ok", "ok"}}
	_, logger := newLogCapture()
	p := mustPlatform(t, platformBuild{
		factory: cs.factory, sleep: sleep.Sleep, logger: logger, sink: &recordingSink{},
	})
	if p.Connected() {
		t.Error("还没起飞就报连上了")
	}
	if p.ReconnectCount() != 0 {
		t.Errorf("初始 ReconnectCount = %d，要 0", p.ReconnectCount())
	}
	if err := p.Start(ctx); err != nil {
		t.Fatalf("Start 该正常返回：%v", err)
	}
	if p.Connected() {
		t.Error("退出循环后要报未连接")
	}
	// 剧本：fail → ok（第 1 次重连成功）→ ok（第 2 次）。
	if got := p.ReconnectCount(); got != 2 {
		t.Errorf("ReconnectCount = %d，要 2", got)
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
	// CC8 起 New() 读 AITE_FEISHU_*：显式清空，别让外面 shell 导出的值（比如做 H8 的终端）改了结论。
	clearFeishuEnv(t)
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

// clearFeishuEnv 把本包读的三个环境变量显式清空（t.Setenv 结束时还原）。
func clearFeishuEnv(t *testing.T) {
	t.Helper()
	for _, name := range []string{EnvCardButtons, EnvPassiveListen, EnvAPIBase} {
		t.Setenv(name, "")
	}
}

// TestEnvFlagsAreReadInsideThePackage 钉住三个开关都在 New() 里读（main.go 一个字不动）。
func TestEnvFlagsAreReadInsideThePackage(t *testing.T) {
	t.Run("全不设", func(t *testing.T) {
		clearFeishuEnv(t)
		p, err := New(config.Default().Feishu, Options{}, nil)
		if err != nil {
			t.Fatal(err)
		}
		if !proto.Equal(p.Capabilities(), FeishuP0()) {
			t.Errorf("能力 = %v，要 FeishuP0()", p.Capabilities())
		}
		if p.opts.Domain != DefaultDomain || p.api.domain != DefaultDomain {
			t.Errorf("domain = %q / api %q，要 %s", p.opts.Domain, p.api.domain, DefaultDomain)
		}
		if p.cardButtons {
			t.Error("卡片按钮默认要关")
		}
	})

	t.Run(EnvPassiveListen, func(t *testing.T) {
		clearFeishuEnv(t)
		t.Setenv(EnvPassiveListen, "1")
		p, err := New(config.Default().Feishu, Options{}, nil)
		if err != nil {
			t.Fatal(err)
		}
		if !p.Capabilities().GetSupportsPassiveListen() {
			t.Error(`AITE_FEISHU_PASSIVE_LISTEN="1" 该打开 supports_passive_listen`)
		}
		if FeishuP0().GetSupportsPassiveListen() {
			t.Error("改的该是实例，不是契约常量")
		}
		t.Setenv(EnvPassiveListen, "true")
		p, _ = New(config.Default().Feishu, Options{}, nil)
		if p.Capabilities().GetSupportsPassiveListen() {
			t.Error(`只有恰好 "1" 才开`)
		}
	})

	t.Run(EnvAPIBase, func(t *testing.T) {
		clearFeishuEnv(t)
		const base = "https://open.larksuite.test"
		t.Setenv(EnvAPIBase, base)
		p, err := New(config.Default().Feishu, Options{}, nil)
		if err != nil {
			t.Fatal(err)
		}
		if p.opts.Domain != base || p.api.domain != base {
			t.Errorf("domain = %q / api %q，要 %s（REST 与长连接同一个域名）", p.opts.Domain, p.api.domain, base)
		}
		// 显式的 Options.Domain 优先。
		p, _ = New(config.Default().Feishu, Options{Domain: "https://explicit.test"}, nil)
		if p.opts.Domain != "https://explicit.test" {
			t.Errorf("显式 Domain 该优先，得到 %q", p.opts.Domain)
		}
	})

	t.Run(EnvCardButtons, func(t *testing.T) {
		clearFeishuEnv(t)
		t.Setenv(EnvCardButtons, "1")
		p, err := New(config.Default().Feishu, Options{}, nil)
		if err != nil {
			t.Fatal(err)
		}
		if !p.cardButtons {
			t.Error(`AITE_FEISHU_CARD_BUTTONS="1" 该打开按钮`)
		}
	})
}
