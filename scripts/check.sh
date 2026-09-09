#!/usr/bin/env bash
# 本地一把过：§2.1 的 A2–A5、§2.3 的 C1/C2，加上 §6 里 T4 那行的附加项。
# 每条都打印实际输出，不只报「通过了」—— 回执里要贴的就是这些行。
#
# 用法：
#   scripts/check.sh              # 自动挑解释器（优先 .venv/bin/python）
#   PYTHON=python3.12 scripts/check.sh
#   scripts/check.sh --with-docker   # 额外跑 docker compose config
set -uo pipefail

cd "$(dirname "$0")/.."

if [ -z "${PYTHON:-}" ]; then
  if [ -x .venv/bin/python ]; then
    PYTHON=.venv/bin/python
  elif command -v python >/dev/null 2>&1; then
    PYTHON=python
  else
    PYTHON=python3
  fi
fi

WITH_DOCKER=0
[ "${1:-}" = "--with-docker" ] && WITH_DOCKER=1

FAILED=()

run() {
  local label="$1"; shift
  echo
  echo "=== ${label} ==="
  echo "\$ $*"
  if "$@"; then
    echo "-> exit 0"
  else
    local code=$?
    echo "-> exit ${code}  ✗"
    FAILED+=("${label}")
  fi
}

echo "解释器：${PYTHON}  ($(${PYTHON} -V 2>&1))"

run "A2 契约版本（期望 p0.1）" "${PYTHON}" -c "import aite.contracts as c; print(c.CONTRACT_VERSION)"
run "A3/C2 契约锁"             "${PYTHON}" -m aite.contracts.lock --check
run "A4 ruff"                  "${PYTHON}" -m ruff check .
run "A5 全仓可收集"            "${PYTHON}" -m pytest -q --co
run "C1 契约测试"              "${PYTHON}" -m pytest tests/contracts -q
run "T4 替身自测"              "${PYTHON}" -m pytest tests/e2e -q
run "T4 场景清单"              "${PYTHON}" -m aite.evals run evals/p0 --list

# 评测本身在 TΩ 之前允许 passed k/10（k<10）退出码 1，所以不计入 FAILED，只打印出来看。
echo
echo "=== B8 评测（TΩ 前允许不满分）==="
echo "\$ ${PYTHON} -m aite.evals run evals/p0 --platform fake --model scripted"
"${PYTHON}" -m aite.evals run evals/p0 --platform fake --model scripted | tail -1

if [ "${WITH_DOCKER}" = "1" ]; then
  run "compose 配置" docker compose config
fi

echo
if [ ${#FAILED[@]} -eq 0 ]; then
  echo "全部通过"
  exit 0
fi
echo "以下没过：${FAILED[*]}"
exit 1
