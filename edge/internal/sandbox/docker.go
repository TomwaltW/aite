// Package sandbox 是 SandboxPort 的 Docker 实现（对应旧 aite/sandbox/docker_sandbox.py）。
//
// owner: R2。按移植清单 §3 逐条落地的要点：
//
//   - Acquire 建的容器一定带标签 aite.task=<task_id>（B4 靠
//     `docker ps -a --filter label=aite.task` 验收），network=none
//     —— 凭证与平台调用都不在沙箱里发生（SandboxSpec.network 在 P0 只允许 "none"）。
//   - /work 是唯一工作面：PutFile / GetFile 的 path 必须在它下面，越界返回
//     SandboxError{InvalidPath}（归一化在 workdir.go 里，是纯函数）。
//   - Exec 的超时**在容器内**由 coreutils timeout 强制执行，超时退出码统一成
//     ExecTimeoutExitCode(124)。这样即使调用方那边的 ctx 先取消，容器里的进程
//     也已经被杀掉了，不会留一个跑满 CPU 的孤儿。
//   - stdout / stderr 各自超 MaxExecOutputChars 截断并置 truncated=true。
//   - files_out 是本次 Exec 新增 / 修改的 /work 下文件（跑之前后各拍一次
//     size+mtime_ns 快照做差集）。
//   - Release 幂等；Touch 刷新最近活动时间；ReapIdle 释放空闲超时的容器并返回被
//     释放的 id。ReapIdle 除了自己记账的容器，还会扫标签捡上一次进程留下的孤儿
//     —— 这条是 B4「reap 之后 docker ps -a 必须为空」的兜底。
//
// 日志事件名：Python 版这一层没有 logger，所以下面这几个是本轨新起的，风格沿用
// 控制面（`sandbox.acquired` / `sandbox.released` / `sandbox.reaped` /
// `sandbox.ready_failed` / `sandbox.exec_timeout` / `sandbox.release_failed`）。
package sandbox

import (
	"archive/tar"
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"path"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/docker/docker/api/types/container"
	"github.com/docker/docker/api/types/filters"
	"github.com/docker/docker/client"
	"github.com/docker/docker/pkg/stdcopy"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
	"aite/edge/internal/config"
)

const (
	// LabelTask 是容器标签键。B4 的验收命令写死了它：`docker ps -a --filter label=aite.task`。
	LabelTask = "aite.task"
	// LabelManaged 再打一个「这是 Aite 管的」标记，方便运维一眼分辨（不参与任何判据）。
	LabelManaged = "aite.managed"

	// MaxExecOutputChars 与契约 MAX_EXEC_OUTPUT_CHARS 一致
	// （proto/aite/v1/sandbox.proto ExecResult.truncated 的注释写死了 20000）。
	MaxExecOutputChars = 20000

	// envWorkdir 是容器里探路用的环境变量名，避免把 workdir 拼进 shell 命令。
	envWorkdir = "AITE_WORKDIR"

	truncMarker = "\n…[输出已截断]"
	readyMark   = "AITE_SANDBOX_OK"
	pidsLimit   = 256
)

const readyCode = `
import os, shutil
wd = os.environ.get("AITE_WORKDIR", "/work")
bad = []
if shutil.which("timeout") is None:
    bad.append("镜像里没有 coreutils timeout，沙箱无法强制执行超时")
if not os.path.isdir(wd):
    bad.append("工作目录不存在：" + wd)
elif not os.access(wd, os.W_OK):
    bad.append("工作目录不可写：" + wd)
print("AITE_SANDBOX_OK" if not bad else "AITE_SANDBOX_BAD: " + "; ".join(bad))
`

const snapshotCode = `
import json, os, sys
root = os.environ.get("AITE_WORKDIR", "/work")
out = {}
for folder, _dirs, files in os.walk(root):
    for name in files:
        path = os.path.join(folder, name)
        try:
            st = os.stat(path)
        except OSError:
            continue
        out[path] = [st.st_size, st.st_mtime_ns]
json.dump(out, sys.stdout)
`

