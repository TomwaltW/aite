//! 5 个 Gateway 工具的行为（对应旧 `tests/gateway/test_gateway_tools.py`，23 条）。
//!
//! 头等大事是 `read_group_history` 的过滤：`PlatformPort::read_history` 的契约注释明写
//! 「不做 sender_kind 过滤（过滤归 Gateway 的 read_group_history 工具）」，§6 的验收也把它
//! 列成独有项。假平台照契约把 bot / app / system 消息原样交上来，所以这里过不了就是真漏了，
//! 不是替身太宽松。
//!
//! content / data 的排版逐字照移植清单 §2：评测场景的断言咬着这些字样。
mod common;

use aite_contracts::{ExecResult, FileEntry, SandboxPort, SenderKind, ToolErrorCode, ToolGateway};
use common::{
    CSV_BYTES, PlatformCall, data_of, error_code, fixture, history_rows, register, req, req_args,
};
use serde_json::json;

// --- read_group_history ------------------------------------------------------

#[tokio::test]
async fn read_group_history_drops_every_non_human_sender() {
    let f = fixture();
    let result = f.gateway().call(&f.ctx, &req("read_group_history")).await;

    assert!(result.ok, "{:?}", result.error);
    let data = data_of(&result);
    let rows = data["messages"].as_array().unwrap();
    for row in rows {
        assert_eq!(row["sender_kind"], json!(SenderKind::Human.as_str()));
    }

    let history = history_rows();
    let humans: Vec<_> = history
        .iter()
        .filter(|m| m.sender_kind == "human")
        .collect();
    let others: Vec<_> = history
        .iter()
        .filter(|m| m.sender_kind != "human")
        .collect();
    assert_eq!(data["count"], json!(humans.len()));
    assert_eq!(data["filtered_out"], json!(others.len()));

    // content 里也不许留下机器人说过的话
    for message in &others {
        assert!(
            !result.content.contains(&message.message_id),
            "{}",
            message.message_id
        );
        assert!(!result.content.contains(&message.text), "{}", message.text);
    }
    for message in &humans {
        assert!(result.content.contains(&message.message_id));
        assert!(result.content.contains(&message.text));
    }
}

