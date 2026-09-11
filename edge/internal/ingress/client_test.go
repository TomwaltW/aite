// edge → core 的 IngressService 客户端测试：起一个假的 IngressServiceServer（gRPC over UDS），
// 验 deadline、INVALID_ARGUMENT 不重推、INTERNAL 重推、断线重连。
// 不碰 Docker，也不碰真 core。

package ingress

import (
	"context"
	"net"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	pb "aite/edge/gen/aitepb"
)

// fakeCore 是 core 侧 IngressService 的替身：只记账，回什么由 reply 决定。
type fakeCore struct {
	pb.UnimplementedIngressServiceServer

	mu    sync.Mutex
	seen  []string
	delay time.Duration
	reply error
}

func (f *fakeCore) HandleEvent(ctx context.Context, ev *pb.NormalizedEvent) (*pb.HandleEventResponse, error) {
	f.mu.Lock()
	f.seen = append(f.seen, ev.GetEventId())
	delay, reply := f.delay, f.reply
	f.mu.Unlock()

	if delay > 0 {
		select {
		case <-time.After(delay):
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
	if reply != nil {
		return nil, reply
	}
	return &pb.HandleEventResponse{}, nil
}

func (f *fakeCore) count() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.seen)
}

func (f *fakeCore) set(delay time.Duration, reply error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.delay, f.reply = delay, reply
}

// socketPath 给测试一个短路径：macOS 的 unix socket 路径上限 104 字节，
// t.TempDir() 那串 /var/folders/... 再套一层就容易顶满。
func socketPath(t *testing.T, name string) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "aite-ing")
	if err != nil {
		t.Fatalf("MkdirTemp 失败：%v", err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	p := filepath.Join(dir, name)
	if len(p) > 100 {
		t.Skipf("socket 路径太长（%d 字节）：%s", len(p), p)
	}
	return p
}

// serveFakeCore 在 sock 上起假 core，返回停它的函数（可重复调）。
func serveFakeCore(t *testing.T, sock string, core *fakeCore) func() {
	t.Helper()
	_ = os.Remove(sock)
	lis, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatalf("listen %s 失败：%v", sock, err)
	}
	srv := grpc.NewServer()
	pb.RegisterIngressServiceServer(srv, core)
	go func() { _ = srv.Serve(lis) }()

	var once sync.Once
	stop := func() { once.Do(srv.Stop) }
	t.Cleanup(stop)
	return stop
}

func event(id string) *pb.NormalizedEvent {
	return &pb.NormalizedEvent{EventId: id, Kind: pb.EventKind_EVENT_KIND_MESSAGE, Platform: "feishu"}
}

// newClient 是带小退避的客户端：默认 1s→30s 在测试里太慢。
func newClient(t *testing.T, sock string, deadline time.Duration) *Client {
	t.Helper()
	c := New(sock, deadline, 64)
	c.baseDelay = 20 * time.Millisecond
	c.maxDelay = 100 * time.Millisecond
	t.Cleanup(func() { _ = c.Close() })
	return c
}

// core 正常返回：计 events.sent，不计失败。
func TestHandleEventCountsSuccess(t *testing.T) {
	sock := socketPath(t, "c.sock")
	core := &fakeCore{}
	serveFakeCore(t, sock, core)
	c := newClient(t, sock, time.Second)

	if err := c.HandleEvent(context.Background(), event("ev-1")); err != nil {
		t.Fatalf("HandleEvent 失败：%v", err)
	}
	if got := c.Counters(); got.Sent != 1 || got.Errors != 0 || got.Invalid != 0 {
		t.Fatalf("计数 = %+v，想要 Sent=1", got)
	}
	if core.count() != 1 {
		t.Fatalf("core 收到 %d 条，想要 1", core.count())
	}
	if !c.Connected() {
		t.Fatal("调通了却报没连上")
	}
}

// core 超过 deadline 没返回 → DEADLINE_EXCEEDED，计 ingress.errors 并让平台重推。
func TestHandleEventDeadline(t *testing.T) {
	sock := socketPath(t, "c.sock")
	core := &fakeCore{}
	serveFakeCore(t, sock, core)
	c := newClient(t, sock, 100*time.Millisecond)
	core.set(2*time.Second, nil)

	started := time.Now()
	err := c.HandleEvent(context.Background(), event("ev-slow"))
	elapsed := time.Since(started)

	if err == nil {
		t.Fatal("超过 deadline 却返回 nil —— 平台不会重推了")
	}
	if code := status.Code(err); code != codes.DeadlineExceeded {
		t.Fatalf("status code = %v，想要 DeadlineExceeded", code)
	}
	if elapsed > time.Second {
		t.Fatalf("等了 %v 才失败，deadline 是 100ms —— 没生效", elapsed)
	}
	if got := c.Counters(); got.Errors != 1 || got.Sent != 0 {
		t.Fatalf("计数 = %+v，想要 Errors=1", got)
	}
}

