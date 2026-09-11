// Package ingress 是 edge → core 的 IngressService 客户端（proto/aite/v1/edge.proto）。
//
// owner: R2。R0 放下能用的最小版本：懒连接 + 每次调用带 deadline。
// R2 要补：core 不在时的退避重连日志、失败计数 ingress.errors、连接态给 EdgeStatus。
package ingress

import (
	"context"
	"net"
	"sync"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"

	pb "aite/edge/gen/aitepb"
)

type Client struct {
	socket   string
	deadline time.Duration
	maxMsg   int

	mu   sync.Mutex
	conn *grpc.ClientConn
}

func New(coreSocket string, deadline time.Duration, maxMessageMB int) *Client {
	return &Client{socket: coreSocket, deadline: deadline, maxMsg: maxMessageMB * 1024 * 1024}
}

func (c *Client) client() (pb.IngressServiceClient, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.conn == nil {
		sock := c.socket
		conn, err := grpc.NewClient("unix:"+sock,
			grpc.WithTransportCredentials(insecure.NewCredentials()),
			grpc.WithDefaultCallOptions(grpc.MaxCallRecvMsgSize(c.maxMsg), grpc.MaxCallSendMsgSize(c.maxMsg)),
			grpc.WithContextDialer(func(ctx context.Context, _ string) (net.Conn, error) {
				var d net.Dialer
				return d.DialContext(ctx, "unix", sock)
			}))
		if err != nil {
			return nil, err
		}
		c.conn = conn
	}
	return pb.NewIngressServiceClient(c.conn), nil
}

// HandleEvent 把一条归一化事件送给 core；超过 deadline 即失败（core 必须 1s 内返回）。
func (c *Client) HandleEvent(ctx context.Context, ev *pb.NormalizedEvent) error {
	cl, err := c.client()
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(ctx, c.deadline)
	defer cancel()
	_, err = cl.HandleEvent(ctx, ev)
	return err
}

func (c *Client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.conn != nil {
		err := c.conn.Close()
		c.conn = nil
		return err
	}
	return nil
}
