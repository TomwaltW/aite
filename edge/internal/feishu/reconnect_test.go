// 对应 tests/adapters/feishu/test_feishu_reconnect.py。
//
// 长连接与重连。判据原文：重连策略对假连接对象的退避序列断言为 [1,2,4,8,16,30,30]。
//
// 假连接对象在这个文件里自建。这里也顺带把「adapter 不去重」
// 「HandleEvent 失败不能带走长连接」两条钉住。
package feishu

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sync"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
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

// recordingSink 是 EventSink 的假实现。
type recordingSink struct {
	mu     sync.Mutex
	events []*pb.NormalizedEvent
	err    error
	before func()
}

func (s *recordingSink) HandleEvent(_ context.Context, ev *pb.NormalizedEvent) error {
	s.mu.Lock()
	before := s.before
	err := s.err
	s.events = append(s.events, ev)
	s.mu.Unlock()
	if before != nil {
		before()
	}
	return err
}

func (s *recordingSink) seen() []*pb.NormalizedEvent {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]*pb.NormalizedEvent, len(s.events))
	copy(out, s.events)
	return out
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
// 事件投递
// ---------------------------------------------------------------------------

func dispatchPlatform(t *testing.T, sink *recordingSink, budget time.Duration, clock *fakeClock) (*Platform, *logCapture) {
	t.Helper()
	capture, logger := newLogCapture()
	b := platformBuild{sink: sink, logger: logger, budget: budget, appID: testAppID}
	if clock != nil {
		b.clock = clock.Now
		b.sleep = clock.Sleep
	}
	return mustPlatform(t, b), capture
}

// TestRawEventsAreNormalizedAndDelivered
// 对应 test_raw_events_are_normalized_and_delivered。
func TestRawEventsAreNormalizedAndDelivered(t *testing.T) {
	sink := &recordingSink{}
	p, _ := dispatchPlatform(t, sink, 0, nil)

	if err := p.dispatchRaw(context.Background(),
		loadFixture(t, "message_at_bot_toplevel", ".json")); err != nil {
		t.Fatalf("dispatchRaw 失败：%v", err)
	}

	seen := sink.seen()
	if len(seen) != 1 {
		t.Fatalf("事件数 = %d，要 1", len(seen))
	}
	if got := seen[0].GetText(); got != "把这个季度的销售数据画成趋势图" {
		t.Errorf("text = %q", got)
	}
	if !seen[0].GetMentioned() {
		t.Error("mentioned 要是 true")
	}
	if got := seen[0].GetWorkspaceId(); got != testAppID {
		t.Errorf("workspace_id = %q", got)
	}
}

// TestAdapterDoesNotDeduplicateReplayedEvents
// 对应 test_adapter_does_not_deduplicate_replayed_events。
//
// 重连后平台重推的重复事件由 core 靠 event_id 去重，adapter 不管。
func TestAdapterDoesNotDeduplicateReplayedEvents(t *testing.T) {
	sink := &recordingSink{}
	p, _ := dispatchPlatform(t, sink, 0, nil)

	raw := loadFixture(t, "message_at_bot_toplevel", ".json")
	for i := 0; i < 2; i++ {
		if err := p.dispatchRaw(context.Background(), raw); err != nil {
			t.Fatalf("第 %d 次 dispatchRaw 失败：%v", i+1, err)
		}
	}

	seen := sink.seen()
	if len(seen) != 2 {
		t.Fatalf("事件数 = %d，要 2（adapter 私自去重的话 core 的 events.duplicate 就永远是 0）", len(seen))
	}
	if seen[0].GetEventId() != seen[1].GetEventId() {
		t.Error("两条的 event_id 该相同")
	}
}

// TestUnsubscribedEventIsDroppedWithoutCallingHandler
// 对应 test_unsubscribed_event_is_dropped_without_calling_handler。
func TestUnsubscribedEventIsDroppedWithoutCallingHandler(t *testing.T) {
	sink := &recordingSink{}
	p, capture := dispatchPlatform(t, sink, 0, nil)

	raw := loadFixture(t, "message_at_bot_toplevel", ".json")
	asMap(raw["header"])["event_type"] = "im.chat.member.user.added_v1"
	if err := p.dispatchRaw(context.Background(), raw); err != nil {
		t.Fatalf("没订阅的事件不该报错：%v", err)
	}

	if got := sink.seen(); len(got) != 0 {
		t.Errorf("没订阅的事件不该进 sink，得到 %d 条", len(got))
	}
	if !capture.has("feishu.event_ignored") {
		t.Error("该打一条 feishu.event_ignored")
	}
}

