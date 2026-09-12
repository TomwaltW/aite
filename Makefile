# Aite —— Rust（core/）+ Go（edge/）常用命令。验收编号对应 docs/dev-spec-2026-09-11-rustgo.md §4。
#
# 需要：rustup（core/rust-toolchain.toml 钉了版本）、go >= 1.27、protoc（只有 proto-gen 用）、docker（沙箱）。
# brew 装的 rustup 把 cargo 放在 /opt/homebrew/opt/rustup/bin，Go 插件在 ~/go/bin —— 都要在 PATH 上。

CARGO ?= cargo
GO ?= go
AITE := core/target/debug/aite
SUITE ?= evals/p0

.DEFAULT_GOAL := help
.PHONY: help build test lint lock c2 evals evals-list proto-gen docker-image compose-config check clean

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

compose-config:  ## compose 编排可解析
	docker compose config

check:  ## 一次跑完 A1–A5 + C1/C2（打印实际输出，回执贴这个）
	scripts/check.sh

clean:  ## 清掉构建产物
	cd core && $(CARGO) clean
	rm -rf edge/bin
