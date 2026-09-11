// Package sandbox 是 SandboxPort 的 Docker 实现（对应旧 aite/sandbox/**）。
//
// owner: R2。R0 只放下骨架：构造函数签名 + 全部方法返回 ErrNotImplemented。
// 容器一律打标签 aite.task=<task_id>，network=none，非 root（uid 1000），工作目录 /work。
package sandbox

import (
	"context"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
	"aite/edge/internal/config"
)

// Docker 实现 server.SandboxPort。
type Docker struct {
	cfg config.Sandbox
}

// NewDocker 只做装配（含 docker client 构造），不碰 daemon；Ping 才探活。
func NewDocker(cfg config.Sandbox) (*Docker, error) {
	return &Docker{cfg: cfg}, nil
}

// Ping 探 docker daemon 是否可达（EdgeStatus.sandbox_ok / preflight 用）。
func (d *Docker) Ping(ctx context.Context) error { return aiteerr.ErrNotImplemented }

func (d *Docker) Acquire(context.Context, string, *pb.SandboxSpec) (string, error) {
	return "", aiteerr.ErrNotImplemented
}

func (d *Docker) Exec(context.Context, string, *pb.ExecRequest) (*pb.ExecResult, error) {
	return nil, aiteerr.ErrNotImplemented
}

func (d *Docker) PutFile(context.Context, string, string, []byte) error {
	return aiteerr.ErrNotImplemented
}

func (d *Docker) GetFile(context.Context, string, string) ([]byte, error) {
	return nil, aiteerr.ErrNotImplemented
}

func (d *Docker) ListFiles(context.Context, string) ([]string, error) {
	return nil, aiteerr.ErrNotImplemented
}

func (d *Docker) Touch(context.Context, string) error { return aiteerr.ErrNotImplemented }

func (d *Docker) Release(context.Context, string) error { return aiteerr.ErrNotImplemented }

func (d *Docker) ReapIdle(context.Context, int) ([]string, error) {
	return nil, aiteerr.ErrNotImplemented
}
