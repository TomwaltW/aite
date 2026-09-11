// /work 路径规范化的纯单元测试（对应旧 tests/sandbox/test_sandbox_workdir.py 的 5 条）。
//
// 这一份不碰 Docker，所以**不打 docker tag**：规范化是 PutFile / GetFile 的第一道闸，
// 容器起不来时也该有人盯着它。

package sandbox

import (
	"errors"
	"testing"

	"aite/edge/internal/aiteerr"
)

// 对应 test_workdir_is_slash_work。
func TestWorkdirIsSlashWork(t *testing.T) {
	if Workdir != "/work" {
		t.Fatalf("Workdir = %q，想要 /work（SandboxSpec.workdir 的默认值，别漂）", Workdir)
	}
}

// 对应 test_exec_timeout_exit_code_is_posix_124。
func TestExecTimeoutExitCodeIsPosix124(t *testing.T) {
	if ExecTimeoutExitCode != 124 {
		t.Fatalf("ExecTimeoutExitCode = %d，想要 124（沙箱与 run_python 靠这个值对话）", ExecTimeoutExitCode)
	}
}

// 对应 test_paths_inside_work_are_normalized（参数化拆成子测试）。
func TestPathsInsideWorkAreNormalized(t *testing.T) {
	cases := []struct{ raw, want string }{
		{"/work", "/work"},
		{"/work/out.png", "/work/out.png"},
		{"/work/in/data.csv", "/work/in/data.csv"},
		{"/work/./out.png", "/work/out.png"},
		{"/work/a/../b/./c.png", "/work/b/c.png"},
		{"/work//double//slash.txt", "/work/double/slash.txt"},
	}
	for _, tc := range cases {
		t.Run(tc.raw, func(t *testing.T) {
			got, err := NormalizeWorkPath(tc.raw, "")
			if err != nil {
				t.Fatalf("NormalizeWorkPath(%q) 报错：%v", tc.raw, err)
			}
			if got != tc.want {
				t.Fatalf("NormalizeWorkPath(%q) = %q，想要 %q", tc.raw, got, tc.want)
			}
		})
	}
}

// 对应 test_paths_outside_work_are_rejected（参数化拆成子测试）。
func TestPathsOutsideWorkAreRejected(t *testing.T) {
	cases := []struct{ name, raw string }{
		{"绝对路径但不在 work 下", "/etc/passwd"},
		{"前缀像但不是", "/workspace/x"},
		{"相对路径", "work/x"},
		{"裸文件名", "out.png"},
		{"用 .. 爬出去", "/work/../etc/passwd"},
		{"多跳爬出去", "/work/a/../../etc/x"},
		{"越过根", "/../work/x"},
		{"空", ""},
		{"只有空白", "   "},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if _, err := NormalizeWorkPath(tc.raw, ""); !isInvalidPath(err) {
				t.Fatalf("NormalizeWorkPath(%q) 应当是 InvalidPath，得到 %v", tc.raw, err)
			}
		})
	}
}

// 对应 test_require_file_path_rejects_the_workdir_itself。
// 回归：workdir 带尾斜杠时曾把所有路径判成 InvalidPath（还报「必须在 /work/ 下」）。
func TestWorkdirWithTrailingSlashStillAccepts(t *testing.T) {
	for _, wd := range []string{"/work", "/work/", "/work//"} {
		got, err := NormalizeWorkPath("/work/out.png", wd)
		if err != nil {
			t.Fatalf("workdir=%q 时被拒了：%v", wd, err)
		}
		if got != "/work/out.png" {
			t.Errorf("workdir=%q → %q", wd, got)
		}
		if _, err := RequireFilePath("/work/in/a.csv", wd); err != nil {
			t.Errorf("workdir=%q 时 RequireFilePath 被拒：%v", wd, err)
		}
	}
}

func TestRequireFilePathRejectsTheWorkdirItself(t *testing.T) {
	if _, err := RequireFilePath(Workdir, ""); !isInvalidPath(err) {
		t.Fatalf("RequireFilePath(%q) 应当是 InvalidPath，得到 %v", Workdir, err)
	}
	got, err := RequireFilePath("/work/x", "")
	if err != nil {
		t.Fatalf("RequireFilePath(/work/x) 报错：%v", err)
	}
	if got != "/work/x" {
		t.Fatalf("RequireFilePath(/work/x) = %q，想要 /work/x", got)
	}
}

func isInvalidPath(err error) bool {
	var se *aiteerr.SandboxError
	return errors.As(err, &se) && se.Kind == aiteerr.SandboxInvalidPath
}
