#!/usr/bin/env bash
# 本地一把过：docs/dev-spec-2026-09-11-rustgo.md §4 的 A1–A5、C1/C2。
# 每条都打印实际输出的尾巴，不只报「通过了」—— 回执里要贴的就是这些行。
#
# 用法：scripts/check.sh                   # 全部
#       scripts/check.sh --quick           # 跳过 clippy（慢）
#       scripts/check.sh --docker          # 另跑真容器那组（要 docker daemon；可与 --quick 同给）
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/go/bin:/opt/homebrew/bin:$PATH"

QUICK=0
DOCKER=0
for a in "$@"; do
  case "$a" in
    --quick) QUICK=1 ;;
    --docker) DOCKER=1 ;;
    *) echo "不认识的参数：$a（只认 --quick / --docker）" >&2; exit 2 ;;
  esac
done
FAILED=()

# 两格全量测试（B cargo / B go）以 root 跑时要摘掉「无视文件权限位」的两个能力。
# 云端会话是 uid 0（2026-09-25 CC1 实测）：root 无视 chmod，7 条「把目录 / 库文件改成只读 →
# 断言写不进去」的测试（store / app 的 crash_recovery、preflight 那几条）确定性变红，
# 890/7 而不是 897/0。不换用户（target、~/.cargo 都归 root，换人就得跟着改权限），
# 只从 bounding set 里去掉 CAP_DAC_OVERRIDE / CAP_DAC_READ_SEARCH：进程仍是 uid 0、仍是
# 那些文件的属主，但权限位重新作数 —— 与本机、CI（非 root）同一个口径。非 root 时是空的。
NOROOT=()
if [ "$(id -u)" = 0 ]; then
  if command -v setpriv >/dev/null 2>&1; then
    NOROOT=(setpriv --bounding-set=-dac_override,-dac_read_search --inh-caps=-dac_override,-dac_read_search --)
  else
    echo "WARN: 以 root 跑、又没有 setpriv：只读类测试会假红（见上面那段注释）" >&2
  fi
fi

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
# 退出码同时看 cargo 自己的（c）：工作区**编不过**时一行 `^test result` 都没有，
# 只看 f 的话 `exit (f>0)` 得 0 —— 2026-09-25 云端首次 CC1 实测过这格假绿
# （`cargo passed= failed=` 却 `-> exit 0`）。NR==0 那一条兜「cargo 退 0 却一条结果都没有」。
# 失败名取 `---- <测试名> stdout ----` 那一行（`error: test failed` 只给到 target 一级）；
# 连同编译错误一共最多 7 行，第 8 行是计数，正好塞进 run() 的 8 行。
run "B 全量 cargo test"             ${NOROOT[@]+"${NOROOT[@]}"} bash -c 'cd core && o=$(cargo test --workspace --no-fail-fast 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^(---- .* stdout ----|error(: test failed|: could not compile|\[E[0-9]+\]))" | sort -u | head -n 7; printf "%s\n" "$o" | grep -E "^test result" | awk -v c="$c" "{p+=\$4; f+=\$6} END {print \"cargo passed=\" p+0 \" failed=\" f+0; exit (c != 0 || f > 0 || NR == 0)}"'
# -race 不是可选项：Go 侧的并发面（长连接重连、令牌桶、容器记账表、gRPC 并发读
# capabilities）都不是单线程的，而竞态在普通 go test 下完全隐形 —— 合流审核那次
# 就是靠它抓出 fakeFeishu 的读取侧没持锁（TestTransportErrorIsRetryable 现行）。
# 输出取舍照 cargo 那格：逐包的 ok 行不再显示（9 行原文本来就塞不进 run() 的 8 行，
# 排第一的 cmd/aite-edge 永远被截掉，红了也看不到名字），先打失败包（最多 5 个），末行计数。
# 口径：ok = `^ok` 行 + `^?` 行（`[no test files]` 也算，今天是 9）；
#       fail = `^FAIL<空白>aite/edge/` 行（含 `[build failed]`；结尾那个光秃秃的 `FAIL` 不算）。
# 这一格的退出码就是 go test 的退出码。
run "B 全量 go test（-race）"       ${NOROOT[@]+"${NOROOT[@]}"} bash -c 'cd edge && o=$(go test -race ./... -count=1 2>&1); c=$?; printf "%s\n" "$o" | grep -E "^FAIL[[:space:]]+aite/edge/" | head -n 5; ok=$(printf "%s\n" "$o" | grep -cE "^(ok|\?)[[:space:]]"); fail=$(printf "%s\n" "$o" | grep -cE "^FAIL[[:space:]]+aite/edge/"); echo "go packages ok=${ok} fail=${fail}"; exit "$c"'

# B8 自 RΩ 合入起是硬门禁：退出码 0 **且**最后一行逐字是 passed 10/10。
# 两条都要判 —— 只看退出码的话，将来 runner 把失败收成 phase="error" 却仍然 exit 0 就漏了。
run "B8 评测（passed 10/10）" bash -c 'o=$(core/target/debug/aite evals run evals/p0 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | grep -qx "passed 10/10"'

# B9：P1 场景（evals/p1，CC7 起往里放）。判据同 B8 的两条，只是总数不钉死：退出码 0 **且**
# 末行是 `passed k/k`（两数相等）。没有场景就 skip、不进 FAILED —— 必须先在 shell 里判空，
# 因为 load_suite 对「目录不存在」「一个 yaml 都没有」都报错（evals/src/scenario.rs 的 load_suite）。
# .yaml / .yml 两种都认，与 load_suite 的扩展名判定对齐。
if ls evals/p1/*.yaml >/dev/null 2>&1 || ls evals/p1/*.yml >/dev/null 2>&1; then
  run "B9 评测 evals/p1（passed k/k）" bash -c 'o=$(core/target/debug/aite evals run evals/p1 --platform fake --model scripted 2>&1); c=$?; printf "%s\n" "$o" | tail -n 2; [ "$c" = 0 ] && printf "%s\n" "$o" | tail -n 1 | awk "\$1 == \"passed\" && split(\$2, a, \"/\") == 2 && a[1] == a[2] {ok = 1} END {exit !ok}"'
else
  echo; echo "=== B9 评测 evals/p1 ==="; echo "B9 skip：evals/p1 尚无场景"
fi

# 真容器那组（--docker 才跑）：与 `make docker-test`、CI 的 sandbox-docker job 同口径 ——
# 建 aite-sandbox:p0 → -tags docker 的沙箱测试 → 不许留下带 aite.task 标签的容器。
# 测试红了也照样数遗留容器（两件事分开报）。
if [ "$DOCKER" = 1 ]; then
  run "D docker 组（-tags docker + 遗留容器 0）" bash -c 'docker build -q -t aite-sandbox:p0 docker/sandbox >/dev/null || { echo "沙箱镜像没建成"; exit 1; }; (cd edge && go test -tags docker ./internal/sandbox/... -count=1); c=$?; n=$(docker ps -a --filter label=aite.task -q | wc -l | tr -d " "); echo "遗留 aite.task 容器：${n}"; [ "$c" = 0 ] && [ "$n" = 0 ]'
fi

echo
if [ ${#FAILED[@]} -eq 0 ]; then echo "全部通过"; exit 0; fi
echo "以下没过：${FAILED[*]}"; exit 1
