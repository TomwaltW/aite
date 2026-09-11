package server

import (
	"context"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/aiteerr"
)

// PlatformService 是 PlatformPort → gRPC 的直通适配；不含任何业务判断。
type PlatformService struct {
	pb.UnimplementedPlatformServiceServer
	port PlatformPort
}

func NewPlatformService(p PlatformPort) *PlatformService { return &PlatformService{port: p} }

func (s *PlatformService) GetCapabilities(context.Context, *pb.GetCapabilitiesRequest) (*pb.PlatformCapabilities, error) {
	return s.port.Capabilities(), nil
}

func (s *PlatformService) SendText(ctx context.Context, msg *pb.OutboundText) (*pb.SendResult, error) {
	r, err := s.port.SendText(ctx, msg)
	return r, aiteerr.ToStatus(err)
}

func (s *PlatformService) SendCard(ctx context.Context, req *pb.SendCardRequest) (*pb.SendResult, error) {
	r, err := s.port.SendCard(ctx, req.GetChatId(), req.ReplyTo, req.GetCard())
	return r, aiteerr.ToStatus(err)
}

func (s *PlatformService) UpdateCard(ctx context.Context, req *pb.UpdateCardRequest) (*pb.UpdateCardResponse, error) {
	if err := s.port.UpdateCard(ctx, req.GetCardId(), req.GetCard()); err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.UpdateCardResponse{}, nil
}

func (s *PlatformService) SendFile(ctx context.Context, msg *pb.OutboundFile) (*pb.SendResult, error) {
	r, err := s.port.SendFile(ctx, msg)
	return r, aiteerr.ToStatus(err)
}

func (s *PlatformService) AddReaction(ctx context.Context, req *pb.AddReactionRequest) (*pb.AddReactionResponse, error) {
	if err := s.port.AddReaction(ctx, req.GetMessageId(), req.GetKind()); err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.AddReactionResponse{}, nil
}

func (s *PlatformService) ReadHistory(ctx context.Context, req *pb.ReadHistoryRequest) (*pb.ReadHistoryResponse, error) {
	limit := int(req.GetLimit())
	if limit <= 0 {
		limit = 50
	}
	msgs, err := s.port.ReadHistory(ctx, req.GetChatId(), limit, req.ThreadId)
	if err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.ReadHistoryResponse{Messages: msgs}, nil
}

func (s *PlatformService) ReadDocument(ctx context.Context, req *pb.ReadDocumentRequest) (*pb.DocumentContent, error) {
	d, err := s.port.ReadDocument(ctx, req.GetUrlOrToken())
	return d, aiteerr.ToStatus(err)
}

func (s *PlatformService) DownloadFile(ctx context.Context, req *pb.DownloadFileRequest) (*pb.DownloadFileResponse, error) {
	data, err := s.port.DownloadFile(ctx, req.GetMessageId(), req.GetFileKey())
	if err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.DownloadFileResponse{Data: data}, nil
}

// SandboxService 是 SandboxPort → gRPC 的直通适配。
type SandboxService struct {
	pb.UnimplementedSandboxServiceServer
	port SandboxPort
}

func NewSandboxService(p SandboxPort) *SandboxService { return &SandboxService{port: p} }

func (s *SandboxService) Acquire(ctx context.Context, req *pb.AcquireRequest) (*pb.AcquireResponse, error) {
	id, err := s.port.Acquire(ctx, req.GetTaskId(), req.GetSpec())
	if err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.AcquireResponse{SandboxId: id}, nil
}

func (s *SandboxService) Exec(ctx context.Context, req *pb.ExecCallRequest) (*pb.ExecResult, error) {
	r, err := s.port.Exec(ctx, req.GetSandboxId(), req.GetReq())
	return r, aiteerr.ToStatus(err)
}

func (s *SandboxService) PutFile(ctx context.Context, req *pb.PutFileRequest) (*pb.PutFileResponse, error) {
	if err := s.port.PutFile(ctx, req.GetSandboxId(), req.GetPath(), req.GetData()); err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.PutFileResponse{}, nil
}

func (s *SandboxService) GetFile(ctx context.Context, req *pb.GetFileRequest) (*pb.GetFileResponse, error) {
	data, err := s.port.GetFile(ctx, req.GetSandboxId(), req.GetPath())
	if err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.GetFileResponse{Data: data}, nil
}

func (s *SandboxService) ListFiles(ctx context.Context, req *pb.ListFilesRequest) (*pb.ListFilesResponse, error) {
	paths, err := s.port.ListFiles(ctx, req.GetSandboxId())
	if err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.ListFilesResponse{Paths: paths}, nil
}

func (s *SandboxService) Touch(ctx context.Context, req *pb.TouchRequest) (*pb.TouchResponse, error) {
	if err := s.port.Touch(ctx, req.GetSandboxId()); err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.TouchResponse{}, nil
}

func (s *SandboxService) Release(ctx context.Context, req *pb.ReleaseRequest) (*pb.ReleaseResponse, error) {
	if err := s.port.Release(ctx, req.GetSandboxId()); err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.ReleaseResponse{}, nil
}

func (s *SandboxService) ReapIdle(ctx context.Context, req *pb.ReapIdleRequest) (*pb.ReapIdleResponse, error) {
	ids, err := s.port.ReapIdle(ctx, int(req.GetIdleSec()))
	if err != nil {
		return nil, aiteerr.ToStatus(err)
	}
	return &pb.ReapIdleResponse{Released: ids}, nil
}

// StatusService 直通 StatusSource。
type StatusService struct {
	pb.UnimplementedEdgeStatusServiceServer
	src StatusSource
}

func NewStatusService(src StatusSource) *StatusService { return &StatusService{src: src} }

func (s *StatusService) GetStatus(ctx context.Context, _ *pb.GetStatusRequest) (*pb.EdgeStatus, error) {
	return s.src.Status(ctx), nil
}
