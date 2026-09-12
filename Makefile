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

.DEFAULT_GOAL := help
.PHONY: help build test lint lock c2 evals evals-list proto-gen docker-image \
        compose-config compose-build compose-up compose-down compose-ps compose-logs check clean

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

proto-gen:  ## 重生成 edge/gen/aitepb（只有 R0/RΩ 在 AITE_RELOCK=1 下跑；Rust 侧由 build.rs 自动生成）
	protoc -I proto --go_out=edge --go_opt=module=aite/edge --go-grpc_out=edge --go-grpc_opt=module=aite/edge proto/aite/v1/*.proto

docker-image:  ## 构建沙箱镜像（内容归 R2）
	docker build -t aite-sandbox:p0 docker/sandbox

# ---- compose：两个常驻进程的容器形态 -----------------------------------------
# 密钥清单只写在这一处，下面三个 target 复用它。`docker compose config` 会把 ${VAR}
# **解析成取值**打到 stdout，所以任何会被人看到输出的地方都必须先 env -u 清掉。
COMPOSE_NOSECRET := env -u FEISHU_APP_ID -u FEISHU_APP_SECRET -u FEISHU_BOT_OPEN_ID -u AITE_MODEL_API_KEY

compose-config:  ## compose 编排可解析（与 .github/workflows/ci.yml 同口径，不打印取值）
	$(COMPOSE_NOSECRET) docker compose config -q
	@$(COMPOSE_NOSECRET) docker compose config --services | sort | tr '\n' ' '; echo

compose-build:  ## 建三个镜像：core、edge、沙箱（沙箱在 images profile 里）
	docker compose --profile images build

compose-up: compose-build  ## 一条命令起飞：先把三个镜像建齐，再起两个常驻 service
	docker compose up -d
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
