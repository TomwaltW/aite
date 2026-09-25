#!/usr/bin/env bash
# Aite 云端环境安装脚本 —— 贴进 claude.ai/code 的环境设置（Setup script）。结果缓存约 7 天。
# 必需的三件（protoc / Rust / Go）失败就退非 0；可选的（codegen 插件、仓库预热、沙箱镜像）失败只告警，
# 免得一个镜像站抖一下就把整个环境的缓存搞坏。
# 官方约束（code.claude.com/docs/en/cloud-environments「Script requirements」「GitHub proxy」）：非 0 退出 = 会话起不来；
# 总时长压在约 5 分钟内环境缓存才建得成；setup 阶段下载「没附加到会话的仓库」的 GitHub release 会 403。
set -uo pipefail
SUDO=""; [ "$(id -u)" = 0 ] || SUDO="sudo"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
export GOTOOLCHAIN=auto
export PATH="${HOME:-/root}/.cargo/bin:/usr/local/go/bin:/usr/local/bin:$PATH"   # setup 阶段未必继承会话的 PATH

# ① protoc v31.1（与 CI 的 arduino/setup-protoc "31.x"、docker/core/Dockerfile 同源；core/crates/proto/build.rs 每次重编都要它）
#    先走 GitHub release；setup 阶段它可能 403（protocolbuffers/protobuf 没附加到会话），就退到 Maven Central 的同版本二进制
#    （protoc 4.31.1 = libprotoc 31.1，按官方 .sha1 校验）+ raw.githubusercontent.com 上 v31.1 的 WKT（events.proto 要 struct / timestamp）。
if ! protoc --version 2>/dev/null | grep -q 'libprotoc 31\.'; then
  command -v unzip >/dev/null || { $SUDO apt-get update -qq && $SUDO apt-get install -y -qq unzip; } || exit 1
  if curl -fsSL -o /tmp/protoc.zip \
      https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-linux-x86_64.zip; then
    $SUDO unzip -o -q /tmp/protoc.zip -d /usr/local bin/protoc 'include/*' || exit 1
  else
    echo "WARN: GitHub release 没下来，改走 Maven Central + raw.githubusercontent.com"
    curl -fsSL -o /tmp/protoc.bin \
      https://repo1.maven.org/maven2/com/google/protobuf/protoc/4.31.1/protoc-4.31.1-linux-x86_64.exe || exit 1
    echo "b419c80e305bc1ee74d2a303e0a3e90d2549b203  /tmp/protoc.bin" | sha1sum -c --quiet || exit 1
    $SUDO install -m 755 /tmp/protoc.bin /usr/local/bin/protoc || exit 1
    $SUDO mkdir -p /usr/local/include/google/protobuf/compiler || exit 1
    for f in any api descriptor duration empty field_mask source_context struct timestamp type wrappers compiler/plugin; do
      $SUDO curl -fsSL -o "/usr/local/include/google/protobuf/$f.proto" \
        "https://raw.githubusercontent.com/protocolbuffers/protobuf/v31.1/src/google/protobuf/$f.proto" || exit 1
    done
  fi
  $SUDO chmod 755 /usr/local/bin/protoc
fi
hash -r
protoc --version | grep -q 'libprotoc 31\.' || { echo "protoc 不是 31.x（PATH 上先命中的是别的 protoc）"; exit 1; }

# ② Rust 1.98.1 + clippy + rustfmt（core/rust-toolchain.toml 钉的版本）
if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none || exit 1
  . "$HOME/.cargo/env"
fi
rustup toolchain install 1.98.1 --profile minimal --component clippy,rustfmt || exit 1

# ③ Go ≥ 1.27：go.mod 的 go 指令 + GOTOOLCHAIN=auto 会自动拉 1.27.x（要求镜像自带的 go ≥ 1.21）
go version || exit 1
# T0 的 --codegen 自测要这两个插件（版本与 edge/gen 文件头一致）；装不上只告警
timeout 120 go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.12 || echo "WARN: protoc-gen-go 没装上"
timeout 120 go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2 || echo "WARN: protoc-gen-go-grpc 没装上"
GB="$(go env GOBIN)"; GB="${GB:-$(go env GOPATH)/bin}"
for p in protoc-gen-go protoc-gen-go-grpc; do [ -x "$GB/$p" ] && $SUDO ln -sf "$GB/$p" "/usr/local/bin/$p"; done
protoc-gen-go --version && protoc-gen-go-grpc --version || echo "WARN: codegen 插件不在 PATH 上，T0 的 --codegen 自测会报 program not found"

# ④ 仓库内预热（setup 若不在仓库里跑就跳过）。注意：只有 setup 与会话共用同一份 checkout 时 core/target 才留得住；
#    否则这里只暖了 ~/.cargo/registry 与 Go 模块缓存。CC1 实测「会话里首编是否仍是冷编译」并写进 docs/p1/cloud-runbook.md。
#    这里不跑 cargo test --no-run 预编译：冷编 10–15 分钟，远超 setup 约 5 分钟的上限（官方说会话从全新 clone 起跑，target 本来也未必留得住）。
if [ -d "$ROOT/core" ] && [ -d "$ROOT/edge" ]; then
  ( cd "$ROOT/edge" && timeout 120 go version && timeout 120 go mod download ) || echo "WARN: go mod download 没过"
  ( cd "$ROOT/core" && timeout 120 cargo fetch ) || echo "WARN: cargo fetch 没过"
  # ⑤ 可选：沙箱镜像（跑 -tags docker 那组才要）
  if docker info >/dev/null 2>&1; then
    timeout 150 docker build -q -t aite-sandbox:p0 "$ROOT/docker/sandbox" \
      || echo "WARN: 沙箱镜像没建成（多半是 deb.debian.org / pypi 不通），会话里要跑 docker 那组时再建"
  else
    echo "WARN: 此刻 docker daemon 不在，沙箱镜像留到会话里建"
  fi
else
  echo "WARN: 不在仓库目录里（ROOT=$ROOT），跳过预热"
fi
echo "cloud-setup 完成"
