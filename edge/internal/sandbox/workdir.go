// /work 这个工作面的规矩（对应旧 aite/sandbox/workdir.py）。
//
// 契约（proto/aite/v1/sandbox.proto §SandboxSpec.workdir）只写了一句「path 必须在 /work 下」，
// 这里把它变成一个能被测试咬住的函数：路径必须是绝对路径，`.` / `..` 先在字符串层面归一，
// 归一后必须落在 workdir 里面。不做符号链接解析 —— 那要进容器才知道；
// 归一化是纯函数，进容器之前先挡一道。

package sandbox

import (
	"fmt"
	"strings"

	"aite/edge/internal/aiteerr"
)

// Workdir 是沙箱里唯一的工作面（SandboxSpec.workdir 的默认值，别漂）。
const Workdir = "/work"

// ExecTimeoutExitCode 是 coreutils `timeout` 超时时的退出码（POSIX 惯例）。
// Docker 沙箱用它统一表达「代码跑超时了」，Gateway 见到它就翻成 ToolErrorCode.timeout。
const ExecTimeoutExitCode = 124

func pathErr(format string, args ...any) *aiteerr.SandboxError {
	return &aiteerr.SandboxError{Kind: aiteerr.SandboxInvalidPath, Msg: fmt.Sprintf(format, args...)}
}

// NormalizeWorkPath 把沙箱内路径归一到 workdir 下的绝对路径；越界返回 InvalidPath。
// workdir 传空串等同 Workdir。
func NormalizeWorkPath(path, workdir string) (string, error) {
	if workdir == "" {
		workdir = Workdir
	}
	if strings.TrimSpace(path) == "" {
		return "", pathErr("路径不能为空：%q", path)
	}
	if !strings.HasPrefix(path, "/") {
		return "", pathErr("必须是绝对路径：%q", path)
	}

	parts := make([]string, 0, 8)
	for _, seg := range strings.Split(path, "/") {
		switch seg {
		case "", ".":
			// 连续斜杠与 `.` 都不产生新的一段。
		case "..":
			if len(parts) == 0 {
				return "", pathErr("路径越过了根目录：%q", path)
			}
			parts = parts[:len(parts)-1]
		default:
			parts = append(parts, seg)
		}
	}

	normalized := "/" + strings.Join(parts, "/")
	// 必须 == workdir 或真在它下面 —— 不能用 startswith(workdir) 判，`/workspace` 前缀像但不是。
	if normalized != workdir && !strings.HasPrefix(normalized, workdir+"/") {
		return "", pathErr("路径必须在 %s 下：%q（归一化后是 %s）", workdir, path, normalized)
	}
	return normalized, nil
}

// RequireFilePath 同 NormalizeWorkPath，但额外要求它是 workdir 下的一个**文件**路径，
// 不能就是 workdir 本身。
func RequireFilePath(path, workdir string) (string, error) {
	if workdir == "" {
		workdir = Workdir
	}
	normalized, err := NormalizeWorkPath(path, workdir)
	if err != nil {
		return "", err
	}
	if normalized == workdir {
		return "", pathErr("%s 是目录，不是文件", workdir)
	}
	return normalized, nil
}
