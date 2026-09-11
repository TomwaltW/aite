//! FakeSandbox 自测（移植自 `tests/e2e/test_t4_fake_sandbox.py`，12 条）：
//! 内存 FS、exec 脚本、release 幂等、reap_idle 可确定性触发。
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aite_contracts::{ExecRequest, SandboxErrorKind, SandboxPort, SandboxSpec};
use aite_testing::{ExecScriptStep, FakeSandbox, PNG_MAGIC};
use serde_json::json;

fn spec() -> SandboxSpec {
    SandboxSpec::new("aite-sandbox:p0")
}

fn sandbox(script: Vec<serde_json::Value>) -> FakeSandbox {
    FakeSandbox::from_values(&script).expect("exec_script")
}

/// SandboxPort 的方法一个都不能少 —— Rust 里由 trait 实现保证；这条顺带钉 spec 默认值。
#[tokio::test]
async fn implements_every_sandbox_port_method() {
    let sb: Arc<dyn SandboxPort> = Arc::new(FakeSandbox::default());
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    assert_eq!(sid, "sb-1");
    assert!(sb.list_files(&sid).await.unwrap().is_empty());
    sb.touch(&sid).await.unwrap();
    assert!(sb.reap_idle(300).await.unwrap().is_empty());
    sb.release(&sid).await.unwrap();
    sb.close_all().await.unwrap();
}

#[tokio::test]
async fn acquire_put_get_list() {
    let sb = FakeSandbox::default();
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    sb.put_file(&sid, "/work/in/a.csv", b"month,amount")
        .await
        .unwrap();
    assert_eq!(
        sb.get_file(&sid, "/work/in/a.csv").await.unwrap(),
        b"month,amount"
    );
    assert_eq!(
        sb.list_files(&sid).await.unwrap(),
        vec!["/work/in/a.csv".to_string()]
    );
}

/// 契约 put_file 注释：path 必须在 /work 下。
#[tokio::test]
async fn put_file_outside_work_is_rejected() {
    let sb = FakeSandbox::default();
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    let err = sb.put_file(&sid, "/etc/passwd", b"x").await.unwrap_err();
    assert!(err.message.contains("/work"), "{}", err.message);
}

#[tokio::test]
async fn get_missing_file_reports_with_listing() {
    let sb = FakeSandbox::default();
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    let err = sb.get_file(&sid, "/work/out.png").await.unwrap_err();
    assert_eq!(err.kind, SandboxErrorKind::FileNotFound);
    assert!(
        err.message.contains("没有 /work/out.png"),
        "{}",
        err.message
    );
}

/// 04_csv_to_chart 的关键：代码里有 savefig 就产出一张真 PNG。
#[tokio::test]
async fn exec_script_matches_by_substring_and_writes_files() {
    let sb = sandbox(vec![json!({
        "match": "savefig", "stdout": "ok",
        "writes": {"/work/out.png": "builtin:png"}
    })]);
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    let res = sb
        .exec(
            &sid,
            &ExecRequest::python("plt.savefig('/work/out.png')", 120),
        )
        .await
        .unwrap();
    assert_eq!(res.exit_code, 0);
    assert_eq!(res.stdout, "ok");
    assert_eq!(
        res.files_out
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>(),
        vec!["/work/out.png".to_string()]
    );
    assert_eq!(
        &sb.get_file(&sid, "/work/out.png").await.unwrap()[..8],
        &PNG_MAGIC
    );
}

#[tokio::test]
async fn unmatched_code_gets_a_harmless_default() {
    let sb = sandbox(vec![json!({"match": "savefig", "stdout": "ok"})]);
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    let res = sb
        .exec(&sid, &ExecRequest::python("print(1)", 120))
        .await
        .unwrap();
    assert_eq!(
        (res.exit_code, res.stdout.as_str(), res.files_out.len()),
        (0, "", 0)
    );
}

