//! T21：崩溃残行 —— 写入侧能面对「上一条命写到一半」的 events.jsonl
//! （移植 `tests/evidence/test_torn_tail.py` 15 条里的 14 条，日志那条在
//! `tests/torn_tail_log.rs`；外加 `tests/integration/test_t18_crash_recovery.py`
//! 里属于 evidence 的 3 条）。
//!
//! 口径只有一条，**换行符是记录终止符**：
//!
//! * 末尾一段没有换行符收尾 = 上一条命写到一半，那条记录从来没写完、也就从来没有效过；
//! * 中间某一行不合法 / hash 对不上 / seq 不连续 = 篡改或真损坏，**必须继续报 false**。
//!
//! 由此推出这个文件里最要紧的两组用例：`tail_is_a_whole_record` 那一组（末段其实
//! 写完了、只差换行符 → 一个字节都不许丢），和「篡改仍然 false」那一组（容错不许
//! 扩大成什么都能吞）。

mod common;

use aite_contracts::{EvidenceKind, EvidenceWriter, GENESIS};
use aite_evidence::{COUNTER_TORN_TAIL_DROPPED, COUNTER_TORN_TAIL_KEPT, FileEvidenceWriter};
use common::{events_of, lines_of, manifest_of, p, rewrite_lines, tear};
use serde_json::{Value, json};
use tempfile::TempDir;

const TASK: &str = "t-torn";

/// 一段没写完的 JSON —— 进程在写到一半没了的那一刻，文件末尾就长这样。
const HALF_LINE: &str = r#"{"task_id": "t-torn", "seq": 9, "kind": "model_ca"#;

fn writer(tmp: &TempDir) -> FileEvidenceWriter {
    FileEvidenceWriter::new(tmp.path().join("evidence"))
}

/// 重启后的新进程：tip 缓存是空的。
fn fresh(tmp: &TempDir) -> FileEvidenceWriter {
    writer(tmp)
}

async fn three_events(w: &FileEvidenceWriter) {
    for i in 0..3 {
        w.append(TASK, EvidenceKind::ModelCall, p(json!({"step": i})))
            .await
            .unwrap();
    }
}

/// 把中间那一行的 payload 改掉（行还是合法 JSON，只是 hash 对不上了）。
fn tamper_middle(w: &FileEvidenceWriter) {
    let mut lines: Vec<Value> = lines_of(w, TASK)
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    lines[1]["payload"] = json!({"有人": "动过这一行"});
    let out: Vec<String> = lines
        .iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect();
    rewrite_lines(w, TASK, &out);
}

/// 砍掉文件末尾的 N 个字节。
fn truncate_by(w: &FileEvidenceWriter, task_id: &str, n: u64) {
    let path = w.events_path(task_id);
    let size = std::fs::metadata(&path).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(size - n)
        .unwrap();
}

// ---- 残行的三种形状 --------------------------------------------------------

