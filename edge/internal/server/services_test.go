package server

import (
	"context"
	"net"
	"path/filepath"
	"testing"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/status"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// 私有替身：只为验直通与错误映射，不模拟任何平台行为。
type fakePlatform struct {
	PlatformPort
	sent []*pb.OutboundText
}

func (f *fakePlatform) Capabilities() *pb.PlatformCapabilities {
	return &pb.PlatformCapabilities{Platform: "fake", SupportsThread: true}
}

func (f *fakePlatform) SendText(_ context.Context, msg *pb.OutboundText) (*pb.SendResult, error) {
	f.sent = append(f.sent, msg)
	return &pb.SendResult{MessageId: "om_1"}, nil
}

func (f *fakePlatform) ReadHistory(_ context.Context, _ string, limit int, _ *string) ([]*pb.HistoryMessage, error) {
	out := make([]*pb.HistoryMessage, 0, limit)
	for i := 0; i < limit; i++ {
		out = append(out, &pb.HistoryMessage{MessageId: "m"})
	}
	return out, nil
}

func (f *fakePlatform) UpdateCard(context.Context, string, *pb.ChecklistCard) error {
	return &aiteerr.PlatformError{Code: "230011", HTTPStatus: 404, Msg: "card gone"}
}

type fakeSandbox struct {
	SandboxPort
}

func (fakeSandbox) Acquire(context.Context, string, *pb.SandboxSpec) (string, error) {
	return "", &aiteerr.SandboxError{Kind: aiteerr.SandboxUnavailable, Msg: "no docker"}
}

func (fakeSandbox) GetFile(context.Context, string, string) ([]byte, error) {
	return nil, aiteerr.ErrNotImplemented
}

type fakeStatus struct{}

func (fakeStatus) Status(context.Context) *pb.EdgeStatus {
	return &pb.EdgeStatus{ContractVersion: ContractVersion, Platform: "fake"}
}

func dial(t *testing.T, fp *fakePlatform) *grpc.ClientConn {
	t.Helper()
	sock := filepath.Join(t.TempDir(), "edge.sock")
	lis, err := ListenUnix(sock)
	if err != nil {
		t.Fatal(err)
	}
	srv := New(fp, fakeSandbox{}, fakeStatus{}, 4)
	go func() { _ = srv.Serve(lis) }()
	t.Cleanup(srv.Stop)
	conn, err := grpc.NewClient("unix:"+sock, grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, addr string) (net.Conn, error) {
			var d net.Dialer
			return d.DialContext(ctx, "unix", sock)
		}))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	return conn
}

func TestPassthroughAndErrorMapping(t *testing.T) {
	fp := &fakePlatform{}
	conn := dial(t, fp)
	ctx := context.Background()
	plat := pb.NewPlatformServiceClient(conn)

	caps, err := plat.GetCapabilities(ctx, &pb.GetCapabilitiesRequest{})
	if err != nil || caps.GetPlatform() != "fake" {
		t.Fatalf("caps: %v %v", caps, err)
	}
	r, err := plat.SendText(ctx, &pb.OutboundText{ChatId: "c", Text: "hi", InThread: true})
	if err != nil || r.GetMessageId() != "om_1" || len(fp.sent) != 1 {
		t.Fatalf("send_text: %v %v", r, err)
	}
	// limit=0 → 50（proto 注释的约定）
	h, err := plat.ReadHistory(ctx, &pb.ReadHistoryRequest{ChatId: "c"})
	if err != nil || len(h.GetMessages()) != 50 {
		t.Fatalf("read_history default limit: %d %v", len(h.GetMessages()), err)
	}
	_, err = plat.UpdateCard(ctx, &pb.UpdateCardRequest{CardId: "x"})
	if status.Code(err) != codes.NotFound {
		t.Fatalf("update_card 404 → NOT_FOUND, got %v", err)
	}

	sb := pb.NewSandboxServiceClient(conn)
	_, err = sb.Acquire(ctx, &pb.AcquireRequest{TaskId: "t"})
	if status.Code(err) != codes.Unavailable {
		t.Fatalf("acquire unavailable → UNAVAILABLE, got %v", err)
	}
	_, err = sb.GetFile(ctx, &pb.GetFileRequest{SandboxId: "s", Path: "/work/x"})
	if status.Code(err) != codes.Unimplemented {
		t.Fatalf("stub → UNIMPLEMENTED, got %v", err)
	}
	// 接口里没覆盖的方法（嵌入 nil 接口）→ 直接 panic 会被 grpc 兜住成 Unknown；这里不测那条路。

	st, err := pb.NewEdgeStatusServiceClient(conn).GetStatus(ctx, &pb.GetStatusRequest{})
	if err != nil || st.GetContractVersion() != ContractVersion {
		t.Fatalf("status: %v %v", st, err)
	}
}
