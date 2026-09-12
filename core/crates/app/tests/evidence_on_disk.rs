//! evidence 真落盘（移植 `test_t8_evidence_on_disk.py`，5 条）。
//!
//! B7 验的是 §3.1 的 hash 测试向量与「篡改 payload → verify false」，用的是直接调
//! `FileEvidenceWriter`。这里换成**一个真跑完的任务**：证据是 worker / control plane
//! 在真实时序里一条条追加出来的，然后：
//!
//! * 逐行重算整条链（用 §3.1 的 `payload_hash_of` / `chain_hash`，不借 `verify` 自证）；
//! * `manifest.json` 的 `root_hash` / `event_count` 与文件对得上；
//! * 库里 `Task.evidence_root_hash` 与盘上的 manifest 对得上（两处口径不许漂）；
//! * **文件里每一行的 payload 逐个改坏，每一次都必须 verify false**。
mod common;

use common::*;

use aite_contracts::{
    CONTRACT_VERSION, EvidenceKind, EvidenceWriter, GENESIS, TaskStatus, chain_hash,
    payload_hash_of,
};
use aite_evidence::FileEvidenceWriter;
use aite_testing::FakeSandbox;
use serde_json::{Value, json};
use std::sync::Arc;

/// 一条会写出丰富证据链的脚本：checklist → run_python（真过 Gateway 和沙箱）
/// → checklist_check → final 带产物。
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

