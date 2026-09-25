# 云端会话运行手册（Claude Code cloud · Aite）

> CC1 在 2026-09-25 实测（会话分支 `claude/intelligent-curie-nmk7nb`，B0 = `c159d12`）。安装脚本本体见 `scripts/cloud-setup.sh`
> （= 计划 §4.1 修订版逐字）；验收口径见仓库根 `CLAUDE.md`「## 验收」「## 云端基线」。

## 1. 环境实测

| 项 | 值 |
|---|---|
| `uname -m` | `x86_64` |
| `nproc` | `4` |
| 内存 | 15 GiB，无 swap |
| 磁盘 | `/dev/vda` 显示 252G，**实际是按会话计的额度**（本会话约 38G：起步已用 8.6G）。一份 `core/target` 就有 15G，**装不下第二份**（本会话在仓库副本里全量编译时撞过 ENOSPC） |
| 运行身份 | **`id -u` = 0（root）** —— 见 §5 |
| docker | CLI 29.3.1、compose v5.1.1、buildx v0.31.1 都在；**daemon 默认没起**（`/var/run/docker.sock` 不存在）。`dockerd > /tmp/dockerd.log 2>&1 &` 手动起得来（overlayfs、containerd snapshotter） |
| protoc | `libprotoc 31.1` |
| rustc | `rustc 1.98.1 (48a229cea 2026-09-01)`（在 `core/` 下，由 `rust-toolchain.toml` 钉） |
| 仓库根 `rustfmt --version` | `rustfmt 1.8.0-stable (e408947bfd 2026-03-25)` —— **仓库根有默认工具链**（stable），不是 1.98.1 那份；所以格式化一律 `(cd core && rustfmt --edition 2024 …)` |
| go | 镜像自带 `go1.24.7`；在 `edge/` 下经 `GOTOOLCHAIN=auto` 拉到 `go1.27.1` |
| codegen 插件 | `protoc-gen-go v1.36.12`、`protoc-gen-go-grpc 1.6.2`（setup 第 ③ 步软链进 `/usr/local/bin`） |
| 环境变量 | `GOTOOLCHAIN=auto`；`GOPROXY` 未设 |

## 2. 网络可达性（会话里 `curl -sSI --max-time 8 https://<host>/`）

| 主机 | exit | 备注 |
|---|---|---|
| deb.debian.org | 0 | |
| security.debian.org | 0 | |
| auth.docker.io | 0 | |
| production.cloudflare.docker.com | 0 | |
| pypi.org | 0 | 主机能到；**容器里** HTTPS 验不过证书，见下 |
| goproxy.cn | 56 | `CONNECT tunnel failed, response 403`（网络策略拒） |

另测（不在清单里，只记录）：`mirrors.tuna.tsinghua.edu.cn` / `mirrors.aliyun.com` / `mirrors.ustc.edu.cn` 走 http 都是代理回 403；
`mirrors.ustc.edu.cn` 的 https 是 exit 56；`mirror.gcr.io` 可达（`/v2/` 回 401 = 正常的 registry 握手）。

**两个会咬 docker 的坑**：

1. **Docker Hub 匿名拉取 429**（`toomanyrequests`，出口 IP 共享）：`docker run debian:trixie-slim`、沙箱镜像的
   `FROM python:3.11-slim` 都撞过。绕法：`docker pull mirror.gcr.io/library/<镜像> && docker tag mirror.gcr.io/library/<镜像> <镜像>`，
   本地有同名镜像后 `docker build` 不再去 Docker Hub 解析。或者直接 `--build-arg BASE_REGISTRY=mirror.gcr.io`（core / edge 两份 Dockerfile，见 §6）。
2. **出口是 TLS 拦截代理**（会话里的 CA 在 `/root/.ccr/ca-bundle.crt`），**容器里不信它**：容器内所有 HTTPS
   （pip 装沙箱依赖、curl 拉 protoc、cargo 拉 crates.io、Go 模块）都报 `self-signed certificate in certificate chain`；http 的 apt 不受影响。
   所以**在云端建不出沙箱镜像、core / edge 镜像的默认构建也过不了**，这不是 Dockerfile 的问题——CI 的 runner 没有这层代理，照常绿。
   只为本地验证时，可以用「每个 FROM 之后注入 CA」的**临时副本**（不入库）：
   `docker build -f <副本> --build-context ccr=/root/.ccr …`，副本里加
   `COPY --from=ccr ca-bundle.crt /etc/ssl/certs/ccr-bundle.crt` 与 `ENV SSL_CERT_FILE=… CURL_CA_BUNDLE=… CARGO_HTTP_CAINFO=…`（pip 用 `PIP_CERT`）。

