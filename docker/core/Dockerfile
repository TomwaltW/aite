# Aite core（Rust）镜像 —— builder 编译，运行层只带一个二进制。
#
#   docker build -f docker/core/Dockerfile -t aite-core:p0 .
#   （context 必须是仓库根，见下面 COPY proto 那条）
#
# 为什么是多阶段：P0 上一版 compose 是「把仓库挂进去、在容器里现编」，代价有三处 ——
# 产物落进宿主机仓库的 core/target（两个 host triple 挤一个目录）、每次重启重编、
# 运行层里带着整条 Rust 工具链。这三条在这里一次解决。
#
# 编的是 **debug**（不是 --release）：对齐台账 §2.1 实测的那份起飞日志与
# docs/acceptance-M.md 里的 `core/target/debug/aite` 口径，也避免为了一个运维形态
# 去换优化档（§1 禁「顺手做一点 P1」）。

# ---------------------------------------------------------------- builder
ARG RUST_VERSION=1.98.1
FROM rust:${RUST_VERSION}-trixie AS builder

# rust 官方镜像装的是 minimal profile（rustc/cargo/rust-std，**没有 rustfmt 与 clippy**），
# 而 core/rust-toolchain.toml 的 components 里写着这两个 —— 不管的话 rustup 每次 build
# 都会去网上拉一遍这两个组件（build 日志里的 `downloading component`）。
# 镜像 tag 已经钉到 patch 位，rustc 就是 toolchain 文件要的那一版，所以用
# RUSTUP_TOOLCHAIN 直接指定，跳过 toolchain 文件的 components 安装。
# 镜像里从不跑 clippy / fmt —— 那两关归 scripts/check.sh 和 CI。
ARG RUST_VERSION
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}

# protoc：core/crates/proto/build.rs 必须要它。
# **不用 Debian 的 protobuf-compiler**（trixie 与 bookworm 都是 libprotoc 3.21.12）——
# 实测编不过：tonic-prost-build 只传自己那一个 -I，Debian 包不把 well-known types
# 放到 protoc 找得到的位置，于是 `google/protobuf/struct.proto: File not found`
# （events.proto:8 起一串）。所以照官方 release 装，版本与 CI 的
# arduino/setup-protoc@v3（version: "31.x"）同源、同一个 protocolbuffers/protobuf。
# 架构用 BuildKit 的 TARGETARCH，不用 `uname -m`：交叉 build 时 uname 报的是 builder 的。
ARG PROTOC_VERSION=31.1
ARG TARGETARCH
RUN set -eu; \
    case "${TARGETARCH}" in \
      arm64) arch=aarch_64 ;; \
      amd64) arch=x86_64 ;; \
      *) echo "不支持的 TARGETARCH=${TARGETARCH}（只接 arm64 / amd64）" >&2; exit 1 ;; \
    esac; \
    apt-get update; \
    apt-get install -y --no-install-recommends curl unzip; \
    rm -rf /var/lib/apt/lists/*; \
    curl -fsSL -o /tmp/protoc.zip \
      "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip"; \
    unzip -q /tmp/protoc.zip -d /usr/local bin/protoc 'include/*'; \
    rm /tmp/protoc.zip; \
    protoc --version

WORKDIR /src
# proto/ 是硬要求：build.rs 走 CARGO_MANIFEST_DIR/../../../proto，
# 只 COPY core/ 的话它当场炸（.dockerignore 里专门写了别排掉它）。
COPY proto ./proto
COPY core ./core

# CARGO_TARGET_DIR 指到 /target —— 仓库树之外。
# 三个 cache mount 让重复 build 只编改动的那部分；它们都不进镜像层，
# 所以运行层不会因为缓存变大，宿主机仓库也一个字节都不会被写到。
# 实测：改一行 Rust 源码后 `docker compose build core` 6.3s，什么都不改 0.7s。
#
# ⚠️ cache mount 会**掩盖缺失的编译期依赖**：把上面装 protoc 那一段删掉，在 cache mount
# 热的本机上 build 照样绿（cargo 复用了 aite-proto 已缓存的 build script 产物，压根没调
# protoc）；换一个缓存为空的 builder（`docker buildx create`，等价于 CI 的 runner）就红在
# `Could not find protoc` / exit 101。所以「本机 build 绿」不等于「依赖齐」——
# 判据在 CI 那一侧（.github/workflows/ci.yml 的 compose-smoke job，runner 每次都是冷的）。
ENV CARGO_TARGET_DIR=/target
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/target,sharing=locked \
    cd core && cargo build --workspace --locked \
    && mkdir -p /out && cp /target/debug/aite /out/aite

# ---------------------------------------------------------------- 运行层
FROM debian:trixie-slim

# ca-certificates 是硬要求，不是「顺手装的」：reqwest 配的是
# default-features=false, features=["json","rustls"]（core/Cargo.toml:48），
# 而依赖树里有 rustls-native-certs 0.8.4（core/Cargo.lock:2082）—— 它读**系统信任库**。
# 不装根证书的话 CI 冒烟（不发 HTTPS）根本不会红，**M1 第一次调模型端点才炸**。
# netcat-openbsd 给 HEALTHCHECK 用：core 的 ingress 没有 gRPC health service
# （edge-client/src/ingress.rs 只 add_service(IngressServiceServer)），
# 所以健康判据是「ingress socket connect 得上」，靠 `nc -U -z`。
# rusqlite 是 bundled（core/Cargo.toml:46），sqlite 静态进二进制，运行层不要 libsqlite3。
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates netcat-openbsd \
 && rm -rf /var/lib/apt/lists/*

# cwd 必须是仓库根：config 里的路径按契约全是相对仓库根的
# （worker.system_prompt_path / storage.* / edge.*_socket）。
WORKDIR /app

COPY --from=builder /out/aite /usr/local/bin/aite

# worker.system_prompt_path 的默认值是 core/crates/worker/prompts/platform.md
# （config/aite.example.yaml:37），相对仓库根。不再整仓挂载之后它得有个来源：
# 这里**烤进镜像里同一个相对路径**，而不是改配置里的路径 —— 这样宿主机直跑
# （docs/acceptance-M.md §0.2）和容器里跑可以共用同一份 config/aite.yaml。
COPY core/crates/worker/prompts /app/core/crates/worker/prompts
# 样例配置随镜像走，方便 `docker compose run --rm core preflight --offline` 之类的干跑。
COPY config/aite.example.yaml /app/config/aite.example.yaml
# 十个 P0 场景：镜像里也能跑 B8（`docker compose run --rm core evals run evals/p0 ...`）。
COPY evals /app/evals

# 直接 exec 二进制，**不过 shell** —— blocker 1（登录 shell 洗掉 PATH）从形态上就没了。
ENTRYPOINT ["aite"]
CMD ["run", "--config", "config/aite.yaml"]
