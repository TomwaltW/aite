// aite-edge 的进程级测试：真编出二进制、真起进程、真发信号。
// 对应 Python 版 tests/integration 里与进程收尾相关的那几条语义（信号、优雅退出、socket 清理）。
// 用 platform: fake，所以不碰飞书；GetStatus 里 sandbox_ok 只是探一下 docker，daemon 不在也不影响判据。

package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/protobuf/encoding/prototext"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/server"
)

var (
	buildOnce sync.Once
	builtBin  string
	buildErr  error
)

// edgeBinary 把 aite-edge 编出来（整包共用一份）。
func edgeBinary(t *testing.T) string {
	t.Helper()
	buildOnce.Do(func() {
		dir, err := os.MkdirTemp("", "aite-edge-bin")
		if err != nil {
			buildErr = err
			return
		}
		bin := filepath.Join(dir, "aite-edge")
		out, err := exec.Command("go", "build", "-o", bin, ".").CombinedOutput()
		if err != nil {
			buildErr = fmt.Errorf("go build 失败：%v\n%s", err, out)
			return
		}
		builtBin = bin
	})
	if buildErr != nil {
		t.Fatalf("%v", buildErr)
	}
	return builtBin
}

// runDir 给测试一个短目录：macOS 的 unix socket 路径上限 104 字节。
func runDir(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "aite-edge")
	if err != nil {
		t.Fatalf("MkdirTemp 失败：%v", err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	return dir
}

// writeConfig 落一份 platform: fake 的配置，socket 都在临时目录里。
func writeConfig(t *testing.T, dir string) (cfgPath, edgeSock, coreSock string) {
	t.Helper()
	edgeSock = filepath.Join(dir, "e.sock")
	coreSock = filepath.Join(dir, "c.sock")
	if len(edgeSock) > 100 {
		t.Skipf("socket 路径太长（%d 字节）：%s", len(edgeSock), edgeSock)
	}
	cfgPath = filepath.Join(dir, "aite.yaml")
	body := fmt.Sprintf(`tenant_id: default
platform: fake
sandbox:
  image: aite-sandbox:p0
edge:
  edge_socket: %s
  core_socket: %s
  handle_event_deadline_ms: 1000
  max_message_mb: 64
`, edgeSock, coreSock)
	if err := os.WriteFile(cfgPath, []byte(body), 0o644); err != nil {
		t.Fatalf("写配置失败：%v", err)
	}
	return cfgPath, edgeSock, coreSock
}

type edgeProc struct {
	cmd    *exec.Cmd
	stderr *strings.Builder
	sock   string
	waited bool
	err    error
}

func startEdge(t *testing.T, cfgPath, edgeSock string, env ...string) *edgeProc {
	t.Helper()
	cmd := exec.Command(edgeBinary(t), "--config", cfgPath)
	if len(env) > 0 {
		cmd.Env = append(os.Environ(), env...)
	}
	var buf strings.Builder
	cmd.Stdout = &buf
	cmd.Stderr = &buf
	if err := cmd.Start(); err != nil {
		t.Fatalf("起 aite-edge 失败：%v", err)
	}
	p := &edgeProc{cmd: cmd, stderr: &buf, sock: edgeSock}
	t.Cleanup(func() {
		if !p.waited {
			_ = cmd.Process.Kill()
			_ = cmd.Wait()
		}
	})
	return p
}

// waitReady 等到 GetStatus 真的能调通（socket 文件先出现，gRPC 面才起来）。
func (p *edgeProc) waitReady(t *testing.T) *pb.EdgeStatus {
	t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	var last error
	for time.Now().Before(deadline) {
		if st, err := getStatus(p.sock); err == nil {
			return st
		} else {
			last = err
		}
		if p.cmd.ProcessState != nil {
			t.Fatalf("aite-edge 提前退了：\n%s", p.stderr.String())
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatalf("20s 内没能调通 GetStatus：%v\n%s", last, p.stderr.String())
	return nil
}

func (p *edgeProc) signal(t *testing.T, sig syscall.Signal) {
	t.Helper()
	if err := p.cmd.Process.Signal(sig); err != nil {
		t.Fatalf("发 %v 失败：%v", sig, err)
	}
}

// waitExit 等进程退出，返回退出码。
func (p *edgeProc) waitExit(t *testing.T, within time.Duration) int {
	t.Helper()
	done := make(chan error, 1)
	go func() { done <- p.cmd.Wait() }()
	select {
	case err := <-done:
		p.waited, p.err = true, err
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			return ee.ExitCode()
		}
		if err != nil {
			t.Fatalf("Wait 报错：%v\n%s", err, p.stderr.String())
		}
		return 0
	case <-time.After(within):
		_ = p.cmd.Process.Kill()
		t.Fatalf("%v 内没退出：\n%s", within, p.stderr.String())
		return -1
	}
}

func getStatus(sock string) (*pb.EdgeStatus, error) {
	conn, err := grpc.NewClient("unix:"+sock,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, _ string) (net.Conn, error) {
			var d net.Dialer
			return d.DialContext(ctx, "unix", sock)
		}))
	if err != nil {
		return nil, err
	}
	defer func() { _ = conn.Close() }()

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	return pb.NewEdgeStatusServiceClient(conn).GetStatus(ctx, &pb.GetStatusRequest{})
}