## 3. `scripts/cloud-setup.sh` 两次实测

| 次 | `real` | 退出码 | 末行 | `WARN:` 行 |
|---|---|---|---|---|
| 1 | 0m4.288s | 0 | `cloud-setup 完成` | `WARN: 沙箱镜像没建成（多半是 deb.debian.org / pypi 不通），会话里要跑 docker 那组时再建` |
| 2 | 0m2.770s | 0 | `cloud-setup 完成` | 同上 |

- 两次都是「环境已装好」的快路径：protoc 已是 31.x、1.98.1 `unchanged`、插件已在，`cargo fetch` 只补了一批 windows / wasm 目标的 crate。
- WARN 的真实原因是 Docker Hub 429（`python:3.11-slim ... 429 Too Many Requests`），不是提示里说的 deb.debian.org / pypi；
  就算过了 429，也会卡在 §2 的 TLS 拦截（pip）。按设计只告警、不影响退出码。提示文案要不要改见 CC1 回执「记账转出去的」。
- 那次 WARN 能出现，是因为 CC1 先在会话里手动起了 dockerd；环境安装阶段 daemon 不在，走的是「此刻 docker daemon 不在」那支。
- 第 ③ 步在仓库根跑 `go version` 打的是 `go1.24.7`（仓库根没有 `go.mod`），第 ④ 步进 `edge/` 后才是 `go1.27.1`；`protoc-gen-go-grpc@v1.6.2`
  要 go ≥ 1.25，`GOTOOLCHAIN=auto` 自动切到 `go1.26.8` 装——正常。

**改 `scripts/cloud-setup.sh` 的人工步骤**：环境的 Setup script 是**贴进 claude.ai/code 环境设置里的一份副本**，不会自己跟着仓库变。
改了仓库这份之后，要由总管把全文重新贴回环境设置（会让约 7 天的环境缓存失效、下一个会话重建）。

## 4. 会话首编是否仍是冷编译：**是**

证据（开场自检、check.sh 之前）：

```
$ pwd; ls -d core/target/debug; du -sh ~/.cargo/registry
/home/user/aite
ls: cannot access 'core/target/debug': No such file or directory
du: cannot access '/root/.cargo/registry': No such file or directory
```

check.sh 的 A1 那格：有 `Compiling` 行、`Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1m 46s`（分钟级）。
setup 的预热（`cargo fetch` / `go mod download`）没有留到会话的 checkout 与 HOME 里——每个会话都从冷编起跑，冷启动 check.sh 约 7.5 分钟。

## 5. root 与只读类测试

云端会话是 uid 0。root 无视 `chmod`，7 条「把目录 / 库文件改成只读 → 断言写不进去」的测试确定性变红（`cargo passed=890 failed=7`）：
`aite --lib` 的 `preflight::tests` 三条、`aite --test cli_smoke` / `crash_recovery` / `preflight_e2e` 各一条、`aite-store --test crash_recovery` 一条。

`scripts/check.sh` 从 CC1 起在 `id -u` = 0 时，给「B 全量 cargo test」「B 全量 go test」两格套一层
`setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search --`：
进程仍是 uid 0、仍是 target 与 `~/.cargo` 的属主，只是权限位重新作数——与本机、CI（非 root）同口径，897/0。
**自己单跑这 7 条时也要带同一个前缀**，否则会看到假红。

## 6. 五个镜像构建参数（`make compose-build` 透传；EE14 写 deploy.md 时引用）

默认值全是上游，不传时与加它们之前逐字等价（CI 的 `docker compose build` 不经 make，一个字节不变）。
三处默认值一致：Makefile 的 `?=`、两份 Dockerfile 的 `ARG`；`docker-compose.yml` **没有**写 `build.args`（免得空串盖掉 Dockerfile 默认值）。

