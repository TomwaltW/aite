//! 冷启动 → 交付的进程级贯通（移植 `test_t13_cold_start_to_delivery.py`，8 条）。
//!
//! 别的几组把 `run_app` 的**停机**面钉死了；「起飞 → 干活 → 交付」这条主干在这一层
//! 一条都没有 —— 它一直被测在**别的层**（评测那 10 个场景自己拼 plane + worker，
//! `aite-worker` 的测试只看 worker 内部）。而 §2.4 的 M1（@ 一下有反应）和
//! M3（CSV 画图、卡片更新、产物回线程）在真机上走的正是这条路。
//!
//! **判据不是评测那 10 个场景的判据的副本。** 同一件事在两层都红时，两层要能把责任
//! 分开：评测红 = 行为本身错了；这里红 = 行为对，但 `build_app` 的装配没把它接上
//! （落盘目录少 mkdir、`session_token` 没登记给 Gateway、沙箱没一路传下去、
//! 收尾把还没落盘的东西吃掉了）。所以这里的断言尽量挑**只有装配才决定得了**的那些。
//!
//! 一趟飞行覆盖八个节点，一个节点一条用例，每条都从 `build_app` + `run_app` 走一遍。
mod common;

use common::*;

use aite_contracts::{
    CONTRACT_VERSION, EvidenceKind, EvidenceWriter, GENESIS, Task, TaskStatus, chain_hash,
    payload_hash_of,
};
use aite_evidence::FileEvidenceWriter;
use aite_testing::{CSV_SAMPLE, FakeSandbox, PNG_MAGIC};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// R7 把这条消息定成话题 root（`plane` 建会话时 `thread_id = ev.anchor.message_id`），
/// 所以它同时是「附件挂在哪条消息上」和「产物该回到哪里」的答案。
const ROOT_MSG: &str = "om_1";
const FILE_KEY: &str = "file_sales_csv";
const INBOX_PATH: &str = "/work/in/file_sales_csv";
const ARTIFACT_PATH: &str = "/work/out.png";

fn csv_bytes() -> Vec<u8> {
    CSV_SAMPLE.as_bytes().to_vec()
}

/// 一整条真机出牌：3 项 checklist → 下附件 → 画图 → 列文件 → 逐项 check → 带产物 final。
///
/// 中间穿插 `checklist_check` 是为了让 W4 的「原地更新」有真东西可更新 ——
/// 卡片内容每一步都在变，不是同一份快照被重推三次。
fn script() -> Vec<aite_testing::ScriptStep> {
    vec![
        tool_step(
            "checklist_add",
            json!({"items": ["下载数据", "画图", "交付"]}),
        ),
        tool_step("download_attachment", json!({"file_key": FILE_KEY})),
        tool_step("checklist_check", json!({"id": "c1"})),
        tool_step(
            "run_python",
            json!({
                "code": format!(
                    "import pandas as pd, matplotlib\nmatplotlib.use(\"Agg\")\n\
                     import matplotlib.pyplot as plt\ndf = pd.read_csv(\"{INBOX_PATH}\")\n\
                     df.plot(x=\"month\", y=\"amount\")\nplt.savefig(\"{ARTIFACT_PATH}\")\n"
                ),
                "timeout_sec": 120,
            }),
        ),
        tool_step("checklist_check", json!({"id": "c2"})),
        tool_step("list_files", json!({})),
        tool_step("checklist_check", json!({"id": "c3"})),
        final_with_artifacts(
            "月度趋势图见附件。",
            vec![json!({"path": ARTIFACT_PATH, "title": "月度趋势"})],
        ),
    ]
}

const SCRIPT_STEPS: usize = 8;

/// 代码里出现 savefig 就当画好了图，往 `/work/out.png` 写一张真 PNG（前 8 字节是魔数）。
fn exec_script() -> Vec<Value> {
    vec![json!({
        "match": "savefig",
        "exit_code": 0,
        "stdout": "saved /work/out.png",
        "writes": {ARTIFACT_PATH: "builtin:png"},
    })]
}

/// M1 + M3 的入口事件：@ 了 Aite，并且带一个 CSV 附件。
fn mention_with_csv() -> aite_contracts::NormalizedEvent {
    Ev::new("e1", "把这个 CSV 画成月度趋势图")
        .message_id(ROOT_MSG)
        .attachments(vec![attachment(FILE_KEY, "sales.csv", ROOT_MSG)])
        .build()
}