// fileMeta 是快照里的一条：[size, mtime_ns]。数组可比较，差集直接用 != 判。
type fileMeta = [2]int64

// box 是一个活着的沙箱在本进程里的记账。
type box struct {
	sandboxID  string
	taskID     string
	workdir    string
	lastActive time.Time
}

// Docker 实现 server.SandboxPort。一个 task 一个容器：谁 Acquire 谁负责给 sandbox_id
// 找地方存（core 侧按 task_id 记着），本类只管容器本身的生死与 /work 的读写。
type Docker struct {
	cfg config.Sandbox

	// PutFile 落到容器里的文件归谁：镜像里跑的是 uid 1000 的 aite 用户，
	// 文件写成 root 的话沙箱里的代码就改不动它。
	fileUID      int
	fileGID      int
	keepaliveCmd []string
	killGraceSec int

	// now 是记账用的时钟，测试可注入（reaper 的判据不该靠真 sleep）。
	now func() time.Time

	cliMu sync.Mutex
	cli   *client.Client

	mu    sync.Mutex
	boxes map[string]*box
}

// NewDocker 只做装配，不碰 daemon；Ping 才探活，client 懒建。
func NewDocker(cfg config.Sandbox) (*Docker, error) {
	return &Docker{
		cfg:          cfg,
		fileUID:      1000,
		fileGID:      1000,
		keepaliveCmd: []string{"sleep", "infinity"},
		killGraceSec: 2,
		now:          time.Now,
		boxes:        map[string]*box{},
	}, nil
}

// ------------------------------------------------------------------ SandboxPort

// Ping 探 docker daemon 是否可达（EdgeStatus.sandbox_ok / preflight 用）。
func (d *Docker) Ping(ctx context.Context) error {
	cli, err := d.dockerClient()
	if err != nil {
		return err
	}
	if _, err := cli.Ping(ctx); err != nil {
		return sbErr(aiteerr.SandboxUnavailable, "连不上 Docker daemon：%v", err)
	}
	return nil
}

func (d *Docker) Acquire(ctx context.Context, taskID string, spec *pb.SandboxSpec) (string, error) {
	cli, err := d.dockerClient()
	if err != nil {
		return "", err
	}
	s := d.resolveSpec(spec)

	if _, err := cli.ImageInspect(ctx, s.image); err != nil {
		if client.IsErrNotFound(err) {
			return "", sbErr(aiteerr.SandboxUnavailable,
				"沙箱镜像不存在：%s。先跑 `docker build -t %s docker/sandbox`", s.image, s.image)
		}
		return "", sbErr(aiteerr.SandboxUnavailable, "查沙箱镜像失败：%v", err)
	}

	pids := int64(pidsLimit)
	created, err := cli.ContainerCreate(ctx,
		&container.Config{
			Image:      s.image,
			Cmd:        d.keepaliveCmd,
			Labels:     map[string]string{LabelTask: taskID, LabelManaged: "p0"},
			WorkingDir: s.workdir,
			Env:        []string{envWorkdir + "=" + s.workdir},
			Tty:        false,
			OpenStdin:  false,
		},
		&container.HostConfig{
			// P0 沙箱无网络。这里照 spec 的值转，别人以后放开这个字段时不会静默连上网。
			NetworkMode: container.NetworkMode(s.network),
			CapDrop:     []string{"ALL"},
			SecurityOpt: []string{"no-new-privileges:true"},
			Resources: container.Resources{
				Memory:    int64(s.memMB) * 1024 * 1024,
				NanoCPUs:  int64(s.cpu * 1e9),
				PidsLimit: &pids,
			},
		}, nil, nil, "")
	if err != nil {
		return "", dockerErr(err, aiteerr.SandboxInternal, "创建沙箱容器失败（task=%s）：%v", taskID, err)
	}

	sandboxID := created.ID
	if err := cli.ContainerStart(ctx, sandboxID, container.StartOptions{}); err != nil {
		// 起不来的容器别留在机器上等 reaper —— 它连 exec 都进不去。
		_ = d.Release(ctx, sandboxID)
		return "", dockerErr(err, aiteerr.SandboxInternal, "创建沙箱容器失败（task=%s）：%v", taskID, err)
	}

	d.mu.Lock()
	d.boxes[sandboxID] = &box{sandboxID: sandboxID, taskID: taskID, workdir: s.workdir, lastActive: d.now()}
	d.mu.Unlock()

	if err := d.checkReady(ctx, sandboxID, s.workdir); err != nil {
		// 探路不过就别把半残的容器留给调用方 —— 它连超时都保证不了。
		slog.Warn("sandbox.ready_failed", "sandbox", short(sandboxID), "task", taskID, "err", err)
		_ = d.Release(ctx, sandboxID)
		return "", err
	}
	slog.Info("sandbox.acquired", "sandbox", short(sandboxID), "task", taskID, "image", s.image)
	return sandboxID, nil
}