// platform: fake 下能起飞、GetStatus 可达、contract_version 对得上，SIGTERM 退出码 0、socket 被清掉。
func TestEdgeTakesOffAndStopsOnSIGTERM(t *testing.T) {
	dir := runDir(t)
	cfgPath, edgeSock, _ := writeConfig(t, dir)
	p := startEdge(t, cfgPath, edgeSock)

	st := p.waitReady(t)
	t.Logf("GetStatus → %s", prototext.MarshalOptions{Multiline: false}.Format(st))
	if st.GetContractVersion() != server.ContractVersion {
		t.Fatalf("contract_version = %q，想要 %q", st.GetContractVersion(), server.ContractVersion)
	}
	if st.GetContractVersion() != "p0.2" {
		t.Fatalf("contract_version = %q，想要 p0.2", st.GetContractVersion())
	}
	if st.GetPlatform() != "fake" {
		t.Fatalf("platform = %q，想要 fake", st.GetPlatform())
	}
	if st.GetPlatformConnected() {
		t.Fatal("platform: fake 下不该报着飞书在线")
	}
	if st.GetReconnectCount() != 0 {
		t.Fatalf("reconnect_count = %d，想要 0", st.GetReconnectCount())
	}
	if st.GetVersion() == "" {
		t.Fatal("version 是空的")
	}

	p.signal(t, syscall.SIGTERM)
	if code := p.waitExit(t, 20*time.Second); code != 0 {
		t.Fatalf("SIGTERM 之后退出码 = %d，想要 0\n%s", code, p.stderr.String())
	}
	if _, err := os.Stat(edgeSock); !os.IsNotExist(err) {
		t.Fatalf("socket 文件没清掉：%s（stat err=%v）", edgeSock, err)
	}
	logs := p.stderr.String()
	for _, want := range []string{"edge.takeoff", "edge.platform_fake", "edge.signal", "edge.shutting_down", "edge.down"} {
		if !strings.Contains(logs, want) {
			t.Fatalf("日志里没有 %s：\n%s", want, logs)
		}
	}
}

// SIGINT 与 SIGTERM 同路：也是优雅退出，退出码 0。
func TestEdgeStopsOnSIGINT(t *testing.T) {
	dir := runDir(t)
	cfgPath, edgeSock, _ := writeConfig(t, dir)
	p := startEdge(t, cfgPath, edgeSock)
	p.waitReady(t)

	p.signal(t, syscall.SIGINT)
	if code := p.waitExit(t, 20*time.Second); code != 0 {
		t.Fatalf("SIGINT 之后退出码 = %d，想要 0\n%s", code, p.stderr.String())
	}
}

// blackholeDocker 造一个只 accept 不回话的 unix socket 当 DOCKER_HOST：
// 这样 GetStatus 里的 docker.Ping 会一直等到自己的 2s 超时，GracefulStop 得等这条在飞的调用
// —— 收尾被拖住，第二次信号才有落点可打。
func blackholeDocker(t *testing.T, dir string) string {
	t.Helper()
	sock := filepath.Join(dir, "d.sock")
	lis, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatalf("造假 docker socket 失败：%v", err)
	}
	var mu sync.Mutex
	var conns []net.Conn
	go func() {
		for {
			c, err := lis.Accept()
			if err != nil {
				return
			}
			mu.Lock()
			conns = append(conns, c) // 只收不回
			mu.Unlock()
		}
	}()
	t.Cleanup(func() {
		_ = lis.Close()
		mu.Lock()
		for _, c := range conns {
			_ = c.Close()
		}
		mu.Unlock()
	})
	return sock
}