#[tokio::test]
async fn exec_script_times_limits_reuse() {
    let sb = sandbox(vec![
        json!({"stdout": "第一次", "times": 1}),
        json!({"stdout": "之后"}),
    ]);
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    assert_eq!(
        sb.exec(&sid, &ExecRequest::python("x", 120))
            .await
            .unwrap()
            .stdout,
        "第一次"
    );
    assert_eq!(
        sb.exec(&sid, &ExecRequest::python("x", 120))
            .await
            .unwrap()
            .stdout,
        "之后"
    );
}

/// 「沙箱创建/执行失败」→ Gateway 要把它翻成 code=sandbox。
#[tokio::test]
async fn exec_error_step_reports_sandbox_error() {
    let sb = sandbox(vec![json!({"error": "Docker daemon 不可用"})]);
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    let err = sb
        .exec(&sid, &ExecRequest::python("x", 120))
        .await
        .unwrap_err();
    assert!(err.message.contains("Docker daemon"), "{}", err.message);
}

#[tokio::test]
async fn release_is_idempotent() {
    let sb = FakeSandbox::default();
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    sb.release(&sid).await.unwrap();
    sb.release(&sid).await.unwrap();
    assert_eq!(sb.released_ids(), vec![sid]);
    assert!(sb.alive().is_empty());
}

#[tokio::test]
async fn released_sandbox_cannot_be_used() {
    let sb = FakeSandbox::default();
    let sid = sb.acquire("task-1", &spec()).await.unwrap();
    sb.release(&sid).await.unwrap();
    let err = sb
        .exec(&sid, &ExecRequest::python("x", 120))
        .await
        .unwrap_err();
    assert!(err.message.contains("已经 release"), "{}", err.message);
}

/// 不用真等 5 分钟：时钟可注入，reap 的判定就可确定性触发。
#[tokio::test]
async fn reap_idle_uses_injectable_clock() {
    let now = Arc::new(AtomicU64::new(1000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst) as f64)
    };
    let sb = FakeSandbox::default().with_clock(clock);
    let idle = sb.acquire("task-idle", &spec()).await.unwrap();
    let busy = sb.acquire("task-busy", &spec()).await.unwrap();

    now.fetch_add(400, Ordering::SeqCst);
    sb.touch(&busy).await.unwrap();
    assert_eq!(sb.reap_idle(300).await.unwrap(), vec![idle]);
    assert_eq!(sb.alive(), vec![busy]);
    assert!(sb.reap_idle(300).await.unwrap().is_empty()); // 已释放的不会被重复收割
}

#[tokio::test]
async fn unknown_sandbox_id_is_reported_with_registry() {
    let sb = FakeSandbox::default();
    let err = sb.list_files("sb-nope").await.unwrap_err();
    assert_eq!(err.kind, SandboxErrorKind::NotFound);
    assert!(err.message.contains("未知 sandbox_id"), "{}", err.message);
}

/// `as_bytes` 的四种写法（清单 §5）。
#[test]
fn as_bytes_accepts_four_shapes() {
    use aite_testing::{CSV_SAMPLE, PNG_1X1, as_bytes};
    assert_eq!(as_bytes(&json!("builtin:png")).unwrap(), *PNG_1X1);
    assert_eq!(
        as_bytes(&json!({"builtin": "csv"})).unwrap(),
        CSV_SAMPLE.as_bytes()
    );
    assert_eq!(as_bytes(&json!({"b64": "aGVsbG8="})).unwrap(), b"hello");
    assert_eq!(as_bytes(&json!("直接文本")).unwrap(), "直接文本".as_bytes());
    assert!(
        as_bytes(&json!("builtin:nope"))
            .unwrap_err()
            .0
            .contains("没有内置样本")
    );
    assert!(as_bytes(&json!(42)).unwrap_err().0.contains("看不懂"));
}

/// exec_script 的默认值（清单 §5 的 ExecScriptStep 那一行）。
#[test]
fn exec_script_step_defaults() {
    let step = ExecScriptStep::default();
    assert!(step.match_.is_none());
    assert_eq!(step.exit_code, 0);
    assert_eq!(step.duration_ms, 5);
    assert!(!step.truncated);
    assert!(step.times.is_none());
}
