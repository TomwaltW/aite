// Package aiteerr 定义 edge 内部的两类错误与它们到 gRPC status 的映射。
//
// 映射表是冻结契约（proto/aite/v1/edge.proto 头注释）：feishu / sandbox 两个包只产生
// 这两种错误，server 层统一用 ToStatus 翻译；core 侧按 status code 判 retryable。
// owner: R0。R1/R2 只读；需要新错误形状 → 停下报告。
package aiteerr

import (
	"errors"
	"fmt"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

// PlatformError 对应旧 aite/adapters/feishu/errors.py 的 PlatformError(code, retryable)。
type PlatformError struct {
	Code       string // 平台错误码（飞书 code 的十进制字符串）或短 token（"network" / "timeout"）
	HTTPStatus int    // 0 表示不是 HTTP 层错误
	Retryable  bool
	Msg        string
}

func (e *PlatformError) Error() string {
	return fmt.Sprintf("%s: %s", e.Code, e.Msg)
}

// SandboxErrKind 对应旧 aite/sandbox/errors.py 的错误分类。
type SandboxErrKind int

const (
	SandboxUnavailable SandboxErrKind = iota + 1 // docker daemon 不可达 / 镜像缺失
	SandboxNotFound                              // sandbox_id 不存在（已 release 或从未 acquire）
	// SandboxFileNotFound：沙箱在、文件不在。必须与 SandboxNotFound 分开：
	// core 侧靠 status.message 的**开头**区分两者（core/crates/proto/src/status.rs），
	// 而 Error() 会在 Msg 前面贴一层 token —— 所以「用 SandboxNotFound 配 file_not_found: 前缀」
	// 这种写法拼出来是 "sandbox_not_found: file_not_found: ..."，core 永远判成「沙箱没了」。
	// 语义上这条对应 Python 的 SandboxFileNotFound（多继承 FileNotFoundError）：
	// worker 靠它把「产物少一个」与「沙箱挂了」分开，前者跳过该产物照常交付。
	SandboxFileNotFound
	SandboxInvalidPath // 路径不在 /work 下或含 ..
	SandboxTimeout     // exec 超时（注意：exec 超时通常表达为 ExecResult.exit_code=124，不走这里）
	SandboxInternal
)

type SandboxError struct {
	Kind SandboxErrKind
	Msg  string
}

func (e *SandboxError) Error() string {
	return fmt.Sprintf("%s: %s", e.Kind.token(), e.Msg)
}

func (k SandboxErrKind) token() string {
	switch k {
	case SandboxUnavailable:
		return "sandbox_unavailable"
	case SandboxNotFound:
		return "sandbox_not_found"
	case SandboxFileNotFound:
		return "file_not_found"
	case SandboxInvalidPath:
		return "sandbox_invalid_path"
	case SandboxTimeout:
		return "sandbox_timeout"
	default:
		return "sandbox_internal"
	}
}

// ErrNotImplemented 骨架期占位：server 层翻译成 UNIMPLEMENTED。
var ErrNotImplemented = errors.New("not implemented")

// ToStatus 把任意 error 翻译成带 gRPC code 的 error。已经是 status 的原样返回。
func ToStatus(err error) error {
	if err == nil {
		return nil
	}
	if _, ok := status.FromError(err); ok {
		return err
	}
	if errors.Is(err, ErrNotImplemented) {
		return status.Error(codes.Unimplemented, "not_implemented: "+err.Error())
	}
	var pe *PlatformError
	if errors.As(err, &pe) {
		return status.Error(platformCode(pe), pe.Error())
	}
	var se *SandboxError
	if errors.As(err, &se) {
		return status.Error(sandboxCode(se), se.Error())
	}
	return status.Error(codes.Internal, "internal: "+err.Error())
}

func platformCode(pe *PlatformError) codes.Code {
	if pe.Retryable {
		if pe.Code == "timeout" {
			return codes.DeadlineExceeded
		}
		return codes.Unavailable
	}
	switch pe.HTTPStatus {
	case 400:
		return codes.InvalidArgument
	case 403:
		return codes.PermissionDenied
	case 404:
		return codes.NotFound
	default:
		return codes.FailedPrecondition
	}
}

func sandboxCode(se *SandboxError) codes.Code {
	switch se.Kind {
	case SandboxUnavailable:
		return codes.Unavailable
	case SandboxNotFound, SandboxFileNotFound:
		return codes.NotFound
	case SandboxInvalidPath:
		return codes.InvalidArgument
	case SandboxTimeout:
		return codes.DeadlineExceeded
	default:
		return codes.Internal
	}
}
