package server

import (
	"fmt"
	"net"
	"os"
	"path/filepath"

	"google.golang.org/grpc"
	"google.golang.org/grpc/health"
	healthpb "google.golang.org/grpc/health/grpc_health_v1"

	pb "aite/edge/gen/aitepb"
)

// ContractVersion 必须与 core/crates/contracts/src/lib.rs 的 CONTRACT_VERSION 一致；
// core 起飞时比对 EdgeStatus.contract_version，不等就拒绝起飞。
const ContractVersion = "p0.2"

// ListenUnix 在 path 上监听（先清掉残留的 socket 文件，建好父目录）。
func ListenUnix(path string) (net.Listener, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, fmt.Errorf("mkdir for socket: %w", err)
	}
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return nil, fmt.Errorf("remove stale socket: %w", err)
	}
	return net.Listen("unix", path)
}

// New 组装 gRPC server：三个业务服务 + 标准 health。
func New(platform PlatformPort, sandbox SandboxPort, status StatusSource, maxMessageMB int) *grpc.Server {
	max := maxMessageMB * 1024 * 1024
	srv := grpc.NewServer(grpc.MaxRecvMsgSize(max), grpc.MaxSendMsgSize(max))
	pb.RegisterPlatformServiceServer(srv, NewPlatformService(platform))
	pb.RegisterSandboxServiceServer(srv, NewSandboxService(sandbox))
	pb.RegisterEdgeStatusServiceServer(srv, NewStatusService(status))
	h := health.NewServer()
	h.SetServingStatus("", healthpb.HealthCheckResponse_SERVING)
	healthpb.RegisterHealthServer(srv, h)
	return srv
}
