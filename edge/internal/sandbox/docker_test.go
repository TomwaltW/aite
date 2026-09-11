//go:build docker

// DockerSandbox 真跑容器的测试（对应旧 tests/sandbox/test_docker_sandbox.py 的 18 条
// + 镜像的三条硬要求）。`go test ./...` 不带 tag 时整份不编译，碰不到 daemon。
//
// B4 原文两条硬判据：
//   - 跑 matplotlib 生成 /work/out.png → GetFile 返回的前 8 字节是 PNG 魔数
//   - ReapIdle(1) 后 `docker ps -a --filter label=aite.task=<task_id>` 为空
//
// 串扰提醒：`docker ps -a --filter label=aite.task` 是**全机器**命名空间。
// 每条用例用自己独有的 task_id 前缀，断言一律按 task_id 过滤，不断言「全局为空」。

package sandbox

import (
	"context"
	"errors"
	"os/exec"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf8"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
	"aite/edge/internal/config"
)

const testImage = "aite-sandbox:p0"

var pngMagic = []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'}

// ------------------------------------------------------------------ 脚手架

type fakeClock struct {
	mu sync.Mutex
	t  time.Time
}

func newFakeClock() *fakeClock { return &fakeClock{t: time.Now()} }

func (c *fakeClock) now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.t
}

func (c *fakeClock) advance(d time.Duration) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.t = c.t.Add(d)
}

func requireDocker(t *testing.T) {
	t.Helper()
	if out, err := exec.Command("docker", "info", "--format", "{{.ServerVersion}}").CombinedOutput(); err != nil {
		t.Skipf("本机 Docker daemon 没起来：%v（%s）", err, strings.TrimSpace(string(out)))
	}
	if err := exec.Command("docker", "image", "inspect", testImage).Run(); err != nil {
		t.Skipf("没有 %s 镜像：先跑 `docker build -t %s docker/sandbox`", testImage, testImage)
	}
}

func testConfig() config.Sandbox {
	return config.Sandbox{Image: testImage, CPU: 1.0, MemMB: 768, IdleSec: 300, ExecTimeoutSec: 120}
}

// newSandbox 起一个干净的 Docker 沙箱；用完把它记账的容器全删掉。时钟是假的，
// reaper 的判据靠注入推进，不靠真 sleep。
func newSandbox(t *testing.T) (*Docker, *fakeClock) {
	t.Helper()
	requireDocker(t)
	d, err := NewDocker(testConfig())
	if err != nil {
		t.Fatalf("NewDocker 失败：%v", err)
	}
	clk := newFakeClock()
	d.now = clk.now
	t.Cleanup(d.CloseAll)
	return d, clk
}

func testSpec() *pb.SandboxSpec {
	return &pb.SandboxSpec{Image: testImage, Cpu: 1.0, MemMb: 768, Network: "none", Workdir: Workdir}
}

// taskID 给每条用例一个本机唯一的 task_id：别的轨 / 旧 Python 树同时跑测试会互相看见。
func taskID(name string) string { return "r2-" + name + "-" + randomHex()[:8] }

// labelledIDs 是 B4 的验收命令：docker ps -a --filter label=aite.task[=<task>]（全长 id 好比对）。
func labelledIDs(t *testing.T, task string) []string {
	t.Helper()
	filter := "label=" + LabelTask
	if task != "" {
		filter += "=" + task
	}
	out, err := exec.Command("docker", "ps", "-a", "--filter", filter, "-q", "--no-trunc").Output()
	if err != nil {
		t.Fatalf("docker ps 失败：%v", err)
	}
	ids := make([]string, 0, 4)
	for _, line := range strings.Split(string(out), "\n") {
		if s := strings.TrimSpace(line); s != "" {
			ids = append(ids, s)
		}
	}
	return ids
}

