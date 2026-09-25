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

# ---- 镜像构建参数（2026-09-25 CC1；`make compose-build` 透传，国内取值见 docs/p1/cloud-runbook.md）----
# 五个都是「不传 = 上游、与加它们之前逐字等价」：CI 的 compose-smoke 直接 `docker compose build`，
# 不经 make，那条路一个字节都不能变（纪律同 docker/edge/Dockerfile 里 BB3 的 GOPROXY）。
#   BASE_REGISTRY  基础镜像仓库（默认 docker.io）。只在 FROM 行里用，所以只要这一条全局声明。
#   APT_MIRROR     空 = 不动 apt 源；非空 = 镜像**主机名**（不是 URL），deb.debian.org 与
#                  security.debian.org 都换成它（debian-security 那条跟着落到镜像上）。每个 FROM 之后重新声明。
#   PROTOC_URL     protoc release 的**前缀**，版本号与架构仍在下面拼（arm64 / amd64 都照旧）。
#   CARGO_REGISTRY 空 = crates.io；非空 = sparse 源（`sparse+https://…/`），写成
#                  [source.crates-io] replace-with —— 对 `--locked` 透明，Cargo.lock 一个字不变。
#   GOPROXY        只有 edge 用（BB3 那个 ARG），这里没有。
# 不用的构建参数 docker 只告警（"build-arg … not consumed"），不报错。
ARG BASE_REGISTRY=docker.io