// 第二次信号硬退 130，并直写 stderr 一行人话。
func TestEdgeSecondSignalHardExits(t *testing.T) {
	dir := runDir(t)
	cfgPath, edgeSock, _ := writeConfig(t, dir)
	blackhole := blackholeDocker(t, dir)
	p := startEdge(t, cfgPath, edgeSock, "DOCKER_HOST=unix://"+blackhole)
	p.waitReady(t)

	// 挂一条在飞的 GetStatus（docker.Ping 要等满 2s），GracefulStop 会卡在它上面。
	inflight := make(chan struct{})
	go func() { defer close(inflight); _, _ = getStatus(edgeSock) }()
	time.Sleep(300 * time.Millisecond)

	p.signal(t, syscall.SIGTERM)
	time.Sleep(300 * time.Millisecond)
	if p.cmd.ProcessState != nil {
		t.Fatalf("收尾没被拖住，第二刀无处可落：\n%s", p.stderr.String())
	}
	p.signal(t, syscall.SIGTERM)

	code := p.waitExit(t, 20*time.Second)
	<-inflight
	logs := p.stderr.String()
	if !strings.Contains(logs, "aite-edge: 又收到 SIGTERM，硬退出。") {
		t.Fatalf("stderr 里没有硬退出那行：\n%s", logs)
	}
	if code != exitHardStop {
		t.Fatalf("第二次信号之后退出码 = %d，想要 %d\n%s", code, exitHardStop, logs)
	}
}

// 配置文件不存在：一行人话 + 退出码 1。
func TestEdgeMissingConfigExitsOne(t *testing.T) {
	dir := runDir(t)
	cmd := exec.Command(edgeBinary(t), "--config", filepath.Join(dir, "nope.yaml"))
	out, err := cmd.CombinedOutput()

	var ee *exec.ExitError
	if !errors.As(err, &ee) {
		t.Fatalf("配置不存在却没失败：err=%v out=%s", err, out)
	}
	if ee.ExitCode() != 1 {
		t.Fatalf("退出码 = %d，想要 1\n%s", ee.ExitCode(), out)
	}
	if !strings.Contains(string(out), "aite-edge: 读配置") {
		t.Fatalf("stderr 不是一行人话：\n%s", out)
	}
}

// 配置里 platform 写错：Validate 拦下来，退出码 1。
func TestEdgeBadPlatformExitsOne(t *testing.T) {
	dir := runDir(t)
	cfgPath := filepath.Join(dir, "aite.yaml")
	body := fmt.Sprintf("platform: wechat\nedge:\n  edge_socket: %s\n  core_socket: %s\n",
		filepath.Join(dir, "e.sock"), filepath.Join(dir, "c.sock"))
	if err := os.WriteFile(cfgPath, []byte(body), 0o644); err != nil {
		t.Fatalf("写配置失败：%v", err)
	}

	out, err := exec.Command(edgeBinary(t), "--config", cfgPath).CombinedOutput()
	var ee *exec.ExitError
	if !errors.As(err, &ee) {
		t.Fatalf("platform 非法却没失败：err=%v out=%s", err, out)
	}
	if ee.ExitCode() != 1 {
		t.Fatalf("退出码 = %d，想要 1\n%s", ee.ExitCode(), out)
	}
	if !strings.Contains(string(out), "platform 必须是 feishu 或 fake") {
		t.Fatalf("stderr 没说清哪里错了：\n%s", out)
	}
}

// 起飞之前 socket 上有残留文件：ListenUnix 先清再监听，不该因此起不来。
func TestEdgeTakesOffOverStaleSocketFile(t *testing.T) {
	dir := runDir(t)
	cfgPath, edgeSock, _ := writeConfig(t, dir)
	if err := os.WriteFile(edgeSock, []byte("stale"), 0o600); err != nil {
		t.Fatalf("造残留文件失败：%v", err)
	}

	p := startEdge(t, cfgPath, edgeSock)
	p.waitReady(t)
	p.signal(t, syscall.SIGTERM)
	if code := p.waitExit(t, 20*time.Second); code != 0 {
		t.Fatalf("退出码 = %d，想要 0\n%s", code, p.stderr.String())
	}
}
