// Package server 把 Go 侧的两个端口接口暴露成 gRPC 服务（proto/aite/v1/edge.proto）。
//
// 接口直接用 pb 类型：edge 内部不再维护第二套 domain struct，归一化的产物就是 pb。
// 实现者：PlatformPort = internal/feishu（R1），SandboxPort = internal/sandbox（R2）。
// 测试替身各自在自己的包里写。owner: R0；本包只读，需要加方法 → 停下报告。
package server

import (
	"context"

	pb "aite/edge/gen/aitepb"
)

// PlatformPort 对应 proto PlatformService（旧 PlatformPort 去掉 start/stop）。
// 所有方法只返回 *aiteerr.PlatformError 或 nil；其他 error 会被映射成 INTERNAL。
type PlatformPort interface {
	Capabilities() *pb.PlatformCapabilities
	SendText(ctx context.Context, msg *pb.OutboundText) (*pb.SendResult, error)
	SendCard(ctx context.Context, chatID string, replyTo *string, card *pb.ChecklistCard) (*pb.SendResult, error)
	UpdateCard(ctx context.Context, cardID string, card *pb.ChecklistCard) error
	SendFile(ctx context.Context, msg *pb.OutboundFile) (*pb.SendResult, error)
	AddReaction(ctx context.Context, messageID string, kind pb.ReactionKind) error
	ReadHistory(ctx context.Context, chatID string, limit int, threadID *string) ([]*pb.HistoryMessage, error)
	ReadDocument(ctx context.Context, urlOrToken string) (*pb.DocumentContent, error)
	DownloadFile(ctx context.Context, messageID, fileKey string) ([]byte, error)
}

// SandboxPort 对应 proto SandboxService。
// 所有方法只返回 *aiteerr.SandboxError 或 nil；其他 error 会被映射成 INTERNAL。
type SandboxPort interface {
	Acquire(ctx context.Context, taskID string, spec *pb.SandboxSpec) (string, error)
	Exec(ctx context.Context, sandboxID string, req *pb.ExecRequest) (*pb.ExecResult, error)
	PutFile(ctx context.Context, sandboxID, path string, data []byte) error
	GetFile(ctx context.Context, sandboxID, path string) ([]byte, error)
	ListFiles(ctx context.Context, sandboxID string) ([]string, error)
	Touch(ctx context.Context, sandboxID string) error
	Release(ctx context.Context, sandboxID string) error
	ReapIdle(ctx context.Context, idleSec int) ([]string, error)
}

// StatusSource 给 EdgeStatusService 供数：feishu 连接状态 + docker 可达性。
type StatusSource interface {
	Status(ctx context.Context) *pb.EdgeStatus
}
