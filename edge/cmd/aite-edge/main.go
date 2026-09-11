// aite-edge —— Aite 的对外连接进程（Go）：飞书长连接 + Docker 沙箱 + gRPC 服务面。
//
// owner: R2（R0 放下能起飞的骨架）。职责边界：
//   - 只做「对外连接」：飞书 WS/REST、Docker daemon；不做任何路由/会话/任务判定
//   - 收到平台事件 → 归一化 → IngressService.HandleEvent(core)，1s deadline
//   - 提供 PlatformService / SandboxService / EdgeStatusService 给 core 调用
//   - SIGTERM/SIGINT：先 GracefulStop gRPC，再关长连接，退出码 0
package main

import (
	"context"
	"flag"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"syscall"
	"time"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/config"
	"aite/edge/internal/feishu"
	"aite/edge/internal/ingress"
	"aite/edge/internal/sandbox"
	"aite/edge/internal/server"
)

const version = "0.0.1"

type statusSource struct {
	cfg      config.Config
	platform *feishu.Platform
	docker   *sandbox.Docker
}

func (s *statusSource) Status(ctx context.Context) *pb.EdgeStatus {
	st := &pb.EdgeStatus{
		Version:         version,
		ContractVersion: server.ContractVersion,
		Platform:        s.cfg.Platform,
	}
	if s.platform != nil {
		st.PlatformConnected = s.platform.Connected()
		st.ReconnectCount = s.platform.ReconnectCount()
	}
	if s.docker != nil {
		pctx, cancel := context.WithTimeout(ctx, 2*time.Second)
		defer cancel()
		st.SandboxOk = s.docker.Ping(pctx) == nil
	}
	return st
}

func run() error {
	cfgPath := flag.String("config", "config/aite.yaml", "配置文件路径（相对仓库根）")
	flag.Parse()

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo}))
	slog.SetDefault(logger)

	cfg, err := config.Load(*cfgPath)
	if err != nil {
		return err
	}

	sink := ingress.New(cfg.Edge.CoreSocket, time.Duration(cfg.Edge.HandleEventDeadlineMS)*time.Millisecond, cfg.Edge.MaxMessageMB)
	defer func() { _ = sink.Close() }()

	platform, err := feishu.New(cfg.Feishu, feishu.Options{
		AppID:     os.Getenv(cfg.Feishu.AppIDEnv),
		AppSecret: os.Getenv(cfg.Feishu.AppSecretEnv),
		BotOpenID: os.Getenv(cfg.Feishu.BotOpenIDEnv),
		TenantID:  cfg.TenantID,
	}, sink)
	if err != nil {
		return fmt.Errorf("feishu: %w", err)
	}
	docker, err := sandbox.NewDocker(cfg.Sandbox)
	if err != nil {
		return fmt.Errorf("sandbox: %w", err)
	}

	lis, err := server.ListenUnix(cfg.Edge.EdgeSocket)
	if err != nil {
		return err
	}
	srv := server.New(platform, docker, &statusSource{cfg: cfg, platform: platform, docker: docker}, cfg.Edge.MaxMessageMB)

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	errc := make(chan error, 2)
	go func() {
		slog.Info("edge.grpc_listening", "socket", cfg.Edge.EdgeSocket, "contract_version", server.ContractVersion)
		errc <- srv.Serve(lis)
	}()
	if cfg.Platform == "feishu" {
		go func() {
			err := platform.Start(ctx)
			if err != nil && ctx.Err() == nil {
				errc <- fmt.Errorf("feishu.start: %w", err)
				return
			}
			errc <- nil
		}()
	} else {
		slog.Info("edge.platform_fake", "note", "platform=fake：不起飞书长连接，只提供 gRPC 服务面")
	}

	select {
	case <-ctx.Done():
		slog.Info("edge.shutting_down")
	case err := <-errc:
		if err != nil {
			slog.Error("edge.component_failed", "err", err)
			srv.Stop()
			return err
		}
	}
	done := make(chan struct{})
	go func() { srv.GracefulStop(); close(done) }()
	select {
	case <-done:
	case <-time.After(10 * time.Second):
		srv.Stop()
	}
	return nil
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "aite-edge:", err)
		os.Exit(1)
	}
}
