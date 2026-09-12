//! 容器被 W7 的 reaper 收走之后，Gateway 自己能不能爬起来（审核记账 4.1 第 7 行）。
//!
//! 现场是这样接上的：`touch` 只在沙箱 RPC 时发生（`python_exec` 里 exec 之后那一发；
//! `list_files` **刻意不 touch**），所以模型在两次工具调用之间想上五分钟，中间一次都不刷；
//! `sandbox.idle_sec` 默认 300s，reaper 每 60s 扫一次，容器就被收走了。
//! edge 那边 `ReapIdle` 把收走的 id 一个不少地交回控制面，可控制面只拿它 bump 一个计数器
//! —— **Gateway 的 `task_id → sandbox_id` 表没有任何人通知**。表里那个死 id 会被
//! `acquire_sandbox` 的「有就返回」原样复用出来，连撞两次就是 §3.3 的
//! 「连续 2 次沙箱失败 → task failed」。
//!
//! 这一组钉住的就是自愈的三条边：**摘掉死 id 重建、只重试一次、把 /work 空了这件事
//! 告诉模型**。第三条尤其重要 —— 只写进 tracing 的话，模型还以为它上一步写的文件在。
mod common;

use aite_contracts::{SandboxError, SandboxErrorKind, ToolErrorCode, ToolGateway};
use common::{TASK_ID, assert_failed, fixture, req, req_args};
use serde_json::json;

/// reaper 收走容器之后，edge 对那个 id 的回答。
fn gone() -> SandboxError {
    SandboxError::new(SandboxErrorKind::NotFound, "no such container: fake-sbx-1")
}

#[tokio::test]
async fn run_python_rebuilds_the_sandbox_the_reaper_took() {
    let f = fixture();
    let gateway = f.gateway();

    // 第一步正常跑一次，把容器建起来并记进账
    let first = gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    assert!(first.ok, "第一发就该正常：{:?}", first.error);
    assert_eq!(
        gateway.sandbox_id_of(TASK_ID).await,
        Some("fake-sbx-1".to_string())
    );

    // 模型想了五分多钟 —— reaper 把容器收走了。下一发 exec 换回 NotFound。
    f.sandbox.fail_exec_once(gone());

    let second = gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(2)"})))
        .await;
    assert!(
        second.ok,
        "容器没了应当重建后重试，实际落成了工具失败：{:?}",
        second.error
    );

    // 真的换了一个容器，而不是把同一个死 id 又打了一遍
    let ids: Vec<String> = f
        .sandbox
        .exec_requests()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        ids,
        vec!["fake-sbx-1", "fake-sbx-1", "fake-sbx-2"],
        "第 2、3 发的 sandbox_id 该从死 id 换成新建的"
    );
    assert_eq!(
        gateway.sandbox_id_of(TASK_ID).await,
        Some("fake-sbx-2".to_string()),
        "记账要跟着换成新容器"
    );

    // 「/work 空了」这句话必须在给模型看的正文最前面 —— 只进 tracing 不算
    assert!(
        second.content.starts_with("【沙箱已重建】"),
        "重建的告知要排在正文第一行，实际：{}",
        second.content
    );
    assert!(
        second.content.contains("之前写进 /work 的文件都不在了"),
        "得说清楚文件没了，实际：{}",
        second.content
    );
    assert_eq!(
        second.data.as_ref().and_then(|d| d.get("sandbox_rebuilt")),
        Some(&json!(true))
    );

    // 没重建的那一次不许平白多出这句话
    assert!(
        !first.content.contains("【沙箱已重建】"),
        "正常那一发不该带重建告知：{}",
        first.content
    );
    assert_eq!(
        first.data.as_ref().and_then(|d| d.get("sandbox_rebuilt")),
        Some(&json!(false))
    );
}

#[tokio::test]
async fn a_second_not_found_still_lands_on_a_sandbox_failure() {
    let f = fixture();
    let gateway = f.gateway();
    // 黏性失败：重建出来的那个也撞 NotFound（docker daemon 真出事时就是这样）
    f.sandbox.fail_exec(gone());

    let result = gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    assert_failed(&result, ToolErrorCode::Sandbox);

    // **只重试一次**：恰好两发 exec。多一发就说明自愈变成了无限重建，
    // §3.3 的 MAX_CONSECUTIVE_SANDBOX_ERRORS=2 那道闸会被它架空 —— 任务永远死不掉。
    let ids: Vec<String> = f
        .sandbox
        .exec_requests()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        ids,
        vec!["fake-sbx-1", "fake-sbx-2"],
        "重建一次、重试一次就收手"
    );
}

#[tokio::test]
async fn list_files_answers_empty_instead_of_failing_when_the_sandbox_is_gone() {
    let f = fixture();
    let gateway = f.gateway();
    gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    f.sandbox.fail_list_files_once(gone());

    let result = gateway.call(&f.ctx, &req("list_files")).await;

    // 容器被收走不是工具失败：算进去的话两问就把任务打死（§3.3 连续 2 次沙箱失败）
    assert!(
        result.ok,
        "容器被 reaper 收走不该算 list_files 失败：{:?}",
        result.error
    );
    assert!(
        result.content.starts_with("【沙箱已回收】"),
        "要如实说 /work 空了，实际：{}",
        result.content
    );
    assert_eq!(
        result.data.as_ref().and_then(|d| d.get("count")),
        Some(&json!(0))
    );

    // 记账摘掉了 —— 下一次 run_python 会建新的，不会再复用死 id
    assert_eq!(gateway.sandbox_id_of(TASK_ID).await, None);
    // 但 list_files 自己不建容器（模块头那条「列目录不该为它起一个容器」）
    assert_eq!(f.sandbox.box_count(), 1, "list_files 不许自己建容器");
}

#[tokio::test]
async fn download_attachment_rewrites_into_a_fresh_sandbox() {
    let f = fixture();
    let gateway = f.gateway();
    gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    f.sandbox.fail_put_file_once(gone());

    let result = gateway
        .call(
            &f.ctx,
            &req_args("download_attachment", json!({"file_key": "file_v3_csv"})),
        )
        .await;

    assert!(
        result.ok,
        "blob 已经从平台下来了，容器没了只该换个容器重写：{:?}",
        result.error
    );
    assert_eq!(
        gateway.sandbox_id_of(TASK_ID).await,
        Some("fake-sbx-2".to_string())
    );
    assert_eq!(
        f.sandbox.files_of("fake-sbx-2"),
        vec!["/work/in/file_v3_csv".to_string()],
        "附件要落在新容器的 /work/in 下"
    );
}