| 参数 | 默认 | 谁消费 | 国内取值（只写在这里，不当默认） |
|---|---|---|---|
| `BASE_REGISTRY` | `docker.io` | core / edge 的四个 `FROM ${BASE_REGISTRY}/library/…`；CC12 之后沙箱的 `FROM` | 私有 / 内网镜像仓库（如 Harbor 上的 `library` 项目代理 Docker Hub）；统一取值由 EE14 在 deploy.md 定。本会话实测 `mirror.gcr.io` 可用（海外） |
| `APT_MIRROR` | 空（= 不动 apt 源） | core builder、core 运行层、edge 运行层（CC12 之后还有沙箱） | `mirrors.tuna.tsinghua.edu.cn`、`mirrors.aliyun.com`（**主机名**，不是 URL；`debian-security` 跟着落到镜像上） |
| `PROTOC_URL` | `https://github.com/protocolbuffers/protobuf/releases/download` | core builder（前缀；`/v31.1/protoc-31.1-linux-<arch>.zip` 仍由 Dockerfile 拼） | 内网制品库里按同样目录结构放一份 release 的前缀 |
| `CARGO_REGISTRY` | 空（= crates.io） | core builder（写 `$CARGO_HOME/config.toml` 的 `[source.crates-io] replace-with`；对 `--locked` 透明） | `sparse+https://mirrors.ustc.edu.cn/crates.io-index/`；rsproxy 待大陆实测后再列 |
| `GOPROXY` | `https://proxy.golang.org,direct` | edge builder（BB3 那个 ARG） | `https://goproxy.cn,direct` |

用法：

```bash
make -n compose-build                                        # 看五个 --build-arg
make compose-build APT_MIRROR=mirrors.tuna.tsinghua.edu.cn GOPROXY=https://goproxy.cn,direct \
  CARGO_REGISTRY=sparse+https://mirrors.ustc.edu.cn/crates.io-index/
```

注意：

- make 会把同名环境变量读进来（`?=` 只在未定义时赋值）：shell 里 export 了 `GOPROXY`，`compose-build` 就用它。
- `docker compose --profile images build` 的全局 `--build-arg` 也发给 `sandbox-image`（沙箱 Dockerfile，CC12 的面）；没声明的参数 docker 只告警。
- `PIP_INDEX_URL`（CC12 的沙箱参数）不在这五个里，Makefile / compose 还没透传。
- `GOPROXY` 坏值只会红在 `go install grpc-health-probe@…`：`go build` 吃的是热的 cache mount，不查 proxy（BB3 注释里写过）。
- core builder 那层 `apt-get update;` 抓取失败只告警、不退非 0（apt 的默认行为，旧代码同样），而 rust 镜像里本来就有 `curl unzip`，
  所以 `APT_MIRROR` 写错时 core 红在**运行层**的 `apt-get install`（`E: Package 'ca-certificates' has no installation candidate`）。

## 7. check.sh 三次墙钟与各行

| 次 | 场合 | `real` | 结论 |
|---|---|---|---|
| 1 | 开场自检（冷编，旧 check.sh） | 7m30.350s | 没过：`cargo passed=890 failed=7`（root，见 §5） |
| 2 | 终版第一次（`core/target` 已清，冷编） | 5m58.476s | 全部通过（A1 `Finished … in 1m 49s`） |
| 3 | 终版第二次（热） | 0m42.042s | 全部通过（A1 `Finished … in 0.36s`） |

终版各行：`OK 25 files`、`contracts passed=25 failed=0`、`cargo passed=897 failed=0`、`go packages ok=9 fail=0`、`passed 10/10`、`B9 skip：evals/p1 尚无场景`。

### 抖动记录

本会话 check.sh 一共全量 / `--quick` 跑了 9 次（自检 1、验证 6、终版 2），外加仓库副本上 1 次：列在 `CLAUDE.md`「已知时序抖动」里的
`graceful_shutdown` / `startup_recovery` / `reconnect_replay` / `hold_spends_scheduler_ticks_not_wall_clock` / Go 的 `internal/ingress` 与 `cmd/aite-edge`（`-race`）
**一次都没抖**。唯一的红是 §5 的 root 问题（确定性，不是抖动）。
