#!/usr/bin/env bash
# 本地一把过：docs/dev-spec-2026-09-11-rustgo.md §4 的 A1–A5、C1/C2。
# 每条都打印实际输出的尾巴，不只报「通过了」—— 回执里要贴的就是这些行。
#
# 用法：scripts/check.sh            # 全部
#       scripts/check.sh --quick    # 跳过 clippy（慢）
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/go/bin:/opt/homebrew/bin:$PATH"

QUICK=0
[ "${1:-}" = "--quick" ] && QUICK=1
FAILED=()

run() {  # label, cmd...
  local label="$1"; shift
  echo; echo "=== ${label} ==="; echo "\$ $*"
  local out; out=$("$@" 2>&1); local code=$?
  printf '%s\n' "$out" | tail -n 8
  if [ $code -eq 0 ]; then echo "-> exit 0"; else echo "-> exit ${code}  ✗"; FAILED+=("${label}"); fi
}

run "A1 cargo build --workspace"   bash -c 'cd core && cargo build --workspace'
run "A2 go build ./..."            bash -c 'cd edge && go build ./...'
run "A3/C2 契约锁 --check"          core/target/debug/aite contracts lock --check
if [ "$QUICK" = 0 ]; then
  run "A4a cargo clippy -D warnings" bash -c 'cd core && cargo clippy --workspace --all-targets -- -D warnings'
fi
run "A4b cargo fmt --check"        bash -c 'cd core && cargo fmt --check'
run "A4c go vet"                   bash -c 'cd edge && go vet ./...'
run "A4d gofmt"                    bash -c 'cd edge && test -z "$(gofmt -l .)"'
run "A5 cargo test --no-run（全部测试可编译）" bash -c 'cd core && cargo test --workspace --no-run'
run "C1 契约测试"                   bash -c 'cd core && cargo test -p aite-contracts 2>&1 | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"contracts passed=\" p \" failed=\" f; exit (f>0)}"'
run "B 全量 cargo test"             bash -c 'cd core && cargo test --workspace --no-fail-fast 2>&1 | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p \" failed=\" f; exit (f>0)}"'
run "B 全量 go test"                bash -c 'cd edge && go test ./... -count=1'

# 评测本身在 RΩ 之前允许 not implemented / passed k/10，不计入 FAILED，只打印出来看。
echo; echo "=== B8 评测（RΩ 前允许不满分 / not implemented）==="
echo "\$ core/target/debug/aite evals run evals/p0 --platform fake --model scripted"
core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1 | tail -n 2

echo
if [ ${#FAILED[@]} -eq 0 ]; then echo "全部通过"; exit 0; fi
echo "以下没过：${FAILED[*]}"; exit 1