func (d *Docker) Exec(ctx context.Context, sandboxID string, req *pb.ExecRequest) (*pb.ExecResult, error) {
	started := time.Now()
	b, err := d.requireBox(sandboxID)
	if err != nil {
		return nil, err
	}

	before, err := d.snapshot(ctx, sandboxID, b.workdir)
	if err != nil {
		return nil, err
	}

	// 用户代码落到 /tmp（不在 /work 下），否则它自己会出现在 files_out 里。
	script := "/tmp/aite_exec_" + randomHex() + ".py"
	if err := d.putBytes(ctx, sandboxID, script, []byte(req.GetCode()), 0o600); err != nil {
		return nil, err
	}

	timeoutSec := int(req.GetTimeoutSec())
	if timeoutSec <= 0 {
		// proto3 标量没有「没设」这一态：0 按契约默认值（config 的 exec_timeout_sec）看。
		timeoutSec = d.cfg.ExecTimeoutSec
	}
	if timeoutSec < 1 {
		timeoutSec = 1
	}
	cmd := []string{
		"timeout", "-k", strconv.Itoa(d.killGraceSec), strconv.Itoa(timeoutSec),
		"python", "-u", script,
	}
	cmdStarted := time.Now()
	exitCode, stdout, stderr, err := d.execRun(ctx, sandboxID, cmd, b.workdir)
	if err != nil {
		return nil, err
	}
	cmdElapsed := time.Since(cmdStarted)

	after, err := d.snapshot(ctx, sandboxID, b.workdir)
	if err != nil {
		return nil, err
	}
	d.rmQuiet(ctx, sandboxID, script)
	d.touchBox(sandboxID)

	// `timeout` 正常超时给 124；进程扛住 SIGTERM 被 -k 补的 SIGKILL 干掉时给 137，
	// 而 137 也可能是 OOM，所以再用「跑够了时长」这一条把两者分开。
	timedOut := exitCode == ExecTimeoutExitCode ||
		(exitCode == 137 && cmdElapsed >= time.Duration(timeoutSec)*time.Second)
	if timedOut {
		exitCode = ExecTimeoutExitCode
		note := fmt.Sprintf("[aite] 执行超过 %ds 上限，已在沙箱内终止", timeoutSec)
		if strings.TrimSpace(stderr) != "" {
			stderr = strings.TrimRightFunc(stderr, unicode.IsSpace) + "\n" + note
		} else {
			stderr = note
		}
		slog.Info("sandbox.exec_timeout", "sandbox", short(sandboxID), "timeout_sec", timeoutSec)
	}

	stdout, cutOut := clip(stdout, MaxExecOutputChars)
	stderr, cutErr := clip(stderr, MaxExecOutputChars)

	return &pb.ExecResult{
		ExitCode:   int32(exitCode),
		Stdout:     stdout,
		Stderr:     stderr,
		DurationMs: time.Since(started).Milliseconds(),
		Truncated:  cutOut || cutErr,
		FilesOut:   diffFiles(before, after),
	}, nil
}

func (d *Docker) PutFile(ctx context.Context, sandboxID, p string, data []byte) error {
	b, err := d.requireBox(sandboxID)
	if err != nil {
		return err
	}
	target, err := RequireFilePath(p, b.workdir)
	if err != nil {
		return err
	}
	if err := d.putBytes(ctx, sandboxID, target, data, 0o644); err != nil {
		return err
	}
	d.touchBox(sandboxID)
	return nil
}