func inspectField(t *testing.T, id, format string) string {
	t.Helper()
	out, err := exec.Command("docker", "inspect", id, "--format", format).Output()
	if err != nil {
		t.Fatalf("docker inspect %s 失败：%v", short(id), err)
	}
	return strings.TrimSpace(string(out))
}

func mustAcquire(t *testing.T, d *Docker, task string) string {
	t.Helper()
	id, err := d.Acquire(context.Background(), task, testSpec())
	if err != nil {
		t.Fatalf("Acquire(%s) 失败：%v", task, err)
	}
	return id
}

func mustExec(t *testing.T, d *Docker, id, code string, timeoutSec int32) *pb.ExecResult {
	t.Helper()
	res, err := d.Exec(context.Background(), id, &pb.ExecRequest{Language: "python", Code: code, TimeoutSec: timeoutSec})
	if err != nil {
		t.Fatalf("Exec 报错（用户代码出错不该走这里）：%v", err)
	}
	return res
}

func sandboxKind(err error) (aiteerr.SandboxErrKind, bool) {
	var se *aiteerr.SandboxError
	if errors.As(err, &se) {
		return se.Kind, true
	}
	return 0, false
}

func requireKind(t *testing.T, err error, want aiteerr.SandboxErrKind, what string) {
	t.Helper()
	kind, ok := sandboxKind(err)
	if !ok {
		t.Fatalf("%s：想要 SandboxError，得到 %v", what, err)
	}
	if kind != want {
		t.Fatalf("%s：Kind = %v，想要 %v（%v）", what, kind, want, err)
	}
}

func contains(ids []string, id string) bool {
	for _, x := range ids {
		if x == id {
			return true
		}
	}
	return false
}

func filePaths(entries []*pb.FileEntry) []string {
	out := make([]string, 0, len(entries))
	for _, e := range entries {
		out = append(out, e.GetPath())
	}
	return out
}

// ------------------------------------------------------------------ 用例

// 对应 test_acquire_labels_the_container_with_task_id。
func TestAcquireLabelsTheContainerWithTaskID(t *testing.T) {
	d, _ := newSandbox(t)
	task := taskID("label")
	id := mustAcquire(t, d, task)

	if got := inspectField(t, id, `{{index .Config.Labels "`+LabelTask+`"}}`); got != task {
		t.Fatalf("标签 %s = %q，想要 %q", LabelTask, got, task)
	}
	if got := inspectField(t, id, `{{index .Config.Labels "`+LabelManaged+`"}}`); got != "p0" {
		t.Fatalf("标签 %s = %q，想要 p0", LabelManaged, got)
	}
	if !contains(labelledIDs(t, task), id) {
		t.Fatalf("docker ps -a --filter label=%s=%s 里没有 %s", LabelTask, task, short(id))
	}
}

// 对应 test_acquire_container_has_no_network 的前半：配置面。
func TestAcquireContainerHasNoNetwork(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("no-network"))

	if got := inspectField(t, id, "{{.HostConfig.NetworkMode}}"); got != "none" {
		t.Fatalf("NetworkMode = %q，想要 none（凭证与平台调用都不在沙箱里发生）", got)
	}
	if got := inspectField(t, id, "{{.HostConfig.PidsLimit}}"); got != "256" {
		t.Fatalf("PidsLimit = %q，想要 256", got)
	}
	if got := inspectField(t, id, "{{index .HostConfig.CapDrop 0}}"); got != "ALL" {
		t.Fatalf("CapDrop[0] = %q，想要 ALL", got)
	}
	if got := inspectField(t, id, "{{index .HostConfig.SecurityOpt 0}}"); got != "no-new-privileges:true" {
		t.Fatalf("SecurityOpt[0] = %q，想要 no-new-privileges:true", got)
	}
	if got := inspectField(t, id, "{{.HostConfig.Memory}}"); got != "805306368" {
		t.Fatalf("Memory = %q，想要 805306368（768 MiB）", got)
	}
}

