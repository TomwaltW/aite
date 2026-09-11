// 对应 aite/adapters/feishu/ratelimit.py。
//
// 出站限速（capabilities.proto 的 outbound_rate_per_min，「adapter 自己令牌桶」）。
// 时钟和 sleep 都可注入：测试要断言「第 N 次调用被挡下来了」，不能真睡一分钟。
package feishu

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"
)

// clockFunc 返回当前时刻；令牌桶只用差值，所以单调与否只影响精度不影响正确性。
type clockFunc func() time.Time

// sleeperFunc 是可注入的 sleep。返回非 nil 表示被 ctx 取消（调用方要一路抬出去）。
type sleeperFunc func(ctx context.Context, d time.Duration) error

// realSleep 是默认实现：可被 ctx 取消的 sleep。
func realSleep(ctx context.Context, d time.Duration) error {
	if d <= 0 {
		return ctx.Err()
	}
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

// TokenBucket 是每分钟 ratePerMin 个令牌的匀速令牌桶。
//
// 桶容量默认等于每分钟额度：空闲一分钟后允许一次突发，之后按 60/rate 的间隔匀速放行。
type TokenBucket struct {
	RatePerMin int
	Capacity   int

	perSec  float64
	clock   clockFunc
	sleep   sleeperFunc
	mu      sync.Mutex
	tokens  float64
	updated time.Time
}

// NewTokenBucket 建一个令牌桶；capacity <= 0 表示取默认值（= ratePerMin）。
func NewTokenBucket(ratePerMin int, capacity int, clock clockFunc, sleep sleeperFunc) (*TokenBucket, error) {
	if ratePerMin <= 0 {
		return nil, fmt.Errorf("rate_per_min 必须为正：%d", ratePerMin)
	}
	if capacity <= 0 {
		capacity = ratePerMin
	}
	if clock == nil {
		clock = time.Now
	}
	if sleep == nil {
		sleep = realSleep
	}
	return &TokenBucket{
		RatePerMin: ratePerMin,
		Capacity:   capacity,
		perSec:     float64(ratePerMin) / 60.0,
		clock:      clock,
		sleep:      sleep,
		tokens:     float64(capacity),
		updated:    clock(),
	}, nil
}

// refill 必须在持锁状态下调用。
func (b *TokenBucket) refill() {
	now := b.clock()
	elapsed := now.Sub(b.updated).Seconds()
	if elapsed > 0 {
		b.tokens = min(float64(b.Capacity), b.tokens+elapsed*b.perSec)
		b.updated = now
	}
}

// Tokens 报告当前可用令牌数（会先补一次）。只给测试和排障看。
func (b *TokenBucket) Tokens() float64 {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.refill()
	return b.tokens
}

// errTokensExceedCapacity 是一次要的令牌数超过桶容量。
var errTokensExceedCapacity = errors.New("一次要的令牌数超过桶容量")

// Acquire 取 tokens 个令牌，不够就等到够。返回实际等待的时长。
//
// 持锁期间 sleep：等待者串行，与 Python 版一致（否则多个等待者会一起抢同一批令牌）。
func (b *TokenBucket) Acquire(ctx context.Context, tokens int) (time.Duration, error) {
	if tokens > b.Capacity {
		return 0, fmt.Errorf("%w：一次要 %d 个令牌，超过桶容量 %d", errTokensExceedCapacity, tokens, b.Capacity)
	}
	var waited time.Duration
	b.mu.Lock()
	defer b.mu.Unlock()
	for {
		b.refill()
		if b.tokens >= float64(tokens) {
			b.tokens -= float64(tokens)
			return waited, nil
		}
		deficit := float64(tokens) - b.tokens
		delay := time.Duration(deficit / b.perSec * float64(time.Second))
		waited += delay
		if err := b.sleep(ctx, delay); err != nil {
			return waited, err
		}
	}
}