func (d *Docker) GetFile(ctx context.Context, sandboxID, p string) ([]byte, error) {
	b, err := d.requireBox(sandboxID)
	if err != nil {
		return nil, err
	}
	target, err := RequireFilePath(p, b.workdir)
	if err != nil {
		return nil, err
	}
	cli, err := d.dockerClient()
	if err != nil {
		return nil, err
	}
	rc, _, err := cli.CopyFromContainer(ctx, sandboxID, target)
	if err != nil {
		if client.IsErrNotFound(err) {
			// 文件不存在与容器没了在 Docker 那边都是 404；与 Python 版一样并成一种。
			return nil, sbErr(aiteerr.SandboxFileNotFound, "%s", target)
		}
		return nil, dockerErr(err, aiteerr.SandboxInternal, "从沙箱读 %s 失败：%v", target, err)
	}
	defer func() { _ = rc.Close() }()

	tr := tar.NewReader(rc)
	for {
		hdr, err := tr.Next()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, dockerErr(err, aiteerr.SandboxInternal, "从沙箱读 %s 失败：%v", target, err)
		}
		if hdr.Typeflag != tar.TypeReg {
			continue
		}
		data, err := io.ReadAll(tr)
		if err != nil {
			return nil, dockerErr(err, aiteerr.SandboxInternal, "从沙箱读 %s 失败：%v", target, err)
		}
		d.touchBox(sandboxID)
		return data, nil
	}
	return nil, sbErr(aiteerr.SandboxFileNotFound, "%s（不是一个文件）", target)
}