// 对应 test_acquire_rejects_missing_image：要给一句能照着做的话，而不是让 SDK 去 pull 一个私有 tag。
func TestAcquireRejectsMissingImage(t *testing.T) {
	d, _ := newSandbox(t)
	spec := testSpec()
	spec.Image = "aite-nope:x"

	_, err := d.Acquire(context.Background(), taskID("missing-image"), spec)
	if err == nil {
		t.Fatal("镜像不存在却 Acquire 成功了")
	}
	requireKind(t, err, aiteerr.SandboxUnavailable, "镜像不存在")
	if !strings.Contains(err.Error(), "docker build") {
		t.Fatalf("错误里没有 `docker build` 的照做指引：%v", err)
	}
}

// 对应 test_exec_returns_stdout_exit_code_and_files_out。
func TestExecReturnsStdoutExitCodeAndFilesOut(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("exec"))

	res := mustExec(t, d, id, "import os\nprint('cwd', os.getcwd())\nopen('/work/hello.txt', 'w').write('你好')\n", 30)

	if res.GetExitCode() != 0 {
		t.Fatalf("exit_code = %d，stderr=%s", res.GetExitCode(), res.GetStderr())
	}
	if !strings.Contains(res.GetStdout(), "cwd /work") {
		t.Fatalf("工作目录不是 /work：%q", res.GetStdout())
	}
	if res.GetTruncated() {
		t.Fatal("truncated 不该是 true")
	}
	if res.GetDurationMs() <= 0 {
		t.Fatalf("duration_ms = %d，该 > 0", res.GetDurationMs())
	}
	if got := filePaths(res.GetFilesOut()); len(got) != 1 || got[0] != "/work/hello.txt" {
		t.Fatalf("files_out = %v，想要 [/work/hello.txt]", got)
	}
	if got := res.GetFilesOut()[0].GetSize(); got != int64(len("你好")) {
		t.Fatalf("files_out[0].size = %d，想要 %d", got, len("你好"))
	}
}

// 对应 test_exec_files_out_only_reports_this_run：上一轮写的文件不能再报一遍。
func TestExecFilesOutOnlyReportsThisRun(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("files-out"))

	mustExec(t, d, id, "open('/work/a.txt','w').write('1')", 30)
	second := mustExec(t, d, id, "open('/work/b.txt','w').write('2')", 30)
	if got := filePaths(second.GetFilesOut()); len(got) != 1 || got[0] != "/work/b.txt" {
		t.Fatalf("第二次 files_out = %v，想要 [/work/b.txt]", got)
	}

	third := mustExec(t, d, id, "open('/work/a.txt','w').write('333')", 30)
	if got := filePaths(third.GetFilesOut()); len(got) != 1 || got[0] != "/work/a.txt" {
		t.Fatalf("改了就要报：第三次 files_out = %v，想要 [/work/a.txt]", got)
	}
}

// 对应 test_exec_user_code_error_is_not_a_sandbox_failure。
func TestExecUserCodeErrorIsNotASandboxFailure(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("user-error"))

	res := mustExec(t, d, id, "raise ValueError('炸了')", 30)
	if res.GetExitCode() != 1 {
		t.Fatalf("exit_code = %d，想要 1", res.GetExitCode())
	}
	if !strings.Contains(res.GetStderr(), "ValueError") || !strings.Contains(res.GetStderr(), "炸了") {
		t.Fatalf("stderr 里没有 traceback：%q", res.GetStderr())
	}
}

