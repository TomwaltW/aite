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
# 计数之外还要把**失败的测试名**打出来 —— 只报 "failed=1" 的话，红了还得自己再跑一遍
# 全量才知道是哪条（2026-09-12 就这么丢过一次现场：main 上 717/1，名字没留下，
# 之后连跑六遍复现不出来）。失败名在前、计数在后，因为 run() 只 tail 最后 8 行。
run "B 全量 cargo test"             bash -c 'cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); printf "%s\n" "$o" | grep -E "^error: test failed" | sort -u | head -n 5; printf "%s\n" "$o" | grep -E "^test result" | awk "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p \" failed=\" f; exit (f>0)}"'
# -race 不是可选项：Go 侧的并发面（长连接重连、令牌桶、容器记账表、gRPC 并发读
# capabilities）都不是单线程的，而竞态在普通 go test 下完全隐形 —— 合流审核那次
# 就是靠它抓出 fakeFeishu 的读取侧没持锁（TestTransportErrorIsRetryable 现行）。
run "B 全量 go test（-race）"       bash -c 'cd edge && go test -race ./... -count=1'

# B8 自 RΩ 合入起是硬门禁：退出码 0 **且**最后一行逐字是 passed 10/10。
# 两条都要判 —— 只看退出码的话，将来 runner 把失败收成 phase="error" 却仍然 exit 0 就漏了。
run "B8 评测（passed 10/10）" bash -c 'o=$(core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | grep -qx "passed 10/10"'

echo
if [ ${#FAILED[@]} -eq 0 ]; then echo "全部通过"; exit 0; fi
echo "以下没过：${FAILED[*]}"; exit 1