// TestHandlerFailureDoesNotKillTheConnection
// 对应 test_handler_exception_does_not_kill_the_connection。
//
// 与 Python 的一处有意差异：Python 把回调异常吞掉只打日志；Go 版把 error 返回给
// SDK 让平台重推（spec §2.1）。「不带走长连接」这条不变 —— Start 的循环照转。
func TestHandlerFailureDoesNotKillTheConnection(t *testing.T) {
	boom := errors.New("上游炸了")
	sink := &recordingSink{err: boom}
	p, capture := dispatchPlatform(t, sink, 0, nil)

	err := p.dispatchRaw(context.Background(), loadFixture(t, "message_at_bot_toplevel", ".json"))
	if !errors.Is(err, boom) {
		t.Errorf("HandleEvent 失败要一路返回给 SDK（让平台重推），得到 %v", err)
	}

	records := capture.find("feishu.on_event_failed")
	if len(records) != 1 {
		t.Fatalf("该打一条 feishu.on_event_failed，得到 %d 条", len(records))
	}
	if records[0].Level != slog.LevelError {
		t.Errorf("feishu.on_event_failed 的级别 = %v，要 ERROR", records[0].Level)
	}
	if v, ok := attr(records[0], "event_id"); !ok || v.String() != "evt_at_bot_toplevel_0001" {
		t.Errorf("event_id = %v", v.Any())
	}

	// 长连接不受影响：投递一路失败，Start 的重连循环照转到 ctx 取消为止。
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	sleep := &recordingSleep{stopAfter: 3, cancel: cancel}
	cs := &connectionScript{script: []string{"ok", "ok", "ok"}}
	_, logger := newLogCapture()
	loopP := mustPlatform(t, platformBuild{
		factory: cs.factory, sleep: sleep.Sleep, logger: logger, sink: sink, appID: testAppID,
	})
	raw := loadFixture(t, "message_at_bot_toplevel", ".json")
	sleep.onSleep = func(int) {
		for _, c := range cs.all() {
			if c.onRaw != nil {
				_ = c.onRaw(context.Background(), raw)
			}
		}
	}
	if err := loopP.Start(ctx); err != nil {
		t.Fatalf("投递一直失败也不该让 Start 抛出去：%v", err)
	}
	if len(sink.seen()) < 3 {
		t.Errorf("投递该一直在发生，得到 %d 条", len(sink.seen()))
	}
}

// TestSlowHandlerIsReported 对应 test_slow_handler_is_reported。
//
// HandleEvent 必须 1s 内返回。超了不拦（拦了会丢事件），但要吼一声。
func TestSlowHandlerIsReported(t *testing.T) {
	clock := newFakeClock()
	sink := &recordingSink{before: func() { clock.Advance(2 * time.Second) }}
	p, capture := dispatchPlatform(t, sink, time.Second, clock)

	if err := p.dispatchRaw(context.Background(),
		loadFixture(t, "message_at_bot_toplevel", ".json")); err != nil {
		t.Fatalf("慢不等于失败：%v", err)
	}

	records := capture.find("feishu.on_event_slow")
	if len(records) != 1 {
		t.Fatalf("该打一条 feishu.on_event_slow，得到 %d 条", len(records))
	}
	if records[0].Level != slog.LevelWarn {
		t.Errorf("feishu.on_event_slow 的级别 = %v，要 WARN", records[0].Level)
	}
	if v, ok := attr(records[0], "elapsed_sec"); !ok || v.Float64() != 2 {
		t.Errorf("elapsed_sec = %v，要 2", v.Any())
	}
	if v, ok := attr(records[0], "budget_sec"); !ok || v.Float64() != 1 {
		t.Errorf("budget_sec = %v，要 1", v.Any())
	}

	// 没超时就不打。
	clock2 := newFakeClock()
	sink2 := &recordingSink{}
	p2, capture2 := dispatchPlatform(t, sink2, time.Second, clock2)
	if err := p2.dispatchRaw(context.Background(),
		loadFixture(t, "message_at_bot_toplevel", ".json")); err != nil {
		t.Fatal(err)
	}
	if capture2.has("feishu.on_event_slow") {
		t.Error("没超预算不该打 feishu.on_event_slow")
	}
}