// 对应 test_exec_truncates_output_over_the_cap。
func TestExecTruncatesOutputOverTheCap(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("truncate"))

	res := mustExec(t, d, id, "print('x' * 40000)", 30)
	if res.GetExitCode() != 0 {
		t.Fatalf("exit_code = %d，stderr=%s", res.GetExitCode(), res.GetStderr())
	}
	if !res.GetTruncated() {
		t.Fatal("超 20000 字符没置 truncated")
	}
	if got := utf8.RuneCountInString(res.GetStdout()); got != MaxExecOutputChars {
		t.Fatalf("stdout 长度 = %d，想要 %d", got, MaxExecOutputChars)
	}
	if !strings.HasSuffix(res.GetStdout(), truncMarker) {
		tail := []rune(res.GetStdout())
		t.Fatalf("stdout 尾巴不是截断标记：%q", string(tail[max(0, len(tail)-30):]))
	}
}

// 对应 test_exec_timeout_is_enforced_inside_the_container：超时由容器内的 coreutils
// timeout 强制执行，退出码统一成 124。
func TestExecTimeoutIsEnforcedInsideTheContainer(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("timeout"))

	res := mustExec(t, d, id, "while True:\n    pass\n", 2)
	if res.GetExitCode() != ExecTimeoutExitCode {
		t.Fatalf("exit_code = %d，想要 %d（stderr=%s）", res.GetExitCode(), ExecTimeoutExitCode, res.GetStderr())
	}
	if !strings.Contains(res.GetStderr(), "超过 2s") {
		t.Fatalf("stderr 里没有超时说明：%q", res.GetStderr())
	}
	// 真被杀掉了，不是等满：2s 上限，加上两次快照也远不到 30s。
	if res.GetDurationMs() < 1500 || res.GetDurationMs() >= 30_000 {
		t.Fatalf("duration_ms = %d，想要 ≈2s（1500..30000）", res.GetDurationMs())
	}
}

// 对应 test_put_and_get_file_round_trip。
func TestPutAndGetFileRoundTrip(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("put-get"))
	ctx := context.Background()
	payload := []byte("月份,销量\n2026-01,120\n2026-02,140\n")

	if err := d.PutFile(ctx, id, "/work/in/sales.csv", payload); err != nil {
		t.Fatalf("PutFile 失败：%v", err)
	}
	got, err := d.GetFile(ctx, id, "/work/in/sales.csv")
	if err != nil {
		t.Fatalf("GetFile 失败：%v", err)
	}
	if string(got) != string(payload) {
		t.Fatalf("往返内容不一致：%q", string(got))
	}

	// 沙箱里的代码要读得动、也要改得动（文件属主必须是容器里那个 uid 1000 的用户）
	res := mustExec(t, d, id, "import pandas as pd\ndf = pd.read_csv('/work/in/sales.csv')\nprint('rows', len(df))\nopen('/work/in/sales.csv', 'a').write('2026-03,90\\n')\n", 60)
	if res.GetExitCode() != 0 {
		t.Fatalf("exit_code = %d，stderr=%s", res.GetExitCode(), res.GetStderr())
	}
	if !strings.Contains(res.GetStdout(), "rows 2") {
		t.Fatalf("stdout = %q，想要含 rows 2", res.GetStdout())
	}
}

// 对应 test_put_file_rejects_paths_outside_work。
func TestPutFileRejectsPathsOutsideWork(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("path-guard"))
	ctx := context.Background()

	err := d.PutFile(ctx, id, "/etc/cron.d/pwn", []byte("x"))
	requireKind(t, err, aiteerr.SandboxInvalidPath, "PutFile 越界")

	_, err = d.GetFile(ctx, id, "/work/../etc/passwd")
	requireKind(t, err, aiteerr.SandboxInvalidPath, "GetFile 爬出去")
}

