package aiteerr

import (
	"errors"
	"fmt"
	"strings"
	"testing"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

func code(t *testing.T, err error) codes.Code {
	t.Helper()
	st, ok := status.FromError(err)
	if !ok {
		t.Fatalf("not a status error: %v", err)
	}
	return st.Code()
}

// 冻结映射表逐条钉住（proto/aite/v1/edge.proto 头注释）。
func TestPlatformErrorMapping(t *testing.T) {
	cases := []struct {
		in   *PlatformError
		want codes.Code
	}{
		{&PlatformError{Code: "99991400", HTTPStatus: 429, Retryable: true}, codes.Unavailable},
		{&PlatformError{Code: "500", HTTPStatus: 500, Retryable: true}, codes.Unavailable},
		{&PlatformError{Code: "network", Retryable: true}, codes.Unavailable},
		{&PlatformError{Code: "timeout", Retryable: true}, codes.DeadlineExceeded},
		{&PlatformError{Code: "230002", HTTPStatus: 400, Retryable: false}, codes.InvalidArgument},
		{&PlatformError{Code: "99991663", HTTPStatus: 403, Retryable: false}, codes.PermissionDenied},
		{&PlatformError{Code: "230011", HTTPStatus: 404, Retryable: false}, codes.NotFound},
		{&PlatformError{Code: "230001", HTTPStatus: 422, Retryable: false}, codes.FailedPrecondition},
	}
	for _, c := range cases {
		if got := code(t, ToStatus(c.in)); got != c.want {
			t.Errorf("%+v -> %v, want %v", c.in, got, c.want)
		}
	}
}

func TestSandboxErrorMapping(t *testing.T) {
	cases := map[SandboxErrKind]codes.Code{
		SandboxUnavailable:  codes.Unavailable,
		SandboxNotFound:     codes.NotFound,
		SandboxFileNotFound: codes.NotFound,
		SandboxInvalidPath:  codes.InvalidArgument,
		SandboxTimeout:      codes.DeadlineExceeded,
		SandboxInternal:     codes.Internal,
	}
	for kind, want := range cases {
		if got := code(t, ToStatus(&SandboxError{Kind: kind, Msg: "x"})); got != want {
			t.Errorf("%v -> %v, want %v", kind, got, want)
		}
	}
}

// core 侧（core/crates/proto/src/status.rs）靠 status.message 的**开头**区分
// 「沙箱没了」和「文件没了」：两者都是 NOT_FOUND，只有前缀能分开。
// Error() 会在 Msg 前面贴 kind token，所以这里钉的是真正上线路的那一整串。
func TestSandboxNotFoundAndFileNotFoundAreToldApartOnTheWire(t *testing.T) {
	box := ToStatus(&SandboxError{Kind: SandboxNotFound, Msg: "sb-1"})
	file := ToStatus(&SandboxError{Kind: SandboxFileNotFound, Msg: "/work/out.png"})

	stBox, _ := status.FromError(box)
	stFile, _ := status.FromError(file)
	if stBox.Code() != codes.NotFound || stFile.Code() != codes.NotFound {
		t.Fatalf("两者都该是 NotFound：%v / %v", stBox.Code(), stFile.Code())
	}
	if !strings.HasPrefix(stBox.Message(), "sandbox_not_found:") {
		t.Errorf("沙箱不存在的 message = %q", stBox.Message())
	}
	if !strings.HasPrefix(stFile.Message(), "file_not_found:") {
		t.Errorf("文件不存在的 message = %q，core 会把它误判成「沙箱没了」", stFile.Message())
	}
	// 反过来也钉一下：文件那条绝不能以 sandbox_not_found 开头，否则前缀判定就废了。
	if strings.HasPrefix(stFile.Message(), "sandbox_not_found") {
		t.Errorf("file_not_found 被 kind token 顶掉了：%q", stFile.Message())
	}
}

func TestMiscMapping(t *testing.T) {
	if ToStatus(nil) != nil {
		t.Fatal("nil must stay nil")
	}
	if got := code(t, ToStatus(fmt.Errorf("wrap: %w", ErrNotImplemented))); got != codes.Unimplemented {
		t.Errorf("not implemented -> %v", got)
	}
	if got := code(t, ToStatus(errors.New("boom"))); got != codes.Internal {
		t.Errorf("plain error -> %v", got)
	}
	orig := status.Error(codes.Aborted, "keep")
	if ToStatus(orig) != orig {
		t.Error("status errors must pass through untouched")
	}
	// message 形如 "<code>: <detail>"
	st, _ := status.FromError(ToStatus(&PlatformError{Code: "230011", HTTPStatus: 404, Msg: "msg not found"}))
	if st.Message() != "230011: msg not found" {
		t.Errorf("message = %q", st.Message())
	}
}