func (d *Docker) ListFiles(ctx context.Context, sandboxID string) ([]string, error) {
	b, err := d.requireBox(sandboxID)
	if err != nil {
		return nil, err
	}
	snap, err := d.snapshot(ctx, sandboxID, b.workdir)
	if err != nil {
		return nil, err
	}
	paths := make([]string, 0, len(snap))
	for p := range snap {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	// 刻意不 touch：列目录是只读动作，不该把 reaper 往后推。
	return paths, nil
}

// Touch 刷新活动时间。纯属提示性动作：沙箱早就没了也不该把调用方炸掉
// （reaper 与 worker 是两条腿，谁先谁后不定）。
func (d *Docker) Touch(_ context.Context, sandboxID string) error {
	d.touchBox(sandboxID)
	return nil
}

// Release 幂等：不认识的 id、已经没了的容器，都当成已经释放。
func (d *Docker) Release(ctx context.Context, sandboxID string) error {
	d.mu.Lock()
	_, known := d.boxes[sandboxID]
	delete(d.boxes, sandboxID)
	d.mu.Unlock()

	cli, err := d.dockerClient()
	if err != nil {
		return err
	}
	if err := cli.ContainerRemove(ctx, sandboxID, container.RemoveOptions{Force: true}); err != nil {
		if client.IsErrNotFound(err) {
			return nil
		}
		return dockerErr(err, aiteerr.SandboxInternal, "释放沙箱失败（%s）：%v", short(sandboxID), err)
	}
	if known {
		slog.Info("sandbox.released", "sandbox", short(sandboxID))
	}
	return nil
}

// ReapIdle 释放空闲超过 idleSec 的沙箱，返回被释放的 sandbox_id。
func (d *Docker) ReapIdle(ctx context.Context, idleSec int) ([]string, error) {
	idle := time.Duration(idleSec) * time.Second
	now := d.now()

	d.mu.Lock()
	known := make(map[string]struct{}, len(d.boxes))
	victims := make([]string, 0, len(d.boxes))
	for id, b := range d.boxes {
		known[id] = struct{}{}
		if now.Sub(b.lastActive) >= idle {
			victims = append(victims, id)
		}
	}
	d.mu.Unlock()
	// Go 的 map 遍历是随机序，排一下让 released 的顺序可复现。
	sort.Strings(victims)
	victims = append(victims, d.orphans(ctx, idle, known)...)

	released, err := reapVictims(ctx, victims, d.Release)
	if len(released) > 0 {
		slog.Info("sandbox.reaped", "count", len(released), "idle_sec", idleSec)
	}
	return released, err
}

// reapVictims 逐个释放，**一个失败不连累其余**，也不把已经真删掉的 id 弄丢。
//
// 为什么不是「一失败就 return」：gRPC 的返回值是二选一的 —— 带错误返回时
// `ReapIdleResponse` 整个丢掉，`released` 到不了 core。而 core 手上按
// `task → sandbox_id` 记着账，收不到这批 id 就不会清，之后拿一个**容器已经没了**的
// 死 id 去 exec，报出来是「sandbox_not_found」，跟真正的原因（reap 半路失败）
// 一点关系都看不出来。
//
// 所以：失败的记一条 WARN 继续扫，成功的一个不少地回去。只有**一个都没释放成**
// （典型是 daemon 整个不可达）才把错误抛上去 —— 那时 released 本来就是空的，
// 两者不会互相吃掉。没收掉的那些是幂等的，下一轮 reaper（60s）自己会再试。
func reapVictims(ctx context.Context, victims []string, release func(context.Context, string) error) ([]string, error) {
	released := make([]string, 0, len(victims))
	seen := make(map[string]struct{}, len(victims))
	var lastErr error
	failed := 0
	for _, id := range victims {
		if _, dup := seen[id]; dup {
			continue
		}
		seen[id] = struct{}{}
		if err := release(ctx, id); err != nil {
			failed++
			lastErr = err
			slog.Warn("sandbox.release_failed", "sandbox", short(id), "err", err)
			continue
		}
		released = append(released, id)
	}
	if len(released) == 0 && failed > 0 {
		return released, lastErr
	}
	return released, nil
}

// ------------------------------------------------------------------ 附加（非契约）

// CloseAll 释放本进程记着的全部沙箱并关掉 docker 客户端（aite-edge 收尾时调）。
func (d *Docker) CloseAll() {
	d.mu.Lock()
	ids := make([]string, 0, len(d.boxes))
	for id := range d.boxes {
		ids = append(ids, id)
	}
	d.mu.Unlock()
	sort.Strings(ids)

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	for _, id := range ids {
		if err := d.Release(ctx, id); err != nil {
			slog.Warn("sandbox.release_failed", "sandbox", short(id), "err", err)
		}
	}

	d.cliMu.Lock()
	cli := d.cli
	d.cli = nil
	d.cliMu.Unlock()
	if cli != nil {
		_ = cli.Close()
	}
}

// ------------------------------------------------------------------ 内部

type resolvedSpec struct {
	image   string
	network string
	workdir string
	cpu     float64
	memMB   int
}

// resolveSpec 把 proto 里的零值补成契约默认值 —— proto3 标量没有「没设」这一态，
// core 不填某一项时按 config 的默认值来。
func (d *Docker) resolveSpec(spec *pb.SandboxSpec) resolvedSpec {
	r := resolvedSpec{image: d.cfg.Image, cpu: d.cfg.CPU, memMB: d.cfg.MemMB, network: "none", workdir: Workdir}
	if spec == nil {
		return r
	}
	if v := spec.GetImage(); v != "" {
		r.image = v
	}
	if v := spec.GetCpu(); v > 0 {
		r.cpu = v
	}
	if v := spec.GetMemMb(); v > 0 {
		r.memMB = int(v)
	}
	if v := spec.GetNetwork(); v != "" {
		r.network = v
	}
	if v := spec.GetWorkdir(); v != "" {
		r.workdir = v
	}
	return r
}

func (d *Docker) dockerClient() (*client.Client, error) {
	d.cliMu.Lock()
	defer d.cliMu.Unlock()
	if d.cli != nil {
		return d.cli, nil
	}
	cli, err := client.NewClientWithOpts(client.FromEnv, client.WithAPIVersionNegotiation())
	if err != nil {
		return nil, sbErr(aiteerr.SandboxUnavailable, "连不上 Docker daemon：%v", err)
	}
	d.cli = cli
	return cli, nil
}

// requireBox 返回记账的副本（调用方只读 workdir，不该跟着锁里的对象跑）。
func (d *Docker) requireBox(sandboxID string) (*box, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	b, ok := d.boxes[sandboxID]
	if !ok {
		return nil, sbErr(aiteerr.SandboxNotFound,
			"不认识的 sandbox_id：%q（没 acquire 过，或已 release）", sandboxID)
	}
	cp := *b
	return &cp, nil
}

func (d *Docker) touchBox(sandboxID string) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if b, ok := d.boxes[sandboxID]; ok {
		b.lastActive = d.now()
	}
}

