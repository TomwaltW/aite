//! `run_python`（对应旧 `aite/tools/python_exec.py`）—— 在无网络沙箱里执行 Python，工作目录 `/work`。
//!
//! 两件容易搞混的事，这里定死：
//!
//! 1. **用户代码报错不是工具失败。** 代码抛异常、退出码非 0，工具照样 `ok=true`，
//!    traceback 原样进 `content` —— 模型看到就能自己改。反过来把它算成 `code=sandbox`
//!    会撞上 §3.3 的「连续 2 次沙箱失败 → task failed」，两个语法错就把任务打死了。
//!    `code=sandbox` 只留给沙箱本身出问题（Docker 不可用、容器没了、写不进 /work）。
//!
//! 2. **超时是工具失败。** §3.3：「工具执行超时 → ToolResult(ok=false, code=timeout)」。
//!    代码的时限由沙箱在容器内用 coreutils `timeout` 强制执行，超时会把退出码统一成
//!    `EXEC_TIMEOUT_EXIT_CODE`(124)；这里见到 124 就翻成 timeout。Gateway 外层的
//!    `tokio::time::timeout` 是同一件事的兜底（Docker daemon 卡住时用），两条路都走到
//!    `code=timeout`，调用方看到的结果一致。
use aite_contracts::sandbox::EXEC_TIMEOUT_EXIT_CODE;
use aite_contracts::{ArtifactRef, ExecRequest, ExecResult, ToolContext};
use serde_json::{Map, Value, json};

use super::{ToolEnv, ToolFailure, ToolOutcome, WORKDIR, as_map, str_arg, u32_arg};

/// 与 §3.1 `gateway_tools()` 里 run_python 的 schema default 一致。
const DEFAULT_TIMEOUT_SEC: u32 = 120;

pub async fn run_python(
    env: ToolEnv,
    _ctx: ToolContext,
    args: Map<String, Value>,
) -> Result<ToolOutcome, ToolFailure> {
    let sandbox = env.sandbox()?;
    let code = str_arg(&args, "code").to_string();
    let timeout_sec = u32_arg(&args, "timeout_sec", DEFAULT_TIMEOUT_SEC);

    let sandbox_id = env
        .acquire_sandbox()
        .await
        .map_err(|e| ToolFailure::sandbox(format!("沙箱起不来：{e}")))?;

    let result = sandbox
        .exec(&sandbox_id, &ExecRequest::python(code, timeout_sec))
        .await
        .map_err(|e| ToolFailure::sandbox(format!("沙箱执行失败：{e}")))?;

    // 刷新空闲计时，别让 reaper 在任务中途收走；刷不动也不影响这次执行。
    if let Err(e) = sandbox.touch(&sandbox_id).await {
        tracing::debug!(sandbox_id = %sandbox_id, error = %e, "gateway.touch_failed");
    }

    if result.exit_code == EXEC_TIMEOUT_EXIT_CODE {
        return Err(ToolFailure::timeout(format!(
            "代码执行超过 {timeout_sec}s 上限，已在沙箱内终止"
        )));
    }

    let files_out: Vec<Value> = result
        .files_out
        .iter()
        .map(|f| json!({"path": f.path, "size": f.size}))
        .collect();
    let artifacts: Vec<ArtifactRef> = result
        .files_out
        .iter()
        .map(|f| ArtifactRef {
            path: f.path.clone(),
            title: f.path.rsplit('/').next().unwrap_or(&f.path).to_string(),
            mime: super::guess_mime(&f.path),
        })
        .collect();

    Ok(ToolOutcome::new(render(&result))
        .with_data(as_map(json!({
            "exit_code": result.exit_code,
            "stdout": result.stdout,
            "stderr": result.stderr,
            "truncated": result.truncated,
            "duration_ms": result.duration_ms,
            "files_out": files_out,
        })))
        .with_artifacts(artifacts))
}

fn render(result: &ExecResult) -> String {
    let mut parts: Vec<String> = Vec::new();
    if result.exit_code == 0 {
        parts.push(format!("执行成功（{} ms）", result.duration_ms));
    } else {
        parts.push(format!(
            "代码以退出码 {} 结束（{} ms）",
            result.exit_code, result.duration_ms
        ));
    }

    if result.stdout.trim().is_empty() {
        parts.push("stdout: （空）".to_string());
    } else {
        parts.push(format!("stdout:\n{}", result.stdout));
    }
    if !result.stderr.trim().is_empty() {
        parts.push(format!("stderr:\n{}", result.stderr));
    }
    if result.truncated {
        parts.push("注意：输出太长已被截断。".to_string());
    }

    if result.files_out.is_empty() {
        parts.push(format!("本次没有在 {WORKDIR} 下产出文件。"));
    } else {
        let listing: Vec<String> = result
            .files_out
            .iter()
            .map(|f| format!("  {}（{} 字节）", f.path, f.size))
            .collect();
        parts.push(format!(
            "{WORKDIR} 下本次新增/修改的文件：\n{}",
            listing.join("\n")
        ));
    }

    parts.join("\n\n")
}