/// 跑完一趟之后的现场。`task` 是刚建出来那一刻的快照，终态一律从盘上读。
struct Flight {
    #[allow(dead_code)] // 每条用例各用一部分
    config: aite_contracts::AiteConfig,
    app: Arc<aite_app::AiteApp>,
    platform: Arc<GatedPlatform>,
    model: Arc<RecordingModel>,
    sandbox: Arc<ClosableFakeSandbox>,
    task: Task,
    run: RunningApp,
}

impl Flight {
    /// 这趟飞行一共开过几只沙箱 —— 答案必须是一只，不然产物就取错了地方。
    fn sandbox_id(&self) -> String {
        let ids = self.sandbox.inner.box_ids();
        assert_eq!(
            ids.len(),
            1,
            "一个任务只该有一只沙箱，实际开了 {ids:?}。多开一只通常意味着取产物时没从 \
             Gateway 手里要 sandbox_id，而是自己 acquire 了一个全新的空容器 —— 那里面永远没有产物。"
        );
        ids[0].clone()
    }

    fn card_id(&self) -> String {
        let ids = self.platform.inner.card_ids();
        assert_eq!(ids.len(), 1, "应当只有一张卡片，实际 {ids:?}");
        ids[0].clone()
    }
}

/// 从空的 data/ 起飞，跑完一整个任务，在**还没停机**的时候把现场交出来。
///
/// 返回那一刻任务已经彻底收完：`worker.in_flight()` 空了 = `worker.run()` 真的返回了，
/// 于是 evidence 已 finalize、沙箱已还、任务终态已进库。只等 `send_text` 是不够的 ——
/// 那时交付还没走到收尾。
async fn fly(config: aite_contracts::AiteConfig) -> Flight {
    let platform = GatedPlatform::from_fake(Arc::new(
        aite_testing::FakePlatform::new()
            .with_files(files_of(vec![((ROOT_MSG, FILE_KEY), csv_bytes())])),
    ));
    let model = RecordingModel::new(script());
    let sandbox = ClosableFakeSandbox::from_fake(Arc::new(
        FakeSandbox::from_values(&exec_script()).expect("exec_script"),
    ));
    let app = build_with(
        config.clone(),
        platform.clone(),
        model.clone(),
        sandbox.clone(),
    )
    .await;

    let run = RunningApp::start(app.clone(), &platform, None).await;
    platform.emit(&mention_with_csv()).await;

    let active = app.store.list_active_tasks(CHAT).await.expect("list");
    assert_eq!(
        active.len(),
        1,
        "@ 一次只该建一个任务，实际 {} 个",
        active.len()
    );
    let task = active[0].clone();

    let p = platform.clone();
    wait_until(
        || p.inner.count("send_text") == 1,
        "W5 的 send_text（交付回帖）",
    )
    .await;
    let a = app.clone();
    wait_until(
        || a.worker.in_flight().is_empty(),
        "worker.run() 返回（收尾做完）",
    )
    .await;

    Flight {
        config,
        app,
        platform,
        model,
        sandbox,
        task,
        run,
    }
}

// --------------------------------------------------------------------------
// 1 冷启动
// --------------------------------------------------------------------------