func (d *Docker) forget(sandboxID string) {
	d.mu.Lock()
	delete(d.boxes, sandboxID)
	d.mu.Unlock()
}

func (d *Docker) checkReady(ctx context.Context, containerID, workdir string) error {
	code, stdout, stderr, err := d.execRun(ctx, containerID, []string{"python", "-c", readyCode}, workdir)
	if err != nil {
		return err
	}
	if code != 0 || !strings.Contains(stdout, readyMark) {
		detail := strings.TrimSpace(stdout + stderr)
		if detail == "" {
			detail = fmt.Sprintf("exit_code=%d", code)
		}
		return sbErr(aiteerr.SandboxInternal, "沙箱镜像不满足 P0 要求：%s", detail)
	}
	return nil
}

// execRun 是 docker-py `container.exec_run(demux=True)` 的对位：create → attach →
// stdcopy 分流 → inspect 取退出码。容器没了按 SandboxNotFound 报并销记账。
func (d *Docker) execRun(ctx context.Context, containerID string, cmd []string, workdir string) (int, string, string, error) {
	cli, err := d.dockerClient()
	if err != nil {
		return -1, "", "", err
	}
	ex, err := cli.ContainerExecCreate(ctx, containerID, container.ExecOptions{
		Cmd:          cmd,
		WorkingDir:   workdir,
		AttachStdout: true,
		AttachStderr: true,
	})
	if err != nil {
		if client.IsErrNotFound(err) {
			d.forget(containerID)
			return -1, "", "", sbErr(aiteerr.SandboxNotFound, "沙箱容器已经不在了：%s", short(containerID))
		}
		return -1, "", "", dockerErr(err, aiteerr.SandboxInternal, "在沙箱里执行失败：%v", err)
	}

	att, err := cli.ContainerExecAttach(ctx, ex.ID, container.ExecAttachOptions{})
	if err != nil {
		return -1, "", "", dockerErr(err, aiteerr.SandboxInternal, "在沙箱里执行失败：%v", err)
	}
	defer att.Close()

	var outBuf, errBuf bytes.Buffer
	// 容器与 exec 都没有 tty，所以流是多路复用的，StdCopy 能把 stdout/stderr 拆开。
	if _, err := stdcopy.StdCopy(&outBuf, &errBuf, att.Reader); err != nil && !errors.Is(err, io.EOF) {
		return -1, outBuf.String(), errBuf.String(),
			dockerErr(err, aiteerr.SandboxInternal, "读沙箱输出失败：%v", err)
	}
	return d.execExitCode(ctx, cli, ex.ID), outBuf.String(), errBuf.String(), nil
}

// execExitCode 在流收完之后取退出码。daemon 偶尔还没把 exec 标成结束，轮询一小会儿；
// 一直取不到就按 Python 版 `exit_code is None → -1` 的口径报 -1。
func (d *Docker) execExitCode(ctx context.Context, cli *client.Client, execID string) int {
	for i := 0; i < 100; i++ {
		insp, err := cli.ContainerExecInspect(ctx, execID)
		if err != nil {
			return -1
		}
		if !insp.Running {
			return insp.ExitCode
		}
		select {
		case <-ctx.Done():
			return -1
		case <-time.After(10 * time.Millisecond):
		}
	}
	return -1
}