#[tokio::test]
async fn read_group_history_content_uses_the_w1_line_format() {
    // 与 §3.6 W1 的群历史窗口一致：[message_id] 姓名: 文本
    let f = fixture();
    let result = f.gateway().call(&f.ctx, &req("read_group_history")).await;
    assert!(
        result
            .content
            .contains("[om_1] 张三: 这周的退款单据我整理好了"),
        "{}",
        result.content
    );
    assert!(
        result
            .content
            .starts_with("本群最近 3 条真人消息（另有 3 条机器人/应用/系统消息已过滤）：\n"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn read_group_history_passes_limit_through() {
    let f = fixture();
    f.gateway()
        .call(&f.ctx, &req_args("read_group_history", json!({"limit": 2})))
        .await;

    assert_eq!(
        f.platform.last_call(),
        Some(PlatformCall::ReadHistory {
            chat_id: f.ctx.chat_id.clone(),
            limit: 2,
            thread_id: None,
        })
    );
}

#[tokio::test]
async fn read_group_history_thread_only_scopes_to_the_thread() {
    let f = fixture();
    let result = f
        .gateway()
        .call(
            &f.ctx,
            &req_args("read_group_history", json!({"thread_only": true})),
        )
        .await;

    assert_eq!(
        f.platform.last_call(),
        Some(PlatformCall::ReadHistory {
            chat_id: f.ctx.chat_id.clone(),
            limit: 50,
            thread_id: f.ctx.thread_id.clone(),
        })
    );
    let data = data_of(&result);
    let ids: Vec<&str> = data["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["message_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["om_1", "om_3"]); // om_6 在话题外，om_2/om_4 不是真人
}

#[tokio::test]
async fn read_group_history_thread_only_without_a_thread_is_honest() {
    // 顶层消息没有 thread_id：如实说没有话题历史，而不是悄悄把全群历史端上来。
    let f = fixture();
    let mut toplevel = f.ctx.clone();
    toplevel.thread_id = None;

    let result = f
        .gateway()
        .call(
            &toplevel,
            &req_args("read_group_history", json!({"thread_only": true})),
        )
        .await;

    assert!(result.ok);
    assert_eq!(result.content, "本次对话不在话题里，没有话题历史可读。");
    assert_eq!(
        data_of(&result),
        json!({"messages": [], "count": 0, "filtered_out": 0, "thread_only": true})
            .as_object()
            .unwrap()
            .clone()
    );
    assert!(f.platform.calls().is_empty(), "压根没去问平台");
}

#[tokio::test]
async fn read_group_history_with_only_bots_says_so() {
    let f = fixture();
    f.platform.set_history(
        history_rows()
            .into_iter()
            .filter(|m| m.sender_kind != "human")
            .collect(),
    );

    let result = f.gateway().call(&f.ctx, &req("read_group_history")).await;

    assert!(result.ok);
    let data = data_of(&result);
    assert_eq!(data["count"], json!(0));
    assert_eq!(data["filtered_out"], json!(3));
    assert_eq!(
        result.content,
        "本群没有可引用的真人消息。（读到 3 条，全都不是真人发的，已丢弃）"
    );
}

#[tokio::test]
async fn read_group_history_with_no_messages_at_all_says_so() {
    let f = fixture();
    f.platform.set_history(Vec::new());

    let result = f.gateway().call(&f.ctx, &req("read_group_history")).await;

    assert!(result.ok);
    assert_eq!(
        result.content,
        "本群没有可引用的真人消息。（这里还没有消息）"
    );
}

// --- read_document -----------------------------------------------------------

#[tokio::test]
async fn read_document_returns_markdown() {
    let f = fixture();
    let result = f
        .gateway()
        .call(
            &f.ctx,
            &req_args(
                "read_document",
                json!({"url_or_token": "https://feishu.cn/docx/abc"}),
            ),
        )
        .await;

    assert!(result.ok, "{:?}", result.error);
    let data = data_of(&result);
    assert_eq!(data["title"], json!("退款流程 SOP"));
    assert!(
        result
            .content
            .starts_with("# 退款流程 SOP\n\n来源：https://feishu.cn/docx/abc\n\n")
    );
    assert!(result.content.contains("1. 核对单据"));
}

#[tokio::test]
async fn read_document_unknown_ref_is_upstream() {
    let f = fixture();
    let result = f
        .gateway()
        .call(
            &f.ctx,
            &req_args(
                "read_document",
                json!({"url_or_token": "https://feishu.cn/docx/nope"}),
            ),
        )
        .await;

    assert_eq!(error_code(&result), Some(ToolErrorCode::Upstream));
}

// --- download_attachment -----------------------------------------------------

#[tokio::test]
async fn download_attachment_lands_in_work_in() {
    let f = fixture();
    let gateway = f.gateway();
    let result = gateway
        .call(
            &f.ctx,
            &req_args("download_attachment", json!({"file_key": "file_v3_csv"})),
        )
        .await;

    assert!(result.ok, "{:?}", result.error);
    let data = data_of(&result);
    assert_eq!(data["path"], json!("/work/in/file_v3_csv"));
    assert_eq!(data["size"], json!(CSV_BYTES.len()));
    assert_eq!(
        result.content,
        format!(
            "附件已下载到沙箱：/work/in/file_v3_csv（{} 字节）",
            CSV_BYTES.len()
        )
    );

    let sandbox_id = gateway.sandbox_id_of(&f.ctx.task_id).await.unwrap();
    let written = f
        .sandbox
        .get_file(&sandbox_id, "/work/in/file_v3_csv")
        .await
        .unwrap();
    assert_eq!(written, CSV_BYTES);
}

#[tokio::test]
async fn download_attachment_uses_the_message_from_ctx() {
    let f = fixture();
    f.gateway()
        .call(
            &f.ctx,
            &req_args("download_attachment", json!({"file_key": "file_v3_csv"})),
        )
        .await;

    assert_eq!(
        f.platform.last_call(),
        Some(PlatformCall::DownloadFile {
            message_id: f.ctx.attachments_message_id.clone().unwrap(),
            file_key: "file_v3_csv".into(),
        })
    );
}

#[tokio::test]
async fn download_attachment_sanitizes_the_file_key() {
    // file_key 是平台给的不透明串，直接当路径用会爬出 /work/in。
    let f = fixture();
    f.platform.put_file_fixture("../../etc/passwd", b"nope");

    let result = f
        .gateway()
        .call(
            &f.ctx,
            &req_args(
                "download_attachment",
                json!({"file_key": "../../etc/passwd"}),
            ),
        )
        .await;

    assert!(result.ok, "{:?}", result.error);
    assert_eq!(data_of(&result)["path"], json!("/work/in/passwd"));
    // data 里的 file_key 是原始串，不是清洗后的
    assert_eq!(data_of(&result)["file_key"], json!("../../etc/passwd"));
}

#[tokio::test]
async fn download_attachment_without_attachment_context_is_upstream() {
    let f = fixture();
    let mut bare = f.ctx.clone();
    bare.attachments_message_id = None;

    let result = f
        .gateway()
        .call(
            &bare,
            &req_args("download_attachment", json!({"file_key": "file_v3_csv"})),
        )
        .await;

    assert_eq!(error_code(&result), Some(ToolErrorCode::Upstream));
}

// --- run_python --------------------------------------------------------------

#[tokio::test]
async fn run_python_execs_in_the_sandbox() {
    let f = fixture();
    let gateway = f.gateway();
    let result = gateway
        .call(
            &f.ctx,
            &req_args(
                "run_python",
                json!({"code": "print('hi')", "timeout_sec": 30}),
            ),
        )
        .await;

    assert!(result.ok, "{:?}", result.error);
    let (sandbox_id, exec_req) = f.sandbox.exec_requests().last().cloned().unwrap();
    assert_eq!(
        Some(sandbox_id),
        gateway.sandbox_id_of(&f.ctx.task_id).await
    );
    assert_eq!(exec_req.code, "print('hi')");
    assert_eq!(exec_req.timeout_sec, 30);
    assert_eq!(exec_req.language.as_str(), "python");
    assert!(result.content.contains("fake stdout"), "{}", result.content);
    assert!(
        result.content.starts_with("执行成功（7 ms）\n\nstdout:\n"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn run_python_reports_files_out_as_artifacts() {
    let f = fixture();
    f.sandbox.set_exec_result(ExecResult {
        exit_code: 0,
        stdout: "saved".into(),
        stderr: String::new(),
        duration_ms: 42,
        truncated: false,
        files_out: vec![FileEntry {
            path: "/work/out.png".into(),
            size: 2048,
        }],
    });

    let result = f
        .gateway()
        .call(&f.ctx, &req_args("run_python", json!({"code": "..."})))
        .await;

    let paths: Vec<&str> = result.artifacts.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, vec!["/work/out.png"]);
    assert_eq!(result.artifacts[0].title, "out.png");
    assert_eq!(result.artifacts[0].mime.as_deref(), Some("image/png"));
    assert_eq!(
        data_of(&result)["files_out"],
        json!([{"path": "/work/out.png", "size": 2048}])
    );
    assert!(
        result
            .content
            .contains("/work 下本次新增/修改的文件：\n  /work/out.png（2048 字节）"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn run_python_user_error_is_not_a_tool_failure() {
    // 代码自己报错 → ok=true + traceback 进 content。
    // 算成 code=sandbox 会撞上 §3.3 的「连续 2 次沙箱失败 → task failed」，两个语法错就把任务打死了。
    let f = fixture();
    f.sandbox.set_exec_result(ExecResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: "Traceback (most recent call last):\nValueError: 列名不对".into(),
        duration_ms: 15,
        truncated: false,
        files_out: Vec::new(),
    });

    let result = f
        .gateway()
        .call(
            &f.ctx,
            &req_args("run_python", json!({"code": "df['nope']"})),
        )
        .await;

    assert!(result.ok);
    assert!(result.error.is_none());
    assert_eq!(data_of(&result)["exit_code"], json!(1));
    assert!(result.content.starts_with("代码以退出码 1 结束（15 ms）"));
    assert!(
        result.content.contains("stdout: （空）"),
        "{}",
        result.content
    );
    assert!(result.content.contains("ValueError: 列名不对"));
    assert!(result.content.ends_with("本次没有在 /work 下产出文件。"));
}

#[tokio::test]
async fn run_python_touches_the_sandbox() {
    // §3.6 W7 的 reaper 每 60s 扫一次，任务跑一半不能被收走。
    let f = fixture();
    let gateway = f.gateway();
    gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;

    assert_eq!(
        f.sandbox.touched(),
        vec![gateway.sandbox_id_of(&f.ctx.task_id).await.unwrap()]
    );
}

#[tokio::test]
async fn run_python_flags_truncated_output() {
    let f = fixture();
    f.sandbox.set_exec_result(ExecResult {
        exit_code: 0,
        stdout: "x".repeat(100),
        stderr: String::new(),
        duration_ms: 1,
        truncated: true,
        files_out: Vec::new(),
    });

    let result = f
        .gateway()
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;

    assert_eq!(data_of(&result)["truncated"], json!(true));
    assert!(result.content.contains("注意：输出太长已被截断。"));
}

// --- list_files --------------------------------------------------------------

#[tokio::test]
async fn list_files_without_a_sandbox_does_not_create_one() {
    // 模型常在跑代码前先问一句「有什么文件」，为这一问起个容器纯属浪费。
    let f = fixture();
    let gateway = f.gateway();
    let result = gateway.call(&f.ctx, &req("list_files")).await;

    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.content, "沙箱还没启动，/work 下还没有文件。");
    assert_eq!(
        data_of(&result),
        json!({"files": [], "count": 0})
            .as_object()
            .unwrap()
            .clone()
    );
    assert!(gateway.sandbox_id_of(&f.ctx.task_id).await.is_none());
    assert_eq!(f.sandbox.box_count(), 0);
}

#[tokio::test]
async fn list_files_lists_the_work_tree() {
    let f = fixture();
    let gateway = f.gateway();
    gateway
        .call(
            &f.ctx,
            &req_args("download_attachment", json!({"file_key": "file_v3_csv"})),
        )
        .await;
    gateway
        .call(
            &f.ctx,
            &req_args(
                "run_python",
                json!({"code": "open('/work/out.txt','w').write('x')"}),
            ),
        )
        .await;

    let result = gateway.call(&f.ctx, &req("list_files")).await;

    assert_eq!(
        data_of(&result)["files"],
        json!(["/work/in/file_v3_csv", "/work/out.txt"])
    );
    assert_eq!(
        result.content,
        "/work 下有 2 个文件：\n/work/in/file_v3_csv\n/work/out.txt"
    );
}

// --- 沙箱的生命周期（一个 task 一个容器）------------------------------------

#[tokio::test]
async fn one_sandbox_per_task_is_reused() {
    let f = fixture();
    let gateway = f.gateway();
    gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    let first = gateway.sandbox_id_of(&f.ctx.task_id).await;
    gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(2)"})))
        .await;

    assert_eq!(gateway.sandbox_id_of(&f.ctx.task_id).await, first);
    assert_eq!(f.sandbox.box_count(), 1);
}

#[tokio::test]
async fn different_tasks_get_different_sandboxes() {
    let f = fixture();
    let gateway = f.gateway();
    gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;

    let mut other = f.ctx_with_task("task-two");
    other.session_token = "a".repeat(32);
    gateway.register_task("task-two", &"a".repeat(32));
    gateway
        .call(&other, &req_args("run_python", json!({"code": "print(2)"})))
        .await;

    assert_ne!(
        gateway.sandbox_id_of(&f.ctx.task_id).await,
        gateway.sandbox_id_of("task-two").await
    );
    assert_eq!(f.sandbox.box_count(), 2);
}

#[tokio::test]
async fn release_task_drops_the_sandbox_and_the_token() {
    // !stop / 任务收尾时用（§3.3）。幂等。
    let f = fixture();
    let gateway = f.gateway();
    gateway
        .call(&f.ctx, &req_args("run_python", json!({"code": "print(1)"})))
        .await;
    let sandbox_id = gateway.sandbox_id_of(&f.ctx.task_id).await.unwrap();

    gateway.release_task(&f.ctx.task_id).await;
    gateway.release_task(&f.ctx.task_id).await;

    assert_eq!(f.sandbox.released(), vec![sandbox_id]);
    assert!(gateway.sandbox_id_of(&f.ctx.task_id).await.is_none());
    // token 也撤了：同一个 task 再来调工具就该被拒
    let result = gateway.call(&f.ctx, &req("list_files")).await;
    assert_eq!(error_code(&result), Some(ToolErrorCode::Denied));
}

// multi_thread：默认的 current_thread runtime 下 4 个 spawn 是一个跑完再跑下一个，
// 配上没有挂起点的替身，这条用例曾经是空跑的（删掉双检锁照样绿）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_run_python_shares_one_sandbox() {
    // per-task 锁：并发两个 tool_call 不会各建一个容器（旧实现的双检锁）。
    use std::sync::Arc;
    let f = fixture();
    let gateway = Arc::new(f.gateway());

    let mut handles = Vec::new();
    for i in 0..4 {
        let gateway = gateway.clone();
        let ctx = f.ctx.clone();
        handles.push(tokio::spawn(async move {
            gateway
                .call(
                    &ctx,
                    &req_args("run_python", json!({"code": format!("print({i})")})),
                )
                .await
        }));
    }
    for handle in handles {
        assert!(handle.await.unwrap().ok);
    }

    assert_eq!(f.sandbox.box_count(), 1);
}

#[tokio::test]
async fn token_resolver_can_replace_the_registry() {
    // RΩ 组装时把它接到 SessionStore 上（Gateway 不认识 store）。
    use std::sync::{Arc, Mutex};
    let f = fixture();
    let tokens: Arc<Mutex<std::collections::HashMap<String, String>>> = Arc::new(Mutex::new(
        std::collections::HashMap::from([(f.ctx.task_id.clone(), f.ctx.session_token.clone())]),
    ));
    let lookup = tokens.clone();
    let gateway = f.raw().with_token_resolver(Arc::new(move |task_id: &str| {
        Ok(lookup.lock().unwrap().get(task_id).cloned())
    }));

    assert!(gateway.call(&f.ctx, &req("list_files")).await.ok);
    tokens.lock().unwrap().clear();
    assert_eq!(
        error_code(&gateway.call(&f.ctx, &req("list_files")).await),
        Some(ToolErrorCode::Denied)
    );
}

#[tokio::test]
async fn unregister_task_closes_the_gate() {
    let f = fixture();
    let gateway = f.gateway();
    assert!(gateway.call(&f.ctx, &req("list_files")).await.ok);

    gateway.unregister_task(&f.ctx.task_id);

    assert_eq!(
        error_code(&gateway.call(&f.ctx, &req("list_files")).await),
        Some(ToolErrorCode::Denied)
    );
    register(&gateway); // 再登记回去还能用
    assert!(gateway.call(&f.ctx, &req("list_files")).await.ok);
}