// 对应 test_get_file_missing_raises_file_not_found：core 侧按 file_not_found: 前缀
// 区分「文件没了」与「沙箱没了」。
func TestGetFileMissingIsFileNotFound(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("missing-file"))

	_, err := d.GetFile(context.Background(), id, "/work/never-written.png")
	requireKind(t, err, aiteerr.SandboxNotFound, "GetFile 缺失")
	var se *aiteerr.SandboxError
	if !errors.As(err, &se) || !strings.HasPrefix(se.Msg, "file_not_found:") {
		t.Fatalf("消息要以 file_not_found: 开头，得到 %v", err)
	}

	// 过一遍 R0 的映射，把 core 真正看到的那串留在日志里。
	// 注意：aiteerr 会在前面再贴一层 kind token，core 侧 aite-proto::status 是按
	// `st.message().starts_with("sandbox_not_found")` 判的 —— 这条留给总管定（见回执）。
	st, ok := status.FromError(aiteerr.ToStatus(err))
	if !ok {
		t.Fatalf("ToStatus 没给出 gRPC status：%v", err)
	}
	if st.Code() != codes.NotFound {
		t.Fatalf("gRPC code = %v，想要 NotFound", st.Code())
	}
	t.Logf("core 侧看到的 status：code=%v message=%q", st.Code(), st.Message())
}

// 对应 test_list_files_lists_work_tree。
func TestListFilesListsWorkTree(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("list"))
	ctx := context.Background()

	got, err := d.ListFiles(ctx, id)
	if err != nil {
		t.Fatalf("ListFiles 失败：%v", err)
	}
	if len(got) != 0 {
		t.Fatalf("新沙箱的 /work 该是空的，得到 %v", got)
	}

	if err := d.PutFile(ctx, id, "/work/b.txt", []byte("2")); err != nil {
		t.Fatalf("PutFile 失败：%v", err)
	}
	if err := d.PutFile(ctx, id, "/work/in/a.txt", []byte("1")); err != nil {
		t.Fatalf("PutFile 失败：%v", err)
	}
	got, err = d.ListFiles(ctx, id)
	if err != nil {
		t.Fatalf("ListFiles 失败：%v", err)
	}
	if len(got) != 2 || got[0] != "/work/b.txt" || got[1] != "/work/in/a.txt" {
		t.Fatalf("ListFiles = %v，想要 [/work/b.txt /work/in/a.txt]", got)
	}
}

// 对应 test_unknown_sandbox_id_is_rejected。
func TestUnknownSandboxIDIsRejected(t *testing.T) {
	d, _ := newSandbox(t)
	ctx := context.Background()
	unknown := strings.Repeat("deadbeef", 8)

	_, err := d.Exec(ctx, unknown, &pb.ExecRequest{Language: "python", Code: "print(1)", TimeoutSec: 10})
	requireKind(t, err, aiteerr.SandboxNotFound, "Exec 未知 id")

	_, err = d.ListFiles(ctx, unknown)
	requireKind(t, err, aiteerr.SandboxNotFound, "ListFiles 未知 id")
}

// 对应 test_release_is_idempotent。
func TestReleaseIsIdempotent(t *testing.T) {
	d, _ := newSandbox(t)
	task := taskID("release")
	id := mustAcquire(t, d, task)
	ctx := context.Background()

	if !contains(labelledIDs(t, task), id) {
		t.Fatalf("Acquire 之后没看到容器 %s", short(id))
	}
	if err := d.Release(ctx, id); err != nil {
		t.Fatalf("第一次 Release 失败：%v", err)
	}
	if err := d.Release(ctx, id); err != nil {
		t.Fatalf("第二次 Release 不许炸：%v", err)
	}
	if err := d.Release(ctx, "no-such-sandbox-id"); err != nil {
		t.Fatalf("压根不认识的 id 也不许炸：%v", err)
	}
	if contains(labelledIDs(t, task), id) {
		t.Fatalf("Release 之后容器 %s 还在", short(id))
	}
}