// INVALID_ARGUMENT 是 core 说事件非法：计 ingress.invalid，返回 nil 让 SDK 确认，不重推。
func TestHandleEventInvalidArgumentIsNotRetried(t *testing.T) {
	sock := socketPath(t, "c.sock")
	core := &fakeCore{}
	serveFakeCore(t, sock, core)
	c := newClient(t, sock, time.Second)
	core.set(0, status.Error(codes.InvalidArgument, "invalid_event: anchor 缺失"))

	if err := c.HandleEvent(context.Background(), event("ev-bad")); err != nil {
		t.Fatalf("非法事件应当返回 nil（重推多少次都还是非法），得到 %v", err)
	}
	if got := c.Counters(); got.Invalid != 1 || got.Errors != 0 || got.Sent != 0 {
		t.Fatalf("计数 = %+v，想要 Invalid=1", got)
	}
}

// INTERNAL 是 core 自己没处理好：计 ingress.errors，返回 error 让平台重推。
func TestHandleEventInternalIsRetried(t *testing.T) {
	sock := socketPath(t, "c.sock")
	core := &fakeCore{}
	serveFakeCore(t, sock, core)
	c := newClient(t, sock, time.Second)
	core.set(0, status.Error(codes.Internal, "ingress_failed: 存储抖了一下"))

	err := c.HandleEvent(context.Background(), event("ev-boom"))
	if err == nil {
		t.Fatal("core 处理失败却返回 nil —— 这条事件就丢了")
	}
	if code := status.Code(err); code != codes.Internal {
		t.Fatalf("status code = %v，想要 Internal", code)
	}
	if got := c.Counters(); got.Errors != 1 || got.Invalid != 0 {
		t.Fatalf("计数 = %+v，想要 Errors=1", got)
	}
}

// core 还没起来时 edge 不该退出：第一次调用失败，core 起来之后自己连上。
func TestHandleEventBeforeCoreIsUp(t *testing.T) {
	sock := socketPath(t, "c.sock")
	c := newClient(t, sock, 300*time.Millisecond)

	if err := c.HandleEvent(context.Background(), event("ev-early")); err == nil {
		t.Fatal("core 不在却成功了")
	}
	if got := c.Counters(); got.Errors != 1 {
		t.Fatalf("计数 = %+v，想要 Errors=1", got)
	}

	core := &fakeCore{}
	serveFakeCore(t, sock, core)
	if err := retryUntilOK(c, "ev-late"); err != nil {
		t.Fatalf("core 起来之后还是连不上：%v", err)
	}
}

// core 重启：连接断掉之后按退避重连，重连成功要计 ingress.reconnects。
func TestReconnectsAfterCoreRestart(t *testing.T) {
	sock := socketPath(t, "c.sock")
	core := &fakeCore{}
	stop := serveFakeCore(t, sock, core)
	c := newClient(t, sock, 300*time.Millisecond)

	if err := c.HandleEvent(context.Background(), event("ev-1")); err != nil {
		t.Fatalf("第一次就没通：%v", err)
	}

	stop()
	if err := waitFor(2*time.Second, func() bool { return !c.Connected() }); err != nil {
		t.Fatalf("core 停了却还报着连上：%v", err)
	}
	if err := c.HandleEvent(context.Background(), event("ev-2")); err == nil {
		t.Fatal("core 停了却成功了")
	}

	core2 := &fakeCore{}
	serveFakeCore(t, sock, core2)
	if err := retryUntilOK(c, "ev-3"); err != nil {
		t.Fatalf("core 重启之后没连回来：%v", err)
	}
	if got := c.Counters(); got.Reconnects < 1 {
		t.Fatalf("计数 = %+v，想要 Reconnects>=1", got)
	}
	if !c.Connected() {
		t.Fatal("连回来了却报没连上")
	}
	if core2.count() != 1 {
		t.Fatalf("新 core 收到 %d 条，想要 1", core2.count())
	}
}

// Close 之后连接状态要归零（EdgeStatus 不能还报着 connected）。
func TestCloseResetsConnected(t *testing.T) {
	sock := socketPath(t, "c.sock")
	core := &fakeCore{}
	serveFakeCore(t, sock, core)
	c := New(sock, time.Second, 64)

	if err := c.HandleEvent(context.Background(), event("ev-1")); err != nil {
		t.Fatalf("HandleEvent 失败：%v", err)
	}
	if err := c.Close(); err != nil {
		t.Fatalf("Close 失败：%v", err)
	}
	if c.Connected() {
		t.Fatal("Close 之后还报着 connected")
	}
	if err := c.Close(); err != nil {
		t.Fatalf("Close 该幂等：%v", err)
	}
}

// ------------------------------------------------------------------ 小工具

func retryUntilOK(c *Client, id string) error {
	deadline := time.Now().Add(5 * time.Second)
	var last error
	for time.Now().Before(deadline) {
		if err := c.HandleEvent(context.Background(), event(id)); err == nil {
			return nil
		} else {
			last = err
		}
		time.Sleep(50 * time.Millisecond)
	}
	return last
}

func waitFor(d time.Duration, cond func() bool) error {
	deadline := time.Now().Add(d)
	for time.Now().Before(deadline) {
		if cond() {
			return nil
		}
		time.Sleep(20 * time.Millisecond)
	}
	return context.DeadlineExceeded
}