func (d *Docker) snapshot(ctx context.Context, containerID, workdir string) (map[string]fileMeta, error) {
	code, stdout, stderr, err := d.execRun(ctx, containerID, []string{"python", "-c", snapshotCode}, workdir)
	if err != nil {
		return nil, err
	}
	if code != 0 {
		detail := strings.TrimSpace(stderr)
		if detail == "" {
			detail = strings.TrimSpace(stdout)
		}
		return nil, sbErr(aiteerr.SandboxInternal, "给 %s 拍文件快照失败：%s", workdir, detail)
	}
	if strings.TrimSpace(stdout) == "" {
		return map[string]fileMeta{}, nil
	}
	out := map[string]fileMeta{}
	if err := json.Unmarshal([]byte(stdout), &out); err != nil {
		return nil, sbErr(aiteerr.SandboxInternal, "文件快照不是合法 JSON：%q", head(stdout, 200))
	}
	return out, nil
}

func (d *Docker) putBytes(ctx context.Context, containerID, p string, data []byte, mode int64) error {
	parent := path.Dir(p)
	name := path.Base(p)

	code, stdout, stderr, err := d.execRun(ctx, containerID, []string{"mkdir", "-p", parent}, "/")
	if err != nil {
		return err
	}
	if code != 0 {
		detail := strings.TrimSpace(stderr)
		if detail == "" {
			detail = strings.TrimSpace(stdout)
		}
		return sbErr(aiteerr.SandboxInternal, "在沙箱里建目录 %s 失败：%s", parent, detail)
	}

	var buf bytes.Buffer
	tw := tar.NewWriter(&buf)
	hdr := &tar.Header{
		Typeflag: tar.TypeReg,
		Name:     name,
		Size:     int64(len(data)),
		Mode:     mode,
		Uid:      d.fileUID,
		Gid:      d.fileGID,
		// 秒级，免得 archive/tar 为了纳秒精度改用 PAX 头。
		ModTime: time.Unix(time.Now().Unix(), 0),
	}
	if err := tw.WriteHeader(hdr); err != nil {
		return dockerErr(err, aiteerr.SandboxInternal, "写入沙箱 %s 失败：%v", p, err)
	}
	if _, err := tw.Write(data); err != nil {
		return dockerErr(err, aiteerr.SandboxInternal, "写入沙箱 %s 失败：%v", p, err)
	}
	if err := tw.Close(); err != nil {
		return dockerErr(err, aiteerr.SandboxInternal, "写入沙箱 %s 失败：%v", p, err)
	}

	cli, err := d.dockerClient()
	if err != nil {
		return err
	}
	if err := cli.CopyToContainer(ctx, containerID, parent, bytes.NewReader(buf.Bytes()),
		container.CopyToContainerOptions{}); err != nil {
		if client.IsErrNotFound(err) {
			d.forget(containerID)
			return sbErr(aiteerr.SandboxNotFound, "沙箱容器已经不在了：%s", short(containerID))
		}
		return dockerErr(err, aiteerr.SandboxInternal, "写入沙箱 %s 失败：%v", p, err)
	}
	return nil
}

// rmQuiet 清理临时脚本失败无所谓，容器整个会被 reap 掉。
func (d *Docker) rmQuiet(ctx context.Context, containerID, p string) {
	_, _, _, _ = d.execRun(ctx, containerID, []string{"rm", "-f", p}, "/")
}

// orphans 扫标签找上一次进程留下的孤儿容器 —— 本进程没记账，只能按容器时间算空闲。
// 与 Python 版一致：list 那一趟出任何错就返回空，不报错（reaper 不该因为 docker 抖动而炸）。
func (d *Docker) orphans(ctx context.Context, idle time.Duration, known map[string]struct{}) []string {
	cli, err := d.dockerClient()
	if err != nil {
		return nil
	}
	list, err := cli.ContainerList(ctx, container.ListOptions{
		All:     true,
		Filters: filters.NewArgs(filters.Arg("label", LabelTask)),
	})
	if err != nil {
		return nil
	}

	now := d.now()
	out := make([]string, 0)
	for _, c := range list {
		if _, ok := known[c.ID]; ok {
			continue
		}
		// docker-py 的 containers.list() 对每个容器再做一次 inspect，State 的两个时间戳
		// 只有 inspect 才有；这里照做，失败整趟放弃（同 Python 的 except → []）。
		insp, err := cli.ContainerInspect(ctx, c.ID)
		if err != nil {
			return nil
		}
		var newest time.Time
		seen := false
		for _, raw := range inspectStamps(insp) {
			if t, ok := parseDockerTime(raw); ok {
				if !seen || t.After(newest) {
					newest, seen = t, true
				}
			}
		}
		// 三个时间戳都解析不出也收 —— 宁可多收一个孤儿，也别把它永远留在机器上。
		if !seen || now.Sub(newest) >= idle {
			out = append(out, c.ID)
		}
	}
	return out
}