// 对应 test_touch_pushes_back_the_reaper：刷过之后就不该被 ReapIdle 收走。
func TestTouchPushesBackTheReaper(t *testing.T) {
	d, clk := newSandbox(t)
	task := taskID("touch")
	id := mustAcquire(t, d, task)
	ctx := context.Background()

	clk.advance(1200 * time.Millisecond)
	if err := d.Touch(ctx, id); err != nil {
		t.Fatalf("Touch 失败：%v", err)
	}
	released, err := d.ReapIdle(ctx, 1)
	if err != nil {
		t.Fatalf("ReapIdle 失败：%v", err)
	}
	if contains(released, id) {
		t.Fatalf("刚 Touch 过的沙箱被收走了：%v", released)
	}
	if !contains(labelledIDs(t, task), id) {
		t.Fatalf("容器 %s 不该没", short(id))
	}

	clk.advance(1200 * time.Millisecond)
	released, err = d.ReapIdle(ctx, 1)
	if err != nil {
		t.Fatalf("ReapIdle 失败：%v", err)
	}
	if !contains(released, id) {
		t.Fatalf("空闲够久了却没被收：released=%v", released)
	}

	if err := d.Touch(ctx, "no-such-sandbox-id"); err != nil {
		t.Fatalf("Touch 不认识的 id 是提示性动作，不许炸：%v", err)
	}
}

// 对应 test_reap_idle_keeps_fresh_sandboxes：刚用过的不能被收，否则任务跑一半容器就没了。
func TestReapIdleKeepsFreshSandboxes(t *testing.T) {
	d, _ := newSandbox(t)
	task := taskID("fresh")
	id := mustAcquire(t, d, task)

	released, err := d.ReapIdle(context.Background(), 300)
	if err != nil {
		t.Fatalf("ReapIdle 失败：%v", err)
	}
	if contains(released, id) {
		t.Fatalf("新鲜的沙箱被收走了：%v", released)
	}
	if !contains(labelledIDs(t, task), id) {
		t.Fatalf("容器 %s 不该没", short(id))
	}
}

// 对应 test_reap_idle_collects_orphans_from_another_instance：进程重启后留下的孤儿
// 容器（本进程没记账）也要能收 —— 这是 B4 那条断言的兜底。
func TestReapIdleCollectsOrphansFromAnotherInstance(t *testing.T) {
	d, clk := newSandbox(t)
	task := taskID("orphan")
	ctx := context.Background()

	stranger, err := NewDocker(testConfig())
	if err != nil {
		t.Fatalf("NewDocker 失败：%v", err)
	}
	orphan := mustAcquire(t, stranger, task)
	t.Cleanup(func() { _ = exec.Command("docker", "rm", "-f", orphan).Run() })
	stranger.forget(orphan) // 假装这是上一个进程留下的，本对象不认识它
	stranger.CloseAll()

	if !contains(labelledIDs(t, task), orphan) {
		t.Fatalf("孤儿容器 %s 没建起来", short(orphan))
	}

	// 孤儿的空闲是按容器时间戳算的，推时钟要盖过建容器本身花的那一秒多。
	clk.advance(30 * time.Second)
	released, err := d.ReapIdle(ctx, 1)
	if err != nil {
		t.Fatalf("ReapIdle 失败：%v", err)
	}
	if !contains(released, orphan) {
		t.Fatalf("孤儿没被收：released=%v", released)
	}
	if contains(labelledIDs(t, task), orphan) {
		t.Fatalf("孤儿容器 %s 还在", short(orphan))
	}
}

