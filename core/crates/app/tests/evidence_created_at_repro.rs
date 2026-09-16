//! BB2 ①：把「`created_at` 不进 hash 链」这条病做成一个**跑完还在**的现场。
//!
//! 这个文件只干一件事 —— 在一个固定路径上留下一份**真跑出来**的证据目录
//! （真 worker、真 control plane、真 `FileEvidenceWriter`，payload 是实际时序里
//! 一条条追加出来的，不是手搓的），好让人在 shell 里对着它跑
//! `aite evidence show --dir …`、改掉里面的 `created_at`、再跑一次。
//!
//! **病的判据故意不写在这里。** 写在这里就成了「拿被验的对象自证」：
//! 说话的是那两次命令行输出本身，回执里逐字贴。这里只断言「不动它的时候链是好的」——
//! 现场本身必须是干净的，否则后面那两次对比什么都说明不了。
//!
//! 为什么不是 `tempfile::tempdir()`：它一 drop 就把目录删了，而这份现场的全部用处
//! 就是跑完之后还在。落点 `core/target/bb2-evidence/` 在 `.gitignore` 的
//! `/core/target/` 里，不会脏工作区。每次跑先整个删掉再建，所以不会积垢。
mod common;

use common::*;

use aite_contracts::EvidenceWriter;
use aite_evidence::FileEvidenceWriter;
use aite_testing::FakeSandbox;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// 现场落点。与 `evidence_on_disk.rs` 用的那条脚本同源，所以链里
/// checklist / 工具 / 产物 / 交付都齐全 —— 时间戳排障要看的就是这种链。
fn out_root() -> PathBuf {
    repo_root().join("core/target/bb2-evidence")
}

fn rich_script() -> Vec<aite_testing::ScriptStep> {
    vec![
        tool_step("checklist_add", json!({"items": ["取数", "画图", "交付"]})),
        tool_step(
            "run_python",
            json!({"code": "import matplotlib\nplt.savefig('/work/out.png')\n", "timeout_sec": 30}),
        ),
        tool_step("checklist_check", json!({"id": "c1"})),
        final_with_artifacts(
            "月度趋势图见附件。",
            vec![json!({"path": "/work/out.png", "title": "趋势图"})],
        ),
    ]
}

fn exec_script() -> Vec<Value> {
    vec![json!({
        "match": "savefig",
        "exit_code": 0,
        "stdout": "saved /work/out.png",
        "writes": {"/work/out.png": "builtin:png"},
    })]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_task_leaves_a_chain_on_disk_to_poke_at() {
    let root = out_root();
    let _ = std::fs::remove_dir_all(&root); // 每次都是新现场，不许有上一次的残留
    std::fs::create_dir_all(&root).expect("建 core/target/bb2-evidence");

    let config = make_config(&root);
    let platform = GatedPlatform::new();
    let app = build_with(
        config.clone(),
        platform.clone(),
        RecordingModel::new(rich_script()),
        Arc::new(FakeSandbox::from_values(&exec_script()).expect("exec_script")),
    )
    .await;
    let mut run = RunningApp::start(app.clone(), &platform, None).await;
    platform
        .emit(&event("e1", "把这个 CSV 画成月度趋势图"))
        .await;
    let p = platform.clone();
    wait_until(|| p.inner.count("send_text") == 1, "任务交付").await;
    let a = app.clone();
    wait_until(|| a.worker.in_flight().is_empty(), "worker.run() 返回").await;
    run.shutdown().await.expect("run_app 正常收场");

    let task = the_only_task_from_disk(&config, "留现场").await;
    let writer = FileEvidenceWriter::new(&config.storage.evidence_dir);
    assert!(
        writer.verify(&task.id),
        "刚跑完的链就不自洽，这份现场没法拿来做对比"
    );

    let dir = writer.task_dir(&task.id);
    let events = read_events(&config, &task.id);
    assert!(events.len() >= 8, "链太短了，不够看时序：{}", events.len());
    println!("BB2 现场：{}（{} 条事件）", dir.display(), events.len());
}
