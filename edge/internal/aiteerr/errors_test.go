package aiteerr

import (
	"errors"
	"fmt"
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
		SandboxUnavailable: codes.Unavailable,
		SandboxNotFound:    codes.NotFound,
		SandboxInvalidPath: codes.InvalidArgument,
		SandboxTimeout:     codes.DeadlineExceeded,
		SandboxInternal:    codes.Internal,
	}
	for kind, want := range cases {
		if got := code(t, ToStatus(&SandboxError{Kind: kind, Msg: "x"})); got != want {
			t.Errorf("%v -> %v, want %v", kind, got, want)
		}
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
