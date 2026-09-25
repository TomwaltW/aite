# Aite —— Rust（core/）+ Go（edge/）常用命令。验收编号对应 docs/dev-spec-2026-09-11-rustgo.md §4。
#
# 需要：rustup（core/rust-toolchain.toml 钉了版本）、go >= 1.27、protoc、docker。
# brew 装的 rustup 把 cargo 放在 /opt/homebrew/opt/rustup/bin，Go 插件在 ~/go/bin —— 都要在 PATH 上。
#
# protoc 不是「只有 proto-gen 用」：core/crates/proto/build.rs 每次重编都要它
# （实测把 /opt/homebrew/bin 从 PATH 里拿掉，`cargo build -p aite-proto` 当场报
# 「Could not find protoc」）。compose 那条路不吃宿主机的 protoc —— 镜像的 builder 阶段
# 自己装官方 release v31.1（docker/core/Dockerfile），与 CI 的 arduino/setup-protoc 同源。

CARGO ?= cargo
GO ?= go
AITE := core/target/debug/aite
SUITE ?= evals/p0

# ---- 镜像构建参数（2026-09-25 CC1）：`make compose-build` 总是把这五个作为 --build-arg 传下去 ----
# 默认值 = 上游，与两份 Dockerfile 的 ARG 默认值逐字一致，所以不传时建出来的东西与以前相同。
# 国内取值（私有 / 内网镜像仓库、goproxy.cn、tuna / aliyun 的 apt、ustc 的 sparse 源、内网制品库里的 protoc）
# 只写在 docs/p1/cloud-runbook.md，不当默认值。
# 注意 make 会把**同名环境变量**读进来（`?=` 只在未定义时赋值）：shell 里 export 了 GOPROXY 的话，
# compose-build 就用它 —— 这是有意的（本机已经配好的 Go 代理一并带进镜像）。
# `docker compose --profile images build` 的全局 --build-arg 也会发给 sandbox-image（CC12 的沙箱
# Dockerfile），所以 BASE_REGISTRY（默认 docker.io）/ APT_MIRROR（默认空 = 上游，非空 = 镜像主机名）
# 的名字、默认值、语义与那边一致；那份 Dockerfile 没声明的参数 docker 只告警、不报错。
BASE_REGISTRY ?= docker.io
GOPROXY ?= https://proxy.golang.org,direct
PROTOC_URL ?= https://github.com/protocolbuffers/protobuf/releases/download
CARGO_REGISTRY ?=
APT_MIRROR ?=

.DEFAULT_GOAL := help
.PHONY: help build test lint lock c2 evals evals-list evals-p1 proto-gen docker-image docker-test \
        compose-config compose-build compose-up compose-down compose-ps compose-logs check clean cloud-setup

help:  ## 列出所有 target
	@grep -E '^[a-zA-Z0-9_ -]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

build:  ## A1/A2 编译 core（Rust）与 edge（Go）
	cd core && $(CARGO) build --workspace
	cd edge && $(GO) build ./...

test:  ## B 全量测试：cargo test + go test -race（要 Docker 的 Go 测试用 -tags docker 单独跑）
	cd core && $(CARGO) test --workspace --no-fail-fast
	cd edge && $(GO) test -race ./... -count=1

lint:  ## A4 静态检查：clippy -D warnings、fmt --check、go vet、gofmt
	cd core && $(CARGO) clippy --workspace --all-targets -- -D warnings
	cd core && $(CARGO) fmt --check
	cd edge && $(GO) vet ./...
	@cd edge && test -z "$$(gofmt -l .)" || (echo "gofmt 未通过："; gofmt -l .; exit 1)

lock c2: build  ## A3/C2 契约锁校验（proto/** + core/crates/contracts/**）
	$(AITE) contracts lock --check

evals: build  ## B8 跑 P0 场景，最后一行 passed k/10
	$(AITE) evals run $(SUITE) --platform fake --model scripted

evals-list: build  ## 列出场景名
	$(AITE) evals run $(SUITE) --list