/// `storage.*` 的父目录一个都不存在时，起飞要把它们建出来。
///
/// 判据取自 `aite preflight` 第 7 项对人说的那句话：目录不在不算 FAIL，因为
/// 「起飞时自动 mkdir」。真机第一次跑就是这个状态 —— 仓库里没有 data/。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_start_creates_every_storage_dir() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    // 往下套一层 data/：三个落盘路径的父目录一个都不存在
    let config = make_config(&tmp.path().join("data"));
    let data_root = Path::new(&config.storage.sqlite_path)
        .parent()
        .expect("父目录")
        .to_path_buf();
    assert!(!data_root.exists(), "这条用例的前提是 data/ 还没建");

    let mut flight = fly(config.clone()).await;

    assert!(data_root.is_dir(), "sqlite 的父目录没被建出来");
    assert!(
        Path::new(&config.storage.evidence_dir).is_dir(),
        "evidence_dir 没被建出来"
    );
    assert!(
        Path::new(&config.storage.artifacts_dir).is_dir(),
        "artifacts_dir 没被建出来。三个落盘目录里只有它落了空 —— \
         preflight 第 7 项对人承诺的是「起飞时自动 mkdir」，起飞这一层就得真的建。"
    );
    // 目录建了还不够，东西得真落进去
    assert!(
        Path::new(&config.storage.sqlite_path).is_file(),
        "SQLite 文件没落盘"
    );
    assert_eq!(evidence_task_dirs(&config), vec![flight.task.id.clone()]);

    // 整轨硬约束：一个字节都不许写进仓库的 data/。
    // 这条用例偏偏自己造了一层 data/，所以不能照搬「路径里不含 /data/」那种字面判断 ——
    // 判据是**落点在哪**：全都在 tmp 下、一个都不在仓库里。
    let repo = repo_root().canonicalize().unwrap_or_else(|_| repo_root());
    for path in [
        &config.storage.sqlite_path,
        &config.storage.evidence_dir,
        &config.storage.artifacts_dir,
    ] {
        assert!(
            Path::new(path).starts_with(tmp.path()),
            "{path} 落到了 tmp 外面"
        );
        assert!(!Path::new(path).starts_with(&repo), "{path} 落进了仓库");
    }
    flight.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 2 R7：@ 一下有反应
// --------------------------------------------------------------------------

/// M1 的全部内容：@ 了就要有 ack 表情，并且建出会话 + `#A1` 任务。
///
/// ack 必须**早于**卡片：R7 的第一反应是让人知道「收到了」，卡片是第一个工具调用
/// 之前才发的（W3）。这两件事在真机上隔着一次模型往返，顺序反了人就要干等。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mention_gets_an_ack_then_a_session_and_task() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(&tmp.path().join("data"));
    let mut flight = fly(config).await;

    let reactions = flight.platform.inner.reactions();
    assert_eq!(
        reactions,
        vec![(ROOT_MSG.to_string(), aite_contracts::ReactionKind::Ack)],
        "R7 应当给 @ 的那条消息加一个 ack"
    );

    let methods = flight.platform.inner.calls.methods();
    let ack_at = methods
        .iter()
        .position(|m| m == "add_reaction")
        .expect("ack");
    let card_at = methods.iter().position(|m| m == "send_card").expect("卡片");
    assert!(
        ack_at < card_at,
        "ack 要发生在卡片之前，实际顺序：{methods:?}"
    );

    assert_eq!(flight.task.task_no, "#A1", "租户里的第一个任务应当是 #A1");
    assert_eq!(flight.task.created_by, SENDER);

    let session = flight
        .app
        .store
        .get_session(&flight.task.session_id)
        .await
        .expect("get_session")
        .expect("R7 建了任务却没有会话");
    assert_eq!(session.chat_id, CHAT);
    assert_eq!(
        session.anchor.thread_id.as_deref(),
        Some(ROOT_MSG),
        "@ 的那条消息就是话题 root（R7），产物和回帖都要回到这里"
    );
    flight.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 3 W3：只有一张卡
// --------------------------------------------------------------------------

/// W3：第一个非 `final` 的 tool_call 之前先 `send_card`，而且**只有一张**。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_card_is_sent_before_the_first_tool_call() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(&tmp.path().join("data"));
    let mut flight = fly(config).await;

    assert_eq!(
        flight.platform.inner.count("send_card"),
        1,
        "整趟只该发一张卡片"
    );
    assert_eq!(flight.platform.inner.card_count(), 1);

    let methods = flight.platform.inner.calls.methods();
    let card_at = methods.iter().position(|m| m == "send_card").expect("卡片");
    let dl_at = methods
        .iter()
        .position(|m| m == "download_file")
        .expect("下附件");
    assert!(
        card_at < dl_at,
        "W3：卡片要发在第一个工具调用之前，实际顺序：{methods:?}"
    );

    let card_id = flight.card_id();
    let first = flight.platform.inner.card_snapshots(Some(&card_id))[0].clone();
    assert_eq!(first.status, aite_contracts::CardStatus::Working);
    assert_eq!(first.task_no, "#A1");
    flight.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 4 W4：原地更新，不新增消息
// --------------------------------------------------------------------------