#[tokio::test]
async fn torn_tail_after_several_whole_records() {
    // 最常见的形状：几条完整记录之后跟着半行。收拾掉，链从它之前接着长。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    three_events(&w).await;
    tear(&w, TASK, HALF_LINE);

    let f = fresh(&tmp);
    let ev = f
        .append(TASK, EvidenceKind::Failed, p(json!({"why": "crash"})))
        .await
        .unwrap();

    assert_eq!(ev.seq, 3, "半行不占号");
    assert!(f.verify(TASK));
    assert_eq!(
        events_of(&w, TASK)
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
}

#[tokio::test]
async fn events_file_that_is_only_a_torn_line() {
    // 整个文件就是半行 —— 崩在第一条证据上。截完是空文件，新链从 seq 0 起。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    tear(&w, TASK, HALF_LINE);
    assert_eq!(
        std::fs::read(w.events_path(TASK)).unwrap(),
        HALF_LINE.as_bytes()
    );

    let ev = w
        .append(TASK, EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();

    assert_eq!((ev.seq, ev.prev_hash.as_str()), (0, GENESIS));
    assert_eq!(w.counter(COUNTER_TORN_TAIL_DROPPED), 1);
    assert!(w.verify(TASK));
    assert_eq!(lines_of(&w, TASK).len(), 1);
}

#[tokio::test]
async fn empty_events_file_is_not_a_torn_tail() {
    // 0 字节的文件没有残行 —— 别把「还没写过」当成崩溃现场。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    let path = w.events_path(TASK);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"").unwrap();

    let ev = w
        .append(TASK, EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();

    assert_eq!(ev.seq, 0);
    assert!(w.counters().is_empty(), "什么都没收拾");
    assert!(w.verify(TASK));
}

#[tokio::test]
async fn torn_tail_cut_inside_a_multibyte_character() {
    // 残行截在一个 UTF-8 中文字符中间。按字符读全文的话会先炸在 decode 上，
    // 那就等于没修 —— 所以收拾必须按字节切。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    w.append(TASK, EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();
    {
        use std::io::Write;
        let raw = r#"{"task_id": "x", "payload": {"b": "文"#.as_bytes();
        let mut fh = std::fs::OpenOptions::new()
            .append(true)
            .open(w.events_path(TASK))
            .unwrap();
        fh.write_all(&raw[..raw.len() - 1]).unwrap(); // 「文」被砍掉一截
    }

    let ev = w
        .append(TASK, EvidenceKind::Failed, p(json!({"why": "crash"})))
        .await
        .unwrap();

    assert_eq!(ev.seq, 1);
    assert_eq!(w.counter(COUNTER_TORN_TAIL_DROPPED), 1);
    assert!(w.verify(TASK));
}

// ---- 只差一个换行符：一个字节都不许丢 --------------------------------------

#[tokio::test]
async fn a_record_missing_only_its_newline_is_kept() {
    // 末段其实整条写完了，只差那个换行符 —— 补上，不许当残行截掉。
    // 收拾的原则是「只丢弃 verify 本来就会拒绝的东西」：这个形状 verify 本来就认它。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    w.append(TASK, EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();
    let second = w
        .append(TASK, EvidenceKind::ModelCall, p(json!({"b": "文"})))
        .await
        .unwrap();

    truncate_by(&w, TASK, 1); // 只砍掉末尾的换行符
    assert!(w.verify(TASK), "前提：这个形状 verify 本来就是 true");

    let third = fresh(&tmp)
        .append(TASK, EvidenceKind::Delivered, p(json!({"c": 3})))
        .await
        .unwrap();

    assert_eq!(third.seq, 2);
    assert_eq!(third.prev_hash, second.hash, "第 2 条还在链上");
    assert!(w.verify(TASK), "收拾前后答案不变");
    assert_eq!(lines_of(&w, TASK).len(), 3);
}

#[tokio::test]
async fn a_whole_record_missing_its_newline_is_counted_separately() {
    // 补换行符和截残行是两件事，留痕也分开计 —— 排障时得分得清丢没丢东西。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    w.append(TASK, EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();
    truncate_by(&w, TASK, 1);

    w.append(TASK, EvidenceKind::Delivered, p(json!({"c": 3})))
        .await
        .unwrap();

    assert_eq!(w.counter(COUNTER_TORN_TAIL_KEPT), 1);
    assert_eq!(w.counter(COUNTER_TORN_TAIL_DROPPED), 0);
}

#[tokio::test]
async fn a_tail_that_parses_but_does_not_fit_the_chain_is_dropped() {
    // 末段是合法的 EvidenceEvent，但 hash 接不住前面 —— 那不是「写完了」，是垃圾。
    // 只看「解析得了吗」不够：判据必须是 verify 那把尺子。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    let first = w
        .append(TASK, EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();

    let mut forged: Value = serde_json::from_str(&lines_of(&w, TASK)[0]).unwrap();
    forged["seq"] = json!(1);
    forged["prev_hash"] = json!(first.hash);
    forged["hash"] = json!("0".repeat(64));
    tear(&w, TASK, &serde_json::to_string(&forged).unwrap());

    let ev = w
        .append(TASK, EvidenceKind::Failed, p(json!({"why": "crash"})))
        .await
        .unwrap();

    assert_eq!(ev.seq, 1, "编的那条被截掉了");
    assert_eq!(w.counter(COUNTER_TORN_TAIL_DROPPED), 1);
    assert!(w.verify(TASK));
}

// ---- 篡改检测：容错不许扩大成什么都能吞 ------------------------------------

#[tokio::test]
async fn a_tampered_middle_line_still_fails_verify() {
    // 中间行被改坏 → verify 永远 false。这是这套证据链存在的理由。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    three_events(&w).await;
    tamper_middle(&w);

    assert!(!w.verify(TASK));
    assert!(!fresh(&tmp).verify(TASK));
}

#[tokio::test]
async fn a_torn_tail_does_not_launder_a_tampered_middle_line() {
    // 最该防的一条 —— 「崩溃恢复」不许变成把中间的篡改一起洗白。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    three_events(&w).await;
    tamper_middle(&w);
    tear(&w, TASK, HALF_LINE);

    let f = fresh(&tmp);
    let ev = f
        .append(TASK, EvidenceKind::Failed, p(json!({"why": "crash"})))
        .await
        .unwrap();

    assert_eq!(ev.seq, 3, "收拾只碰末尾那半行");
    assert_eq!(f.counter(COUNTER_TORN_TAIL_DROPPED), 1);
    assert!(!f.verify(TASK), "中间那一行还是坏的");
}

#[tokio::test]
async fn append_refuses_to_extend_a_tampered_chain() {
    // 中间行不合法时 append / finalize 仍然报错：不许「无脑容错」——
    // 在一条已经不可信的链上继续追加，等于给篡改盖章。
    // 这里的文件是以换行符正常收尾的 —— 没有残行，坏的就是中间那一行。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    three_events(&w).await;
    let mut lines = lines_of(&w, TASK);
    lines[1] = r#"{"task_id": "t-torn", "kind": 坏掉的"#.to_string();
    rewrite_lines(&w, TASK, &lines);

    let err = fresh(&tmp)
        .append(TASK, EvidenceKind::Failed, p(json!({"why": "crash"})))
        .await
        .expect_err("必须报错");
    assert!(
        matches!(err, aite_contracts::EvidenceError::Corrupt { .. }),
        "{err:?}"
    );

    let err = fresh(&tmp)
        .finalize(TASK, Default::default())
        .await
        .expect_err("必须报错");
    assert!(
        matches!(err, aite_contracts::EvidenceError::Corrupt { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn dropping_a_middle_line_still_fails_verify() {
    // 整行被删（seq 断了）也不许被当成「残行」收拾掉。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    three_events(&w).await;
    let lines = lines_of(&w, TASK);
    rewrite_lines(&w, TASK, &[lines[0].clone(), lines[2].clone()]);

    let f = fresh(&tmp);
    f.append(TASK, EvidenceKind::Failed, p(json!({"why": "crash"})))
        .await
        .unwrap();

    assert!(f.counters().is_empty(), "文件以换行符收尾，没残行");
    assert!(!f.verify(TASK));
}

// ---- finalize -------------------------------------------------------------

#[tokio::test]
async fn finalize_after_a_torn_tail_writes_a_manifest() {
    // 崩溃后第一个被调到的常常是 finalize —— 它也要走收拾，而不是报错。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    let mut last = None;
    for i in 0..3 {
        last = Some(
            w.append(TASK, EvidenceKind::ModelCall, p(json!({"step": i})))
                .await
                .unwrap(),
        );
    }
    tear(&w, TASK, HALF_LINE);

    let f = fresh(&tmp);
    let root = f
        .finalize(TASK, p(json!({"session_id": "s-1", "task_no": "#A9"})))
        .await
        .unwrap();

    assert_eq!(root, last.unwrap().hash, "半行不改 root_hash");
    let m = manifest_of(&f, TASK);
    assert_eq!(m["event_count"], json!(3), "也不算进 event_count");
    assert_eq!(m["root_hash"], json!(root));
    assert!(f.verify(TASK));
}

#[tokio::test]
async fn finalize_on_a_file_that_is_only_a_torn_line() {
    // 一条完整证据都没有、只有半行：root_hash 是 GENESIS，不是错误。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    tear(&w, TASK, HALF_LINE);

    assert_eq!(w.finalize(TASK, Default::default()).await.unwrap(), GENESIS);
    assert_eq!(manifest_of(&w, TASK)["event_count"], json!(0));
    assert_eq!(std::fs::read(w.events_path(TASK)).unwrap(), b"");
}

// ---- 成本 -----------------------------------------------------------------

#[tokio::test]
async fn a_healthy_file_is_never_read_whole_on_append() {
    // 健康文件上 append 只探一个字节，不读全文 —— 这条路每条证据都会走。
    // 钉住成本：哪天有人把 O(1) 的探测换成「每次读一遍再判断」，这里会红。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    for i in 0..5 {
        w.append(TASK, EvidenceKind::ModelCall, p(json!({"step": i})))
            .await
            .unwrap();
    }

    assert_eq!(w.whole_file_reads(), 0);
    assert!(w.counters().is_empty());
}

// ---- T18 里属于 evidence 的三条 -------------------------------------------

/// 往 events.jsonl 末尾写半行 JSON —— 进程在 write 中途没的那一刻的样子。
fn tear_last_line(w: &FileEvidenceWriter, task_id: &str) {
    tear(
        w,
        task_id,
        &format!(r#"{{"task_id": "{task_id}", "seq": 2, "kind": "model_ca"#),
    );
}

#[tokio::test]
async fn verify_rejects_a_torn_last_line() {
    // 残行必须让 verify 报 false。「完整篡改」验过，这里验「写了一半」。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    w.append("t-torn2", EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();
    w.append("t-torn2", EvidenceKind::ModelCall, p(json!({"b": 2})))
        .await
        .unwrap();
    assert!(w.verify("t-torn2"));

    tear_last_line(&w, "t-torn2");
    assert!(!w.verify("t-torn2"));
    assert!(!writer(&tmp).verify("t-torn2"), "换实例也一样");
}

#[tokio::test]
async fn append_after_a_torn_line_recovers_on_a_fresh_writer() {
    // 崩溃留下残行后，重启的新进程能接着写 —— §3.3 的收尾链不再自锁。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    w.append("t-cold", EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();
    tear_last_line(&w, "t-cold");

    let f = fresh(&tmp); // 重启后的新进程：tip 缓存是空的
    let failed = f
        .append("t-cold", EvidenceKind::Failed, p(json!({"why": "crash"})))
        .await
        .unwrap();
    assert_eq!(failed.seq, 1, "残行没写完，不占号");
    assert_eq!(f.counter(COUNTER_TORN_TAIL_DROPPED), 1, "收拾这件事留了痕");
    assert!(f.verify("t-cold"), "链是通的，不是「不炸但坏着」");

    // finalize 走的是同一条收拾路径（它也可能是崩溃后第一个被调到的）
    let other = fresh(&tmp);
    assert_eq!(
        other.finalize("t-cold", Default::default()).await.unwrap(),
        failed.hash
    );
    assert_eq!(
        manifest_of(&other, "t-cold")["event_count"],
        json!(2),
        "那半行从来不是一条事件"
    );
}

#[tokio::test]
async fn append_after_a_torn_line_keeps_the_chain_on_a_hot_writer() {
    // tip 还热时也不再坏链 —— 磁盘满那一路：write 只落一半，进程还活着。
    // 挡法不依赖 tip 是冷是热：append 每次都真去看文件末尾那一个字节。
    let tmp = TempDir::new().unwrap();
    let w = writer(&tmp);
    w.append("t-hot", EvidenceKind::TaskCreated, p(json!({"a": 1})))
        .await
        .unwrap();
    tear_last_line(&w, "t-hot");

    let second = w
        .append("t-hot", EvidenceKind::ModelCall, p(json!({"b": 2})))
        .await
        .unwrap();
    assert_eq!(second.seq, 1);
    assert_eq!(w.counter(COUNTER_TORN_TAIL_DROPPED), 1);
    assert!(w.verify("t-hot"));

    let lines = lines_of(&w, "t-hot");
    assert_eq!(lines.len(), 2);
    assert!(
        lines.iter().all(|l| l.matches(r#""task_id""#).count() == 1),
        "一行一条，不再挤"
    );
}