# 与 scripts/check.sh 的 B9 同一个判空条件：load_suite 对「目录不存在 / 没有 yaml」都报错，
# 所以先在 shell 里判，没场景就 skip 退 0。
evals-p1: build  ## B9 跑 P1 场景（evals/p1 没场景就打 skip 退 0）
	@if ls evals/p1/*.yaml >/dev/null 2>&1 || ls evals/p1/*.yml >/dev/null 2>&1; then \
		$(AITE) evals run evals/p1 --platform fake --model scripted; \
	else echo "B9 skip：evals/p1 尚无场景"; fi

proto-gen:  ## 重生成 edge/gen/aitepb（只有 R0/RΩ 在 AITE_RELOCK=1 下跑；Rust 侧由 build.rs 自动生成）
	protoc -I proto --go_out=edge --go_opt=module=aite/edge --go-grpc_out=edge --go-grpc_opt=module=aite/edge proto/aite/v1/*.proto

docker-image:  ## 构建沙箱镜像（内容归 R2）
	docker build -t aite-sandbox:p0 docker/sandbox

# 真容器那组：与 scripts/check.sh --docker、CI 的 sandbox-docker job 同口径。
# 测试红了也照样数遗留容器（两件事分开报），断言写法对齐 ci.yml「收干净」那一步。
docker-test: docker-image  ## 真容器那组：-tags docker 的沙箱测试 + 不许留下 aite.task 容器
	@cd edge && $(GO) test -tags docker ./internal/sandbox/... -count=1; c=$$?; \
	n=$$(docker ps -a --filter label=aite.task -q | wc -l | tr -d ' '); \
	echo "遗留 aite.task 容器：$$n"; \
	test "$$c" = 0 && test "$$n" = 0

cloud-setup:  ## 云端环境安装脚本（与 claude.ai/code 环境设置里贴的是同一份）
	bash scripts/cloud-setup.sh

# ---- compose：两个常驻进程的容器形态 -----------------------------------------
# 密钥清单只写在这一处，下面三个 target 复用它。`docker compose config` 会把 ${VAR}
# **解析成取值**打到 stdout，所以任何会被人看到输出的地方都必须先 env -u 清掉。
COMPOSE_NOSECRET := env -u FEISHU_APP_ID -u FEISHU_APP_SECRET -u FEISHU_BOT_OPEN_ID -u AITE_MODEL_API_KEY

compose-config:  ## compose 编排可解析（与 .github/workflows/ci.yml 同口径，不打印取值）
	$(COMPOSE_NOSECRET) docker compose config -q
	@$(COMPOSE_NOSECRET) docker compose config --services | sort | tr '\n' ' '; echo

compose-build:  ## 建三个镜像：core、edge、沙箱（沙箱在 images profile 里）；透传五个镜像参数
	docker compose --profile images build \
		--build-arg "BASE_REGISTRY=$(BASE_REGISTRY)" \
		--build-arg "GOPROXY=$(GOPROXY)" \
		--build-arg "PROTOC_URL=$(PROTOC_URL)" \
		--build-arg "CARGO_REGISTRY=$(CARGO_REGISTRY)" \
		--build-arg "APT_MIRROR=$(APT_MIRROR)"

# 两个容器以非 root 跑（2026-09-13 AA1 降权），代价是起飞前**宿主机这一侧**要先备三件事。
# 这一段与 .github/workflows/ci.yml 的「容器身份」那一步同源 —— 那边是 runner 版，这边是本机版。
# 2026-09-15（BB3）之前这条 target 一件都没做，Linux 上 `make compose-up` 因此起不来：
# core 死在 `aite 起不来：建不出目录 data/evidence：Permission denied`，配
# restart: unless-stopped 就是崩溃循环。macOS 撞不到（VirtioFS 双向翻译 uid）。
#
# ① `mkdir -p data` —— data/ 的**内容**不入库、**目录本身**入库（data/.gitkeep，见
#    .gitignore 那条 `/data/*` + `!/data/.gitkeep`）。两者覆盖的不是同一个时刻，所以都留着：
#    .gitkeep 管「这棵树是 git 给出来的」那一刻，这条 mkdir 管「目录后来没了」。
#    边界实测过（2026-09-15，一次性空仓）：`git clean -xdf` 之后 data/ 与 data/.gitkeep
#    都还在、里面别的全没了；`git archive HEAD | tar -t` 里 **data/.gitkeep 也在**
#    （所以「tarball 里没有它」那个说法是错的，别再写）。真正只有 mkdir 接得住的是：
#    有人手工把整个 data/ 删了（`git status` 会显示 ` D data/.gitkeep`）、树不是 checkout
#    出来的（rsync / 拷贝 / 别的打包方式）、或者哪天 .gitkeep 被谁清掉了。
#    这条命令幂等、一行、零成本，当保险留着。
#    目录不在的话 bind mount 时由 dockerd 建成 root:root 0755，非 root 容器当场写不进去。
#
# ② `AITE_UID` / `AITE_GID` —— 两个 service 的 `user:`。**必须解析出同一个 uid**：
#    两个进程互相 connect 对方的 unix socket，而 connect 要 socket 文件的写权限，
#    socket 是 `srwxr-xr-x`、只有属主有写位。问 `id` 自己，不写死。
#
# ③ `AITE_DOCKER_GID` —— 只有 edge 用（它要读 /var/run/docker.sock）。要的是 **daemon
#    那一侧**的取值，判别式就是「它是不是一个本机的真 socket」：
#      · 真 socket（Linux 的 docker-ce）：宿主机与容器同一个内核，bind mount 不翻译 uid/gid，
#        直接 `stat -c %g` 就是对的 —— 与 CI 逐字同源。本机能量到的那半证据：nsenter 进
#        Docker Desktop 的 Linux VM（`Linux 6.12.76-linuxkit`，真 Linux 内核）里看
#        /var/run/docker.sock 是 `0:0 660 socket`，而容器 bind-mount 进去看到的也是
#        `0:0 660 socket` —— 两侧一致。（gid 的取值本身在别人的 Linux 上是 root:docker，本机验不到。）
#      · 符号链接（macOS 的 Docker Desktop，实测指向 ~/.docker/run/docker.sock）：宿主机
#        stat 出来是 `0:1`（root:daemon），容器里看到的却是 `0:0` —— 直接 stat 会给出错的值。
#        这一档落回 0，也就是 docker-compose.yml 里那个默认值，Desktop 上它是对的。
#    `! -L` 那一半不是多余的：`-S` 对**指向** socket 的符号链接同样成立（test 跟随链接），
#    只用 `-S` 判别的话 macOS 会走进 stat 分支。而 `stat -c` 是 GNU 写法，macOS 的 BSD stat
#    报 `stat: illegal option -- c` 退 1 —— 所以再兜一层 `|| echo 0`，给「真 socket 但没有
#    GNU stat」的机器留条退路。
#
#    **刻意不用** AA1 记账里那条
#    `docker run --rm -v /var/run/docker.sock:... alpine stat -c %g /var/run/docker.sock`：
#    它测的确实是 daemon 视角、correct-by-construction，代价是给起飞命令加一条
#    **镜像 / registry 依赖**。本机实测：alpine 在缓存里时这一发 0.92s；不在时是
#    `Error response from daemon: failed to resolve reference docker.io/library/alpine...`
#    退 125 —— compose 一步都还没跑就死了。起飞命令不该因为拉不到一个探针镜像而失败。
#    真碰上判别不了的编排（远端 daemon、Linux 上的 Docker Desktop —— 那边 /var/run/docker.sock
#    也是个符号链接，会走进 `else` 拿到 0，而 Desktop 的 VM 里正好就是 0），
#    或者你就是想按 AA1 那条走，手动问一次再传进来：
#      AITE_DOCKER_GID=$$(docker run --rm -v /var/run/docker.sock:/var/run/docker.sock \
#        alpine stat -c %g /var/run/docker.sock) make compose-up
#
# 三个变量**外部传了就一律听外部的**（CI 就是从 $$GITHUB_ENV 传进来的），下面只在没传时兜底。
compose-up: compose-build  ## 一条命令起飞：建齐三个镜像 → 备好宿主机一侧 → 起两个常驻 service
	mkdir -p data
	@set -eu; \
	uid=$${AITE_UID:-$$(id -u)}; \
	gid=$${AITE_GID:-$$(id -g)}; \
	if [ -n "$${AITE_DOCKER_GID:-}" ]; then dgid=$$AITE_DOCKER_GID; \
	elif [ -S /var/run/docker.sock ] && [ ! -L /var/run/docker.sock ]; then \
		dgid=$$(stat -c '%g' /var/run/docker.sock 2>/dev/null || echo 0); \
	else dgid=0; fi; \
	echo "容器身份：AITE_UID=$$uid AITE_GID=$$gid AITE_DOCKER_GID=$$dgid"; \
	set -x; \
	AITE_UID=$$uid AITE_GID=$$gid AITE_DOCKER_GID=$$dgid docker compose up -d
	@docker compose ps

compose-down:  ## 停掉两个 service（要连命名卷一起收就自己加 -v，先确认没别人在用 project aite）
	docker compose down

compose-ps:  ## 看谁就绪（STATUS 一栏带 healthy / Restarting）
	docker compose ps

# 刻意**不加** --timestamps：两个进程都自己打时间戳（core 的 tracing、edge 的 slog），
# 加上去就是一行两个时间戳（实测只差 0.2ms，纯噪声）。
# 要 grep 或往回执里贴，再接一层剥 ANSI：core 的 tracing 默认开 ANSI，而 `--no-color`
# 只关 compose 那个 `core-1 |` 前缀的着色、管不到应用吐的字节 ——
#   docker compose logs --no-color core | perl -pe 's/\e\[[0-9;]*m//g'
compose-logs:  ## 跟两个进程的日志 —— docs/acceptance-M.md §0.3 的第一个观察窗
	docker compose logs -f --tail=200 core edge

check:  ## 一次跑完 A1–A5 + C1/C2（打印实际输出，回执贴这个）
	scripts/check.sh

clean:  ## 清掉构建产物
	cd core && $(CARGO) clean
	rm -rf edge/bin
