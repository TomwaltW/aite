# Aite P0 常用命令（owner: T4，dev-spec §3.4 归属表）
#
# 每个 target 对应 §2 验收表里的一条，名字就是那条的编号，红了直接对得上。
#
# PYTHON 怎么挑：优先用 worktree 里的 .venv（本机默认 python3 是 3.11，满足不了
# requires-python>=3.12），其次 PATH 上的 python（CI 里 setup-python 提供的就是它），
# 最后退回 python3。想指定就 `make test PYTHON=/path/to/python`。
PYTHON ?= $(shell if [ -x .venv/bin/python ]; then echo .venv/bin/python; \
	elif command -v python >/dev/null 2>&1; then echo python; else echo python3; fi)
PYTEST := $(PYTHON) -m pytest
SUITE ?= evals/p0

.DEFAULT_GOAL := help
.PHONY: help install a2 a3 a4 a5 c1 c2 check test e2e contracts evals evals-list compose-config docker-image clean

help:  ## 列出所有 target
	@echo "Aite P0 —— 验收标准见 docs/dev-spec-2026-09-09.md §2"
	@echo "当前 PYTHON = $(PYTHON)"
	@echo
	@grep -E '^[a-zA-Z0-9_ -]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

install:  ## A1 装成 editable（含 dev 依赖）
	$(PYTHON) -m pip install -e ".[dev]"

a2:  ## A2 契约版本必须是 p0.1
	$(PYTHON) -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"

a3 c2:  ## A3/C2 契约锁校验
	$(PYTHON) -m aite.contracts.lock --check

a4:  ## A4 ruff
	$(PYTHON) -m ruff check .

a5:  ## A5 全仓可收集
	$(PYTEST) -q --co

c1 contracts:  ## C1 契约测试（通过数不得减少）
	$(PYTEST) tests/contracts -q

test:  ## 全量测试（跳过要 Docker 的）
	$(PYTEST) -q -m "not docker"

e2e:  ## T4 的替身与 runner 自测
	$(PYTEST) tests/e2e -q

evals:  ## B8 跑 10 个 P0 场景，最后一行 passed k/10
	$(PYTHON) -m aite.evals run $(SUITE) --platform fake --model scripted

evals-list:  ## 列出 §3.8 的 10 个场景名
	$(PYTHON) -m aite.evals run $(SUITE) --list

compose-config:  ## compose 编排可解析
	docker compose config

docker-image:  ## 构建沙箱镜像（内容归 T3）
	docker build -t aite-sandbox:p0 docker/sandbox

check: a2 a3 a4 a5 c1 e2e evals-list compose-config  ## 一次跑完 A2-A5 + C1 + T4 附加项
	@echo "全部通过"

clean:  ## 清掉缓存
	rm -rf .pytest_cache .ruff_cache
	find . -name __pycache__ -type d -prune -not -path "./.venv/*" -exec rm -rf {} +