func inspectStamps(insp container.InspectResponse) []string {
	stamps := make([]string, 0, 3)
	if insp.ContainerJSONBase != nil {
		stamps = append(stamps, insp.Created)
		if insp.State != nil {
			stamps = append(stamps, insp.State.StartedAt, insp.State.FinishedAt)
		}
	}
	return stamps
}

// parseDockerTime 解析 Docker 的 RFC3339 纳秒时间戳（如 2026-09-09T12:00:00.123456789Z）。
func parseDockerTime(value string) (time.Time, bool) {
	text := strings.TrimSpace(value)
	// Docker 用 0001-01-01 表示「没发生过」。
	if text == "" || strings.HasPrefix(text, "0001-01-01") {
		return time.Time{}, false
	}
	t, err := time.Parse(time.RFC3339Nano, text)
	if err != nil {
		return time.Time{}, false
	}
	return t.UTC(), true
}

// diffFiles 是本次 Exec 新增 / 修改的文件（比 size + mtime_ns）。删掉的不出现。
func diffFiles(before, after map[string]fileMeta) []*pb.FileEntry {
	paths := make([]string, 0, len(after))
	for p := range after {
		paths = append(paths, p)
	}
	sort.Strings(paths)

	out := make([]*pb.FileEntry, 0)
	for _, p := range paths {
		meta := after[p]
		if old, ok := before[p]; ok && old == meta {
			continue
		}
		out = append(out, &pb.FileEntry{Path: p, Size: meta[0]})
	}
	return out
}

// clip 超长就截断，返回 (文本, 是否截断过)。按 rune 数算，与 Python 的 len(str) 一致。
func clip(text string, limit int) (string, bool) {
	if utf8.RuneCountInString(text) <= limit {
		return text, false
	}
	keep := limit - utf8.RuneCountInString(truncMarker)
	if keep < 0 {
		keep = 0
	}
	return string([]rune(text)[:keep]) + truncMarker, true
}

func sbErr(kind aiteerr.SandboxErrKind, format string, args ...any) *aiteerr.SandboxError {
	return &aiteerr.SandboxError{Kind: kind, Msg: fmt.Sprintf(format, args...)}
}

// dockerErr 和 sbErr 一样，但先认一认「是不是 daemon 没了」。
//
// 契约（proto/aite/v1/edge.proto 头注释）把「docker daemon 不可达」冻结成 UNAVAILABLE，
// 而 core 侧 is_retryable 只认 UNAVAILABLE / DEADLINE_EXCEEDED / ABORTED / RESOURCE_EXHAUSTED。
// 报成 Internal 的话，daemon 重启这种最典型的可重试抖动会被 core 当成不可重试。
// daemon 挂掉时走的正是 ContainerCreate / Start / exec / copy 这些路径，不是只有 Ping。
func dockerErr(err error, kind aiteerr.SandboxErrKind, format string, args ...any) *aiteerr.SandboxError {
	if client.IsErrConnectionFailed(err) {
		kind = aiteerr.SandboxUnavailable
	}
	return sbErr(kind, format, args...)
}

func randomHex() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		return strconv.FormatInt(time.Now().UnixNano(), 16)
	}
	return hex.EncodeToString(b[:])
}

func short(id string) string {
	if len(id) > 12 {
		return id[:12]
	}
	return id
}

func head(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n])
}