# ---------------------------------------------------------------- builder
ARG RUST_VERSION=1.98.1
FROM ${BASE_REGISTRY}/library/rust:${RUST_VERSION}-trixie AS builder

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
ARG APT_MIRROR=
ARG PROTOC_URL=https://github.com/protocolbuffers/protobuf/releases/download
RUN set -eu; \
    case "${TARGETARCH}" in \
      arm64) arch=aarch_64 ;; \
      amd64) arch=x86_64 ;; \
      *) echo "不支持的 TARGETARCH=${TARGETARCH}（只接 arm64 / amd64）" >&2; exit 1 ;; \
    esac; \
    if [ -n "${APT_MIRROR}" ]; then \
      sed -i -e "s#deb\.debian\.org#${APT_MIRROR}#g" -e "s#security\.debian\.org#${APT_MIRROR}#g" \
        /etc/apt/sources.list.d/debian.sources; \
    fi; \
    apt-get update; \
    apt-get install -y --no-install-recommends curl unzip; \
    rm -rf /var/lib/apt/lists/*; \
    curl -fsSL -o /tmp/protoc.zip \
      "${PROTOC_URL:-https://github.com/protocolbuffers/protobuf/releases/download}/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip"; \
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
# CARGO_REGISTRY 非空时的配置写在 $CARGO_HOME（/usr/local/cargo）下、cache mount 之外，
# 只影响这一个 builder 阶段，运行层不带。
ARG CARGO_REGISTRY=
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/target,sharing=locked \
    if [ -n "${CARGO_REGISTRY}" ]; then \
      printf '[source.crates-io]\nreplace-with = "aite-mirror"\n\n[source.aite-mirror]\nregistry = "%s"\n' \
        "${CARGO_REGISTRY}" > "${CARGO_HOME}/config.toml"; \
    fi \
    && cd core && cargo build --workspace --locked \
    && mkdir -p /out && cp /target/debug/aite /out/aite

# ---------------------------------------------------------------- 运行层
FROM ${BASE_REGISTRY}/library/debian:trixie-slim
ARG APT_MIRROR=

# ca-certificates 是硬要求，不是「顺手装的」：reqwest 配的是
# default-features=false, features=["json","rustls"]（core/Cargo.toml:48），
# 而依赖树里有 rustls-native-certs 0.8.4（core/Cargo.lock:2082）—— 它读**系统信任库**。
# 不装根证书的话 CI 冒烟（不发 HTTPS）根本不会红，**M1 第一次调模型端点才炸**。
# netcat-openbsd 给 HEALTHCHECK 用：core 的 ingress 没有 gRPC health service
# （edge-client/src/ingress.rs 只 add_service(IngressServiceServer)），
# 所以健康判据是「ingress socket connect 得上」，靠 `nc -U -z`。
# rusqlite 是 bundled（core/Cargo.toml:46），sqlite 静态进二进制，运行层不要 libsqlite3。
RUN if [ -n "${APT_MIRROR}" ]; then \
      sed -i -e "s#deb\.debian\.org#${APT_MIRROR}#g" -e "s#security\.debian\.org#${APT_MIRROR}#g" \
        /etc/apt/sources.list.d/debian.sources; \
    fi \
 && apt-get update \
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
# ==== 病史：2026-09-12（W3 ⑤）量清楚了但刻意没修的那一轮，别删 ====
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
# 真后果：Linux 上 compose 跑过一次之后，宿主机直跑 `aite run` 会因为落盘目录不可写
# 起不来。AA1 把这条也量出来了（命名卷形态、以 uid 1001 跑 core 镜像自己）：
#   [7/7] FAIL 落盘目录可写   写不下去：data/aite.db（卡在 data）；data/evidence…
#   汇总：OK 1 · WARN 1 · FAIL 1 · SKIP 4，退出码 1
#
# ==== 2026-09-13（AA1）：降权那一轮做了什么 ====
#
# 上面那笔账销掉了。先说 W3 留下的那句话**错在哪**：它写的是「两个 service 不必
# 一视同仁，真要降权 core 先降」。**实测证伪** —— 两个 unix socket 把两个进程的身份
# 锁死在一起，只降一个的形态根本起不来：
#
#   connect() 一个 unix socket 要的是**该 socket 文件的写权限**，而两边建出来的
#   socket 都是 `srwxr-xr-x`（0777 & ~umask 022），**只有属主有写位**。
#   实测（root 建 socket，另一个容器去连）：
#     uid 1001 去连 → nc: /v/s.sock: Permission denied（退 1）
#     root     去连 → 退 0
#   core 要连 edge 的 PlatformService/SandboxService/EdgeStatusService，
#   edge 要连 core 的 IngressService —— 两个方向都要。所以**两个进程必须同一个 uid**。
#   只降 core 的症状正是「core 起来了但连不上 edge」，不是「起不来」。
#
# 于是这一轮 core 与 edge **一起**降到同一个非 root 身份。三条决定：
#
# 1. **uid/gid 取 1001:1001，但它只是默认值。** 1001 是 GitHub runner 那个已知锚点
#    （CI 里 `id -u` 就是它），拿来当 `docker run` 直跑时的安全默认。**真正的旋钮在
#    运行期**：docker-compose.yml 给两个 service 都写了
#    `user: "${AITE_UID:-1001}:${AITE_GID:-1001}"`。`./data` 是 bind mount，宿主机那边
#    的属主是**当前用户**，而当前用户的 uid 每台机器都不一样 —— 除了把它从宿主机传进来，
#    没有第二种让两边对上的办法。刻意**不做**成 build ARG：build 期定的 uid 和运行期
#    传的 uid 不一致是个纯陷阱，一个旋钮就够了。
#
# 2. **`COPY` 进来的东西不 chown。** `/app/core/crates/worker/prompts`、
#    `/app/config/aite.example.yaml`、`/app/evals` 运行期只读，默认就是 root:root
#    0644/0755 —— 对任何 uid 都开着读。chown 到 1001 反而会在 `user:` 被改成别的 uid
#    时读不了。同理 `/app` 本身留给 root。
#
# 3. **命名卷 `run:` 的挂载点：`mkdir -p /app/data/run` 再 `chmod 1777`，两个镜像口径
#    必须一致。** 这是最容易漏的一条，实测过 Docker 的语义才敢这么写：
#      · 空命名卷第一次被挂上时，**属主和权限位是从镜像里那个目录抄过来的**
#        （实测：镜像里 `/app/data/run` 是 1001:1001 → 卷根就是 1001:1001）；
#      · 只要卷里还没有文件，**每挂一次就抄一次**（实测：同一个空卷先挂 root 属主的
#        镜像变 0:0、再挂 1001 属主的镜像又变回 1001:1001）；卷里一有文件就冻住。
#      → 所以「谁先起」会决定卷的属主。edge 先起、而 edge 镜像里那个目录归 root 的话，
#        core 就再也建不出自己的 socket。两个镜像写同一份 mkdir+chmod 才没有这个竞态。
#      → 权限位取 **1777 而不是 chown 到某个 uid**：运行期 uid 是从宿主机传进来的、
#        build 期不知道，只有「谁都写得进 + 只能删自己的」这一档扛得住任意 uid。
#        这就是 /tmp 的语义。代价写在下一条。
#
# **代价，别装作没有：**
#   · 换 `AITE_UID` 之后必须 `docker compose down -v`。sticky 位只让属主删自己的文件，
#     上一个 uid 留下的残留 socket 新 uid 删不掉 —— core 会报「已经有人在监听」或
#     bind 失败，edge 会报清残留失败。是**响的**，但要知道该怎么收。
#   · Linux 上 `./data` 必须**先存在且归当前用户**。目录本身入库（data/.gitkeep）、
#     内容不入库（.gitignore 的 `/data/*` + `!/data/.gitkeep`；2026-09-15 BB3 改，
#     行号漂过一次，所以这里写措辞不写行号）；目录不在的话
#     不先建的话 dockerd 会替你建成 root:root 0755，非 root 的容器当场写不进去。
#     CI 在 up 之前 `mkdir -p data`；手起飞见 README 与 docs/acceptance-M.md §0.2.5。
#   · macOS 上这两条都撞不到，也验不出来：VirtioFS 双向翻译 uid（实测容器里看见的是
#     它自己的 uid、宿主机看见的是当前用户），换什么 uid 都写得进。
#
# 门禁在 .github/workflows/ci.yml 的 compose-smoke 里，两条都是**能被证伪的**：
#   · 「读得动」（W3 留的）—— 把 data/aite.db 的权限位改成 0600，那一步就红并点名它；
#   · 「写得进」（AA1 加的）—— 把 compose 的 `user:` 拿掉让容器退回 root，那一步就红。
#   两条都本机实跑验过。

# 运行层的默认身份。**别把它当成唯一的旋钮** —— compose 的 `user:` 会覆盖它，
# 见上面第 1 条。写成纯数字：`user:` 传进来的 uid 不会在 /etc/passwd 里有条目，
# 镜像里也就没必要 useradd 一个假装自己是主角的用户（实测纯数字 uid 跑 preflight /
# run / evals 都正常）。
RUN mkdir -p /app/data/run && chmod 1777 /app/data/run
USER 1001:1001

# 直接 exec 二进制，**不过 shell** —— blocker 1（登录 shell 洗掉 PATH）从形态上就没了。
ENTRYPOINT ["aite"]
CMD ["run", "--config", "config/aite.yaml"]
