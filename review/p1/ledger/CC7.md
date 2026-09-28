情形 A（`git diff --stat --no-renames --diff-filter=AM 8458435 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'` 输出为空）

# 回执 CC7：P1 评测底座

> 状态：**进行中**（骨架先落，开 draft PR 让 CI 早跑；各节随工作项补齐）
> 分支：`claude/cc7-p1-eval-base` · 代码基线 `8458435`（HEAD `30b00e5` = 8458435 + D0 文档提交）

## 1. 开场自检原文

### 第 1 步：代码基线

```
$ git cat-file -e 8458435 || git fetch -q --unshallow origin
$ git diff --stat --no-renames --diff-filter=AM 8458435 HEAD -- . ':!review' ':!docs' ':!CLAUDE.md' ':!.gitignore'
（空）
$ git log --oneline -3
30b00e52 docs(p1): 第 1 波剩余 9 份派单刷新到 CC1–CC4 合并后的基线 8458435（946）+ T0 并入 W1 契约缺口
8458435c CC4: 网关插件注册表 + 策略钩子 + app 功能接缝 (#5)
a2e9377a CC3: 执行面拆分 + 助手回复入 transcript (#4)
```

→ 情形 A，基线行应为 `cargo passed=946 failed=0`、`contracts passed=25 failed=0`、`OK 25 files`、`passed 10/10`、Go 9 包全 ok。

### 第 2 步：守卫挂上了（Read 工具读守卫脚本，被拦 = 通过）

拦截原文逐字：

```
PreToolUse:Read hook error: [d=$(git rev-parse --show-toplevel 2>/dev/null); [ -f "$d/.claude/hooks/guard_bash.py" ] || d="$CLAUDE_PROJECT_DIR"; python3 "$d/.claude/hooks/guard_bash.py"]: blocked: 该操作触碰受保护面 .claude/hooks/guard_bash.py（读取位置）。停止当前工作并向人类报告。
```

### 第 3 步：工具链

```
$ protoc --version
libprotoc 31.1
$ (cd core && rustc --version)
rustc 1.98.1 (48a229cea 2026-09-01)
$ (cd edge && go version)
go version go1.27.1 linux/amd64
$ protoc-gen-go --version
protoc-gen-go v1.36.12
$ protoc-gen-go-grpc --version
protoc-gen-go-grpc 1.6.2
```

### 第 4 步：`scripts/check.sh`

后台跑 `scripts/check.sh > /tmp/cc7-check.log 2>&1; echo "exit=$?" >> /tmp/cc7-check.log`，末尾各格原文（全文见下「§4 check.sh 完整输出」一节的基线版）：

```
=== A3/C2 契约锁 --check ===
OK 25 files
=== C1 契约测试 ===
contracts passed=25 failed=0
=== B 全量 cargo test ===
cargo passed=946 failed=0
=== B 全量 go test（-race） ===
go packages ok=9 fail=0
=== B8 评测（passed 10/10） ===
}
passed 10/10
=== B9 评测 evals/p1 ===
B9 skip：evals/p1 尚无场景

全部通过
exit=0
```

与情形 A 基线逐格一致。CC1 已合并，Go 那一格已是新口径 `go packages ok=9 fail=0`（不再逐包列 `ok`），所以没有单跑 `cmd/aite-edge` 的必要 —— 以这行为准。
B9 门已在 check.sh 里（CC1 加的）：本轨落 `evals/p1/CC7_*.yaml` 之后它会跑 p1、要求 `passed 3/3`。

### 第 5 步（本轨追加）

```
$ ls evals/p1
ls: cannot access 'evals/p1': No such file or directory
$ core/target/debug/aite evals run evals/p0 --platform fake --model scripted
…
passed 10/10          （exit=0，stderr 0 字节）
```

另存了一份基线 `--protocol-report`，各场景的 `unknown_tools` 全是 `{}`（⑤ 前后对照用）。
