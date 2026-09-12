// 连接态与生命周期的三条（RΩ 补，审核记账 R2）：
//
//  1. 第一次连上不算重连 —— core 晚起来是常态，那一路必然先 TransientFailure；
//  2. 一次成功的 RPC 就是「现在连得上」，不许等 watch 的状态迁移；
//  3. Close() 之后是终态，再投事件不许把连接和 watch goroutine 复活。
//
// 与 client_test.go 共用同包的 fakeCore / socketPath / serveFakeCore / newClient。
package ingress

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestFirstConnectAfterWaitingIsNotCountedAsReconnect(t *testing.T) {
	sock := socketPath(t, "first.sock")
	c := newClient(t, sock, 500*time.Millisecond)

	// core 还没起：这一发必然失败，并把连接推进 TransientFailure
	if err := c.HandleEvent(context.Background(), event("e0")); err == nil {
		t.Fatal("core 没起时该报错让平台重推")
	}

	core := &fakeCore{}
	serveFakeCore(t, sock, core)

	var lastErr error
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		lastErr = c.HandleEvent(context.Background(), event("e1"))
		if lastErr == nil {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if lastErr != nil {
		t.Fatalf("core 起来之后该连得上：%v", lastErr)
	}
	if got := c.Counters().Reconnects; got != 0 {
		t.Fatalf("第一次连上不该算重连，实际 reconnects=%d", got)
	}
	if !c.Connected() {
		t.Fatal("RPC 都跑通了，Connected() 还报 false")
	}
}

func TestConnectedIsTrueRightAfterASuccessfulRpc(t *testing.T) {
	sock := socketPath(t, "noteok.sock")
	core := &fakeCore{}
	serveFakeCore(t, sock, core)
	c := newClient(t, sock, time.Second)

	if err := c.HandleEvent(context.Background(), event("e1")); err != nil {
		t.Fatalf("该成功：%v", err)
	}
	// 不 sleep、不等 watch：这一行就是判据
	if !c.Connected() {
		t.Fatal("RPC 成功之后 Connected() 必须立刻是 true")
	}
}

func TestHandleEventAfterCloseDoesNotResurrectTheConnection(t *testing.T) {
	sock := socketPath(t, "closed.sock")
	core := &fakeCore{}
	serveFakeCore(t, sock, core)
	c := New(sock, time.Second, 64)
	c.baseDelay = 20 * time.Millisecond
	c.maxDelay = 100 * time.Millisecond

	if err := c.HandleEvent(context.Background(), event("e1")); err != nil {
		t.Fatalf("关之前该成功：%v", err)
	}
	if err := c.Close(); err != nil {
		t.Fatalf("Close 失败：%v", err)
	}

	before := core.count()
	err := c.HandleEvent(context.Background(), event("e2"))
	if err == nil {
		t.Fatal("关掉之后还能投事件 —— 连接被复活了")
	}
	if !errors.Is(err, ErrClosed) {
		t.Fatalf("该是 ErrClosed，实际 %v", err)
	}
	if core.count() != before {
		t.Fatalf("事件真的送到 core 了：%d → %d", before, core.count())
	}
	if c.Connected() {
		t.Fatal("关掉之后 Connected() 该是 false")
	}
	// 这条事件没送到，按「让平台重推」计账
	if c.Counters().Errors == 0 {
		t.Fatal("没送到的事件该计 ingress.errors")
	}
	// 幂等：再关一次不许炸
	if err := c.Close(); err != nil {
		t.Fatalf("重复 Close 该幂等：%v", err)
	}
}
