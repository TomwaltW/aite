# Aite edge（Go）镜像 —— builder 编译，运行层只带两个二进制。
#
#   docker build -f docker/edge/Dockerfile -t aite-edge:p0 .
#   （context 是仓库根，跟 core 那份一致；edge 自己其实只要 edge/ 一个子树）
#
# edge 不需要 proto/：edge/gen/aitepb 是入库的生成码（Makefile 的 proto-gen 只有
# R0/RΩ 在 AITE_RELOCK=1 下重生成）。所以这里只 COPY edge/。

# ---------------------------------------------------------------- builder
ARG GO_VERSION=1.27.1
FROM golang:${GO_VERSION}-trixie AS builder

WORKDIR /src
COPY edge ./edge

# 两个 cache mount 顶掉「换一个容器就把 233M module 缓存重下一遍」
# （上一版 compose 只给了 GOCACHE 的卷，没给 /go/pkg/mod —— 实测每个新容器 12–14s）。
# 它们不进镜像层。
RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked \
    --mount=type=cache,target=/root/.cache/go-build,sharing=locked \
    cd edge && go build -o /out/aite-edge ./cmd/aite-edge

# grpc_health_probe 给 HEALTHCHECK 用：edge 的 gRPC server 已经注册了标准 health
# （edge/internal/server/server.go:38-40，SetServingStatus("", SERVING)），
# 与三个业务服务同一个 unix socket，探针支持 -addr=unix:///…。
# 版本钉死，别用 @latest：镜像要能重复构建出同一个东西。
# `go install pkg@version` 在主模块之外跑，不会动 edge/go.mod 与 go.sum。
ARG HEALTH_PROBE_VERSION=v0.4.57
RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked \
    --mount=type=cache,target=/root/.cache/go-build,sharing=locked \
    GOBIN=/out go install "github.com/grpc-ecosystem/grpc-health-probe@${HEALTH_PROBE_VERSION}"

# ---------------------------------------------------------------- 运行层
FROM debian:trixie-slim

# ca-certificates 是硬要求：edge 这一侧才是发飞书 HTTPS 的那个（open.feishu.cn，
# 出站 REST + 长连接握手）。Go 的 crypto/x509 读系统信任库，不装的话
# M1 的第一条事件就死在 TLS 上，而 CI 冒烟（platform=fake，不连飞书）不会红。
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# cwd 必须是仓库根，不是 /app/edge —— 否则两个 socket 会落到 /app/edge/data/run/
# （共享卷挂的是 /app/data/run），ListenUnix 的 MkdirAll 会把错路径静默建出来，
# 两个进程各拿各的 socket 永远连不上（台账 §2.1 的 blocker 4）。
WORKDIR /app

COPY --from=builder /out/aite-edge /usr/local/bin/aite-edge
COPY --from=builder /out/grpc-health-probe /usr/local/bin/grpc-health-probe

# ---------------------------------------------------------------- 以谁的身份跑
#
# **刻意没有 `USER`：edge 以 root 跑。** 与 core 那份同一笔账（W3 ⑤），
# 完整的实测输出与判据写在 docker/core/Dockerfile 的同名小节里，这里只记不一样的那半。
#
# 结论那一半是共享的：Linux 上 `./data` 与里面的产物都是 `root:root`（0755 / 0644），
# 宿主机**读得动**（`aite evidence show` 照常），但写不进、删不掉。
#
# 不一样的是**降权代价**：edge 要读 `/var/run/docker.sock`（沙箱是它按任务起的兄弟容器）。
# 那个 socket 在宿主机上归 `root:docker`，容器里换成非 root uid 之后要么加进 docker 组
# （组 gid 在不同宿主机上不一样，得在 compose 里传进来），要么压根连不上 daemon ——
# 而 daemon 连不上 = `Ping` 失败 = `EdgeStatus.sandbox_ok` false = preflight 第 6 组 FAIL。
# 所以**两个 service 不必一视同仁**：真要降权，core 先降，edge 这一侧得单独算一遍。

# 直接 exec 二进制，不过 shell。
ENTRYPOINT ["aite-edge"]
CMD ["--config", "config/aite.yaml"]