/// W4：过程中 `update_card` ≥3 次，全落在同一张卡上，群里不多出任何一条消息。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn card_is_updated_in_place_and_nothing_new_is_posted() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(&tmp.path().join("data"));
    let mut flight = fly(config).await;
    let card_id = flight.card_id();

    assert!(
        flight.platform.inner.update_count() >= 3,
        "清单一路在变，update_card 至少该有 3 次，实际 {} 次",
        flight.platform.inner.update_count()
    );
    let targets: std::collections::BTreeSet<String> = flight
        .platform
        .inner
        .calls
        .of("update_card")
        .iter()
        .filter_map(|c| c.arg_str("card_id").map(str::to_string))
        .collect();
    assert_eq!(
        targets,
        std::collections::BTreeSet::from([card_id.clone()]),
        "所有更新必须落在同一张卡片上"
    );

    // 群里的出站消息只有：1 张卡 + 1 个产物 + 1 条回帖。没有第二条卡片、没有中途播报。
    assert_eq!(flight.platform.inner.count("send_card"), 1);
    assert_eq!(flight.platform.inner.count("send_file"), 1);
    assert_eq!(flight.platform.inner.count("send_text"), 1);

    let snapshots = flight.platform.inner.card_snapshots(Some(&card_id));
    assert!(
        snapshots.len() >= 4,
        "一次 send_card + ≥3 次 update，实际 {} 份快照",
        snapshots.len()
    );
    assert!(
        snapshots[0].items.is_empty(),
        "第一张卡发出去时清单还是空的"
    );

    let last = snapshots.last().expect("最后一张").clone();
    assert_eq!(
        last.status,
        aite_contracts::CardStatus::Delivered,
        "收尾那次更新要把卡片置成 delivered"
    );
    assert_eq!(
        last.items
            .iter()
            .map(|i| i.text.clone())
            .collect::<Vec<_>>(),
        vec!["下载数据", "画图", "交付"]
    );
    assert!(
        last.items
            .iter()
            .all(|i| i.state == aite_contracts::ChecklistState::Done)
    );
    flight.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 5 工具链：一路落到同一只沙箱
// --------------------------------------------------------------------------

/// `download_attachment` → `run_python` → 产出文件 → `list_files` 看得见。
///
/// 这条最能区分「行为对」与「装配对」：`P0ToolGateway` 的 token 校验是失败关闭的，
/// 组装没把 `Task.session_token` 登记进去的话，每个工具调用都会是 `denied`，
/// 而任务照样会 delivered —— 群里只是收不到图。所以这里逐个查 tool_result 的 ok。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tool_chain_lands_in_one_sandbox() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(&tmp.path().join("data"));
    let mut flight = fly(config.clone()).await;

    let events = read_events(&config, &flight.task.id);
    for name in ["download_attachment", "run_python", "list_files"] {
        let payload = events
            .iter()
            .filter(|e| e.kind == EvidenceKind::ToolResult)
            .filter_map(|e| e.payload.as_ref())
            .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("{name} 一次都没跑到"));
        assert_eq!(
            payload.get("ok"),
            Some(&json!(true)),
            "{name} 没跑成：{:?}。denied 的话多半是组装没把 Task.session_token 登记给 Gateway",
            payload.get("error")
        );
    }

    // 三个工具用的是同一只沙箱，产物就在里面
    let sid = flight.sandbox_id();
    let all = flight.sandbox.inner.all_files();
    let files = all.get(&sid).expect("那只沙箱");
    assert!(
        files.contains_key(INBOX_PATH),
        "附件没进沙箱，/work 下只有：{:?}",
        files.keys().collect::<Vec<_>>()
    );
    assert_eq!(files[INBOX_PATH], csv_bytes().len());
    assert!(
        files.contains_key(ARTIFACT_PATH),
        "run_python 没产出图，/work 下只有：{:?}",
        files.keys().collect::<Vec<_>>()
    );

    let listed: Vec<String> = flight
        .sandbox
        .inner
        .calls
        .of("list_files")
        .iter()
        .filter_map(|c| c.arg_str("sandbox_id").map(str::to_string))
        .collect();
    assert_eq!(listed, vec![sid.clone()], "list_files 应当只列那一只沙箱");

    // 产物真的进了模型的下一轮上下文 —— 模型是「看见」了它才敢 final 的
    assert!(
        flight
            .model
            .prompt_texts()
            .iter()
            .any(|ms| ms.iter().any(|t| t.contains(ARTIFACT_PATH))),
        "沙箱里产出的文件没有出现在任何一轮模型上下文里"
    );
    flight.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 6 W5：产物与交付
