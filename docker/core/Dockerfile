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

# ---------------------------------------------------------------- 以谁的身份跑
#
# **刻意没有 `USER`：core 以 root 跑。** 这不是漏了，是 W3 ⑤ 量过之后留下的。
#
# 这条账原本的说法是「Linux 上 `data/evidence` 会是 root 拥有，宿主机那条
# `aite evidence show` 可能读不了」，而且只在 macOS 上验过 —— macOS 验不出来：
# Docker Desktop 的 bind mount 过 VirtioFS，会把 uid 映射回宿主用户（实测 owner
# 是当前用户，容器里明明是 root 建的）。
#
# Linux 上的实测（真镜像真起飞，`./data` 换成命名卷绕开 VirtioFS 的映射）：
#
#   0:0 755 directory    data/            ← ./data 不入库，bind mount 时由 dockerd 建
#   0:0 644 regular file data/aite.db     ← 证据链主存（storage.sqlite_path）
#   0:0 755 directory    data/evidence/
#   0:0 755 directory    data/artifacts/
#
# 以宿主机用户（GitHub runner 是 uid 1001）去碰同一批文件：
#   READ  OK      ← **原来那条担心不成立**：0644 + 0755 对任何用户都开着读，
#                    `aite evidence show` 在 Linux 上照常跑得通
#   WRITE DENIED
#   MKDIR DENIED  ← 但宿主机写不进、删不掉，也没法在 ./data 里新建
#
# 「读得动」这一条是硬门禁，钉在 .github/workflows/ci.yml 的 compose-smoke 里
# （把 data/aite.db 的权限位改成 0600，那一步就红并点名它 —— 本机实跑验过）。
#
# 为什么没顺手加 `USER`：写不进那半边确实是个真后果（Linux 上 compose 跑过一次之后，
# 宿主机直跑 `aite run` 会因为落盘目录不可写起不来），但修它的代价不在这一行 ——
# 命名卷 `run:` 的挂载点属主得跟着换（两个 socket 要建得出来）、`./data` 这条 bind
# mount 在宿主机那边是当前用户而容器里是新 uid。那笔账连后果一起记在
# review/review-findings-2026-09-12-vmerge.md 的 W3 回执里，不在这一轮的范围内。
# **真要加的话 core 是先降的那个**：它不碰 Docker daemon，代价比 edge 小得多。

# 直接 exec 二进制，**不过 shell** —— blocker 1（登录 shell 洗掉 PATH）从形态上就没了。
ENTRYPOINT ["aite"]
CMD ["run", "--config", "config/aite.yaml"]