// B4 逐条：PNG 魔数 + ReapIdle(1) 后本 task 的容器为空。
func TestB4MatplotlibPNGAndReapLeavesNothing(t *testing.T) {
	d, clk := newSandbox(t)
	task := taskID("b4")
	id := mustAcquire(t, d, task)
	ctx := context.Background()

	res := mustExec(t, d, id, "import matplotlib.pyplot as plt\n"+
		"fig, ax = plt.subplots()\n"+
		"ax.plot([1, 2, 3], [120, 140, 90], marker='o')\n"+
		"ax.set_title('月度销量趋势')\n"+
		"ax.set_xlabel('月份')\n"+
		"ax.set_ylabel('销量（件）')\n"+
		"fig.savefig('/work/out.png')\n"+
		"print('saved')\n", 120)

	if res.GetExitCode() != 0 {
		t.Fatalf("exit_code = %d，stderr=%s", res.GetExitCode(), res.GetStderr())
	}
	if !strings.Contains(res.GetStdout(), "saved") {
		t.Fatalf("stdout = %q，想要含 saved", res.GetStdout())
	}
	// 中文字体真的生效了：缺字形的话 matplotlib 会往 stderr 刷 "Glyph … missing from font(s)"
	if strings.Contains(res.GetStderr(), "missing from font") {
		t.Fatalf("中文字形缺失：%s", res.GetStderr())
	}
	if !contains(filePaths(res.GetFilesOut()), "/work/out.png") {
		t.Fatalf("files_out = %v，想要含 /work/out.png", filePaths(res.GetFilesOut()))
	}

	png, err := d.GetFile(ctx, id, "/work/out.png")
	if err != nil {
		t.Fatalf("GetFile 失败：%v", err)
	}
	if len(png) < 8 || string(png[:8]) != string(pngMagic) {
		t.Fatalf("前 8 字节不是 PNG 魔数：% x", png[:min(8, len(png))])
	}
	if len(png) <= 1000 {
		t.Fatalf("PNG 只有 %d 字节，太小了", len(png))
	}

	clk.advance(30 * time.Second)
	released, err := d.ReapIdle(ctx, 1)
	if err != nil {
		t.Fatalf("ReapIdle 失败：%v", err)
	}
	if !contains(released, id) {
		t.Fatalf("B4：沙箱没被收，released=%v", released)
	}
	if left := labelledIDs(t, task); len(left) != 0 {
		t.Fatalf("B4：reap 之后 docker ps -a --filter label=%s=%s 还有 %v", LabelTask, task, left)
	}
}

// ------------------------------------------------------------------ 镜像的三条硬要求

// 镜像里四个库都在（docker/sandbox/requirements.txt 钉死的版本）。
func TestImageHasPythonDataStack(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("image-libs"))

	res := mustExec(t, d, id, "import pandas, matplotlib, openpyxl, docx\nprint('libs ok')\n", 60)
	if res.GetExitCode() != 0 {
		t.Fatalf("import 失败：exit=%d stderr=%s", res.GetExitCode(), res.GetStderr())
	}
	if !strings.Contains(res.GetStdout(), "libs ok") {
		t.Fatalf("stdout = %q", res.GetStdout())
	}
}

// 镜像里有中文字体：`fc-list :lang=zh` 非空。
func TestImageHasChineseFonts(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("image-fonts"))

	code, stdout, stderr, err := d.execRun(context.Background(), id, []string{"fc-list", ":lang=zh"}, Workdir)
	if err != nil {
		t.Fatalf("execRun 失败：%v", err)
	}
	if code != 0 {
		t.Fatalf("fc-list 退出码 %d：%s", code, stderr)
	}
	if strings.TrimSpace(stdout) == "" {
		t.Fatal("fc-list :lang=zh 是空的 —— 镜像里没有中文字体")
	}
}

// 沙箱出不去网：network=none 是运行期强制的，真拨一次号必须失败。
func TestImageHasNoNetworkEgress(t *testing.T) {
	d, _ := newSandbox(t)
	id := mustAcquire(t, d, taskID("image-nonet"))

	res := mustExec(t, d, id, "import urllib.request\nurllib.request.urlopen('https://example.com', timeout=3)\n", 20)
	if res.GetExitCode() == 0 {
		t.Fatalf("沙箱居然连上网了：stdout=%q", res.GetStdout())
	}
	if !strings.Contains(res.GetStderr(), "URLError") && !strings.Contains(res.GetStderr(), "urlopen error") {
		t.Fatalf("失败原因不像是没网：%s", res.GetStderr())
	}
}