/// 跑完一个多步任务，返回 (task_id, session_id)。
async fn run_one_task(config: &aite_contracts::AiteConfig) -> (String, String) {
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
    let task = app.store.list_active_tasks(CHAT).await.expect("list")[0].clone();
    let p = platform.clone();
    wait_until(|| p.inner.count("send_text") == 1, "任务交付").await;
    assert_eq!(
        platform.inner.count("send_file"),
        1,
        "final.artifacts 应当走 W5 发回线程"
    );
    let a = app.clone();
    wait_until(|| a.worker.in_flight().is_empty(), "worker.run() 返回").await;
    run.shutdown().await.expect("run_app 正常收场");
    (task.id, task.session_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_jsonl_and_manifest_are_real_files() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let (task_id, session_id) = run_one_task(&config).await;

    let path = events_path(&config, &task_id);
    assert!(path.is_file(), "{} 应当存在", path.display());
    let text = std::fs::read_to_string(&path).expect("读 events.jsonl");
    let lines: Vec<&str> = text.lines().collect();
    assert!(!lines.is_empty(), "任务跑完了却一条证据都没有");
    assert!(
        lines.iter().all(|l| !l.trim().is_empty()),
        "events.jsonl 不许有空行"
    );

    let events = read_events(&config, &task_id);
    assert_eq!(events.len(), lines.len()); // 一行一个事件
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (0..events.len() as u64).collect::<Vec<_>>(),
        "seq 要从 0 连续"
    );
    assert!(events.iter().all(|e| e.task_id == task_id));

    // 链条在**文件里**重算一遍。不调 verify —— 那是被验的对象，不能拿它自证。
    let mut prev = GENESIS.to_string();
    for e in &events {
        let payload = e
            .payload
            .as_ref()
            .unwrap_or_else(|| panic!("seq={} 的 payload 没内联", e.seq));
        assert_eq!(e.payload_hash, payload_hash_of(payload));
        assert_eq!(e.prev_hash, prev);
        assert_eq!(e.hash, chain_hash(&prev, &e.payload_hash));
        prev = e.hash.clone();
    }

    // §3.1 写死的证据顺序：建任务 → 收到事件 → …… → 交付收尾
    let kinds: Vec<EvidenceKind> = events.iter().map(|e| e.kind).collect();
    assert_eq!(kinds[0], EvidenceKind::TaskCreated);
    assert_eq!(kinds[1], EvidenceKind::EventReceived);
    for want in [
        EvidenceKind::ModelCall,
        EvidenceKind::ChecklistOp,
        EvidenceKind::ToolCall,
        EvidenceKind::ToolResult,
        EvidenceKind::Artifact,
    ] {
        assert!(kinds.contains(&want), "少了 {want} 这一类证据");
    }
    assert_eq!(*kinds.last().expect("最后一条"), EvidenceKind::Delivered);

    // Gateway 那条路真的通了：`run_python` 不是被 denied 掉的。
    // 这一条钉的是 RΩ 必须补的那段接线 —— `P0ToolGateway` 的 token 校验是失败关闭的。
    let run_python: Vec<&serde_json::Map<String, Value>> = events
        .iter()
        .filter(|e| e.kind == EvidenceKind::ToolResult)
        .filter_map(|e| e.payload.as_ref())
        .filter(|p| p.get("name").and_then(Value::as_str) == Some("run_python"))
        .collect();
    assert_eq!(run_python.len(), 1);
    assert_eq!(
        run_python[0].get("ok"),
        Some(&json!(true)),
        "run_python 没跑成：{:?}。多半是组装没把 Task.session_token 登记给 Gateway",
        run_python[0].get("error")
    );

    let manifest = read_manifest(&config, &task_id);
    assert_eq!(
        manifest["root_hash"],
        json!(events.last().expect("最后一条").hash)
    );
    assert_eq!(manifest["event_count"], json!(events.len()));
    assert_eq!(manifest["task_id"], json!(task_id));
    assert_eq!(manifest["session_id"], json!(session_id));
    assert_eq!(manifest["created_by"], json!(SENDER));
    assert_eq!(manifest["contract_version"], json!(CONTRACT_VERSION));

    // 库里记的 root_hash 与盘上的 manifest 必须是同一个值，两处口径不许漂
    let task = read_task_from_disk(&config, &task_id)
        .await
        .expect("任务在库里");
    assert_eq!(task.status, TaskStatus::Delivered);
    assert_eq!(
        task.evidence_root_hash.as_deref(),
        manifest["root_hash"].as_str()
    );
    assert_eq!(manifest["task_no"], json!(task.task_no));
    assert_eq!(manifest["model"], json!(task.model));

    // 现成的 verify 路径：用一个**全新**的 writer 实例，绕开进程里的 tip 缓存
    assert!(FileEvidenceWriter::new(&config.storage.evidence_dir).verify(&task_id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampering_any_line_breaks_verify() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let (task_id, _) = run_one_task(&config).await;
    let path = events_path(&config, &task_id);
    let original = std::fs::read_to_string(&path).expect("读 events.jsonl");
    let lines: Vec<String> = original.lines().map(str::to_string).collect();
    assert!(
        lines.len() >= 8,
        "这条脚本该产出至少 8 条证据，实际 {}，太少就说明前面哪里没跑到",
        lines.len()
    );

    // 每次都新建 writer：verify 必须只看文件，不许吃任何进程内缓存
    let verify = || FileEvidenceWriter::new(&config.storage.evidence_dir).verify(&task_id);
    assert!(verify());

    for i in 0..lines.len() {
        let mut record: Value = serde_json::from_str(&lines[i]).expect("每行都是 JSON");
        let payload = record
            .get_mut("payload")
            .expect("payload 字段在")
            .as_object_mut()
            .expect("payload 不是 null");
        payload.insert("篡改".into(), json!("有人动过这一行"));
        let mut broken = lines.clone();
        broken[i] = serde_json::to_string(&record).expect("序列化");
        std::fs::write(&path, format!("{}\n", broken.join("\n"))).expect("写回");
        assert!(
            !verify(),
            "第 {i} 行的 payload 被改了，verify 却仍然是 true"
        );

        std::fs::write(&path, &original).expect("还原");
        assert!(verify(), "还原第 {i} 行之后 verify 应当回到 true");
    }
}

/// 整行被删掉（seq 断了 / 链断了）同样要认出来。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_line_breaks_verify() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let (task_id, _) = run_one_task(&config).await;
    let path = events_path(&config, &task_id);
    let lines: Vec<String> = std::fs::read_to_string(&path)
        .expect("读")
        .lines()
        .map(str::to_string)
        .collect();

    let middle = lines.len() / 2;
    let kept: Vec<String> = lines[..middle]
        .iter()
        .chain(lines[middle + 1..].iter())
        .cloned()
        .collect();
    std::fs::write(&path, format!("{}\n", kept.join("\n"))).expect("写回");
    assert!(!FileEvidenceWriter::new(&config.storage.evidence_dir).verify(&task_id));
}

/// `!stop` 掉一个**还没被 worker 领走**的任务，证据链照样要收口。
///
/// 这条路不过 worker 的收尾 —— 任务不在 worker 手里，收尾全在
/// `InProcessControlPlane::cancel_task` 的「不在跑」分支。少了 finalize 的话盘上没有
/// manifest.json、库里 `Task.evidence_root_hash` 留空，`aite evidence show`
/// 和卡片上的证据按钮就都指了个空。
///
/// P0 是单 worker：任务 A 停在模型里不出牌，任务 B 就一直排在队列上。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_task_that_never_ran_still_gets_a_manifest() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let platform = GatedPlatform::new();
    let model = RecordingModel::new(vec![holding(
        final_step("永远到不了这一句。"),
        HOLD_FOREVER,
    )]);
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        Arc::new(FakeSandbox::new(Vec::new())),
    )
    .await;
    let mut run = RunningApp::start(app.clone(), &platform, Some(0.05)).await;

    platform.emit(&event("e1", "干个收不完的活")).await;
    wait_until(|| model.holds() >= 1, "任务 A 被 worker 领走、停在模型里").await;
    let task_a = app.store.list_active_tasks(CHAT).await.expect("list")[0].clone();

    // 任务 B：换一条消息 root，于是是新会话新任务，而不是给 A 的 steer
    platform
        .emit(&Ev::new("e2", "再干一件").message_id("om_2").build())
        .await;
    let a = app.clone();
    wait_until(|| a.plane.pending() == 1, "任务 B 排进队列").await;
    let pending: Vec<_> = app
        .store
        .list_active_tasks(CHAT)
        .await
        .expect("list")
        .into_iter()
        .filter(|t| t.id != task_a.id)
        .collect();
    assert_eq!(pending.len(), 1, "应当只有任务 B 在等派发");
    let task_b = pending[0].clone();

    platform
        .emit(
            &Ev::new("e3", &format!("!stop {}", task_b.task_no))
                .message_id("om_3")
                .build(),
        )
        .await;
    let cfg = config.clone();
    let bid = task_b.id.clone();
    wait_until(
        || manifest_path(&cfg, &bid).is_file(),
        "任务 B 的 manifest 落盘",
    )
    .await;
    let stopped = app
        .store
        .get_task(&task_b.id)
        .await
        .expect("get_task")
        .expect("任务 B 在库里");
    run.shutdown().await.expect("run_app 正常收场");

    assert_eq!(stopped.status, TaskStatus::Cancelled);

    // 证据链最后一条就是 cancelled，manifest 的 root_hash 与它对得上
    let events = read_events(&config, &task_b.id);
    assert_eq!(
        events.last().expect("最后一条").kind,
        EvidenceKind::Cancelled
    );
    let manifest = read_manifest(&config, &task_b.id);
    assert_eq!(
        manifest["root_hash"],
        json!(events.last().expect("最后一条").hash)
    );
    assert_eq!(manifest["event_count"], json!(events.len()));
    assert_eq!(manifest["task_id"], json!(task_b.id));
    assert_eq!(manifest["session_id"], json!(task_b.session_id));
    assert_eq!(manifest["task_no"], json!(task_b.task_no));
    assert_eq!(manifest["created_by"], json!(SENDER));
    assert_eq!(manifest["contract_version"], json!(CONTRACT_VERSION));

    // root_hash 得真的进库 —— cancel_task 里那次 update_task 在 finalize 之前
    let from_disk = read_task_from_disk(&config, &task_b.id)
        .await
        .expect("任务在库里");
    assert_eq!(
        from_disk.evidence_root_hash.as_deref(),
        manifest["root_hash"].as_str()
    );
    // manifest 的 model 与库里那份同源（worker 收尾的那条路也是这个口径）
    assert_eq!(manifest["model"], json!(from_disk.model));
    assert!(!from_disk.model.is_empty());

    assert!(FileEvidenceWriter::new(&config.storage.evidence_dir).verify(&task_b.id));
}

/// 整轨的硬约束：一个字节都不许写进仓库的 data/。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn evidence_stays_inside_the_tmp_dir() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(tmp.path());
    let (task_id, _) = run_one_task(&config).await;
    let root = std::path::Path::new(&config.storage.evidence_dir);
    assert!(events_path(&config, &task_id).starts_with(root));
    assert!(manifest_path(&config, &task_id).starts_with(root));
    assert!(root.starts_with(tmp.path()));
    let repo = repo_root().canonicalize().unwrap_or_else(|_| repo_root());
    assert!(
        !root.starts_with(&repo),
        "证据落进了仓库：{}",
        root.display()
    );
}