// --------------------------------------------------------------------------

/// W5：`final(artifacts)` → `get_file` → `send_file`（回话题 root）→ 再 `send_text`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_artifacts_go_back_to_the_thread_then_the_reply() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(&tmp.path().join("data"));
    let mut flight = fly(config.clone()).await;
    let sid = flight.sandbox_id();

    let methods = flight.platform.inner.calls.methods();
    let file_at = methods.iter().position(|m| m == "send_file").expect("产物");
    let text_at = methods.iter().position(|m| m == "send_text").expect("回帖");
    assert!(
        file_at < text_at,
        "产物要先于回帖发出去，实际顺序：{methods:?}"
    );

    let sent = flight.platform.inner.sent_files()[0].clone();
    assert_eq!(&sent.data[..8], &PNG_MAGIC, "发回去的不是沙箱里那张 PNG");
    assert_eq!(sent.data, *aite_testing::PNG_1X1, "发的不是沙箱里那份字节");
    assert_eq!(sent.name, "out.png");
    assert_eq!(sent.mime, "image/png");
    assert_eq!(sent.chat_id, CHAT);
    assert_eq!(
        sent.reply_to.as_deref(),
        Some(ROOT_MSG),
        "产物要回到话题 root"
    );

    // 产物是从 **Gateway 那只**沙箱里取的。自己新 acquire 一个的话拿到的是空容器，
    // 取产物会静默跳过，回帖变成「产物 … 未找到」而任务照样 delivered。
    let fetched: Vec<(String, String)> = flight
        .sandbox
        .inner
        .calls
        .of("get_file")
        .iter()
        .map(|c| {
            (
                c.arg_str("sandbox_id").unwrap_or_default().to_string(),
                c.arg_str("path").unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        fetched,
        vec![(sid.clone(), ARTIFACT_PATH.to_string())],
        "产物应当从任务那只沙箱里取一次"
    );

    let reply = flight.platform.inner.calls.of("send_text")[0].clone();
    assert_eq!(
        reply.arg_str("text"),
        Some("月度趋势图见附件。"),
        "回帖不该带「产物未找到」之类的尾巴"
    );
    assert_eq!(reply.arg_str("reply_to"), Some(ROOT_MSG));
    assert_eq!(reply.arg("in_thread"), Some(&json!(true)));

    // evidence 里的 artifact 记的就是真发出去的那份字节，不是「打算发」的元数据
    let artifacts: Vec<_> = read_events(&config, &flight.task.id)
        .into_iter()
        .filter(|e| e.kind == EvidenceKind::Artifact)
        .filter_map(|e| e.payload)
        .collect();
    assert_eq!(artifacts.len(), 1, "发了一个产物就该有一条 artifact 证据");
    assert_eq!(artifacts[0]["title"], json!("月度趋势"));
    assert_eq!(artifacts[0]["mime"], json!("image/png"));
    assert_eq!(artifacts[0]["size"], json!(sent.data.len()));
    assert_eq!(artifacts[0]["sha256"], json!(sha256_hex(&sent.data)));

    // 库里的终态：从**新连接**读，确认真落盘了而不是只在内存里
    let delivered = read_task_from_disk(&config, &flight.task.id)
        .await
        .expect("任务在库里");
    assert_eq!(delivered.status, TaskStatus::Delivered);
    assert_eq!(delivered.result_summary, "月度趋势图见附件。");
    assert_eq!(delivered.steps as usize, SCRIPT_STEPS);
    flight.run.shutdown().await.expect("run_app 正常收场");
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

// --------------------------------------------------------------------------
// 7 evidence：链条收口
// --------------------------------------------------------------------------

/// 这条贯通路径跑完之后，证据链要能收口并自证。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_evidence_chain_closes_on_this_path() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(&tmp.path().join("data"));
    let mut flight = fly(config.clone()).await;
    let task_id = flight.task.id.clone();
    let events = read_events(&config, &task_id);

    let mut prev = GENESIS.to_string();
    for e in &events {
        let payload = e.payload.as_ref().unwrap_or_else(|| {
            panic!("seq={} 的 payload 没内联", e.seq);
        });
        assert_eq!(e.payload_hash, payload_hash_of(payload));
        assert_eq!(e.prev_hash, prev);
        assert_eq!(e.hash, chain_hash(&prev, &e.payload_hash));
        prev = e.hash.clone();
    }

    let kinds: Vec<EvidenceKind> = events.iter().map(|e| e.kind).collect();
    assert_eq!(kinds[0], EvidenceKind::TaskCreated);
    assert_eq!(kinds[1], EvidenceKind::EventReceived);
    assert_eq!(*kinds.last().expect("最后一条"), EvidenceKind::Delivered);
    assert!(
        kinds.contains(&EvidenceKind::Artifact),
        "产物发出去了却没写 artifact 证据"
    );

    // 每个 tool_call 都要有配对的 tool_result（本地 checklist_* 与 Gateway 工具一视同仁）。
    // `final` 是唯一的例外：它不回结果给模型，它的「结果」就是最后那条 delivered。
    let name_of = |p: &serde_json::Map<String, Value>| {
        p.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let call_id_of = |p: &serde_json::Map<String, Value>| {
        p.get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let calls: Vec<String> = events
        .iter()
        .filter(|e| e.kind == EvidenceKind::ToolCall)
        .filter_map(|e| e.payload.as_ref())
        .filter(|p| name_of(p) != "final")
        .map(call_id_of)
        .collect();
    let done: Vec<String> = events
        .iter()
        .filter(|e| e.kind == EvidenceKind::ToolResult)
        .filter_map(|e| e.payload.as_ref())
        .map(call_id_of)
        .collect();
    assert!(!calls.is_empty(), "一个 tool_call 证据都没有");
    assert_eq!(calls, done, "tool_call 与 tool_result 没配上");

    let finals = events
        .iter()
        .filter(|e| e.kind == EvidenceKind::ToolCall)
        .filter_map(|e| e.payload.as_ref())
        .filter(|p| name_of(p) == "final")
        .count();
    assert_eq!(finals, 1, "final 只该出一次");

    let manifest = read_manifest(&config, &task_id);
    assert_eq!(
        manifest["root_hash"],
        json!(events.last().expect("最后一条").hash)
    );
    assert_eq!(manifest["event_count"], json!(events.len()));
    assert_eq!(manifest["task_no"], json!("#A1"));
    assert_eq!(manifest["contract_version"], json!(CONTRACT_VERSION));
    assert!(
        FileEvidenceWriter::new(&config.storage.evidence_dir).verify(&task_id),
        "链自证没过"
    );

    let from_disk = read_task_from_disk(&config, &task_id)
        .await
        .expect("任务在库里");
    assert_eq!(
        from_disk.evidence_root_hash,
        manifest["root_hash"].as_str().map(str::to_string),
        "库里与盘上的 root_hash 漂了"
    );
    flight.run.shutdown().await.expect("run_app 正常收场");
}

// --------------------------------------------------------------------------
// 8 停机
// --------------------------------------------------------------------------

/// 队列空了之后停机：沙箱还回去，库关掉，盘上的东西一样不少。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_after_delivery_returns_the_sandbox() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let config = make_config(&tmp.path().join("data"));
    let mut flight = fly(config.clone()).await;
    let sid = flight.sandbox_id();

    assert_eq!(
        flight.sandbox.inner.alive(),
        Vec::<String>::new(),
        "任务收尾就该把沙箱还掉"
    );
    assert_eq!(flight.sandbox.inner.released_ids(), vec![sid.clone()]);

    flight.run.shutdown().await.expect("run_app 正常收场");

    // ── 退出之后 ────────────────────────────────────────────────
    assert!(flight.platform.inner.stopped());
    assert_eq!(
        flight.sandbox.close_calls(),
        1,
        "C-TΩ-1 的退出序列里有 sandbox.close_all()"
    );
    assert_eq!(
        flight.sandbox.inner.released_ids(),
        vec![sid],
        "已经还过的沙箱不该被重复 release"
    );
    // store.close() 调过了
    assert!(flight.app.store.get_task(&flight.task.id).await.is_err());

    // 进程没了，盘上的交付物还在 —— 这才是「收干净」而不是「擦干净」
    assert!(manifest_path(&config, &flight.task.id).is_file());
    assert_eq!(
        read_manifest(&config, &flight.task.id)["task_no"],
        json!("#A1")
    );
}
