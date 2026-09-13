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
# 与 core 那份同一笔账（W3 ⑤ 量、AA1 修），**完整的病史、实测输出与三条决定写在
# docker/core/Dockerfile 的同名小节里**，这里只记不一样的那半。
#
# ==== W3 当时写的「两个 service 不必一视同仁，core 先降」——**2026-09-13 证伪** ====
#
# 证伪它的是一条 unix socket 的权限语义（实测在 core 那份里）：`connect()` 要的是
# socket 文件的**写权限**，而两边建出来的都是 `srwxr-xr-x`，只有属主有写位。
# core 要连 edge 的三个服务、edge 要连 core 的 IngressService —— **两个方向都要**，
# 所以两个进程只能是同一个 uid。只降 core 的形态起不来（症状是「core 起来了但连不上
# edge」，不是「起不来」），edge 没有「这一轮先不动」这个选项。
#
# ==== 不一样的那半：/var/run/docker.sock ====
#
# edge 要读 `/var/run/docker.sock`（沙箱是它按任务起的兄弟容器）。那个 socket 不归
# 跑 edge 的这个 uid，所以降权之后必须把它的**组**加进来，否则连不上 daemon ——
# 而 daemon 连不上 = `Ping` 失败 = `EdgeStatus.sandbox_ok` false = preflight 第 6 组 FAIL。
#
# 做法在 docker-compose.yml：edge 那个 service 多一条
# `group_add: ["${AITE_DOCKER_GID:-0}"]`。**这个 gid 每台宿主机不一样，是真代价，
# 不是走个形式**：
#   · Docker Desktop（本机实测）：容器里看到的是 `0:0 660 socket` ——
#     归 **root:root**，所以默认值 0 在这一档正好是对的；
#   · Linux：一般是 `root:docker`，docker 组的 gid 各机器不同（常见 999/998）。
#     **默认值 0 在 Linux 上是错的** —— 要么从 socket 自己问出来
#     （`stat -c '%g' /var/run/docker.sock`，CI 就是这么传的），要么 `sandbox_ok` 是 false。
#   · 实测过这条链是真管用的：`--user 1001:1001` 直连 docker.sock 是
#     `Permission denied`；加上 `--group-add 0` 之后退 0。
#
# 换一台机器还跑不跑得起来，答案就写在这儿：**跑得起来，但要带一个环境变量**。
# 不带的后果不是静默的 —— `aite preflight` 第 6 组会 FAIL 并点名沙箱不可用。
#
# ==== 命名卷 run: 的挂载点 ====
#
# 下面这行 `mkdir -p /app/data/run && chmod 1777` 与 core 那份**必须逐字一致**。
# 理由（Docker 会把镜像里那个目录的属主/权限位抄到空命名卷上，且只要卷还空着就每挂
# 一次抄一次）写在 core 那份的第 3 条。两份不一致的话，卷的属主就取决于「谁先起」——
# 而 compose 不保证顺序（刻意不写 depends_on）。
RUN mkdir -p /app/data/run && chmod 1777 /app/data/run
USER 1001:1001

# 直接 exec 二进制，不过 shell。
ENTRYPOINT ["aite-edge"]
CMD ["--config", "config/aite.yaml"]
