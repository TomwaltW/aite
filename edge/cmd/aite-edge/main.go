// aite-edge —— Aite 的对外连接进程（Go）：飞书长连接 + Docker 沙箱 + gRPC 服务面。
//
// owner: R2。职责边界：
//   - 只做「对外连接」：飞书 WS/REST、Docker daemon；不做任何路由/会话/任务判定
//   - 收到平台事件 → 归一化 → IngressService.HandleEvent(core)，1s deadline
//   - 提供 PlatformService / SandboxService / EdgeStatusService 给 core 调用
//   - SIGTERM/SIGINT：第一次优雅（GracefulStop → 关长连接 → 还沙箱），退出码 0；
//     第二次硬退 130
//
// 收尾序列对位 Python 版 aite/app.py 的 `_shutdown`（移植清单 inventory-core §7）：
// 先停投递面，再等在跑的善终，最后还沙箱 —— 顺序是冻结的，别调。
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"syscall"
	"time"

	"google.golang.org/grpc"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/config"
	"aite/edge/internal/feishu"
	"aite/edge/internal/ingress"
	"aite/edge/internal/sandbox"
	"aite/edge/internal/server"
)

const version = "0.0.1"

// gRPC 优雅停的上限：超了就硬停，不能让收尾无限等一个卡住的调用。
const gracefulStopTimeout = 10 * time.Second

// 长连接收尾的上限（platform.Start 收到 ctx 取消之后该自己返回）。
const platformStopTimeout = 5 * time.Second

// 硬退出码：与 Python 版 app.py 第二次信号的 os._exit(130) 一致。
const exitHardStop = 130

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
	// platform: fake 时压根没起长连接，connected 恒 false —— 别让 !status 误报「飞书在线」。
	if s.platform != nil && s.cfg.Platform == "feishu" {
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
		return fmt.Errorf("读配置 %s 失败：%w", *cfgPath, err)
	}

	sink := ingress.New(cfg.Edge.CoreSocket,
		time.Duration(cfg.Edge.HandleEventDeadlineMS)*time.Millisecond, cfg.Edge.MaxMessageMB)

	platform, err := feishu.New(cfg.Feishu, feishu.Options{
		AppID:     os.Getenv(cfg.Feishu.AppIDEnv),
		AppSecret: os.Getenv(cfg.Feishu.AppSecretEnv),
		BotOpenID: os.Getenv(cfg.Feishu.BotOpenIDEnv),
		TenantID:  cfg.TenantID,
	}, sink)
	if err != nil {
		_ = sink.Close()
		return fmt.Errorf("装配飞书 adapter 失败：%w", err)
	}
	docker, err := sandbox.NewDocker(cfg.Sandbox)
	if err != nil {
		_ = sink.Close()
		return fmt.Errorf("装配 Docker 沙箱失败：%w", err)
	}

	lis, err := server.ListenUnix(cfg.Edge.EdgeSocket)
	if err != nil {
		_ = sink.Close()
		return fmt.Errorf("监听 %s 失败（socket 被占用？父目录不可写？）：%w", cfg.Edge.EdgeSocket, err)
	}
	srv := server.New(platform, docker,
		&statusSource{cfg: cfg, platform: platform, docker: docker}, cfg.Edge.MaxMessageMB)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	watchSignals(cancel)

	slog.Info("edge.takeoff",
		"version", version, "contract_version", server.ContractVersion, "platform", cfg.Platform,
		"edge_socket", cfg.Edge.EdgeSocket, "core_socket", cfg.Edge.CoreSocket,
		"image", cfg.Sandbox.Image)

	errc := make(chan error, 2)
	go func() {
		slog.Info("edge.grpc_listening", "socket", cfg.Edge.EdgeSocket, "contract_version", server.ContractVersion)
		errc <- srv.Serve(lis)
	}()
	// 探针单开一条腿：daemon 不可达时它要等满超时，不能让 socket 跟着晚就绪。
	go probeSandbox(ctx, docker)

	platformDone := make(chan struct{})
	if cfg.Platform == "feishu" {
		go func() {
			defer close(platformDone)
			err := platform.Start(ctx)
			if ctx.Err() != nil {
				return // 收到停机信号，Start 返回是正常收尾
			}
			// 没收到停机信号却返回了 —— 哪怕返回 nil 也是组件挂了：
			// 再吊着就是「进程还在、事件再也不来」，比直接退更难发现。
			if err == nil {
				err = errors.New("长连接自己结束了（Start 返回 nil，但没收到停机信号）")
			}
			errc <- fmt.Errorf("feishu.start: %w", err)
		}()
	} else {
		close(platformDone)
		slog.Info("edge.platform_fake", "note", "platform=fake：不起飞书长连接，只提供 gRPC 服务面")
	}

	var runErr error
	select {
	case <-ctx.Done():
		slog.Info("edge.shutting_down")
	case err := <-errc:
		if err != nil {
			slog.Error("edge.component_failed", "err", err)
			runErr = err
		}
		cancel() // 一个组件倒了，另一个也别吊着
	}

	shutdown(srv, cancel, platformDone, sink, docker)
	return runErr
}

// probeSandbox 起飞时探一次 docker。不可达不拦着起飞（daemon 可能晚点才起，
// core 那边看 EdgeStatus.sandbox_ok 就知道），但得留一行。
func probeSandbox(ctx context.Context, docker *sandbox.Docker) {
	pctx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	if err := docker.Ping(pctx); err != nil {
		slog.Warn("edge.sandbox_unavailable", "err", err)
		return
	}
	slog.Info("edge.sandbox_ok")
}

// shutdown 是冻结序列：1 停 gRPC 面（上限 10s）2 关长连接 3 关到 core 的连接
// 4 还沙箱 5 报 down。
func shutdown(srv *grpc.Server, cancel context.CancelFunc, platformDone <-chan struct{},
	sink *ingress.Client, docker *sandbox.Docker) {
	done := make(chan struct{})
	go func() { srv.GracefulStop(); close(done) }()
	select {
	case <-done:
	case <-time.After(gracefulStopTimeout):
		slog.Warn("edge.graceful_timeout", "grace_sec", int(gracefulStopTimeout.Seconds()))
		srv.Stop()
		<-done
	}

	cancel()
	select {
	case <-platformDone:
	case <-time.After(platformStopTimeout):
		slog.Warn("edge.platform_stop_timeout")
	}

	c := sink.Counters()
	slog.Info("edge.counters",
		"events.sent", c.Sent, "ingress.invalid", c.Invalid,
		"ingress.errors", c.Errors, "ingress.reconnects", c.Reconnects)
	_ = sink.Close()

	docker.CloseAll()
	slog.Info("edge.down")
}

// watchSignals：第一次 SIGINT/SIGTERM 走优雅退出，第二次直写 stderr 硬退 130
// （日志可能正卡在收尾里，所以不走 slog）。
func watchSignals(first context.CancelFunc) {
	sigc := make(chan os.Signal, 4)
	signal.Notify(sigc, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		sig := <-sigc
		slog.Info("edge.signal", "signal", signalName(sig), "note", "开始优雅退出")
		first()
		for again := range sigc {
			fmt.Fprintf(os.Stderr, "aite-edge: 又收到 %s，硬退出。\n", signalName(again))
			os.Exit(exitHardStop)
		}
	}()
}

func signalName(sig os.Signal) string {
	switch sig {
	case syscall.SIGINT:
		return "SIGINT"
	case syscall.SIGTERM:
		return "SIGTERM"
	default:
		return sig.String()
	}
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "aite-edge:", err)
		os.Exit(1)
	}
}
